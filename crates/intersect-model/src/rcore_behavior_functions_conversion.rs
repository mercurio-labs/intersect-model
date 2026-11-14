//! Handwritten collection conversion continuation over the shared value authority.
use super::{
    collections::CollectionOutput,
    equivalence::Equivalence,
    machine::{CallSite, State},
    values::{Data, Id},
    *,
};
use intersect_model::definitions::rcore_behavior::{ir_v2::TypeId, types::CollectionKind};

pub(super) struct Conversion {
    input: Id,
    ty: TypeId,
    site: CallSite,
    unique: bool,
    cursor: usize,
    probe: usize,
    length: usize,
    comparison: Option<Equivalence>,
    output: CollectionOutput,
}
impl<I: super::inputs::InputFamily> State<'_, '_, '_, I> {
    pub(super) fn start_conversion(&mut self, input: Id, ty: TypeId, site: CallSite) -> Result<()> {
        let span = self
            .image
            .call_span(site)
            .ok_or_else(|| self.invalid("Missing conversion source", None))?;
        self.transfer(input, Some(span))?;
        let (source, _, _, _) = self.collection_info(self.node(input)?.ty)?;
        let (target, _, _, _) = self.collection_info(ty)?;
        if matches!(source, CollectionKind::Set | CollectionKind::Bag)
            && matches!(
                target,
                CollectionKind::Sequence | CollectionKind::OrderedSet
            )
        {
            return Err(self.invalid(
                "Unordered collection cannot supply positional order",
                Some(span),
            ));
        }
        if !matches!(self.node(input)?.data, Data::Collection(_)) {
            return Err(self.invalid(
                "Collection conversion requires collection value",
                Some(span),
            ));
        }
        self.collection_storage(128, span)?;
        let pending = Conversion {
            input,
            ty,
            site,
            unique: matches!(target, CollectionKind::Set | CollectionKind::OrderedSet),
            cursor: 0,
            probe: 0,
            length: 0,
            comparison: None,
            output: CollectionOutput::new(ty),
        };
        let Some(frame) = self.data.frames.last_mut() else {
            return Err(self.invalid("Missing conversion frame", Some(span)));
        };
        frame.conversion = Some(pending);
        Ok(())
    }
    pub(super) fn advance_conversion(&mut self) -> Result<()> {
        let mut pending = self
            .data
            .frames
            .last_mut()
            .and_then(|f| f.conversion.take())
            .ok_or_else(|| self.invalid("Missing conversion continuation", None))?;
        let span = self
            .image
            .call_span(pending.site)
            .ok_or_else(|| self.invalid("Missing conversion source", None))?;
        self.step(span)?;
        let Data::Collection(items) = &self.node(pending.input)?.data else {
            return Err(self.invalid("Missing conversion collection", Some(span)));
        };
        if let Some(item) = items.get(pending.cursor).copied() {
            if let Some(prior) = pending
                .output
                .item(pending.probe)
                .filter(|_| pending.unique)
            {
                match self.poll_equivalence(&mut pending.comparison, prior, item, span)? {
                    None => {}
                    Some(false) => pending.probe += 1,
                    Some(true) => {
                        pending.cursor += 1;
                        pending.probe = 0;
                    }
                }
            } else {
                pending.output.append(self, item, span)?;
                pending.length += 1;
                pending.cursor += 1;
                pending.probe = 0;
            }
            let Some(frame) = self.data.frames.last_mut() else {
                return Err(self.invalid("Missing conversion frame", Some(span)));
            };
            frame.conversion = Some(pending);
        } else {
            let (_, _, lower, upper) = self.collection_info(pending.ty)?;
            if pending.length < lower || (upper >= 0 && pending.length > upper as usize) {
                return Err(self.error(
                    ExecutionErrorKind::Arguments,
                    "Collection multiplicity mismatch",
                    Some(span),
                ));
            }
            let id = pending.output.finish(self, span)?;
            let Some(frame) = self.data.frames.last_mut() else {
                return Err(self.invalid("Missing conversion result frame", Some(span)));
            };
            frame.registers.push(id);
        }
        Ok(())
    }
}
