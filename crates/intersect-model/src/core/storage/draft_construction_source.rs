//! Invocation-borrowed immutable native contract metadata, never graph authority.
use super::*;
use std::cell::{Cell, RefCell};

/// A lazy paid contract-source lease. Dynamic contexts re-read the native
/// provider and compare distinct slices with the original ordered comparator.
/// Explicit frozen contexts use checked Core-owned maps without callbacks.
/// Neither route grants graph or semantic authority.
/// ```compile_fail
/// use intersect_model::storage::{DraftConstructionSource, ModelRead};
/// fn raw<'a>(value: &'a DraftConstructionSource<'a, 'a>) -> &'a dyn ModelRead { value }
/// ```
/// ```compile_fail
/// use intersect_model::storage::DraftConstructionSource;
/// fn forge<'a>() { let _: DraftConstructionSource<'a, 'a> = serde_json::from_str("{}").unwrap(); }
/// ```
pub struct DraftConstructionSource<'context, 'metadata> {
    context: &'context DraftConstructionContext<'context>,
    metadata: &'context crate::definitions::preparation::PreparedPhysicalMetadata<'metadata>,
    sources: &'context PendingSourceInputs<'metadata>,
    checked: Cell<Option<&'context [DraftClassContract]>>,
    busy: Cell<bool>,
    failed: Cell<bool>,
    logical: Cell<usize>,
    // Pure immutable bound-context metadata only. No graph/provider facts.
    members:
        RefCell<BTreeMap<&'context str, BTreeMap<&'context str, &'context DraftMemberDisposition>>>,
    member_busy: Cell<bool>,
}
impl<'context, 'metadata> DraftConstructionSource<'context, 'metadata> {
    pub(super) fn new(
        context: &'context DraftConstructionContext<'context>,
        metadata: &'context crate::definitions::preparation::PreparedPhysicalMetadata<'metadata>,
        sources: &'context PendingSourceInputs<'metadata>,
        budget: &mut QueryBudget,
    ) -> StorageResult<Self> {
        // Same measured-header admission below covers the new empty map; map
        // nodes/keys/Arc transfers are separately admitted at publication.
        const { assert!(std::mem::size_of::<DraftConstructionSource<'_, '_>>() <= 256) };
        let result = (|| {
            budget.charge()?;
            sources.check_constructor_context(metadata, &mut || budget.charge())?;
            context.check_construction_schema(metadata.definition().schema())?;
            // This is fixed borrowed bookkeeping. The language/context already own
            // both contract vectors; no new vector/row/default is cloned or warmed.
            let logical = crate::logical_size!(Self, 88);
            sources.admit_constructor_context(logical, budget)?;
            Ok(Self {
                context,
                metadata,
                sources,
                checked: Cell::new(None),
                busy: Cell::new(false),
                failed: Cell::new(false),
                logical: Cell::new(logical),
                members: RefCell::new(BTreeMap::new()),
                member_busy: Cell::new(false),
            })
        })();
        if result.is_err() {
            sources.invalidate();
        }
        result
    }
    /// Logical sidecar size, not RSS, a graph proof, or a global memory bound.
    pub fn logical_bytes(&self) -> usize {
        self.logical.get()
    }
    pub(in crate::core::storage) fn invalidate(&self) {
        self.failed.set(true);
        self.checked.set(None);
        // Callback-free best effort: the poisoned owner can never reuse rows.
        // No observer/provider is called while this short borrow is held.
        if let Ok(mut members) = self.members.try_borrow_mut() {
            members.clear();
        }
        self.sources.invalidate();
    }
    fn association(
        &self,
        context: &DraftConstructionContext<'_>,
        charge: &mut impl FnMut() -> StorageResult<()>,
    ) -> StorageResult<()> {
        charge()?;
        if self.failed.get() || !std::ptr::eq(self.context, context) {
            return Err("Draft construction source context changed or failed".into());
        }
        self.sources
            .check_constructor_context(self.metadata, charge)?;
        context.check_construction_schema(self.metadata.definition().schema())
    }
    pub(super) fn check_live(
        &self,
        context: &DraftConstructionContext<'_>,
        charge: &mut impl FnMut() -> StorageResult<()>,
    ) -> StorageResult<()> {
        // Arm before any work observer or provider callback. Nested failure is
        // monotonic even when the native callback catches it and returns a slice.
        if self.member_busy.get() {
            self.invalidate();
            return Err("Draft construction source is already in use".into());
        }
        if self.busy.replace(true) {
            self.invalidate();
            return Err("Draft construction source is already in use".into());
        }
        struct Ticket<'a, 'c, 'm> {
            source: &'a DraftConstructionSource<'c, 'm>,
            complete: bool,
        }
        impl Drop for Ticket<'_, '_, '_> {
            fn drop(&mut self) {
                if !self.complete {
                    self.source.invalidate();
                }
                self.source.busy.set(false);
            }
        }
        let mut ticket = Ticket {
            source: self,
            complete: false,
        };
        let result = (|| {
            self.association(context, charge)?;
            if self.context.is_frozen() {
                // Core owns the immutable checked maps. No native callback or
                // provider-slice token participates in this explicit mode.
                charge()?;
                return self.association(context, charge);
            }
            charge()?;
            let provider: &'context dyn DraftPhysicalDefaults = self
                .context
                .provider
                .ok_or("Missing dynamic draft construction provider")?;
            let live = provider.contracts();
            charge()?;
            let same = self
                .checked
                .get()
                .is_some_and(|old| std::ptr::eq(old, live));
            if !same {
                // One canonical algorithm; no hash/token/assumed receiver list.
                context.compare_live_physical_contracts(live, charge)?;
            }
            self.association(context, charge)?;
            // Only a completely checked immutable slice may be retained. No
            // callbacks occur during this Cell update; a failed ticket clears it.
            if !same {
                self.checked.set(Some(live));
            }
            Ok(())
        })();
        if result.is_ok() {
            ticket.complete = true;
        }
        result
    }
    // Borrow actual owner-local parts for the paired final-audit sibling only.
    // Exact borrowed setting service association. No source/graph authority is
    // inferred from matching contents; normal live-provider and transaction
    // checks still execute at each old service boundary.
    pub(in crate::core::storage) fn setting_access(
        &self,
        context: &DraftConstructionContext<'_>,
        store: &impl DraftConstructionStore,
        metadata: &crate::definitions::preparation::PreparedPhysicalMetadata<'_>,
        schema: &Schema,
        charge: &mut impl FnMut() -> StorageResult<()>,
    ) -> StorageResult<()> {
        let result = (|| {
            charge()?;
            if !std::ptr::eq(self.metadata, metadata) {
                return Err("Computed draft setting source metadata association changed".into());
            }
            self.pending_access(context, store, schema, charge)
        })();
        if result.is_err() {
            self.invalidate();
        }
        result
    }
    pub(in crate::core::storage) fn permits_absence(
        &self,
        context: &DraftConstructionContext<'_>,
        element: ObjectView<'_>,
        name: &str,
        identity: &MemberIdentity,
        charge: &mut impl FnMut() -> StorageResult<()>,
    ) -> StorageResult<bool> {
        // The existing check arms its own ticket before every callback. Arm
        // the member ticket only after it completes, before any index work.
        self.check_live(context, charge)?;
        if self.member_busy.replace(true) {
            self.invalidate();
            return Err("Draft construction source is already in use".into());
        }
        struct MemberTicket<'a, 'c, 'm> {
            source: &'a DraftConstructionSource<'c, 'm>,
            complete: bool,
        }
        impl Drop for MemberTicket<'_, '_, '_> {
            fn drop(&mut self) {
                if !self.complete {
                    self.source.invalidate();
                }
                self.source.member_busy.set(false);
            }
        }
        let mut ticket = MemberTicket {
            source: self,
            complete: false,
        };
        let result = (|| {
            // Same actual receiver and identity checks as the old linear route.
            // References below come from this owner, never the caller's context.
            charge()?;
            let contract: &'context DraftClassContract = self
                .context
                .contracts
                .get(element.class())
                .ok_or("Unsupported draft physical construction receiver")?;
            charge()?;
            if element.type_identity() != contract.class_identity {
                return Err("Draft physical construction actual receiver identity mismatch".into());
            }
            let member = self.checked_member(context, contract, name, charge)?;
            charge()?;
            if &member.identity != identity {
                return Err("Draft physical construction member identity mismatch".into());
            }
            if member.disposition != DraftDisposition::DelegatedAbsent {
                return Err("Missing stored draft physical construction slot".into());
            }
            self.association(context, charge)?;
            Ok(true)
        })();
        if result.is_ok() {
            ticket.complete = true;
        }
        result
    }
    fn admit_member_index(
        &self,
        bytes: usize,
        charge: &mut impl FnMut() -> StorageResult<()>,
    ) -> StorageResult<()> {
        // QueryBudget has work only. This is explicit cumulative logical
        // admission, not allocator/RSS accounting or a new memory allowance.
        self.sources.admit_constructor_member_index(bytes, charge)?;
        self.logical.set(
            self.logical
                .get()
                .checked_add(bytes)
                .ok_or("Draft construction source admission overflow")?,
        );
        Ok(())
    }
    fn checked_member(
        &self,
        context: &DraftConstructionContext<'_>,
        contract: &'context DraftClassContract,
        name: &str,
        charge: &mut impl FnMut() -> StorageResult<()>,
    ) -> StorageResult<&'context DraftMemberDisposition> {
        // Both attempted map lookups are paid before the short borrow, including
        // a cold class miss. Borrowed member references outlive this map borrow.
        charge()?;
        charge()?;
        let cached = {
            let members = self
                .members
                .try_borrow()
                .map_err(|_| "Draft construction member index is already borrowed")?;
            members
                .get(contract.class.as_str())
                .map(|row| row.get(name).copied())
        };
        if let Some(member) = cached {
            return member
                .ok_or_else(|| "Missing checked draft physical member disposition".into());
        }
        // Lazy actual class only. Every retained row is borrowed from the
        // immutable context that already passed the full canonical bind check.
        // A logical node policy is explicit; BTreeMap/string atoms remain S09.
        self.admit_member_index(
            crate::logical_size!((&str, BTreeMap<&str, &DraftMemberDisposition>), 40)
                + 3 * crate::logical_size::LOGICAL_WORD,
            charge,
        )?;
        let mut row = BTreeMap::new();
        for member in &contract.members {
            self.admit_member_index(
                crate::logical_size!((&str, &DraftMemberDisposition), 24)
                    + 3 * crate::logical_size::LOGICAL_WORD,
                charge,
            )?;
            charge()?;
            if row.insert(member.name.as_str(), member).is_some() {
                return Err("Duplicate checked draft physical member disposition".into());
            }
        }
        charge()?;
        let selected = row.get(name).copied();
        // Publication is paid before a final live check; no callback follows
        // that check until the immutable row is installed and borrow released.
        charge()?;
        self.association(context, charge)?;
        if self
            .members
            .try_borrow_mut()
            .map_err(|_| "Draft construction member index is already borrowed")?
            .insert(contract.class.as_str(), row)
            .is_some()
        {
            return Err("Duplicate checked draft physical class index".into());
        }
        selected.ok_or_else(|| "Missing checked draft physical member disposition".into())
    }
    /// Same atomic pending factory, native storage and the caller ledger.
    #[allow(clippy::too_many_arguments)]
    pub fn create_pending(
        &self,
        store: &mut impl DraftConstructionStore,
        request: Construction<'_>,
        class: &str,
        start: usize,
        end: usize,
        budget: &mut QueryBudget,
    ) -> StorageResult<DraftConstructionReceipt> {
        let result = self.context.create_pending_with_sources(
            store,
            request,
            class,
            start,
            end,
            self.metadata,
            budget,
            self,
        );
        if result.is_err() {
            self.invalidate();
        }
        result
    }
    /// Apply the canonical native initializer only to the object inserted by
    /// this SAME atomic source-bound pending creation. No existing-object write,
    /// second create, model copy or class/default substitution is exposed.
    #[allow(clippy::too_many_arguments)]
    pub fn create_pending_with_initializer(
        &self,
        store: &mut impl DraftConstructionStore,
        request: Construction<'_>,
        class: &str,
        start: usize,
        end: usize,
        initializer: &super::super::initialization::InitializationBinding<'_>,
        budget: &mut QueryBudget,
    ) -> StorageResult<DraftConstructionReceipt> {
        struct InitializerTicket<'a, 'c, 'm> {
            source: &'a DraftConstructionSource<'c, 'm>,
            complete: bool,
        }
        impl Drop for InitializerTicket<'_, '_, '_> {
            fn drop(&mut self) {
                if !self.complete {
                    self.source.invalidate();
                }
            }
        }
        let mut ticket = InitializerTicket {
            source: self,
            complete: false,
        };
        // New-route-only local guard operation; no heap or lease field added.
        budget.charge()?;
        let result = self.context.create_pending_with_sources_and_initializer(
            store,
            request,
            class,
            start,
            end,
            self.metadata,
            budget,
            self,
            Some(initializer),
        );
        ticket.complete = result.is_ok();
        result
    }
}

pub(super) trait ConstructionSources {
    const ENABLED: bool;
    fn pending_access(
        &self,
        context: &DraftConstructionContext<'_>,
        store: &impl DraftConstructionStore,
        schema: &Schema,
        charge: &mut impl FnMut() -> StorageResult<()>,
    ) -> StorageResult<()>;
    fn advance<S: ModelStore>(
        &self,
        pending: &mut StructuralPreparation,
        store: &S,
        schema: &Schema,
        context: &DraftConstructionContext<'_>,
        budget: &mut QueryBudget,
    ) -> StorageResult<bool>;
    fn advance_with_conversions<S: ModelStore>(
        &self,
        pending: &mut StructuralPreparation,
        store: &S,
        schema: &Schema,
        conversions: Option<&ConversionRegistry<'_>>,
        context: &DraftConstructionContext<'_>,
        budget: &mut QueryBudget,
    ) -> StorageResult<bool>;
    fn finish(&self, _: &DraftConstructionContext<'_>, _: &mut QueryBudget) -> StorageResult<()> {
        Ok(())
    }
}
pub(super) struct NoConstructionSources;
impl ConstructionSources for NoConstructionSources {
    const ENABLED: bool = false;
    fn pending_access(
        &self,
        context: &DraftConstructionContext<'_>,
        store: &impl DraftConstructionStore,
        schema: &Schema,
        charge: &mut impl FnMut() -> StorageResult<()>,
    ) -> StorageResult<()> {
        context.check_pending_access(store, schema, charge)
    }
    fn advance<S: ModelStore>(
        &self,
        pending: &mut StructuralPreparation,
        store: &S,
        schema: &Schema,
        context: &DraftConstructionContext<'_>,
        budget: &mut QueryBudget,
    ) -> StorageResult<bool> {
        pending.advance_construction(store, schema, context, budget)
    }
    fn advance_with_conversions<S: ModelStore>(
        &self,
        pending: &mut StructuralPreparation,
        store: &S,
        schema: &Schema,
        conversions: Option<&ConversionRegistry<'_>>,
        context: &DraftConstructionContext<'_>,
        budget: &mut QueryBudget,
    ) -> StorageResult<bool> {
        pending.advance_construction_with_conversions(store, schema, conversions, context, budget)
    }
}
impl ConstructionSources for DraftConstructionSource<'_, '_> {
    const ENABLED: bool = true;
    fn pending_access(
        &self,
        context: &DraftConstructionContext<'_>,
        store: &impl DraftConstructionStore,
        schema: &Schema,
        charge: &mut impl FnMut() -> StorageResult<()>,
    ) -> StorageResult<()> {
        charge()?;
        context.check_construction_schema(schema)?;
        self.check_live(context, charge)?;
        charge()?;
        if !store.is_transaction_active() {
            return Err("Storage write requires a transaction".into());
        }
        self.association(context, charge)
    }
    fn advance<S: ModelStore>(
        &self,
        pending: &mut StructuralPreparation,
        store: &S,
        schema: &Schema,
        context: &DraftConstructionContext<'_>,
        budget: &mut QueryBudget,
    ) -> StorageResult<bool> {
        pending.advance_construction_with_sources(store, schema, context, self, budget)
    }
    fn advance_with_conversions<S: ModelStore>(
        &self,
        pending: &mut StructuralPreparation,
        store: &S,
        schema: &Schema,
        conversions: Option<&ConversionRegistry<'_>>,
        context: &DraftConstructionContext<'_>,
        budget: &mut QueryBudget,
    ) -> StorageResult<bool> {
        pending.advance_construction_with_sources_and_conversions(
            store,
            schema,
            conversions,
            context,
            self,
            budget,
        )
    }
    fn finish(
        &self,
        context: &DraftConstructionContext<'_>,
        budget: &mut QueryBudget,
    ) -> StorageResult<()> {
        let result = self.association(context, &mut || budget.charge());
        if result.is_err() {
            self.invalidate();
        }
        result
    }
}

#[cfg(test)]
#[path = "draft_construction_source_tests.rs"]
mod tests;
