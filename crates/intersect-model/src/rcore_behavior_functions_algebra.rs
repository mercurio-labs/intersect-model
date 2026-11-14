//! Handwritten collection algebra using shared equivalence and budgeted outputs.
use super::{
    collections::CollectionOutput,
    machine::{CallSite, State},
    values::{Data, Id},
    *,
};
use intersect_model::definitions::rcore_behavior::{
    ir_v2::{AlgebraOp, TypeId},
    types::CollectionKind,
};
enum Phase {
    InitMatches,
    InitKeep,
    Scan,
    Publish,
}
pub(super) struct AlgebraState {
    comparison: Option<super::equivalence::Equivalence>,
    operation: AlgebraOp,
    left: Id,
    right: Option<Id>,
    site: CallSite,
    output: CollectionOutput,
    left_len: usize,
    right_len: usize,
    index: usize,
    probe: usize,
    current: Option<Id>,
    second: bool,
    unique: bool,
    matched: Vec<bool>,
    keep: Vec<bool>,
    phase: Phase,
}
impl<I: super::inputs::InputFamily> State<'_, '_, '_, I> {
    fn algebra_len(&self, id: Id, span: &SourceSpan) -> Result<usize> {
        match &self.node(id)?.data {
            Data::Collection(items) => Ok(items.len()),
            _ => Err(self.invalid("Algebra requires collection data", Some(span))),
        }
    }
    fn algebra_item(&self, id: Id, index: usize, span: &SourceSpan) -> Result<Id> {
        match &self.node(id)?.data {
            Data::Collection(items) => items
                .get(index)
                .copied()
                .ok_or_else(|| self.invalid("Invalid algebra cursor", Some(span))),
            _ => Err(self.invalid("Algebra requires collection data", Some(span))),
        }
    }
    pub(super) fn start_algebra(
        &mut self,
        operation: AlgebraOp,
        left: Id,
        right: Option<Id>,
        ty: TypeId,
        site: CallSite,
    ) -> Result<()> {
        let span = self
            .image
            .call_span(site)
            .ok_or_else(|| self.invalid("Missing algebra source", None))?;
        let left_len = self.algebra_len(left, span)?;
        let right_len = right
            .map(|id| self.algebra_len(id, span))
            .transpose()?
            .unwrap_or(0);
        let matching = matches!(operation, AlgebraOp::Intersection | AlgebraOp::Difference);
        let matching_bytes = if matching { right_len } else { 0 };
        let keep_bytes = if operation == AlgebraOp::Difference {
            left_len
        } else {
            0
        };
        let bytes = 256usize
            .checked_add(matching_bytes)
            .and_then(|n| n.checked_add(keep_bytes))
            .ok_or_else(|| self.resource("Algebra storage overflow", Some(span)))?;
        self.collection_storage(bytes, span)?;
        let mut matched = Vec::new();
        matched
            .try_reserve_exact(matching_bytes)
            .map_err(|_| self.resource("Algebra match allocation failed", Some(span)))?;
        let mut keep = Vec::new();
        keep.try_reserve_exact(keep_bytes)
            .map_err(|_| self.resource("Algebra survivor allocation failed", Some(span)))?;
        let (kind, _, _, _) = self.collection_info(ty)?;
        let unique = matches!(kind, CollectionKind::Set | CollectionKind::OrderedSet);
        let state = AlgebraState {
            comparison: None,
            operation,
            left,
            right,
            site,
            output: CollectionOutput::new(ty),
            left_len,
            right_len,
            index: if operation == AlgebraOp::Difference {
                left_len
            } else {
                0
            },
            probe: 0,
            current: None,
            second: false,
            unique,
            matched,
            keep,
            phase: if matching {
                Phase::InitMatches
            } else {
                Phase::Scan
            },
        };
        let Some(frame) = self.data.frames.last_mut() else {
            return Err(self.invalid("Missing algebra frame", Some(span)));
        };
        frame.set_algebra(state);
        Ok(())
    }
    pub(super) fn advance_algebra(&mut self) -> Result<()> {
        let mut algebra = self
            .data
            .frames
            .last_mut()
            .and_then(|frame| frame.take_algebra())
            .ok_or_else(|| self.invalid("Missing algebra continuation", None))?;
        let span = self
            .image
            .call_span(algebra.site)
            .ok_or_else(|| self.invalid("Missing algebra source", None))?;
        self.step(span)?;
        let result = algebra.advance(self, span)?;
        let Some(frame) = self.data.frames.last_mut() else {
            return Err(self.invalid("Missing algebra result frame", Some(span)));
        };
        if let Some(result) = result {
            frame.registers.push(result);
        } else {
            frame.set_algebra(algebra);
        }
        Ok(())
    }
}
impl AlgebraState {
    fn advance<I: super::inputs::InputFamily>(
        &mut self,
        state: &mut State<'_, '_, '_, I>,
        span: &SourceSpan,
    ) -> Result<Option<Id>> {
        match self.phase {
            Phase::InitMatches => {
                if self.matched.len() < self.right_len {
                    self.matched.push(false);
                } else {
                    self.phase = if self.operation == AlgebraOp::Difference {
                        Phase::InitKeep
                    } else {
                        Phase::Scan
                    };
                }
                return Ok(None);
            }
            Phase::InitKeep => {
                if self.keep.len() < self.left_len {
                    self.keep.push(false);
                } else {
                    self.phase = Phase::Scan;
                }
                return Ok(None);
            }
            Phase::Publish => {
                if self.index == self.left_len {
                    return self.output.finish(state, span).map(Some);
                }
                let index = self.index;
                self.index += 1;
                if self.keep[index] {
                    let value = state.algebra_item(self.left, index, span)?;
                    self.output.append(state, value, span)?;
                }
                return Ok(None);
            }
            Phase::Scan => {}
        }
        if matches!(
            self.operation,
            AlgebraOp::Intersection | AlgebraOp::Difference
        ) {
            return self.matching(state, span);
        }
        if let Some(current) = self.current {
            if let Some(prior) = self.output.item(self.probe) {
                let Some(equal) =
                    state.poll_equivalence(&mut self.comparison, current, prior, span)?
                else {
                    return Ok(None);
                };
                self.probe += 1;
                if equal {
                    self.current = None;
                }
            } else {
                self.output.append(state, current, span)?;
                self.current = None;
            }
            return Ok(None);
        }
        let input = if self.second {
            self.right
                .ok_or_else(|| state.invalid("Missing algebra right operand", Some(span)))?
        } else {
            self.left
        };
        let len = if self.second {
            self.right_len
        } else {
            self.left_len
        };
        if self.index == len {
            if !self.second && self.right.is_some() {
                self.second = true;
                self.index = 0;
                return Ok(None);
            }
            return self.output.finish(state, span).map(Some);
        }
        let value = state.algebra_item(input, self.index, span)?;
        self.index += 1;
        if self.unique {
            self.current = Some(value);
            self.probe = 0;
        } else {
            self.output.append(state, value, span)?;
        }
        Ok(None)
    }
    fn matching<I: super::inputs::InputFamily>(
        &mut self,
        state: &mut State<'_, '_, '_, I>,
        span: &SourceSpan,
    ) -> Result<Option<Id>> {
        let difference = self.operation == AlgebraOp::Difference;
        let Some(current) = self.current else {
            if difference && self.index == 0 {
                self.phase = Phase::Publish;
                return Ok(None);
            }
            if !difference && self.index == self.left_len {
                return self.output.finish(state, span).map(Some);
            }
            // Remove matching occurrences from the rightmost left values first,
            // then publish retained values in forward order: earliest representatives survive.
            let index = if difference {
                self.index -= 1;
                self.index
            } else {
                let index = self.index;
                self.index += 1;
                index
            };
            self.current = Some(state.algebra_item(self.left, index, span)?);
            self.probe = 0;
            return Ok(None);
        };
        if self.probe == self.right_len {
            if difference {
                self.keep[self.index] = true;
            }
            self.current = None;
            return Ok(None);
        }
        let index = self.probe;
        if !self.matched[index] {
            let right = self
                .right
                .ok_or_else(|| state.invalid("Missing algebra right operand", Some(span)))?;
            let candidate = state.algebra_item(right, index, span)?;
            let Some(equal) =
                state.poll_equivalence(&mut self.comparison, current, candidate, span)?
            else {
                return Ok(None);
            };
            if equal {
                self.matched[index] = true;
                if !difference {
                    self.output.append(state, current, span)?;
                }
                self.current = None;
            }
        }
        self.probe += 1;
        Ok(None)
    }
}
