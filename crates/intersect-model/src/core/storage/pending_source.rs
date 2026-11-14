//! Invocation-local source identity for frozen construction and setters.
use super::*;
use crate::{core::query::QueryBudget, definitions::preparation::PreparedPhysicalMetadata};
use std::cell::Cell;

/// Opaque source-only service. The private host owns one cold instance and its
/// original request ledger; budget parameters do not encode ledger identity.
/// No model/read/selection/index/provider or serialized authority is exposed.
/// ```compile_fail
/// use intersect_model::storage::PendingSourceInputs;
/// fn detached(value: &PendingSourceInputs<'_>) { let _ = &value.completed; }
/// ```
/// ```compile_fail
/// use intersect_model::storage::{PendingSourceInputs, ModelRead};
/// fn raw<'a>(value: &'a PendingSourceInputs<'a>) -> &'a dyn ModelRead { value }
/// ```
pub struct PendingSourceInputs<'metadata> {
    metadata: &'metadata PreparedPhysicalMetadata<'metadata>,
    admitted: Cell<usize>,
    poisoned: Cell<bool>,
}
impl<'metadata> PendingSourceInputs<'metadata> {
    pub fn new(
        metadata: &'metadata PreparedPhysicalMetadata<'metadata>,
        budget: &mut QueryBudget,
    ) -> StorageResult<Self> {
        budget.charge()?;
        // Fixed bookkeeping is admitted before initializing the source context.
        // QueryBudget owns work only; this counter is logical admission, not RSS.
        let admitted = Cell::new(160);
        Ok(Self {
            metadata,
            admitted,
            poisoned: Cell::new(false),
        })
    }
    /// Logical retention charged to this source-only context.
    pub fn logical_bytes(&self) -> usize {
        self.admitted.get()
    }
    /// Explicit extra source-owner bookkeeping for a new opt-in invocation.
    /// This admits no graph, source row, provider or serialized authority.
    pub fn admit_invocation_context(&self, budget: &mut QueryBudget) -> StorageResult<()> {
        budget.charge()?;
        self.account(64)
    }
    /// Failure is monotonic and grants no authority. A fresh request needs its
    /// own new cold source scope; callers cannot revive this one.
    pub fn invalidate(&self) {
        self.poison();
    }
    /// A paid invocation reborrow/final-read liveness check. This is only source
    /// association/failure state; it certifies no model, provider or graph fact.
    pub fn check_invocation_context(
        &self,
        metadata: &PreparedPhysicalMetadata<'_>,
        budget: &mut QueryBudget,
    ) -> StorageResult<()> {
        self.check_constructor_context(metadata, &mut || budget.charge())
    }
    pub(super) fn check_constructor_context(
        &self,
        metadata: &PreparedPhysicalMetadata<'_>,
        charge: &mut impl FnMut() -> StorageResult<()>,
    ) -> StorageResult<()> {
        let result = (|| {
            charge()?;
            self.check_after_observation(metadata)
        })();
        if result.is_err() {
            self.poison();
        }
        result
    }
    // Callback-free final state check, after explicitly paid observer work.
    pub(in crate::core::storage) fn check_after_observation(
        &self,
        metadata: &PreparedPhysicalMetadata<'_>,
    ) -> StorageResult<()> {
        self.live_status()?;
        self.check_association(metadata)
    }
    pub(super) fn admit_constructor_context(
        &self,
        bytes: usize,
        budget: &mut QueryBudget,
    ) -> StorageResult<()> {
        let result = (|| {
            budget.charge()?;
            self.live_status()?;
            self.account(bytes)
        })();
        if result.is_err() {
            self.poison();
        }
        result
    }
    // Same constructor charge adapter; source-only index admission never
    // opens or resets a QueryBudget and never authorizes a graph observation.
    pub(super) fn admit_constructor_member_index(
        &self,
        bytes: usize,
        charge: &mut impl FnMut() -> StorageResult<()>,
    ) -> StorageResult<()> {
        let result = (|| {
            charge()?;
            self.live_status()?;
            self.account(bytes)
        })();
        if result.is_err() {
            self.poison();
        }
        result
    }
    pub(super) fn poison(&self) {
        self.poisoned.set(true);
    }
    fn live_status(&self) -> StorageResult<()> {
        if self.poisoned.get() {
            return Err("Pending source inputs have failed".into());
        }
        Ok(())
    }
    fn check_association(&self, metadata: &PreparedPhysicalMetadata<'_>) -> StorageResult<()> {
        if !std::ptr::eq(self.metadata, metadata)
            || !std::ptr::eq(self.metadata.definition(), metadata.definition())
            || !metadata.matches_schema(self.metadata.definition().schema())
        {
            self.poison();
            return Err("Pending source metadata association changed".into());
        }
        Ok(())
    }
    fn account(&self, bytes: usize) -> StorageResult<()> {
        self.admitted.set(
            self.admitted
                .get()
                .checked_add(bytes)
                .ok_or("Pending source admission overflow")?,
        );
        Ok(())
    }
}

#[cfg(test)]
#[path = "pending_source_tests.rs"]
pub(super) mod tests;
