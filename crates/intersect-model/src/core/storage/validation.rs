//! Handwritten structural checks shared by complete audits and bounded draft edits.
use super::{ModelRead, ObjectView, Query, Row, StorageResult};
use intersect_model::definitions::schema::{FeatureMapPreparation, PreparedFeatures, Schema};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

type EffectiveFeatures = BTreeMap<String, Arc<PreparedFeatures>>;

fn effective_features(schema: &Schema) -> StorageResult<EffectiveFeatures> {
    schema
        .classes
        .keys()
        .map(|name| {
            let mut pending = FeatureMapPreparation::new(name);
            while !pending.advance(schema)? {}
            Ok((name.clone(), Arc::new(pending.finish()?)))
        })
        .collect()
}

/// Immutable metadata owned alongside exactly one immutable facade schema.
/// There is no mutable validity cache to restore after a failed operation.
pub(crate) struct DraftValidationPlan {
    features: EffectiveFeatures,
    reciprocal_names: BTreeSet<String>,
    #[cfg(feature = "components")]
    metamodel: Option<Arc<crate::core::frozen::Metamodel>>,
}
impl DraftValidationPlan {
    pub(crate) fn new(schema: &Schema) -> StorageResult<Self> {
        let features = effective_features(schema)?;
        // Even unusual schemas can name a non-reference feature as the other
        // end of an opposite. Changing that value requires a graph audit too.
        // Do not eagerly reject unused malformed opposite declarations here.
        let reciprocal_names = features
            .values()
            .flat_map(|fields| fields.fields().values())
            .filter_map(|field| field.opposite.as_deref())
            .filter_map(|opposite| opposite.split_once('/').map(|(_, back)| back.to_owned()))
            .collect();
        Ok(Self {
            features,
            reciprocal_names,
            #[cfg(feature = "components")]
            metamodel: None,
        })
    }

    /// The frozen engine admits MOF redefinitions separately from the schema.
    /// Keep that exact physical layout when running the shared graph audit.
    #[cfg(feature = "components")]
    pub(crate) fn for_metamodel(
        metamodel: Arc<crate::core::frozen::Metamodel>,
    ) -> StorageResult<Self> {
        let mut plan = Self::new(metamodel.schema())?;
        plan.metamodel = Some(metamodel);
        Ok(plan)
    }

    fn select_layout(
        &self,
        preparation: &mut super::StructuralPreparation,
        schema: &Schema,
    ) -> StorageResult<()> {
        #[cfg(feature = "components")]
        if let Some(metamodel) = &self.metamodel {
            if !std::ptr::eq(metamodel.schema(), schema) {
                return Err("Validation layout belongs to another schema".into());
            }
            preparation.select_metamodel_layout(metamodel)?;
        }
        #[cfg(not(feature = "components"))]
        let _ = (preparation, schema);
        Ok(())
    }

    pub(crate) fn is_local_attribute(
        &self,
        store: &impl ModelRead,
        id: &str,
        name: &str,
    ) -> StorageResult<bool> {
        let class = store.class(id)?.ok_or("Missing element")?;
        let field = self
            .features
            .get(&class)
            .and_then(|fields| fields.get(name))
            .ok_or("Unknown feature")?;
        Ok(!field.reference
            && !field.containment
            && field.opposite.is_none()
            && !self.reciprocal_names.contains(name))
    }

    pub(crate) fn validate_created(
        &self,
        store: &impl ModelRead,
        schema: &Schema,
        id: &str,
    ) -> StorageResult<()> {
        let mut relationships = false;
        if !store.inspect(id, &mut |element| {
            let fields = self.features.get(element.class()).ok_or("Unknown class")?;
            relationships = fields.fields().iter().any(|(name, field)| {
                (field.reference
                    || field.containment
                    || field.opposite.is_some()
                    || self.reciprocal_names.contains(name))
                    && element.value(name).is_some_and(|value| {
                        !value.is_null() && value.collection().is_none_or(|items| !items.is_empty())
                    })
            });
            Ok(())
        })? {
            return Err("Missing element".into());
        }
        if relationships {
            self.validate_all(store, schema, false)
        } else {
            self.validate_objects(store, schema, &BTreeSet::from([id]))
        }
    }

    /// The facade holds the same schema and plan together for their lifetime.
    /// Only immutable schema work is reused. Each graph and value is audited.
    pub(crate) fn validate_all(
        &self,
        store: &impl ModelRead,
        schema: &Schema,
        require_lower_bounds: bool,
    ) -> StorageResult<()> {
        let mut preparation = super::StructuralPreparation::with_prepared_features(
            require_lower_bounds,
            &self.features,
        );
        self.select_layout(&mut preparation, schema)?;
        preparation.run(store, schema, None, None).map(|_| ())
    }

    /// Only use after a locally confined edit to a known draft-valid isolated
    /// store. Graph-changing operations must use complete draft validation.
    pub(crate) fn validate_objects(
        &self,
        store: &impl ModelRead,
        schema: &Schema,
        ids: &BTreeSet<&str>,
    ) -> StorageResult<()> {
        let selected = SelectedRead { store, ids };
        let mut preparation = super::StructuralPreparation::selected(&self.features);
        self.select_layout(&mut preparation, schema)?;
        preparation.run(&selected, schema, None, None).map(|_| ())
    }
}

// Restrict the shared validator's traversal to eligible affected objects. Target
// lookups still see the whole immutable store; no alternate structural rules.
trait SelectedIdentities {
    fn id_page(&self, after: Option<u64>, limit: usize) -> StorageResult<Vec<(u64, String)>>;
}
impl SelectedIdentities for BTreeSet<&str> {
    fn id_page(&self, after: Option<u64>, limit: usize) -> StorageResult<Vec<(u64, String)>> {
        self.iter()
            .enumerate()
            .filter(|(row, _)| after.is_none_or(|previous| (*row as u64) + 1 > previous))
            .take(limit)
            .map(|(row, id)| Ok((row as u64 + 1, (*id).to_owned())))
            .collect()
    }
}
struct SelectedRead<'a, S: ModelRead, I: SelectedIdentities> {
    store: &'a S,
    ids: &'a I,
}
impl<S: ModelRead, I: SelectedIdentities> ModelRead for SelectedRead<'_, S, I> {
    fn resource_records(
        &self,
    ) -> StorageResult<BTreeMap<String, crate::core::resources::ResourceRecord>> {
        self.store.resource_records()
    }
    fn resource_record(
        &self,
        uri: &str,
    ) -> StorageResult<Option<crate::core::resources::ResourceRecord>> {
        self.store.resource_record(uri)
    }
    fn revision(&self) -> StorageResult<String> {
        self.store.revision()
    }
    fn inspect_revision(&self, f: &mut dyn FnMut(&str) -> StorageResult<()>) -> StorageResult<()> {
        self.store.inspect_revision(f)
    }
    fn get(&self, id: &str) -> StorageResult<Option<crate::core::model::Element>> {
        self.store.get(id)
    }
    fn class(&self, id: &str) -> StorageResult<Option<String>> {
        self.store.class(id)
    }
    fn inspect_class(
        &self,
        id: &str,
        f: &mut dyn FnMut(&str) -> StorageResult<()>,
    ) -> StorageResult<bool> {
        self.store.inspect_class(id, f)
    }
    fn page(&self, after: Option<u64>, limit: usize) -> StorageResult<Vec<Row>> {
        self.id_page(after, limit)?
            .into_iter()
            .map(|(row, id)| Ok((row, self.store.get(&id)?.ok_or("Missing element")?)))
            .collect()
    }
    fn id_page(&self, after: Option<u64>, limit: usize) -> StorageResult<Vec<(u64, String)>> {
        self.ids.id_page(after, limit)
    }
    fn query(&self, query: Query<'_>) -> StorageResult<Vec<String>> {
        self.store.query(query)
    }
    fn matches(&self, id: &str, query: Query<'_>) -> StorageResult<bool> {
        self.store.matches(id, query)
    }
    fn value(&self, id: &str, field: &str) -> StorageResult<Option<Value>> {
        self.store.value(id, field)
    }
    fn is_pending_reference(&self, id: &str, field: &str, index: usize, wire: &str) -> bool {
        self.store.is_pending_reference(id, field, index, wire)
    }
    fn inspect(
        &self,
        id: &str,
        f: &mut dyn FnMut(ObjectView<'_>) -> StorageResult<()>,
    ) -> StorageResult<bool> {
        self.store.inspect(id, f)
    }
}
