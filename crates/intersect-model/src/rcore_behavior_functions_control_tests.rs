use super::*;
use intersect_model::definitions::{
    bundle::{DefinitionSource, SourceKind},
    rcore_behavior::{
        function_lowering::compile_function,
        ir,
        ir_v2::{DataWire, EntrySelection, Limits},
        modules::SymbolId,
        types::Scalar,
    },
};
use std::cell::Cell;
struct Counter {
    polls: Cell<usize>,
    stop: usize,
}
impl ExecutionControl for Counter {
    fn cancellation_requested(&self) -> bool {
        let next = self.polls.get() + 1;
        self.polls.set(next);
        next >= self.stop
    }
}
struct Probe<F>(F);
impl<F: Fn() -> bool> ExecutionControl for Probe<F> {
    fn cancellation_requested(&self) -> bool {
        (self.0)()
    }
}
fn fixture(body: &str) -> (DefinitionBundle, VerifiedExecutable) {
    let source = DefinitionSource::new(
        "memory:control-behavior.rcore",
        SourceKind::RcoreBehavior,
        format!("rcore 1;package control version \"1\";{body}"),
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
fn finish(
    session: &mut EvaluationSession<'_>,
    quantum: NonZeroUsize,
    control: &dyn ExecutionControl,
) -> Result<Evaluation> {
    loop {
        match session.resume_with_control(quantum, control) {
            SessionProgress::Pending => {}
            SessionProgress::Complete(outcome) => return *outcome,
            SessionProgress::Finished => panic!("missing outcome"),
        }
    }
}
#[test]
fn every_preparation_call_and_encoding_checkpoint_cancels_with_quantum_independent_evidence() {
    let (bundle, image) = fixture(
        "fn run(x:Int8)->Int8={let f=make(x);f(2)};fn make(x:Int8)->(Int8)->Int8=|y:Int8|x+y;",
    );
    let context = ExecutionContext::new(&bundle);
    let limits = ExecutionLimits::default();
    let args = [ValueWire {
        ty: image.program().functions[image.program().entry.0 as usize].parameters[0],
        data: DataWire::Scalar(ir::Value {
            ty: ir::ValueType::Scalar(Scalar::Int8),
            payload: serde_json::json!(40),
        }),
    }];
    let counter = Counter {
        polls: Cell::new(0),
        stop: usize::MAX,
    };
    let mut session = EvaluationSession::new(&image, &args, &context, &limits).unwrap();
    let expected = finish(&mut session, NonZeroUsize::MAX, &counter).unwrap();
    let checkpoints = counter.polls.get();
    assert!(checkpoints > expected.consumed.steps);
    let mut saw_prepared = false;
    let mut saw_running = false;
    let mut saw_encoding = false;
    for stop in 1..=checkpoints {
        let mut baseline = None;
        for quantum in [1, 3, usize::MAX] {
            let counter = Counter {
                polls: Cell::new(0),
                stop,
            };
            let mut session = EvaluationSession::new(&image, &args, &context, &limits).unwrap();
            let failure =
                finish(&mut session, NonZeroUsize::new(quantum).unwrap(), &counter).unwrap_err();
            assert_eq!(failure.kind, ExecutionErrorKind::Cancelled);
            assert_eq!(counter.polls.get(), stop);
            saw_prepared |= failure.consumed.value_nodes > 0 && failure.consumed.steps == 0;
            saw_running |=
                failure.consumed.steps > 0 && failure.consumed.steps < expected.consumed.steps;
            saw_encoding |= failure.consumed.steps == expected.consumed.steps;
            let wire = serde_json::to_value(failure).unwrap();
            if let Some(baseline) = &baseline {
                assert_eq!(&wire, baseline);
            } else {
                baseline = Some(wire);
            }
            assert!(matches!(
                session.resume_with_control(
                    NonZeroUsize::MAX,
                    &Probe(|| panic!("finished session polled control"))
                ),
                SessionProgress::Finished
            ));
        }
    }
    assert!(saw_prepared && saw_running && saw_encoding);
    assert_eq!(
        serde_json::to_value(evaluate(&image, &args, &context, &limits).unwrap()).unwrap(),
        serde_json::to_value(expected).unwrap()
    );
}
#[test]
fn failure_from_current_unit_wins_over_later_cancellation_and_budget_precedence_is_explicit() {
    let (bundle, image) = fixture("fn run()->Int8=1/0;");
    let context = ExecutionContext::new(&bundle);
    let limits = ExecutionLimits::default();
    let mut session = EvaluationSession::new(&image, &[], &context, &limits).unwrap();
    loop {
        let division = matches!(session.phase(), Phase::Running)
            && session.execution().frames.last().is_some_and(|frame| {
                matches!(
                    image.program().functions[frame.function.0 as usize].blocks[frame.block]
                        .instructions
                        .get(frame.pc)
                        .map(|i| &i.kind),
                    Some(InstructionKind::Binary { .. })
                )
            });
        if division {
            break;
        }
        assert!(matches!(
            session.resume(NonZeroUsize::MIN),
            SessionProgress::Pending
        ));
    }
    let requested = Cell::new(false);
    let control = Probe(|| requested.replace(true));
    let failure = finish(&mut session, NonZeroUsize::MIN, &control).unwrap_err();
    assert!(requested.get());
    // The numeric checkpoint now observes this request before division executes.
    assert_eq!(failure.kind, ExecutionErrorKind::Cancelled);
    assert!(failure.arithmetic_kind.is_none());
    assert!(matches!(
        session.resume_with_control(NonZeroUsize::MAX, &Probe(|| true)),
        SessionProgress::Finished
    ));
    // An actual arithmetic failure still wins over cancellation at a later poll.
    let counter = Counter {
        polls: Cell::new(0),
        stop: usize::MAX,
    };
    let mut baseline = EvaluationSession::new(&image, &[], &context, &limits).unwrap();
    let expected = finish(&mut baseline, NonZeroUsize::MIN, &counter).unwrap_err();
    assert_eq!(expected.kind, ExecutionErrorKind::Arithmetic);
    assert_eq!(
        expected.arithmetic_kind,
        Some(ArithmeticErrorKind::DivisionByZero)
    );
    let after_failure = Counter {
        polls: Cell::new(0),
        stop: counter.polls.get() + 1,
    };
    let mut repeated = EvaluationSession::new(&image, &[], &context, &limits).unwrap();
    let actual = finish(&mut repeated, NonZeroUsize::MIN, &after_failure).unwrap_err();
    assert_eq!(
        serde_json::to_value(actual).unwrap(),
        serde_json::to_value(expected).unwrap()
    );
    assert_eq!(after_failure.polls.get(), counter.polls.get());
    let mut limited = ExecutionLimits::default();
    limited.scalar.max_steps = 0;
    let mut session = EvaluationSession::new(&image, &[], &context, &limited).unwrap();
    let failure = finish(&mut session, NonZeroUsize::MAX, &Probe(|| true)).unwrap_err();
    assert_eq!(failure.kind, ExecutionErrorKind::Cancelled);
    assert_eq!(failure.consumed.steps, 0);
    assert_eq!(failure.consumed.value_nodes, 0);
}
#[test]
fn a_native_controller_thread_cancels_started_work_without_cancelling_an_independent_session() {
    use std::sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc,
    };
    let (bundle, image) =
        fixture("fn run()->Int8={let f=make(40);f(2)};fn make(x:Int8)->(Int8)->Int8=|y:Int8|x+y;");
    let context = ExecutionContext::new(&bundle);
    let limits = ExecutionLimits::default();
    let mut session = EvaluationSession::new(&image, &[], &context, &limits).unwrap();
    let mut independent = EvaluationSession::new(&image, &[], &context, &limits).unwrap();
    assert!(matches!(
        independent.resume(NonZeroUsize::MIN),
        SessionProgress::Pending
    ));
    while session.execution().consumed.steps < 3 {
        assert!(matches!(
            session.resume(NonZeroUsize::MIN),
            SessionProgress::Pending
        ));
    }
    let steps = session.execution().consumed.steps;
    let flag = Arc::new(AtomicBool::new(false));
    let (started_tx, started_rx) = mpsc::channel();
    let (ack_tx, ack_rx) = mpsc::channel();
    std::thread::scope(|scope| {
        let controller_flag = flag.clone();
        scope.spawn(move || {
            started_rx.recv().unwrap();
            controller_flag.store(true, Ordering::Release);
            ack_tx.send(()).unwrap();
        });
        // The handshake is test-only host coordination, never a runtime clock or sleep.
        let control = Probe(|| {
            started_tx.send(()).unwrap();
            ack_rx.recv().unwrap();
            flag.load(Ordering::Acquire)
        });
        let failure = finish(&mut session, NonZeroUsize::MAX, &control).unwrap_err();
        assert_eq!(failure.kind, ExecutionErrorKind::Cancelled);
        assert_eq!(failure.consumed.steps, steps);
    });
    assert!(flag.load(Ordering::Acquire));
    assert!(finish(&mut independent, NonZeroUsize::MAX, &Probe(|| false)).is_ok());
    assert!(evaluate(&image, &[], &context, &limits).is_ok());
}

#[test]
fn preparation_cancellation_attributes_the_next_function_not_the_previous_cursor() {
    let (bundle, image) = fixture("fn run()->Int8=helper();fn helper()->Int8=42;");
    let context = ExecutionContext::new(&bundle);
    let limits = ExecutionLimits::default();
    let mut session = EvaluationSession::new(&image, &[], &context, &limits).unwrap();
    while !matches!(
        session.phase(),
        Phase::Constants {
            function: 1,
            block: 0,
            instruction: 0
        }
    ) {
        assert!(matches!(
            session.resume(NonZeroUsize::MIN),
            SessionProgress::Pending
        ));
    }
    let error = finish(&mut session, NonZeroUsize::MIN, &Probe(|| true)).unwrap_err();
    assert_eq!(error.kind, ExecutionErrorKind::Cancelled);
    assert_eq!(error.trace.len(), 1);
    assert_eq!(
        error.trace[0].function,
        image.program().functions[1].identity
    );
    assert_eq!(error.trace[0].definition, image.program().functions[1].span);
    assert_eq!(
        error.span.as_deref(),
        Some(&image.program().functions[1].blocks[0].instructions[0].span)
    );
    assert_eq!(error.consumed.steps, 0);
}

#[test]
fn nested_decode_and_encode_cancel_after_partial_traversal_without_success() {
    let (bundle, image) = fixture("fn run(x:Sequence<Int8>)->Sequence<Int8> = x;");
    let context = ExecutionContext::new(&bundle);
    let limits = ExecutionLimits::default();
    let ty = image.program().functions[image.program().entry.0 as usize].parameters[0];
    let element = match image.type_node(ty).unwrap() {
        TypeNode::Collection { element, .. } => *element,
        other => panic!("Expected collection, got {other:?}"),
    };
    let args = [ValueWire {
        ty,
        data: DataWire::Collection(
            (0..8)
                .map(|n| ValueWire {
                    ty: element,
                    data: DataWire::Scalar(ir::Value {
                        ty: ir::ValueType::Scalar(Scalar::Int8),
                        payload: serde_json::json!(n),
                    }),
                })
                .collect(),
        ),
    }];
    for encoding in [false, true] {
        let mut outcomes = Vec::new();
        for quantum in [1, 3, usize::MAX] {
            let mut session = EvaluationSession::new(&image, &args, &context, &limits).unwrap();
            while if encoding {
                !matches!(session.phase(), Phase::Encoding(_))
            } else {
                !matches!(session.phase(), Phase::Arguments { index: 0 })
            } {
                assert!(matches!(
                    session.resume(NonZeroUsize::MIN),
                    SessionProgress::Pending
                ));
            }
            let before = session.execution().consumed.value_nodes;
            // Admission now cancels before any runtime value is decoded. Encoding
            // cancels before scheduling its first child, after visiting the root.
            let control = Counter {
                polls: Cell::new(0),
                stop: 4,
            };
            let failure =
                finish(&mut session, NonZeroUsize::new(quantum).unwrap(), &control).unwrap_err();
            assert_eq!(failure.kind, ExecutionErrorKind::Cancelled);
            assert_eq!(control.polls.get(), 4);
            assert_eq!(failure.consumed.value_nodes - before, usize::from(encoding));
            assert!(matches!(
                session.resume(NonZeroUsize::MAX),
                SessionProgress::Finished
            ));
            outcomes.push(serde_json::to_value(failure).unwrap());
        }
        assert_eq!(outcomes[0], outcomes[1]);
        assert_eq!(outcomes[1], outcomes[2]);
    }
    assert_eq!(
        evaluate(&image, &args, &context, &limits).unwrap().value,
        args[0]
    );
}

#[test]
fn native_host_cancels_inside_reference_context_traversal() {
    use std::sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc,
    };
    let (bundle, image) = fixture("fn run()->Int8=42;");
    let references = [intersect_model::definitions::conversion::ReferenceContext {
        bundle: bundle.clone(),
        entry: bundle.sources[0].id.clone(),
        namespace: "unused-snapshot".into(),
        conversions: vec![],
        materialized_types: vec![],
        applied_conversions: vec![],
        constructors: vec![],
        model: serde_json::json!({"nested":[{"value":1},{"value":2}]}),
    }];
    let context = ExecutionContext {
        bundle: &bundle,
        conversions: None,
        references: &references,
    };
    let limits = ExecutionLimits::default();
    let mut session = EvaluationSession::new(&image, &[], &context, &limits).unwrap();
    while !session.context_preparation().is_some_and(|v| v.in_model()) {
        assert!(matches!(
            session.resume(NonZeroUsize::MIN),
            SessionProgress::Pending
        ));
    }
    // Visit the root before arranging cancellation from the external controller.
    assert!(matches!(
        session.resume(NonZeroUsize::MIN),
        SessionProgress::Pending
    ));
    assert_eq!(
        session.context_preparation().and_then(|v| v.model_nodes()),
        Some(1)
    );
    let polls = Cell::new(0);
    let cancelled = Arc::new(AtomicBool::new(false));
    let (started_tx, started_rx) = mpsc::channel();
    let (ack_tx, ack_rx) = mpsc::channel();
    std::thread::scope(|scope| {
        let controller_cancelled = cancelled.clone();
        scope.spawn(move || {
            started_rx.recv().unwrap();
            controller_cancelled.store(true, Ordering::Release);
            ack_tx.send(()).unwrap();
        });
        let control = Probe(|| {
            polls.set(polls.get() + 1);
            // Resume within the model traversal; cancel before its next scheduled child.
            if polls.get() == 4 {
                started_tx.send(()).unwrap();
                ack_rx.recv().unwrap();
            }
            // The flag belongs to the external controller, not semantic state.
            cancelled.load(Ordering::Acquire)
        });
        let failure = finish(&mut session, NonZeroUsize::MAX, &control).unwrap_err();
        assert_eq!(failure.kind, ExecutionErrorKind::Cancelled);
        assert_eq!(polls.get(), 4);
        assert_eq!(failure.consumed.steps, 0);
        assert!(session.execution().reference_inputs.is_empty());
    });
    assert!(evaluate(&image, &[], &context, &limits).is_ok());
}

#[test]
fn cancellation_preserves_active_recursive_frames_and_work() {
    let (bundle, image) = fixture("fn run()->Int8=again();fn again()->Int8=again();");
    let context = ExecutionContext::new(&bundle);
    let limits = ExecutionLimits::default();
    let mut session = EvaluationSession::new(&image, &[], &context, &limits).unwrap();
    while session.execution().frames.len() < 4 {
        assert!(matches!(
            session.resume(NonZeroUsize::MIN),
            SessionProgress::Pending
        ));
    }
    let steps = session.execution().consumed.steps;
    let error = finish(&mut session, NonZeroUsize::MAX, &Probe(|| true)).unwrap_err();
    assert_eq!(error.kind, ExecutionErrorKind::Cancelled);
    assert_eq!(error.consumed.steps, steps);
    assert_eq!(error.trace.len(), 4);
    assert_eq!(error.trace[0].function, error.trace[1].function);
    assert_eq!(error.trace[1].function, error.trace[2].function);
    assert!(matches!(
        session.resume(NonZeroUsize::MAX),
        SessionProgress::Finished
    ));
}

#[test]
fn context_preparation_yields_per_node_and_cancels_at_every_boundary() {
    let (bundle, image) = fixture("fn run()->Int8=42;");
    let references = [intersect_model::definitions::conversion::ReferenceContext {
        bundle: bundle.clone(),
        entry: bundle.sources[0].id.clone(),
        namespace: "unused-snapshot".into(),
        conversions: vec![],
        materialized_types: vec![],
        applied_conversions: vec![],
        constructors: vec![],
        model: serde_json::json!({
            "items": (0..12).map(|n| serde_json::json!({"n":n,"text":"é"})).collect::<Vec<_>>(),
            "empty": [],
            "tail": {"ok":true,"null":null}
        }),
    }];
    let context = ExecutionContext {
        bundle: &bundle,
        conversions: None,
        references: &references,
    };
    let limits = ExecutionLimits::default();
    let mut session = EvaluationSession::new(&image, &[], &context, &limits).unwrap();
    let mut boundaries = 0;
    let mut visited = 0;
    while matches!(session.phase(), Phase::Context) {
        let before = session.context_preparation().and_then(|v| v.model_nodes());
        assert!(matches!(
            session.resume(NonZeroUsize::MIN),
            SessionProgress::Pending
        ));
        let after = session.context_preparation().and_then(|v| v.model_nodes());
        if let (Some(before), Some(after)) = (before, after) {
            assert!(after >= before && after - before <= 1);
            visited = after;
        }
        assert_eq!(session.execution().consumed.steps, 0);
        boundaries += 1;
    }
    assert!(visited >= 40);
    assert!(boundaries > visited * 2);
    assert_eq!(session.execution().reference_inputs.len(), 1);
    let expected = finish(&mut session, NonZeroUsize::MAX, &Probe(|| false)).unwrap();
    assert!(expected.consumed.provider_work > 0);
    assert_eq!(expected.consumed.provider_invocations, 0);
    assert!(expected.consumed_providers.is_empty());
    for quantum in [1, 7, 4096] {
        let mut fresh = EvaluationSession::new(&image, &[], &context, &limits).unwrap();
        assert_eq!(
            serde_json::to_value(
                finish(
                    &mut fresh,
                    NonZeroUsize::new(quantum).unwrap(),
                    &Probe(|| false)
                )
                .unwrap()
            )
            .unwrap(),
            serde_json::to_value(&expected).unwrap()
        );
    }
    for boundary in 0..boundaries {
        let mut fresh = EvaluationSession::new(&image, &[], &context, &limits).unwrap();
        for _ in 0..boundary {
            assert!(matches!(
                fresh.resume(NonZeroUsize::MIN),
                SessionProgress::Pending
            ));
        }
        let mut before = fresh.execution().consumed.clone();
        // Pending observer work lives in the shared receipt sink until result
        // capture. Compare its actual pre-cancellation counters as well.
        fresh.execution().provider_receipts.counters(&mut before);
        let before = serde_json::to_value(before).unwrap();
        let failure = finish(&mut fresh, NonZeroUsize::MIN, &Probe(|| true)).unwrap_err();
        assert_eq!(failure.kind, ExecutionErrorKind::Cancelled);
        assert_eq!(serde_json::to_value(failure.consumed).unwrap(), before);
        assert!(matches!(
            fresh.resume(NonZeroUsize::MAX),
            SessionProgress::Finished
        ));
    }
    let mut exact = limits.clone();
    exact.scalar.max_value_bytes = expected.consumed.value_bytes;
    assert!(evaluate(&image, &[], &context, &exact).is_ok());
    exact.scalar.max_value_bytes -= 1;
    assert_eq!(
        evaluate(&image, &[], &context, &exact).unwrap_err().kind,
        ExecutionErrorKind::ResourceExhausted
    );
    let duplicates = [references[0].clone(), references[0].clone()];
    let duplicate_context = ExecutionContext {
        references: &duplicates,
        ..context
    };
    let failure = evaluate(&image, &[], &duplicate_context, &limits).unwrap_err();
    assert_eq!(failure.kind, ExecutionErrorKind::Nominal);
    assert!(failure.message.contains("Duplicate reference context"));
    let mut foreign = references.clone();
    foreign[0].bundle.sources[0].text.push(' ');
    let foreign_context = ExecutionContext {
        references: &foreign,
        ..context
    };
    let failure = evaluate(&image, &[], &foreign_context, &limits).unwrap_err();
    assert_eq!(failure.kind, ExecutionErrorKind::Nominal);
    assert!(failure.message.contains("foreign source"));
    assert!(evaluate(&image, &[], &context, &limits).is_ok());
}

#[test]
fn invocations_yield_capture_transfers_and_checks_before_entering_the_callee() {
    let locals = (0..16)
        .map(|n| format!("let c{n}:Int32={n};"))
        .collect::<String>();
    let captured = (0..16)
        .map(|n| format!("c{n}"))
        .collect::<Vec<_>>()
        .join("+");
    let parameters = (0..16)
        .map(|n| format!("p{n}:Int32"))
        .collect::<Vec<_>>()
        .join(",");
    let arguments = (0..16).map(|n| n.to_string()).collect::<Vec<_>>().join(",");
    let sum = (0..16)
        .map(|n| format!("p{n}"))
        .collect::<Vec<_>>()
        .join("+");
    let bodies = [
        format!("fn run()->Int32={{{locals}let f=|{parameters}|{sum}+{captured};f({arguments})}};"),
        format!("fn run()->Int32={{{locals}fold([1,2],0,|acc:Int32,x:Int32|acc+x+{captured})}};"),
    ];
    for body in bodies {
        let (bundle, image) = fixture(&body);
        let context = ExecutionContext::new(&bundle);
        let limits = ExecutionLimits::default();
        let mut session = EvaluationSession::new(&image, &[], &context, &limits).unwrap();
        let mut units = 0;
        let mut boundaries = Vec::new();
        let expected = loop {
            let pending = session
                .execution()
                .frames
                .last()
                .and_then(|frame| frame.invocation.as_ref());
            let previous = pending.map(|p| p.progress());
            let site = pending.map(|p| p.site);
            if let Some(site) = site {
                boundaries.push((units, site));
            }
            let depth = session.execution().frames.len();
            match session.resume(NonZeroUsize::MIN) {
                SessionProgress::Pending => {}
                SessionProgress::Complete(result) => break result.unwrap(),
                SessionProgress::Finished => panic!("missing result"),
            }
            if let (Some(before), Some(after)) = (
                previous,
                session
                    .execution()
                    .frames
                    .last()
                    .and_then(|f| f.invocation.as_ref())
                    .map(|p| p.progress()),
            ) {
                assert_eq!(session.execution().frames.len(), depth);
                assert!(after.0 >= before.0 && after.0 - before.0 <= 1);
                assert!(after.1 >= before.1 && after.1 - before.1 <= 1);
            }
            units += 1;
        };
        assert!(boundaries.len() >= 48);
        for quantum in [1, 7, 4096] {
            let mut fresh = EvaluationSession::new(&image, &[], &context, &limits).unwrap();
            let result = finish(
                &mut fresh,
                NonZeroUsize::new(quantum).unwrap(),
                &Probe(|| false),
            )
            .unwrap();
            assert_eq!(
                serde_json::to_value(result).unwrap(),
                serde_json::to_value(&expected).unwrap()
            );
        }
        for (boundary, site) in boundaries {
            let mut fresh = EvaluationSession::new(&image, &[], &context, &limits).unwrap();
            for _ in 0..boundary {
                assert!(matches!(
                    fresh.resume(NonZeroUsize::MIN),
                    SessionProgress::Pending
                ));
            }
            let failure = finish(&mut fresh, NonZeroUsize::MIN, &Probe(|| true)).unwrap_err();
            assert_eq!(failure.kind, ExecutionErrorKind::Cancelled);
            assert_eq!(
                failure.span.as_deref(),
                ExecutionImage::Functions(&image).call_span(site)
            );
            assert!(matches!(
                fresh.resume(NonZeroUsize::MAX),
                SessionProgress::Finished
            ));
        }
        for bytes in [
            expected.consumed.value_bytes,
            expected.consumed.value_bytes - 1,
        ] {
            let mut limited = limits.clone();
            limited.scalar.max_value_bytes = bytes;
            let result = evaluate(&image, &[], &context, &limited);
            if bytes == expected.consumed.value_bytes {
                assert!(result.is_ok());
            } else {
                assert_eq!(
                    result.unwrap_err().kind,
                    ExecutionErrorKind::ResourceExhausted
                );
            }
        }
        for under in [false, true] {
            let mut limited = limits.clone();
            limited.max_captures = expected.consumed.captures - usize::from(under);
            let result = evaluate(&image, &[], &context, &limited);
            if under {
                assert_eq!(
                    result.unwrap_err().kind,
                    ExecutionErrorKind::ResourceExhausted
                );
            } else {
                assert!(result.is_ok());
            }
            let mut limited = limits.clone();
            limited.scalar.max_steps = expected.consumed.steps - usize::from(under);
            let result = evaluate(&image, &[], &context, &limited);
            if under {
                assert_eq!(
                    result.unwrap_err().kind,
                    ExecutionErrorKind::ResourceExhausted
                );
            } else {
                assert!(result.is_ok());
            }
        }
        let mut limited = limits.clone();
        limited.max_frames = 1;
        let failure = evaluate(&image, &[], &context, &limited).unwrap_err();
        assert_eq!(failure.kind, ExecutionErrorKind::ResourceExhausted);
        assert_eq!(&*failure.message, "Function frame depth limit exceeded");
        assert_eq!(failure.consumed.captures, 32);
        assert!(evaluate(&image, &[], &context, &limits).is_ok());
    }
}

#[test]
fn operand_preparation_steps_closures_calls_and_every_selected_edge() {
    let locals = (0..16)
        .map(|n| format!("let c{n}:Int32={n};"))
        .collect::<String>();
    let captured = (0..16)
        .map(|n| format!("c{n}"))
        .collect::<Vec<_>>()
        .join("+");
    let parameters = (0..16)
        .map(|n| format!("p{n}:Int32"))
        .collect::<Vec<_>>()
        .join(",");
    let arguments = (0..16).map(|n| n.to_string()).collect::<Vec<_>>().join(",");
    let sum = (0..16)
        .map(|n| format!("p{n}"))
        .collect::<Vec<_>>()
        .join("+");
    let mut kinds = 0u16;
    for flag in [true, false] {
        let body = format!(
            "fn run()->Int32={{{locals}let flag:Bool={flag};let f=|{parameters}|{sum}+{captured};let maybe:Option<Int32> =if flag {{Some(f({arguments}))}} else {{None}};let result:Result<Int32,Int32> =if flag {{Ok::<Int32,Int32>(42)}} else {{Err::<Int32,Int32>(1)}};let x:Int32=match maybe {{Some(n)=>n,None=>0}};let y:Int32=match result {{Ok(n)=>n,Err(e)=>e}};x+y+{captured}}};"
        );
        let (bundle, image) = fixture(&body);
        let context = ExecutionContext::new(&bundle);
        let limits = ExecutionLimits::default();
        let mut session = EvaluationSession::new(&image, &[], &context, &limits).unwrap();
        let mut units = 0;
        let mut boundaries = Vec::new();
        let expected = loop {
            let frame = session.execution().frames.last();
            let pending = frame.and_then(|frame| frame.operands.as_ref());
            let previous = pending.map(|pending| pending.progress());
            let location = frame.map(|frame| (frame.function, frame.block, frame.pc));
            let bytes = session.execution().consumed.value_bytes;
            if let Some(pending) = pending {
                let source = pending.source;
                let span = source
                    .span(ExecutionImage::Functions(&image))
                    .unwrap()
                    .clone();
                boundaries.push((units, span));
                kinds |= match source {
                    super::super::operands::Source::Instruction(site) => {
                        match image.program().functions[site.function].blocks[site.block]
                            .instructions[site.instruction]
                            .kind
                        {
                            InstructionKind::MakeClosure { .. } => 1,
                            InstructionKind::Call { .. } => 2,
                            _ => panic!("unexpected operand instruction"),
                        }
                    }
                    super::super::operands::Source::Edge {
                        function,
                        block,
                        choice,
                    } => match image.program().functions[function].blocks[block].terminator {
                        Terminator::Jump(_) => 4,
                        Terminator::Branch { .. } => 8 << choice,
                        Terminator::Option { .. } => 32 << choice,
                        Terminator::Result { .. } => 128 << choice,
                        _ => panic!("unexpected operand edge"),
                    },
                };
            }
            match session.resume(NonZeroUsize::MIN) {
                SessionProgress::Pending => {}
                SessionProgress::Complete(result) => break result.unwrap(),
                SessionProgress::Finished => panic!("missing result"),
            }
            if let (Some(before), Some(after)) = (
                previous,
                session
                    .execution()
                    .frames
                    .last()
                    .and_then(|f| f.operands.as_ref())
                    .map(|p| p.progress()),
            ) {
                assert!(after.0 >= before.0 && after.0 - before.0 <= 1);
                assert!(after.1 >= before.1 && after.1 - before.1 <= 1);
                if after.1 == before.1 {
                    assert_eq!(session.execution().consumed.value_bytes, bytes);
                }
                let frame = session.execution().frames.last().unwrap();
                assert_eq!(Some((frame.function, frame.block, frame.pc)), location);
            }
            units += 1;
        };
        assert!(boundaries.len() > 80);
        let DataWire::Scalar(value) = &expected.value.data else {
            panic!("expected scalar");
        };
        assert_eq!(
            value.payload,
            serde_json::json!(if flag { 402 } else { 121 })
        );
        for quantum in [1, 7, 4096] {
            let mut fresh = EvaluationSession::new(&image, &[], &context, &limits).unwrap();
            let result = finish(
                &mut fresh,
                NonZeroUsize::new(quantum).unwrap(),
                &Probe(|| false),
            )
            .unwrap();
            assert_eq!(
                serde_json::to_value(result).unwrap(),
                serde_json::to_value(&expected).unwrap()
            );
        }
        for (boundary, span) in boundaries {
            let mut fresh = EvaluationSession::new(&image, &[], &context, &limits).unwrap();
            for _ in 0..boundary {
                assert!(matches!(
                    fresh.resume(NonZeroUsize::MIN),
                    SessionProgress::Pending
                ));
            }
            let failure = finish(&mut fresh, NonZeroUsize::MIN, &Probe(|| true)).unwrap_err();
            assert_eq!(failure.kind, ExecutionErrorKind::Cancelled);
            assert_eq!(failure.span.as_deref(), Some(&span));
            assert!(matches!(
                fresh.resume(NonZeroUsize::MAX),
                SessionProgress::Finished
            ));
        }
        for under in [false, true] {
            let mut limited = limits.clone();
            limited.scalar.max_value_bytes = expected.consumed.value_bytes - usize::from(under);
            let result = evaluate(&image, &[], &context, &limited);
            if under {
                assert_eq!(
                    result.unwrap_err().kind,
                    ExecutionErrorKind::ResourceExhausted
                );
            } else {
                assert!(result.is_ok());
            }
            let mut limited = limits.clone();
            limited.scalar.max_steps = expected.consumed.steps - usize::from(under);
            let result = evaluate(&image, &[], &context, &limited);
            if under {
                assert_eq!(
                    result.unwrap_err().kind,
                    ExecutionErrorKind::ResourceExhausted
                );
            } else {
                assert!(result.is_ok());
            }
        }
        assert!(evaluate(&image, &[], &context, &limits).is_ok());
    }
    assert_eq!(kinds, 511);
}

#[test]
fn entry_parameter_checks_yield_without_publishing_a_frame_or_charging_execution() {
    let parameters = (0..32)
        .map(|n| format!("p{n}:Int32"))
        .collect::<Vec<_>>()
        .join(",");
    let sum = (0..32)
        .map(|n| format!("p{n}"))
        .collect::<Vec<_>>()
        .join("+");
    let (bundle, image) = fixture(&format!(
        "fn run({parameters})->Int32=if true {{{sum}}} else {{{sum}}};"
    ));
    let context = ExecutionContext::new(&bundle);
    let limits = ExecutionLimits::default();
    let args = image.program().functions[image.program().entry.0 as usize]
        .parameters
        .iter()
        .enumerate()
        .map(|(n, ty)| ValueWire {
            ty: *ty,
            data: DataWire::Scalar(ir::Value {
                ty: ir::ValueType::Scalar(Scalar::Int32),
                payload: serde_json::json!(n),
            }),
        })
        .collect::<Vec<_>>();
    let mut session = EvaluationSession::new(&image, &args, &context, &limits).unwrap();
    while !matches!(session.phase(), Phase::Entry { index: 0 }) {
        assert!(matches!(
            session.resume(NonZeroUsize::MIN),
            SessionProgress::Pending
        ));
    }
    let preparation = serde_json::to_value(&session.execution().consumed).unwrap();
    for index in 0..32 {
        assert!(matches!(session.phase(), Phase::Entry {index: actual} if actual==index));
        assert!(session.execution().frames.is_empty());
        assert_eq!(session.execution().consumed.steps, 0);
        assert!(matches!(
            session.resume(NonZeroUsize::MIN),
            SessionProgress::Pending
        ));
        assert_eq!(
            serde_json::to_value(&session.execution().consumed).unwrap(),
            preparation
        );
    }
    assert!(matches!(session.phase(), Phase::Entry { index: 32 }));
    assert!(session.execution().frames.is_empty());
    assert!(matches!(
        session.resume(NonZeroUsize::MIN),
        SessionProgress::Pending
    ));
    assert!(matches!(session.phase(), Phase::Running));
    assert_eq!(session.execution().frames.len(), 1);
    let expected = finish(&mut session, NonZeroUsize::MAX, &Probe(|| false)).unwrap();
    let DataWire::Scalar(value) = &expected.value.data else {
        panic!("expected scalar");
    };
    assert_eq!(value.payload, serde_json::json!(496));
    for quantum in [1, 7, 4096] {
        let mut fresh = EvaluationSession::new(&image, &args, &context, &limits).unwrap();
        assert_eq!(
            serde_json::to_value(
                finish(
                    &mut fresh,
                    NonZeroUsize::new(quantum).unwrap(),
                    &Probe(|| false)
                )
                .unwrap()
            )
            .unwrap(),
            serde_json::to_value(&expected).unwrap()
        );
    }
    for index in 0..=32 {
        let mut fresh = EvaluationSession::new(&image, &args, &context, &limits).unwrap();
        while !matches!(fresh.phase(),Phase::Entry {index:actual} if actual==index) {
            assert!(matches!(
                fresh.resume(NonZeroUsize::MIN),
                SessionProgress::Pending
            ));
        }
        let failure = finish(&mut fresh, NonZeroUsize::MIN, &Probe(|| true)).unwrap_err();
        assert_eq!(failure.kind, ExecutionErrorKind::Cancelled);
        assert!(failure.span.is_none());
        assert_eq!(serde_json::to_value(failure.consumed).unwrap(), preparation);
        assert!(fresh.execution().frames.is_empty());
        assert!(matches!(
            fresh.resume(NonZeroUsize::MAX),
            SessionProgress::Finished
        ));
    }
    // Internal corruption proves that admission still calls the type authority,
    // even though external argument decoding has already validated normal inputs.
    let mut malformed = EvaluationSession::new(&image, &args, &context, &limits).unwrap();
    while !matches!(malformed.phase(), Phase::Entry { index: 0 }) {
        assert!(matches!(
            malformed.resume(NonZeroUsize::MIN),
            SessionProgress::Pending
        ));
    }
    let boolean = *malformed
        .execution()
        .constants
        .values()
        .find(|id| {
            matches!(&malformed.execution().nodes[**id].data, Data::Scalar(value)
            if value.ty==ir::ValueType::Scalar(Scalar::Bool))
        })
        .unwrap();
    malformed.decoded_arguments_mut()[31] = boolean;
    let failure = finish(&mut malformed, NonZeroUsize::MIN, &Probe(|| false)).unwrap_err();
    assert_eq!(failure.kind, ExecutionErrorKind::Arguments);
    assert!(malformed.execution().frames.is_empty());
    assert_eq!(failure.consumed.steps, 0);
    let mut limited = limits.clone();
    limited.max_frames = 0;
    let failure = evaluate(&image, &args, &context, &limited).unwrap_err();
    assert_eq!(failure.kind, ExecutionErrorKind::ResourceExhausted);
    assert_eq!(&*failure.message, "Function frame depth limit exceeded");
    assert_eq!(serde_json::to_value(failure.consumed).unwrap(), preparation);
    assert!(evaluate(&image, &args, &context, &limits).is_ok());
}

#[test]
fn context_membership_scans_yield_per_candidate_and_reject_foreign_entries() {
    use intersect_model::definitions::bundle::NamespaceBinding;
    let (mut bundle, _) = fixture("fn run()->Int8=42;");
    for index in 0..12 {
        let source = DefinitionSource::new(
            format!("memory:context-{index}.rcore"),
            SourceKind::Rcore,
            format!(
                r#"rcore 1; package p{index} version "1"; metamodel "urn:p{index}" prefix p{index} {{}}"#
            ),
        );
        bundle.namespaces.push(NamespaceBinding {
            namespace: format!("urn:p{index}"),
            source: source.id.clone(),
        });
        bundle.sources.push(source);
    }
    let image = compile_function(
        &bundle,
        &bundle.sources[0].id,
        &EntrySelection {
            symbol: SymbolId {
                source: bundle.sources[0].id.clone(),
                declaration: 0,
            },
            types: vec![],
            arguments: vec![],
        },
        &Limits::default(),
    )
    .unwrap();
    let references = [intersect_model::definitions::conversion::ReferenceContext {
        bundle: DefinitionBundle {
            version: 1,
            sources: vec![bundle.sources[12].clone()],
            namespaces: vec![bundle.namespaces[11].clone()],
        },
        entry: bundle.sources[12].id.clone(),
        namespace: "urn:p11".into(),
        conversions: vec![],
        materialized_types: vec![],
        applied_conversions: vec![],
        constructors: vec![],
        model: serde_json::json!({}),
    }];
    let context = ExecutionContext {
        bundle: &bundle,
        conversions: None,
        references: &references,
    };
    let limits = ExecutionLimits::default();
    let mut session = EvaluationSession::new(&image, &[], &context, &limits).unwrap();
    let mut boundaries = Vec::new();
    let mut source_candidates = Vec::new();
    let mut namespace_candidates = Vec::new();
    let mut units = 0;
    while matches!(session.phase(), Phase::Context) {
        if let Some((namespace, item, candidate)) = session
            .context_preparation()
            .and_then(|v| v.membership_cursor())
        {
            assert!(session.execution().reference_inputs.is_empty());
            boundaries.push(units);
            if item == 0 {
                if namespace {
                    namespace_candidates.push(candidate)
                } else {
                    source_candidates.push(candidate)
                }
            }
        }
        assert!(matches!(
            session.resume(NonZeroUsize::MIN),
            SessionProgress::Pending
        ));
        assert_eq!(session.execution().consumed.steps, 0);
        units += 1;
    }
    assert_eq!(source_candidates, (0..13).collect::<Vec<_>>());
    assert_eq!(namespace_candidates, (0..12).collect::<Vec<_>>());
    let expected = finish(&mut session, NonZeroUsize::MAX, &Probe(|| false)).unwrap();
    for boundary in boundaries {
        let mut fresh = EvaluationSession::new(&image, &[], &context, &limits).unwrap();
        for _ in 0..boundary {
            assert!(matches!(
                fresh.resume(NonZeroUsize::MIN),
                SessionProgress::Pending
            ));
        }
        let failure = finish(&mut fresh, NonZeroUsize::MAX, &Probe(|| true)).unwrap_err();
        assert_eq!(failure.kind, ExecutionErrorKind::Cancelled);
        assert_eq!(failure.consumed.steps, 0);
        assert!(fresh.execution().reference_inputs.is_empty());
        assert!(matches!(
            fresh.resume(NonZeroUsize::MAX),
            SessionProgress::Finished
        ));
    }
    for quantum in [1, 7, 4096] {
        let mut fresh = EvaluationSession::new(&image, &[], &context, &limits).unwrap();
        let actual = finish(
            &mut fresh,
            NonZeroUsize::new(quantum).unwrap(),
            &Probe(|| false),
        )
        .unwrap();
        assert_eq!(
            serde_json::to_value(actual).unwrap(),
            serde_json::to_value(&expected).unwrap()
        );
    }
    for source in [true, false] {
        let mut foreign = references.clone();
        let message = if source {
            foreign[0].bundle.sources[0].text.push(' ');
            "foreign source/dependency identity"
        } else {
            foreign[0].bundle.namespaces[0].namespace = "urn:foreign".into();
            "namespace differs"
        };
        let context = ExecutionContext {
            bundle: &bundle,
            conversions: None,
            references: &foreign,
        };
        let failure = evaluate(&image, &[], &context, &limits).unwrap_err();
        assert_eq!(failure.kind, ExecutionErrorKind::Nominal);
        assert!(failure.message.contains(message), "{failure:?}");
        assert_eq!(failure.consumed.steps, 0);
    }
    assert_eq!(
        serde_json::to_value(evaluate(&image, &[], &context, &limits).unwrap()).unwrap(),
        serde_json::to_value(expected).unwrap()
    );
}

#[test]
fn reference_policies_yield_per_exact_lookup_without_invoking_providers() {
    use intersect_model::definitions::{
        conversion::{ConversionContract, ProviderId, ReferenceContext},
        rcore_behavior::function_lowering::compile_function_with_conversions,
    };
    let (mut bundle, _) = fixture("fn run()->Int8=42;");
    let migration = crate::migration::migrate_ecore(
        DefinitionSource::new(
            "memory:policies.ecore",
            SourceKind::Ecore,
            include_str!("../assets/fixtures/ecore-generics/rcore-behavior-provider-handoff.ecore"),
        ),
        "memory:policies.rcore",
    )
    .unwrap();
    let model = migration.converted.unwrap();
    let contracts: Vec<_> = (0..2)
        .map(|n| ConversionContract {
            datatype: migration
                .correspondence
                .iter()
                .find(|row| row.original.path == vec![n])
                .unwrap()
                .converted
                .clone(),
            provider: ProviderId {
                name: "example.rgb".into(),
                version: "1".into(),
            },
            serializable: true,
            declarations: vec![],
            members: vec![],
        })
        .collect();
    bundle.sources.push(model.clone());
    crate::conversion::with_catalog_policies(&bundle, &contracts, &[], &[], |registry| {
        let entry = &bundle.sources[0].id;
        let selection = EntrySelection {
            symbol: SymbolId {
                source: entry.clone(),
                declaration: 0,
            },
            types: vec![],
            arguments: vec![],
        };
        let image = compile_function_with_conversions(
            &bundle,
            entry,
            &selection,
            registry,
            &Limits::default(),
        )
        .unwrap();
        let refs = [ReferenceContext {
            bundle: bundle.clone(),
            entry: entry.clone(),
            namespace: "unused".into(),
            conversions: (0..24).map(|n| contracts[n % 2].clone()).collect(),
            materialized_types: vec![],
            applied_conversions: vec![],
            constructors: vec![],
            model: serde_json::json!({}),
        }];
        let context = ExecutionContext {
            bundle: &bundle,
            conversions: Some(registry),
            references: &refs,
        };
        let limits = ExecutionLimits::default();
        let mut session = EvaluationSession::new(&image, &[], &context, &limits).unwrap();
        let mut seen = vec![];
        while matches!(session.phase(), Phase::Context) {
            if let Some(cursor) = session
                .context_preparation()
                .and_then(|v| v.policy_cursor())
            {
                seen.push(cursor);
            }
            assert!(matches!(
                session.resume(NonZeroUsize::MIN),
                SessionProgress::Pending
            ));
            assert_eq!(session.execution().consumed.steps, 0);
        }
        assert_eq!(seen, (0..=24).collect::<Vec<_>>());
        let expected = serde_json::to_value(
            finish(&mut session, NonZeroUsize::MAX, &Probe(|| false)).unwrap(),
        )
        .unwrap();
        assert_eq!(
            expected["consumed"]["provider_invocations"],
            serde_json::json!(0)
        );
        assert_eq!(expected["consumed_providers"], serde_json::json!([]));
        for quantum in [1, 7, 4096] {
            let mut session = EvaluationSession::new(&image, &[], &context, &limits).unwrap();
            assert_eq!(
                serde_json::to_value(
                    finish(
                        &mut session,
                        NonZeroUsize::new(quantum).unwrap(),
                        &Probe(|| false)
                    )
                    .unwrap()
                )
                .unwrap(),
                expected
            );
        }
        for stop in 0..=24 {
            let mut session = EvaluationSession::new(&image, &[], &context, &limits).unwrap();
            while session
                .context_preparation()
                .and_then(|v| v.policy_cursor())
                != Some(stop)
            {
                assert!(matches!(
                    session.resume(NonZeroUsize::MIN),
                    SessionProgress::Pending
                ));
            }
            assert!(session.execution().reference_inputs.is_empty());
            let error = finish(&mut session, NonZeroUsize::MIN, &Probe(|| true)).unwrap_err();
            assert_eq!(error.kind, ExecutionErrorKind::Cancelled);
            assert_eq!(error.consumed.steps, 0);
            assert!(matches!(
                session.resume(NonZeroUsize::MAX),
                SessionProgress::Finished
            ));
        }
        let mut bad = refs.clone();
        bad[0].conversions[23].provider.version = "foreign".into();
        let rejected = ExecutionContext {
            references: &bad,
            ..context
        };
        assert_eq!(
            evaluate(&image, &[], &rejected, &limits).unwrap_err().kind,
            ExecutionErrorKind::Nominal
        );
        assert_eq!(
            serde_json::to_value(evaluate(&image, &[], &context, &limits).unwrap()).unwrap(),
            expected
        );
        // Registry metadata and all admitted reference snapshots share one
        // aggregate allowance, even before any nominal value consumes them.
        let mut multiple = vec![refs[0].clone(), refs[0].clone()];
        multiple[0].namespace = "first".into();
        multiple[1].namespace = "second".into();
        for reference in &mut multiple {
            reference.model = serde_json::json!({"padding":"x".repeat(2048)});
        }
        let registry_bytes = registry
            .observation_copy_bytes(
                limits.max_context_bytes,
                limits.max_value_nodes,
                limits.max_value_depth,
            )
            .unwrap();
        let reference_bytes: usize = multiple
            .iter()
            .map(|value| serde_json::to_vec(value).unwrap().len())
            .sum();
        assert!(registry_bytes > 0);
        let combined = ExecutionContext {
            references: &multiple,
            ..context
        };
        let mut exact = limits.clone();
        exact.max_context_bytes = registry_bytes + reference_bytes;
        assert!(evaluate(&image, &[], &combined, &exact).is_ok());
        exact.max_context_bytes -= 1;
        let error = evaluate(&image, &[], &combined, &exact).unwrap_err();
        assert_eq!(error.kind, ExecutionErrorKind::ResourceExhausted);
        assert_eq!(error.consumed.steps, 0);
        assert!(error.message.contains("Reference input byte limit"));
        exact.max_context_bytes += 2;
        assert!(evaluate(&image, &[], &combined, &exact).is_ok());
        exact.max_context_bytes = 0;
        let zero = evaluate(&image, &[], &combined, &exact).unwrap_err();
        assert_eq!(zero.kind, ExecutionErrorKind::ResourceExhausted);
        assert_eq!(zero.consumed.steps, 0);
        assert!(evaluate(&image, &[], &combined, &limits).is_ok());

        Ok(())
    })
    .unwrap();
}

#[test]
fn atomic_context_admission_rejects_width_and_depth_on_native_sized_stack() {
    std::thread::Builder::new()
        .stack_size(1024 * 1024)
        .spawn(|| {
            let (bundle, image) = fixture("fn run()->Int8=42;");
            let run_with_work = |model: serde_json::Value, max_provider_work: usize| {
                let references = [intersect_model::definitions::conversion::ReferenceContext {
                    bundle: bundle.clone(),
                    entry: bundle.sources[0].id.clone(),
                    namespace: "unused".into(),
                    conversions: vec![],
                    materialized_types: vec![],
                    applied_conversions: vec![],
                    constructors: vec![],
                    model,
                }];
                evaluate(
                    &image,
                    &[],
                    &ExecutionContext {
                        bundle: &bundle,
                        conversions: None,
                        references: &references,
                    },
                    &ExecutionLimits {
                        max_provider_work,
                        ..ExecutionLimits::default()
                    },
                )
            };
            let run = |model| run_with_work(model, ExecutionLimits::default().max_provider_work);
            let wide = |count: usize| {
                serde_json::Value::Object(
                    (0..count)
                        .map(|n| (format!("k{n:05}"), serde_json::json!(true)))
                        .collect(),
                )
            };
            for model in [wide(30000), serde_json::json!({"text":"x".repeat(524288)})] {
                assert!(run(model).is_ok());
            }
            // A 1 MiB atom is within metadata/retained-input limits, but its
            // exact JSON bytes plus metadata exceed the default hashing budget.
            let large = serde_json::json!({"text":"x".repeat(1048576)});
            let failure = run(large.clone()).unwrap_err();
            assert_eq!(failure.kind, ExecutionErrorKind::ResourceExhausted);
            assert_eq!(failure.message.as_ref(), "Provider work limit exceeded");
            assert_eq!(failure.consumed.provider_invocations, 0);
            let recovered = run_with_work(large.clone(), 2097152).unwrap();
            let exact = recovered.consumed.provider_work;
            assert!(exact > 1048576);
            assert_eq!(
                serde_json::to_value(run_with_work(large.clone(), exact).unwrap()).unwrap(),
                serde_json::to_value(&recovered).unwrap()
            );
            let bounded = run_with_work(large, exact - 1).unwrap_err();
            assert_eq!(bounded.kind, ExecutionErrorKind::ResourceExhausted);
            assert_eq!(bounded.message.as_ref(), "Provider work limit exceeded");
            assert_eq!(bounded.consumed.provider_invocations, 0);
            let failure = run(wide(33000)).unwrap_err();
            assert_eq!(failure.kind, ExecutionErrorKind::ResourceExhausted);
            assert_eq!(failure.consumed.steps, 0);
            assert!(failure.message.contains("metadata"));
            let nested = |depth: usize| {
                let mut model = serde_json::json!(true);
                for _ in 0..depth {
                    model = serde_json::json!({"x":model});
                }
                model
            };
            assert!(run(nested(60)).is_ok());
            for depth in [64, 96] {
                let failure = run(nested(depth)).unwrap_err();
                assert_eq!(failure.kind, ExecutionErrorKind::ResourceExhausted);
                assert!(failure.message.contains("metadata depth"));
                assert_eq!(failure.consumed.steps, 0);
            }
            assert!(run(serde_json::json!({})).is_ok());
        })
        .unwrap()
        .join()
        .unwrap();
}

#[test]
fn navigation_target_qualification_yields_and_cancels_before_domain_publication() {
    use intersect_model::definitions::conversion::ReferenceContext;
    let ecore = DefinitionSource::new(
        "memory:membership.rcore",
        SourceKind::Rcore,
        r#"rcore 1; package m version "1"; metamodel "urn:membership" prefix m { class A {} class B { ref links: A [0,*] containment; ref one: A [0,1]; ref required: A [1,1]; } }"#,
    );
    let model_bundle = DefinitionBundle {
        version: 1,
        sources: vec![ecore.clone()],
        namespaces: vec![],
    };
    let schema = intersect_model::definitions::compiler::compile_structure_bundle_with_conversions(
        &model_bundle,
        &ecore.id,
        "urn:membership",
        &ConversionRegistry::default(),
    )
    .unwrap();
    let mut model = intersect_model::core::model::Model::default();
    for (id, class) in [("a", "A"), ("b", "B"), ("c", "B")] {
        model.create(&schema, id, class, "r", 0, 0).unwrap();
    }
    model.elements[1]
        .values
        .insert("links".into(), serde_json::json!(["a"]));
    model.elements[1]
        .values
        .insert("one".into(), serde_json::json!("a"));
    for index in [1, 2] {
        model.elements[index]
            .values
            .insert("required".into(), serde_json::json!("a"));
    }
    // This authored snapshot wires containment directly rather than through an
    // edit, so retain only the two owner objects as explicit resource roots.
    let resource = model.resources.get_mut("r").unwrap();
    resource.roots = std::sync::Arc::new(vec!["b".into(), "c".into()]);
    resource.touch().unwrap();
    let reference = ReferenceContext {
        bundle: model_bundle,
        entry: ecore.id.clone(),
        namespace: "urn:membership".into(),
        conversions: vec![],
        materialized_types: vec![],
        applied_conversions: vec![],
        constructors: vec![],
        model: serde_json::to_value(model).unwrap(),
    };
    for (body, id, kind, expected_ids, retargets) in [
        (
            "fn run(x:m.A,y:m.B)->m.B =y;",
            "a",
            "model",
            vec!["b"],
            true,
        ),
        (
            "fn run(x:m.A)->OrderedSet<m.B> =incoming::<m.B>(x,\"links\");",
            "a",
            "collection",
            vec!["b"],
            true,
        ),
        (
            "fn run(x:m.A)->Option<m.B> =container::<m.B>(x);",
            "a",
            "some",
            vec!["b"],
            true,
        ),
        (
            "fn run(x:m.B)->Sequence<m.A> =Sequence(x.links);",
            "b",
            "collection",
            vec!["a"],
            true,
        ),
        (
            "fn run(x:m.B)->Option<m.A> =x.one;",
            "b",
            "some",
            vec!["a"],
            true,
        ),
        (
            "fn run(x:m.B)->m.A =x.required;",
            "b",
            "model",
            vec!["a"],
            true,
        ),
        (
            "fn run(x:m.B)->Sequence<m.A> =Sequence(x.links);",
            "c",
            "collection",
            vec![],
            false,
        ),
        (
            "fn run(x:m.B)->Option<m.A> =x.one;",
            "c",
            "none",
            vec![],
            false,
        ),
    ] {
        let mut source = DefinitionSource::new(
            "memory:membership-behavior.rcore",
            SourceKind::RcoreBehavior,
            format!(
                "rcore 1;package membership version \"1\";import model \"urn:membership\" as m;{body}"
            ),
        );
        source.dependencies.push(ecore.id.clone());
        let bundle = DefinitionBundle {
            version: 1,
            sources: vec![ecore.clone(), source.clone()],
            namespaces: vec![],
        };
        let image = compile_function(
            &bundle,
            &source.id,
            &EntrySelection {
                symbol: SymbolId {
                    source: source.id.clone(),
                    declaration: 1,
                },
                types: vec![],
                arguments: vec![],
            },
            &Limits::default(),
        )
        .unwrap();
        let ty = image.program().functions[image.program().entry.0 as usize].parameters[0];
        let intersect_model::definitions::rcore_behavior::ir_v2::TypeNode::Model(target) =
            &image.program().types[ty.0 as usize]
        else {
            panic!("model")
        };
        let domain = crate::model_reference::ReferenceDomain::new_runtime(
            target.clone(),
            reference.clone(),
            &ConversionRegistry::default(),
        )
        .unwrap();
        let mut arguments = vec![ValueWire {
            ty,
            data: DataWire::Model(domain.bind(id).unwrap()),
        }];
        if let Some(second) = image.program().functions[image.program().entry.0 as usize]
            .parameters
            .get(1)
        {
            arguments.push(ValueWire {ty:*second,data:DataWire::Model(serde_json::json!({"kind":"model_reference","context":reference.digest().unwrap(),"element":"b"}))});
        }
        let references = [reference.clone()];
        let mut context = ExecutionContext::new(&bundle);
        context.references = &references;
        let limits = ExecutionLimits::default();
        let mut session = EvaluationSession::new(&image, &arguments, &context, &limits).unwrap();
        let mut observations = vec![];
        let mut initial_slices = 0;
        let mut slices = 0;
        let expected = loop {
            match session.resume(NonZeroUsize::MIN) {
                SessionProgress::Pending => {
                    slices += 1;
                    if session.execution().domains.is_empty()
                        && session
                            .decoder()
                            .as_ref()
                            .is_some_and(|d| d.preparing_domain())
                    {
                        assert!(session.execution().contexts.is_empty());
                        initial_slices += 1;
                    }
                    if session.execution().domains.len() == 1
                        && (session
                            .decoder()
                            .as_ref()
                            .is_some_and(|d| d.preparing_domain())
                            || session.execution().frames.last().is_some_and(|f| {
                                f.incoming.is_some()
                                    || f.ownership.is_some()
                                    || f.stored_read.as_ref().is_some_and(|r| r.preparing_domain())
                            }))
                    {
                        assert_eq!(session.execution().contexts.len(), 1);
                        observations.push((session.execution().consumed.steps, slices));
                    }
                }
                SessionProgress::Complete(result) => {
                    break result.unwrap();
                }
                SessionProgress::Finished => panic!("missing outcome"),
            }
        };
        assert!(initial_slices > 10, "initial admission must yield: {body}");
        if retargets {
            // Selection and ancestry must yield as well as the three object rows.
            assert!(observations.len() > 3, "qualification must yield: {body}");
        } else {
            assert!(observations.is_empty(), "absent values remain lazy: {body}");
        }
        assert!(
            observations
                .windows(2)
                .all(|pair| pair[1].0 == pair[0].0 + 1)
        );
        assert_eq!(
            session.execution().domains.len(),
            if retargets { 2 } else { 1 }
        );
        assert_eq!(expected.consumed.provider_invocations, 0);
        let values = match (&expected.value.data, kind) {
            (DataWire::Model(_), "model") => vec![&expected.value],
            (DataWire::Some(value), "some") => vec![value.as_ref()],
            (DataWire::Collection(values), "collection") => values.iter().collect(),
            (DataWire::None, "none") => vec![],
            _ => panic!("unexpected result shape: {body}"),
        };
        let digest = reference.digest().unwrap();
        let actual_ids = values
            .iter()
            .map(|value| {
                let DataWire::Model(reference) = &value.data else {
                    panic!("expected a model reference: {body}")
                };
                assert_eq!(reference["kind"], "model_reference");
                assert_eq!(reference["context"], digest);
                reference["element"].as_str().unwrap()
            })
            .collect::<Vec<_>>();
        assert_eq!(actual_ids, expected_ids, "{body}");
        // Every pending retarget boundary must leave the admitted context intact
        // without publishing a second, partially qualified domain.
        for (_, boundary) in observations {
            let mut fresh = EvaluationSession::new(&image, &arguments, &context, &limits).unwrap();
            for _ in 0..boundary {
                assert!(matches!(
                    fresh.resume(NonZeroUsize::MIN),
                    SessionProgress::Pending
                ));
            }
            assert_eq!(fresh.execution().domains.len(), 1);
            assert_eq!(fresh.execution().contexts.len(), 1);
            let failure = finish(&mut fresh, NonZeroUsize::MAX, &Probe(|| true)).unwrap_err();
            assert_eq!(failure.kind, ExecutionErrorKind::Cancelled);
            assert_eq!(failure.consumed.provider_invocations, 0);
            assert_eq!(fresh.execution().domains.len(), 1);
            assert_eq!(fresh.execution().contexts.len(), 1);
            assert!(matches!(
                fresh.resume(NonZeroUsize::MAX),
                SessionProgress::Finished
            ));
        }
        for quantum in [7, 4096] {
            let mut fresh = EvaluationSession::new(&image, &arguments, &context, &limits).unwrap();
            let actual = finish(
                &mut fresh,
                NonZeroUsize::new(quantum).unwrap(),
                &Probe(|| false),
            )
            .unwrap();
            assert_eq!(
                serde_json::to_value(actual).unwrap(),
                serde_json::to_value(&expected).unwrap()
            );
        }
    }
}
