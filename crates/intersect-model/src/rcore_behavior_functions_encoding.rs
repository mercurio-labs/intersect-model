//! Handwritten resumable result transport. No aggregate ID container is cloned.
use super::{
    machine::State,
    values::{Data, Id},
    *,
};
use intersect_model::definitions::rcore_behavior::ir_v2::{DataWire, TypeId};
use std::{
    collections::BTreeMap,
    ops::Bound::{Excluded, Unbounded},
};

// The arena retains the sole source owner. No borrow is stored across a resume.
pub(super) struct StringCopy {
    copied: String,
    offset: usize,
    length: usize,
}
impl StringCopy {
    fn new(source: &str) -> std::result::Result<Self, String> {
        Self::reserve(source, "Result transport string allocation failed")
    }
    pub(super) fn for_input(source: &str) -> std::result::Result<Self, String> {
        Self::reserve(source, "Input String decoding allocation failed")
    }
    fn reserve(source: &str, allocation_error: &str) -> std::result::Result<Self, String> {
        let copied =
            intersect_model::definitions::retained_json::reserve_string(source, allocation_error)?;
        Ok(Self {
            copied,
            offset: 0,
            length: source.len(),
        })
    }
    pub(super) fn advance(&mut self, source: &str) -> std::result::Result<bool, String> {
        intersect_model::definitions::retained_json::advance_string_copy(
            &mut self.copied,
            &mut self.offset,
            self.length,
            source,
        )
    }
    pub(super) fn into_string(self) -> String {
        self.copied
    }
    #[cfg(any(test, feature = "allocation-profile"))]
    pub(super) fn progress(&self) -> (usize, usize, usize) {
        (self.offset, self.copied.capacity(), self.length)
    }
}

enum Frame<'a> {
    Copy {
        id: Id,
        ty: TypeId,
        job: intersect_model::definitions::retained_json_frontier::CopyJob<'a>,
    },
    String {
        id: Id,
        ty: TypeId,
        copy: StringCopy,
    },
    Visit {
        id: Id,
        depth: usize,
    },
    Wrap {
        ty: TypeId,
        kind: u8,
    },
    Items {
        id: Id,
        depth: usize,
        values: Vec<ValueWire>,
    },
    Record {
        id: Id,
        depth: usize,
        values: BTreeMap<String, ValueWire>,
    },
}
const _: () = assert!(std::mem::size_of::<Frame>() <= 128);
pub(super) struct Encoding<'a> {
    frames: Vec<Frame<'a>>,
    returned: Option<ValueWire>,
}
impl<'a> Encoding<'a> {
    pub(super) fn added_payload_continuation(&self) -> bool {
        matches!(self.frames.last(), Some(Frame::Copy { .. }))
    }
    #[cfg(feature = "allocation-profile")]
    pub(super) fn payload_frontier(&self) -> Option<(&'static str, [usize; 8])> {
        self.frames.last().and_then(|v| match v {
            Frame::Copy { job, .. } => Some(("copy", job.retained_owners())),
            Frame::String { copy, .. } => {
                let (offset, capacity, length) = copy.progress();
                Some(("string-copy", [0, 0, 0, capacity, 0, 0, length, offset]))
            }
            _ => None,
        })
    }
    #[cfg(test)]
    pub(super) fn retained_string_layout() -> (usize, usize, usize) {
        (
            std::mem::size_of::<Frame>(),
            std::mem::size_of::<StringCopy>(),
            std::mem::size_of::<Self>(),
        )
    }
    #[cfg(test)]
    pub(super) fn string_copy_progress(&self) -> Option<(Id, usize, usize)> {
        self.frames.iter().rev().find_map(|frame| match frame {
            Frame::String { id, copy, .. } => Some((*id, copy.offset, copy.copied.capacity())),
            _ => None,
        })
    }
    pub(super) fn new<I: super::inputs::InputFamily>(
        state: &mut State<'_, '_, '_, I>,
        id: Id,
    ) -> Result<Self> {
        state.retained_storage(64, None)?;
        let mut encoding = Self {
            frames: Vec::new(),
            returned: None,
        };
        encoding.push(state, Frame::Visit { id, depth: 0 })?;
        Ok(encoding)
    }
    fn push<I: super::inputs::InputFamily>(
        &mut self,
        state: &mut State<'_, '_, '_, I>,
        frame: Frame<'a>,
    ) -> Result<()> {
        if self.frames.len() >= state.limits.max_value_depth.saturating_add(2) {
            return Err(state.resource("Result transport frame depth exceeded", None));
        }
        if self.frames.len() == self.frames.capacity() {
            let capacity = self
                .frames
                .capacity()
                .saturating_mul(2)
                .max(1)
                .min(state.limits.max_value_depth.saturating_add(2));
            let additional = capacity.saturating_sub(self.frames.len());
            let bytes = additional
                .checked_mul(128)
                .ok_or_else(|| state.resource("Result transport storage overflow", None))?;
            state.retained_storage(bytes, None)?;
            self.frames
                .try_reserve_exact(additional)
                .map_err(|_| state.resource("Result transport frame allocation failed", None))?;
        }
        self.frames.push(frame);
        Ok(())
    }
    pub(super) fn advance<I: super::inputs::InputFamily>(
        &mut self,
        state: &mut State<'_, '_, '_, I>,
        nodes: &'a [super::values::Node],
    ) -> Result<Option<ValueWire>> {
        state.checkpoint(None)?;
        let Some(frame) = self.frames.pop() else {
            return self
                .returned
                .take()
                .map(Some)
                .ok_or_else(|| state.invalid("Missing encoded result", None));
        };
        match frame {
            Frame::Copy { id, ty, mut job } => {
                let node = state.node_in(nodes, id)?;
                let mut admission = super::payload::JsonAdmission {
                    limits: state.limits,
                    admitted_bytes: Some(node.bytes),
                };
                if !job
                    .advance(&mut admission)
                    .map_err(|e| state.resource(&e, None))?
                {
                    self.push(state, Frame::Copy { id, ty, job })?;
                    return Ok(None);
                }
                let payload = job.finish().map_err(|e| state.invalid(&e, None))?;
                let data = match node.data {
                    Data::Scalar(value) => {
                        DataWire::Scalar(intersect_model::definitions::rcore_behavior::ir::Value {
                            ty: value.ty.clone(),
                            payload,
                        })
                    }
                    Data::Datatype(_) => DataWire::Datatype(payload),
                    Data::Model(_) => DataWire::Model(payload),
                    _ => return Err(state.invalid("Missing encoded JSON source", None)),
                };
                self.returned = Some(ValueWire { ty, data });
            }
            Frame::String { id, ty, mut copy } => {
                let node = state.node_in(nodes, id)?;
                let Data::Scalar(value) = node.data else {
                    return Err(state.invalid("Missing retained String transport owner", None));
                };
                let source = value.payload.as_str().ok_or_else(|| {
                    state.invalid("Missing retained String transport source", None)
                })?;
                if copy.advance(source).map_err(|e| state.invalid(&e, None))? {
                    self.returned = Some(ValueWire {
                        ty,
                        data: DataWire::Scalar(
                            intersect_model::definitions::rcore_behavior::ir::Value {
                                ty: value.ty.clone(),
                                payload: serde_json::Value::String(copy.copied),
                            },
                        ),
                    });
                } else {
                    // Replaces the popped frame in its already-admitted slot.
                    self.push(state, Frame::String { id, ty, copy })?;
                }
            }
            Frame::Visit { id, depth } => {
                if depth > state.limits.max_value_depth
                    || state.data.consumed.value_nodes >= state.limits.max_value_nodes
                {
                    return Err(state.resource("Result transport node/depth limit exceeded", None));
                }
                state.data.consumed.value_nodes += 1;
                state.transfer_in(nodes, id, None)?;
                let node = state.node_in(nodes, id)?;
                let ty = node.ty;
                let leaf = match &node.data {
                    Data::View(_) => return Err(state.invalid("Unresolved result view",None)),
                    Data::Scalar(v) if matches!(v.ty, intersect_model::definitions::rcore_behavior::ir::ValueType::Scalar(intersect_model::definitions::rcore_behavior::types::Scalar::String)) && v.payload.is_string() => {
                        let source = v.payload.as_str().ok_or_else(|| state.invalid("Missing String transport source", None))?;
                        // Visit's original node/depth/transfer admission precedes reservation.
                        let mut copy = StringCopy::new(source).map_err(|e| state.resource(&e, None))?;
                        let done = copy.advance(source).map_err(|e| state.invalid(&e, None))?;
                        if done {
                            Some(DataWire::Scalar(intersect_model::definitions::rcore_behavior::ir::Value { ty: v.ty.clone(), payload: serde_json::Value::String(copy.copied) }))
                        } else {
                            self.push(state, Frame::String { id, ty, copy })?;
                            None
                        }
                    },
                    Data::Scalar(v)=>{
                        let mut admission=super::payload::JsonAdmission{limits:state.limits,admitted_bytes:Some(node.bytes)};
                        let job=intersect_model::definitions::retained_json_frontier::CopyJob::new(&v.payload,&mut admission).map_err(|e|state.resource(&e,None))?;
                        self.push(state,Frame::Copy{id,ty,job})?;None
                    },
                    Data::Enum(v) => Some(DataWire::Enum(v.clone())),
                    Data::Datatype(v)|Data::Model(v)=>{
                        let mut admission=super::payload::JsonAdmission{limits:state.limits,admitted_bytes:Some(node.bytes)};
                        let job=intersect_model::definitions::retained_json_frontier::CopyJob::new(v,&mut admission).map_err(|e|state.resource(&e,None))?;
                        self.push(state,Frame::Copy{id,ty,job})?;None
                    },
                    Data::None => Some(DataWire::None),
                    Data::Some(child) | Data::Ok(child) | Data::Err(child) => {
                        let child = *child;
                        let kind = match node.data { Data::Some(_) => 0, Data::Ok(_) => 1, _ => 2 };
                        self.push(state, Frame::Wrap {ty, kind})?;
                        self.push(state, Frame::Visit {id: child, depth: depth+1})?;
                        None
                    },
                    Data::Tuple(_) | Data::Collection(_) => {
                        self.push(state, Frame::Items {id, depth, values: Vec::new()})?;
                        None
                    },
                    Data::Record(_) => {
                        self.push(state, Frame::Record {id, depth, values: BTreeMap::new()})?;
                        None
                    },
                    Data::Closure(_) => return Err(state.error(ExecutionErrorKind::Unsupported, "Executable closure transport is unsupported; invoke it within the evaluation", None)),
                };
                if let Some(data) = leaf {
                    self.returned = Some(ValueWire { ty, data });
                }
            }
            Frame::Wrap { ty, kind } => {
                let value = self
                    .returned
                    .take()
                    .ok_or_else(|| state.invalid("Missing encoded child", None))?;
                state.retained_storage(128, None)?;
                let child = Box::new(value);
                let data = match kind {
                    0 => DataWire::Some(child),
                    1 => DataWire::Ok(child),
                    _ => DataWire::Err(child),
                };
                self.returned = Some(ValueWire { ty, data });
            }
            Frame::Items {
                id,
                depth,
                mut values,
            } => {
                if let Some(value) = self.returned.take() {
                    if values.len() == values.capacity() {
                        let capacity = values
                            .capacity()
                            .saturating_mul(2)
                            .max(1)
                            .min(state.limits.max_value_nodes);
                        let additional = capacity.saturating_sub(values.len());
                        if additional == 0 {
                            return Err(
                                state.resource("Result transport item limit exceeded", None)
                            );
                        }
                        state.retained_storage(
                            additional.checked_mul(128).ok_or_else(|| {
                                state.resource("Result transport storage overflow", None)
                            })?,
                            None,
                        )?;
                        values.try_reserve_exact(additional).map_err(|_| {
                            state.resource("Result transport output allocation failed", None)
                        })?;
                    }
                    values.push(value);
                }
                let node = state.node_in(nodes, id)?;
                let (items, tuple) = match &node.data {
                    Data::Tuple(items) => (items, true),
                    Data::Collection(items) => (items, false),
                    _ => return Err(state.invalid("Invalid result transport collection", None)),
                };
                if let Some(child) = items.get(values.len()).copied() {
                    self.push(state, Frame::Items { id, depth, values })?;
                    self.push(
                        state,
                        Frame::Visit {
                            id: child,
                            depth: depth + 1,
                        },
                    )?;
                } else {
                    self.returned = Some(ValueWire {
                        ty: node.ty,
                        data: if tuple {
                            DataWire::Tuple(values)
                        } else {
                            DataWire::Collection(values)
                        },
                    });
                }
            }
            Frame::Record {
                id,
                depth,
                mut values,
            } => {
                let node = state.node_in(nodes, id)?;
                let ty = node.ty;
                let Data::Record(fields) = &node.data else {
                    return Err(state.invalid("Invalid result transport record", None));
                };
                let next = match values.last_key_value() {
                    Some((last, _)) => fields
                        .range::<str, _>((Excluded(last.as_str()), Unbounded))
                        .next(),
                    None => fields.first_key_value(),
                };
                if let Some(value) = self.returned.take() {
                    let (key, _) =
                        next.ok_or_else(|| state.invalid("Missing result transport field", None))?;
                    let bytes = key
                        .len()
                        .checked_add(192)
                        .ok_or_else(|| state.resource("Result transport storage overflow", None))?;
                    // Charge before retaining the key and map entry.
                    state.retained_storage(bytes, None)?;
                    let Data::Record(fields) = &state.node_in(nodes, id)?.data else {
                        return Err(state.invalid("Invalid result transport record", None));
                    };
                    let next = match values.last_key_value() {
                        Some((last, _)) => fields
                            .range::<str, _>((Excluded(last.as_str()), Unbounded))
                            .next(),
                        None => fields.first_key_value(),
                    };
                    let (key, _) =
                        next.ok_or_else(|| state.invalid("Missing result transport field", None))?;
                    values.insert(key.clone(), value);
                    self.push(state, Frame::Record { id, depth, values })?;
                } else if let Some((_, child)) = next {
                    let child = *child;
                    self.push(state, Frame::Record { id, depth, values })?;
                    self.push(
                        state,
                        Frame::Visit {
                            id: child,
                            depth: depth + 1,
                        },
                    )?;
                } else {
                    self.returned = Some(ValueWire {
                        ty,
                        data: DataWire::Record(values),
                    });
                }
            }
        }
        if self.frames.is_empty() {
            Ok(self.returned.take())
        } else {
            Ok(None)
        }
    }
}
