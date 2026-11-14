//! Evaluation-scoped, bounded receipts from actual shared provider dispatch.
use super::{Consumption, ExecutionLimits, preflight};
use intersect_model::definitions::conversion::{
    InvocationObserver, ProviderDependencyKind, ProviderId,
};
use std::cell::{Cell, RefCell};

#[derive(Default)]
struct Evidence {
    selected: Vec<ProviderId>,
    consumed: Vec<ProviderId>,
    bytes: usize,
    invocations: usize,
    incomplete_selection: bool,
}
#[derive(Clone, Copy)]
enum ResourceLimit {
    Evidence,
    Invocations,
    Work,
    Storage,
}
impl ResourceLimit {
    fn message(self) -> &'static str {
        match self {
            Self::Evidence => "Provider evidence byte limit exceeded",
            Self::Invocations => "Provider invocation limit exceeded",
            Self::Work => "Provider work limit exceeded",
            Self::Storage => "Retained model storage limit exceeded",
        }
    }
}
pub(super) struct Receipts {
    evidence: RefCell<Evidence>,
    exhausted: Cell<Option<ResourceLimit>>,
    cancelled: Cell<bool>,
    missing: Cell<Option<ProviderDependencyKind>>,
    max_bytes: usize,
    max_invocations: usize,
    work: Cell<usize>,
    max_work: usize,
    storage: Cell<usize>,
    max_storage: usize,
}
impl Receipts {
    pub fn new(limits: &ExecutionLimits) -> Self {
        Self {
            evidence: RefCell::new(Evidence::default()),
            exhausted: Cell::new(None),
            cancelled: Cell::new(false),
            missing: Cell::new(None),
            max_bytes: limits.max_provider_evidence_bytes,
            max_invocations: limits.max_provider_invocations,
            work: Cell::new(0),
            max_work: limits.max_provider_work,
            storage: Cell::new(0),
            max_storage: limits.max_model_storage_bytes,
        }
    }
    fn retain(
        items: &mut Vec<ProviderId>,
        bytes: &mut usize,
        limit: usize,
        provider: &ProviderId,
    ) -> Result<(), String> {
        let position = items
            .binary_search_by(|p| (&p.name, &p.version).cmp(&(&provider.name, &provider.version)));
        let Err(position) = position else {
            return Ok(());
        };
        let remaining = limit
            .checked_sub(*bytes)
            .ok_or("Provider evidence byte limit exceeded")?;
        let size = preflight::serialized_size(provider, remaining)
            .map_err(|_| "Provider evidence byte limit exceeded")?;
        // Only one retained identity owns these strings; map keys do not duplicate them.
        items.insert(position, provider.clone());
        *bytes += size;
        Ok(())
    }
    pub fn select(&self, provider: &ProviderId) -> Result<(), String> {
        self.checkpoint()?;
        let mut state = self
            .evidence
            .try_borrow_mut()
            .map_err(|_| "Provider receipt is already borrowed")?;
        let Evidence {
            selected, bytes, ..
        } = &mut *state;
        if let Err(error) = Self::retain(selected, bytes, self.max_bytes, provider) {
            state.incomplete_selection = true;
            self.exhausted.set(Some(ResourceLimit::Evidence));
            return Err(error);
        }
        Ok(())
    }
    pub fn scoped<'a>(
        self: &std::rc::Rc<Self>,
        control: Option<&'a dyn super::ExecutionControl>,
    ) -> intersect_model::definitions::conversion::InvocationContext<'a> {
        intersect_model::definitions::conversion::InvocationContext::new(std::rc::Rc::new(Scoped {
            receipts: self.clone(),
            control,
        }))
    }
    /// New borrowed inputs keep the original caller observer AND the existing
    /// provider/storage limits. The ordinary scoped observer above is literal.
    /// This sidecar is transient; it adds no field to Receipts or old sessions.
    pub fn scoped_borrowed<'a>(
        self: &std::rc::Rc<Self>,
        original: intersect_model::definitions::conversion::InvocationContext<'a>,
        control: Option<&'a dyn super::ExecutionControl>,
    ) -> Result<intersect_model::definitions::conversion::InvocationContext<'a>, String> {
        original.work(1)?;
        let bytes = crate::logical_size!(BorrowedScoped<'_>, 40);
        self.retain_storage(bytes)?;
        original.retain_storage(bytes)?;
        Ok(
            intersect_model::definitions::conversion::InvocationContext::new(std::rc::Rc::new(
                BorrowedScoped {
                    original,
                    receipts: self.clone(),
                    control,
                },
            )),
        )
    }
    pub fn cancelled(&self) -> bool {
        self.cancelled.get()
    }
    pub fn missing(&self) -> Option<ProviderDependencyKind> {
        self.missing.get()
    }
    pub fn exhausted(&self) -> bool {
        self.exhausted.get().is_some()
    }
    pub fn resource_message(&self) -> Option<&'static str> {
        self.exhausted.get().map(ResourceLimit::message)
    }
    pub fn counters(&self, output: &mut Consumption) {
        output.provider_work = self.work.get();
        output.model_storage_bytes = self.storage.get();
        if let Ok(state) = self.evidence.try_borrow() {
            output.provider_invocations = state.invocations;
            output.provider_evidence_bytes = state.bytes;
        }
    }
    pub fn error_evidence(
        &self,
        remaining: &mut usize,
    ) -> (Vec<ProviderId>, Vec<ProviderId>, bool) {
        let Ok(state) = self.evidence.try_borrow() else {
            return (vec![], vec![], true);
        };
        let mut truncated = state.incomplete_selection;
        let mut copy = |items: &[ProviderId]| {
            let mut output = Vec::new();
            for item in items {
                match preflight::serialized_size(item, *remaining) {
                    Ok(size) => {
                        *remaining -= size;
                        output.push(item.clone());
                    }
                    Err(_) => truncated = true,
                }
            }
            output
        };
        let selected = copy(&state.selected);
        let consumed = copy(&state.consumed);
        (selected, consumed, truncated)
    }
    pub fn take(&self) -> Result<(Vec<ProviderId>, Vec<ProviderId>), String> {
        let mut state = self
            .evidence
            .try_borrow_mut()
            .map_err(|_| "Provider receipt is already borrowed")?;
        Ok((
            std::mem::take(&mut state.selected),
            std::mem::take(&mut state.consumed),
        ))
    }
}
impl InvocationObserver for Receipts {
    fn retain_storage(&self, bytes: usize) -> Result<(), String> {
        self.checkpoint()?;
        let next = self
            .storage
            .get()
            .checked_add(bytes)
            .filter(|next| *next <= self.max_storage);
        let Some(next) = next else {
            self.exhausted.set(Some(ResourceLimit::Storage));
            return Err(ResourceLimit::Storage.message().into());
        };
        self.storage.set(next);
        Ok(())
    }
    fn dependency_unavailable(&self, kind: ProviderDependencyKind) {
        // Preserve the first terminal failure even when provider code catches it.
        if !self.cancelled.get() && !self.exhausted() && self.missing.get().is_none() {
            self.missing.set(Some(kind));
        }
    }
    fn checkpoint(&self) -> Result<(), String> {
        if self.cancelled.get() {
            return Err("Execution cancelled during provider work".into());
        }
        if self.missing.get().is_some() {
            return Err("Required provider dependency is unavailable".into());
        }
        if let Some(message) = self.resource_message() {
            return Err(message.into());
        }
        Ok(())
    }
    fn work(&self, units: usize) -> Result<(), String> {
        self.checkpoint()?;
        let next = self
            .work
            .get()
            .checked_add(units)
            .filter(|next| *next <= self.max_work);
        let Some(next) = next else {
            self.exhausted.set(Some(ResourceLimit::Work));
            return Err(ResourceLimit::Work.message().into());
        };
        self.work.set(next);
        Ok(())
    }
    fn before_invoke(&self, provider: &ProviderId) -> Result<(), String> {
        self.checkpoint()?;
        let mut state = self
            .evidence
            .try_borrow_mut()
            .map_err(|_| "Provider receipt is already borrowed")?;
        if state.invocations >= self.max_invocations {
            self.exhausted.set(Some(ResourceLimit::Invocations));
            return Err(ResourceLimit::Invocations.message().into());
        }
        // Reserve dispatch work before copying its receipt or invoking the callback.
        self.work(1)?;
        let Evidence {
            consumed, bytes, ..
        } = &mut *state;
        if let Err(error) = Self::retain(consumed, bytes, self.max_bytes, provider) {
            self.exhausted.set(Some(ResourceLimit::Evidence));
            return Err(error);
        }
        state.invocations += 1;
        Ok(())
    }
}

// The registry/session retains receipts only. Borrowed control lives for one dispatch.
struct Scoped<'a> {
    receipts: std::rc::Rc<Receipts>,
    control: Option<&'a dyn super::ExecutionControl>,
}

// All inner work is admitted by the original caller BEFORE the unchanged
// provider counter, and before its semantic worker. A later provider-limit
// denial does not refund caller admission work. No callback occurs while any
// receipt/QueryBudget interior borrow is live. Logical storage remains a
// separate cumulative limit, not a claim about total RSS or allocator traffic.
struct BorrowedScoped<'a> {
    original: intersect_model::definitions::conversion::InvocationContext<'a>,
    receipts: std::rc::Rc<Receipts>,
    control: Option<&'a dyn super::ExecutionControl>,
}
impl InvocationObserver for BorrowedScoped<'_> {
    fn checkpoint(&self) -> Result<(), String> {
        self.original.checkpoint()?;
        self.receipts.checkpoint()?;
        if self
            .control
            .is_some_and(|control| control.cancellation_requested())
        {
            self.receipts.cancelled.set(true);
            return Err("Execution cancelled during provider work".into());
        }
        Ok(())
    }
    fn work(&self, units: usize) -> Result<(), String> {
        self.checkpoint()?;
        self.original.work(units)?;
        self.receipts.work(units)
    }
    fn retain_storage(&self, bytes: usize) -> Result<(), String> {
        self.checkpoint()?;
        self.original.retain_storage(bytes)?;
        self.receipts.retain_storage(bytes)
    }
    fn before_invoke(&self, provider: &ProviderId) -> Result<(), String> {
        self.checkpoint()?;
        self.original.invoke(provider)?;
        self.receipts.before_invoke(provider)
    }
    fn dependency_unavailable(&self, kind: ProviderDependencyKind) {
        self.original.dependency_unavailable(kind);
        self.receipts.dependency_unavailable(kind);
    }
    fn failed_result(&self, error: &str) {
        self.original.failed_result(error);
        self.receipts.failed_result(error);
    }
}
impl InvocationObserver for Scoped<'_> {
    fn retain_storage(&self, bytes: usize) -> Result<(), String> {
        self.checkpoint()?;
        self.receipts.retain_storage(bytes)
    }
    fn checkpoint(&self) -> Result<(), String> {
        self.receipts.checkpoint()?;
        if self
            .control
            .is_some_and(|control| control.cancellation_requested())
        {
            self.receipts.cancelled.set(true);
            return Err("Execution cancelled during provider work".into());
        }
        Ok(())
    }
    fn work(&self, units: usize) -> Result<(), String> {
        self.checkpoint()?;
        self.receipts.work(units)
    }
    fn before_invoke(&self, provider: &ProviderId) -> Result<(), String> {
        self.checkpoint()?;
        self.receipts.before_invoke(provider)
    }
    fn dependency_unavailable(&self, kind: ProviderDependencyKind) {
        self.receipts.dependency_unavailable(kind);
    }
}

#[cfg(test)]
mod borrowed_work_tests {
    use super::*;
    use crate::{core::query::QueryBudget, semantic_bindings::BorrowedBindingWork};

    #[test]
    fn borrowed_inner_work_debits_original_before_provider_work_and_denial() {
        let mut caller = QueryBudget::new(4);
        {
            let work = BorrowedBindingWork::bind(&mut caller);
            let receipts = std::rc::Rc::new(Receipts::new(&ExecutionLimits::default()));
            let observation = receipts
                .scoped_borrowed(work.observer(), Some(&work))
                .unwrap();
            assert_eq!(work.used(), 1, "new transient observer admission");
            assert_eq!(receipts.work.get(), 0);
            observation.work(3).unwrap();
            assert_eq!(work.used(), 4);
            assert_eq!(receipts.work.get(), 3);
            assert!(observation.work(1).is_err());
            assert_eq!(work.used(), 4);
            assert_eq!(
                receipts.work.get(),
                3,
                "denied original work cannot reach provider worker"
            );
            assert!(observation.checkpoint().is_err());
            assert!(work.check().is_err());
        }
        assert_eq!(caller.reads, 4);
        assert_eq!(caller.remaining(), 0);

        let mut fresh = QueryBudget::new(2);
        let work = BorrowedBindingWork::bind(&mut fresh);
        let limits = ExecutionLimits {
            max_provider_work: 0,
            ..Default::default()
        };
        let receipts = std::rc::Rc::new(Receipts::new(&limits));
        let observation = receipts
            .scoped_borrowed(work.observer(), Some(&work))
            .unwrap();
        assert!(observation.work(1).is_err());
        assert_eq!(
            work.used(),
            2,
            "admission attempt stays spent on later provider denial"
        );
        assert_eq!(receipts.work.get(), 0);
        assert!(receipts.exhausted());
        assert!(observation.checkpoint().is_err());
    }

    #[test]
    fn default_observer_keeps_old_literal_work_storage_and_holder_layout() {
        let limits = ExecutionLimits::default();
        let receipts = std::rc::Rc::new(Receipts::new(&limits));
        let mut caller = QueryBudget::new(0);
        let original = BorrowedBindingWork::bind(&mut caller);
        let observation = receipts.scoped(Some(&original));
        observation.work(3).unwrap();
        observation.retain_storage(5).unwrap();
        assert_eq!(receipts.work.get(), 3);
        assert_eq!(receipts.storage.get(), 5);
        assert_eq!(
            original.used(),
            0,
            "ordinary evaluator retains its old receipt-only observer"
        );
        struct OldReceipts {
            _evidence: RefCell<Evidence>,
            _exhausted: Cell<Option<ResourceLimit>>,
            _cancelled: Cell<bool>,
            _missing: Cell<Option<ProviderDependencyKind>>,
            _max_bytes: usize,
            _max_invocations: usize,
            _work: Cell<usize>,
            _max_work: usize,
            _storage: Cell<usize>,
            _max_storage: usize,
        }
        assert_eq!(
            std::mem::size_of::<Receipts>(),
            std::mem::size_of::<OldReceipts>()
        );
        assert_eq!(
            std::mem::align_of::<Receipts>(),
            std::mem::align_of::<OldReceipts>()
        );
    }
}

#[cfg(test)]
mod work_tests {
    use super::*;
    #[test]
    fn each_resource_cause_survives_later_failures_and_cancellation() {
        struct Cancel(Cell<usize>);
        impl super::super::ExecutionControl for Cancel {
            fn cancellation_requested(&self) -> bool {
                self.0.set(self.0.get() + 1);
                true
            }
        }
        let provider = ProviderId {
            name: "test.receipt".into(),
            version: "1".into(),
        };
        for which in 0..5 {
            let limits = ExecutionLimits {
                max_provider_evidence_bytes: if which < 2 { 0 } else { 65536 },
                max_provider_invocations: if which == 2 { 0 } else { 1024 },
                max_provider_work: if which == 3 { 0 } else { 1024 },
                max_model_storage_bytes: if which == 4 { 0 } else { 1024 },
                ..ExecutionLimits::default()
            };
            let receipts = std::rc::Rc::new(Receipts::new(&limits));
            let original = if which == 0 {
                receipts.select(&provider)
            } else if which == 4 {
                receipts.retain_storage(1)
            } else {
                receipts.before_invoke(&provider)
            }
            .unwrap_err();
            assert_eq!(
                original,
                match which {
                    0 | 1 => "Provider evidence byte limit exceeded",
                    2 => "Provider invocation limit exceeded",
                    3 => "Provider work limit exceeded",
                    _ => "Retained model storage limit exceeded",
                }
            );
            let mut counts = Consumption::default();
            receipts.counters(&mut counts);
            assert_eq!(receipts.checkpoint().unwrap_err(), original);
            assert_eq!(receipts.select(&provider).unwrap_err(), original);
            assert_eq!(receipts.work(usize::MAX).unwrap_err(), original);
            assert_eq!(receipts.retain_storage(usize::MAX).unwrap_err(), original);
            assert_eq!(receipts.before_invoke(&provider).unwrap_err(), original);
            receipts.dependency_unavailable(ProviderDependencyKind::Constructor);
            assert!(receipts.missing().is_none());
            let control = Cancel(Cell::new(0));
            assert_eq!(
                receipts.scoped(Some(&control)).checkpoint().unwrap_err(),
                original
            );
            assert_eq!(
                control.0.get(),
                0,
                "later cancellation cannot replace an observed failure"
            );
            let mut after = Consumption::default();
            receipts.counters(&mut after);
            assert_eq!(
                serde_json::to_value(counts).unwrap(),
                serde_json::to_value(after).unwrap()
            );
        }
    }
    #[test]
    fn work_overflow_and_first_failure_remain_terminal() {
        let limits = ExecutionLimits {
            max_provider_work: usize::MAX,
            ..ExecutionLimits::default()
        };
        let receipts = Receipts::new(&limits);
        receipts.work(usize::MAX).unwrap();
        assert!(receipts.work(1).is_err());
        assert_eq!(receipts.work.get(), usize::MAX);
        receipts.dependency_unavailable(ProviderDependencyKind::Conversion);
        assert!(receipts.missing().is_none());
        assert!(receipts.work(0).is_err());
        let fresh = Receipts::new(&limits);
        fresh.work(2).unwrap();
        fresh.dependency_unavailable(ProviderDependencyKind::Constructor);
        assert!(fresh.work(usize::MAX).is_err());
        assert_eq!(fresh.work.get(), 2);
        assert!(!fresh.exhausted());
    }
    #[test]
    fn storage_overflow_is_terminal_and_a_fresh_scope_recovers() {
        let limits = ExecutionLimits {
            max_model_storage_bytes: usize::MAX,
            ..Default::default()
        };
        let receipts = Receipts::new(&limits);
        receipts.retain_storage(usize::MAX).unwrap();
        assert_eq!(
            receipts.retain_storage(1).unwrap_err(),
            "Retained model storage limit exceeded"
        );
        assert_eq!(receipts.storage.get(), usize::MAX);
        assert!(receipts.retain_storage(0).is_err());
        let fresh = Receipts::new(&limits);
        fresh.retain_storage(2).unwrap();
        assert_eq!(fresh.storage.get(), 2);
        fresh.dependency_unavailable(ProviderDependencyKind::Constructor);
        assert!(fresh.retain_storage(usize::MAX).is_err());
        assert_eq!(fresh.storage.get(), 2);
        assert!(!fresh.exhausted());
    }
}
