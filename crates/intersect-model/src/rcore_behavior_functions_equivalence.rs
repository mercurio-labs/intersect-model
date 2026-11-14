//! One stepped structural equivalence authority for synchronous and suspended callers.
use super::{
    machine::State,
    values::{Data, Id},
    *,
};
use intersect_model::definitions::rcore_behavior::types::CollectionKind;
use std::ops::Bound::{Excluded, Unbounded};

enum Frame {
    Compare {
        a: Id,
        b: Id,
        depth: usize,
    },
    Ordered {
        a: Id,
        b: Id,
        index: usize,
        len: usize,
        depth: usize,
    },
    Record {
        a: Id,
        b: Id,
        last: Option<String>,
        depth: usize,
    },
    Unordered {
        a: Id,
        b: Id,
        needle: usize,
        probe: usize,
        len: usize,
        matched: Vec<bool>,
        waiting: bool,
        depth: usize,
    },
}
pub(super) struct Equivalence {
    frames: Vec<Frame>,
    returned: Option<bool>,
    total: bool,
}
impl Equivalence {
    pub(super) fn new<I: super::inputs::InputFamily>(
        state: &mut State<'_, '_, '_, I>,
        a: Id,
        b: Id,
        total: bool,
        depth: usize,
        span: Option<&SourceSpan>,
    ) -> Result<Self> {
        state.retained_storage(64, span)?;
        let mut value = Self {
            frames: Vec::new(),
            returned: None,
            total,
        };
        value.push(state, Frame::Compare { a, b, depth }, span)?;
        Ok(value)
    }
    fn push<I: super::inputs::InputFamily>(
        &mut self,
        state: &mut State<'_, '_, '_, I>,
        frame: Frame,
        span: Option<&SourceSpan>,
    ) -> Result<()> {
        if self.frames.len() == self.frames.capacity() {
            let bound = state.limits.max_value_depth.saturating_add(2);
            let capacity = self.frames.capacity().saturating_mul(2).max(1).min(bound);
            let additional = capacity
                .checked_sub(self.frames.len())
                .filter(|n| *n > 0)
                .ok_or_else(|| state.resource("Comparison stack limit exceeded", span))?;
            state.retained_storage(
                additional
                    .checked_mul(128)
                    .ok_or_else(|| state.resource("Comparison storage overflow", span))?,
                span,
            )?;
            self.frames
                .try_reserve_exact(additional)
                .map_err(|_| state.resource("Comparison allocation failed", span))?;
        }
        self.frames.push(frame);
        Ok(())
    }
    pub(super) fn advance<I: super::inputs::InputFamily>(
        &mut self,
        state: &mut State<'_, '_, '_, I>,
        span: Option<&SourceSpan>,
    ) -> Result<Option<bool>> {
        self.advance_numeric(state, span, None)
    }
    pub(super) fn advance_numeric<I: super::inputs::InputFamily>(
        &mut self,
        state: &mut State<'_, '_, '_, I>,
        span: Option<&SourceSpan>,
        mut deferred: Option<&mut Option<(Id, Id)>>,
    ) -> Result<Option<bool>> {
        state.checkpoint(span)?;
        let frame = self
            .frames
            .pop()
            .ok_or_else(|| state.invalid("Comparison already finished", span))?;
        match frame {
            Frame::Compare { a, b, depth } => {
                if depth > state.limits.max_value_depth {
                    return Err(state.resource("Value comparison depth exceeded", span));
                }
                state.compare_charge(span)?;
                state.transfer(a, span)?;
                state.transfer(b, span)?;
                match (&state.node(a)?.data, &state.node(b)?.data) {
                    (Data::Some(a), Data::Some(b))
                    | (Data::Ok(a), Data::Ok(b))
                    | (Data::Err(a), Data::Err(b)) => {
                        let (a, b) = (*a, *b);
                        self.push(
                            state,
                            Frame::Compare {
                                a,
                                b,
                                depth: depth + 1,
                            },
                            span,
                        )?;
                    }
                    (Data::Tuple(av), Data::Tuple(bv)) => {
                        if av.len() != bv.len() {
                            self.returned = Some(false);
                        } else {
                            let len = av.len();
                            self.push(
                                state,
                                Frame::Ordered {
                                    a,
                                    b,
                                    index: 0,
                                    len,
                                    depth,
                                },
                                span,
                            )?;
                        }
                    }
                    (Data::Record(av), Data::Record(bv)) => {
                        if av.len() != bv.len() {
                            self.returned = Some(false);
                        } else {
                            self.push(
                                state,
                                Frame::Record {
                                    a,
                                    b,
                                    last: None,
                                    depth,
                                },
                                span,
                            )?;
                        }
                    }
                    (Data::Collection(av), Data::Collection(bv)) => {
                        if !self.total {
                            return Err(state.error(
                                ExecutionErrorKind::Unsupported,
                                "Source equality for collections is unsupported",
                                span,
                            ));
                        }
                        let (ak, _, _, _) = state.collection_info(state.node(a)?.ty)?;
                        let (bk, _, _, _) = state.collection_info(state.node(b)?.ty)?;
                        if ak != bk || av.len() != bv.len() {
                            self.returned = Some(false);
                        } else {
                            let len = av.len();
                            if matches!(ak, CollectionKind::Sequence | CollectionKind::OrderedSet) {
                                self.push(
                                    state,
                                    Frame::Ordered {
                                        a,
                                        b,
                                        index: 0,
                                        len,
                                        depth,
                                    },
                                    span,
                                )?;
                            } else {
                                state.retained_storage(len, span)?;
                                let mut matched = Vec::new();
                                matched.try_reserve_exact(len).map_err(|_| {
                                    state.resource("Comparison match allocation failed", span)
                                })?;
                                self.push(
                                    state,
                                    Frame::Unordered {
                                        a,
                                        b,
                                        needle: 0,
                                        probe: 0,
                                        len,
                                        matched,
                                        waiting: false,
                                        depth,
                                    },
                                    span,
                                )?;
                            }
                        }
                    }
                    _ => {
                        let selected = deferred.is_some()
                            && !self.total
                            && matches!((&state.node(a)?.data,&state.node(b)?.data),(Data::Scalar(left),Data::Scalar(right)) if left.ty==right.ty);
                        if selected {
                            if let Some(deferred) = deferred.as_mut() {
                                **deferred = Some((a, b));
                            }
                        } else {
                            self.returned = Some(state.equivalent_leaf(a, b, self.total, span)?);
                        }
                    }
                }
            }
            Frame::Ordered {
                a,
                b,
                index,
                len,
                depth,
            } => {
                if self.returned.take() == Some(false) {
                    self.returned = Some(false);
                } else if index == len {
                    self.returned = Some(true);
                } else {
                    let left = state.comparison_item(a, index, span)?;
                    let right = state.comparison_item(b, index, span)?;
                    self.push(
                        state,
                        Frame::Ordered {
                            a,
                            b,
                            index: index + 1,
                            len,
                            depth,
                        },
                        span,
                    )?;
                    self.push(
                        state,
                        Frame::Compare {
                            a: left,
                            b: right,
                            depth: depth + 1,
                        },
                        span,
                    )?;
                }
            }
            Frame::Record { a, b, last, depth } => {
                if self.returned.take() == Some(false) {
                    self.returned = Some(false);
                } else {
                    match (
                        state.comparison_field(a, last.as_deref(), span)?,
                        state.comparison_field(b, last.as_deref(), span)?,
                    ) {
                        (None, None) => self.returned = Some(true),
                        (Some((ak, av)), Some((bk, bv))) if ak == bk => {
                            let bytes = ak.len();
                            state.retained_storage(bytes, span)?;
                            let key = state
                                .comparison_field(a, last.as_deref(), span)?
                                .ok_or_else(|| state.invalid("Missing comparison field", span))?
                                .0
                                .to_owned();
                            self.push(
                                state,
                                Frame::Record {
                                    a,
                                    b,
                                    last: Some(key),
                                    depth,
                                },
                                span,
                            )?;
                            self.push(
                                state,
                                Frame::Compare {
                                    a: av,
                                    b: bv,
                                    depth: depth + 1,
                                },
                                span,
                            )?;
                        }
                        _ => self.returned = Some(false),
                    }
                }
            }
            Frame::Unordered {
                a,
                b,
                mut needle,
                mut probe,
                len,
                mut matched,
                mut waiting,
                depth,
            } => {
                if matched.len() < len {
                    matched.push(false);
                } else if waiting {
                    if self
                        .returned
                        .take()
                        .ok_or_else(|| state.invalid("Missing nested comparison result", span))?
                    {
                        matched[probe] = true;
                        needle += 1;
                        probe = 0;
                    } else {
                        probe += 1;
                    }
                    waiting = false;
                } else if needle == len {
                    self.returned = Some(true);
                    return Ok(if self.frames.is_empty() {
                        self.returned.take()
                    } else {
                        None
                    });
                } else if probe == len {
                    self.returned = Some(false);
                    return Ok(if self.frames.is_empty() {
                        self.returned.take()
                    } else {
                        None
                    });
                } else if matched[probe] {
                    probe += 1;
                } else {
                    let left = state.comparison_item(a, needle, span)?;
                    let right = state.comparison_item(b, probe, span)?;
                    self.push(
                        state,
                        Frame::Unordered {
                            a,
                            b,
                            needle,
                            probe,
                            len,
                            matched,
                            waiting: true,
                            depth,
                        },
                        span,
                    )?;
                    self.push(
                        state,
                        Frame::Compare {
                            a: left,
                            b: right,
                            depth: depth + 1,
                        },
                        span,
                    )?;
                    return Ok(None);
                }
                self.push(
                    state,
                    Frame::Unordered {
                        a,
                        b,
                        needle,
                        probe,
                        len,
                        matched,
                        waiting,
                        depth,
                    },
                    span,
                )?;
            }
        }
        Ok(if self.frames.is_empty() {
            self.returned.take()
        } else {
            None
        })
    }
}
impl<I: super::inputs::InputFamily> State<'_, '_, '_, I> {
    pub(super) fn poll_equivalence(
        &mut self,
        pending: &mut Option<Equivalence>,
        a: Id,
        b: Id,
        span: &SourceSpan,
    ) -> Result<Option<bool>> {
        if pending.is_none() {
            *pending = Some(Equivalence::new(self, a, b, true, 0, Some(span))?);
        }
        let result = pending
            .as_mut()
            .ok_or_else(|| self.invalid("Missing comparison continuation", Some(span)))?
            .advance(self, Some(span))?;
        if result.is_some() {
            *pending = None;
        }
        Ok(result)
    }

    fn comparison_item(&self, id: Id, index: usize, span: Option<&SourceSpan>) -> Result<Id> {
        match &self.node(id)?.data {
            Data::Collection(v) | Data::Tuple(v) => v
                .get(index)
                .copied()
                .ok_or_else(|| self.invalid("Invalid comparison cursor", span)),
            _ => Err(self.invalid("Invalid comparison sequence", span)),
        }
    }
    fn comparison_field<'a>(
        &'a self,
        id: Id,
        last: Option<&str>,
        span: Option<&SourceSpan>,
    ) -> Result<Option<(&'a str, Id)>> {
        let Data::Record(fields) = &self.node(id)?.data else {
            return Err(self.invalid("Invalid comparison record", span));
        };
        let next = match last {
            Some(last) => fields.range::<str, _>((Excluded(last), Unbounded)).next(),
            None => fields.iter().next(),
        };
        Ok(next.map(|(key, value)| (key.as_str(), *value)))
    }
}

pub(super) struct InstructionComparison {
    comparison: Equivalence,
    site: super::machine::CallSite,
    ty: intersect_model::definitions::rcore_behavior::ir_v2::TypeId,
    equal: bool,
}
impl<I: super::inputs::InputFamily> State<'_, '_, '_, I> {
    pub(super) fn start_comparison(
        &mut self,
        a: Id,
        b: Id,
        ty: intersect_model::definitions::rcore_behavior::ir_v2::TypeId,
        equal: bool,
        site: super::machine::CallSite,
    ) -> Result<()> {
        let span = self
            .image
            .call_span(site)
            .ok_or_else(|| self.invalid("Missing equality source", None))?;
        let comparison = Equivalence::new(self, a, b, false, 0, Some(span))?;
        if let Some(frame) = self.data.frames.last_mut() {
            frame.set_comparison(InstructionComparison {
                comparison,
                site,
                ty,
                equal,
            });
        } else {
            return Err(self.invalid("Missing comparison frame", Some(span)));
        }
        Ok(())
    }
    pub(super) fn advance_comparison(&mut self) -> Result<()> {
        let mut pending = self
            .data
            .frames
            .last_mut()
            .and_then(|frame| frame.take_comparison())
            .ok_or_else(|| self.invalid("Missing comparison continuation", None))?;
        let span = self
            .image
            .call_span(pending.site)
            .ok_or_else(|| self.invalid("Missing equality source", None))?;
        self.step(span)?;
        if let Some(equal) = pending.comparison.advance(self, Some(span))? {
            use intersect_model::definitions::rcore_behavior::{ir, types::Scalar};
            let id = self.alloc(
                pending.ty,
                Data::Scalar(ir::Value {
                    ty: ir::ValueType::Scalar(Scalar::Bool),
                    payload: serde_json::json!(equal == pending.equal),
                }),
                Some(span),
            )?;
            if let Some(frame) = self.data.frames.last_mut() {
                frame.registers.push(id);
            } else {
                return Err(self.invalid("Missing comparison result frame", Some(span)));
            }
        } else if let Some(frame) = self.data.frames.last_mut() {
            frame.set_comparison(pending);
        } else {
            return Err(self.invalid("Missing comparison continuation frame", Some(span)));
        }
        Ok(())
    }
}
