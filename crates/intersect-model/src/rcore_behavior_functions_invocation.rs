//! Handwritten invocation preparation shared by ordinary and collection calls.
use super::*;

enum Stage {
    Transfer,
    Frame,
    Parameters(usize),
    Captures(usize),
    Ready,
}
pub(super) struct Invocation {
    pub(super) site: CallSite,
    closure: usize,
    function: FunctionId,
    arguments: Vec<Id>,
    captures: Vec<Id>,
    result: TypeId,
    stage: Stage,
}
#[cfg(test)]
impl Invocation {
    pub(super) fn progress(&self) -> (usize, usize) {
        let checked = match self.stage {
            Stage::Transfer | Stage::Frame => 0,
            Stage::Parameters(index) => index,
            Stage::Captures(index) => self.arguments.len() + index,
            Stage::Ready => self.arguments.len() + self.captures.len(),
        };
        (self.captures.len(), checked)
    }
}
impl<I: super::inputs::InputFamily> State<'_, '_, '_, I> {
    pub(in crate::rcore_behavior_functions) fn invoke_closure(
        &mut self,
        id: Id,
        arguments: Vec<Id>,
        site: CallSite,
        result: TypeId,
    ) -> Result<()> {
        let span = self
            .image
            .call_span(site)
            .ok_or_else(|| self.invalid("Missing call site", None))?;
        let Data::Closure(index) = self.node(id)?.data else {
            return Err(self.invalid("Call requires a private closure value", Some(span)));
        };
        let index = *index;
        let closure = self
            .data
            .closures
            .get(index)
            .ok_or_else(|| self.invalid("Unknown closure", Some(span)))?;
        let function = closure.function;
        let capture_count = closure.captures.len();
        self.data.consumed.captures = self
            .data
            .consumed
            .captures
            .checked_add(capture_count)
            .filter(|n| *n <= self.limits.max_captures)
            .ok_or_else(|| {
                self.resource("Cumulative capture transfer limit exceeded", Some(span))
            })?;
        self.retained_storage(128, Some(span))?;
        let pending = Invocation {
            site,
            closure: index,
            function,
            arguments,
            captures: Vec::new(),
            result,
            stage: Stage::Transfer,
        };
        let Some(frame) = self.data.frames.last_mut() else {
            return Err(self.invalid("Missing invocation frame", Some(span)));
        };
        if frame.invocation.is_some() {
            return Err(self.invalid("Invocation already pending", Some(span)));
        }
        frame.invocation = Some(pending);
        Ok(())
    }
    pub(super) fn advance_invocation(&mut self) -> Result<()> {
        let mut pending = self
            .data
            .frames
            .last_mut()
            .and_then(|frame| frame.invocation.take())
            .ok_or_else(|| self.invalid("Missing invocation continuation", None))?;
        let image = self.image;
        let span = image
            .call_span(pending.site)
            .ok_or_else(|| self.invalid("Missing invocation source", None))?;
        self.step(span)?;
        match pending.stage {
            Stage::Transfer => {
                let closure = self
                    .data
                    .closures
                    .get(pending.closure)
                    .ok_or_else(|| self.invalid("Unknown invocation closure", Some(span)))?;
                if let Some(id) = closure.captures.get(pending.captures.len()).copied() {
                    self.transfer(id, Some(span))?;
                    if pending.captures.len() == pending.captures.capacity() {
                        self.retained_storage(8, Some(span))?;
                        pending.captures.try_reserve_exact(1).map_err(|_| {
                            self.resource("Invocation capture allocation failed", Some(span))
                        })?;
                    }
                    pending.captures.push(id);
                } else {
                    pending.stage = Stage::Frame;
                }
            }
            Stage::Frame => {
                self.frame_signature(
                    pending.function,
                    pending.arguments.len(),
                    pending.captures.len(),
                    Some(span),
                )?;
                pending.stage = Stage::Parameters(0);
            }
            Stage::Parameters(index) => {
                if let Some(id) = pending.arguments.get(index).copied() {
                    let ty = image
                        .function(pending.function.0 as usize)
                        .and_then(|target| target.parameters.get(index).copied())
                        .ok_or_else(|| {
                            self.invalid("Missing invocation parameter type", Some(span))
                        })?;
                    self.permit(self.node(id)?.ty, ty, Some(span))?;
                    pending.stage = Stage::Parameters(index + 1);
                } else {
                    pending.stage = Stage::Captures(0);
                }
            }
            Stage::Captures(index) => {
                if let Some(id) = pending.captures.get(index).copied() {
                    let ty = image
                        .function(pending.function.0 as usize)
                        .and_then(|target| target.captures.get(index).map(|capture| capture.ty))
                        .ok_or_else(|| {
                            self.invalid("Missing invocation capture type", Some(span))
                        })?;
                    self.permit(self.node(id)?.ty, ty, Some(span))?;
                    pending.stage = Stage::Captures(index + 1);
                } else {
                    pending.stage = Stage::Ready;
                }
            }
            Stage::Ready => {
                self.enter_frame(
                    pending.function,
                    pending.arguments,
                    pending.captures,
                    Some(pending.site),
                    pending.result,
                );
                return Ok(());
            }
        }
        let Some(frame) = self.data.frames.last_mut() else {
            return Err(self.invalid("Missing invocation continuation frame", Some(span)));
        };
        frame.invocation = Some(pending);
        Ok(())
    }
}
