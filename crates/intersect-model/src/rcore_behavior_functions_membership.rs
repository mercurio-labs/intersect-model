//! Handwritten resumable membership over the shared checked collection equivalence.
use super::{
    machine::{CallSite, State},
    values::{Data, Id},
    *,
};
use intersect_model::definitions::rcore_behavior::{
    ir,
    ir_v2::{MembershipOp, TypeId},
    types::Scalar,
};
pub(super) struct MembershipState {
    comparison: Option<super::equivalence::Equivalence>,
    operation: MembershipOp,
    value: Id,
    other: Id,
    ty: TypeId,
    site: CallSite,
    needle: usize,
    candidate: usize,
    count: usize,
}
impl<I: super::inputs::InputFamily> State<'_, '_, '_, I> {
    pub(super) fn start_membership(
        &mut self,
        operation: MembershipOp,
        value: Id,
        other: Id,
        ty: TypeId,
        site: CallSite,
    ) -> Result<()> {
        let span = self
            .image
            .call_span(site)
            .ok_or_else(|| self.invalid("Missing membership source", None))?;
        self.collection_storage(96, span)?;
        let state = MembershipState {
            comparison: None,
            operation,
            value,
            other,
            ty,
            site,
            needle: 0,
            candidate: 0,
            count: 0,
        };
        let Some(frame) = self.data.frames.last_mut() else {
            return Err(self.invalid("Missing membership frame", Some(span)));
        };
        frame.membership = Some(state);
        Ok(())
    }
    pub(super) fn advance_membership(&mut self) -> Result<()> {
        let mut membership = self
            .data
            .frames
            .last_mut()
            .and_then(|f| f.membership.take())
            .ok_or_else(|| self.invalid("Missing membership continuation", None))?;
        let span = self
            .image
            .call_span(membership.site)
            .ok_or_else(|| self.invalid("Missing membership source", None))?;
        self.step(span)?;
        let outcome = membership.advance(self, span)?;
        let Some(frame) = self.data.frames.last_mut() else {
            return Err(self.invalid("Missing membership result frame", Some(span)));
        };
        if let Some(value) = outcome {
            frame.registers.push(value);
        } else {
            frame.membership = Some(membership);
        }
        Ok(())
    }
}
impl MembershipState {
    fn boolean<I: super::inputs::InputFamily>(
        &self,
        state: &mut State<'_, '_, '_, I>,
        value: bool,
        span: &SourceSpan,
    ) -> Result<Option<Id>> {
        Ok(Some(state.alloc(
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
    ) -> Result<Option<Id>> {
        let needle = if self.operation.multiple() {
            let Data::Collection(items) = &state.node(self.other)?.data else {
                return Err(state.invalid("Membership requires collection needles", Some(span)));
            };
            match items.get(self.needle).copied() {
                Some(value) => value,
                None => {
                    return self.boolean(state, self.operation == MembershipOp::IncludesAll, span);
                }
            }
        } else {
            self.other
        };
        let Data::Collection(items) = &state.node(self.value)?.data else {
            return Err(state.invalid("Membership requires collection candidates", Some(span)));
        };
        let Some(candidate) = items.get(self.candidate).copied() else {
            return match self.operation {
                MembershipOp::Count => {
                    let count = i64::try_from(self.count).map_err(|_| {
                        state.resource("Occurrence count exceeds Int64", Some(span))
                    })?;
                    Ok(Some(state.alloc(
                        self.ty,
                        Data::Scalar(ir::Value {
                            ty: ir::ValueType::Scalar(Scalar::Int64),
                            payload: serde_json::json!({"kind":"int64","value":count.to_string()}),
                        }),
                        Some(span),
                    )?))
                }
                MembershipOp::Excludes => self.boolean(state, true, span),
                MembershipOp::IncludesAny => {
                    self.needle += 1;
                    self.candidate = 0;
                    Ok(None)
                }
                _ => self.boolean(state, false, span),
            };
        };
        let Some(equal) = state.poll_equivalence(&mut self.comparison, candidate, needle, span)?
        else {
            return Ok(None);
        };
        self.candidate += 1;
        if equal {
            match self.operation {
                MembershipOp::Count => self.count += 1,
                MembershipOp::Contains | MembershipOp::IncludesAny => {
                    return self.boolean(state, true, span);
                }
                MembershipOp::Excludes => return self.boolean(state, false, span),
                MembershipOp::IncludesAll => {
                    self.needle += 1;
                    self.candidate = 0;
                }
            }
        }
        Ok(None)
    }
}
