//! Seed-inclusive breadth-first model closure using the ordinary callback frames.
use super::{
    collections::{Advance, CollectionOutput},
    machine::State,
    values::{Data, Id},
    *,
};
use intersect_model::definitions::rcore_behavior::ir_v2::{TypeId, TypeNode};
use std::collections::{BTreeMap, BTreeSet};

pub(super) struct GraphClosure {
    seen: BTreeMap<String, BTreeSet<String>>,
    output: CollectionOutput,
    seeds_done: bool,
    seed_cursor: usize,
    expanded: usize,
    successors: Option<(Id, usize)>,
}
impl GraphClosure {
    pub(super) fn new(ty: TypeId) -> Self {
        Self {
            seen: BTreeMap::new(),
            output: CollectionOutput::new(ty),
            seeds_done: false,
            seed_cursor: 0,
            expanded: 0,
            successors: None,
        }
    }
    fn discover<I: super::inputs::InputFamily>(
        &mut self,
        state: &mut State<'_, '_, '_, I>,
        value: Id,
        span: &SourceSpan,
    ) -> Result<()> {
        let payload = state.admit_stored_receiver(value, span)?;
        let (context, element) = crate::model_reference::reference_identity(&payload)
            .map_err(|e| state.invalid(&e, Some(span)))?;
        state.compare_charge(Some(span))?;
        if self
            .seen
            .get(context)
            .is_some_and(|ids| ids.contains(element))
        {
            return Ok(());
        }
        let bytes = 64usize
            .checked_add(element.len())
            .and_then(|n| {
                n.checked_add(if self.seen.contains_key(context) {
                    0
                } else {
                    64usize.saturating_add(context.len())
                })
            })
            .ok_or_else(|| state.resource("Closure visited storage overflow", Some(span)))?;
        state.collection_storage(bytes, span)?;
        self.output.append(state, value, span)?;
        self.seen
            .entry(context.into())
            .or_default()
            .insert(element.into());
        Ok(())
    }
    pub(super) fn advance<I: super::inputs::InputFamily>(
        &mut self,
        state: &mut State<'_, '_, '_, I>,
        seeds: Id,
        callback: Id,
        returned: Option<Id>,
        span: &SourceSpan,
    ) -> Result<Advance> {
        if let Some(value) = returned {
            if !matches!(state.node(value)?.data, Data::Collection(_)) {
                return Err(
                    state.invalid("Closure successor must return collection data", Some(span))
                );
            }
            self.successors = Some((value, 0));
            return Ok(Advance::Pending);
        }
        if !self.seeds_done {
            let Data::Collection(items) = &state.node(seeds)?.data else {
                return Err(state.invalid("Closure requires seed collection", Some(span)));
            };
            if let Some(value) = items.get(self.seed_cursor).copied() {
                self.discover(state, value, span)?;
                self.seed_cursor = self
                    .seed_cursor
                    .checked_add(1)
                    .ok_or_else(|| state.resource("Closure seed cursor overflow", Some(span)))?;
            } else {
                self.seeds_done = true;
            }
            return Ok(Advance::Pending);
        }
        if let Some((collection, cursor)) = self.successors {
            let Data::Collection(items) = &state.node(collection)?.data else {
                return Err(state.invalid("Missing closure successors", Some(span)));
            };
            if let Some(value) = items.get(cursor).copied() {
                self.discover(state, value, span)?;
                self.successors = Some((
                    collection,
                    cursor.checked_add(1).ok_or_else(|| {
                        state.resource("Closure successor cursor overflow", Some(span))
                    })?,
                ));
            } else {
                self.successors = None;
            }
            return Ok(Advance::Pending);
        }
        let Some(value) = self.output.item(self.expanded) else {
            return Ok(Advance::Done(self.output.finish(state, span)?));
        };
        self.expanded = self
            .expanded
            .checked_add(1)
            .ok_or_else(|| state.resource("Closure queue cursor overflow", Some(span)))?;
        let TypeNode::Function { result, .. } = state.type_node(state.node(callback)?.ty)? else {
            return Err(state.invalid("Missing closure callback type", Some(span)));
        };
        let result = *result;
        state.transfer(value, Some(span))?;
        Ok(Advance::Call {
            callback,
            arguments: vec![value],
            result,
        })
    }
}
