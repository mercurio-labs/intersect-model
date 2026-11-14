//! Operand admission and transfer over immutable instruction/edge cursors.
use super::*;

#[derive(Clone, Copy)]
pub(super) enum Source {
    Instruction(CallSite),
    Edge {
        function: usize,
        block: usize,
        choice: u8,
    },
}
impl Source {
    pub(super) fn span(self, image: ExecutionImage<'_>) -> Option<&SourceSpan> {
        match self {
            Self::Instruction(site) => image.call_span(site),
            Self::Edge {
                function, block, ..
            } => image.block(function, block).map(|block| block.span),
        }
    }
    fn edge(self, image: ExecutionImage<'_>) -> Option<(u32, &[Operand])> {
        let Self::Edge {
            function,
            block,
            choice,
        } = self
        else {
            return None;
        };
        match (image.block(function, block)?.terminator, choice) {
            (Terminator::Jump(edge), 0) => Some((edge.block, &edge.arguments)),
            (Terminator::Branch { then_edge, .. }, 0) => {
                Some((then_edge.block, &then_edge.arguments))
            }
            (Terminator::Branch { else_edge, .. }, 1) => {
                Some((else_edge.block, &else_edge.arguments))
            }
            (Terminator::Option { none, .. }, 0) => Some((none.block, &none.arguments)),
            (Terminator::Option { some, .. }, 1) => Some((some.block, &some.arguments)),
            (Terminator::Result { ok, .. }, 0) => Some((ok.block, &ok.arguments)),
            (Terminator::Result { error, .. }, 1) => Some((error.block, &error.arguments)),
            _ => None,
        }
    }
    fn values(self, image: ExecutionImage<'_>) -> Option<&[Operand]> {
        match self {
            Self::Instruction(site) => {
                let instruction = image
                    .block(site.function, site.block)?
                    .instructions
                    .get(site.instruction)?;
                match instruction.kind? {
                    InstructionKind::MakeClosure { captures, .. } => Some(captures),
                    InstructionKind::Call { arguments, .. } => Some(arguments),
                    _ => None,
                }
            }
            Self::Edge { .. } => self.edge(image).map(|(_, values)| values),
        }
    }
}
enum Destination {
    Closure {
        function: FunctionId,
        ty: TypeId,
    },
    Call {
        callee: Id,
        ty: TypeId,
        site: CallSite,
    },
    Jump(u32),
}
pub(super) struct Preparation {
    pub(super) source: Source,
    destination: Destination,
    measured: usize,
    copied: usize,
    bytes: usize,
    admitted: bool,
    output: Vec<Id>,
}
#[cfg(test)]
impl Preparation {
    pub(super) fn progress(&self) -> (usize, usize) {
        (self.measured, self.copied)
    }
}
impl<I: super::inputs::InputFamily> State<'_, '_, '_, I> {
    pub(super) fn start_instruction_operands(&mut self, site: CallSite) -> Result<()> {
        let image = self.image;
        let instruction = image
            .block(site.function, site.block)
            .and_then(|block| block.instructions.get(site.instruction))
            .ok_or_else(|| self.invalid("Missing operand instruction", None))?;
        let span = instruction.span;
        let destination = match instruction.kind {
            Some(InstructionKind::MakeClosure { function, captures }) => {
                self.check_allocation(32, 1, Some(span))?;
                if self.data.closures.len() >= self.limits.max_closures {
                    return Err(self.resource("Closure count limit exceeded", Some(span)));
                }
                self.data.consumed.captures = self
                    .data
                    .consumed
                    .captures
                    .checked_add(captures.len())
                    .filter(|n| *n <= self.limits.max_captures)
                    .ok_or_else(|| self.resource("Closure capture limit exceeded", Some(span)))?;
                Destination::Closure {
                    function: *function,
                    ty: instruction.ty,
                }
            }
            Some(InstructionKind::Call { callee, .. }) => Destination::Call {
                callee: self.operand(*callee, span)?,
                ty: instruction.ty,
                site,
            },
            _ => return Err(self.invalid("Expected closure or call operands", Some(span))),
        };
        self.prepare_operands(Source::Instruction(site), destination, None)
    }
    pub(super) fn start_edge_operands(&mut self, choice: u8, payload: Option<Id>) -> Result<()> {
        let frame = self
            .data
            .frames
            .last()
            .ok_or_else(|| self.invalid("Missing jump frame", None))?;
        let source = Source::Edge {
            function: frame.function.0 as usize,
            block: frame.block,
            choice,
        };
        let span = source.span(self.image);
        let (target, _) = source
            .edge(self.image)
            .ok_or_else(|| self.invalid("Missing selected edge", span))?;
        // Preserve payload admission before the ordinary edge operand vector.
        if let Some(id) = payload {
            self.transfer(id, span)?;
        }
        self.prepare_operands(source, Destination::Jump(target), payload)
    }
    fn prepare_operands(
        &mut self,
        source: Source,
        destination: Destination,
        payload: Option<Id>,
    ) -> Result<()> {
        let span = source.span(self.image);
        let values = source
            .values(self.image)
            .ok_or_else(|| self.invalid("Missing operand source", span))?;
        if values.len() > self.limits.max_value_nodes {
            return Err(self.resource("Operand vector limit exceeded", span));
        }
        self.retained_storage(128, span)?;
        let mut pending = Preparation {
            source,
            destination,
            measured: 0,
            copied: 0,
            bytes: 0,
            admitted: false,
            output: Vec::new(),
        };
        if let Some(id) = payload {
            pending.append(self, id)?;
        }
        let Some(frame) = self.data.frames.last_mut() else {
            return Err(self.invalid("Missing operand preparation frame", span));
        };
        if frame.operands.is_some() {
            return Err(self.invalid("Operands already pending", span));
        }
        frame.operands = Some(pending);
        Ok(())
    }
    pub(super) fn advance_operands(&mut self) -> Result<()> {
        let mut pending = self
            .data
            .frames
            .last_mut()
            .and_then(|frame| frame.operands.take())
            .ok_or_else(|| self.invalid("Missing operand continuation", None))?;
        let image = self.image;
        let span = pending
            .source
            .span(image)
            .ok_or_else(|| self.invalid("Missing operand span", None))?;
        self.step(span)?;
        let values = pending
            .source
            .values(image)
            .ok_or_else(|| self.invalid("Missing operand source", Some(span)))?;
        if !pending.admitted {
            if let Some(value) = values.get(pending.measured) {
                let id = self.operand(*value, span)?;
                pending.bytes = pending
                    .bytes
                    .checked_add(self.node(id)?.bytes)
                    .ok_or_else(|| self.resource("Operand transfer size overflow", Some(span)))?;
                pending.measured += 1;
            } else {
                if self
                    .data
                    .consumed
                    .value_bytes
                    .checked_add(pending.bytes)
                    .is_none_or(|n| n > self.limits.scalar.max_value_bytes)
                {
                    return Err(self.resource("Operand transfer byte limit exceeded", Some(span)));
                }
                pending.admitted = true;
            }
        } else if let Some(value) = values.get(pending.copied) {
            let id = self.operand(*value, span)?;
            self.transfer(id, Some(span))?;
            pending.append(self, id)?;
            pending.copied += 1;
        } else {
            match pending.destination {
                Destination::Closure { function, ty } => {
                    self.check_allocation(32, 1, Some(span))?;
                    let index = self.data.closures.len();
                    self.data.closures.push(Closure {
                        function,
                        captures: pending.output,
                    });
                    self.data.consumed.closures += 1;
                    let id = self.alloc(ty, Data::Closure(index), Some(span))?;
                    self.permit(self.node(id)?.ty, ty, Some(span))?;
                    let Some(frame) = self.data.frames.last_mut() else {
                        return Err(self.invalid("Missing closure result frame", Some(span)));
                    };
                    frame.registers.push(id);
                }
                Destination::Call { callee, ty, site } => {
                    self.invoke_closure(callee, pending.output, site, ty)?
                }
                Destination::Jump(block) => {
                    let Some(frame) = self.data.frames.last_mut() else {
                        return Err(self.invalid("Missing jump frame", Some(span)));
                    };
                    frame.block = block as usize;
                    frame.pc = 0;
                    frame.parameters = pending.output;
                    frame.registers.clear();
                }
            }
            return Ok(());
        }
        let Some(frame) = self.data.frames.last_mut() else {
            return Err(self.invalid("Missing operand continuation frame", Some(span)));
        };
        frame.operands = Some(pending);
        Ok(())
    }
}
impl Preparation {
    fn append<I: super::inputs::InputFamily>(
        &mut self,
        state: &mut State<'_, '_, '_, I>,
        id: Id,
    ) -> Result<()> {
        let span = self.source.span(state.image);
        if self.output.len() == self.output.capacity() {
            state.retained_storage(8, span)?;
            self.output
                .try_reserve_exact(1)
                .map_err(|_| state.resource("Operand storage allocation failed", span))?;
        }
        self.output.push(id);
        Ok(())
    }
}
