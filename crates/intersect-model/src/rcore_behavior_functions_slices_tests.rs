//! Internal controls for the shared machine's scheduling boundary.
use super::*;
use intersect_model::definitions::{
    bundle::{DefinitionSource, SourceKind},
    rcore_behavior::{
        function_lowering::compile_function,
        ir_v2::{EntrySelection, Limits},
        modules::SymbolId,
    },
};
use std::num::NonZeroUsize;

fn fixture(body: &str) -> (DefinitionBundle, VerifiedExecutable) {
    let source = DefinitionSource::new(
        "memory:slices-behavior.rcore",
        SourceKind::RcoreBehavior,
        format!("rcore 1;package slices version \"1\";{body}"),
    );
    let selection = EntrySelection {
        symbol: SymbolId {
            source: source.id.clone(),
            declaration: 0,
        },
        types: vec![],
        arguments: vec![],
    };
    let bundle = DefinitionBundle {
        version: 1,
        sources: vec![source.clone()],
        namespaces: vec![],
    };
    let image = compile_function(&bundle, &source.id, &selection, &Limits::default()).unwrap();
    (bundle, image)
}

fn compare_schedules(body: &str, limits: &ExecutionLimits) -> Result<Evaluation> {
    let (bundle, image) = fixture(body);
    let context = ExecutionContext::new(&bundle);
    let baseline = evaluate(&image, &[], &context, limits);
    let expected = serde_json::to_value(&baseline).unwrap();
    for quantum in [1, 2, 3, 7, 64, 4096] {
        let outcome = evaluate_with_quantum(
            &image,
            &[],
            &context,
            limits,
            NonZeroUsize::new(quantum).unwrap(),
        );
        assert_eq!(
            serde_json::to_value(outcome).unwrap(),
            expected,
            "quantum {quantum}"
        );
    }
    baseline
}

#[test]
fn slices_preserve_escaping_captures_nested_calls_and_structural_branch_results() {
    let value = compare_schedules(
        "fn run()->Int8={let first=make(40);let second=make(10);if second(2)==12 {first(2)} else {1/0}};fn make(x:Int8)->(Int8)->Int8=|y:Int8|add(x,y);fn add(x:Int8,y:Int8)->Int8=x+y;",
        &ExecutionLimits::default(),
    ).unwrap();
    let intersect_model::definitions::rcore_behavior::ir_v2::DataWire::Scalar(value) =
        value.value.data
    else {
        panic!("scalar");
    };
    assert_eq!(value.payload, serde_json::json!(42));
    compare_schedules(
        "fn run()->Result<(Int8,Int8),String> ={let f=make(40);Ok((f(1),f(2)))};fn make(x:Int8)->(Int8)->Int8=|y:Int8|x+y;",
        &ExecutionLimits::default(),
    ).unwrap();
}

#[test]
fn slices_preserve_generic_failure_provenance_and_aggregate_exhaustion_then_recover() {
    let failure = compare_schedules(
        "fn run()->Int8=fail::<Bool>(true);fn fail<T>(x:T)->Int8=1/0;",
        &ExecutionLimits::default(),
    )
    .unwrap_err();
    assert_eq!(failure.kind, ExecutionErrorKind::Arithmetic);
    assert_eq!(failure.trace.len(), 2);
    let body =
        "fn run()->Int8={let f=make(40);f(1)+f(1)};fn make(x:Int8)->(Int8)->Int8=|y:Int8|x+y;";
    let baseline = compare_schedules(body, &ExecutionLimits::default()).unwrap();
    let mut limits = ExecutionLimits::default();
    limits.scalar.max_steps = baseline.consumed.steps;
    compare_schedules(body, &limits).unwrap();
    limits.scalar.max_steps -= 1;
    assert_eq!(
        compare_schedules(body, &limits).unwrap_err().kind,
        ExecutionErrorKind::ResourceExhausted
    );
    assert_eq!(
        serde_json::to_value(compare_schedules(body, &ExecutionLimits::default()).unwrap())
            .unwrap(),
        serde_json::to_value(baseline).unwrap()
    );
}

#[test]
fn suspended_data_has_no_borrowed_authority_or_source_spans() {
    fn owns_all_data<T: 'static>() {}
    owns_all_data::<ExecutionState>();
    owns_all_data::<Frame>();
    // The source-driven schedule controls above recreate the transient view
    // between slices, including pending calls and returns with live captures.
}

#[test]
fn public_sessions_interleave_preparation_and_calls_and_deliver_results_once() {
    let (bundle, image) =
        fixture("fn run()->Int8={let f=make(40);f(2)};fn make(x:Int8)->(Int8)->Int8=|y:Int8|x+y;");
    let context = ExecutionContext::new(&bundle);
    let limits = ExecutionLimits::default();
    let expected = serde_json::to_value(evaluate(&image, &[], &context, &limits).unwrap()).unwrap();
    let mut first = EvaluationSession::new(&image, &[], &context, &limits).unwrap();
    let mut second = EvaluationSession::new(&image, &[], &context, &limits).unwrap();
    assert!(matches!(
        first.resume(NonZeroUsize::MIN),
        SessionProgress::Pending
    ));
    assert!(matches!(
        second.resume(NonZeroUsize::MIN),
        SessionProgress::Pending
    ));
    let mut pending = 0;
    loop {
        match first.resume(NonZeroUsize::MIN) {
            SessionProgress::Pending => pending += 1,
            SessionProgress::Complete(outcome) => {
                assert_eq!(serde_json::to_value((*outcome).unwrap()).unwrap(), expected);
                break;
            }
            SessionProgress::Finished => panic!("lost result"),
        }
    }
    assert!(
        pending > 10,
        "preparation and call execution actually yielded"
    );
    for _ in 0..3 {
        assert!(matches!(
            first.resume(NonZeroUsize::MAX),
            SessionProgress::Finished
        ));
    }
    match second.resume(NonZeroUsize::MAX) {
        SessionProgress::Complete(outcome) => {
            assert_eq!(serde_json::to_value((*outcome).unwrap()).unwrap(), expected)
        }
        _ => panic!("second session must complete independently"),
    }
}

#[test]
fn failed_session_is_terminal_and_new_session_recovers_without_resetting_old_work() {
    let (bundle, image) = fixture("fn run()->Int8=add(1,2);fn add(x:Int8,y:Int8)->Int8=x+y;");
    let context = ExecutionContext::new(&bundle);
    let mut limits = ExecutionLimits::default();
    limits.scalar.max_steps = 1;
    let expected =
        serde_json::to_value(evaluate(&image, &[], &context, &limits).unwrap_err()).unwrap();
    let mut session = EvaluationSession::new(&image, &[], &context, &limits).unwrap();
    loop {
        match session.resume(NonZeroUsize::MIN) {
            SessionProgress::Pending => {}
            SessionProgress::Complete(outcome) => {
                assert_eq!(
                    serde_json::to_value((*outcome).unwrap_err()).unwrap(),
                    expected
                );
                break;
            }
            SessionProgress::Finished => panic!("lost failure"),
        }
    }
    assert!(matches!(
        session.resume(NonZeroUsize::MAX),
        SessionProgress::Finished
    ));
    assert!(evaluate(&image, &[], &context, &ExecutionLimits::default()).is_ok());
}

fn owned_fixture(body: &str) -> (EvaluationSession<'static>, serde_json::Value) {
    let (bundle, image) = fixture(body);
    let limits = ExecutionLimits::default();
    let expected = serde_json::to_value(evaluate(
        &image,
        &[],
        &ExecutionContext::new(&bundle),
        &limits,
    ))
    .unwrap();
    let session =
        EvaluationSession::new_owned(image, vec![], bundle, vec![], None, limits).unwrap();
    (session, expected)
}
#[test]
fn owned_sessions_survive_creating_scopes_and_preserve_values_and_generic_failures() {
    fn host_storable<T: 'static>() {}
    host_storable::<EvaluationSession<'static>>();
    for body in [
        "fn run()->Int8={let f=make(40);f(2)};fn make(x:Int8)->(Int8)->Int8=|y:Int8|x+y;",
        "fn run()->Int8=fail::<Bool>(true);fn fail<T>(x:T)->Int8=1/0;",
    ] {
        let (session, expected) = owned_fixture(body);
        // Moving through a host-like store proves that no field points into the
        // original request's stack, image or provider owner.
        let mut jobs = std::collections::BTreeMap::from([(7, session)]);
        let session = jobs.get_mut(&7).unwrap();
        assert!(matches!(
            session.resume(NonZeroUsize::MIN),
            SessionProgress::Pending
        ));
        loop {
            match session.resume(NonZeroUsize::new(3).unwrap()) {
                SessionProgress::Pending => {}
                SessionProgress::Complete(outcome) => {
                    assert_eq!(serde_json::to_value(*outcome).unwrap(), expected);
                    break;
                }
                SessionProgress::Finished => panic!("lost outcome"),
            }
        }
        assert!(matches!(
            session.resume(NonZeroUsize::MAX),
            SessionProgress::Finished
        ));
        jobs.remove(&7);
        assert!(jobs.is_empty());
    }
}
#[test]
fn owned_session_cancellation_and_dropping_pending_jobs_leave_fresh_jobs_usable() {
    struct Cancel;
    impl ExecutionControl for Cancel {
        fn cancellation_requested(&self) -> bool {
            true
        }
    }
    let body = "fn run()->Int8=42;";
    let (mut cancelled, _) = owned_fixture(body);
    match cancelled.resume_with_control(NonZeroUsize::MAX, &Cancel) {
        SessionProgress::Complete(outcome) => {
            assert_eq!((*outcome).unwrap_err().kind, ExecutionErrorKind::Cancelled)
        }
        _ => panic!("cancellation must finish"),
    }
    assert!(matches!(
        cancelled.resume(NonZeroUsize::MAX),
        SessionProgress::Finished
    ));
    let (mut abandoned, _) = owned_fixture(body);
    assert!(matches!(
        abandoned.resume(NonZeroUsize::MIN),
        SessionProgress::Pending
    ));
    drop(abandoned);
    let (mut fresh, expected) = owned_fixture(body);
    match fresh.resume(NonZeroUsize::MAX) {
        SessionProgress::Complete(outcome) => {
            assert_eq!(serde_json::to_value(*outcome).unwrap(), expected)
        }
        _ => panic!("fresh job must complete"),
    }
}
