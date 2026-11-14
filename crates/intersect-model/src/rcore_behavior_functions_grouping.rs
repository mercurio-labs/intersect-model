//! Grouping state is subordinate to the common callback frame continuation.
use super::{
    collections::CollectionOutput,
    machine::State,
    values::{Data, Id},
    *,
};
use intersect_model::definitions::rcore_behavior::ir_v2::{TypeId, TypeNode};
struct Group {
    key: Id,
    members: CollectionOutput,
}
pub(super) struct Grouping {
    comparison: Option<super::equivalence::Equivalence>,
    key_type: TypeId,
    pair_type: TypeId,
    members_type: TypeId,
    groups: Vec<Group>,
    pending: Option<Id>,
    probe: usize,
    publish: usize,
}
impl Grouping {
    pub(super) fn new<I: super::inputs::InputFamily>(
        state: &State<'_, '_, '_, I>,
        ty: TypeId,
        span: &SourceSpan,
    ) -> Result<Self> {
        let (_, pair_type, _, _) = state.collection_info(ty)?;
        let TypeNode::Tuple(fields) = state.type_node(pair_type)? else {
            return Err(state.invalid("Grouping requires tuple output", Some(span)));
        };
        if fields.len() != 2 {
            return Err(state.invalid("Grouping requires key/member pair", Some(span)));
        }
        state.collection_info(fields[1])?;
        Ok(Self {
            comparison: None,
            key_type: fields[0],
            pair_type,
            members_type: fields[1],
            groups: Vec::new(),
            pending: None,
            probe: 0,
            publish: 0,
        })
    }
    pub(super) fn key<I: super::inputs::InputFamily>(
        &mut self,
        state: &State<'_, '_, '_, I>,
        value: Id,
        span: &SourceSpan,
    ) -> Result<()> {
        state.permit(state.node(value)?.ty, self.key_type, Some(span))?;
        self.pending = Some(value);
        self.probe = 0;
        Ok(())
    }
    pub(super) fn pending(&self) -> bool {
        self.pending.is_some()
    }
    pub(super) fn add_member<I: super::inputs::InputFamily>(
        &mut self,
        state: &mut State<'_, '_, '_, I>,
        value: Id,
        span: &SourceSpan,
    ) -> Result<()> {
        let key = self
            .pending
            .ok_or_else(|| state.invalid("Missing grouping key", Some(span)))?;
        if let Some(group) = self.groups.get_mut(self.probe) {
            let Some(equal) = state.poll_equivalence(&mut self.comparison, key, group.key, span)?
            else {
                return Ok(());
            };
            if equal {
                group.members.append(state, value, span)?;
                self.pending = None;
            } else {
                self.probe += 1;
            }
            return Ok(());
        }
        if self.groups.len() >= state.limits.max_value_nodes {
            return Err(state.resource("Grouping group limit exceeded", Some(span)));
        }
        if self.groups.len() == self.groups.capacity() {
            let capacity = self
                .groups
                .capacity()
                .saturating_mul(2)
                .max(1)
                .min(state.limits.max_value_nodes);
            let additional = capacity.saturating_sub(self.groups.len());
            let storage = additional
                .checked_mul(96)
                .ok_or_else(|| state.resource("Grouping storage overflow", Some(span)))?;
            state.collection_storage(storage, span)?;
            self.groups
                .try_reserve_exact(additional)
                .map_err(|_| state.resource("Grouping allocation failed", Some(span)))?;
        }
        let mut members = CollectionOutput::new(self.members_type);
        members.append(state, value, span)?;
        self.groups.push(Group { key, members });
        self.pending = None;
        Ok(())
    }
    pub(super) fn publish<I: super::inputs::InputFamily>(
        &mut self,
        state: &mut State<'_, '_, '_, I>,
        span: &SourceSpan,
    ) -> Result<Option<Id>> {
        let Some(group) = self.groups.get_mut(self.publish) else {
            return Ok(None);
        };
        let members = group.members.finish(state, span)?;
        state.transfer(group.key, Some(span))?;
        state.transfer(members, Some(span))?;
        let pair = state.alloc(
            self.pair_type,
            Data::Tuple(vec![group.key, members]),
            Some(span),
        )?;
        self.publish += 1;
        Ok(Some(pair))
    }
}
