//! Checked collection mutations share inverse maintenance, validation and history.
use super::*;

/// Stored collection edits; indexed operations require an ordered Many feature.
#[derive(Clone, Debug)]
pub enum CollectionEdit {
    Add(Value),
    Remove(Value),
    Clear,
    Insert {
        index: usize,
        value: Value,
    },
    Replace {
        index: usize,
        value: Value,
    },
    RemoveAt(usize),
    /// Destination is an index in the final list, not a gap before removal.
    Move {
        from: usize,
        to: usize,
    },
}

/// Edit a stored Many feature, including non-changeable dynamic collection views.
/// Returns whether contents changed; clearing an empty unsettable collection still
/// sets its assignment marker. Duplicate unique adds and missing removes are no-ops.
/// Draft lower bounds may be incomplete; all other structural checks are atomic.
pub fn edit_collection(
    store: &mut (impl ModelStore + ?Sized),
    schema: &Schema,
    id: &str,
    name: &str,
    edit: CollectionEdit,
) -> StorageResult<bool> {
    edit_context(store, schema, id, name, edit, None)
}
pub fn edit_collection_with_conversions(
    store: &mut (impl ModelStore + ?Sized),
    schema: &Schema,
    id: &str,
    name: &str,
    edit: CollectionEdit,
    conversions: &ConversionRegistry<'_>,
) -> StorageResult<bool> {
    edit_context(store, schema, id, name, edit, Some(conversions))
}
fn edit_context(
    store: &mut (impl ModelStore + ?Sized),
    schema: &Schema,
    id: &str,
    name: &str,
    edit: CollectionEdit,
    conversions: Option<&ConversionRegistry<'_>>,
) -> StorageResult<bool> {
    atomic(store, |store| {
        bind_schema(store, schema)?;
        validate_structure(store, schema, conversions, None, false, false)?;
        let class = store.class(id)?.ok_or("Missing element")?;
        let fields = schema.features(&class)?;
        let feature = fields.get(name).ok_or("Unknown feature")?;
        if feature.derived {
            return Err("Collection mutation requires a stored feature".into());
        }
        if feature.upper == 1 {
            return Err("Collection mutation requires a Many feature".into());
        }
        if !feature.ordered
            && matches!(
                edit,
                CollectionEdit::Insert { .. }
                    | CollectionEdit::Replace { .. }
                    | CollectionEdit::RemoveAt(_)
                    | CollectionEdit::Move { .. }
            )
        {
            return Err("Indexed collection edits require an ordered feature".into());
        }
        let mut value = store.value(id, name)?.ok_or("Missing collection")?;
        let list = value.as_array_mut().ok_or("Invalid collection")?;
        let changed = match edit {
            CollectionEdit::Add(item) => {
                if feature.unique && list.contains(&item) {
                    return Ok(false);
                }
                list.push(item);
                true
            }
            CollectionEdit::Remove(item) => {
                let Some(index) = list.iter().position(|value| value == &item) else {
                    return Ok(false);
                };
                list.remove(index);
                true
            }
            CollectionEdit::Insert { index, value } => {
                if index > list.len() {
                    return Err("Collection insert index out of bounds".into());
                }
                if feature.unique && list.contains(&value) {
                    return Err("Duplicate value in unique collection".into());
                }
                list.insert(index, value);
                true
            }
            CollectionEdit::Replace { index, value } => {
                let previous = list
                    .get(index)
                    .ok_or("Collection replace index out of bounds")?;
                if previous == &value {
                    return Ok(false);
                }
                if feature.unique && list.contains(&value) {
                    return Err("Duplicate value in unique collection".into());
                }
                list[index] = value;
                true
            }
            CollectionEdit::RemoveAt(index) => {
                if index >= list.len() {
                    return Err("Collection remove index out of bounds".into());
                }
                list.remove(index);
                true
            }
            CollectionEdit::Move { from, to } => {
                if from >= list.len() || to >= list.len() {
                    return Err("Collection move index out of bounds".into());
                }
                if list[from.min(to)..=from.max(to)]
                    .iter()
                    .all(|value| value == &list[from])
                {
                    return Ok(false);
                }
                let item = list.remove(from);
                list.insert(to, item);
                true
            }
            CollectionEdit::Clear => {
                let changed = !list.is_empty();
                list.clear();
                changed
            }
        };
        write_inner(
            store,
            schema,
            None,
            id,
            name,
            value,
            conversions,
            WriteMode::Collection,
        )?;
        validate_structure(store, schema, conversions, None, false, false)?;
        Ok(changed)
    })
}
