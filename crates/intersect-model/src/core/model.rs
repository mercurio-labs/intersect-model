//! Handwritten schema-driven Intersect Model services. No language dependency.
use crate::core::frozen::FrozenStore;
use intersect_model::definitions::schema::Schema;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};

#[path = "model_assignment.rs"]
mod assignment;
pub use assignment::AssignmentState;
#[path = "model_snapshot.rs"]
mod snapshot;
pub use snapshot::{SnapshotError, SnapshotPreparation};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Element {
    /// Retained construction evidence; never permission to bypass validation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub initialization: Option<intersect_model::definitions::initialization::InitializationReceipt>,
    /// None is legacy/unspecified state, never inferred from effective values.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub assignment_state: Option<AssignmentState>,
    pub id: String,
    pub class: String,
    pub type_identity: String,
    pub resource: String,
    pub start: usize,
    pub end: usize,
    pub values: BTreeMap<String, Value>,
    /// Added by the language (`bind implicit`) rather than written in source;
    /// such elements are not printed back to text.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub implicit: bool,
}
impl Element {
    /// Construct stored schema defaults and identity. Computed values/state are
    /// supplied at access time by language providers, never invented here.
    /// Collection owners must
    /// enforce identity uniqueness and schema revision consistency.
    pub fn new(
        schema: &Schema,
        id: &str,
        class: &str,
        resource: &str,
        start: usize,
        end: usize,
    ) -> Result<Self, String> {
        Self::new_with_stored_selection(schema, id, class, resource, start, end, None)
    }
    /// Shared physical factory; selection is checked by the draft construction service.
    pub(crate) fn new_with_stored_selection(
        schema: &Schema,
        id: &str,
        class: &str,
        resource: &str,
        start: usize,
        end: usize,
        selected: Option<&BTreeSet<String>>,
    ) -> Result<Self, String> {
        schema.validate_instantiations()?;
        if id.is_empty() {
            return Err("Element identity must be unique and nonempty".into());
        }
        if schema
            .classes
            .get(class)
            .ok_or("Unknown class")?
            .abstract_type
        {
            return Err(format!("Cannot instantiate abstract class {class}"));
        }
        let features = schema.features(class)?;
        Self::from_selected_fields(schema, id, class, resource, start, end, selected, features)
    }
    #[allow(clippy::too_many_arguments)]
    fn from_selected_fields(
        schema: &Schema,
        id: &str,
        class: &str,
        resource: &str,
        start: usize,
        end: usize,
        selected: Option<&BTreeSet<String>>,
        features: BTreeMap<String, crate::definitions::schema::Feature>,
    ) -> Result<Self, String> {
        let assignment_state = features
            .values()
            .any(|f| f.unsettable && !f.derived)
            .then(|| AssignmentState::new(Vec::<String>::new()))
            .transpose()?;
        let values = features
            .into_iter()
            .filter(|(name, f)| !f.derived && selected.is_none_or(|names| names.contains(name)))
            .map(|(k, f)| (k, f.default))
            .collect();
        Ok(Self {
            implicit: false,
            initialization: None,
            assignment_state,
            id: id.into(),
            class: class.into(),
            type_identity: schema.identity(class),
            resource: resource.into(),
            start,
            end,
            values,
        })
    }
}
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct Model {
    pub schema_revision: String,
    pub resources: BTreeMap<String, super::resources::ResourceRecord>,
    pub elements: Vec<Element>,
}
impl Model {
    pub fn create(
        &mut self,
        schema: &Schema,
        id: &str,
        class: &str,
        resource: &str,
        start: usize,
        end: usize,
    ) -> Result<usize, String> {
        if !self.schema_revision.is_empty() && self.schema_revision != schema.revision {
            return Err("Model belongs to a different schema revision".into());
        }
        if id.is_empty() || self.elements.iter().any(|e| e.id == id) {
            return Err("Element identity must be unique and nonempty".into());
        }
        let element = Element::new(schema, id, class, resource, start, end)?;
        let index = self.elements.len();
        if schema.features(class)?.values().any(|field| {
            field.containment
                && !field.derived
                && !field.default.is_null()
                && !field.default.as_array().is_some_and(Vec::is_empty)
        }) {
            // Populated relationship defaults need the same resource delta as
            // store-backed construction. Keep the normal append path compact;
            // this exceptional materialization uses the existing shared adapter.
            let mut store = FrozenStore::from_materialized(schema, self)?;
            crate::core::storage::atomic(&mut store, |store| {
                crate::core::storage::insert(store, schema, element)
            })?;
            *self = crate::core::storage::export(&store)?;
            return Ok(index);
        }
        let mut record = self
            .resources
            .get(resource)
            .cloned()
            .unwrap_or(super::resources::ResourceRecord::new(resource)?);
        if record.load_state != super::resources::ResourceLoadState::Loaded {
            return Err("Cannot create an element in an unavailable resource".into());
        }
        record.touch()?;
        std::sync::Arc::make_mut(&mut record.roots).push(id.into());
        self.schema_revision = schema.revision.clone();
        self.resources.insert(resource.into(), record);
        self.elements.push(element);
        Ok(index)
    }
    pub fn validate(&self, schema: &Schema) -> Result<(), String> {
        crate::core::storage::validate(&crate::core::storage::ModelView::new(self)?, schema)
    }
    pub fn edit(
        &mut self,
        schema: &Schema,
        edits: &[(String, String, Value)],
    ) -> Result<(), String> {
        let mut store = FrozenStore::from_materialized(schema, self)?;
        crate::core::storage::edit(&mut store, schema, edits)?;
        *self = crate::core::storage::export(&store)?;
        Ok(())
    }
    pub fn validate_with_conversions(
        &self,
        schema: &Schema,
        conversions: &intersect_model::definitions::conversion::ConversionRegistry<'_>,
    ) -> Result<(), String> {
        crate::core::storage::validate_with_conversions(
            &crate::core::storage::ModelView::new(self)?,
            schema,
            conversions,
        )
    }
    /// Validate an immutable snapshot with a borrowed operation observer.
    pub fn validate_with_conversions_observed(
        &self,
        schema: &Schema,
        conversions: &intersect_model::definitions::conversion::ConversionRegistry<'_>,
        observation: &intersect_model::definitions::conversion::InvocationContext<'_>,
    ) -> Result<(), String> {
        crate::core::storage::validate_with_conversions_observed(
            &crate::core::storage::ModelView::new_observed(self, observation)?,
            schema,
            conversions,
            observation,
        )
    }
    pub fn edit_with_conversions(
        &mut self,
        schema: &Schema,
        edits: &[(String, String, Value)],
        conversions: &intersect_model::definitions::conversion::ConversionRegistry<'_>,
    ) -> Result<(), String> {
        let mut store = FrozenStore::from_materialized(schema, self)?;
        crate::core::storage::edit_with_conversions(&mut store, schema, edits, conversions)?;
        *self = crate::core::storage::export(&store)?;
        Ok(())
    }
    pub fn unset(&mut self, schema: &Schema, id: &str, name: &str) -> Result<(), String> {
        let mut store = FrozenStore::from_materialized(schema, self)?;
        crate::core::storage::unset(&mut store, schema, id, name)?;
        *self = crate::core::storage::export(&store)?;
        Ok(())
    }
    pub fn unset_with_conversions(
        &mut self,
        schema: &Schema,
        id: &str,
        name: &str,
        conversions: &intersect_model::definitions::conversion::ConversionRegistry<'_>,
    ) -> Result<(), String> {
        let mut store = FrozenStore::from_materialized(schema, self)?;
        crate::core::storage::unset_with_conversions(&mut store, schema, id, name, conversions)?;
        *self = crate::core::storage::export(&store)?;
        Ok(())
    }
    pub fn is_set(&self, schema: &Schema, id: &str, name: &str) -> Result<bool, String> {
        crate::core::storage::is_set(
            &crate::core::storage::ModelView::new(self)?,
            schema,
            id,
            name,
        )
    }
    pub fn edit_collection_with_conversions(
        &mut self,
        schema: &Schema,
        id: &str,
        name: &str,
        edit: crate::core::storage::CollectionEdit,
        conversions: &intersect_model::definitions::conversion::ConversionRegistry<'_>,
    ) -> Result<bool, String> {
        let mut store = FrozenStore::from_materialized(schema, self)?;
        let changed = crate::core::storage::edit_collection_with_conversions(
            &mut store,
            schema,
            id,
            name,
            edit,
            conversions,
        )?;
        *self = crate::core::storage::export(&store)?;
        Ok(changed)
    }
    /// Compatibility-model adapter over the shared atomic computed setting service.
    pub fn edit_computed(
        &mut self,
        settings: &crate::core::storage::ComputedSettings<'_>,
        id: &str,
        feature: &str,
        edit: crate::core::storage::ComputedEdit,
        budget: &mut crate::core::query::QueryBudget,
    ) -> Result<(), String> {
        let mut store = FrozenStore::from_materialized(settings.schema, self)?;
        settings.apply(&mut store, id, feature, edit, budget)?;
        *self = crate::core::storage::export(&store)?;
        Ok(())
    }
    pub fn set_with_opposite_and_conversions(
        &mut self,
        schema: &Schema,
        id: &str,
        name: &str,
        value: Value,
        conversions: &intersect_model::definitions::conversion::ConversionRegistry<'_>,
    ) -> Result<(), String> {
        let mut store = FrozenStore::from_materialized(schema, self)?;
        crate::core::storage::set_with_opposite_and_conversions(
            &mut store,
            schema,
            id,
            name,
            value,
            conversions,
        )?;
        *self = crate::core::storage::export(&store)?;
        Ok(())
    }
    pub fn set_with_opposite(
        &mut self,
        schema: &Schema,
        id: &str,
        name: &str,
        value: Value,
    ) -> Result<(), String> {
        let mut store = FrozenStore::from_materialized(schema, self)?;
        crate::core::storage::set_with_opposite(&mut store, schema, id, name, value)?;
        *self = crate::core::storage::export(&store)?;
        Ok(())
    }
}
pub fn contains(value: Option<&Value>, id: &str) -> bool {
    value.is_some_and(|v| {
        v.as_str() == Some(id)
            || v.as_array()
                .is_some_and(|a| a.iter().any(|v| v.as_str() == Some(id)))
    })
}
pub fn values(value: &Value, upper: i32) -> Result<Vec<&Value>, String> {
    Ok(value_slice(value, upper)?.iter().collect())
}
/// Borrow multiplicity contents without allocating a temporary vector.
pub(crate) fn value_slice(value: &Value, upper: i32) -> Result<&[Value], String> {
    if upper == 1 {
        if value.is_array() {
            Err("Singular feature cannot hold a list".into())
        } else if value.is_null() {
            Ok(&[])
        } else {
            Ok(std::slice::from_ref(value))
        }
    } else {
        value
            .as_array()
            .map(Vec::as_slice)
            .ok_or_else(|| "Multiple feature requires a list".into())
    }
}
