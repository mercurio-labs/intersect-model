//! Incremental framing over Serde's authoritative key/value encodings.
//! The caller owns and seals the complete map and prior revision for the attempt.
use super::{Blocks, ObservedDigestWriter};
use crate::definitions::{
    bundle::{SourceId, SourceSpan},
    ecore::{ErrorKind, ImportError, ParserObservation},
};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::ops::Bound::{Excluded, Unbounded};
#[derive(Clone, Copy)]
enum Phase {
    Prefix,
    Class,
    Row,
    Complete,
}
pub(in crate::definitions::compiler) struct MapRevisionPreparation {
    digest: Sha256,
    bytes: [u8; 256],
    used: usize,
    cursor: Option<String>,
    row: usize,
    phase: Phase,
    failed: bool,
}
const STATE_BYTES: usize = 512;
const _: () = assert!(std::mem::size_of::<MapRevisionPreparation>() <= STATE_BYTES);
fn error(source: &SourceId, message: impl Into<String>) -> ImportError {
    ImportError::new(
        ErrorKind::InvalidDefinition,
        &SourceSpan {
            source: source.clone(),
            start: 0,
            end: 0,
        },
        message,
    )
}
impl MapRevisionPreparation {
    pub(in crate::definitions::compiler) fn new(
        source: &SourceId,
        observation: &ParserObservation<'_, '_>,
    ) -> Result<Self, ImportError> {
        observation.reserve(STATE_BYTES, source)?;
        Ok(Self {
            digest: Sha256::new(),
            bytes: [0; 256],
            used: 0,
            cursor: None,
            row: 0,
            phase: Phase::Prefix,
            failed: false,
        })
    }
    /// One prefix, class header, complete domain row, class close or map close.
    /// A row still uses synchronous Serde; this is not a universal work quantum.
    pub(in crate::definitions::compiler) fn advance<T: serde::Serialize>(
        &mut self,
        revision: &str,
        map: &BTreeMap<String, Vec<T>>,
        source: &SourceId,
        observation: &ParserObservation<'_, '_>,
    ) -> Result<bool, ImportError> {
        if self.failed {
            return Err(error(source, "Revision preparation has failed"));
        }
        let result = self.advance_inner(revision, map, source, observation);
        if result.is_err() {
            self.failed = true;
        }
        result
    }
    fn advance_inner<T: serde::Serialize>(
        &mut self,
        revision: &str,
        map: &BTreeMap<String, Vec<T>>,
        source: &SourceId,
        observation: &ParserObservation<'_, '_>,
    ) -> Result<bool, ImportError> {
        observation.work(0, source)?;
        if matches!(self.phase, Phase::Complete) {
            return Ok(true);
        }
        enum Part<'a, T> {
            Prefix(&'a str),
            Class(&'a str, bool),
            Row(&'a T, bool),
            EndClass,
            EndMap,
        }
        let part = match self.phase {
            Phase::Prefix => {
                self.phase = Phase::Class;
                Part::Prefix(revision)
            }
            Phase::Class => {
                observation.work(1, source)?;
                let next = match self.cursor.as_deref() {
                    Some(cursor) => map.range::<str, _>((Excluded(cursor), Unbounded)).next(),
                    None => map.iter().next(),
                };
                if let Some((key, _)) = next {
                    let comma = self.cursor.is_some();
                    observation.string(key.len(), source)?;
                    self.cursor = Some(key.clone());
                    self.row = 0;
                    self.phase = Phase::Row;
                    Part::Class(key, comma)
                } else {
                    self.phase = Phase::Complete;
                    Part::EndMap
                }
            }
            Phase::Row => {
                observation.work(1, source)?;
                let key = self
                    .cursor
                    .as_deref()
                    .ok_or_else(|| error(source, "Missing revision class cursor"))?;
                let rows = map
                    .get(key)
                    .ok_or_else(|| error(source, "Revision input class disappeared"))?;
                if let Some(row) = rows.get(self.row) {
                    let comma = self.row != 0;
                    self.row += 1;
                    Part::Row(row, comma)
                } else {
                    self.phase = Phase::Class;
                    Part::EndClass
                }
            }
            Phase::Complete => return Ok(true),
        };
        let mut writer = ObservedDigestWriter {
            digest: std::mem::replace(&mut self.digest, Sha256::new()),
            source,
            observation,
        };
        let mut blocks = Blocks {
            writer: &mut writer,
            bytes: std::mem::replace(&mut self.bytes, [0; 256]),
            used: self.used,
        };
        let serialized = (|| -> Result<(), serde_json::Error> {
            use std::io::Write;
            match part {
                Part::Prefix(revision) => {
                    blocks.write_all(b"[").map_err(serde_json::Error::io)?;
                    serde_json::to_writer(&mut blocks, revision)?;
                    blocks.write_all(b",{").map_err(serde_json::Error::io)?;
                }
                Part::Class(key, comma) => {
                    if comma {
                        blocks.write_all(b",").map_err(serde_json::Error::io)?;
                    }
                    serde_json::to_writer(&mut blocks, key)?;
                    blocks.write_all(b":[").map_err(serde_json::Error::io)?;
                }
                Part::Row(row, comma) => {
                    if comma {
                        blocks.write_all(b",").map_err(serde_json::Error::io)?;
                    }
                    serde_json::to_writer(&mut blocks, row)?;
                }
                Part::EndClass => {
                    blocks.write_all(b"]").map_err(serde_json::Error::io)?;
                }
                Part::EndMap => {
                    blocks.write_all(b"}]").map_err(serde_json::Error::io)?;
                    blocks.flush().map_err(serde_json::Error::io)?;
                }
            }
            Ok(())
        })();
        self.used = blocks.used;
        self.bytes = blocks.bytes;
        self.digest = writer.digest;
        serialized.map_err(|e| error(source, e.to_string()))?;
        let complete = matches!(self.phase, Phase::Complete);
        if complete {
            observation.work(1, source)?;
            observation.string(64, source)?;
        }
        Ok(complete)
    }
    pub(in crate::definitions::compiler) fn finish(self) -> Result<String, String> {
        if self.failed || !matches!(self.phase, Phase::Complete) || self.used != 0 {
            return Err("Revision preparation is incomplete or failed".into());
        }
        Ok(format!("{:x}", self.digest.finalize()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::definitions::{
        bundle::{DefinitionSource, SourceKind},
        conversion::{InvocationContext, InvocationObserver, ProviderId},
    };
    use std::{cell::Cell, rc::Rc};
    struct Meter {
        work: Cell<usize>,
        bytes: Cell<usize>,
        calls: Cell<usize>,
        stop: usize,
        work_limit: usize,
        byte_limit: usize,
    }
    impl Meter {
        fn new(stop: usize, work_limit: usize, byte_limit: usize) -> Rc<Self> {
            Rc::new(Self {
                work: Cell::new(0),
                bytes: Cell::new(0),
                calls: Cell::new(0),
                stop,
                work_limit,
                byte_limit,
            })
        }
        fn touch(&self) -> Result<(), String> {
            let call = self.calls.get();
            self.calls.set(call + 1);
            if call == self.stop {
                Err("map revision cancelled".into())
            } else {
                Ok(())
            }
        }
    }
    impl InvocationObserver for Meter {
        fn before_invoke(&self, _: &ProviderId) -> Result<(), String> {
            Err("Unexpected provider".into())
        }
        fn work(&self, units: usize) -> Result<(), String> {
            self.touch()?;
            let next = self.work.get() + units;
            if next > self.work_limit {
                return Err("map revision work denied".into());
            }
            self.work.set(next);
            Ok(())
        }
        fn retain_storage(&self, bytes: usize) -> Result<(), String> {
            self.touch()?;
            let next = self.bytes.get() + bytes;
            if next > self.byte_limit {
                return Err("map revision storage denied".into());
            }
            self.bytes.set(next);
            Ok(())
        }
    }
    #[test]
    fn incremental_map_revision_matches_exact_json_and_each_interruption_without_observer_retention()
     {
        let source = DefinitionSource::new("memory:map-hash", SourceKind::Ecore, "<a/>");
        for map in [
            BTreeMap::new(),
            BTreeMap::from([
                ("empty".into(), vec![]),
                (
                    "quoted\"é".into(),
                    vec![
                        serde_json::json!({"line":"é\n\\", "values":[true,null,-17]}),
                        serde_json::json!("x".repeat(4096)),
                    ],
                ),
                ("z".into(), vec![serde_json::json!([1, 2, 3])]),
            ]),
        ] {
            let revision = "prior\"é\n";
            let encoded = serde_json::to_vec(&(revision, &map)).unwrap();
            let expected = format!("{:x}", Sha256::digest(&encoded));
            let run = |meter: Rc<Meter>| {
                let operation = InvocationContext::new(meter);
                let observation = ParserObservation::new(&operation);
                let result = (|| {
                    let mut pending = MapRevisionPreparation::new(&source.id, &observation)?;
                    while !pending.advance(revision, &map, &source.id, &observation)? {}
                    pending.finish().map_err(|e| error(&source.id, e))
                })();
                observation.finish(result)
            };
            let full = Meter::new(usize::MAX, usize::MAX, usize::MAX);
            assert_eq!(run(full.clone()).unwrap(), expected);
            for call in 0..full.calls.get() {
                assert!(run(Meter::new(call, usize::MAX, usize::MAX)).is_err());
                assert_eq!(
                    run(Meter::new(usize::MAX, usize::MAX, usize::MAX)).unwrap(),
                    expected
                );
            }
            for (work, bytes, success) in [
                (full.work.get() - 1, full.bytes.get(), false),
                (full.work.get(), full.bytes.get() - 1, false),
                (full.work.get(), full.bytes.get(), true),
            ] {
                let result = run(Meter::new(usize::MAX, work, bytes));
                assert_eq!(result.is_ok(), success);
                if success {
                    assert_eq!(result.unwrap(), expected);
                }
            }
            let operation = InvocationContext::default();
            let mut pending =
                MapRevisionPreparation::new(&source.id, &ParserObservation::new(&operation))
                    .unwrap();
            loop {
                let meter = Meter::new(usize::MAX, usize::MAX, usize::MAX);
                let weak = Rc::downgrade(&meter);
                let done = {
                    let operation = InvocationContext::new(meter.clone());
                    pending
                        .advance(
                            revision,
                            &map,
                            &source.id,
                            &ParserObservation::new(&operation),
                        )
                        .unwrap()
                };
                drop(meter);
                assert!(weak.upgrade().is_none());
                if done {
                    break;
                }
            }
            assert_eq!(pending.finish().unwrap(), expected);
            let mut pending =
                MapRevisionPreparation::new(&source.id, &ParserObservation::new(&operation))
                    .unwrap();
            let cancelled = InvocationContext::new(Meter::new(0, usize::MAX, usize::MAX));
            assert!(
                pending
                    .advance(
                        revision,
                        &map,
                        &source.id,
                        &ParserObservation::new(&cancelled)
                    )
                    .is_err()
            );
            assert!(
                pending
                    .advance(
                        revision,
                        &map,
                        &source.id,
                        &ParserObservation::new(&operation)
                    )
                    .is_err()
            );
            assert!(pending.finish().is_err());
            assert!(
                MapRevisionPreparation::new(&source.id, &ParserObservation::new(&operation))
                    .unwrap()
                    .finish()
                    .is_err()
            );
            assert_eq!(serde_json::to_vec(&(revision, &map)).unwrap(), encoded);
        }
    }
}
