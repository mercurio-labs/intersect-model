//! Handwritten basic model facade, independent of higher-level services.

use super::{ModelEdit, ModelState, create, require_idle, set_many};
use crate::core::frozen::FrozenStore;
use crate::core::storage::{self, ModelStore, StorageResult};
use intersect_model::definitions::{conversion::ConversionRegistry, schema::Schema};
use serde_json::Value;

/// An owned schema and store with individually atomic, checked edits.
///
/// Construction and basic edits allow missing required values. Call `validate`
/// explicitly to require a complete structurally valid model. Isolated stores use
/// bounded checks for eligible creation and attribute edits; relationship edits
/// and unqualified stores retain whole-model checks. This is not a scale claim.
pub struct BasicModel<S: ModelStore = FrozenStore> {
    state: ModelState<S>,
}

impl<S: storage::DraftConstructionStore> BasicModel<S> {
    #[allow(clippy::too_many_arguments)] // SAME admission services; only the concrete validation lane differs.
    pub(crate) fn from_native_with_structural_values(
        schema: Schema,
        store: S,
        conversions: Option<&ConversionRegistry<'_>>,
        values: &dyn storage::StructuralValueSelector,
        budget: &mut crate::core::query::QueryBudget,
        control: &storage::InvocationContext<'_>,
        validate_language: impl FnOnce(&S, &mut crate::core::query::QueryBudget) -> StorageResult<()>,
    ) -> StorageResult<Self> {
        Self::from_store_with_structural_validation(
            schema,
            store,
            budget,
            control,
            |store, schema, budget| {
                storage::validate_native_with_structural_values(
                    store,
                    schema,
                    conversions,
                    values,
                    budget,
                    control,
                )
            },
            validate_language,
        )
    }
}

impl BasicModel<FrozenStore> {
    /// Capture an isolated, immutable snapshot, allowing missing required values.
    /// The native structural profile is checked; provider-backed values require
    /// their separately qualified snapshot admission profile.
    pub fn snapshot(&self) -> StorageResult<super::ModelSnapshot> {
        super::versioned::snapshot_draft(
            std::sync::Arc::new(self.state.schema.clone()),
            self.state.store.fork_arena()?,
        )
    }

    pub fn new(schema: Schema) -> StorageResult<Self> {
        let store = FrozenStore::for_schema(&schema).map_err(|error| error.to_string())?;
        Self::from_store(schema, store)
    }

    pub fn new_with_conversions(
        schema: Schema,
        conversions: &ConversionRegistry<'_>,
    ) -> StorageResult<Self> {
        let store = FrozenStore::for_schema(&schema).map_err(|error| error.to_string())?;
        Self::from_store_with_conversions(schema, store, conversions)
    }
}

impl<S: ModelStore> BasicModel<S> {
    /// Internal residency adoption. Unavailable data never acquires full model
    /// validity; ResourceSet gates editing/execution until a validated reload.
    pub(crate) fn from_resident_store(schema: Schema, mut store: S) -> StorageResult<Self> {
        require_idle(&store)?;
        storage::atomic(&mut store, |store| {
            storage::bind_schema(store, &schema)?;
            crate::core::resources::validate_records(store, &schema)
        })?;
        let validation = storage::DraftValidationPlan::new(&schema)?;
        Ok(Self {
            state: ModelState {
                schema,
                store,
                validation,
            },
        })
    }
    /// Adopt a store only when it has no active transaction and its existing data
    /// passes draft structural validation against the supplied schema.
    pub fn from_store(schema: Schema, store: S) -> StorageResult<Self> {
        Self::from_store_context(schema, store, None)
    }
    pub fn from_store_with_conversions(
        schema: Schema,
        store: S,
        conversions: &ConversionRegistry<'_>,
    ) -> StorageResult<Self> {
        Self::from_store_context(schema, store, Some(conversions))
    }
    fn from_store_context(
        schema: Schema,
        mut store: S,
        conversions: Option<&ConversionRegistry<'_>>,
    ) -> StorageResult<Self> {
        require_idle(&store)?;
        storage::atomic(&mut store, |store| {
            storage::bind_schema(store, &schema)?;
            super::draft(store, &schema, conversions)
        })?;
        let validation = storage::DraftValidationPlan::new(&schema)?;
        Ok(Self {
            state: ModelState {
                schema,
                store,
                validation,
            },
        })
    }

    /// Internal full admission with explicit effective values. Binding and all
    /// validation remain in one checkpoint; no unchecked facade can escape.
    pub(crate) fn from_store_with_structural_values(
        schema: Schema,
        store: S,
        conversions: Option<&ConversionRegistry<'_>>,
        values: &dyn storage::StructuralValueSelector,
        budget: &mut crate::core::query::QueryBudget,
        control: &storage::InvocationContext<'_>,
        validate_language: impl FnOnce(&S, &mut crate::core::query::QueryBudget) -> StorageResult<()>,
    ) -> StorageResult<Self> {
        Self::from_store_with_structural_validation(
            schema,
            store,
            budget,
            control,
            |store, schema, budget| {
                storage::validate_with_structural_values(
                    store,
                    schema,
                    conversions,
                    values,
                    budget,
                    control,
                )
            },
            validate_language,
        )
    }
    fn from_store_with_structural_validation(
        schema: Schema,
        mut store: S,
        budget: &mut crate::core::query::QueryBudget,
        control: &storage::InvocationContext<'_>,
        validate_structure: impl FnOnce(
            &S,
            &Schema,
            &mut crate::core::query::QueryBudget,
        ) -> StorageResult<()>,
        validate_language: impl FnOnce(&S, &mut crate::core::query::QueryBudget) -> StorageResult<()>,
    ) -> StorageResult<Self> {
        require_idle(&store)?;
        let validation = storage::DraftValidationPlan::new(&schema)?;
        storage::atomic(&mut store, |store| {
            control.checkpoint()?;
            storage::bind_schema(store, &schema)?;
            validate_structure(store, &schema, budget)?;
            validate_language(store, budget)?;
            control.checkpoint()
        })?;
        Ok(Self {
            state: ModelState {
                schema,
                store,
                validation,
            },
        })
    }

    pub fn schema(&self) -> &Schema {
        &self.state.schema
    }

    /// Check full structure, including required lower multiplicities.
    /// Language-specific validation is a separate service.
    pub fn validate(&self) -> StorageResult<()> {
        self.state
            .validation
            .validate_all(&self.state.store, &self.state.schema, true)
    }

    /// Validate using caller-owned providers; no registry is retained by the model.
    pub fn validate_with_conversions(
        &self,
        conversions: &ConversionRegistry<'_>,
    ) -> StorageResult<()> {
        storage::validate_with_conversions(&self.state.store, &self.state.schema, conversions)
    }
    pub fn create_with_conversions(
        &mut self,
        id: &str,
        class: &str,
        resource: &str,
        conversions: &ConversionRegistry<'_>,
    ) -> StorageResult<()> {
        require_idle(&self.state.store)?;
        super::create_context(
            &mut self.state.store,
            &self.state.schema,
            &self.state.validation,
            id,
            class,
            resource,
            Some(conversions),
        )
    }
    pub fn set_many_with_conversions(
        &mut self,
        edits: &[(&str, &str, Value)],
        conversions: &ConversionRegistry<'_>,
    ) -> StorageResult<()> {
        require_idle(&self.state.store)?;
        super::set_many_context(
            &mut self.state.store,
            &self.state.schema,
            &self.state.validation,
            edits,
            Some(conversions),
        )
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
    pub fn unset_with_conversions(
        &mut self,
        id: &str,
        feature: &str,
        conversions: &ConversionRegistry<'_>,
    ) -> StorageResult<()> {
        require_idle(&self.state.store)?;
        super::unset_context(
            &mut self.state.store,
            &self.state.schema,
            id,
            feature,
            Some(conversions),
        )
    }
    pub fn edit_collection_with_conversions(
        &mut self,
        id: &str,
        feature: &str,
        edit: storage::CollectionEdit,
        conversions: &ConversionRegistry<'_>,
    ) -> StorageResult<bool> {
        require_idle(&self.state.store)?;
        storage::edit_collection_with_conversions(
            &mut self.state.store,
            &self.state.schema,
            id,
            feature,
            edit,
            conversions,
        )
    }

    // Ownership-only boundary for sibling facades. No borrowed mutable store is
    // exposed, and the basic layer does not name or construct any higher service.
    pub(super) fn into_state(self) -> ModelState<S> {
        self.state
    }

    pub(super) fn from_state(state: ModelState<S>) -> Self {
        Self { state }
    }

    /// Relinquish checked ownership. Subsequent raw writes are the caller's duty.
    pub fn into_store(self) -> S {
        self.state.store
    }

    fn reader(&self) -> &S {
        &self.state.store
    }
}

impl<S: ModelStore> ModelEdit for BasicModel<S> {
    fn create_resource(&mut self, uri: &str) -> StorageResult<()> {
        require_idle(&self.state.store)?;
        crate::core::resources::create_resource(&mut self.state.store, uri)
    }
    fn reorder_roots(&mut self, uri: &str, roots: &[String]) -> StorageResult<()> {
        require_idle(&self.state.store)?;
        crate::core::resources::reorder_roots(&mut self.state.store, uri, roots)
    }
    fn reparent(
        &mut self,
        request: storage::ReparentRequest<'_>,
        conversions: Option<&ConversionRegistry<'_>>,
        budget: &mut crate::core::query::QueryBudget,
    ) -> StorageResult<bool> {
        require_idle(&self.state.store)?;
        storage::StructuralContext {
            schema: &self.state.schema,
            conversions,
        }
        .reparent(&mut self.state.store, request, budget)
    }

    fn delete_objects(
        &mut self,
        request: storage::DeleteRequest<'_>,
        conversions: Option<&ConversionRegistry<'_>>,
        budget: &mut crate::core::query::QueryBudget,
    ) -> StorageResult<Vec<String>> {
        require_idle(&self.state.store)?;
        storage::StructuralContext {
            schema: &self.state.schema,
            conversions,
        }
        .delete(&mut self.state.store, request, budget)
    }

    fn edit_computed(
        &mut self,
        id: &str,
        feature: &str,
        edit: storage::ComputedEdit,
        context: storage::SettingContext<'_>,
        budget: &mut crate::core::query::QueryBudget,
    ) -> StorageResult<()> {
        require_idle(&self.state.store)?;
        storage::ComputedSettings {
            schema: &self.state.schema,
            provider: context.provider,
            conversions: context.conversions,
        }
        .apply(&mut self.state.store, id, feature, edit, budget)
    }

    fn schema(&self) -> &Schema {
        &self.state.schema
    }

    fn create_initialized(
        &mut self,
        request: storage::Construction<'_>,
        binding: storage::InitializationBinding<'_>,
        budget: &mut crate::core::query::QueryBudget,
    ) -> StorageResult<intersect_model::definitions::initialization::InitializationReceipt> {
        require_idle(&self.state.store)?;
        storage::InitializationContext {
            schema: &self.state.schema,
            expected: binding.expected,
            sources: binding.sources,
            provider: binding.provider,
            conversions: binding.conversions,
        }
        .create(&mut self.state.store, request, budget)
    }

    fn create(&mut self, id: &str, class: &str, resource: &str) -> StorageResult<()> {
        require_idle(&self.state.store)?;
        create(
            &mut self.state.store,
            &self.state.schema,
            &self.state.validation,
            id,
            class,
            resource,
        )
    }

    fn unset(&mut self, id: &str, feature: &str) -> StorageResult<()> {
        require_idle(&self.state.store)?;
        super::unset(&mut self.state.store, &self.state.schema, id, feature)
    }
    fn edit_collection(
        &mut self,
        id: &str,
        feature: &str,
        edit: storage::CollectionEdit,
    ) -> StorageResult<bool> {
        require_idle(&self.state.store)?;
        storage::edit_collection(&mut self.state.store, &self.state.schema, id, feature, edit)
    }
    fn set_many(&mut self, edits: &[(&str, &str, Value)]) -> StorageResult<()> {
        require_idle(&self.state.store)?;
        set_many(
            &mut self.state.store,
            &self.state.schema,
            &self.state.validation,
            edits,
        )
    }
}

forward_reads!(BasicModel<S>);
