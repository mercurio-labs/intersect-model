//! Explicit language-owned effective values for a structural validation operation.
//! Values are not inserted into the physical graph or retained as getter authority.
use super::{ModelRead, StorageResult};
use crate::core::query::QueryBudget;
use crate::definitions::schema::{Feature, MemberIdentity, Schema};
use serde_json::Value;

/// Canonical effective member selected by the structural engine, never by the
/// provider. Source/language adapters must check their exact owner/identity and
/// schema before invoking their existing delegated semantic implementation.
#[derive(Clone, Copy)]
pub struct StructuralValueRequest<'a> {
    pub object: &'a str,
    pub actual: &'a str,
    pub owner: &'a str,
    pub identity: &'a MemberIdentity,
    pub feature: &'a Feature,
}

/// Borrowed authority for one immutable structural audit. None keeps the exact
/// raw value path. Some supplies an explicit delegated value only for an absent,
/// source-declared nonderived noncontainment field; raw shadows reject. The
/// engine applies its original shape, lower/upper bounds and target checks.
/// Implementations consume the supplied original caller budget and may not
/// mutate/retain the store or install providers from model metadata.
pub trait StructuralValueSelector {
    /// Optional stack-scoped native query context. The canonical audit owns the
    /// callback result; default selectors retain their literal selection path.
    fn with_immutable_audit(
        &self,
        _audit: &super::ImmutableStructuralAudit<'_>,
        _control: &super::InvocationContext<'_>,
        _run: &mut dyn FnMut(&dyn StructuralValueSelector),
    ) -> StorageResult<bool> {
        Ok(false)
    }
    fn select(
        &self,
        store: &dyn ModelRead,
        schema: &Schema,
        request: StructuralValueRequest<'_>,
        budget: &mut QueryBudget,
    ) -> StorageResult<Option<Value>>;
}

use super::{InvocationContext, StructuralPreparation, ValueView};
use crate::definitions::{
    conversion::{ConversionRegistry, InvocationObserver, ProviderDependencyKind, ProviderId},
    retained_storage,
};
use std::{cell::RefCell, rc::Rc};

struct ValueObservation<'a, 'budget, 'control> {
    caller: &'a RefCell<&'budget mut QueryBudget>,
    control: &'a InvocationContext<'control>,
}
impl InvocationObserver for ValueObservation<'_, '_, '_> {
    fn before_invoke(&self, provider: &ProviderId) -> StorageResult<()> {
        self.control.invoke(provider)
    }
    fn dependency_unavailable(&self, kind: ProviderDependencyKind) {
        self.control.dependency_unavailable(kind);
    }
    fn failed_result(&self, error: &str) {
        self.control.failed_result(error);
    }
    fn checkpoint(&self) -> StorageResult<()> {
        self.control.checkpoint()
    }
    fn work(&self, units: usize) -> StorageResult<()> {
        self.control.checkpoint()?;
        let mut budget = self
            .caller
            .try_borrow_mut()
            .map_err(|_| "Structural value caller already borrowed")?;
        for _ in 0..units {
            budget.charge()?;
        }
        Ok(())
    }
    fn retain_storage(&self, bytes: usize) -> StorageResult<()> {
        self.control.checkpoint()?;
        self.control.retain_storage(bytes)
    }
}

// One operation-local effective value, never a graph, store view or retained
// validity cache. Existing StructuralPreparation layout/default path is exact.
pub(super) struct StructuralValueCursor<'a, 'budget, 'observation> {
    pub(super) store: &'a dyn ModelRead,
    pub(super) schema: &'a Schema,
    selector: &'a dyn StructuralValueSelector,
    caller: &'a RefCell<&'budget mut QueryBudget>,
    observation: &'a InvocationContext<'observation>,
    current: Option<Value>,
    require_selected_absence: bool,
}
impl StructuralValueCursor<'_, '_, '_> {
    pub(super) fn clear(&mut self) {
        self.current = None;
    }
    pub(super) fn select(&mut self, request: StructuralValueRequest<'_>) -> StorageResult<()> {
        self.current = None;
        self.observation.work(1)?;
        let value = {
            let mut budget = self
                .caller
                .try_borrow_mut()
                .map_err(|_| "Structural value caller already borrowed")?;
            self.selector
                .select(self.store, self.schema, request, &mut budget)?
        };
        self.observation.checkpoint()?;
        let Some(value) = value else {
            if self.require_selected_absence {
                self.observation.work(1)?;
                let mut absent = false;
                if !self.store.inspect(request.object, &mut |object| {
                    absent = object.value(&request.feature.name).is_none();
                    Ok(())
                })? {
                    return Err("Missing structural value receiver".into());
                }
                if absent {
                    return Err(format!(
                        "Absent stored member requires an explicit supported structural value [stage=persistent-structural-selection; object={}; actual={}; owner={}; feature={}; feature_identity={:?}; target={}; lower={}; upper={}; reference={}; changeable={}; transient={}; volatile={}]",
                        request.object,
                        request.actual,
                        request.owner,
                        request.feature.name,
                        request.identity,
                        request.feature.target,
                        request.feature.lower,
                        request.feature.upper,
                        request.feature.reference,
                        request.feature.changeable,
                        request.feature.transient,
                        request.feature.volatile,
                    ));
                }
            }
            return Ok(());
        };
        if request.feature.derived
            || request.feature.containment
            || !matches!(request.identity, MemberIdentity::Source { .. })
        {
            return Err("Delegated structural values require an exact source-declared nonderived noncontainment member".into());
        }
        self.observation.work(1)?;
        let mut shadow = false;
        if !self.store.inspect(request.object, &mut |object| {
            if object.class() != request.actual || object.id() != request.object {
                return Err("Structural value actual receiver changed".into());
            }
            shadow = object.value(&request.feature.name).is_some();
            Ok(())
        })? {
            return Err("Missing structural value receiver".into());
        }
        if shadow {
            return Err("Delegated structural value would shadow a raw stored member".into());
        }
        retained_storage::reserve_serialized(&value, self.observation)?;
        self.observation.checkpoint()?;
        self.current = Some(value);
        Ok(())
    }
    pub(super) fn effective<'a>(&'a self, raw: Option<ValueView<'a>>) -> Option<ValueView<'a>> {
        self.current.as_ref().map(ValueView::json).or(raw)
    }
}

/// Full structural validation of the SAME immutable physical store with explicit
/// source/language delegated values. Provider results pass the original shape,
/// requiredness, reference target/conformance and reciprocal checks. The caller
/// and observer are borrowed for this one operation; nothing is installed.
#[allow(clippy::too_many_arguments)] // Explicit borrowed conversions, semantic authority, and original work/control.
pub fn validate_with_structural_values(
    store: &dyn ModelRead,
    schema: &Schema,
    conversions: Option<&ConversionRegistry<'_>>,
    selector: &dyn StructuralValueSelector,
    budget: &mut QueryBudget,
    control: &InvocationContext<'_>,
) -> StorageResult<()> {
    StructuralValueContext::new(selector, budget, control).validate(store, schema, conversions)
}

/// Explicit concrete immutable-store lane over the SAME full validator. The
/// canonical trait-object view is minted once and carried unchanged to every
/// selector; arbitrary ModelRead implementations never acquire this authority.
pub fn validate_native_with_structural_values(
    store: &impl super::DraftConstructionStore,
    schema: &Schema,
    conversions: Option<&ConversionRegistry<'_>>,
    selector: &dyn StructuralValueSelector,
    budget: &mut QueryBudget,
    control: &InvocationContext<'_>,
) -> StorageResult<()> {
    StructuralValueContext::new(selector, budget, control).validate_native(
        store,
        schema,
        conversions,
    )
}

/// Borrowed executable structural authority for a sequence of synchronous
/// operations. It owns neither graph nor providers, and uses only the original
/// caller ledger. A RefCell permits Copy format contexts to borrow this binding;
/// reentrant budget use returns an error instead of creating a second allowance.
pub struct StructuralValueContext<'a> {
    selector: &'a dyn StructuralValueSelector,
    caller: RefCell<&'a mut QueryBudget>,
    control: InvocationContext<'a>,
}
// Private engine: hosts cannot implement a replacement/no-op validator. The
// opaque borrowed handle erases the invariant original-ledger lifetime while
// still admitting only this canonical StructuralPreparation implementation.
trait StructuralValueEngine {
    fn validate(
        &self,
        store: &dyn ModelRead,
        schema: &Schema,
        conversions: Option<&ConversionRegistry<'_>>,
    ) -> StorageResult<()>;
    fn validate_persistent(
        &self,
        store: &dyn ModelRead,
        schema: &Schema,
        conversions: Option<&ConversionRegistry<'_>>,
    ) -> StorageResult<()>;
    fn validate_model(
        &self,
        model: &crate::core::model::Model,
        schema: &Schema,
        conversions: Option<&ConversionRegistry<'_>>,
    ) -> StorageResult<()>;
    #[cfg(feature = "xmi")]
    fn verify_fresh_readonly(
        &self,
        model: &crate::core::model::Model,
        schema: &Schema,
        request: super::FreshReadonlySnapshot<'_>,
        selector: &dyn super::SnapshotInitializationSelector,
    ) -> StorageResult<super::VerifiedReadonlyInitialization>;
    #[cfg(feature = "xmi")]
    fn seal_readonly(
        &self,
        model: &crate::core::model::Model,
        schema: &Schema,
        rows: Vec<super::VerifiedReadonlyInitialization>,
    ) -> StorageResult<Option<super::ReadonlyDecodeProof>>;
    fn check_decode_proof(
        &self,
        model: &crate::core::model::Model,
        schema: &Schema,
        proof: &super::ReadonlyDecodeProof,
        selected: &std::collections::BTreeSet<String>,
    ) -> StorageResult<()>;
    fn revalidate_decode_proof(
        &self,
        store: &dyn ModelRead,
        schema: &Schema,
        proof: &super::ReadonlyDecodeProof,
        selector: &dyn super::SnapshotInitializationSelector,
    ) -> StorageResult<()>;
}
#[derive(Clone, Copy)]
pub struct BorrowedStructuralValues<'a>(&'a dyn StructuralValueEngine);
impl BorrowedStructuralValues<'_> {
    pub fn validate(
        &self,
        store: &dyn ModelRead,
        schema: &Schema,
        conversions: Option<&ConversionRegistry<'_>>,
    ) -> StorageResult<()> {
        self.0.validate(store, schema, conversions)
    }
    pub fn validate_persistent(
        &self,
        store: &dyn ModelRead,
        schema: &Schema,
        conversions: Option<&ConversionRegistry<'_>>,
    ) -> StorageResult<()> {
        self.0.validate_persistent(store, schema, conversions)
    }
    pub(crate) fn validate_model(
        &self,
        model: &crate::core::model::Model,
        schema: &Schema,
        conversions: Option<&ConversionRegistry<'_>>,
    ) -> StorageResult<()> {
        self.0.validate_model(model, schema, conversions)
    }
    #[cfg(feature = "xmi")]
    pub(crate) fn verify_fresh_readonly(
        &self,
        model: &crate::core::model::Model,
        schema: &Schema,
        request: super::FreshReadonlySnapshot<'_>,
        selector: &dyn super::SnapshotInitializationSelector,
    ) -> StorageResult<super::VerifiedReadonlyInitialization> {
        self.0
            .verify_fresh_readonly(model, schema, request, selector)
    }
    #[cfg(feature = "xmi")]
    pub(crate) fn seal_readonly(
        &self,
        model: &crate::core::model::Model,
        schema: &Schema,
        rows: Vec<super::VerifiedReadonlyInitialization>,
    ) -> StorageResult<Option<super::ReadonlyDecodeProof>> {
        self.0.seal_readonly(model, schema, rows)
    }
    pub(crate) fn check_decode_proof(
        &self,
        model: &crate::core::model::Model,
        schema: &Schema,
        proof: &super::ReadonlyDecodeProof,
        selected: &std::collections::BTreeSet<String>,
    ) -> StorageResult<()> {
        self.0.check_decode_proof(model, schema, proof, selected)
    }
    pub(crate) fn revalidate_decode_proof(
        &self,
        store: &dyn ModelRead,
        schema: &Schema,
        proof: &super::ReadonlyDecodeProof,
        selector: &dyn super::SnapshotInitializationSelector,
    ) -> StorageResult<()> {
        self.0
            .revalidate_decode_proof(store, schema, proof, selector)
    }
}
impl StructuralValueEngine for StructuralValueContext<'_> {
    fn validate(
        &self,
        store: &dyn ModelRead,
        schema: &Schema,
        conversions: Option<&ConversionRegistry<'_>>,
    ) -> StorageResult<()> {
        StructuralValueContext::validate(self, store, schema, conversions)
    }
    fn validate_persistent(
        &self,
        store: &dyn ModelRead,
        schema: &Schema,
        conversions: Option<&ConversionRegistry<'_>>,
    ) -> StorageResult<()> {
        StructuralValueContext::validate_persistent(self, store, schema, conversions)
    }
    fn validate_model(
        &self,
        model: &crate::core::model::Model,
        schema: &Schema,
        conversions: Option<&ConversionRegistry<'_>>,
    ) -> StorageResult<()> {
        StructuralValueContext::validate_model(self, model, schema, conversions)
    }
    #[cfg(feature = "xmi")]
    fn verify_fresh_readonly(
        &self,
        model: &crate::core::model::Model,
        schema: &Schema,
        request: super::FreshReadonlySnapshot<'_>,
        selector: &dyn super::SnapshotInitializationSelector,
    ) -> StorageResult<super::VerifiedReadonlyInitialization> {
        let observation = self.observation()?;
        let fresh = super::ModelView::new_observed(model, &observation)?;
        let binding = self.with_budget(|budget| {
            budget.charge()?;
            let binding = selector
                .select(&fresh, schema, request.object, budget)?
                .ok_or("Required fresh snapshot initializer is not installed")?;
            observation.checkpoint()?;
            let context = super::InitializationContext {
                schema,
                expected: binding.expected,
                sources: binding.sources,
                provider: binding.provider,
                conversions: binding.conversions,
            };
            context.verify_fresh_readonly(&fresh, request, budget)?;
            observation.checkpoint()?;
            Ok(binding)
        })?;
        retained_storage::reserve_serialized(binding.expected, &observation)?;
        retained_storage::reserve_serialized(&request.values, &observation)?;
        observation.retain_storage(256)?;
        let mut resource = None;
        fresh.inspect(request.object, &mut |row| {
            observation.retain_storage(row.resource().len())?;
            resource = Some(row.resource().to_owned());
            Ok(())
        })?;
        observation.retain_storage(request.object.len())?;
        let verified = super::VerifiedReadonlyInitialization {
            object: request.object.into(),
            resource: resource.ok_or("Missing fresh snapshot receiver")?,
            contract: binding.expected.clone(),
            values: request.values.to_vec(),
        };
        observation.checkpoint()?;
        if binding
            .provider
            .ok_or("Missing fresh snapshot initializer")?
            .contract()
            != binding.expected
        {
            return Err("Fresh snapshot initializer changed after proof admission".into());
        }
        Ok(verified)
    }
    #[cfg(feature = "xmi")]
    fn seal_readonly(
        &self,
        model: &crate::core::model::Model,
        schema: &Schema,
        rows: Vec<super::VerifiedReadonlyInitialization>,
    ) -> StorageResult<Option<super::ReadonlyDecodeProof>> {
        super::ReadonlyDecodeProof::seal(schema, model, rows, &self.observation()?)
    }
    fn check_decode_proof(
        &self,
        model: &crate::core::model::Model,
        schema: &Schema,
        proof: &super::ReadonlyDecodeProof,
        selected: &std::collections::BTreeSet<String>,
    ) -> StorageResult<()> {
        proof.check_model(schema, model, &self.observation()?)?;
        proof.check_selection(selected)
    }
    fn revalidate_decode_proof(
        &self,
        store: &dyn ModelRead,
        schema: &Schema,
        proof: &super::ReadonlyDecodeProof,
        selector: &dyn super::SnapshotInitializationSelector,
    ) -> StorageResult<()> {
        let observation = self.observation()?;
        self.with_budget(|budget| proof.revalidate(store, schema, selector, budget, &observation))
    }
}
impl<'a> StructuralValueContext<'a> {
    /// Copyable borrowed authority for format/transaction composition. Its
    /// private constructor cannot wrap arbitrary validation callbacks.
    pub fn borrowed(&self) -> BorrowedStructuralValues<'_> {
        BorrowedStructuralValues(self)
    }

    pub fn new(
        selector: &'a dyn StructuralValueSelector,
        budget: &'a mut QueryBudget,
        control: &InvocationContext<'a>,
    ) -> Self {
        Self {
            selector,
            caller: RefCell::new(budget),
            control: control.clone(),
        }
    }
    pub fn with_budget<T>(
        &self,
        operation: impl FnOnce(&mut QueryBudget) -> StorageResult<T>,
    ) -> StorageResult<T> {
        let mut caller = self
            .caller
            .try_borrow_mut()
            .map_err(|_| "Structural value caller already borrowed")?;
        operation(&mut caller)
    }
    fn observation(&self) -> StorageResult<InvocationContext<'_>> {
        self.control.checkpoint()?;
        self.with_budget(|caller| caller.charge())?;
        // Preflight the additional operation carrier before allocating its Rc.
        self.control.retain_storage(256)?;
        Ok(InvocationContext::new(Rc::new(ValueObservation {
            caller: &self.caller,
            control: &self.control,
        })))
    }
    /// Ordinary structural semantics: None follows the unchanged raw value path.
    pub fn validate(
        &self,
        store: &dyn ModelRead,
        schema: &Schema,
        conversions: Option<&ConversionRegistry<'_>>,
    ) -> StorageResult<()> {
        self.validate_inner(
            store,
            schema,
            conversions,
            &self.observation()?,
            false,
            None,
        )
    }
    /// Codec/publication semantics additionally require explicit selection for
    /// every absent nonderived noncontainment field. An optional raw absence is
    /// not a delegated storage contract merely because its lower bound is zero.
    pub fn validate_persistent(
        &self,
        store: &dyn ModelRead,
        schema: &Schema,
        conversions: Option<&ConversionRegistry<'_>>,
    ) -> StorageResult<()> {
        self.validate_inner(store, schema, conversions, &self.observation()?, true, None)
    }
    pub(crate) fn validate_model(
        &self,
        model: &crate::core::model::Model,
        schema: &Schema,
        conversions: Option<&ConversionRegistry<'_>>,
    ) -> StorageResult<()> {
        let observation = self.observation()?;
        let view = super::ModelView::new_observed(model, &observation)?;
        self.validate_inner(&view, schema, conversions, &observation, true, None)
    }
    /// Opt in only through the sealed native store boundary.
    pub fn validate_native(
        &self,
        store: &impl super::DraftConstructionStore,
        schema: &Schema,
        conversions: Option<&ConversionRegistry<'_>>,
    ) -> StorageResult<()> {
        let observation = self.observation()?;
        let audit = super::ImmutableStructuralAudit::native(store, schema);
        self.validate_inner(
            audit.store(),
            schema,
            conversions,
            &observation,
            false,
            Some(&audit),
        )
    }
    fn validate_inner(
        &self,
        store: &dyn ModelRead,
        schema: &Schema,
        conversions: Option<&ConversionRegistry<'_>>,
        observation: &InvocationContext<'_>,
        require_selected_absence: bool,
        audit: Option<&super::ImmutableStructuralAudit<'_>>,
    ) -> StorageResult<()> {
        observation.checkpoint()?;
        let mut run = |selector: &dyn StructuralValueSelector| {
            let mut values = StructuralValueCursor {
                store,
                schema,
                selector,
                caller: &self.caller,
                observation,
                current: None,
                require_selected_absence,
            };
            let mut pending = StructuralPreparation::new(true, false);
            while !pending.advance_with_structural_values(
                store,
                schema,
                conversions,
                observation,
                &mut values,
            )? {}
            pending.finish().map(|_| ())
        };
        if let Some(audit) = audit {
            if !audit.matches(store, schema) {
                return Err("Immutable structural audit does not match actual store/schema".into());
            }
            super::with_immutable_structural_audit(self.selector, audit, observation, &mut run)
        } else {
            run(self.selector)
        }
    }
}
