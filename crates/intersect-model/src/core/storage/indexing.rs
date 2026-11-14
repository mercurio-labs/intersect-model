//! Cooperative immutable index preparation; key semantics are shared with adapters.
use super::*;
use std::{
    borrow::Borrow,
    ops::Bound::{Excluded, Unbounded},
};

#[derive(Default)]
pub(super) struct KeyCursor {
    metadata: u8,
    field: Option<String>,
    atom: Option<usize>,
}
impl KeyCursor {
    pub(super) fn next(&mut self, element: &Element) -> Option<Option<(String, String)>> {
        match self.next_action(element)? {
            KeyAction::Metadata(field, value) => Some(Some((field.into(), value.into()))),
            KeyAction::Atom(field, value) => {
                Some((!value.is_null()).then(|| (format!("value:{field}"), value.to_string())))
            }
            KeyAction::Field(field) => {
                self.select_field(field);
                Some(None)
            }
        }
    }
    fn select_field(&mut self, field: &str) {
        self.field = Some(field.into());
        self.atom = Some(0);
    }
    fn next_action<'a>(&mut self, element: &'a Element) -> Option<KeyAction<'a>> {
        if self.metadata < 2 {
            let key = if self.metadata == 0 {
                ("class", &element.class)
            } else {
                ("resource", &element.resource)
            };
            self.metadata += 1;
            return Some(KeyAction::Metadata(key.0, key.1));
        }
        if let (Some(name), Some(index)) = (&self.field, self.atom) {
            let (name, value) = element.values.get_key_value(name)?;
            let values = value
                .as_array()
                .map_or(std::slice::from_ref(value), Vec::as_slice);
            if let Some(value) = values.get(index) {
                self.atom = Some(index + 1);
                return Some(KeyAction::Atom(name, value));
            }
        }
        let next = match &self.field {
            Some(previous) => element
                .values
                .range::<str, _>((Excluded(previous.as_str()), Unbounded))
                .next(),
            None => element.values.iter().next(),
        };
        let (name, _) = next?;
        Some(KeyAction::Field(name))
    }
}
enum KeyAction<'a> {
    Metadata(&'a str, &'a str),
    Atom(&'a str, &'a Value),
    Field(&'a str),
}

fn reserve(
    observation: &InvocationContext<'_>,
    fixed: usize,
    lengths: &[usize],
) -> StorageResult<()> {
    let bytes = lengths
        .iter()
        .try_fold(fixed, |sum, length| sum.checked_add(*length))
        .ok_or("Model index storage overflow")?;
    observation.retain_storage(bytes)
}
enum Stage {
    Object,
    Keys,
    Publish,
    Done,
}
/// Owns or borrows one immutable model while constructing its read indexes.
/// A failed advance poisons preparation. Only successful completion exposes a view.
pub struct IndexedModelPreparation<M> {
    view: IndexedModel<M>,
    row: usize,
    stage: Stage,
    cursor: KeyCursor,
    keys: BTreeSet<(String, String)>,
    failed: bool,
}
impl<M: Borrow<Model>> IndexedModelPreparation<M> {
    pub(super) fn new(model: M, observation: &InvocationContext<'_>) -> StorageResult<Self> {
        observation.work(1)?;
        Ok(Self {
            view: IndexedModel {
                model,
                ids: BTreeMap::new(),
                indexes: BTreeMap::new(),
            },
            row: 0,
            stage: Stage::Object,
            cursor: KeyCursor::default(),
            keys: BTreeSet::new(),
            failed: false,
        })
    }
    /// Perform one object, field/atom, or distinct-key publication transition.
    /// Individual JSON atom serialization is not subdivided by this cursor.
    pub fn advance(&mut self, observation: &InvocationContext<'_>) -> StorageResult<bool> {
        if self.failed {
            return Err("Index preparation has failed".into());
        }
        let result = self.advance_inner(observation);
        if result.is_err() {
            self.failed = true;
        }
        result
    }
    fn advance_inner(&mut self, observation: &InvocationContext<'_>) -> StorageResult<bool> {
        observation.checkpoint()?;
        match self.stage {
            Stage::Object => {
                let Some(element) = self.view.model.borrow().elements.get(self.row) else {
                    self.stage = Stage::Done;
                    return Ok(true);
                };
                observation.work(1)?;
                if element.id.is_empty() || self.view.ids.contains_key(&element.id) {
                    return Err("Duplicate or empty model identity".into());
                }
                reserve(observation, 64, &[element.id.len()])?;
                self.view.ids.insert(element.id.clone(), self.row);
                self.cursor = KeyCursor::default();
                self.stage = Stage::Keys;
            }
            Stage::Keys => {
                observation.work(1)?;
                let element = self
                    .view
                    .model
                    .borrow()
                    .elements
                    .get(self.row)
                    .ok_or("Missing index row")?;
                match self.cursor.next_action(element) {
                    Some(KeyAction::Metadata(field, value)) => {
                        reserve(observation, 128, &[field.len(), value.len()])?;
                        self.keys.insert((field.into(), value.into()));
                    }
                    Some(KeyAction::Atom(field, value)) if !value.is_null() => {
                        let bytes =
                            intersect_model::definitions::metadata_limits::serialized_size_with_work(
                                value,
                                usize::MAX,
                                &|units| observation.work(units),
                            )?;
                        reserve(observation, 134, &[field.len(), bytes])?; // 128 + "value:".
                        self.keys
                            .insert((format!("value:{field}"), value.to_string()));
                    }
                    Some(KeyAction::Atom(_, _)) => {}
                    Some(KeyAction::Field(field)) => {
                        reserve(observation, 64, &[field.len()])?;
                        self.cursor.select_field(field);
                    }
                    None => self.stage = Stage::Publish,
                }
            }
            Stage::Publish => {
                if !self.keys.is_empty() {
                    observation.work(1)?;
                    let key = self.keys.pop_first().ok_or("Missing pending index key")?;
                    let new_bucket = !self.view.indexes.contains_key(&key);
                    reserve(observation, if new_bucket { 160 } else { 32 }, &[])?;
                    let rows = match self.view.indexes.entry(key) {
                        std::collections::btree_map::Entry::Vacant(entry) => {
                            entry.insert(Vec::new())
                        }
                        std::collections::btree_map::Entry::Occupied(entry) => entry.into_mut(),
                    };
                    rows.try_reserve(1)
                        .map_err(|_| "Model index row allocation failed")?;
                    rows.push(self.row);
                } else {
                    self.row += 1;
                    self.stage = Stage::Object;
                }
            }
            Stage::Done => return Ok(true),
        }
        Ok(false)
    }
    pub fn finish(self, observation: &InvocationContext<'_>) -> StorageResult<IndexedModel<M>> {
        if self.failed || !matches!(self.stage, Stage::Done) {
            return Err("Index preparation is incomplete or failed".into());
        }
        observation.checkpoint()?;
        Ok(self.view)
    }
}
