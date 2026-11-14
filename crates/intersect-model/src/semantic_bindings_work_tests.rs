use super::*;
use crate::semantic_bindings::NativeValue;
use std::mem::size_of;

#[test]
fn borrowed_zero_exact_one_short_and_preconsumed_debit_original_caller() {
    let mut caller = QueryBudget::new(7);
    caller.charge().unwrap();
    {
        let work = BorrowedBindingWork::bind(&mut caller);
        work.charge(0).unwrap();
        work.charge(6).unwrap();
        assert_eq!(work.used(), 6);
        assert!(
            work.check().is_ok(),
            "exactly zero remaining is still success"
        );
        assert_eq!(
            work.charge(1).unwrap_err().kind,
            ErrorKind::ResourceExhausted
        );
        assert_eq!(work.used(), 6);
        assert_eq!(
            work.charge(0).unwrap_err().kind,
            ErrorKind::ResourceExhausted
        );
        assert!(work.cancellation_requested());
    }
    assert_eq!((caller.remaining(), caller.reads), (0, 7));
    let mut short = QueryBudget::new(5);
    {
        let work = BorrowedBindingWork::bind(&mut short);
        assert_eq!(
            work.charge(6).unwrap_err().kind,
            ErrorKind::ResourceExhausted
        );
        assert_eq!(work.used(), 0);
        assert!(work.charge(1).is_err(), "caught denial stays sticky");
    }
    assert_eq!((short.remaining(), short.reads), (5, 0));
}

#[test]
fn borrowed_observer_cancel_reentry_and_counter_overflow_never_reset_or_hide_failure() {
    let mut caller = QueryBudget::new(9);
    {
        let work = BorrowedBindingWork::bind(&mut caller);
        work.observer().work(3).unwrap();
        assert_eq!(work.used(), 3);
        work.cancel();
        let cancellation = work.check().unwrap_err();
        assert_eq!(cancellation.kind, ErrorKind::Cancelled);
        assert_eq!(cancellation.message, "Binding invocation cancelled");
        assert!(work.observer().work(1).is_err());
        assert_eq!(work.used(), 3);
    }
    assert_eq!((caller.remaining(), caller.reads), (6, 3));
    {
        let work = BorrowedBindingWork::bind(&mut caller);
        let held = work.caller.borrow_mut();
        let failure = work.charge(1).unwrap_err();
        assert_eq!(failure.kind, ErrorKind::Unsupported);
        assert_eq!(
            failure.message,
            "Binding work authority is already borrowed"
        );
        drop(held);
        assert!(work.charge(1).is_err());
        assert_eq!(work.used(), 0);
    }
    assert_eq!((caller.remaining(), caller.reads), (6, 3));
    caller.reads = usize::MAX;
    {
        let work = BorrowedBindingWork::bind(&mut caller);
        assert_eq!(
            work.charge(1).unwrap_err().kind,
            ErrorKind::ResourceExhausted
        );
    }
    assert_eq!((caller.remaining(), caller.reads), (6, usize::MAX));
}

#[test]
fn owned_binding_budget_literal_fees_errors_and_existing_holders_remain_exact() {
    let owned = Budget::new(7);
    owned.charge_work(2).unwrap();
    owned.charge_work(4).unwrap();
    assert_eq!(owned.used_work(), 6);
    assert!(owned.check_work().is_ok());
    let failure = owned.charge_work(2).unwrap_err();
    assert_eq!(failure.kind, ErrorKind::ResourceExhausted);
    assert_eq!(failure.message, "Binding work budget exhausted");
    assert_eq!(owned.used_work(), 6);
    assert_eq!(owned.charge_work(0).unwrap_err().message, failure.message);
    assert_eq!(
        size_of::<Budget>(),
        size_of::<Rc<super::super::BudgetState>>()
    );
    struct OldQueryBudget {
        _remaining: usize,
        _reads: usize,
    }
    assert_eq!(size_of::<QueryBudget>(), size_of::<OldQueryBudget>());
}

// A test driver over the SAME two F10 continuations. It invokes no native
// provider, semantic getter, operation algorithm, reference bridge or body.
fn phase_result<W: BindingWork>(
    contract: &crate::definitions::semantic_bindings::Contract,
    arguments: &[crate::definitions::rcore_behavior::ir_v2::ValueWire],
    context: &crate::rcore_behavior_functions::ExecutionContext<'_>,
    limits: &crate::rcore_behavior_functions::ExecutionLimits,
    work: &W,
) -> super::Result<crate::rcore_behavior_functions::Evaluation> {
    let mut session = crate::rcore_behavior_functions::EvaluationSession::new_binding(
        contract.admission_image(),
        arguments,
        context,
        limits,
    )?;
    loop {
        if session.payload_continuation() {
            work.check_work()?;
        } else {
            work.charge_work(1)?;
        }
        if session.prepare_binding_step_with_work(work)? {
            break;
        }
    }
    session.set_binding_result(arguments[0].clone())?;
    loop {
        if session.payload_continuation() {
            work.check_work()?;
        } else {
            work.charge_work(1)?;
        }
        if let Some(value) = session.finish_binding_step_with_work(work)? {
            work.check_work()?;
            return Ok(value);
        }
    }
}

#[test]
fn same_f10_phase_workers_preserve_whole_result_work_exact_short_and_context() {
    let sources = crate::definitions::semantic_bindings::compile_sources(
        include_str!("../../../fixtures/rcore/bindings/model.rcore"),
        "rcore 1; package borrowed version \"1\"; extern fn identity(value:Int64)->Int64;",
    )
    .unwrap();
    let contract = &sources.contracts[0];
    let args = [21_i64
        .into_wire(contract.descriptor().parameters[0])
        .unwrap()];
    let context = crate::rcore_behavior_functions::ExecutionContext::new(&sources.bundle);
    let limits = crate::rcore_behavior_functions::ExecutionLimits::default();
    let old = Budget::new(100_000);
    let baseline = phase_result(contract, &args, &context, &limits, &old).unwrap();
    assert_eq!(i64::from_wire(&baseline.value).unwrap(), 21);
    let measured = old.used();
    assert!(measured > 1 && measured < 100_000);
    let mut caller = QueryBudget::new(measured as usize + 3);
    caller.charge().unwrap();
    caller.charge().unwrap();
    caller.charge().unwrap();
    {
        let work = BorrowedBindingWork::bind(&mut caller);
        let value = phase_result(contract, &args, &context, &limits, &work).unwrap();
        assert_eq!(
            serde_json::to_value(value).unwrap(),
            serde_json::to_value(baseline).unwrap()
        );
        assert_eq!(work.used(), measured);
        work.check().unwrap();
    }
    assert_eq!(caller.remaining(), 0);
    assert_eq!(caller.reads, measured as usize + 3);
    let mut short = QueryBudget::new(measured as usize - 1);
    {
        let work = BorrowedBindingWork::bind(&mut short);
        assert_eq!(
            phase_result(contract, &args, &context, &limits, &work)
                .unwrap_err()
                .kind,
            ErrorKind::ResourceExhausted
        );
        assert_eq!(work.used(), measured - 1);
        assert!(work.check().is_err());
    }
    assert_eq!(short.remaining(), 0);
}
