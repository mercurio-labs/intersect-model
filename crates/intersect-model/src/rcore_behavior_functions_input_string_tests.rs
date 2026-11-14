use super::*;
use intersect_model::definitions::{
    bundle::{DefinitionSource, SourceKind},
    rcore_behavior::{
        function_lowering::compile_function,
        ir,
        ir_v2::{DataWire, EntrySelection, Limits, TypeId, TypeNode},
        modules::SymbolId,
        types::Scalar,
    },
};

fn compile(body: &str) -> (DefinitionBundle, VerifiedExecutable) {
    let source = DefinitionSource::new(
        "memory:retained-input-string.rcore",
        SourceKind::RcoreBehavior,
        format!("rcore 1;package leafTransport version \"1\";{body}"),
    );
    let bundle = DefinitionBundle {
        version: 1,
        sources: vec![source.clone()],
        namespaces: vec![],
    };
    let image = compile_function(
        &bundle,
        &source.id,
        &EntrySelection {
            symbol: SymbolId {
                source: source.id.clone(),
                declaration: 0,
            },
            types: vec![],
            arguments: vec![],
        },
        &Limits::default(),
    )
    .unwrap();
    (bundle, image)
}
fn string(ty: TypeId, text: &str) -> ValueWire {
    ValueWire {
        ty,
        data: DataWire::Scalar(ir::Value {
            ty: ir::ValueType::Scalar(Scalar::String),
            payload: text.into(),
        }),
    }
}
fn fixture(text: &str) -> (DefinitionBundle, VerifiedExecutable, Vec<ValueWire>) {
    let (bundle, image) = compile("fn echo(value:String)->String=value;");
    let ty = image.program().functions[image.program().entry.0 as usize].parameters[0];
    let args = vec![string(ty, text)];
    (bundle, image, args)
}
fn progress(session: &EvaluationSession<'_>) -> Option<(usize, usize, usize)> {
    session
        .decoder()
        .as_ref()
        .and_then(|d| d.string_copy_progress())
}
fn input<'s>(session: &'s EvaluationSession<'_>) -> &'s str {
    let DataWire::Scalar(v) = &session.arguments()[0].data else {
        panic!("original scalar owner")
    };
    v.payload.as_str().unwrap()
}
fn session<'a>(
    owned: bool,
    bundle: &'a DefinitionBundle,
    image: &'a VerifiedExecutable,
    args: &'a [ValueWire],
    limits: &'a ExecutionLimits,
) -> EvaluationSession<'a> {
    if owned {
        EvaluationSession::new_owned(
            image.clone(),
            args.to_vec(),
            bundle.clone(),
            vec![],
            None,
            limits.clone(),
        )
        .unwrap()
    } else {
        EvaluationSession::new(image, args, &ExecutionContext::new(bundle), limits).unwrap()
    }
}
fn first_chunk(session: &mut EvaluationSession<'_>) {
    for _ in 0..10000 {
        assert!(matches!(
            session.resume(NonZeroUsize::MIN),
            SessionProgress::Pending
        ));
        if progress(session).is_some_and(|(copied, _, _)| copied > 0) {
            return;
        }
    }
    panic!("copy not reached")
}

#[test]
fn retained_input_string_bounds_copy_keeps_original_owner_and_defers_publication() {
    let (frame, cursor, decoder) = crate::rcore_behavior_functions::decoding::Decoding::<
        crate::rcore_behavior_functions::inputs::NoBorrowedModelInputs,
    >::retained_string_layout();
    assert!(frame <= 192 && cursor <= 64);
    eprintln!(
        "retained-input-layout frame={frame} cursor={cursor} decoder={decoder} session={} ticket={}",
        std::mem::size_of::<EvaluationSession<'_>>(),
        std::mem::size_of::<crate::rcore_behavior_functions::values::ScalarAllocation>()
    );
    let unicode = ['"', '\\', '\n', '\t', '\0', '\u{e9}', '\u{1f600}']
        .into_iter()
        .collect::<String>()
        .repeat(513);
    for text in [String::new(), "x".repeat(16), "x".repeat(4097), unicode] {
        let (bundle, image, args) = fixture(&text);
        let limits = ExecutionLimits::default();
        let DataWire::Scalar(value) = &args[0].data else {
            panic!("scalar")
        };
        let admitted_bytes = 576 + serde_json::to_vec(value).unwrap().len();
        let expected = evaluate(&image, &args, &ExecutionContext::new(&bundle), &limits).unwrap();
        for owned in [false, true] {
            let mut s = session(owned, &bundle, &image, &args, &limits);
            let pointer = input(&s).as_ptr();
            let mut previous = 0;
            let mut chunks = 0;
            let mut reserved_checkpoint = false;
            loop {
                assert_eq!(input(&s).as_ptr(), pointer);
                assert_eq!(input(&s), text);
                match s.resume(NonZeroUsize::MIN) {
                    SessionProgress::Pending => {
                        if let Some((copied, capacity, length)) = progress(&s) {
                            assert_eq!(capacity, text.len());
                            assert_eq!(length, text.len());
                            if copied == 0 {
                                assert!(
                                    !reserved_checkpoint,
                                    "one new admitted reserve checkpoint only"
                                );
                                reserved_checkpoint = true;
                                assert_eq!(previous, 0);
                                assert!(s.execution().nodes.is_empty());
                                assert_eq!(s.execution().consumed.value_nodes, 0);
                                assert_eq!(s.execution().consumed.value_bytes, admitted_bytes);
                                continue;
                            }
                            assert!(
                                copied > previous
                                    && copied - previous <= 256
                                    && text.is_char_boundary(copied)
                            );
                            assert!(s.execution().nodes.is_empty());
                            assert_eq!(s.execution().consumed.value_nodes, 0);
                            assert_eq!(s.execution().consumed.value_bytes, admitted_bytes);
                            previous = copied;
                            chunks += 1;
                        }
                    }
                    SessionProgress::Complete(result) => {
                        assert_eq!(
                            serde_json::to_value(result.unwrap()).unwrap(),
                            serde_json::to_value(&expected).unwrap()
                        );
                        break;
                    }
                    SessionProgress::Finished => panic!("missing result"),
                }
            }
            assert!(
                reserved_checkpoint,
                "admit and reserve before first byte copy"
            );
            assert!(chunks >= text.len().div_ceil(256).saturating_sub(1));
            assert!(matches!(
                s.resume(NonZeroUsize::MAX),
                SessionProgress::Finished
            ));
        }
    }
}

#[test]
fn retained_input_string_cancel_drop_at_every_copy_boundary_and_retry() {
    struct Cancel;
    impl ExecutionControl for Cancel {
        fn cancellation_requested(&self) -> bool {
            true
        }
    }
    let (bundle, image, args) = fixture(&"x".repeat(4097));
    let limits = ExecutionLimits::default();
    let baseline = evaluate(&image, &args, &ExecutionContext::new(&bundle), &limits).unwrap();
    for owned in [false, true] {
        for boundary in 0..16 {
            for cancel in [false, true] {
                let mut s = session(owned, &bundle, &image, &args, &limits);
                first_chunk(&mut s);
                for _ in 0..boundary {
                    assert!(matches!(
                        s.resume(NonZeroUsize::MIN),
                        SessionProgress::Pending
                    ));
                }
                assert_eq!(progress(&s).unwrap().0, (boundary + 1) * 256);
                assert!(s.execution().nodes.is_empty());
                if cancel {
                    let SessionProgress::Complete(result) =
                        s.resume_with_control(NonZeroUsize::MIN, &Cancel)
                    else {
                        panic!("cancel")
                    };
                    let err = result.unwrap_err();
                    assert_eq!(err.kind, ExecutionErrorKind::Cancelled);
                    assert_eq!(err.consumed.value_nodes, 0);
                    assert_eq!(err.consumed.value_bytes, 4712);
                    assert!(s.execution().nodes.is_empty());
                    assert!(matches!(
                        s.resume(NonZeroUsize::MAX),
                        SessionProgress::Finished
                    ));
                }
                drop(s);
                assert_eq!(
                    serde_json::to_value(
                        evaluate(&image, &args, &ExecutionContext::new(&bundle), &limits).unwrap()
                    )
                    .unwrap(),
                    serde_json::to_value(&baseline).unwrap()
                );
            }
        }
    }
}

#[test]
fn retained_input_string_original_exact_refusals_precede_any_copy_cursor() {
    let (bundle, image, args) = fixture(&"x".repeat(240000));
    for owned in [false, true] {
        for per_value in [false, true] {
            let mut limits = ExecutionLimits::default();
            if per_value {
                limits.scalar.values.max_value_bytes = 240038;
            } else {
                limits.scalar.max_value_bytes = 240614;
            }
            let mut s = session(owned, &bundle, &image, &args, &limits);
            loop {
                assert!(progress(&s).is_none());
                match s.resume(NonZeroUsize::MIN) {
                    SessionProgress::Pending => {}
                    SessionProgress::Complete(result) => {
                        let err = result.unwrap_err();
                        assert_eq!(
                            &*err.message,
                            if per_value {
                                "Runtime value depth or byte limit exceeded"
                            } else {
                                "Cumulative function value byte budget exceeded"
                            }
                        );
                        assert_eq!(err.consumed.value_nodes, 0);
                        assert_eq!(err.consumed.value_bytes, 576);
                        assert!(s.execution().nodes.is_empty());
                        assert!(progress(&s).is_none());
                        break;
                    }
                    SessionProgress::Finished => panic!("missing error"),
                }
            }
        }
    }
}

#[test]
fn retained_input_string_nested_option_collection_route_stays_with_original_wire() {
    let (bundle, image) = compile(
        "fn echo(value:(Option<String>,Sequence<String>))->(Option<String>,Sequence<String>)=value;",
    );
    let ty = image.program().functions[image.program().entry.0 as usize].parameters[0];
    let TypeNode::Tuple(fields) = image.type_node(ty).unwrap() else {
        panic!("tuple")
    };
    let TypeNode::Option(first) = image.type_node(fields[0]).unwrap() else {
        panic!("option")
    };
    let TypeNode::Collection { element, .. } = image.type_node(fields[1]).unwrap() else {
        panic!("collection")
    };
    let a = "a".repeat(4097);
    let b = "\u{1f600}".repeat(1025);
    let args = vec![ValueWire {
        ty,
        data: DataWire::Tuple(vec![
            ValueWire {
                ty: fields[0],
                data: DataWire::Some(Box::new(string(*first, &a))),
            },
            ValueWire {
                ty: fields[1],
                data: DataWire::Collection(vec![string(*element, &b)]),
            },
        ]),
    }];
    let limits = ExecutionLimits::default();
    let baseline = evaluate(&image, &args, &ExecutionContext::new(&bundle), &limits).unwrap();
    for owned in [false, true] {
        let mut s = session(owned, &bundle, &image, &args, &limits);
        let mut suspended = 0;
        let mut phase_nodes = None;
        loop {
            match s.resume(NonZeroUsize::MIN) {
                SessionProgress::Pending => {
                    if let Some((offset, capacity, length)) = progress(&s) {
                        assert!(offset <= length && offset <= capacity);
                        assert!(capacity == a.len() || capacity == b.len());
                        if offset <= 256 {
                            phase_nodes = Some(s.execution().nodes.len());
                        }
                        assert_eq!(phase_nodes, Some(s.execution().nodes.len()));
                        suspended += 1;
                    }
                }
                SessionProgress::Complete(result) => {
                    assert_eq!(
                        serde_json::to_value(result.unwrap()).unwrap(),
                        serde_json::to_value(&baseline).unwrap()
                    );
                    break;
                }
                SessionProgress::Finished => panic!("missing result"),
            }
        }
        assert!(suspended >= 32);
    }
}

#[test]
fn retained_input_string_nested_record_and_whole_root_invalid_scalar() {
    let (bundle, image) =
        compile("fn echo(value:{a:String,b:Option<String>})->{a:String,b:Option<String>}=value;");
    let ty = image.program().functions[image.program().entry.0 as usize].parameters[0];
    let TypeNode::Record(fields) = image.type_node(ty).unwrap() else {
        panic!("record")
    };
    let TypeNode::Option(b) = image.type_node(fields["b"]).unwrap() else {
        panic!("option")
    };
    let a = "x".repeat(4097);
    let b_text = ['"', '\\', '\n', '\t', '\0', '\u{e9}', '\u{1f600}']
        .into_iter()
        .collect::<String>()
        .repeat(513);
    let mut values = std::collections::BTreeMap::new();
    values.insert("a".into(), string(fields["a"], &a));
    values.insert(
        "b".into(),
        ValueWire {
            ty: fields["b"],
            data: DataWire::Some(Box::new(string(*b, &b_text))),
        },
    );
    let args = vec![ValueWire {
        ty,
        data: DataWire::Record(values),
    }];
    let limits = ExecutionLimits::default();
    let baseline = evaluate(&image, &args, &ExecutionContext::new(&bundle), &limits).unwrap();
    for owned in [false, true] {
        let mut s = session(owned, &bundle, &image, &args, &limits);
        let mut phase_nodes = None;
        let mut chunks = 0;
        loop {
            match s.resume(NonZeroUsize::MIN) {
                SessionProgress::Pending => {
                    if let Some((offset, _, _)) = progress(&s) {
                        if offset <= 256 {
                            phase_nodes = Some(s.execution().nodes.len());
                        }
                        assert_eq!(phase_nodes, Some(s.execution().nodes.len()));
                        chunks += 1;
                    }
                }
                SessionProgress::Complete(result) => {
                    assert_eq!(
                        serde_json::to_value(result.unwrap()).unwrap(),
                        serde_json::to_value(&baseline).unwrap()
                    );
                    break;
                }
                SessionProgress::Finished => panic!("missing result"),
            }
        }
        assert!(chunks > 30);
        let mut invalid = args.clone();
        let DataWire::Record(fields) = &mut invalid[0].data else {
            panic!("record")
        };
        let DataWire::Some(child) = &mut fields.get_mut("b").unwrap().data else {
            panic!("option")
        };
        let DataWire::Scalar(value) = &mut child.data else {
            panic!("scalar")
        };
        value.payload = serde_json::Value::Bool(true);
        let expected =
            evaluate(&image, &invalid, &ExecutionContext::new(&bundle), &limits).unwrap_err();
        let mut s = session(owned, &bundle, &image, &invalid, &limits);
        loop {
            assert!(s.execution().nodes.is_empty());
            assert!(progress(&s).is_none());
            match s.resume(NonZeroUsize::MIN) {
                SessionProgress::Pending => {}
                SessionProgress::Complete(result) => {
                    assert_eq!(
                        serde_json::to_value(result.unwrap_err()).unwrap(),
                        serde_json::to_value(&expected).unwrap()
                    );
                    assert!(s.execution().nodes.is_empty());
                    break;
                }
                SessionProgress::Finished => panic!("missing rejection"),
            }
        }
    }
}

#[test]
fn retained_input_string_ticket_debits_once_and_never_refunds_or_republishes() {
    use crate::rcore_behavior_functions::{
        machine::{ExecutionImage, State},
        values::Data,
    };
    let (bundle, image, args) = fixture(&"x".repeat(4097));
    let limits = ExecutionLimits::default();
    let context = ExecutionContext::new(&bundle);
    let mut s = session(false, &bundle, &image, &args, &limits);
    let ty = args[0].ty;
    let DataWire::Scalar(value) = &args[0].data else {
        panic!("scalar")
    };
    let mut state = State {
        image: ExecutionImage::Functions(&image),
        context: Some(&context),
        limits: &limits,
        control: None,
        data: s.execution_mut(),
    };
    let ticket = state.admit_scalar_allocation(ty, value, None).unwrap();
    assert_eq!(state.data.consumed.value_bytes, 4136);
    assert!(state.data.nodes.is_empty());
    let id = state.publish_scalar(ticket, value.clone(), None).unwrap();
    assert_eq!(id, 0);
    assert_eq!(state.data.consumed.value_nodes, 1);
    assert_eq!(state.data.consumed.value_bytes, 4136); // no publication debit
    {
        let _abandoned = state.admit_scalar_allocation(ty, value, None).unwrap();
        assert_eq!(state.data.consumed.value_bytes, 8272);
    }
    assert_eq!(state.data.consumed.value_bytes, 8272); // cumulative admission is not refunded
    assert_eq!(state.data.consumed.value_nodes, 1);
    let stale = state.admit_scalar_allocation(ty, value, None).unwrap();
    let small = ir::Value {
        ty: value.ty.clone(),
        payload: "z".into(),
    };
    state.alloc(ty, Data::Scalar(small), None).unwrap();
    let prior_bytes = state.data.consumed.value_bytes;
    let prior_nodes = state.data.nodes.len();
    let error = state
        .publish_scalar(stale, value.clone(), None)
        .unwrap_err();
    assert_eq!(error.kind, ExecutionErrorKind::InvalidProgram);
    assert_eq!(
        &*error.message,
        "Scalar allocation changed before publication"
    );
    assert_eq!(state.data.consumed.value_bytes, prior_bytes);
    assert_eq!(state.data.nodes.len(), prior_nodes);
}

#[test]
fn retained_input_string_reserved_zero_boundary_cancels_without_publication_or_refund() {
    struct Cancel;
    impl ExecutionControl for Cancel {
        fn cancellation_requested(&self) -> bool {
            true
        }
    }
    let text = "x".repeat(4097);
    let (bundle, image, args) = fixture(&text);
    let limits = ExecutionLimits::default();
    let baseline = evaluate(&image, &args, &ExecutionContext::new(&bundle), &limits).unwrap();
    for owned in [false, true] {
        for cancel in [false, true] {
            let mut s = session(owned, &bundle, &image, &args, &limits);
            let pointer = input(&s).as_ptr();
            for step in 0..10000 {
                assert!(matches!(
                    s.resume(NonZeroUsize::MIN),
                    SessionProgress::Pending
                ));
                if let Some((copied, capacity, length)) = progress(&s) {
                    assert_eq!((copied, capacity, length), (0, 4097, 4097));
                    break;
                }
                assert!(step < 9999, "reserve checkpoint reached");
            }
            assert_eq!(input(&s).as_ptr(), pointer);
            assert_eq!(input(&s), text);
            assert!(s.execution().nodes.is_empty());
            assert_eq!(s.execution().consumed.value_nodes, 0);
            assert_eq!(s.execution().consumed.value_bytes, 4712);
            if cancel {
                let SessionProgress::Complete(result) =
                    s.resume_with_control(NonZeroUsize::MAX, &Cancel)
                else {
                    panic!("cancel before first copy")
                };
                let failure = result.unwrap_err();
                assert_eq!(failure.kind, ExecutionErrorKind::Cancelled);
                assert_eq!(failure.consumed.value_nodes, 0);
                assert_eq!(failure.consumed.value_bytes, 4712);
                assert!(s.execution().nodes.is_empty());
                assert!(matches!(
                    s.resume(NonZeroUsize::MAX),
                    SessionProgress::Finished
                ));
            }
            drop(s);
            assert_eq!(
                serde_json::to_value(
                    evaluate(&image, &args, &ExecutionContext::new(&bundle), &limits).unwrap()
                )
                .unwrap(),
                serde_json::to_value(&baseline).unwrap()
            );
        }
    }
}
