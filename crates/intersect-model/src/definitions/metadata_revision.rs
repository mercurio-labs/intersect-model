//! Retained framing of the existing (revision, raw bounds, warnings) JSON tuple.
//! Inputs belong to one sealed unpublished compiler output. No observer is retained.
use super::{Blocks, ObservedDigestWriter};
use crate::definitions::{
    bundle::{SourceId, SourceSpan},
    ecore::{ErrorKind, ImportError, ParserObservation},
    generic_resolution::Warning,
};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, btree_map};

type RawBounds = BTreeMap<String, BTreeMap<String, Vec<String>>>;
#[derive(Clone, Copy)]
enum Phase {
    Prefix,
    Class,
    Feature,
    Bound,
    Warning,
    Complete,
}
pub(in crate::definitions::compiler) struct MetadataValues {
    pub(in crate::definitions::compiler) revision: String,
    pub(in crate::definitions::compiler) raw_reference_bounds: RawBounds,
    pub(in crate::definitions::compiler) runtime_warnings: Vec<Warning>,
}
struct MetadataDigest<'input> {
    digest: Sha256,
    bytes: [u8; 256],
    used: usize,
    revision: &'input str,
    classes: btree_map::Iter<'input, String, BTreeMap<String, Vec<String>>>,
    fields: Option<btree_map::Iter<'input, String, Vec<String>>>,
    rows: Option<&'input [String]>,
    warnings: &'input [Warning],
    have_class: bool,
    have_feature: bool,
    index: usize,
    phase: Phase,
    failed: bool,
}
self_cell::self_cell!(
    struct FrozenMetadata {
        owner: MetadataValues,
        #[covariant]
        dependent: MetadataDigest,
    }
);
pub(in crate::definitions::compiler) struct MetadataRevisionPreparation {
    holder: FrozenMetadata,
}
const STATE_BYTES: usize = 1024;
const _: () = assert!(
    std::mem::size_of::<MetadataValues>() + std::mem::size_of::<MetadataDigest<'static>>() + 128
        <= STATE_BYTES
);
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
impl MetadataRevisionPreparation {
    pub(in crate::definitions::compiler) fn new(
        revision: String,
        raw_reference_bounds: RawBounds,
        runtime_warnings: Vec<Warning>,
        source: &SourceId,
        observation: &ParserObservation<'_, '_>,
    ) -> Result<Self, ImportError> {
        // Move already admitted metadata into a sealed owner. Iterators borrow
        // canonical keys and slices; there are no copied name cursors or payloads.
        observation.reserve(STATE_BYTES, source)?;
        let holder = FrozenMetadata::new(
            MetadataValues {
                revision,
                raw_reference_bounds,
                runtime_warnings,
            },
            |input| MetadataDigest::new(input),
        );
        Ok(Self { holder })
    }
    pub(in crate::definitions::compiler) fn advance(
        &mut self,
        source: &SourceId,
        observation: &ParserObservation<'_, '_>,
    ) -> Result<bool, ImportError> {
        self.holder
            .with_dependent_mut(|_, digest| digest.advance(source, observation))
    }
    pub(in crate::definitions::compiler) fn finish(self) -> Result<MetadataValues, String> {
        let revision = self.holder.with_dependent(|_, digest| digest.finish())?;
        let mut values = self.holder.into_owner();
        values.revision = revision;
        Ok(values)
    }
}
impl<'input> MetadataDigest<'input> {
    fn new(input: &'input MetadataValues) -> Self {
        Self {
            digest: Sha256::new(),
            bytes: [0; 256],
            used: 0,
            revision: &input.revision,
            classes: input.raw_reference_bounds.iter(),
            fields: None,
            rows: None,
            warnings: &input.runtime_warnings,
            have_class: false,
            have_feature: false,
            index: 0,
            phase: Phase::Prefix,
            failed: false,
        }
    }
    /// One tuple prefix, class/feature boundary, bound string, warning or close.
    /// A single string or warning still uses synchronous authoritative Serde.
    fn advance(
        &mut self,
        source: &SourceId,
        observation: &ParserObservation<'_, '_>,
    ) -> Result<bool, ImportError> {
        if self.failed {
            return Err(error(source, "Metadata revision preparation has failed"));
        }
        let result = self.advance_inner(source, observation);
        if result.is_err() {
            self.failed = true;
        }
        result
    }
    fn advance_inner(
        &mut self,
        source: &SourceId,
        observation: &ParserObservation<'_, '_>,
    ) -> Result<bool, ImportError> {
        observation.work(0, source)?;
        if matches!(self.phase, Phase::Complete) {
            return Ok(true);
        }
        observation.work(1, source)?;
        enum Part<'a> {
            Prefix(&'a str),
            Class(&'a str, bool),
            Feature(&'a str, bool),
            Bound(&'a str, bool),
            EndFeature,
            EndClass,
            Warnings,
            Warning(&'a Warning, bool),
            End,
        }
        let part = match self.phase {
            Phase::Prefix => {
                self.phase = Phase::Class;
                Part::Prefix(self.revision)
            }
            Phase::Class => {
                if let Some((key, fields)) = self.classes.next() {
                    let comma = self.have_class;
                    self.have_class = true;
                    self.have_feature = false;
                    self.fields = Some(fields.iter());
                    self.phase = Phase::Feature;
                    Part::Class(key, comma)
                } else {
                    self.index = 0;
                    self.phase = Phase::Warning;
                    Part::Warnings
                }
            }
            Phase::Feature => {
                let fields = self
                    .fields
                    .as_mut()
                    .ok_or_else(|| error(source, "Missing metadata feature iterator"))?;
                if let Some((key, rows)) = fields.next() {
                    let comma = self.have_feature;
                    self.have_feature = true;
                    self.rows = Some(rows.as_slice());
                    self.index = 0;
                    self.phase = Phase::Bound;
                    Part::Feature(key, comma)
                } else {
                    self.phase = Phase::Class;
                    Part::EndClass
                }
            }
            Phase::Bound => {
                let rows = self
                    .rows
                    .ok_or_else(|| error(source, "Missing metadata bound slice"))?;
                if let Some(row) = rows.get(self.index) {
                    let comma = self.index != 0;
                    self.index += 1;
                    Part::Bound(row, comma)
                } else {
                    self.phase = Phase::Feature;
                    Part::EndFeature
                }
            }
            Phase::Warning => {
                if let Some(warning) = self.warnings.get(self.index) {
                    let comma = self.index != 0;
                    self.index += 1;
                    Part::Warning(warning, comma)
                } else {
                    self.phase = Phase::Complete;
                    Part::End
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
                    blocks.write_all(b":{").map_err(serde_json::Error::io)?;
                }
                Part::Feature(key, comma) => {
                    if comma {
                        blocks.write_all(b",").map_err(serde_json::Error::io)?;
                    }
                    serde_json::to_writer(&mut blocks, key)?;
                    blocks.write_all(b":[").map_err(serde_json::Error::io)?;
                }
                Part::Bound(value, comma) => {
                    if comma {
                        blocks.write_all(b",").map_err(serde_json::Error::io)?;
                    }
                    serde_json::to_writer(&mut blocks, value)?;
                }
                Part::EndFeature => blocks.write_all(b"]").map_err(serde_json::Error::io)?,
                Part::EndClass => blocks.write_all(b"}").map_err(serde_json::Error::io)?,
                Part::Warnings => blocks.write_all(b"},[").map_err(serde_json::Error::io)?,
                Part::Warning(warning, comma) => {
                    if comma {
                        blocks.write_all(b",").map_err(serde_json::Error::io)?;
                    }
                    serde_json::to_writer(&mut blocks, warning)?;
                }
                Part::End => {
                    blocks.write_all(b"]]").map_err(serde_json::Error::io)?;
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
    fn finish(&self) -> Result<String, String> {
        if self.failed || !matches!(self.phase, Phase::Complete) || self.used != 0 {
            return Err("Metadata revision preparation is incomplete or failed".into());
        }
        Ok(format!("{:x}", self.digest.clone().finalize()))
    }
}

#[cfg(test)]
#[path = "metadata_revision_tests.rs"]
mod tests;
