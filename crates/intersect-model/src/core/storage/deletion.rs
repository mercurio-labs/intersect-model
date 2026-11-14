//! Handwritten loaded-graph deletion using the shared writer and checkpoints.
use super::*;
use crate::core::query::QueryBudget;
use intersect_model::definitions::conversion::{InvocationObserver, ProviderId};
use std::{cell::RefCell, rc::Rc};

struct ValidationBudget<'a>(RefCell<&'a mut QueryBudget>);
impl InvocationObserver for ValidationBudget<'_> {
    fn before_invoke(&self, _: &ProviderId) -> StorageResult<()> {
        self.work(1)
    }
    fn checkpoint(&self) -> StorageResult<()> {
        self.work(1)
    }
    fn work(&self, units: usize) -> StorageResult<()> {
        let mut budget = self
            .0
            .try_borrow_mut()
            .map_err(|_| "Reentrant structural budget observation")?;
        for _ in 0..units {
            budget.charge()?;
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DeleteSelection {
    /// Only the requested objects; contained survivors make the request invalid.
    Objects,
    /// Requested objects and their stored containment descendants.
    Subtrees,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IncomingReferences {
    /// Reject surviving noncontainment references into the selection.
    Reject,
    /// Remove those references through checked stored-feature mutation.
    Detach,
}
#[derive(Clone, Copy)]
pub struct DeleteRequest<'a> {
    pub roots: &'a [&'a str],
    pub selection: DeleteSelection,
    pub incoming: IncomingReferences,
}
/// Providers are borrowed for validation and are never retained by the model.
pub struct StructuralContext<'a> {
    pub schema: &'a Schema,
    pub conversions: Option<&'a ConversionRegistry<'a>>,
}
struct ReferenceField {
    name: String,
    many: bool,
    containment: bool,
    changeable: bool,
    targets: Vec<String>,
}
impl StructuralContext<'_> {
    pub(super) fn validate(
        &self,
        store: &impl ModelRead,
        budget: &mut QueryBudget,
    ) -> StorageResult<()> {
        let observation = InvocationContext::new(Rc::new(ValidationBudget(RefCell::new(budget))));
        validate_structure(
            store,
            self.schema,
            self.conversions,
            Some(&observation),
            false,
            false,
        )
        .map(|_| ())
    }
    /// Delete selected loaded objects atomically, returning sorted removed IDs.
    /// Lower bounds may remain incomplete until the facade's publication check.
    /// Planning, mutation and observed validation work share one budget. Providers
    /// retain the existing cooperative contract; validation is not incremental.
    pub fn delete(
        &self,
        store: &mut impl ModelStore,
        request: DeleteRequest<'_>,
        budget: &mut QueryBudget,
    ) -> StorageResult<Vec<String>> {
        budget.charge()?;
        self.validate(store, budget)?;
        let mut graph = BTreeMap::new();
        let mut cursor = None;
        loop {
            budget.charge()?;
            let rows = store.id_page(cursor, 128)?;
            if rows.is_empty() {
                break;
            }
            for (row, id) in rows {
                budget.charge()?;
                cursor = Some(row);
                let mut references = Vec::new();
                if !store.inspect(&id, &mut |object| {
                    for (name, field) in self.schema.features(object.class())? {
                        budget.charge()?;
                        if !field.reference || field.derived {
                            continue;
                        }
                        let mut targets = Vec::new();
                        let mut append = |value: ValueView<'_>| -> StorageResult<()> {
                            budget.charge()?;
                            targets.push(
                                value
                                    .as_str()
                                    .ok_or("Unresolved reference in deletion graph")?
                                    .to_owned(),
                            );
                            Ok(())
                        };
                        if let Some(value) = object.value(&name) {
                            if let Some(values) = value.collection() {
                                for value in values.iter() {
                                    append(value)?;
                                }
                            } else if !value.is_null() {
                                append(value)?;
                            }
                        }
                        references.push(ReferenceField {
                            name,
                            many: field.upper != 1,
                            containment: field.containment,
                            changeable: field.changeable,
                            targets,
                        });
                    }
                    Ok(())
                })? {
                    return Err("Missing object during deletion planning".into());
                }
                graph.insert(id, references);
            }
        }
        let mut selected = BTreeSet::new();
        let mut pending = Vec::new();
        for id in request.roots {
            budget.charge()?;
            if !graph.contains_key(*id) {
                return Err(format!("Missing deletion target {id}"));
            }
            if selected.insert((*id).to_owned()) {
                pending.push((*id).to_owned());
            }
        }
        if request.selection == DeleteSelection::Subtrees {
            while let Some(id) = pending.pop() {
                budget.charge()?;
                for field in graph.get(&id).ok_or("Missing containment target")? {
                    budget.charge()?;
                    if field.containment {
                        for target in &field.targets {
                            budget.charge()?;
                            if selected.insert(target.clone()) {
                                pending.push(target.clone());
                            }
                        }
                    }
                }
            }
        }
        let mut edits = Vec::new();
        for (id, fields) in &graph {
            budget.charge()?;
            for field in fields {
                budget.charge()?;
                if selected.contains(id) {
                    if field.containment {
                        for target in &field.targets {
                            budget.charge()?;
                            if !selected.contains(target) {
                                return Err("Object deletion would leave a contained survivor; select its subtree or detach it first".into());
                            }
                        }
                    }
                    continue;
                }
                let mut retained = Vec::new();
                let mut changed = false;
                for target in &field.targets {
                    budget.charge()?;
                    if selected.contains(target) {
                        changed = true;
                    } else {
                        retained.push(Value::String(target.clone()));
                    }
                }
                if !changed {
                    continue;
                }
                if !field.containment && request.incoming == IncomingReferences::Reject {
                    return Err(format!(
                        "Incoming reference prevents deletion: {id}.{}",
                        field.name
                    ));
                }
                if !field.changeable {
                    return Err(format!(
                        "Cannot detach a non-changeable reference: {id}.{}",
                        field.name
                    ));
                }
                // None means scalar unset; a Many removal preserves explicit state.
                edits.push((
                    id.clone(),
                    field.name.clone(),
                    field.many.then_some(Value::Array(retained)),
                ));
            }
        }
        atomic(store, |store| {
            bind_schema(store, self.schema)?;
            for (id, name, value) in edits {
                budget.charge()?;
                match value {
                    Some(value) => {
                        set_inner(store, self.schema, &id, &name, value, self.conversions)?
                    }
                    None => match self.conversions {
                        Some(registry) => {
                            unset_with_conversions(store, self.schema, &id, &name, registry)?
                        }
                        None => unset(store, self.schema, &id, &name)?,
                    },
                }
            }
            for id in &selected {
                budget.charge()?;
                store.remove(id)?;
            }
            self.validate(store, budget)?;
            Ok(selected.into_iter().collect())
        })
    }
}
