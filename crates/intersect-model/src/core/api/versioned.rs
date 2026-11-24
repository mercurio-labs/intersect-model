//! Handwritten immutable state and optional in-memory version service.
//! Snapshot data codecs are separate; hosts own Git, I/O and retained history.
use super::{BasicModel, ModelEdit};
use crate::core::{
    frozen::FrozenStore,
    storage::{self, ModelRead, StorageResult},
};
use intersect_model::definitions::{conversion::ConversionRegistry, schema::Schema};
use serde_json::Value;
use std::sync::Arc;

/// An opaque version identity scoped to one model, separate from schema revision.
#[derive(Clone, Debug)]
pub struct VersionId {
    scope: Arc<()>,
    sequence: u64,
}
impl PartialEq for VersionId {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.scope, &other.scope) && self.sequence == other.sequence
    }
}
impl Eq for VersionId {}

struct Committed {
    store: CommittedStore,
    version: VersionId,
}

struct CommittedStore {
    schema: Arc<Schema>,
    validation: Arc<storage::DraftValidationPlan>,
    arena: Arc<FrozenStore>,
}

/// Immutable model state. Cloning shares its storage and is constant-space.
#[derive(Clone)]
pub struct Snapshot(Arc<Committed>);
impl Snapshot {
    pub fn version(&self) -> VersionId {
        self.0.version.clone()
    }
    pub fn schema(&self) -> &Schema {
        &self.0.store.schema
    }
    fn with_reader<T>(
        &self,
        read: impl FnOnce(&dyn ModelRead) -> StorageResult<T>,
    ) -> StorageResult<T> {
        read(self.0.store.arena.as_ref())
    }
}

/// An isolated editable draft with a pinned baseline for publication checks.
pub struct ModelFork {
    base: Snapshot,
    draft: Box<FrozenStore>,
}

macro_rules! edit_fork {
    ($fork:ident, |$store:ident, $schema:pat_param, $validation:pat_param| $edit:expr) => {{
        let $store = $fork.draft.as_mut();
        let $schema = $fork.base.0.store.schema.as_ref();
        let $validation = $fork.base.0.store.validation.as_ref();
        $edit
    }};
}

impl ModelFork {
    /// Capture the current candidate without committing it to its owner.
    pub fn snapshot(&self) -> StorageResult<Snapshot> {
        snapshot_draft(self.base.0.store.schema.clone(), self.draft.fork_arena()?)
    }
    pub fn base_version(&self) -> VersionId {
        self.base.version()
    }
    pub fn schema(&self) -> &Schema {
        self.base.schema()
    }
    pub fn validate(&self) -> StorageResult<()> {
        storage::validate(self, self.schema())
    }
    /// Validate using providers borrowed for this call only.
    pub fn validate_with_conversions(
        &self,
        conversions: &ConversionRegistry<'_>,
    ) -> StorageResult<()> {
        storage::validate_with_conversions(self, self.schema(), conversions)
    }
    pub fn create_with_conversions(
        &mut self,
        id: &str,
        class: &str,
        resource: &str,
        conversions: &ConversionRegistry<'_>,
    ) -> StorageResult<()> {
        edit_fork!(self, |store, schema, _validation| {
            super::create_context(
                store,
                schema,
                _validation,
                id,
                class,
                resource,
                Some(conversions),
            )
        })
    }
    pub fn set_with_conversions(
        &mut self,
        id: &str,
        feature: &str,
        value: Value,
        conversions: &ConversionRegistry<'_>,
    ) -> StorageResult<()> {
        self.set_many_with_conversions(&[(id, feature, value)], conversions)
    }
    pub fn set_many_with_conversions(
        &mut self,
        edits: &[(&str, &str, Value)],
        conversions: &ConversionRegistry<'_>,
    ) -> StorageResult<()> {
        edit_fork!(self, |store, schema, _validation| {
            super::set_many_context(store, schema, _validation, edits, Some(conversions))
        })
    }
    pub fn unset_with_conversions(
        &mut self,
        id: &str,
        feature: &str,
        conversions: &ConversionRegistry<'_>,
    ) -> StorageResult<()> {
        edit_fork!(self, |store, schema, _validation| {
            super::unset_context(store, schema, id, feature, Some(conversions))
        })
    }
    pub fn edit_collection_with_conversions(
        &mut self,
        id: &str,
        feature: &str,
        edit: storage::CollectionEdit,
        conversions: &ConversionRegistry<'_>,
    ) -> StorageResult<bool> {
        edit_fork!(self, |store, schema, _validation| {
            storage::edit_collection_with_conversions(store, schema, id, feature, edit, conversions)
        })
    }
    fn with_reader<T>(
        &self,
        read: impl FnOnce(&dyn ModelRead) -> StorageResult<T>,
    ) -> StorageResult<T> {
        read(self.draft.as_ref())
    }
}

// Standalone captures reuse exactly the immutable storage and readers used by
// versioned heads. Their VersionId is fresh and cannot impersonate an owner head.
pub(super) fn snapshot_draft(schema: Arc<Schema>, store: FrozenStore) -> StorageResult<Snapshot> {
    storage::validate_draft(&store, &schema)?;
    let validation = Arc::new(storage::DraftValidationPlan::new(&schema)?);
    Ok(VersionedModel::initial(CommittedStore {
        schema,
        validation,
        arena: Arc::new(store),
    })
    .head)
}

impl ModelEdit for ModelFork {
    fn create_resource(&mut self, uri: &str) -> StorageResult<()> {
        edit_fork!(self, |store, _, _| crate::core::resources::create_resource(
            store, uri
        ))
    }
    fn reorder_roots(&mut self, uri: &str, roots: &[String]) -> StorageResult<()> {
        edit_fork!(self, |store, _, _| crate::core::resources::reorder_roots(
            store, uri, roots
        ))
    }
    fn reparent(
        &mut self,
        request: storage::ReparentRequest<'_>,
        conversions: Option<&ConversionRegistry<'_>>,
        budget: &mut crate::core::query::QueryBudget,
    ) -> StorageResult<bool> {
        edit_fork!(self, |store, schema, _validation| {
            storage::StructuralContext {
                schema,
                conversions,
            }
            .reparent(store, request, budget)
        })
    }

    fn delete_objects(
        &mut self,
        request: storage::DeleteRequest<'_>,
        conversions: Option<&ConversionRegistry<'_>>,
        budget: &mut crate::core::query::QueryBudget,
    ) -> StorageResult<Vec<String>> {
        edit_fork!(self, |store, schema, _validation| {
            storage::StructuralContext {
                schema,
                conversions,
            }
            .delete(store, request, budget)
        })
    }

    fn edit_computed(
        &mut self,
        id: &str,
        feature: &str,
        edit: storage::ComputedEdit,
        context: storage::SettingContext<'_>,
        budget: &mut crate::core::query::QueryBudget,
    ) -> StorageResult<()> {
        edit_fork!(self, |store, schema, _validation| {
            storage::ComputedSettings {
                schema,
                provider: context.provider,
                conversions: context.conversions,
            }
            .apply(store, id, feature, edit, budget)
        })
    }

    fn schema(&self) -> &Schema {
        self.schema()
    }
    fn create_initialized(
        &mut self,
        request: storage::Construction<'_>,
        binding: storage::InitializationBinding<'_>,
        budget: &mut crate::core::query::QueryBudget,
    ) -> StorageResult<intersect_model::definitions::initialization::InitializationReceipt> {
        edit_fork!(self, |store, schema, _validation| {
            storage::InitializationContext {
                schema,
                expected: binding.expected,
                sources: binding.sources,
                provider: binding.provider,
                conversions: binding.conversions,
            }
            .create(store, request, budget)
        })
    }

    fn create(&mut self, id: &str, class: &str, resource: &str) -> StorageResult<()> {
        edit_fork!(self, |store, schema, validation| {
            super::create(store, schema, validation, id, class, resource)
        })
    }
    fn unset(&mut self, id: &str, feature: &str) -> StorageResult<()> {
        edit_fork!(self, |store, schema, _validation| super::unset(
            store, schema, id, feature
        ))
    }
    fn edit_collection(
        &mut self,
        id: &str,
        feature: &str,
        edit: storage::CollectionEdit,
    ) -> StorageResult<bool> {
        edit_fork!(self, |store, schema, _validation| {
            storage::edit_collection(store, schema, id, feature, edit)
        })
    }
    fn set_many(&mut self, edits: &[(&str, &str, Value)]) -> StorageResult<()> {
        edit_fork!(self, |store, schema, validation| super::set_many(
            store, schema, validation, edits
        ))
    }
}

/// Optional owner/publisher composed around a basic, exclusively owned
/// model. Structural and domain validation finish before replacing the head.
pub struct VersionedModel {
    head: Snapshot,
}
impl VersionedModel {
    pub fn new(basic: BasicModel) -> StorageResult<Self> {
        Self::new_context(basic, None)
    }
    /// Adopt validated state without retaining conversion implementations in snapshots.
    pub fn new_with_conversions(
        basic: BasicModel,
        conversions: &ConversionRegistry<'_>,
    ) -> StorageResult<Self> {
        Self::new_context(basic, Some(conversions))
    }
    fn new_context(
        basic: BasicModel,
        conversions: Option<&ConversionRegistry<'_>>,
    ) -> StorageResult<Self> {
        validate_seed(&basic, conversions)?;
        let state = basic.into_state();
        Ok(Self::initial(CommittedStore {
            schema: Arc::new(state.schema),
            validation: Arc::new(state.validation),
            arena: Arc::new(state.store),
        }))
    }

    fn initial(store: CommittedStore) -> Self {
        Self {
            head: Snapshot(Arc::new(Committed {
                store,
                version: VersionId {
                    scope: Arc::new(()),
                    sequence: 0,
                },
            })),
        }
    }
    pub fn snapshot(&self) -> StorageResult<Snapshot> {
        Ok(self.head.clone())
    }
    /// Older snapshots of this model may be explored; publishing requires the
    /// current head as the baseline. A frozen fork clones its editable arena.
    /// Snapshots from other model owners are rejected.
    pub fn fork(&self, snapshot: &Snapshot) -> StorageResult<ModelFork> {
        if !Arc::ptr_eq(&self.head.0.version.scope, &snapshot.0.version.scope) {
            return Err("Snapshot belongs to another versioned model".into());
        }
        Ok(ModelFork {
            base: snapshot.clone(),
            draft: Box::new(snapshot.0.store.arena.fork_arena()?),
        })
    }
    pub fn commit(&mut self, fork: ModelFork) -> StorageResult<VersionId> {
        self.commit_with(fork, |_| Ok(()))
    }
    /// Domain validation is supplied by the caller; core owns no domain policy.
    pub fn commit_with(
        &mut self,
        fork: ModelFork,
        validate: impl FnOnce(&ModelFork) -> StorageResult<()>,
    ) -> StorageResult<VersionId> {
        self.commit_context(fork, validate, None)
    }
    pub fn commit_with_conversions(
        &mut self,
        fork: ModelFork,
        conversions: &ConversionRegistry<'_>,
    ) -> StorageResult<VersionId> {
        self.commit_context(fork, |_| Ok(()), Some(conversions))
    }
    /// Check the baseline and providers before invoking caller-owned domain policy.
    pub fn commit_with_conversions_and_validation(
        &mut self,
        fork: ModelFork,
        conversions: &ConversionRegistry<'_>,
        validate: impl FnOnce(&ModelFork) -> StorageResult<()>,
    ) -> StorageResult<VersionId> {
        self.commit_context(fork, validate, Some(conversions))
    }
    fn commit_context(
        &mut self,
        fork: ModelFork,
        validate: impl FnOnce(&ModelFork) -> StorageResult<()>,
        conversions: Option<&ConversionRegistry<'_>>,
    ) -> StorageResult<VersionId> {
        if fork.base.version() != self.head.version() {
            return Err("Fork has a stale or foreign baseline".into());
        }
        let sequence = self
            .head
            .0
            .version
            .sequence
            .checked_add(1)
            .ok_or("Model version counter exhausted")?;
        match conversions {
            Some(registry) => fork.validate_with_conversions(registry)?,
            None => fork.validate()?,
        }
        validate(&fork)?;
        let version = VersionId {
            scope: self.head.0.version.scope.clone(),
            sequence,
        };
        let store = CommittedStore {
            schema: fork.base.0.store.schema.clone(),
            validation: fork.base.0.store.validation.clone(),
            arena: Arc::from(fork.draft),
        };
        self.head = Snapshot(Arc::new(Committed {
            store,
            version: version.clone(),
        }));
        Ok(version)
    }
}

fn validate_seed(
    basic: &BasicModel<impl storage::ModelStore>,
    conversions: Option<&ConversionRegistry<'_>>,
) -> StorageResult<()> {
    match conversions {
        Some(registry) => basic.validate_with_conversions(registry),
        None => basic.validate(),
    }
}

// Identical read capability for immutable snapshots and mutable drafts.
macro_rules! reads {
    ($model:ty) => {
        impl ModelRead for $model {
            fn resource_records(
                &self,
            ) -> StorageResult<
                std::collections::BTreeMap<String, crate::core::resources::ResourceRecord>,
            > {
                self.with_reader(|reader| reader.resource_records())
            }
            fn resource_record(
                &self,
                uri: &str,
            ) -> StorageResult<Option<crate::core::resources::ResourceRecord>> {
                self.with_reader(|reader| reader.resource_record(uri))
            }
            fn revision(&self) -> StorageResult<String> {
                self.with_reader(|reader| reader.revision())
            }
            fn inspect_revision(
                &self,
                f: &mut dyn FnMut(&str) -> StorageResult<()>,
            ) -> StorageResult<()> {
                self.with_reader(|reader| reader.inspect_revision(f))
            }
            fn get(&self, id: &str) -> StorageResult<Option<crate::core::model::Element>> {
                self.with_reader(|reader| reader.get(id))
            }
            fn class(&self, id: &str) -> StorageResult<Option<String>> {
                self.with_reader(|reader| reader.class(id))
            }
            fn is_pending_reference(
                &self,
                id: &str,
                field: &str,
                index: usize,
                wire: &str,
            ) -> bool {
                self.with_reader(|reader| Ok(reader.is_pending_reference(id, field, index, wire)))
                    .is_ok_and(|pending| pending)
            }
            fn inspect_class(
                &self,
                id: &str,
                f: &mut dyn FnMut(&str) -> StorageResult<()>,
            ) -> StorageResult<bool> {
                self.with_reader(|reader| reader.inspect_class(id, f))
            }
            fn page(&self, after: Option<u64>, limit: usize) -> StorageResult<Vec<storage::Row>> {
                self.with_reader(|reader| reader.page(after, limit))
            }
            fn id_page(
                &self,
                after: Option<u64>,
                limit: usize,
            ) -> StorageResult<Vec<(u64, String)>> {
                self.with_reader(|reader| reader.id_page(after, limit))
            }
            fn query_candidates_observed(
                &self,
                query: storage::Query<'_>,
                after: Option<u64>,
                limit: usize,
                observation: &storage::InvocationContext<'_>,
            ) -> storage::StorageResult<storage::QueryCandidates> {
                self.with_reader(|reader| {
                    reader.query_candidates_observed(query, after, limit, observation)
                })
            }
            fn matches_observed(
                &self,
                id: &str,
                query: storage::Query<'_>,
                observation: &storage::InvocationContext<'_>,
            ) -> storage::StorageResult<bool> {
                self.with_reader(|reader| reader.matches_observed(id, query, observation))
            }
            fn id_page_observed(
                &self,
                after: Option<u64>,
                limit: usize,
                observation: &storage::InvocationContext<'_>,
            ) -> storage::StorageResult<Vec<(u64, String)>> {
                self.with_reader(|reader| reader.id_page_observed(after, limit, observation))
            }
            fn query_candidates(
                &self,
                query: storage::Query<'_>,
                after: Option<u64>,
                limit: usize,
            ) -> StorageResult<storage::QueryCandidates> {
                self.with_reader(|reader| reader.query_candidates(query, after, limit))
            }
            fn query(&self, query: storage::Query<'_>) -> StorageResult<Vec<String>> {
                self.with_reader(|reader| reader.query(query))
            }
            fn matches(&self, id: &str, query: storage::Query<'_>) -> StorageResult<bool> {
                self.with_reader(|reader| reader.matches(id, query))
            }
            fn value_observed(
                &self,
                id: &str,
                field: &str,
                observation: &storage::InvocationContext<'_>,
            ) -> storage::StorageResult<Option<serde_json::Value>> {
                self.with_reader(|reader| reader.value_observed(id, field, observation))
            }
            fn collection_page_observed(
                &self,
                id: &str,
                field: &str,
                offset: usize,
                limit: usize,
                observation: &storage::InvocationContext<'_>,
            ) -> storage::StorageResult<Vec<serde_json::Value>> {
                self.with_reader(|reader| {
                    reader.collection_page_observed(id, field, offset, limit, observation)
                })
            }
            fn value(&self, id: &str, field: &str) -> StorageResult<Option<Value>> {
                self.with_reader(|reader| reader.value(id, field))
            }
            fn value_len(&self, id: &str, field: &str) -> StorageResult<Option<usize>> {
                self.with_reader(|reader| reader.value_len(id, field))
            }
            fn collection_page(
                &self,
                id: &str,
                field: &str,
                offset: usize,
                limit: usize,
            ) -> StorageResult<Vec<Value>> {
                self.with_reader(|reader| reader.collection_page(id, field, offset, limit))
            }
            fn inspect(
                &self,
                id: &str,
                f: &mut dyn FnMut(storage::ObjectView<'_>) -> StorageResult<()>,
            ) -> StorageResult<bool> {
                self.with_reader(|reader| reader.inspect(id, f))
            }
        }
    };
}
reads!(Snapshot);
reads!(ModelFork);
