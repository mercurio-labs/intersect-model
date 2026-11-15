//! Per-cell resolution in exclusive, unindexed, non-containment reference lists.
//! Unsupported shapes retain the shared wire-value compatibility operation.
use super::super::FeatureId;
use super::*;

impl FrozenStore {
    pub(super) fn resolve_item(
        &mut self,
        id: &str,
        field: &str,
        index: Option<usize>,
        target: &str,
    ) -> StorageResult<()> {
        let Some((obj, feature, offset, list, item)) =
            self.resolution_cell(id, field, index, target)
        else {
            return storage::resolve_reference_compat(self, id, field, index, target);
        };
        let previous = *self
            .builder
            .data
            .lists
            .get(list as usize)
            .and_then(|items| items.get(item))
            .ok_or("Missing reference item")?;
        let value = Value::String(target.into());
        if self.decode(previous) == value {
            return Ok(());
        }
        let header = self.builder.data.header(obj).ok_or("Missing object")?;
        let resource = *self
            .builder
            .data
            .resources
            .get(header.resource as usize)
            .ok_or("Missing object resource")?;
        let resource = self.builder.data.strings.resolve(resource).to_owned();
        self.touch(obj)?;
        let next = storage::atomic(self, |store| {
            let list = store.owned_append_list(
                obj,
                feature,
                FeatureKind::Reference,
                offset,
                Slot::Many(list),
            )?;
            let atom = store.reference(obj, feature, &value, ReferenceEncoding::Wire)?;
            // This is the same non-containment delta, including revision,
            // residency, source correspondence and diagnostic invalidation.
            crate::core::resources::field_delta_inner(store, &resource, false, None, Some(&value))?;
            let before = *store
                .builder
                .data
                .lists
                .get(list as usize)
                .and_then(|items| items.get(item))
                .ok_or("Missing reference item")?;
            let frame = store
                .frames
                .last_mut()
                .ok_or("Storage write requires a transaction")?;
            if (list as usize) < frame.lists {
                frame.list_items.push(((list, item), before));
            }
            *store
                .builder
                .data
                .lists
                .get_mut(list as usize)
                .and_then(|items| items.get_mut(item))
                .ok_or("Missing reference item")? = atom;
            Ok(Slot::Many(list))
        })?;
        *self
            .builder
            .data
            .slots
            .get_mut(offset)
            .ok_or("Missing object slots")? = next;
        Ok(())
    }

    fn resolution_cell(
        &self,
        id: &str,
        field: &str,
        item: Option<usize>,
        target: &str,
    ) -> Option<(ObjId, FeatureId, usize, u32, usize)> {
        if self.frames.is_empty() || target.starts_with('?') {
            return None;
        }
        let item = item?;
        let obj = self.object(id)?;
        let mm = &self.builder.metamodel;
        let header = self.builder.data.header(obj)?;
        let feature = mm.feature(header.class, field)?;
        if mm.feature_info(feature)?.kind != FeatureKind::Reference {
            return None;
        }
        let slot = mm.slot(header.class, feature)?;
        let stored = *mm.class_info(header.class)?.stored.get(slot)?;
        if self.indexes.indexes(mm.feature_name(stored)) {
            return None;
        }
        let offset = header.slots as usize + slot;
        let Slot::Many(list) = *self.builder.data.slots.get(offset)? else {
            return None;
        };
        let items = self.builder.data.lists.get(list as usize)?;
        items.get(item)?;
        // Re-encoding a whole wire list can normalize other cells. Only reuse
        // lists whose untouched cells have the SAME observable reference state.
        if items
            .iter()
            .enumerate()
            .any(|(index, &atom)| index != item && !self.wire_reference_stable(obj, feature, atom))
        {
            return None;
        }
        Some((obj, feature, offset, list, item))
    }

    fn wire_reference_stable(&self, owner: ObjId, feature: FeatureId, atom: Slot) -> bool {
        let data = &self.builder.data;
        match atom {
            Slot::Ref(target) => data.header(target).is_some_and(|header| {
                is_live(data, target) && !data.strings.resolve(header.id).starts_with('?')
            }),
            Slot::Proxy(index) => data.proxies.get(index as usize).is_some_and(|proxy| {
                proxy.owner == owner
                    && proxy.feature == feature
                    && (!self.identity_proxies.contains(&index)
                        || (!data.strings.resolve(proxy.spelling).starts_with('?')
                            && !data.by_id.contains_key(&proxy.spelling)))
            }),
            _ => false,
        }
    }
}

#[cfg(test)]
#[path = "store_resolve_tests.rs"]
mod tests;
