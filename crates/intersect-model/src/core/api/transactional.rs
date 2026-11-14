//! Handwritten opt-in transaction service composed over owned model state.

use super::{BasicModel, ModelEdit, ModelState, require_idle};
use crate::core::{
    frozen::FrozenStore,
    storage::{self, ModelStore, StorageResult},
};
use intersect_model::definitions::{conversion::ConversionRegistry, schema::Schema};
use serde_json::Value;

/// Opt-in grouped editing. All mutation requires a transaction callback; read
/// services use the same `ModelRead` interface as a basic model.
///
/// The callback exclusively borrows this model. It is not an optimistic detached
/// transaction and does not replace the existing scripting publication API.
pub struct TransactionalModel<S: ModelStore = FrozenStore> {
    state: ModelState<S>,
}

impl<S: ModelStore> TransactionalModel<S> {
    pub fn new(model: BasicModel<S>) -> Self {
        Self {
            state: model.into_state(),
        }
    }

    pub fn schema(&self) -> &Schema {
        &self.state.schema
    }

    pub fn validate(&self) -> StorageResult<()> {
        self.state
            .validation
            .validate_all(&self.state.store, &self.state.schema, true)
    }

    pub fn validate_with_conversions(
        &self,
        conversions: &ConversionRegistry<'_>,
    ) -> StorageResult<()> {
        storage::validate_with_conversions(&self.state.store, &self.state.schema, conversions)
    }

    pub fn into_basic(self) -> BasicModel<S> {
        BasicModel::from_state(self.state)
    }

    /// Commit all edits only after full structural validation. A returned callback
    /// error rolls back all edits. Unwinding uses the store's best-effort guard.
    pub fn transaction<T>(
        &mut self,
        edit: impl FnOnce(&mut EditSession<'_, S>) -> StorageResult<T>,
    ) -> StorageResult<T> {
        self.transaction_with(edit, |_| Ok(()))
    }

    /// Run a read-only domain validator after structural checks and before commit.
    /// The validator is a policy for this call, not a permanently installed policy.
    /// Its error rolls back the entire batch. Rollback failures are reported by the
    /// underlying atomic service, and subsequent outer edits reject an active store.
    pub fn transaction_with<T>(
        &mut self,
        edit: impl FnOnce(&mut EditSession<'_, S>) -> StorageResult<T>,
        validate_language: impl FnOnce(&EditSession<'_, S>) -> StorageResult<()>,
    ) -> StorageResult<T> {
        self.transaction_context(edit, validate_language, None, None)
    }

    /// Borrow converters for this transaction only. Every checked edit and the
    /// final structural validation use the same registry; no provider is retained.
    pub fn transaction_with_conversions<T>(
        &mut self,
        conversions: &ConversionRegistry<'_>,
        edit: impl FnOnce(&mut EditSession<'_, S>) -> StorageResult<T>,
    ) -> StorageResult<T> {
        self.transaction_context(edit, |_| Ok(()), Some(conversions), None)
    }
    pub fn transaction_with_conversions_and_validation<T>(
        &mut self,
        conversions: &ConversionRegistry<'_>,
        edit: impl FnOnce(&mut EditSession<'_, S>) -> StorageResult<T>,
        validate_language: impl FnOnce(&EditSession<'_, S>) -> StorageResult<()>,
    ) -> StorageResult<T> {
        self.transaction_context(edit, validate_language, Some(conversions), None)
    }
    /// Use the existing atomic checked edit session, then the same explicit
    /// structural-value audit before the language callback and commit.
    pub fn transaction_with_format_context<T>(
        &mut self,
        formats: &crate::core::interchange::FormatContext<'_>,
        edit: impl FnOnce(&mut EditSession<'_, S>) -> StorageResult<T>,
        validate_language: impl FnOnce(&EditSession<'_, S>) -> StorageResult<()>,
    ) -> StorageResult<T> {
        self.transaction_context(
            edit,
            validate_language,
            formats.conversions,
            formats.structural_values,
        )
    }
    fn transaction_context<T>(
        &mut self,
        edit: impl FnOnce(&mut EditSession<'_, S>) -> StorageResult<T>,
        validate_language: impl FnOnce(&EditSession<'_, S>) -> StorageResult<()>,
        conversions: Option<&ConversionRegistry<'_>>,
        structural_values: Option<storage::BorrowedStructuralValues<'_>>,
    ) -> StorageResult<T> {
        require_idle(&self.state.store)?;
        let schema = &self.state.schema;
        let validation = &self.state.validation;
        storage::atomic(&mut self.state.store, |store| {
            let mut session = EditSession {
                schema,
                store,
                validation,
                conversions,
            };
            let result = edit(&mut session)?;
            if let Some(values) = structural_values {
                values.validate(session.reader(), schema, conversions)?;
            } else {
                session.validate()?;
            }
            validate_language(&session)?;
            Ok(result)
        })
    }

    pub(crate) fn reader(&self) -> &S {
        &self.state.store
    }
    pub(crate) fn store_mut(&mut self) -> &mut S {
        &mut self.state.store
    }
}

/// A scoped checked edit capability inside a transaction. No raw mutable store
/// access is exposed. Each operation has its own checkpoint, so a caught operation
/// error preserves earlier successful edits without retaining partial writes.
pub struct EditSession<'a, S: ModelStore> {
    schema: &'a Schema,
    store: &'a mut S,
    validation: &'a storage::DraftValidationPlan,
    conversions: Option<&'a ConversionRegistry<'a>>,
}

impl<S: ModelStore> EditSession<'_, S> {
    pub fn validate(&self) -> StorageResult<()> {
        match self.conversions {
            Some(registry) => storage::validate_with_conversions(self.store, self.schema, registry),
            None => self.validation.validate_all(self.store, self.schema, true),
        }
    }

    // Internal adapters may borrow checked editing and immutable projections,
    // but cannot obtain raw mutable storage.
    pub(crate) fn reborrow(&mut self) -> EditSession<'_, S> {
        EditSession {
            schema: self.schema,
            store: self.store,
            validation: self.validation,
            conversions: self.conversions,
        }
    }

    pub(crate) fn reader(&self) -> &S {
        self.store
    }
}

impl<S: ModelStore> ModelEdit for EditSession<'_, S> {
    fn create_resource(&mut self, uri: &str) -> StorageResult<()> {
        crate::core::resources::create_resource(self.store, uri)
    }
    fn reorder_roots(&mut self, uri: &str, roots: &[String]) -> StorageResult<()> {
        crate::core::resources::reorder_roots(self.store, uri, roots)
    }
    fn reparent(
        &mut self,
        request: storage::ReparentRequest<'_>,
        conversions: Option<&ConversionRegistry<'_>>,
        budget: &mut crate::core::query::QueryBudget,
    ) -> StorageResult<bool> {
        require_active(self.store)?;
        storage::StructuralContext {
            schema: self.schema,
            conversions: conversions.or(self.conversions),
        }
        .reparent(self.store, request, budget)
    }

    fn delete_objects(
        &mut self,
        request: storage::DeleteRequest<'_>,
        conversions: Option<&ConversionRegistry<'_>>,
        budget: &mut crate::core::query::QueryBudget,
    ) -> StorageResult<Vec<String>> {
        require_active(self.store)?;
        storage::StructuralContext {
            schema: self.schema,
            conversions: conversions.or(self.conversions),
        }
        .delete(self.store, request, budget)
    }

    fn edit_computed(
        &mut self,
        id: &str,
        feature: &str,
        edit: storage::ComputedEdit,
        context: storage::SettingContext<'_>,
        budget: &mut crate::core::query::QueryBudget,
    ) -> StorageResult<()> {
        require_active(self.store)?;
        storage::ComputedSettings {
            schema: self.schema,
            provider: context.provider,
            conversions: context.conversions.or(self.conversions),
        }
        .apply(self.store, id, feature, edit, budget)
    }

    fn schema(&self) -> &Schema {
        self.schema
    }

    fn create_initialized(
        &mut self,
        request: storage::Construction<'_>,
        binding: storage::InitializationBinding<'_>,
        budget: &mut crate::core::query::QueryBudget,
    ) -> StorageResult<intersect_model::definitions::initialization::InitializationReceipt> {
        require_active(self.store)?;
        storage::InitializationContext {
            schema: self.schema,
            expected: binding.expected,
            sources: binding.sources,
            provider: binding.provider,
            conversions: binding.conversions.or(self.conversions),
        }
        .create(self.store, request, budget)
    }

    fn create(&mut self, id: &str, class: &str, resource: &str) -> StorageResult<()> {
        require_active(self.store)?;
        super::create_context(
            self.store,
            self.schema,
            self.validation,
            id,
            class,
            resource,
            self.conversions,
        )
    }

    fn unset(&mut self, id: &str, feature: &str) -> StorageResult<()> {
        require_active(self.store)?;
        super::unset_context(self.store, self.schema, id, feature, self.conversions)
    }
    fn edit_collection(
        &mut self,
        id: &str,
        feature: &str,
        edit: storage::CollectionEdit,
    ) -> StorageResult<bool> {
        require_active(self.store)?;
        match self.conversions {
            Some(registry) => storage::edit_collection_with_conversions(
                self.store,
                self.schema,
                id,
                feature,
                edit,
                registry,
            ),
            None => storage::edit_collection(self.store, self.schema, id, feature, edit),
        }
    }
    fn set_many(&mut self, edits: &[(&str, &str, Value)]) -> StorageResult<()> {
        require_active(self.store)?;
        super::set_many_context(
            self.store,
            self.schema,
            self.validation,
            edits,
            self.conversions,
        )
    }
}

fn require_active(store: &impl ModelStore) -> StorageResult<()> {
    if store.is_transaction_active() {
        Ok(())
    } else {
        Err("Edit session requires an active transaction".into())
    }
}

forward_reads!(TransactionalModel<S>);
forward_reads!(EditSession<'_, S>);
