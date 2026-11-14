//! One arbitrary-JSON traversal for retained exact measurement and ownership.
//! Source/type/nominal authority and runtime allocation policy remain callers.
use crate::definitions::retained_json::{Admission, Allocation};
use serde_json::{
    Map, Value,
    ser::{CompactFormatter, Formatter},
};
use std::{
    io::{self, Write},
    mem::size_of,
};
struct Sources<'a> {
    input: &'a Value,
    output: self_cell::MutBorrow<Value>,
}
struct Task<'a> {
    input: &'a Value,
    output: Option<&'a mut Value>,
}
enum Children<'a> {
    Measure(serde_json::map::Iter<'a>),
    Copy {
        input: &'a Map<String, Value>,
        output: serde_json::map::IterMut<'a>,
    },
}
enum Phase<'a> {
    Root(Task<'a>),
    ArrayInit {
        input: &'a [Value],
        output: &'a mut Vec<Value>,
    },
    ArrayChildren {
        input: std::slice::Iter<'a, Value>,
        output: Option<std::slice::IterMut<'a, Value>>,
    },
    ObjectKeys {
        input: &'a Map<String, Value>,
        output: Option<&'a mut Map<String, Value>>,
        iterator: serde_json::map::Iter<'a>,
    },
    Key {
        input: &'a Map<String, Value>,
        output: Option<&'a mut Map<String, Value>>,
        iterator: serde_json::map::Iter<'a>,
        key: &'a str,
        copied: Option<String>,
        offset: usize,
    },
    ObjectChildren(Children<'a>),
    Text {
        input: &'a str,
        output: Option<&'a mut Value>,
        copied: Option<String>,
        offset: usize,
    },
    Idle,
    Done,
    Failed,
}
struct Frontier<'a> {
    pending: Vec<Task<'a>>,
    phase: Phase<'a>,
    bytes: usize,
}
self_cell::self_cell!(
    struct Cell<'input> {
        owner: Sources<'input>,
        #[covariant]
        dependent: Frontier,
    }
);
pub struct CopyJob<'a> {
    cell: Cell<'a>,
    complete: bool,
}
pub struct MeasureJob<'a> {
    frontier: Frontier<'a>,
    complete: bool,
}
impl<'a> CopyJob<'a> {
    pub const OWNER_BYTES: usize = size_of::<(Sources<'a>, Frontier<'a>)>();
    pub const TASK_BYTES: usize = size_of::<Task<'a>>();
    #[cfg(feature = "allocation-profile")]
    pub fn layouts() -> [usize; 7] {
        [
            size_of::<Self>(),
            size_of::<Sources<'a>>(),
            size_of::<Frontier<'a>>(),
            size_of::<(Sources<'a>, Frontier<'a>)>(),
            size_of::<Task<'a>>(),
            size_of::<MeasureJob<'a>>(),
            size_of::<Phase<'a>>(),
        ]
    }
    #[cfg(feature = "allocation-profile")]
    pub fn retained_owners(&self) -> [usize; 8] {
        let capacity = self.frontier_capacity();
        self.cell.with_dependent(|_, v| {
            let mut out = [Self::OWNER_BYTES, v.pending.len(), capacity, 0, 0, 0, 0, 0];
            match &v.phase {
                Phase::ArrayInit { output, .. } => {
                    out[4] = output.len();
                    out[5] = output.capacity();
                }
                Phase::ObjectKeys {
                    output: Some(output),
                    ..
                }
                | Phase::Key {
                    output: Some(output),
                    ..
                } => out[6] = output.len(),
                _ => {}
            }
            match &v.phase {
                Phase::Text {
                    copied: Some(copied),
                    offset,
                    ..
                }
                | Phase::Key {
                    copied: Some(copied),
                    offset,
                    ..
                } => {
                    out[3] = copied.capacity();
                    out[7] = *offset;
                }
                _ => {}
            }
            out
        })
    }
    pub fn new(input: &'a Value, admission: &mut impl Admission) -> Result<Self, String> {
        admission.allocate(Allocation::Frames {
            old_slots: 0,
            new_slots: 1,
            element_bytes: size_of::<(Sources<'a>, Frontier<'a>)>(),
        })?;
        let cell = Cell::new(
            Sources {
                input,
                output: self_cell::MutBorrow::new(Value::Null),
            },
            |owner| Frontier::new(owner.input, Some(owner.output.borrow_mut())),
        );
        Ok(Self {
            cell,
            complete: false,
        })
    }
    #[cfg(feature = "allocation-profile")]
    pub fn encoded_bytes(&self) -> usize {
        self.cell.borrow_dependent().bytes
    }
    #[cfg(feature = "allocation-profile")]
    pub fn frontier_capacity(&self) -> usize {
        self.cell.borrow_dependent().pending.capacity()
    }
    pub fn advance(&mut self, admission: &mut impl Admission) -> Result<bool, String> {
        if self.complete {
            return Ok(true);
        }
        let done = self
            .cell
            .with_dependent_mut(|_, cursor| cursor.advance(admission))?;
        self.complete = done;
        Ok(done)
    }
    pub fn finish(self) -> Result<Value, String> {
        if !self.complete {
            return Err("JSON copy is not complete".into());
        }
        Ok(self.cell.into_owner().output.into_inner())
    }
}
impl<'a> MeasureJob<'a> {
    pub fn new(input: &'a Value) -> Self {
        Self {
            frontier: Frontier::new(input, None),
            complete: false,
        }
    }
    pub fn encoded_bytes(&self) -> usize {
        self.frontier.bytes
    }
    #[cfg(feature = "allocation-profile")]
    pub fn frontier_capacity(&self) -> usize {
        self.frontier.pending.capacity()
    }
    pub fn advance(&mut self, admission: &mut impl Admission) -> Result<bool, String> {
        if self.complete {
            return Ok(true);
        }
        self.complete = self.frontier.advance(admission)?;
        Ok(self.complete)
    }
}
struct Counter(usize);
impl Write for Counter {
    fn write(&mut self, b: &[u8]) -> io::Result<usize> {
        self.0 = self
            .0
            .checked_add(b.len())
            .ok_or_else(|| io::Error::other("Diagnostic metadata byte limit exceeded"))?;
        Ok(b.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
impl<'a> Frontier<'a> {
    fn new(input: &'a Value, output: Option<&'a mut Value>) -> Self {
        Self {
            pending: Vec::new(),
            phase: Phase::Root(Task { input, output }),
            bytes: 0,
        }
    }
    fn add(&mut self, n: usize) -> Result<(), String> {
        self.bytes = self
            .bytes
            .checked_add(n)
            .ok_or("Diagnostic metadata byte limit exceeded")?;
        Ok(())
    }
    fn multiply(n: usize, each: usize) -> Result<usize, String> {
        n.checked_mul(each)
            .ok_or_else(|| "Diagnostic metadata byte limit exceeded".into())
    }
    fn container_bytes(&mut self, count: usize, object: bool) -> Result<(), String> {
        // Official formatter supplies structural spelling. Only additive repetition
        // uses arithmetic; no JSON/wire schema or field names are reimplemented.
        let mut f = CompactFormatter;
        let mut base = Counter(0);
        let mut comma = Counter(0);
        let mut colon = Counter(0);
        if object {
            f.begin_object(&mut base).map_err(|e| e.to_string())?;
            f.end_object(&mut base).map_err(|e| e.to_string())?;
            f.begin_object_key(&mut comma, false)
                .map_err(|e| e.to_string())?;
            f.begin_object_value(&mut colon)
                .map_err(|e| e.to_string())?;
        } else {
            f.begin_array(&mut base).map_err(|e| e.to_string())?;
            f.end_array(&mut base).map_err(|e| e.to_string())?;
            f.begin_array_value(&mut comma, false)
                .map_err(|e| e.to_string())?;
        }
        self.add(base.0)?;
        self.add(Self::multiply(count.saturating_sub(1), comma.0)?)?;
        self.add(Self::multiply(count, colon.0)?)
    }
    fn reserve(&mut self, count: usize, admission: &mut impl Admission) -> Result<(), String> {
        let needed = self
            .pending
            .len()
            .checked_add(count)
            .ok_or("JSON frontier size overflow")?;
        if needed > self.pending.capacity() {
            admission.allocate(Allocation::Frames {
                old_slots: self.pending.capacity(),
                new_slots: needed,
                element_bytes: size_of::<Task<'a>>(),
            })?;
            self.pending
                .try_reserve_exact(count)
                .map_err(|_| "JSON frontier reservation failed")?;
        }
        Ok(())
    }
    fn text(input: &str, admission: &mut impl Admission, key: bool) -> Result<String, String> {
        admission.allocate(Allocation::String {
            bytes: input.len(),
            key,
        })?;
        crate::definitions::retained_json::reserve_string(input, "JSON text reservation failed")
    }
    fn enter(&mut self, task: Task<'a>, admission: &mut impl Admission) -> Result<(), String> {
        match task.input {
            Value::Null | Value::Bool(_) | Value::Number(_) => {
                self.add(crate::definitions::metadata_limits::serialized_size(
                    task.input,
                    usize::MAX,
                )?)?;
                if let Some(output) = task.output {
                    *output = task.input.clone()
                }
                self.phase = Phase::Idle;
            }
            Value::String(input) => {
                self.add(crate::definitions::metadata_limits::serialized_size(
                    "",
                    usize::MAX,
                )?)?;
                let copied = if task.output.is_some() {
                    Some(Self::text(input, admission, false)?)
                } else {
                    None
                };
                self.phase = Phase::Text {
                    input,
                    output: task.output,
                    copied,
                    offset: 0,
                };
            }
            Value::Array(input) => {
                self.container_bytes(input.len(), false)?;
                if let Some(output) = task.output {
                    admission.allocate(Allocation::Array {
                        elements: input.len(),
                        element_bytes: size_of::<Value>(),
                    })?;
                    let mut out = Vec::new();
                    out.try_reserve_exact(input.len())
                        .map_err(|_| "JSON array reservation failed")?;
                    *output = Value::Array(out);
                    let Value::Array(output) = output else {
                        return Err("JSON array target changed".into());
                    };
                    self.phase = Phase::ArrayInit { input, output };
                } else {
                    self.reserve(input.len(), admission)?;
                    self.phase = Phase::ArrayChildren {
                        input: input.iter(),
                        output: None,
                    };
                }
            }
            Value::Object(input) => {
                self.container_bytes(input.len(), true)?;
                let output = if let Some(output) = task.output {
                    admission.allocate(Allocation::Object {
                        entries: input.len(),
                        key_value_bytes: size_of::<(String, Value)>(),
                    })?;
                    *output = Value::Object(Map::new());
                    let Value::Object(output) = output else {
                        return Err("JSON object target changed".into());
                    };
                    Some(output)
                } else {
                    None
                };
                self.phase = Phase::ObjectKeys {
                    input,
                    output,
                    iterator: input.iter(),
                };
            }
        }
        Ok(())
    }
    fn advance(&mut self, admission: &mut impl Admission) -> Result<bool, String> {
        if matches!(self.phase, Phase::Failed) {
            return Err("JSON cursor already failed".into());
        }
        if matches!(self.phase, Phase::Done) {
            return Ok(true);
        }
        let r = self.step(admission);
        if r.is_err() {
            self.phase = Phase::Failed;
        }
        r
    }
    fn fragment(&mut self, text: &str, start: usize) -> Result<usize, String> {
        let end = crate::definitions::bundle::input_chunk_end(text, start);
        let fragment = text.get(start..end).ok_or("Invalid JSON text boundary")?;
        let bytes = crate::definitions::metadata_limits::serialized_size(fragment, usize::MAX)?
            .checked_sub(2)
            .ok_or("Invalid JSON text fragment size")?;
        self.add(bytes)?;
        Ok(end)
    }
    fn step(&mut self, admission: &mut impl Admission) -> Result<bool, String> {
        match std::mem::replace(&mut self.phase, Phase::Idle) {
            Phase::Root(task) => self.enter(task, admission)?,
            Phase::Idle => {
                if let Some(task) = self.pending.pop() {
                    self.enter(task, admission)?
                } else {
                    self.phase = Phase::Done;
                    return Ok(true);
                }
            }
            Phase::ArrayInit { input, output } => {
                if output.len() < input.len() {
                    output.push(Value::Null);
                    self.phase = Phase::ArrayInit { input, output };
                } else {
                    self.reserve(input.len(), admission)?;
                    self.phase = Phase::ArrayChildren {
                        input: input.iter(),
                        output: Some(output.iter_mut()),
                    };
                }
            }
            Phase::ArrayChildren {
                mut input,
                mut output,
            } => {
                if let Some(value) = input.next() {
                    let target = match &mut output {
                        Some(output) => {
                            Some(output.next().ok_or("JSON array target length changed")?)
                        }
                        None => None,
                    };
                    self.pending.push(Task {
                        input: value,
                        output: target,
                    });
                } else if output.as_mut().is_some_and(|v| v.next().is_some()) {
                    return Err("JSON array child source changed".into());
                }
                if input.len() != 0 {
                    self.phase = Phase::ArrayChildren { input, output };
                }
            }
            Phase::ObjectKeys {
                input,
                output,
                mut iterator,
            } => {
                if let Some((key, _)) = iterator.next() {
                    self.add(crate::definitions::metadata_limits::serialized_size(
                        "",
                        usize::MAX,
                    )?)?;
                    let copied = if output.is_some() {
                        Some(Self::text(key, admission, true)?)
                    } else {
                        None
                    };
                    self.phase = Phase::Key {
                        input,
                        output,
                        iterator,
                        key,
                        copied,
                        offset: 0,
                    };
                } else {
                    self.reserve(input.len(), admission)?;
                    self.phase = Phase::ObjectChildren(match output {
                        Some(output) => Children::Copy {
                            input,
                            output: output.iter_mut(),
                        },
                        None => Children::Measure(input.iter()),
                    });
                }
            }
            Phase::Key {
                input,
                mut output,
                iterator,
                key,
                mut copied,
                offset,
            } => {
                let end = self.fragment(key, offset)?;
                if let Some(copied) = &mut copied {
                    let mut written = offset;
                    crate::definitions::retained_json::advance_string_copy(
                        copied,
                        &mut written,
                        key.len(),
                        key,
                    )?;
                    if written != end {
                        return Err("JSON key chunk progress changed".into());
                    }
                }
                if end == key.len() {
                    if let Some(output) = &mut output {
                        output.insert(copied.ok_or("Missing JSON key output")?, Value::Null);
                    }
                    self.phase = Phase::ObjectKeys {
                        input,
                        output,
                        iterator,
                    };
                } else {
                    self.phase = Phase::Key {
                        input,
                        output,
                        iterator,
                        key,
                        copied,
                        offset: end,
                    };
                }
            }
            Phase::ObjectChildren(mut children) => {
                let remaining = match &mut children {
                    Children::Measure(iter) => {
                        if let Some((_, value)) = iter.next() {
                            self.pending.push(Task {
                                input: value,
                                output: None,
                            });
                        }
                        iter.len()
                    }
                    Children::Copy { input, output } => {
                        if let Some((key, target)) = output.next() {
                            let value = input.get(key).ok_or("JSON child source changed")?;
                            self.pending.push(Task {
                                input: value,
                                output: Some(target),
                            });
                        }
                        output.len()
                    }
                };
                if remaining != 0 {
                    self.phase = Phase::ObjectChildren(children);
                }
            }
            Phase::Text {
                input,
                output,
                mut copied,
                offset,
            } => {
                let end = self.fragment(input, offset)?;
                if let Some(copied) = &mut copied {
                    let mut written = offset;
                    crate::definitions::retained_json::advance_string_copy(
                        copied,
                        &mut written,
                        input.len(),
                        input,
                    )?;
                    if written != end {
                        return Err("JSON String chunk progress changed".into());
                    }
                }
                if end == input.len() {
                    if let Some(output) = output {
                        *output = Value::String(copied.ok_or("Missing JSON text output")?);
                    }
                } else {
                    self.phase = Phase::Text {
                        input,
                        output,
                        copied,
                        offset: end,
                    };
                }
            }
            Phase::Done => {
                self.phase = Phase::Done;
                return Ok(true);
            }
            Phase::Failed => return Err("JSON cursor already failed".into()),
        }
        Ok(false)
    }
}
