use intersect_model::{
    definitions::{rcore_behavior::ir_v2::ValueWire, semantic_bindings::compile_sources},
    rcore_behavior_functions::{ExecutionContext, ExecutionLimits},
    semantic_bindings::*,
};
use std::{cell::Cell, num::NonZeroUsize, rc::Rc};

fn finish(session: &mut BindingSession<'_>) -> InvocationResult {
    loop {
        match session.resume(NonZeroUsize::MIN) {
            BindingProgress::Pending => {}
            BindingProgress::Complete(value) => return (*value).unwrap(),
            BindingProgress::Finished => panic!("Unexpected finished session"),
        }
    }
}
#[test]
fn replacement_during_suspended_rcore_execution_retains_original_selection() {
    let s=compile_sources(include_str!("../../../fixtures/rcore/bindings/model.rcore"),
        "rcore 1; package suspension version \"1\"; fn twice(value:Int64)->Int64 { count(value,100) } fn count(value:Int64,left:Int64)->Int64 { if left==0 { value+value } else { count(value,left-1) } }").unwrap();
    let c = &s.contracts[0];
    let mut registry = Registry::default();
    registry.install_rcore(c).unwrap();
    let pinned = registry.pin(c).unwrap();
    let args = [21_i64.into_wire(c.descriptor().parameters[0]).unwrap()];
    let context = ExecutionContext::new(&s.bundle);
    let limits = ExecutionLimits::default();
    let budget = Budget::new(100_000);
    let mut session = pinned
        .start(&args, &context, &limits, budget.clone())
        .unwrap();
    // Resource continuations add Pending without inventing binding work fees.
    // Wait for the original charged-work boundary before replacing the registry.
    for _ in 0..10_000 {
        if budget.used() >= 100 {
            break;
        }
        assert!(matches!(
            session.resume(NonZeroUsize::MIN),
            BindingProgress::Pending
        ));
    }
    assert!(budget.used() >= 100);
    registry
        .replace(
            c,
            Rc::new(NativeProvider::new(
                c,
                "replacement",
                |call: &Call<'_>, _: &[ValueWire]| call.result(7_i64),
            )),
        )
        .unwrap();
    let next = registry
        .pin(c)
        .unwrap()
        .invoke(&args, &context, &limits, Budget::new(100_000))
        .unwrap();
    assert_eq!(i64::from_wire(&next.evaluation.value).unwrap(), 7);
    registry.remove(c).unwrap();
    let result = finish(&mut session);
    assert_eq!(result.implementation.backend, "rcore");
    assert_eq!(i64::from_wire(&result.evaluation.value).unwrap(), 42);
    assert!(matches!(
        session.resume(NonZeroUsize::MIN),
        BindingProgress::Finished
    ));
}
#[test]
fn native_invokes_once_and_pending_cancellation_is_terminal() {
    let (bundle, contracts, _) = crate::contract::contracts().unwrap();
    let c = &contracts[0];
    let count = Rc::new(Cell::new(0));
    let observed = count.clone();
    let mut registry = Registry::default();
    registry
        .install(
            c,
            Rc::new(NativeProvider::new(
                c,
                "once",
                move |call: &Call<'_>, _: &[ValueWire]| {
                    observed.set(observed.get() + 1);
                    call.result(42_i64)
                },
            )),
        )
        .unwrap();
    let pinned = registry.pin(c).unwrap();
    let args = [21_i64.into_wire(c.descriptor().parameters[0]).unwrap()];
    let context = ExecutionContext::new(&bundle);
    let limits = ExecutionLimits::default();
    let mut session = pinned
        .start(&args, &context, &limits, Budget::new(100_000))
        .unwrap();
    assert_eq!(
        i64::from_wire(&finish(&mut session).evaluation.value).unwrap(),
        42
    );
    assert_eq!(count.get(), 1);
    assert!(matches!(
        session.resume(NonZeroUsize::MIN),
        BindingProgress::Finished
    ));
    let budget = Budget::new(100_000);
    let mut cancelled = pinned
        .start(&args, &context, &limits, budget.clone())
        .unwrap();
    assert!(matches!(
        cancelled.resume(NonZeroUsize::MIN),
        BindingProgress::Pending
    ));
    budget.cancel();
    let BindingProgress::Complete(error) = cancelled.resume(NonZeroUsize::MIN) else {
        panic!("Cancellation must finish");
    };
    assert_eq!(error.unwrap_err().kind, ErrorKind::Cancelled);
    assert_eq!(count.get(), 1);
    assert!(matches!(
        cancelled.resume(NonZeroUsize::MIN),
        BindingProgress::Finished
    ));
}
#[test]
fn generated_package_publication_is_atomic_and_revision_checked() {
    let (bundle, contracts, _) = crate::checked_contract::contracts().unwrap();
    let mut registry = Registry::default();
    crate::generated::register_checked_twice(&mut registry, &contracts[0]).unwrap();
    assert_eq!(
        crate::generated::register_checked_package(&mut registry, &contracts)
            .unwrap_err()
            .kind,
        ErrorKind::DuplicateImplementation
    );
    assert!(registry.pin(&contracts[1]).is_err());
    let args = [21_i64
        .into_wire(contracts[0].descriptor().parameters[0])
        .unwrap()];
    let result = registry
        .pin(&contracts[0])
        .unwrap()
        .invoke(
            &args,
            &ExecutionContext::new(&bundle),
            &ExecutionLimits::default(),
            Budget::new(100_000),
        )
        .unwrap();
    assert_eq!(i64::from_wire(&result.evaluation.value).unwrap(), 42);
    let mut registry = Registry::default();
    let mut wrong = contracts.clone();
    wrong.reverse();
    assert_eq!(
        crate::generated::register_checked_package(&mut registry, &wrong)
            .unwrap_err()
            .kind,
        ErrorKind::Contract
    );
    assert!(registry.pin(&contracts[0]).is_err());
    crate::generated::register_checked_package(&mut registry, &contracts).unwrap();
    assert!(registry.pin(&contracts[1]).is_ok());
}
#[test]
fn generation_is_deterministic_and_rejects_duplicate_exports_and_unsupported_types() {
    use intersect_model::codegen::semantic_bindings::{Export, generate, generate_package};
    let (_, contracts, _) = crate::checked_contract::contracts().unwrap();
    let c = &contracts[0];
    let export = || Export {
        contract: c,
        rust_path: "crate::native::twice",
        registration: "register_twice",
        implementation_revision: "v1",
    };
    assert_eq!(
        generate_package(&[export()], "register_package").unwrap(),
        generate_package(&[export()], "register_package").unwrap()
    );
    assert!(generate_package(&[export(), export()], "register_package").is_err());
    assert!(generate(c, "crate::native::twice", "register_twice", "").is_err());
    assert!(generate(c, "crate::native::twice", "fn", "v1").is_err());
    let s = compile_sources(
        include_str!("../../../fixtures/rcore/bindings/model.rcore"),
        "rcore 1; package unsupported version \"1\"; extern fn real(value:Float64)->Float64;",
    )
    .unwrap();
    assert!(generate(&s.contracts[0], "crate::real", "register_real", "v1").is_err());
}
