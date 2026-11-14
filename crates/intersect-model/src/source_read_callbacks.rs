//! Compatibility leaves for source programs over an existing checked reader.
//! This adapter owns no language policy, object map or alternative allowance.
use crate::{
    core::query::QueryBudget,
    definitions::{conversion::InvocationContext, resources::DeclarationId},
    scope_runtime::{ScopePage, ScopeRead, ScopeValue},
    semantic_bindings::BorrowedBindingWork,
};
use std::cell::RefCell;

/// Checked read leaves that still use the native query ledger. The caller lends
/// that same ledger for each callback; implementations retain their source and
/// physical guards and admit output storage through the supplied observation.
/// Callback work must use `budget`; invoking `observation.work` while that
/// ledger is lent is rejected as reentry. Storage/control observations remain
/// available without lending a second allowance.
pub trait SourceReadCallbacks {
    fn check(&self, budget: &mut QueryBudget) -> Result<(), String> {
        budget.charge()
    }
    fn class(
        &self,
        object: &str,
        observation: &InvocationContext<'_>,
        budget: &mut QueryBudget,
    ) -> Result<String, String>;
    fn singular(
        &self,
        object: &str,
        member: &DeclarationId,
        observation: &InvocationContext<'_>,
        budget: &mut QueryBudget,
    ) -> Result<ScopeValue, String>;
    fn page(
        &self,
        object: &str,
        member: &DeclarationId,
        cursor: usize,
        observation: &InvocationContext<'_>,
        budget: &mut QueryBudget,
    ) -> Result<ScopePage, String>;
}
pub(crate) struct CallbackReads<'a, 'caller> {
    pub reads: &'a dyn SourceReadCallbacks,
    pub work: &'a BorrowedBindingWork<'caller>,
    pub failure: RefCell<Option<String>>,
}
impl CallbackReads<'_, '_> {
    fn call<T>(
        &self,
        callback: impl FnOnce(&mut QueryBudget) -> Result<T, String>,
    ) -> Result<T, String> {
        if self.failure.borrow().is_some() {
            return Err("Source read callback has failed".into());
        }
        match self.work.read_callback(callback) {
            Ok(value) => Ok(value),
            Err(error) => {
                *self.failure.borrow_mut() = Some(error);
                Err("Source read callback failed".into())
            }
        }
    }
}
impl ScopeRead for CallbackReads<'_, '_> {
    fn check(&self, _: &InvocationContext<'_>) -> Result<(), String> {
        self.call(|budget| self.reads.check(budget))
    }
    fn class(&self, object: &str, observation: &InvocationContext<'_>) -> Result<String, String> {
        self.call(|budget| self.reads.class(object, observation, budget))
    }
    fn singular(
        &self,
        object: &str,
        member: &DeclarationId,
        observation: &InvocationContext<'_>,
    ) -> Result<ScopeValue, String> {
        self.call(|budget| self.reads.singular(object, member, observation, budget))
    }
    fn page(
        &self,
        object: &str,
        member: &DeclarationId,
        cursor: usize,
        observation: &InvocationContext<'_>,
    ) -> Result<ScopePage, String> {
        self.call(|budget| self.reads.page(object, member, cursor, observation, budget))
    }
}
