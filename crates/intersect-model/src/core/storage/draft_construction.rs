//! Source-bound physical defaults for immutable draft observations only.
//! This is not language validation or strict class-initializer authority.
use super::*;
use crate::core::query::QueryBudget;
use crate::definitions::{
    bundle::{DefinitionBundle, SourceId},
    schema::MemberIdentity,
};
use serde::Serialize;
use std::borrow::Cow;

#[path = "draft_construction_source.rs"]
mod source;
pub use source::DraftConstructionSource;
use source::{ConstructionSources, NoConstructionSources};

mod setting_stores {
    pub trait Sealed {}
    impl Sealed for crate::core::frozen::FrozenStore {}
}

/// Native stores eligible for the source-bound provisional constructor.
/// External adapters cannot supply storage authority to this service.
#[doc(hidden)]
pub trait DraftConstructionStore: ModelStore + setting_stores::Sealed {}
impl DraftConstructionStore for crate::core::frozen::FrozenStore {}

#[cfg(test)]
#[path = "draft_construction_absence_tests.rs"]
mod absence_tests;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub enum DraftDisposition {
    StoredDefault,
    /// Source-qualified native constructor absence in a physically present slot.
    /// Optional null is not a present codec payload or a semantic getter.
    StoredNullDefault,
    /// A native constructor physically starts this required scalar slot null.
    /// Pending draft construction may observe it; completed constructor-aware
    /// snapshots require a nonnull payload, then run the canonical type audit.
    /// This does not waive any other lower bound or grant semantic validity.
    StoredRequiredNullDefault,
    DelegatedAbsent,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct DraftMemberDisposition {
    pub name: String,
    pub identity: MemberIdentity,
    pub disposition: DraftDisposition,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct DraftClassContract {
    pub version: u32,
    pub schema_revision: String,
    pub class: String,
    pub class_identity: String,
    pub sources: Vec<SourceId>,
    pub members: Vec<DraftMemberDisposition>,
}

/// Implemented by a native, caller-selected language policy. Deserialized
/// metadata alone cannot activate this service. No semantic getter is inferred.
pub trait DraftPhysicalDefaults {
    fn contracts(&self) -> &[DraftClassContract];
}

#[derive(Clone, Debug, Serialize)]
pub struct DraftConstructionReceipt {
    pub object: String,
    pub contract: DraftClassContract,
}

// The existing map retains checked names. Null overrides add heap state only
// inside this new draft context; no Element/Machine/legacy holder is widened.
struct StoredSelection {
    names: BTreeSet<String>,
    null_defaults: BTreeSet<String>,
    // Context-wide route, repeated in existing private per-class map values;
    // no public context/engine holder field.
    absent_many: bool,
    // Context-wide completion obligation; fits the existing row's padding.
    required_nulls: bool,
}
const _: () = {
    struct PreviousStoredSelection {
        _names: BTreeSet<String>,
        _null_defaults: BTreeSet<String>,
        _absent_many: bool,
    }
    assert!(
        std::mem::size_of::<StoredSelection>() == std::mem::size_of::<PreviousStoredSelection>()
    );
    assert!(
        std::mem::align_of::<StoredSelection>() == std::mem::align_of::<PreviousStoredSelection>()
    );
};

/// Checked physical disposition binding: dynamic provider or frozen owned maps.
pub struct DraftConstructionContext<'a> {
    schema: &'a Schema,
    provider: Option<&'a dyn DraftPhysicalDefaults>,
    contracts: BTreeMap<String, Cow<'a, DraftClassContract>>,
    selected: BTreeMap<String, StoredSelection>,
}
impl<'a> DraftConstructionContext<'a> {
    pub fn bind(
        schema: &'a Schema,
        sources: &DefinitionBundle,
        provider: &'a dyn DraftPhysicalDefaults,
        budget: &mut QueryBudget,
    ) -> StorageResult<Self> {
        let bound = bind_draft_contracts(
            schema,
            sources,
            provider,
            None,
            &mut DraftBindWork::Legacy(budget),
            None,
        )?;
        Ok(Self {
            schema,
            provider: Some(provider),
            contracts: bound.contracts,
            selected: bound.selected,
        })
    }
    /// Explicitly capture canonically checked native physical dispositions in
    /// core-owned immutable maps. No provider is retained or called afterward.
    /// This grants no model, initializer behavior or language validation.
    pub fn bind_frozen(
        metadata: &'a crate::definitions::preparation::PreparedPhysicalMetadata<'_>,
        provider: &dyn DraftPhysicalDefaults,
        observation: &InvocationContext<'_>,
    ) -> StorageResult<Self> {
        Self::bind_frozen_with_capture::<OwnedContractCapture>(metadata, provider, observation)
    }
    /// Capture immutable borrowed native rows for this exact metadata owner.
    /// The provider borrow must outlive the returned context. Only initial and
    /// final checked callbacks run; later operations use the immutable rows.
    /// Per-object construction receipts stay owned and independently charged.
    ///
    /// ```compile_fail
    /// use intersect_model::{storage::{DraftClassContract, DraftConstructionContext, DraftPhysicalDefaults}, definitions::{preparation::PreparedPhysicalMetadata, conversion::InvocationContext}};
    /// fn escape<'a>(metadata: &'a PreparedPhysicalMetadata<'a>, observation: &InvocationContext<'_>) -> Result<DraftConstructionContext<'a>, String> {
    ///     struct Local { rows: Vec<DraftClassContract> }
    ///     impl DraftPhysicalDefaults for Local {
    ///         fn contracts(&self) -> &[DraftClassContract] { &self.rows }
    ///     }
    ///     let local = Local { rows: Vec::new() };
    ///     DraftConstructionContext::bind_frozen_borrowed(metadata, &local, observation)
    /// }
    /// ```
    pub fn bind_frozen_borrowed(
        metadata: &'a crate::definitions::preparation::PreparedPhysicalMetadata<'_>,
        provider: &'a dyn DraftPhysicalDefaults,
        observation: &InvocationContext<'_>,
    ) -> StorageResult<Self> {
        Self::bind_frozen_with_capture::<BorrowedContractCapture>(metadata, provider, observation)
    }
    fn bind_frozen_with_capture<'provider, C: ContractCapture<'provider, 'a>>(
        metadata: &'a crate::definitions::preparation::PreparedPhysicalMetadata<'_>,
        provider: &'provider dyn DraftPhysicalDefaults,
        observation: &InvocationContext<'_>,
    ) -> StorageResult<Self> {
        if !observation.has_observer() {
            return Err("Frozen construction binding requires an invocation observer".into());
        }
        let definition = metadata.definition();
        let schema = definition.schema();
        let result = bind_draft_contracts_with_capture::<C>(
            schema,
            definition.sources(),
            provider,
            metadata.compiled_sources(),
            &mut DraftBindWork::Observed(observation),
            metadata.compiled_sources().map(|_| metadata),
        );
        // All native getters, comparisons and copies precede this final
        // cooperative observer check. Only immutable/pure checks follow it.
        let final_check = observation.checkpoint();
        let bound = match result {
            Err(error) if is_budget_failure(&error) => return Err(error),
            other => {
                final_check?;
                other?
            }
        };
        if !metadata.matches_schema(schema) {
            return Err("Frozen construction metadata association changed".into());
        }
        Ok(Self {
            schema,
            provider: None,
            contracts: bound.contracts,
            selected: bound.selected,
        })
    }
    pub(in crate::core::storage) fn is_frozen(&self) -> bool {
        self.provider.is_none()
    }
    fn contract(&self, class: &str) -> StorageResult<&DraftClassContract> {
        if let Some(provider) = self.provider
            && provider
                .contracts()
                .iter()
                .ne(self.contracts.values().map(Cow::as_ref))
        {
            return Err("Draft physical capability changed after binding".into());
        }
        self.contracts
            .get(class)
            .map(Cow::as_ref)
            .ok_or_else(|| format!("Unsupported draft physical creation: {class}"))
    }
    pub(super) fn check_construction_schema(&self, schema: &Schema) -> StorageResult<()> {
        if !std::ptr::eq(self.schema, schema) {
            return Err("Draft physical construction schema association changed".into());
        }
        Ok(())
    }
    fn check_live_physical_contracts(
        &self,
        charge: &mut impl FnMut() -> StorageResult<()>,
    ) -> StorageResult<()> {
        // Charge the live callback and each compared source contract/member
        // before comparing it. Individual identity/string atoms remain atomic.
        charge()?;
        match self.provider {
            Some(provider) => self.compare_live_physical_contracts(provider.contracts(), charge),
            None => Ok(()),
        }
    }
    fn compare_live_physical_contracts(
        &self,
        live: &[DraftClassContract],
        charge: &mut impl FnMut() -> StorageResult<()>,
    ) -> StorageResult<()> {
        compare_draft_contracts(&self.contracts, live, charge)
    }
    // The constructor and pending audit use one provider/member algorithm on
    // their original caller's charge adapter, never a nested/reset grant.
    pub(super) fn permits_absent_physical_slot(
        &self,
        element: ObjectView<'_>,
        name: &str,
        identity: &MemberIdentity,
        charge: &mut impl FnMut() -> StorageResult<()>,
    ) -> StorageResult<bool> {
        self.check_live_physical_contracts(charge)?;
        self.checked_absent_physical_slot(element, name, identity, charge)
    }
    fn checked_absent_physical_slot(
        &self,
        element: ObjectView<'_>,
        name: &str,
        identity: &MemberIdentity,
        charge: &mut impl FnMut() -> StorageResult<()>,
    ) -> StorageResult<bool> {
        charge()?;
        let contract = self
            .contracts
            .get(element.class())
            .ok_or("Unsupported draft physical construction receiver")?;
        charge()?;
        if element.type_identity() != contract.class_identity {
            return Err("Draft physical construction actual receiver identity mismatch".into());
        }
        for member in &contract.members {
            charge()?;
            if member.name == name {
                charge()?;
                if &member.identity != identity {
                    return Err("Draft physical construction member identity mismatch".into());
                }
                if member.disposition != DraftDisposition::DelegatedAbsent {
                    return Err("Missing stored draft physical construction slot".into());
                }
                return Ok(true);
            }
        }
        Err("Missing checked draft physical member disposition".into())
    }
    #[cfg(test)]
    pub(super) fn permits_absent_construction_slot(
        &self,
        element: ObjectView<'_>,
        name: &str,
        identity: &MemberIdentity,
        budget: &mut QueryBudget,
    ) -> StorageResult<bool> {
        self.permits_absent_physical_slot(element, name, identity, &mut || budget.charge())
    }
    pub(super) fn check_pending_access(
        &self,
        store: &impl DraftConstructionStore,
        schema: &Schema,
        charge: &mut impl FnMut() -> StorageResult<()>,
    ) -> StorageResult<()> {
        charge()?;
        self.check_construction_schema(schema)?;
        self.check_live_physical_contracts(charge)?;
        charge()?;
        if !store.is_transaction_active() {
            return Err("Storage write requires a transaction".into());
        }
        Ok(())
    }
    pub fn create(
        &self,
        store: &mut impl ModelStore,
        request: Construction<'_>,
        class: &str,
        start: usize,
        end: usize,
        budget: &mut QueryBudget,
    ) -> StorageResult<DraftConstructionReceipt> {
        self.create_reference_mode(
            store,
            request,
            class,
            start,
            end,
            budget,
            DraftReferencePolicy::Resolved,
        )
    }

    /// Construction inside a caller-owned native store transaction only.
    /// This is still a provisional physical draft, never final graph authority.
    #[allow(clippy::too_many_arguments)]
    pub fn create_pending(
        &self,
        store: &mut impl DraftConstructionStore,
        request: Construction<'_>,
        class: &str,
        start: usize,
        end: usize,
        metadata: &crate::definitions::preparation::PreparedPhysicalMetadata<'_>,
        budget: &mut QueryBudget,
    ) -> StorageResult<DraftConstructionReceipt> {
        self.create_pending_with_sources(
            store,
            request,
            class,
            start,
            end,
            metadata,
            budget,
            &NoConstructionSources,
        )
    }
    /// Pure source bookkeeping; no provider is invoked until the original
    /// constructor's first live-contract boundary. No new allowance is created.
    pub fn source_lease<'scope, 'metadata>(
        &'scope self,
        metadata: &'scope crate::definitions::preparation::PreparedPhysicalMetadata<'metadata>,
        sources: &'scope PendingSourceInputs<'metadata>,
        budget: &mut QueryBudget,
    ) -> StorageResult<DraftConstructionSource<'scope, 'metadata>> {
        DraftConstructionSource::new(self, metadata, sources, budget)
    }
    #[allow(clippy::too_many_arguments)]
    fn create_pending_with_sources<L: ConstructionSources>(
        &self,
        store: &mut impl DraftConstructionStore,
        request: Construction<'_>,
        class: &str,
        start: usize,
        end: usize,
        metadata: &crate::definitions::preparation::PreparedPhysicalMetadata<'_>,
        budget: &mut QueryBudget,
        sources: &L,
    ) -> StorageResult<DraftConstructionReceipt> {
        self.create_pending_with_sources_and_initializer(
            store, request, class, start, end, metadata, budget, sources, None,
        )
    }
    #[allow(clippy::too_many_arguments)]
    fn create_pending_with_sources_and_initializer<L: ConstructionSources>(
        &self,
        store: &mut impl DraftConstructionStore,
        request: Construction<'_>,
        class: &str,
        start: usize,
        end: usize,
        metadata: &crate::definitions::preparation::PreparedPhysicalMetadata<'_>,
        budget: &mut QueryBudget,
        sources: &L,
        initializer: Option<&super::initialization::InitializationBinding<'_>>,
    ) -> StorageResult<DraftConstructionReceipt> {
        budget.charge()?;
        if !store.is_transaction_active() {
            return Err("Storage write requires a transaction".into());
        }
        atomic(store, |store| {
            budget.charge()?;
            let schema = metadata.definition().schema();
            sources.pending_access(self, store, schema, &mut || budget.charge())?;
            budget.charge()?;
            if store.revision()? != schema.revision {
                return Err(
                    "Pending construction store revision differs from live metadata".into(),
                );
            }
            let receipt = self.create_reference_mode_with_sources_and_initializer(
                store,
                request,
                class,
                start,
                end,
                budget,
                DraftReferencePolicy::PendingNonContainment,
                sources,
                initializer,
            )?;
            sources.pending_access(self, store, schema, &mut || budget.charge())?;
            Ok(receipt)
        })
    }

    #[allow(clippy::too_many_arguments)] // Same factory with an explicit private reference policy.
    fn create_reference_mode(
        &self,
        store: &mut impl ModelStore,
        request: Construction<'_>,
        class: &str,
        start: usize,
        end: usize,
        budget: &mut QueryBudget,
        references: DraftReferencePolicy,
    ) -> StorageResult<DraftConstructionReceipt> {
        self.create_reference_mode_with_sources(
            store,
            request,
            class,
            start,
            end,
            budget,
            references,
            &NoConstructionSources,
        )
    }
    #[allow(clippy::too_many_arguments)]
    fn create_reference_mode_with_sources<L: ConstructionSources>(
        &self,
        store: &mut impl ModelStore,
        request: Construction<'_>,
        class: &str,
        start: usize,
        end: usize,
        budget: &mut QueryBudget,
        references: DraftReferencePolicy,
        sources: &L,
    ) -> StorageResult<DraftConstructionReceipt> {
        self.create_reference_mode_with_sources_and_initializer(
            store, request, class, start, end, budget, references, sources, None,
        )
    }
    #[allow(clippy::too_many_arguments)]
    fn create_reference_mode_with_sources_and_initializer<L: ConstructionSources>(
        &self,
        store: &mut impl ModelStore,
        request: Construction<'_>,
        class: &str,
        start: usize,
        end: usize,
        budget: &mut QueryBudget,
        references: DraftReferencePolicy,
        sources: &L,
        initializer: Option<&super::initialization::InitializationBinding<'_>>,
    ) -> StorageResult<DraftConstructionReceipt> {
        budget.charge()?;
        let contract = self.contract(class)?;
        if !request.authored.is_empty() {
            return Err(
                "Draft physical construction requires shared parser authored writes".into(),
            );
        }
        let selected = self
            .selected
            .get(class)
            .ok_or("Missing checked draft physical selection")?;
        if !store.is_transaction_active() {
            return Err("Storage write requires a transaction".into());
        }
        let mut initialization =
            self.prepare_constructor_initializer(contract, &request, class, initializer, budget)?;
        let element = self.physical_element(&request, class, start, end, selected, budget)?;
        atomic(store, |store| {
            bind_schema(store, self.schema)?;
            super::initialization::insert_created_element(store, self.schema, element)?;
            if let Some(prepared) = initialization.as_mut() {
                let conversions = initializer.and_then(|binding| binding.conversions);
                prepared.apply_to_new(store, &request, conversions, budget)?;
            }
            // Same existing structural engine; lower multiplicities remain relaxed.
            let mut validation = StructuralPreparation::draft(references);
            if selected.absent_many {
                if let Some(binding) = initializer {
                    while !sources.advance_with_conversions(
                        &mut validation,
                        store,
                        self.schema,
                        binding.conversions,
                        self,
                        budget,
                    )? {}
                } else {
                    while !sources.advance(&mut validation, store, self.schema, self, budget)? {}
                }
            } else {
                // Literal old service route for Minimal/Feature/C1 and every
                // contract without physically absent multiple delegate slots.
                let conversions = initializer.and_then(|binding| binding.conversions);
                while !validation.advance(store, self.schema, conversions, None)? {}
            }
            validation.finish()?;
            self.contract(class)?;
            if let Some(prepared) = initialization.as_ref() {
                prepared.check_after_execution()?;
            }
            if L::ENABLED {
                sources.finish(self, budget)?;
            }
            Ok(DraftConstructionReceipt {
                object: request.id.into(),
                contract: contract.clone(),
            })
        })
    }
    // Shared original initializer binding checks for ordinary/full-audit and
    // explicit provisional construction; neither route gains new authority.
    fn prepare_constructor_initializer<'initialization>(
        &'initialization self,
        contract: &DraftClassContract,
        request: &Construction<'_>,
        class: &str,
        initializer: Option<
            &'initialization super::initialization::InitializationBinding<'initialization>,
        >,
        budget: &mut QueryBudget,
    ) -> StorageResult<Option<super::initialization::PreparedInitialization<'initialization>>> {
        let initialization_context =
            initializer.map(|binding| super::initialization::InitializationContext {
                schema: self.schema,
                expected: binding.expected,
                sources: binding.sources,
                provider: binding.provider,
                conversions: binding.conversions,
            });
        let initialization = match initialization_context.as_ref() {
            None => None,
            Some(context) => {
                for _ in 0..1 + context.expected.class.len().max(class.len()).div_ceil(64) {
                    budget.charge()?;
                }
                if context.expected.class != class {
                    return Err("Draft initializer class differs from new constructor class".into());
                }
                // Initializer provenance must be in the SAME physical catalog
                // source closure; its existing worker then validates actual text.
                for source in &context.expected.sources {
                    let mut present = false;
                    for actual in &contract.sources {
                        for _ in 0..2
                            + actual.uri.len().max(source.uri.len()).div_ceil(64)
                            + actual.sha256.len().max(source.sha256.len()).div_ceil(64)
                        {
                            budget.charge()?;
                        }
                        present |= actual == source;
                    }
                    if !present {
                        return Err(
                            "Draft initializer source is outside constructor sources".into()
                        );
                    }
                }
                Some(context.prepare_new(request.authored, budget)?)
            }
        };
        Ok(initialization)
    }
    // Shared physical/default worker; a partial batch uses the same factory.
    #[allow(clippy::too_many_arguments)]
    fn physical_element(
        &self,
        request: &Construction<'_>,
        class: &str,
        start: usize,
        end: usize,
        selected: &StoredSelection,
        budget: &mut QueryBudget,
    ) -> StorageResult<Element> {
        let mut element = Element::new_with_stored_selection(
            self.schema,
            request.id,
            class,
            request.resource,
            start,
            end,
            Some(&selected.names),
        )?;
        for name in &selected.null_defaults {
            budget.charge()?;
            // Null is constructor absence, never a present enum/datatype payload.
            // The same pending worker relaxes lower bounds; the explicit new
            // required-null disposition adds a completion obligation.
            *element
                .values
                .get_mut(name)
                .ok_or("Missing checked draft constructor null slot")? = serde_json::Value::Null;
        }
        Ok(element)
    }
    /// Observe a bound disposition on an actual raw receiver. This metadata is
    /// not a proof of semantic construction and grants no mutation authority.
    pub fn observe(
        &self,
        store: &impl ModelRead,
        object: &str,
        budget: &mut QueryBudget,
    ) -> StorageResult<DraftConstructionReceipt> {
        budget.charge()?;
        if store.revision()? != self.schema.revision {
            return Err("Draft observation store revision mismatch".into());
        }
        budget.charge()?;
        let mut receipt = None;
        store.inspect(object, &mut |view| {
            let contract = self.contract(view.class())?;
            if view.type_identity() != contract.class_identity {
                return Err("Draft observation actual receiver identity mismatch".into());
            }
            for member in &contract.members {
                budget.charge()?;
                if view.value(&member.name).is_some()
                    != (member.disposition != DraftDisposition::DelegatedAbsent)
                {
                    return Err("Draft observation physical member disposition mismatch".into());
                }
            }
            receipt = Some(DraftConstructionReceipt {
                object: object.into(),
                contract: contract.clone(),
            });
            Ok(())
        })?;
        receipt.ok_or_else(|| "Missing draft observation receiver".into())
    }
}

// Handwritten shared binder product. No provider callbacks/observer escape;
// explicit borrowed capture retains immutable row references, never the provider.
struct BoundDraftContracts<'captured> {
    contracts: BTreeMap<String, Cow<'captured, DraftClassContract>>,
    selected: BTreeMap<String, StoredSelection>,
}
trait ContractCapture<'provider, 'captured> {
    fn begin(work: &mut DraftBindWork<'_, '_>) -> StorageResult<()>;
    fn retain(
        contract: &'provider DraftClassContract,
        work: &mut DraftBindWork<'_, '_>,
    ) -> StorageResult<()>;
    fn capture(contract: &'provider DraftClassContract) -> Cow<'captured, DraftClassContract>;
}
struct OwnedContractCapture;
impl<'provider, 'captured> ContractCapture<'provider, 'captured> for OwnedContractCapture {
    fn begin(_: &mut DraftBindWork<'_, '_>) -> StorageResult<()> {
        Ok(())
    }
    fn retain(
        contract: &'provider DraftClassContract,
        work: &mut DraftBindWork<'_, '_>,
    ) -> StorageResult<()> {
        work.retain(contract)
    }
    fn capture(contract: &'provider DraftClassContract) -> Cow<'captured, DraftClassContract> {
        Cow::Owned(contract.clone())
    }
}
// Fixed logical handle fees are identical on native and WASM. These actual
// pointer representations must fit their admitted logical slice/row bounds.
const _: () = {
    assert!(std::mem::size_of::<&[DraftClassContract]>() <= 16);
    assert!(std::mem::size_of::<&DraftClassContract>() <= 8);
};
struct BorrowedContractCapture;
impl<'provider> ContractCapture<'provider, 'provider> for BorrowedContractCapture {
    fn begin(work: &mut DraftBindWork<'_, '_>) -> StorageResult<()> {
        if !matches!(work, DraftBindWork::Observed(_)) {
            return Err("Borrowed frozen contracts require observed capture".into());
        }
        work.charge()?;
        work.storage(16)
    }
    fn retain(
        _: &'provider DraftClassContract,
        work: &mut DraftBindWork<'_, '_>,
    ) -> StorageResult<()> {
        work.charge()?;
        work.storage(8)
    }
    fn capture(contract: &'provider DraftClassContract) -> Cow<'provider, DraftClassContract> {
        Cow::Borrowed(contract)
    }
}
// Existing256byte per-row map admission covers Cow discriminant/key/header;
// owned payload cloning retains its separate original serialization admission.
const _: () = assert!(
    std::mem::size_of::<Cow<'_, DraftClassContract>>()
        + std::mem::size_of::<String>()
        + 6 * std::mem::size_of::<usize>()
        <= 256
);
fn bind_draft_contracts(
    schema: &Schema,
    sources: &DefinitionBundle,
    provider: &dyn DraftPhysicalDefaults,
    completed: Option<&crate::definitions::preparation::CompletedCompiledSources<'_>>,
    work: &mut DraftBindWork<'_, '_>,
    metadata: Option<&crate::definitions::preparation::PreparedPhysicalMetadata<'_>>,
) -> StorageResult<BoundDraftContracts<'static>> {
    bind_draft_contracts_with_capture::<OwnedContractCapture>(
        schema, sources, provider, completed, work, metadata,
    )
}
fn bind_draft_contracts_with_capture<
    'provider,
    'captured,
    C: ContractCapture<'provider, 'captured>,
>(
    schema: &Schema,
    sources: &DefinitionBundle,
    provider: &'provider dyn DraftPhysicalDefaults,
    completed: Option<&crate::definitions::preparation::CompletedCompiledSources<'_>>,
    work: &mut DraftBindWork<'_, '_>,
    metadata: Option<&crate::definitions::preparation::PreparedPhysicalMetadata<'_>>,
) -> StorageResult<BoundDraftContracts<'captured>> {
    work.charge()
        .map_err(|e| budget_failure_context(e, "", "", "entry", "Pending"))?;
    work.validate_sources(schema, sources, completed)
        .map_err(|e| budget_failure_context(e, "", "", "source_validation", "Pending"))?;
    let source_ids = work
        .source_ids(sources)
        .map_err(|e| budget_failure_context(e, "", "", "source_ids", "Pending"))?;
    let mut contracts = BTreeMap::new();
    let mut selected = BTreeMap::new();
    // Pure source payloads shared only within this one immutable-schema bind.
    let mut input_cache = None;
    let mut any_absent_many = false;
    let mut any_required_nulls = false;
    C::begin(work).map_err(|e| budget_failure_context(e, "", "", "capture_header", "Pending"))?;
    for contract in work
        .provider_contracts(provider)
        .map_err(|e| budget_failure_context(e, "", "", "provider_rows", "Pending"))?
    {
        work.charge().map_err(|e| {
            budget_failure_context(e, &contract.class, "", "contract_entry", "Pending")
        })?;
        C::retain(contract, work).map_err(|e| {
            budget_failure_context(e, &contract.class, "", "contract_capture", "Pending")
        })?;
        work.identity(schema, &contract.class).map_err(|e| {
            budget_failure_context(e, &contract.class, "", "class_identity", "Pending")
        })?;
        if contract.version != 1
            || contract.schema_revision != schema.revision
            || contract.class_identity != schema.identity(&contract.class)
            || contract.sources != source_ids
            || contract.sources.is_empty()
            || schema
                .classes
                .get(&contract.class)
                .is_none_or(|class| class.abstract_type)
        {
            return Err("Invalid or stale draft physical construction contract".into());
        }
        let fields = work
            .fields(schema, &contract.class, &mut input_cache, metadata)
            .map_err(|e| {
                budget_failure_context(e, &contract.class, "", "effective_inputs", "Pending")
            })?;
        let mut seen = BTreeSet::new();
        let mut stored = StoredSelection {
            names: BTreeSet::new(),
            null_defaults: BTreeSet::new(),
            absent_many: false,
            required_nulls: false,
        };
        // This contract's effective features, computed at most once.
        let mut effective = None;
        for member in &contract.members {
            work.charge().map_err(|e| {
                budget_failure_context(
                    e,
                    &contract.class,
                    &member.name,
                    "member_entry",
                    fields.input_kind(),
                )
            })?;
            let (field, identity_matches) = fields
                .member_among(schema, &contract.class, member, work, &mut effective)
                .map_err(|e| {
                    budget_failure_context(
                        e,
                        &contract.class,
                        &member.name,
                        "member_selection",
                        fields.input_kind(),
                    )
                })?;
            work.key(&member.name).map_err(|e| {
                budget_failure_context(
                    e,
                    &contract.class,
                    &member.name,
                    "member_name",
                    fields.input_kind(),
                )
            })?;
            if field.derived || !identity_matches || !seen.insert(member.name.clone()) {
                return Err("Invalid or duplicate draft physical member disposition".into());
            }
            match member.disposition {
                DraftDisposition::StoredDefault => {
                    work.key(&member.name).map_err(|e| {
                        budget_failure_context(
                            e,
                            &contract.class,
                            &member.name,
                            "stored_selection_key",
                            fields.input_kind(),
                        )
                    })?;
                    stored.names.insert(member.name.clone());
                }
                DraftDisposition::StoredNullDefault => {
                    if field.reference
                        || field.lower != 0
                        || field.upper != 1
                        || field.default.is_null()
                    {
                        return Err("Draft constructor null default requires a nonnull-default optional single attribute".into());
                    }
                    work.key(&member.name).map_err(|e| {
                        budget_failure_context(
                            e,
                            &contract.class,
                            &member.name,
                            "stored_selection_key",
                            fields.input_kind(),
                        )
                    })?;
                    stored.names.insert(member.name.clone());
                    work.key(&member.name).map_err(|e| {
                        budget_failure_context(
                            e,
                            &contract.class,
                            &member.name,
                            "stored_selection_key",
                            fields.input_kind(),
                        )
                    })?;
                    stored.null_defaults.insert(member.name.clone());
                }
                DraftDisposition::StoredRequiredNullDefault => {
                    if field.reference
                        || field.lower != 1
                        || field.upper != 1
                        || field.default.is_null()
                    {
                        return Err("Required constructor null default requires a nonnull-default required single attribute".into());
                    }
                    work.key(&member.name).map_err(|e| {
                        budget_failure_context(
                            e,
                            &contract.class,
                            &member.name,
                            "stored_selection_key",
                            fields.input_kind(),
                        )
                    })?;
                    stored.names.insert(member.name.clone());
                    work.key(&member.name).map_err(|e| {
                        budget_failure_context(
                            e,
                            &contract.class,
                            &member.name,
                            "stored_selection_key",
                            fields.input_kind(),
                        )
                    })?;
                    stored.null_defaults.insert(member.name.clone());
                    stored.required_nulls = true;
                }
                DraftDisposition::DelegatedAbsent => {
                    // The member/source/shape selection was already charged
                    // at this bind iteration. No new old-route detection scan.
                    stored.absent_many |= field.upper != 1;
                }
            }
        }
        if seen
            != fields.coverage(work).map_err(|e| {
                budget_failure_context(e, &contract.class, "", "coverage", fields.input_kind())
            })?
        {
            return Err(
                "Draft physical dispositions must cover every nonderived effective member".into(),
            );
        }
        work.key(&contract.class).map_err(|e| {
            budget_failure_context(
                e,
                &contract.class,
                "",
                "capture_map_key",
                fields.input_kind(),
            )
        })?;
        work.key(&contract.class).map_err(|e| {
            budget_failure_context(
                e,
                &contract.class,
                "",
                "capture_map_key",
                fields.input_kind(),
            )
        })?;
        work.storage(256).map_err(|e| {
            budget_failure_context(e, &contract.class, "", "capture_maps", fields.input_kind())
        })?;
        if contracts
            .insert(contract.class.clone(), C::capture(contract))
            .is_some()
        {
            return Err("Duplicate draft physical class disposition".into());
        }
        any_absent_many |= stored.absent_many;
        any_required_nulls |= stored.required_nulls;
        selected.insert(contract.class.clone(), stored);
    }
    // Any existing object from this native context may contain absent
    // multiple delegates. Propagate the route to later ordinary creations
    // in the same graph. Old contexts execute no extra pass or charge.
    if any_absent_many {
        for stored in selected.values_mut() {
            work.charge().map_err(|e| {
                budget_failure_context(e, "", "", "propagate_selection", "Captured")
            })?;
            stored.absent_many = true;
        }
    }
    if any_required_nulls {
        for stored in selected.values_mut() {
            work.charge().map_err(|e| {
                budget_failure_context(e, "", "", "propagate_selection", "Captured")
            })?;
            stored.required_nulls = true;
        }
    }
    if contracts.is_empty() {
        return Err("Draft construction requires a native physical capability".into());
    }
    work.final_contracts(provider, &contracts)
        .map_err(|e| budget_failure_context(e, "", "", "final_contracts", "Captured"))?;
    Ok(BoundDraftContracts {
        contracts,
        selected,
    })
}
fn compare_draft_contracts(
    contracts: &BTreeMap<String, Cow<'_, DraftClassContract>>,
    live: &[DraftClassContract],
    charge: &mut impl FnMut() -> StorageResult<()>,
) -> StorageResult<()> {
    if live.len() != contracts.len() {
        return Err("Draft physical capability changed after binding".into());
    }
    for (actual, checked) in live.iter().zip(contracts.values()) {
        charge().map_err(|e| {
            budget_failure_context(e, &checked.class, "", "final_contract_row", "Captured")
        })?;
        if actual.version != checked.version
            || actual.schema_revision != checked.schema_revision
            || actual.class != checked.class
            || actual.class_identity != checked.class_identity
            || actual.sources != checked.sources
            || actual.members.len() != checked.members.len()
        {
            return Err("Draft physical capability changed after binding".into());
        }
        let receiver = checked.class.as_str();
        for (actual, checked) in actual.members.iter().zip(&checked.members) {
            charge().map_err(|e| {
                budget_failure_context(
                    e,
                    receiver,
                    &checked.name,
                    "final_contract_member",
                    "Captured",
                )
            })?;
            if actual != checked {
                return Err("Draft physical capability changed after binding".into());
            }
        }
    }
    Ok(())
}
#[path = "draft_construction_binding.rs"]
mod binding;
use binding::{DraftBindWork, budget_failure_context, is_budget_failure};

#[cfg(test)]
#[path = "pending_required_constructor_attribute_tests.rs"]
mod pending_required_attribute_tests;

#[cfg(test)]
#[path = "frozen_compiled_bundle_validation_tests.rs"]
mod compiled_bundle_validation_tests;

#[cfg(test)]
#[path = "draft_construction_borrowed_tests.rs"]
mod borrowed_controls;
