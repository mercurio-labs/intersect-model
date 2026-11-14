//! Explicit resource membership changes over the existing checked store.
use super::*;
use crate::core::{query::QueryBudget, resources};

impl StructuralContext<'_> {
    /// Move one stored containment subtree to a logical resource. Object IDs,
    /// values and containment ownership stay stable. Cross-resource containment
    /// is legal in core, so resource relocation is an explicit separate action.
    pub fn relocate_subtree(
        &self,
        store: &mut impl ModelStore,
        root: &str,
        destination: &str,
        budget: &mut QueryBudget,
    ) -> StorageResult<bool> {
        budget.charge()?;
        resources::validate_resource_uri(destination)?;
        self.validate(store, budget)?;
        if let Some(record) = store.resource_record(destination)?
            && record.load_state != resources::ResourceLoadState::Loaded
        {
            return Err("Cannot relocate into an unavailable resource".into());
        }
        let mut pending = vec![root.to_owned()];
        let mut selected = BTreeSet::new();
        let mut changes = Vec::new();
        while let Some(id) = pending.pop() {
            budget.charge()?;
            if !selected.insert(id.clone()) {
                continue;
            }
            let mut element = store.get(&id)?.ok_or("Missing relocation target")?;
            for (name, field) in self.schema.features(&element.class)? {
                budget.charge()?;
                if !field.containment || field.derived {
                    continue;
                }
                let mut append = |value: &Value| -> StorageResult<()> {
                    budget.charge()?;
                    pending.push(
                        value
                            .as_str()
                            .ok_or("Invalid relocation containment")?
                            .into(),
                    );
                    Ok(())
                };
                if let Some(value) = element.values.get(&name) {
                    match value {
                        Value::Null => {}
                        Value::Array(items) => {
                            for value in items {
                                append(value)?;
                            }
                        }
                        value => append(value)?,
                    }
                }
            }
            if element.resource != destination {
                let record = store
                    .resource_record(&element.resource)?
                    .ok_or("Missing relocation source resource")?;
                let was_root = record.roots.contains(&id);
                element.resource = destination.into();
                changes.push((element, was_root));
            }
        }
        if changes.is_empty() {
            return Ok(false);
        }
        atomic(store, |store| {
            bind_schema(store, self.schema)?;
            for (element, _) in &changes {
                budget.charge()?;
                store.put(element.clone())?;
            }
            for (element, was_root) in &changes {
                budget.charge()?;
                resources::change_root(store, destination, &element.id, *was_root)?;
            }
            self.validate(store, budget)?;
            Ok(true)
        })
    }
}
