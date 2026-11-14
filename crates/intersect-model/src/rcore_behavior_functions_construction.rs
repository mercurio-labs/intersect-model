//! Handwritten aggregate construction: one member or comparison frame per tick.
use super::*;
use crate::rcore_behavior_functions::equivalence::Equivalence;
use intersect_model::definitions::rcore_behavior::types::CollectionKind;
use std::ops::Bound::{Excluded, Unbounded};

pub(super) struct Construction {
    site: CallSite,
    output: Data,
    cursor: usize,
    probe: usize,
    bytes: usize,
    depth: usize,
    comparison: Option<Equivalence>,
}
impl<I: super::inputs::InputFamily> State<'_, '_, '_, I> {
    pub(super) fn start_construction(&mut self, site: CallSite) -> Result<()> {
        let image = self.image;
        let instruction = image
            .block(site.function, site.block)
            .and_then(|block| block.instructions.get(site.instruction))
            .ok_or_else(|| self.invalid("Missing aggregate instruction", None))?;
        let span = instruction.span;
        let output = match instruction.kind {
            Some(InstructionKind::Tuple(_)) => Data::Tuple(Vec::new()),
            Some(InstructionKind::Collection(items)) => {
                let (_, _, lower, upper) = self.collection_info(instruction.ty)?;
                if items.len() < lower || (upper >= 0 && items.len() > upper as usize) {
                    return Err(self.error(
                        ExecutionErrorKind::Arguments,
                        "Collection multiplicity mismatch",
                        Some(span),
                    ));
                }
                Data::Collection(Vec::new())
            }
            Some(InstructionKind::Record(_)) => Data::Record(BTreeMap::new()),
            _ => return Err(self.invalid("Expected aggregate instruction", Some(span))),
        };
        self.collection_storage(128, span)?;
        let pending = Construction {
            site,
            output,
            cursor: 0,
            probe: 0,
            bytes: 32,
            depth: 1,
            comparison: None,
        };
        let Some(frame) = self.data.frames.last_mut() else {
            return Err(self.invalid("Missing construction frame", Some(span)));
        };
        frame.construction = Some(pending);
        Ok(())
    }
    pub(super) fn advance_construction(&mut self) -> Result<()> {
        let mut pending = self
            .data
            .frames
            .last_mut()
            .and_then(|f| f.construction.take())
            .ok_or_else(|| self.invalid("Missing construction continuation", None))?;
        // Borrow from the immutable image, never from the mutable frame or output.
        let image = self.image;
        let instruction = image
            .block(pending.site.function, pending.site.block)
            .and_then(|block| block.instructions.get(pending.site.instruction))
            .ok_or_else(|| self.invalid("Missing aggregate instruction", None))?;
        let span = instruction.span;
        self.step(span)?;
        let next = match (instruction.kind, &pending.output) {
            (Some(InstructionKind::Tuple(items) | InstructionKind::Collection(items)), _) => {
                items.get(pending.cursor).map(|operand| (None, *operand))
            }
            (Some(InstructionKind::Record(fields)), Data::Record(output)) => {
                let next = match output.last_key_value() {
                    Some((last, _)) => fields
                        .range::<str, _>((Excluded(last.as_str()), Unbounded))
                        .next(),
                    None => fields.first_key_value(),
                };
                next.map(|(name, operand)| (Some(name), *operand))
            }
            _ => return Err(self.invalid("Aggregate continuation shape mismatch", Some(span))),
        };
        if let Some((name, operand)) = next {
            let id = self.operand(operand, span)?;
            if let Data::Collection(output) = &pending.output {
                let (kind, element, _, _) = self.collection_info(instruction.ty)?;
                self.permit(self.node(id)?.ty, element, Some(span))?;
                if matches!(kind, CollectionKind::Set | CollectionKind::OrderedSet)
                    && let Some(prior) = output.get(pending.probe).copied()
                {
                    match self.poll_equivalence(&mut pending.comparison, id, prior, span)? {
                        Some(true) => {
                            return Err(self.error(
                                ExecutionErrorKind::Arguments,
                                "Duplicate value in unique collection",
                                Some(span),
                            ));
                        }
                        Some(false) => pending.probe += 1,
                        None => {}
                    }
                    let Some(frame) = self.data.frames.last_mut() else {
                        return Err(self.invalid("Missing construction frame", Some(span)));
                    };
                    frame.construction = Some(pending);
                    return Ok(());
                }
            }
            let bytes = pending
                .bytes
                .checked_add(self.node(id)?.bytes)
                .and_then(|n| n.checked_add(name.map_or(0, |name| name.len())))
                .ok_or_else(|| self.resource("Aggregate size overflow", Some(span)))?;
            let depth = pending.depth.max(self.node(id)?.depth.saturating_add(1));
            self.check_allocation(bytes, depth, Some(span))?;
            self.transfer(id, Some(span))?;
            match &mut pending.output {
                Data::Tuple(output) | Data::Collection(output) => {
                    if output.len() >= self.limits.max_value_nodes {
                        return Err(self.resource("Aggregate item limit exceeded", Some(span)));
                    }
                    if output.len() == output.capacity() {
                        let capacity = output
                            .capacity()
                            .saturating_mul(2)
                            .max(1)
                            .min(self.limits.max_value_nodes);
                        let additional = capacity.saturating_sub(output.len());
                        let storage = additional.checked_mul(8).ok_or_else(|| {
                            self.resource("Aggregate storage overflow", Some(span))
                        })?;
                        self.collection_storage(storage, span)?;
                        output.try_reserve_exact(additional).map_err(|_| {
                            self.resource("Aggregate allocation failed", Some(span))
                        })?;
                    }
                    output.push(id);
                }
                Data::Record(output) => {
                    let name =
                        name.ok_or_else(|| self.invalid("Missing record field", Some(span)))?;
                    self.collection_storage(
                        name.len()
                            .checked_add(64)
                            .ok_or_else(|| self.resource("Record storage overflow", Some(span)))?,
                        span,
                    )?;
                    output.insert(name.clone(), id);
                }
                _ => return Err(self.invalid("Invalid aggregate output", Some(span))),
            }
            pending.bytes = bytes;
            pending.depth = depth;
            pending.cursor += 1;
            pending.probe = 0;
            let Some(frame) = self.data.frames.last_mut() else {
                return Err(self.invalid("Missing construction frame", Some(span)));
            };
            frame.construction = Some(pending);
        } else {
            let id = self.alloc_measured(
                instruction.ty,
                pending.output,
                pending.bytes,
                pending.depth,
                Some(span),
            )?;
            let Some(frame) = self.data.frames.last_mut() else {
                return Err(self.invalid("Missing construction result frame", Some(span)));
            };
            frame.registers.push(id);
        }
        Ok(())
    }
}
