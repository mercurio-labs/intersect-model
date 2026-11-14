//! Handwritten resumable collection continuations over the common function frames.
use super::{
    machine::{CallSite, State},
    values::{Data, Id},
    *,
};
use intersect_model::definitions::rcore_behavior::{
    ir,
    ir_v2::{CollectionOp, TypeId, TypeNode},
    types::Scalar,
};

pub(super) struct CollectionState {
    closure: Option<Box<super::closure::GraphClosure>>,
    sorting: Option<Box<super::sorting::Sorting>>,
    grouping: Option<Box<super::grouping::Grouping>>,
    operation: CollectionOp,
    input: Id,
    callback: Option<Id>,
    accumulator: Option<Id>,
    ty: TypeId,
    site: CallSite,
    cursor: usize,
    current: Option<Id>,
    inner: Option<(Id, usize)>,
    pub returned: Option<Id>,
    matches: usize,
    output: CollectionOutput,
}
pub(super) enum Advance {
    Pending,
    Call {
        callback: Id,
        arguments: Vec<Id>,
        result: TypeId,
    },
    Done(Id),
}
impl<I: super::inputs::InputFamily> State<'_, '_, '_, I> {
    pub(super) fn collection_storage(&mut self, bytes: usize, span: &SourceSpan) -> Result<()> {
        self.retained_storage(bytes, Some(span))
    }
    pub(super) fn retained_storage(
        &mut self,
        bytes: usize,
        span: Option<&SourceSpan>,
    ) -> Result<()> {
        self.data.consumed.value_bytes = self
            .data
            .consumed
            .value_bytes
            .checked_add(bytes)
            .filter(|n| *n <= self.limits.scalar.max_value_bytes)
            .ok_or_else(|| self.resource("Continuation storage budget exceeded", span))?;
        Ok(())
    }
    pub(super) fn start_collection(
        &mut self,
        operation: CollectionOp,
        input: Id,
        callback: Option<Id>,
        seed: Option<Id>,
        ty: TypeId,
        site: CallSite,
    ) -> Result<()> {
        let span = self
            .image
            .call_span(site)
            .ok_or_else(|| self.invalid("Missing collection source", None))?;
        // Logical storage charges are identical on native and wasm32 hosts.
        self.collection_storage(256, span)?;
        if !matches!(self.node(input)?.data, Data::Collection(_)) {
            return Err(self.invalid("Collection operation requires collection data", Some(span)));
        }
        let grouping = if operation == CollectionOp::GroupBy {
            self.collection_storage(128, span)?;
            Some(Box::new(super::grouping::Grouping::new(self, ty, span)?))
        } else {
            None
        };
        let sorting = if matches!(operation, CollectionOp::SortedWith | CollectionOp::SortedBy) {
            Some(Box::new(super::sorting::Sorting::new(
                self,
                input,
                ty,
                operation == CollectionOp::SortedBy,
                span,
            )?))
        } else {
            None
        };
        let closure = if operation == CollectionOp::Closure {
            self.collection_storage(128, span)?;
            Some(Box::new(super::closure::GraphClosure::new(ty)))
        } else {
            None
        };
        let state = CollectionState {
            closure,
            sorting,
            grouping,
            operation,
            input,
            callback,
            accumulator: seed,
            ty,
            site,
            cursor: 0,
            current: None,
            inner: None,
            returned: None,
            matches: 0,
            output: CollectionOutput::new(ty),
        };
        let Some(frame) = self.data.frames.last_mut() else {
            return Err(self.invalid("Missing collection frame", Some(span)));
        };
        frame.collection = Some(state);
        Ok(())
    }
    pub(super) fn advance_collection(&mut self) -> Result<()> {
        let mut state = self
            .data
            .frames
            .last_mut()
            .and_then(|frame| frame.collection.take())
            .ok_or_else(|| self.invalid("Missing collection continuation", None))?;
        let span = self
            .image
            .call_span(state.site)
            .ok_or_else(|| self.invalid("Missing collection source", None))?;
        self.step(span)?;
        let action = state.advance(self, span)?;
        match action {
            Advance::Done(id) => {
                self.permit(self.node(id)?.ty, state.ty, Some(span))?;
                if let Some(frame) = self.data.frames.last_mut() {
                    frame.registers.push(id);
                } else {
                    return Err(self.invalid("Missing collection result frame", Some(span)));
                }
            }
            Advance::Pending => {
                if let Some(frame) = self.data.frames.last_mut() {
                    frame.collection = Some(state);
                } else {
                    return Err(self.invalid("Missing collection continuation frame", Some(span)));
                }
            }
            Advance::Call {
                callback,
                arguments,
                result,
            } => {
                let site = state.site;
                if let Some(frame) = self.data.frames.last_mut() {
                    frame.collection = Some(state);
                } else {
                    return Err(self.invalid("Missing collection callback frame", Some(span)));
                }
                self.invoke_closure(callback, arguments, site, result)?;
            }
        }
        Ok(())
    }
}
impl CollectionState {
    fn boolean<I: super::inputs::InputFamily>(
        &self,
        state: &mut State<'_, '_, '_, I>,
        value: bool,
        span: &SourceSpan,
    ) -> Result<Advance> {
        Ok(Advance::Done(state.alloc(
            self.ty,
            Data::Scalar(ir::Value {
                ty: ir::ValueType::Scalar(Scalar::Bool),
                payload: value.into(),
            }),
            Some(span),
        )?))
    }
    fn advance<I: super::inputs::InputFamily>(
        &mut self,
        state: &mut State<'_, '_, '_, I>,
        span: &SourceSpan,
    ) -> Result<Advance> {
        if let Some(closure) = &mut self.closure {
            let callback = self
                .callback
                .ok_or_else(|| state.invalid("Missing closure successor", Some(span)))?;
            return closure.advance(state, self.input, callback, self.returned.take(), span);
        }
        if let Some(sorting) = &mut self.sorting {
            let callback = self
                .callback
                .ok_or_else(|| state.invalid("Missing sorting callback", Some(span)))?;
            return sorting.advance(state, self.input, callback, self.returned.take(), span);
        }
        if let Some(value) = self.returned.take() {
            match self.operation {
                CollectionOp::GroupBy => {
                    let grouping = self.grouping.as_mut().ok_or_else(|| {
                        state.invalid("Missing grouping continuation", Some(span))
                    })?;
                    grouping.key(state, value, span)?;
                }
                CollectionOp::Map => self.output.append(state, value, span)?,
                CollectionOp::FlatMap => {
                    if !matches!(state.node(value)?.data, Data::Collection(_)) {
                        return Err(
                            state.invalid("FlatMap result requires collection data", Some(span))
                        );
                    }
                    self.inner = Some((value, 0));
                }
                CollectionOp::Fold => {
                    state.permit(state.node(value)?.ty, self.ty, Some(span))?;
                    self.accumulator = Some(value);
                }
                operation => {
                    let Data::Scalar(result) = &state.node(value)?.data else {
                        return Err(state.invalid("Predicate requires Bool data", Some(span)));
                    };
                    let accepted = result.payload.as_bool().ok_or_else(|| {
                        state.invalid("Predicate requires Bool payload", Some(span))
                    })?;
                    let current = self
                        .current
                        .ok_or_else(|| state.invalid("Missing collection item", Some(span)))?;
                    match operation {
                        CollectionOp::Filter if accepted => {
                            self.output.append(state, current, span)?
                        }
                        CollectionOp::Exists if accepted => return self.boolean(state, true, span),
                        CollectionOp::ForAll if !accepted => {
                            return self.boolean(state, false, span);
                        }
                        CollectionOp::One if accepted => {
                            self.matches += 1;
                            if self.matches == 2 {
                                return self.boolean(state, false, span);
                            }
                        }
                        CollectionOp::Find if accepted => {
                            return Ok(Advance::Done(state.alloc(
                                self.ty,
                                Data::Some(current),
                                Some(span),
                            )?));
                        }
                        _ => {}
                    }
                }
            }
            return Ok(Advance::Pending);
        }
        if let Some(grouping) = &mut self.grouping
            && grouping.pending()
        {
            let current = self
                .current
                .ok_or_else(|| state.invalid("Missing grouping member", Some(span)))?;
            grouping.add_member(state, current, span)?;
            return Ok(Advance::Pending);
        }
        if let Some((inner, cursor)) = self.inner {
            let Data::Collection(items) = &state.node(inner)?.data else {
                return Err(state.invalid("Invalid flatMap continuation", Some(span)));
            };
            if let Some(value) = items.get(cursor).copied() {
                self.output.append(state, value, span)?;
                self.inner = Some((inner, cursor + 1));
            } else {
                self.inner = None;
            }
            return Ok(Advance::Pending);
        }
        let Data::Collection(items) = &state.node(self.input)?.data else {
            return Err(state.invalid("Invalid collection input", Some(span)));
        };
        if let Some(value) = items.get(self.cursor).copied() {
            self.cursor += 1;
            self.current = Some(value);
            let Some(callback) = self.callback else {
                // Flatten shares flatMap's one-output-per-tick continuation without
                // manufacturing a callback or a second evaluator.
                if self.operation != CollectionOp::FlatMap
                    || !matches!(state.node(value)?.data, Data::Collection(_))
                {
                    return Err(state.invalid("Invalid flatten element", Some(span)));
                }
                self.inner = Some((value, 0));
                return Ok(Advance::Pending);
            };
            let TypeNode::Function { result, .. } = state.type_node(state.node(callback)?.ty)?
            else {
                return Err(state.invalid("Collection callback is not a function", Some(span)));
            };
            let result = *result;
            let mut arguments = Vec::with_capacity(if self.operation == CollectionOp::Fold {
                2
            } else {
                1
            });
            if self.operation == CollectionOp::Fold {
                let seed = self
                    .accumulator
                    .ok_or_else(|| state.invalid("Missing fold accumulator", Some(span)))?;
                state.transfer(seed, Some(span))?;
                arguments.push(seed);
            }
            state.transfer(value, Some(span))?;
            arguments.push(value);
            return Ok(Advance::Call {
                callback,
                arguments,
                result,
            });
        }
        match self.operation {
            CollectionOp::GroupBy => {
                let grouping = self
                    .grouping
                    .as_mut()
                    .ok_or_else(|| state.invalid("Missing grouping output", Some(span)))?;
                if let Some(pair) = grouping.publish(state, span)? {
                    self.output.append(state, pair, span)?;
                    Ok(Advance::Pending)
                } else {
                    Ok(Advance::Done(self.output.finish(state, span)?))
                }
            }
            CollectionOp::Fold => {
                let value = self
                    .accumulator
                    .ok_or_else(|| state.invalid("Missing fold seed", Some(span)))?;
                Ok(Advance::Done(state.view(value, self.ty, Some(span))?))
            }
            CollectionOp::Exists => self.boolean(state, false, span),
            CollectionOp::ForAll => self.boolean(state, true, span),
            CollectionOp::One => self.boolean(state, self.matches == 1, span),
            CollectionOp::Find => Ok(Advance::Done(state.alloc(
                self.ty,
                Data::None,
                Some(span),
            )?)),
            _ => {
                // Elements were checked and measured at each append. Filter retains a
                // subset of a validated input; map/flatMap never promise uniqueness.
                Ok(Advance::Done(self.output.finish(state, span)?))
            }
        }
    }
}

/// One budgeted output builder shared by callbacks, flatten and collection algebra.
pub(super) struct CollectionOutput {
    ty: TypeId,
    values: Vec<Id>,
    bytes: usize,
    depth: usize,
}
impl CollectionOutput {
    pub(super) fn new(ty: TypeId) -> Self {
        Self {
            ty,
            values: Vec::new(),
            bytes: 32,
            depth: 1,
        }
    }
    pub(super) fn item(&self, index: usize) -> Option<Id> {
        self.values.get(index).copied()
    }
    pub(super) fn append<I: super::inputs::InputFamily>(
        &mut self,
        state: &mut State<'_, '_, '_, I>,
        value: Id,
        span: &SourceSpan,
    ) -> Result<()> {
        let (_, element, _, _) = state.collection_info(self.ty)?;
        state.permit(state.node(value)?.ty, element, Some(span))?;
        if self.values.len() >= state.limits.max_value_nodes {
            return Err(state.resource("Collection output item limit exceeded", Some(span)));
        }
        let bytes = self
            .bytes
            .checked_add(state.node(value)?.bytes)
            .ok_or_else(|| state.resource("Collection output size overflow", Some(span)))?;
        let depth = self.depth.max(state.node(value)?.depth.saturating_add(1));
        state.check_allocation(bytes, depth, Some(span))?;
        state.transfer(value, Some(span))?;
        if self.values.len() == self.values.capacity() {
            let capacity = self
                .values
                .capacity()
                .saturating_mul(2)
                .max(1)
                .min(state.limits.max_value_nodes);
            let additional = capacity.saturating_sub(self.values.len());
            let storage = additional
                .checked_mul(8)
                .ok_or_else(|| state.resource("Collection output storage overflow", Some(span)))?;
            state.collection_storage(storage, span)?;
            self.values
                .try_reserve_exact(additional)
                .map_err(|_| state.resource("Collection output allocation failed", Some(span)))?;
        }
        self.values.push(value);
        self.bytes = bytes;
        self.depth = depth;
        Ok(())
    }

    pub(super) fn finish<I: super::inputs::InputFamily>(
        &mut self,
        state: &mut State<'_, '_, '_, I>,
        span: &SourceSpan,
    ) -> Result<Id> {
        state.alloc_measured(
            self.ty,
            Data::Collection(std::mem::take(&mut self.values)),
            self.bytes,
            self.depth,
            Some(span),
        )
    }
}
