//! Deterministic stable bottom-up merge sorting, stepped by the common frame machine.
use super::{
    collections::{Advance, CollectionOutput},
    machine::State,
    values::{Data, Id},
    *,
};
use intersect_model::definitions::rcore_behavior::ir_v2::{TypeId, TypeNode};

pub(super) struct Sorting {
    by_key: bool,
    keys: Vec<Id>,
    scratch_keys: Vec<Id>,
    source: Vec<Id>,
    scratch: Vec<Id>,
    len: usize,
    width: usize,
    start: usize,
    left: usize,
    right: usize,
    middle: usize,
    end: usize,
    write: usize,
    waiting: bool,
    publish: usize,
    output: CollectionOutput,
}
impl Sorting {
    pub(super) fn new<I: super::inputs::InputFamily>(
        state: &mut State<'_, '_, '_, I>,
        input: Id,
        ty: TypeId,
        by_key: bool,
        span: &SourceSpan,
    ) -> Result<Self> {
        let Data::Collection(values) = &state.node(input)?.data else {
            return Err(state.invalid("Sorting requires collection input", Some(span)));
        };
        let len = values.len();
        let storage = len
            .checked_mul(if by_key { 32 } else { 16 })
            .and_then(|n| n.checked_add(192))
            .ok_or_else(|| state.resource("Sorting storage overflow", Some(span)))?;
        state.collection_storage(storage, span)?;
        let mut source = Vec::new();
        let mut scratch = Vec::new();
        source
            .try_reserve_exact(len)
            .map_err(|_| state.resource("Sorting allocation failed", Some(span)))?;
        scratch
            .try_reserve_exact(len)
            .map_err(|_| state.resource("Sorting allocation failed", Some(span)))?;
        let mut keys = Vec::new();
        let mut scratch_keys = Vec::new();
        if by_key {
            keys.try_reserve_exact(len)
                .map_err(|_| state.resource("Sort key allocation failed", Some(span)))?;
            scratch_keys
                .try_reserve_exact(len)
                .map_err(|_| state.resource("Sort key allocation failed", Some(span)))?;
        }
        Ok(Self {
            by_key,
            keys,
            scratch_keys,
            source,
            scratch,
            len,
            width: 1,
            start: 0,
            left: 0,
            right: 1.min(len),
            middle: 1.min(len),
            end: 2.min(len),
            write: 0,
            waiting: false,
            publish: 0,
            output: CollectionOutput::new(ty),
        })
    }
    fn emit(&mut self, from_left: bool) {
        let cursor = if from_left {
            &mut self.left
        } else {
            &mut self.right
        };
        self.scratch[self.write] = self.source[*cursor];
        if self.by_key {
            self.scratch_keys[self.write] = self.keys[*cursor];
        }
        *cursor += 1;
        self.write += 1;
    }
    fn block(&mut self) {
        self.left = self.start;
        self.middle = self.start.saturating_add(self.width).min(self.len);
        self.right = self.middle;
        self.end = self.middle.saturating_add(self.width).min(self.len);
        self.write = self.start;
    }
    pub(super) fn advance<I: super::inputs::InputFamily>(
        &mut self,
        state: &mut State<'_, '_, '_, I>,
        input: Id,
        callback: Id,
        returned: Option<Id>,
        span: &SourceSpan,
    ) -> Result<Advance> {
        if let Some(value) = returned {
            if !self.waiting {
                return Err(state.invalid("Unexpected sorting callback return", Some(span)));
            }
            if self.by_key && self.source.len() < self.len {
                let Data::Collection(items) = &state.node(input)?.data else {
                    return Err(state.invalid("Invalid sorting input", Some(span)));
                };
                let item = items[self.source.len()];
                state.transfer(value, Some(span))?;
                self.source.push(item);
                self.scratch.push(item);
                self.keys.push(value);
                self.scratch_keys.push(value);
                self.waiting = false;
                return Ok(Advance::Pending);
            }
            let Data::Scalar(value) = &state.node(value)?.data else {
                return Err(state.invalid("Comparator requires Int data", Some(span)));
            };
            let comparison = value
                .payload
                .get("value")
                .and_then(|v| v.as_str())
                .and_then(|v| v.parse::<i64>().ok())
                .ok_or_else(|| state.invalid("Comparator requires Int payload", Some(span)))?;
            // Choosing the left occurrence on zero is the stability guarantee.
            self.emit(comparison <= 0);
            self.waiting = false;
            return Ok(Advance::Pending);
        }
        if self.waiting {
            return Err(state.invalid("Missing sorting callback return", Some(span)));
        }
        if self.source.len() < self.len {
            let Data::Collection(values) = &state.node(input)?.data else {
                return Err(state.invalid("Invalid sorting input", Some(span)));
            };
            let value = values[self.source.len()];
            state.transfer(value, Some(span))?;
            if self.by_key {
                let TypeNode::Function { result, .. } =
                    state.type_node(state.node(callback)?.ty)?
                else {
                    return Err(state.invalid("Sort key must be callable", Some(span)));
                };
                self.waiting = true;
                return Ok(Advance::Call {
                    callback,
                    arguments: vec![value],
                    result: *result,
                });
            }
            self.source.push(value);
            self.scratch.push(value);
            return Ok(Advance::Pending);
        }
        if self.width >= self.len {
            if let Some(value) = self.source.get(self.publish).copied() {
                self.output.append(state, value, span)?;
                self.publish += 1;
                return Ok(Advance::Pending);
            }
            return Ok(Advance::Done(self.output.finish(state, span)?));
        }
        if self.write == self.end {
            self.start = self.end;
            if self.start == self.len {
                std::mem::swap(&mut self.source, &mut self.scratch);
                if self.by_key {
                    std::mem::swap(&mut self.keys, &mut self.scratch_keys);
                }
                self.width = self.width.saturating_mul(2);
                self.start = 0;
            }
            self.block();
            return Ok(Advance::Pending);
        }
        if self.left == self.middle {
            self.emit(false);
            return Ok(Advance::Pending);
        }
        if self.right == self.end {
            self.emit(true);
            return Ok(Advance::Pending);
        }
        state.compare_charge(Some(span))?;
        if self.by_key {
            let a = self.keys[self.left];
            let b = self.keys[self.right];
            state.transfer(a, Some(span))?;
            state.transfer(b, Some(span))?;
            let Data::Scalar(a) = &state.node(a)?.data else {
                return Err(state.invalid("Invalid scalar sort key", Some(span)));
            };
            let a = a.clone();
            let Data::Scalar(b) = &state.node(b)?.data else {
                return Err(state.invalid("Invalid scalar sort key", Some(span)));
            };
            let b = b.clone();
            let control = state.control;
            let order = crate::rcore_behavior_scalar::sort_order_controlled(
                &a,
                &b,
                &mut state.data.numeric,
                Some(&|| control.is_some_and(|c| c.cancellation_requested())),
            )
            .map_err(|e| state.arithmetic(e, span))?;
            self.emit(order != std::cmp::Ordering::Greater);
            return Ok(Advance::Pending);
        }
        let left = self.source[self.left];
        let right = self.source[self.right];
        state.transfer(left, Some(span))?;
        state.transfer(right, Some(span))?;
        let TypeNode::Function { result, .. } = state.type_node(state.node(callback)?.ty)? else {
            return Err(state.invalid("Sorting callback must be callable", Some(span)));
        };
        self.waiting = true;
        Ok(Advance::Call {
            callback,
            arguments: vec![left, right],
            result: *result,
        })
    }
}
