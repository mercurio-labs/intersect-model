//! Handwritten model facades composed over shared storage and checked editing.
//!
//! The basic implementation is independent of the transactional implementation.
//! Compose grouped editing explicitly with `TransactionalModel::new(basic)`.
//! Both facades share the owned state, operations and read forwarding below.
//! Optional versioning adds shared snapshots and isolated forks; no durable history or text edits.

use crate::core::storage::{self, ModelRead, ModelStore, StorageResult};
use intersect_model::definitions::{
    conversion::ConversionRegistry,
    schema::{FeatureIndex, Schema},
};
use serde_json::Value;

pub use basic::BasicModel;
pub use transactional::{EditSession, TransactionalModel};
pub use versioned::{ModelFork, Snapshot, VersionId, VersionedModel};

/// Checked editing capability for reflective callers and domain-owned typed wrappers.
///
/// Implementations must preserve supported structural contracts after each operation,
/// except that required values may remain absent until full validation. A failed
/// operation must not retain partial writes. This is not the raw persistence API.
pub trait ModelEdit: ModelRead {
    fn create_resource(&mut self, uri: &str) -> StorageResult<()>;
    fn reorder_roots(&mut self, uri: &str, roots: &[String]) -> StorageResult<()>;
    fn schema(&self) -> &Schema;

    /// Create a model-authored object with schema defaults and no source text span.
    /// Identities and resource URIs are supplied by the caller, without host I/O.
    fn create(&mut self, id: &str, class: &str, resource: &str) -> StorageResult<()>;

    /// Construct through an explicitly selected language initializer, atomically
    /// applying its plan and authored values. The receipt is provisional until
    /// any enclosing transaction or fork is successfully published.
    fn create_initialized(
        &mut self,
        request: storage::Construction<'_>,
        binding: storage::InitializationBinding<'_>,
        budget: &mut crate::core::query::QueryBudget,
    ) -> StorageResult<intersect_model::definitions::initialization::InitializationReceipt>;

    /// Move stored containment across owners/features, or detach to a root.
    fn reparent(
        &mut self,
        request: storage::ReparentRequest<'_>,
        conversions: Option<&ConversionRegistry<'_>>,
        budget: &mut crate::core::query::QueryBudget,
    ) -> StorageResult<bool>;

    /// Delete explicit loaded objects or subtrees with an incoming-reference policy.
    /// Uses shared atomic cleanup; publication still checks required survivors.
    fn delete_objects(
        &mut self,
        request: storage::DeleteRequest<'_>,
        conversions: Option<&ConversionRegistry<'_>>,
        budget: &mut crate::core::query::QueryBudget,
    ) -> StorageResult<Vec<String>>;

    /// Set a stored attribute or reference, maintaining declared opposites.
    fn set(&mut self, id: &str, feature: &str, value: Value) -> StorageResult<()> {
        self.set_many(&[(id, feature, value)])
    }

    /// Apply ordered edits as one atomic operation. Occupied singular opposites
    /// disconnect their previous owner. Lower multiplicities may be incomplete.
    fn set_many(&mut self, edits: &[(&str, &str, Value)]) -> StorageResult<()>;

    /// Reset a stored feature to its default and clear any unsettable marker.
    fn unset(&mut self, id: &str, feature: &str) -> StorageResult<()>;

    /// Mutate a stored collection without reflective replacement.
    fn edit_collection(
        &mut self,
        id: &str,
        feature: &str,
        edit: storage::CollectionEdit,
    ) -> StorageResult<bool>;

    /// Apply a language-owned computed set/reset through the same atomic storage
    /// service as reflective callers. The facade supplies its own schema identity.
    fn edit_computed(
        &mut self,
        id: &str,
        feature: &str,
        edit: storage::ComputedEdit,
        context: storage::SettingContext<'_>,
        budget: &mut crate::core::query::QueryBudget,
    ) -> StorageResult<()>;

    /// Semantic stored-feature state, distinct from raw field presence.
    fn is_set(&self, id: &str, feature: &str) -> StorageResult<bool> {
        storage::is_set(self, self.schema(), id, feature)
    }

    fn is_explicitly_set(&self, id: &str, feature: &str) -> StorageResult<bool> {
        storage::is_explicitly_set(self, self.schema(), id, feature)
    }
}

// Private shared ownership unit. Neither facade exposes mutable raw-store access.
// Consuming transfers preserve the exact schema, store and scoped handles.
struct ModelState<S: ModelStore> {
    schema: Schema,
    store: S,
    validation: storage::DraftValidationPlan,
}

fn require_idle(store: &impl ModelStore) -> StorageResult<()> {
    if store.is_transaction_active() {
        Err("Model API requires an idle store; an active or failed transaction must be resolved first".into())
    } else {
        Ok(())
    }
}

fn draft(
    store: &impl ModelStore,
    schema: &Schema,
    conversions: Option<&ConversionRegistry<'_>>,
) -> StorageResult<()> {
    match conversions {
        Some(registry) => storage::validate_draft_with_conversions(store, schema, registry),
        None => storage::validate_draft(store, schema),
    }
}
fn create(
    store: &mut impl ModelStore,
    schema: &Schema,
    validation: &storage::DraftValidationPlan,
    id: &str,
    class: &str,
    resource: &str,
) -> StorageResult<()> {
    create_context(store, schema, validation, id, class, resource, None)
}
pub(crate) fn create_context(
    store: &mut impl ModelStore,
    schema: &Schema,
    validation: &storage::DraftValidationPlan,
    id: &str,
    class: &str,
    resource: &str,
    conversions: Option<&ConversionRegistry<'_>>,
) -> StorageResult<()> {
    storage::atomic(store, |store| {
        store.create_element(schema, id, class, resource, 0, 0)?;
        if conversions.is_none() && store.supports_incremental_validation() {
            validation.validate_created(store, schema, id)
        } else {
            draft(store, schema, conversions)
        }
    })
}
fn unset(
    store: &mut impl ModelStore,
    schema: &Schema,
    id: &str,
    feature: &str,
) -> StorageResult<()> {
    unset_context(store, schema, id, feature, None)
}
fn unset_context(
    store: &mut impl ModelStore,
    schema: &Schema,
    id: &str,
    feature: &str,
    conversions: Option<&ConversionRegistry<'_>>,
) -> StorageResult<()> {
    storage::atomic(store, |store| {
        match conversions {
            Some(registry) => {
                storage::unset_with_conversions(store, schema, id, feature, registry)?
            }
            None => storage::unset(store, schema, id, feature)?,
        }
        draft(store, schema, conversions)
    })
}
fn set_many(
    store: &mut impl ModelStore,
    schema: &Schema,
    validation: &storage::DraftValidationPlan,
    edits: &[(&str, &str, Value)],
) -> StorageResult<()> {
    set_many_context(store, schema, validation, edits, None)
}
pub(crate) fn set_many_context(
    store: &mut impl ModelStore,
    schema: &Schema,
    validation: &storage::DraftValidationPlan,
    edits: &[(&str, &str, Value)],
    conversions: Option<&ConversionRegistry<'_>>,
) -> StorageResult<()> {
    storage::atomic(store, |store| {
        let mut local = conversions.is_none() && store.supports_incremental_validation();
        let batch_index =
            (conversions.is_none() && edits.len() > 1).then(|| FeatureIndex::new(schema));
        let mut changed = std::collections::BTreeSet::new();
        for (id, feature, value) in edits {
            match (conversions, &batch_index) {
                (Some(registry), _) => storage::set_with_opposite_and_conversions(
                    store,
                    schema,
                    id,
                    feature,
                    value.clone(),
                    registry,
                )?,
                (None, Some(index)) => {
                    storage::set_batch_member(store, index, id, feature, value.clone())?
                }
                (None, None) => {
                    storage::set_with_opposite(store, schema, id, feature, value.clone())?
                }
            }
            if local {
                local = validation.is_local_attribute(store, id, feature)?;
                changed.insert(*id);
            }
        }
        if local {
            validation.validate_objects(store, schema, &changed)
        } else if conversions.is_none() {
            validation.validate_all(store, schema, false)
        } else {
            draft(store, schema, conversions)
        }
    })
}

// Forward every projection, including inspect, so adapters retain their canonical
// views instead of falling back to compatibility materialization or whole values.
macro_rules! forward_reads {
    ($model:ty) => {
        impl<S: $crate::core::storage::ModelStore> $crate::core::storage::ModelRead for $model {
            fn resource_records(&self) -> $crate::core::storage::StorageResult<std::collections::BTreeMap<String, $crate::core::resources::ResourceRecord>> {
                self.reader().resource_records()
            }
            fn resource_record(&self, uri: &str) -> $crate::core::storage::StorageResult<Option<$crate::core::resources::ResourceRecord>> {
                self.reader().resource_record(uri)
            }
            fn revision(&self) -> $crate::core::storage::StorageResult<String> {
                self.reader().revision()
            }
            fn inspect_revision(
                &self,
                f: &mut dyn FnMut(&str) -> $crate::core::storage::StorageResult<()>,
            ) -> $crate::core::storage::StorageResult<()> {
                self.reader().inspect_revision(f)
            }
            fn get(
                &self,
                id: &str,
            ) -> $crate::core::storage::StorageResult<Option<$crate::core::model::Element>> {
                self.reader().get(id)
            }
            fn class(&self, id: &str) -> $crate::core::storage::StorageResult<Option<String>> {
                self.reader().class(id)
            }
            fn is_pending_reference(&self, id: &str, field: &str, index: usize, wire: &str) -> bool {
                self.reader().is_pending_reference(id, field, index, wire)
            }
            fn inspect_class(
                &self,
                id: &str,
                f: &mut dyn FnMut(&str) -> $crate::core::storage::StorageResult<()>,
            ) -> $crate::core::storage::StorageResult<bool> {
                self.reader().inspect_class(id, f)
            }
            fn page(
                &self,
                after: Option<u64>,
                limit: usize,
            ) -> $crate::core::storage::StorageResult<Vec<$crate::core::storage::Row>> {
                self.reader().page(after, limit)
            }
            fn query_candidates_observed(
                &self,
                query: $crate::core::storage::Query<'_>,
                after: Option<u64>,
                limit: usize,
                observation: &$crate::core::storage::InvocationContext<'_>,
            ) -> $crate::core::storage::StorageResult<$crate::core::storage::QueryCandidates> {
                self.reader()
                    .query_candidates_observed(query, after, limit, observation)
            }
            fn matches_observed(
                &self,
                id: &str,
                query: $crate::core::storage::Query<'_>,
                observation: &$crate::core::storage::InvocationContext<'_>,
            ) -> $crate::core::storage::StorageResult<bool> {
                self.reader().matches_observed(id, query, observation)
            }
            fn id_page_observed(
                &self,
                after: Option<u64>,
                limit: usize,
                observation: &$crate::core::storage::InvocationContext<'_>,
            ) -> $crate::core::storage::StorageResult<Vec<(u64, String)>> {
                self.reader().id_page_observed(after, limit, observation)
            }
            fn query_candidates(
                &self,
                query: $crate::core::storage::Query<'_>,
                after: Option<u64>,
                limit: usize,
            ) -> $crate::core::storage::StorageResult<$crate::core::storage::QueryCandidates> {
                self.reader().query_candidates(query, after, limit)
            }
            fn query(
                &self,
                query: $crate::core::storage::Query<'_>,
            ) -> $crate::core::storage::StorageResult<Vec<String>> {
                self.reader().query(query)
            }
            fn matches(
                &self,
                id: &str,
                query: $crate::core::storage::Query<'_>,
            ) -> $crate::core::storage::StorageResult<bool> {
                self.reader().matches(id, query)
            }
            fn id_page(
                &self,
                after: Option<u64>,
                limit: usize,
            ) -> $crate::core::storage::StorageResult<Vec<(u64, String)>> {
                self.reader().id_page(after, limit)
            }
            fn value_observed(
                &self,
                id: &str,
                field: &str,
                observation: &$crate::core::storage::InvocationContext<'_>,
            ) -> $crate::core::storage::StorageResult<Option<serde_json::Value>> {
                self.reader().value_observed(id, field, observation)
            }
            fn collection_page_observed(
                &self,
                id: &str,
                field: &str,
                offset: usize,
                limit: usize,
                observation: &$crate::core::storage::InvocationContext<'_>,
            ) -> $crate::core::storage::StorageResult<Vec<serde_json::Value>> {
                self.reader()
                    .collection_page_observed(id, field, offset, limit, observation)
            }
            fn value(
                &self,
                id: &str,
                field: &str,
            ) -> $crate::core::storage::StorageResult<Option<serde_json::Value>> {
                self.reader().value(id, field)
            }
            fn value_len(
                &self,
                id: &str,
                field: &str,
            ) -> $crate::core::storage::StorageResult<Option<usize>> {
                self.reader().value_len(id, field)
            }
            fn collection_page(
                &self,
                id: &str,
                field: &str,
                offset: usize,
                limit: usize,
            ) -> $crate::core::storage::StorageResult<Vec<serde_json::Value>> {
                self.reader().collection_page(id, field, offset, limit)
            }
            fn inspect(
                &self,
                id: &str,
                f: &mut dyn FnMut(
                    $crate::core::storage::ObjectView<'_>,
                ) -> $crate::core::storage::StorageResult<()>,
            ) -> $crate::core::storage::StorageResult<bool> {
                self.reader().inspect(id, f)
            }
        }
    };
}

mod basic;
mod transactional;

mod versioned;
