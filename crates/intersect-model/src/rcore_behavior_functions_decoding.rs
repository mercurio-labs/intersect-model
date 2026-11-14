//! Handwritten decoding after whole-root shape admission. Nominal semantics stay shared.
use super::{
    equivalence::Equivalence,
    machine::State,
    values::{Data, Id},
    *,
};
use intersect_model::definitions::rcore_behavior::{
    ir_v2::{DataWire, TypeId},
    types::CollectionKind,
};
use std::{
    collections::BTreeMap,
    ops::Bound::{Excluded, Unbounded},
};
enum Route {
    Wrapped,
    Index(usize),
    Field(String),
}
enum Frame<'a> {
    Nominal(intersect_model::definitions::conversion::OwnedConversionValidationPreparation<'a, 'a>),
    Measure(intersect_model::definitions::retained_json_frontier::MeasureJob<'a>),
    Copy {
        ticket: super::values::ScalarAllocation,
        job: intersect_model::definitions::retained_json_frontier::CopyJob<'a>,
    },
    String {
        ticket: super::values::ScalarAllocation,
        copy: super::encoding::StringCopy,
        first_chunk_free: bool,
    },
    Visit,
    Leave,
    Root(TypeId),
    Wrap {
        ty: TypeId,
        kind: u8,
    },
    Aggregate {
        ty: TypeId,
        output: Data,
        bytes: usize,
        depth: usize,
    },
    Unique {
        ty: TypeId,
        items: Vec<Id>,
        bytes: usize,
        depth: usize,
        index: usize,
        probe: usize,
        comparison: Option<Equivalence>,
    },
}
const _: () = assert!(std::mem::size_of::<Frame>() <= 192);
pub(super) struct PendingDomain {
    pub(super) admission: Option<crate::model_reference::RuntimeReferenceAdmission>,
    pub(super) cursor: Option<crate::model_reference::RuntimeReferenceDomainPreparation>,
    pub(super) ty: TypeId,
    pub(super) digest: String,
}
pub(super) struct Decoding<'a, I: super::inputs::InputFamily = super::inputs::NoBorrowedModelInputs>
{
    source: &'a ValueWire,
    domain: Option<Box<I::Pending>>,
    frames: Vec<Frame<'a>>,
    path: Vec<Route>,
    returned: Option<Id>,
}
impl<'a, I: super::inputs::InputFamily> Decoding<'a, I> {
    pub(super) fn added_payload_continuation(&self) -> bool {
        matches!(
            self.frames.last(),
            Some(
                Frame::Measure(_)
                    | Frame::Nominal(_)
                    | Frame::Copy { .. }
                    | Frame::String {
                        first_chunk_free: true,
                        ..
                    }
            )
        )
    }
    #[cfg(feature = "allocation-profile")]
    pub(super) fn payload_frontier(&self) -> Option<(&'static str, [usize; 8])> {
        self.frames.last().and_then(|v| match v {
            Frame::Measure(job) => {
                Some(("measure", [0, 0, job.frontier_capacity(), 0, 0, 0, 0, 0]))
            }
            Frame::Copy { job, .. } => Some(("copy", job.retained_owners())),
            Frame::String { copy, .. } => {
                let (offset, capacity, length) = copy.progress();
                Some(("string-copy", [0, 0, 0, capacity, 0, 0, length, offset]))
            }
            _ => None,
        })
    }
    #[cfg(test)]
    pub(super) fn string_copy_progress(&self) -> Option<(usize, usize, usize)> {
        self.frames.iter().find_map(|frame| match frame {
            Frame::String { copy, .. } => Some(copy.progress()),
            _ => None,
        })
    }
    #[cfg(test)]
    pub(super) fn retained_string_layout() -> (usize, usize, usize) {
        (
            std::mem::size_of::<Frame>(),
            std::mem::size_of::<super::encoding::StringCopy>(),
            std::mem::size_of::<Self>(),
        )
    }
    #[cfg(test)]
    pub(super) fn diagnostic_domain_phase(&self) -> Option<&'static str> {
        self.domain
            .as_ref()
            .and_then(|domain| I::diagnostic_phase(domain))
    }

    #[cfg(test)]
    pub(super) fn preparing_domain(&self) -> bool {
        self.domain.is_some()
    }
    pub(super) fn new(
        state: &mut State<'_, '_, '_, I>,
        expected: TypeId,
        root: &'a ValueWire,
        span: Option<&SourceSpan>,
    ) -> Result<Self> {
        state.retained_storage(64, span)?;
        let mut decoder = Self {
            source: root,
            domain: None,
            frames: Vec::new(),
            path: Vec::new(),
            returned: None,
        };
        decoder.push(state, Frame::Root(expected), span)?;
        decoder.push(state, Frame::Visit, span)?;
        Ok(decoder)
    }
    fn push(
        &mut self,
        state: &mut State<'_, '_, '_, I>,
        frame: Frame<'a>,
        span: Option<&SourceSpan>,
    ) -> Result<()> {
        if self.frames.len()
            >= state
                .limits
                .max_value_depth
                .saturating_add(2)
                .saturating_mul(3)
        {
            return Err(state.resource("Decode frame limit exceeded", span));
        }
        if self.frames.len() == self.frames.capacity() {
            state.retained_storage(192, span)?;
            self.frames
                .try_reserve_exact(1)
                .map_err(|_| state.resource("Decode frame allocation failed", span))?;
        }
        self.frames.push(frame);
        Ok(())
    }
    fn descend(
        &mut self,
        state: &mut State<'_, '_, '_, I>,
        route: Route,
        span: Option<&SourceSpan>,
    ) -> Result<()> {
        if self.path.len() == self.path.capacity() {
            state.retained_storage(64, span)?;
            self.path
                .try_reserve_exact(1)
                .map_err(|_| state.resource("Decode path allocation failed", span))?;
        }
        self.path.push(route);
        self.push(state, Frame::Leave, span)?;
        self.push(state, Frame::Visit, span)
    }
    fn wire<'wire>(
        &self,
        state: &State<'_, '_, '_, I>,
        root: &'wire ValueWire,
        span: Option<&SourceSpan>,
    ) -> Result<&'wire ValueWire> {
        let mut wire = root;
        for route in &self.path {
            wire = match (route, &wire.data) {
                (Route::Wrapped, DataWire::Some(v) | DataWire::Ok(v) | DataWire::Err(v)) => v,
                (Route::Index(index), DataWire::Tuple(v) | DataWire::Collection(v)) => v
                    .get(*index)
                    .ok_or_else(|| state.invalid("Missing decoded child", span))?,
                (Route::Field(key), DataWire::Record(v)) => v
                    .get(key)
                    .ok_or_else(|| state.invalid("Missing decoded field", span))?,
                _ => return Err(state.invalid("Decode path shape changed", span)),
            };
        }
        Ok(wire)
    }
    fn finish(
        &mut self,
        state: &mut State<'_, '_, '_, I>,
        id: Id,
        span: Option<&SourceSpan>,
    ) -> Result<()> {
        // Each child retains the original same-type view transfer. The root's
        // final view owns its transfer and any requested type adaptation.
        if !self.path.is_empty() {
            state.transfer(id, span)?;
        }
        self.returned = Some(id);
        Ok(())
    }
    fn prepare_model(
        &mut self,
        state: &mut State<'_, '_, '_, I>,
        ty: TypeId,
        payload: &serde_json::Value,
        span: Option<&SourceSpan>,
    ) -> Result<bool> {
        if !I::NESTED_MODELS && !self.path.is_empty() {
            return Err(state.error(
                ExecutionErrorKind::Unsupported,
                "Unsupported nested model argument on borrowed operation inputs",
                span,
            ));
        }
        let Some(pending) = I::begin_model(state, ty, payload, span)? else {
            return Ok(false);
        };
        self.domain = Some(Box::new(pending));
        Ok(true)
    }
    pub(super) fn advance(
        &mut self,
        state: &mut State<'_, '_, '_, I>,
        root: &'a ValueWire,
        registry: Option<&'a intersect_model::definitions::conversion::ConversionRegistry<'a>>,
        span: Option<&SourceSpan>,
    ) -> Result<Option<Id>> {
        state.checkpoint(span)?;
        if !std::ptr::eq(root, self.source) {
            return Err(state.invalid("Decode source changed", span));
        }
        let root = self.source;
        if let Some(mut pending) = self.domain.take() {
            let done = I::advance_model(state, &mut pending, span)?;
            if !done {
                self.domain = Some(pending);
            }
            return Ok(None);
        }
        let frame = self
            .frames
            .pop()
            .ok_or_else(|| state.invalid("Decode already finished", span))?;
        match frame {
            Frame::Nominal(mut job) => {
                if !state.advance_nominal(&mut job, span)? {
                    self.push(state, Frame::Nominal(job), span)?;
                    return Ok(None);
                }
                let wire = self.wire(state, root, span)?;
                let DataWire::Datatype(payload) = &wire.data else {
                    return Err(state.invalid("Nominal continuation source changed", span));
                };
                state.ensure_leaf_node(span)?;
                self.push(
                    state,
                    Frame::Measure(
                        intersect_model::definitions::retained_json_frontier::MeasureJob::new(
                            payload,
                        ),
                    ),
                    span,
                )?;
            }
            Frame::String {
                ticket, mut copy, ..
            } => {
                let wire = self.wire(state, root, span)?;
                let DataWire::Scalar(value) = &wire.data else {
                    return Err(state.invalid("Missing retained input String owner", span));
                };
                let source = value
                    .payload
                    .as_str()
                    .ok_or_else(|| state.invalid("Missing retained input String source", span))?;
                if copy.advance(source).map_err(|e| state.invalid(&e, span))? {
                    let value = intersect_model::definitions::rcore_behavior::ir::Value {
                        ty: value.ty.clone(),
                        payload: serde_json::Value::String(copy.into_string()),
                    };
                    let id = state.publish_scalar(ticket, value, span)?;
                    self.finish(state, id, span)?;
                } else {
                    // Replace the popped Visit/String in its already charged slot.
                    self.push(
                        state,
                        Frame::String {
                            ticket,
                            copy,
                            first_chunk_free: false,
                        },
                        span,
                    )?;
                }
            }
            Frame::Measure(mut job) => {
                let mut admission = super::payload::JsonAdmission {
                    limits: state.limits,
                    admitted_bytes: None,
                };
                if !job
                    .advance(&mut admission)
                    .map_err(|e| state.resource(&e, span))?
                {
                    self.push(state, Frame::Measure(job), span)?;
                    return Ok(None);
                }
                let wire = self.wire(state, root, span)?;
                let payload = match &wire.data {
                    DataWire::Scalar(value) => &value.payload,
                    DataWire::Datatype(value) | DataWire::Model(value) => value,
                    _ => return Err(state.invalid("Missing measured JSON source", span)),
                };
                let mut bytes = job.encoded_bytes();
                if let DataWire::Scalar(value) = &wire.data {
                    bytes = bytes
                        .checked_add(state.scalar_header_bytes(value, span)?)
                        .ok_or_else(|| state.invalid("Scalar value size overflow", span))?;
                }
                let ticket = state.admit_measured_leaf(wire.ty, bytes, span)?;
                if let DataWire::Scalar(value) = &wire.data
                    && matches!(
                        value.ty,
                        intersect_model::definitions::rcore_behavior::ir::ValueType::Scalar(
                            intersect_model::definitions::rcore_behavior::types::Scalar::String
                        )
                    )
                    && value.payload.is_string()
                {
                    let source = value
                        .payload
                        .as_str()
                        .ok_or_else(|| state.invalid("Missing input String source", span))?;
                    let copy = super::encoding::StringCopy::for_input(source)
                        .map_err(|e| state.resource(&e, span))?;
                    self.push(
                        state,
                        Frame::String {
                            ticket,
                            copy,
                            first_chunk_free: true,
                        },
                        span,
                    )?;
                    return Ok(None);
                }
                let mut admission = super::payload::JsonAdmission {
                    limits: state.limits,
                    admitted_bytes: Some(bytes),
                };
                let job = intersect_model::definitions::retained_json_frontier::CopyJob::new(
                    payload,
                    &mut admission,
                )
                .map_err(|e| state.resource(&e, span))?;
                self.push(state, Frame::Copy { ticket, job }, span)?;
            }
            Frame::Copy { ticket, mut job } => {
                let wire = self.wire(state, root, span)?;
                // The ticket's exact debit is already retained; no second charge.
                let bytes = state.admitted_leaf_bytes(&ticket);
                let mut admission = super::payload::JsonAdmission {
                    limits: state.limits,
                    admitted_bytes: Some(bytes),
                };
                if !job
                    .advance(&mut admission)
                    .map_err(|e| state.resource(&e, span))?
                {
                    self.push(state, Frame::Copy { ticket, job }, span)?;
                    return Ok(None);
                }
                let payload = job.finish().map_err(|e| state.invalid(&e, span))?;
                let id = match &wire.data {
                    DataWire::Scalar(value) => state.publish_scalar(
                        ticket,
                        intersect_model::definitions::rcore_behavior::ir::Value {
                            ty: value.ty.clone(),
                            payload,
                        },
                        span,
                    )?,
                    DataWire::Datatype(_) => {
                        state.publish_json_leaf(ticket, Data::Datatype(payload), span)?
                    }
                    DataWire::Model(_) => {
                        state.publish_json_leaf(ticket, Data::Model(payload), span)?
                    }
                    _ => return Err(state.invalid("JSON copy source changed", span)),
                };
                self.finish(state, id, span)?;
            }
            Frame::Leave => {
                self.path.pop();
            }
            Frame::Root(expected) => {
                if matches!(
                    state.type_node(expected)?,
                    intersect_model::definitions::rcore_behavior::ir_v2::TypeNode::Model(_)
                ) {
                    let id = self
                        .returned
                        .ok_or_else(|| state.invalid("Missing decoded root", span))?;
                    if let Data::Model(payload) = &state.node(id)?.data {
                        let payload = payload.clone();
                        if self.prepare_model(state, expected, &payload, span)? {
                            self.push(state, Frame::Root(expected), span)?;
                            return Ok(None);
                        }
                    }
                }
                let id = self
                    .returned
                    .take()
                    .ok_or_else(|| state.invalid("Missing decoded root", span))?;
                self.returned = Some(state.view(id, expected, span)?);
            }
            Frame::Visit => {
                if self.path.len() > state.limits.max_value_depth {
                    return Err(state.resource("Value nesting limit exceeded", span));
                }
                let wire = self.wire(state, root, span)?;
                let data = match &wire.data {
                    DataWire::Scalar(value) => {
                        state.ensure_leaf_node(span)?;
                        self.push(state,Frame::Measure(intersect_model::definitions::retained_json_frontier::MeasureJob::new(&value.payload)),span)?;
                        None
                    }
                    DataWire::Enum(v) => Some(Data::Enum(v.clone())),
                    DataWire::Datatype(v) => {
                        if let Some(job) = state.begin_nominal(wire.ty, v, registry, span)? {
                            self.push(state, Frame::Nominal(job), span)?;
                            return Ok(None);
                        }
                        state.ensure_leaf_node(span)?;
                        self.push(state,Frame::Measure(intersect_model::definitions::retained_json_frontier::MeasureJob::new(v)),span)?;
                        None
                    }
                    DataWire::Model(v) => {
                        if self.prepare_model(state, wire.ty, v, span)? {
                            self.push(state, Frame::Visit, span)?;
                            return Ok(None);
                        }
                        state.nominal(wire.ty, v, span)?;
                        state.ensure_leaf_node(span)?;
                        self.push(state,Frame::Measure(intersect_model::definitions::retained_json_frontier::MeasureJob::new(v)),span)?;
                        None
                    }
                    DataWire::None => Some(Data::None),
                    DataWire::Some(_) | DataWire::Ok(_) | DataWire::Err(_) => {
                        let kind = match wire.data {
                            DataWire::Some(_) => 0,
                            DataWire::Ok(_) => 1,
                            _ => 2,
                        };
                        self.push(state, Frame::Wrap { ty: wire.ty, kind }, span)?;
                        self.descend(state, Route::Wrapped, span)?;
                        None
                    }
                    DataWire::Tuple(_) | DataWire::Collection(_) | DataWire::Record(_) => {
                        let output = match wire.data {
                            DataWire::Tuple(_) => Data::Tuple(Vec::new()),
                            DataWire::Record(_) => Data::Record(BTreeMap::new()),
                            _ => Data::Collection(Vec::new()),
                        };
                        self.push(
                            state,
                            Frame::Aggregate {
                                ty: wire.ty,
                                output,
                                bytes: 32,
                                depth: 1,
                            },
                            span,
                        )?;
                        None
                    }
                };
                if let Some(data) = data {
                    let id = state.alloc(wire.ty, data, span)?;
                    self.finish(state, id, span)?;
                }
            }
            Frame::Wrap { ty, kind } => {
                let child = self
                    .returned
                    .take()
                    .ok_or_else(|| state.invalid("Missing decoded wrapper child", span))?;
                let data = match kind {
                    0 => Data::Some(child),
                    1 => Data::Ok(child),
                    _ => Data::Err(child),
                };
                let id = state.alloc(ty, data, span)?;
                self.finish(state, id, span)?;
            }
            Frame::Aggregate {
                ty,
                mut output,
                mut bytes,
                mut depth,
            } => {
                let wire = self.wire(state, root, span)?;
                if let Some(id) = self.returned.take() {
                    bytes = bytes
                        .checked_add(state.node(id)?.bytes)
                        .ok_or_else(|| state.resource("Decoded aggregate size overflow", span))?;
                    depth = depth.max(state.node(id)?.depth.saturating_add(1));
                    if matches!(output, Data::Collection(_)) {
                        let (_, element, _, _) = state.collection_info(ty)?;
                        state.permit(state.node(id)?.ty, element, span)?;
                    }
                    match &mut output {
                        Data::Tuple(ids) | Data::Collection(ids) => {
                            state.check_allocation(bytes, depth, span)?;
                            if ids.len() == ids.capacity() {
                                let capacity = ids
                                    .capacity()
                                    .saturating_mul(2)
                                    .max(1)
                                    .min(state.limits.max_value_nodes);
                                let additional = capacity.saturating_sub(ids.len());
                                if additional == 0 {
                                    return Err(state
                                        .resource("Decoded aggregate item limit exceeded", span));
                                }
                                state.retained_storage(
                                    additional.checked_mul(8).ok_or_else(|| {
                                        state.resource("Decode storage overflow", span)
                                    })?,
                                    span,
                                )?;
                                ids.try_reserve_exact(additional).map_err(|_| {
                                    state.resource("Decode output allocation failed", span)
                                })?;
                            }
                            ids.push(id);
                        }
                        Data::Record(ids) => {
                            let DataWire::Record(fields) = &wire.data else {
                                return Err(state.invalid("Missing record wire", span));
                            };
                            let next = match ids.last_key_value() {
                                Some((last, _)) => fields
                                    .range::<str, _>((Excluded(last.as_str()), Unbounded))
                                    .next(),
                                None => fields.first_key_value(),
                            };
                            let (key, _) = next
                                .ok_or_else(|| state.invalid("Missing record wire field", span))?;
                            bytes = bytes.checked_add(key.len()).ok_or_else(|| {
                                state.resource("Decoded record size overflow", span)
                            })?;
                            state.check_allocation(bytes, depth, span)?;
                            state.retained_storage(
                                key.len().checked_add(64).ok_or_else(|| {
                                    state.resource("Decode record storage overflow", span)
                                })?,
                                span,
                            )?;
                            ids.insert(key.clone(), id);
                        }
                        _ => return Err(state.invalid("Invalid decode aggregate", span)),
                    }
                }
                let next = match (&output, &wire.data) {
                    (
                        Data::Tuple(ids) | Data::Collection(ids),
                        DataWire::Tuple(values) | DataWire::Collection(values),
                    ) => values.get(ids.len()).map(|_| Route::Index(ids.len())),
                    (Data::Record(ids), DataWire::Record(fields)) => {
                        let next = match ids.last_key_value() {
                            Some((last, _)) => fields
                                .range::<str, _>((Excluded(last.as_str()), Unbounded))
                                .next(),
                            None => fields.first_key_value(),
                        };
                        if let Some((key, _)) = next {
                            state.retained_storage(key.len(), span)?;
                            Some(Route::Field(key.clone()))
                        } else {
                            None
                        }
                    }
                    _ => return Err(state.invalid("Decode aggregate shape mismatch", span)),
                };
                if let Some(route) = next {
                    self.push(
                        state,
                        Frame::Aggregate {
                            ty,
                            output,
                            bytes,
                            depth,
                        },
                        span,
                    )?;
                    self.descend(state, route, span)?;
                } else {
                    let unique = if let Data::Collection(ids) = &output {
                        let (kind, _, lower, upper) = state.collection_info(ty)?;
                        if ids.len() < lower || (upper >= 0 && ids.len() > upper as usize) {
                            return Err(state.error(
                                ExecutionErrorKind::Arguments,
                                "Collection multiplicity mismatch",
                                span,
                            ));
                        }
                        matches!(kind, CollectionKind::Set | CollectionKind::OrderedSet)
                    } else {
                        false
                    };
                    if unique {
                        let Data::Collection(items) = output else {
                            return Err(state.invalid("Missing unique output", span));
                        };
                        self.push(
                            state,
                            Frame::Unique {
                                ty,
                                items,
                                bytes,
                                depth,
                                index: 1,
                                probe: 0,
                                comparison: None,
                            },
                            span,
                        )?;
                    } else {
                        let id = state.alloc_measured(ty, output, bytes, depth, span)?;
                        self.finish(state, id, span)?;
                    }
                }
            }
            Frame::Unique {
                ty,
                items,
                bytes,
                depth,
                mut index,
                mut probe,
                mut comparison,
            } => {
                if index >= items.len() {
                    let id =
                        state.alloc_measured(ty, Data::Collection(items), bytes, depth, span)?;
                    self.finish(state, id, span)?;
                } else {
                    if probe >= index {
                        index += 1;
                        probe = 0;
                    } else {
                        if comparison.is_none() {
                            comparison = Some(Equivalence::new(
                                state,
                                items[index],
                                items[probe],
                                true,
                                0,
                                span,
                            )?);
                        }
                        let pending = comparison
                            .as_mut()
                            .ok_or_else(|| state.invalid("Missing decode comparison", span))?;
                        if let Some(equal) = pending.advance(state, span)? {
                            if equal {
                                return Err(state.error(
                                    ExecutionErrorKind::Arguments,
                                    "Duplicate value in unique collection",
                                    span,
                                ));
                            }
                            comparison = None;
                            probe += 1;
                        }
                    }
                    self.push(
                        state,
                        Frame::Unique {
                            ty,
                            items,
                            bytes,
                            depth,
                            index,
                            probe,
                            comparison,
                        },
                        span,
                    )?;
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
