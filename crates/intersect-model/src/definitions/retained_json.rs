//! Shared retained JSON ownership and exact measurement.
//!
//! Interpreter preparation and final value transport use this shared engine.
//! Intermediate instruction sizing and host JSON preparation remain explicit
//! boundaries. Callers must admit each cursor/root owner and allocation request
//! before construction. A
//! successful traversal is not nominal/type/source authority. Map insertion/comparison, numeric formatting and
//! allocator reservations remain atomic; foreign callbacks are not invoked here.
use serde_json::{Map, Value};
use std::mem::size_of;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Allocation {
    Frames {
        old_slots: usize,
        new_slots: usize,
        element_bytes: usize,
    },
    String {
        bytes: usize,
        key: bool,
    },
    Array {
        elements: usize,
        element_bytes: usize,
    },
    Object {
        entries: usize,
        key_value_bytes: usize,
    },
    // Retain the probe variant/layout even when only internal consumers are built.
    #[cfg_attr(not(feature = "allocation-profile"), allow(dead_code))]
    Number {
        encoded_bytes: usize,
    },
}
/// An in-process owner-policy adapter, never deserialized authority. A caller
/// must account for simultaneous old/new reservation ownership, keys, map nodes,
/// root holders and publication separately. No credit is created by this trait.
pub trait Admission {
    fn allocate(&mut self, request: Allocation) -> Result<(), String>;
}
#[cfg(feature = "allocation-profile")]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Measure,
    Copy,
    /// Resource inspection retains the original all-keys-first/reverse-child policy.
    Inspect,
}
/// Source-authoritative policy receives borrowed nodes before ownership/reservation.
/// The remaining sibling count matches the original pending-stack admission.
pub trait Inspection {
    fn node(&mut self, value: &Value, depth: usize, pending: usize) -> Result<(), String>;
    fn key(&mut self, key: &str) -> Result<(), String>;
    /// Called after every key is inspected and before child-frontier growth.
    fn object_keys_finished(&mut self) -> Result<(), String> {
        Ok(())
    }
}
#[cfg(feature = "allocation-profile")]
struct Ignore;
#[cfg(feature = "allocation-profile")]
impl Inspection for Ignore {
    fn node(&mut self, _: &Value, _: usize, _: usize) -> Result<(), String> {
        Ok(())
    }
    fn key(&mut self, _: &str) -> Result<(), String> {
        Ok(())
    }
}

// Shared primitive: original String transport and general JSON frontier both
// use this exact reservation/chunk implementation, with immutable source owners.
pub(crate) fn reserve_string(source: &str, error: &str) -> Result<String, String> {
    let mut copied = String::new();
    copied
        .try_reserve_exact(source.len())
        .map_err(|_| error.to_string())?;
    Ok(copied)
}
pub(crate) fn advance_string_copy(
    copied: &mut String,
    offset: &mut usize,
    length: usize,
    source: &str,
) -> Result<bool, String> {
    if source.len() != length || copied.len() != *offset {
        return Err("Retained String transport source changed".into());
    }
    let end = crate::definitions::bundle::input_chunk_end(source, *offset);
    let chunk = source
        .get(*offset..end)
        .ok_or("Invalid retained String transport boundary")?;
    copied.push_str(chunk);
    *offset = end;
    Ok(end == length)
}
#[cfg(feature = "allocation-profile")]
enum CursorState<'a> {
    Start(&'a Value, Mode),
    Measure(crate::definitions::retained_json_frontier::MeasureJob<'a>),
    Copy(crate::definitions::retained_json_frontier::CopyJob<'a>),
    Inspect(ResourceCursor<'a>),
    Done { bytes: usize, value: Option<Value> },
    Failed,
}
#[cfg(feature = "allocation-profile")]
pub struct Cursor<'a> {
    state: CursorState<'a>,
}
#[cfg(feature = "allocation-profile")]
impl<'a> Cursor<'a> {
    pub fn new(value: &'a Value, mode: Mode) -> Self {
        Self {
            state: CursorState::Start(value, mode),
        }
    }
    pub fn layouts() -> (usize, usize) {
        (
            size_of::<Self>(),
            crate::definitions::retained_json_frontier::CopyJob::layouts()[4],
        )
    }
    pub fn encoded_bytes(&self) -> usize {
        match &self.state {
            CursorState::Measure(v) => v.encoded_bytes(),
            CursorState::Copy(v) => v.encoded_bytes(),
            CursorState::Done { bytes, .. } => *bytes,
            _ => 0,
        }
    }
    pub fn take_value(&mut self) -> Result<Value, String> {
        match &mut self.state {
            CursorState::Done { value, .. } => value
                .take()
                .ok_or_else(|| "JSON output was already taken".into()),
            _ => Err("JSON copy is not complete".into()),
        }
    }
    pub fn advance(&mut self, admission: &mut impl Admission) -> Result<bool, String> {
        self.advance_observed(admission, &mut Ignore)
    }
    pub fn advance_observed(
        &mut self,
        admission: &mut impl Admission,
        inspection: &mut impl Inspection,
    ) -> Result<bool, String> {
        let r = self.step(admission, inspection);
        if r.is_err() {
            self.state = CursorState::Failed;
        }
        r
    }
    fn step(
        &mut self,
        admission: &mut impl Admission,
        inspection: &mut impl Inspection,
    ) -> Result<bool, String> {
        let current = std::mem::replace(&mut self.state, CursorState::Failed);
        match current {
            CursorState::Start(value, mode) => {
                self.state = match mode {
                    Mode::Copy => CursorState::Copy(
                        crate::definitions::retained_json_frontier::CopyJob::new(value, admission)?,
                    ),
                    Mode::Measure => CursorState::Measure(
                        crate::definitions::retained_json_frontier::MeasureJob::new(value),
                    ),
                    Mode::Inspect => CursorState::Inspect(ResourceCursor::new(value)),
                };
                Ok(false)
            }
            CursorState::Measure(mut job) => {
                if job.advance(admission)? {
                    self.state = CursorState::Done {
                        bytes: job.encoded_bytes(),
                        value: None,
                    };
                    Ok(true)
                } else {
                    self.state = CursorState::Measure(job);
                    Ok(false)
                }
            }
            CursorState::Copy(mut job) => {
                if job.advance(admission)? {
                    let bytes = job.encoded_bytes();
                    self.state = CursorState::Done {
                        bytes,
                        value: Some(job.finish()?),
                    };
                    Ok(true)
                } else {
                    self.state = CursorState::Copy(job);
                    Ok(false)
                }
            }
            CursorState::Inspect(mut job) => {
                if job.advance(admission, inspection)? {
                    self.state = CursorState::Done {
                        bytes: 0,
                        value: None,
                    };
                    Ok(true)
                } else {
                    self.state = CursorState::Inspect(job);
                    Ok(false)
                }
            }
            CursorState::Done { bytes, value } => {
                self.state = CursorState::Done { bytes, value };
                Ok(true)
            }
            CursorState::Failed => Err("JSON cursor already failed".into()),
        }
    }
}
#[cfg(feature = "allocation-profile")]
pub fn copy(value: &Value, admission: &mut impl Admission) -> Result<Value, String> {
    let mut cursor = Cursor::new(value, Mode::Copy);
    while !cursor.advance(admission)? {}
    cursor.take_value()
}
#[cfg(feature = "allocation-profile")]
pub fn serialized_size(value: &Value, admission: &mut impl Admission) -> Result<usize, String> {
    let mut cursor = Cursor::new(value, Mode::Measure);
    while !cursor.advance(admission)? {}
    Ok(cursor.encoded_bytes())
}
/// Stage-specific pending frontier in the shared general JSON engine. Ancestors
/// do not each retain an iterator frame. No source pointer is execution authority:
/// this cursor borrows the actual immutable input for its complete lifetime.
pub struct ResourceCursor<'a> {
    pending: Vec<(&'a Value, usize)>,
    phase: Expansion<'a>,
}
enum Expansion<'a> {
    Root(&'a Value),
    Array(std::slice::Iter<'a, Value>, usize),
    ObjectKeys(&'a Map<String, Value>, serde_json::map::Iter<'a>, usize),
    ObjectChildren(serde_json::map::Iter<'a>, usize),
    Idle,
    Done,
    Failed,
}
impl<'a> ResourceCursor<'a> {
    pub fn new(value: &'a Value) -> Self {
        Self {
            pending: Vec::new(),
            phase: Expansion::Root(value),
        }
    }
    #[cfg(feature = "allocation-profile")]
    pub fn layouts() -> (usize, usize, usize) {
        (
            size_of::<Self>(),
            size_of::<(&Value, usize)>(),
            size_of::<Expansion<'a>>(),
        )
    }
    pub fn retained_slots(&self) -> (usize, usize) {
        (self.pending.len(), self.pending.capacity())
    }
    pub(crate) fn is_complete(&self) -> bool {
        matches!(self.phase, Expansion::Done)
    }
    pub(crate) fn restart(&mut self, value: &'a Value) -> Result<(), String> {
        if !matches!(self.phase, Expansion::Done) || !self.pending.is_empty() {
            return Err("JSON resource cursor must complete before restart".into());
        }
        self.phase = Expansion::Root(value);
        Ok(())
    }
    fn reserve(&mut self, additional: usize, admission: &mut impl Admission) -> Result<(), String> {
        let needed = self
            .pending
            .len()
            .checked_add(additional)
            .ok_or("JSON frontier size overflow")?;
        if needed > self.pending.capacity() {
            admission.allocate(Allocation::Frames {
                old_slots: self.pending.capacity(),
                new_slots: needed,
                element_bytes: size_of::<(&Value, usize)>(),
            })?;
            self.pending
                .try_reserve_exact(additional)
                .map_err(|_| "JSON frontier reservation failed")?;
        }
        Ok(())
    }
    pub fn advance(
        &mut self,
        admission: &mut impl Admission,
        inspection: &mut impl Inspection,
    ) -> Result<bool, String> {
        if matches!(self.phase, Expansion::Failed) {
            return Err("JSON cursor already failed".into());
        }
        if matches!(self.phase, Expansion::Done) {
            return Ok(true);
        }
        let result = self.advance_inner(admission, inspection);
        if result.is_err() {
            self.phase = Expansion::Failed;
        }
        result
    }
    fn enter(
        &mut self,
        value: &'a Value,
        depth: usize,
        admission: &mut impl Admission,
        inspection: &mut impl Inspection,
    ) -> Result<(), String> {
        inspection.node(value, depth, self.pending.len())?;
        match value {
            Value::Array(values) if !values.is_empty() => {
                // Child count and container bytes were admitted by the original policy.
                self.reserve(values.len(), admission)?;
                self.phase = Expansion::Array(values.iter(), depth.saturating_add(1));
            }
            Value::Object(values) if !values.is_empty() => {
                // All keys must pass before child traversal. Keep their admission
                // before the pending reservation too, avoiding growth at every key.
                self.phase = Expansion::ObjectKeys(values, values.iter(), depth.saturating_add(1));
            }
            _ => self.phase = Expansion::Idle,
        }
        Ok(())
    }
    fn advance_inner(
        &mut self,
        admission: &mut impl Admission,
        inspection: &mut impl Inspection,
    ) -> Result<bool, String> {
        match std::mem::replace(&mut self.phase, Expansion::Idle) {
            Expansion::Root(value) => self.enter(value, 0, admission, inspection)?,
            Expansion::Array(mut iter, depth) => {
                if let Some(value) = iter.next() {
                    self.pending.push((value, depth));
                }
                if iter.len() != 0 {
                    self.phase = Expansion::Array(iter, depth);
                }
            }
            Expansion::ObjectKeys(source, mut iter, depth) => {
                if let Some((key, _)) = iter.next() {
                    inspection.key(key)?;
                }
                if iter.len() != 0 {
                    self.phase = Expansion::ObjectKeys(source, iter, depth);
                } else {
                    // Key-byte errors retain their original order/counters and
                    // precede reservation. Source is still immutable; no callback
                    // or semantic validation is repeated by this storage phase.
                    inspection.object_keys_finished()?;
                    self.reserve(source.len(), admission)?;
                    self.phase = Expansion::ObjectChildren(source.iter(), depth);
                }
            }
            Expansion::ObjectChildren(mut iter, depth) => {
                if let Some((_, value)) = iter.next() {
                    self.pending.push((value, depth));
                }
                if iter.len() != 0 {
                    self.phase = Expansion::ObjectChildren(iter, depth);
                }
            }
            Expansion::Idle => {
                if let Some((value, depth)) = self.pending.pop() {
                    self.enter(value, depth, admission, inspection)?;
                } else {
                    self.phase = Expansion::Done;
                    return Ok(true);
                }
            }
            Expansion::Done => {
                self.phase = Expansion::Done;
                return Ok(true);
            }
            Expansion::Failed => return Err("JSON cursor already failed".into()),
        }
        Ok(false)
    }
}
