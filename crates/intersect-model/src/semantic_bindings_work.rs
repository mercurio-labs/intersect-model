//! Handwritten opt-in work authority. This grants no model/reference/read input.
use super::{Budget, Error, ErrorKind, Result};
use crate::{core::query::QueryBudget, rcore_behavior_functions::ExecutionControl};
use intersect_model::definitions::conversion::{InvocationContext, InvocationObserver, ProviderId};
use std::{cell::Cell, cell::RefCell, rc::Rc};

mod sealed {
    pub trait Sealed {}
    impl Sealed for super::Budget {}
    impl Sealed for super::BorrowedBindingWork<'_> {}
}

/// Invocation work only. Implementations cannot mint source or model authority.
/// Existing owned bindings still use their original concrete `Budget` facades.
pub trait BindingWork: sealed::Sealed + ExecutionControl + InvocationObserver {
    fn check_work(&self) -> Result<()>;
    fn charge_work(&self, units: u64) -> Result<()>;
    fn used_work(&self) -> u64;
}
impl BindingWork for Budget {
    fn check_work(&self) -> Result<()> {
        self.check()
    }
    fn charge_work(&self, units: u64) -> Result<()> {
        self.charge(units)
    }
    fn used_work(&self) -> u64 {
        self.used()
    }
}

#[derive(Clone, Copy)]
struct Failure {
    kind: ErrorKind,
    message: &'static str,
}

/// Borrows the original caller's allowance for this entire scope. There is no
/// copied allowance, debit-after reconciliation, raw QueryBudget getter or
/// conversion to an owned F10 Budget. The sidecar is neither Clone nor Serialize.
/// It does not enable legacy DerivedValues, model handles or semantic operations.
pub struct BorrowedBindingWork<'caller> {
    caller: RefCell<&'caller mut QueryBudget>,
    failure: Cell<Option<Failure>>,
    used: Cell<u64>,
}
impl<'caller> BorrowedBindingWork<'caller> {
    pub fn bind(caller: &'caller mut QueryBudget) -> Self {
        Self {
            caller: RefCell::new(caller),
            failure: Cell::new(None),
            used: Cell::new(0),
        }
    }
    fn fail(&self, kind: ErrorKind, message: &'static str) -> Error {
        let failure = match self.failure.get() {
            Some(failure) => failure,
            None => Failure { kind, message },
        };
        self.failure.set(Some(failure));
        Error::new(failure.kind, failure.message)
    }
    pub fn check(&self) -> Result<()> {
        match self.failure.get() {
            Some(failure) => Err(Error::new(failure.kind, failure.message)),
            None => Ok(()),
        }
    }
    pub fn cancel(&self) {
        let _ = self.fail(ErrorKind::Cancelled, "Binding invocation cancelled");
    }
    /// Atomic multi-unit admission, matching owned Budget semantics: a denied
    /// request debits nothing. Every accepted unit invokes original charge().
    pub fn charge(&self, units: u64) -> Result<()> {
        self.check()?;
        let units = usize::try_from(units).map_err(|_| {
            self.fail(
                ErrorKind::ResourceExhausted,
                "Binding work budget exhausted",
            )
        })?;
        let mut caller = self.caller.try_borrow_mut().map_err(|_| {
            self.fail(
                ErrorKind::Unsupported,
                "Binding work authority is already borrowed",
            )
        })?;
        if units > caller.remaining() || caller.reads.checked_add(units).is_none() {
            return Err(self.fail(
                ErrorKind::ResourceExhausted,
                "Binding work budget exhausted",
            ));
        }
        for _ in 0..units {
            caller.charge().map_err(|_| {
                self.fail(
                    ErrorKind::ResourceExhausted,
                    "Binding work budget exhausted",
                )
            })?;
        }
        self.used.set(self.used.get().saturating_add(units as u64));
        Ok(())
    }
    /// Accepted work in this scope; a metric, never another allowance.
    pub fn used(&self) -> u64 {
        self.used.get()
    }
    pub fn observer(&self) -> InvocationContext<'_> {
        InvocationContext::new(Rc::new(BorrowedObservation(self)))
    }
    /// Private compatibility bridge for an existing native read callback. The
    /// callback borrows the actual ledger; no allowance is copied/reconciled or
    /// made available to source code. Public callers cannot obtain this borrow.
    pub(crate) fn read_callback<T>(
        &self,
        read: impl FnOnce(&mut QueryBudget) -> std::result::Result<T, String>,
    ) -> std::result::Result<T, String> {
        self.check().map_err(|error| error.to_string())?;
        let mut caller = self
            .caller
            .try_borrow_mut()
            .map_err(|_| "Read callback budget is already borrowed")?;
        let before = caller.reads;
        let remaining = caller.remaining();
        let result = read(&mut caller);
        if caller.reads < before
            || caller.remaining() > remaining
            || caller.reads - before != remaining - caller.remaining()
        {
            caller.invalidate_replacement(before);
            return Err(self
                .fail(
                    ErrorKind::Contract,
                    "Read callback changed its original allowance",
                )
                .to_string());
        }
        self.used.set(
            self.used
                .get()
                .saturating_add((caller.reads - before) as u64),
        );
        result
    }
}
impl BindingWork for BorrowedBindingWork<'_> {
    fn check_work(&self) -> Result<()> {
        self.check()
    }
    fn charge_work(&self, units: u64) -> Result<()> {
        self.charge(units)
    }
    fn used_work(&self) -> u64 {
        self.used()
    }
}
impl ExecutionControl for BorrowedBindingWork<'_> {
    fn cancellation_requested(&self) -> bool {
        self.failure.get().is_some()
    }
}
impl InvocationObserver for BorrowedBindingWork<'_> {
    fn before_invoke(&self, _: &ProviderId) -> std::result::Result<(), String> {
        self.work(1)
    }
    fn checkpoint(&self) -> std::result::Result<(), String> {
        self.check().map_err(|error| error.to_string())
    }
    fn work(&self, units: usize) -> std::result::Result<(), String> {
        self.charge(units as u64).map_err(|error| error.to_string())
    }
}
struct BorrowedObservation<'scope, 'caller>(&'scope BorrowedBindingWork<'caller>);
impl InvocationObserver for BorrowedObservation<'_, '_> {
    fn before_invoke(&self, provider: &ProviderId) -> std::result::Result<(), String> {
        self.0.before_invoke(provider)
    }
    fn checkpoint(&self) -> std::result::Result<(), String> {
        self.0.checkpoint()
    }
    fn work(&self, units: usize) -> std::result::Result<(), String> {
        self.0.work(units)
    }
}

#[cfg(test)]
#[path = "semantic_bindings_work_tests.rs"]
mod tests;
