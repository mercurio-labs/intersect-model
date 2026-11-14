//! Independent boundary consumers across actual source calls and closure frames.
use intersect_model::definitions::{
    bundle::{DefinitionBundle, DefinitionSource, SourceKind},
    rcore_behavior::{
        self,
        function_lowering::compile_function,
        ir,
        ir_v2::{
            DataWire, EntrySelection, FunctionIdentity, Limits, TypeId, TypeNode, ValueWire,
            VerifiedExecutable,
        },
        modules::SymbolId,
        types::Scalar,
    },
};
use intersect_model::rcore_behavior_functions::{
    ExecutionContext, ExecutionErrorKind, ExecutionLimits, evaluate,
};

fn fixture(body: &str) -> (DefinitionBundle, DefinitionSource) {
    let source = DefinitionSource::new(
        "memory:boundaries-behavior.rcore",
        SourceKind::RcoreBehavior,
        format!("rcore 1;package boundaries version \"1\";{body}"),
    );
    (
        DefinitionBundle {
            version: 1,
            sources: vec![source.clone()],
            namespaces: vec![],
        },
        source,
    )
}
fn selection(source: &DefinitionSource) -> EntrySelection {
    let document = rcore_behavior::parse(source.clone());
    let declaration = document
        .declarations
        .iter()
        .position(|d| matches!(d.value, rcore_behavior::ast::Declaration::Function { .. }))
        .unwrap();
    EntrySelection {
        symbol: SymbolId {
            source: source.id.clone(),
            declaration,
        },
        types: vec![],
        arguments: vec![],
    }
}
fn compile(bundle: &DefinitionBundle, source: &DefinitionSource) -> VerifiedExecutable {
    compile_function(bundle, &source.id, &selection(source), &Limits::default()).unwrap()
}
fn scalar(ty: TypeId, scalar: Scalar, payload: serde_json::Value) -> ValueWire {
    ValueWire {
        ty,
        data: DataWire::Scalar(ir::Value {
            ty: ir::ValueType::Scalar(scalar),
            payload,
        }),
    }
}
fn number(value: &ValueWire) -> i64 {
    let DataWire::Scalar(value) = &value.data else {
        panic!("scalar")
    };
    value.payload.as_i64().unwrap()
}
#[test]
fn generic_failure_trace_keeps_actuals_and_exact_frame_caps() {
    let (bundle, source) = fixture(
        "fn run(flag:Bool)->Int8=if flag {relay::<Bool>(true,0)} else {relay::<Int8>(1,0)};fn relay<T>(value:T,zero:Int8)->Int8=fail::<T>(value,zero);fn fail<T>(value:T,zero:Int8)->Int8=1/zero;",
    );
    let image = compile(&bundle, &source);
    // Three declarations yield five concrete named instances, including both
    // actuals of relay and fail; finite specialization has an exact boundary.
    assert_eq!(image.program().functions.len(), 5);
    assert!(
        compile_function(
            &bundle,
            &source.id,
            &selection(&source),
            &Limits {
                max_functions: 5,
                ..Default::default()
            }
        )
        .is_ok()
    );
    let errors = compile_function(
        &bundle,
        &source.id,
        &selection(&source),
        &Limits {
            max_functions: 4,
            ..Default::default()
        },
    )
    .unwrap_err();
    assert!(
        errors
            .iter()
            .any(|e| e.message.contains("Function instance limit")),
        "{errors:?}"
    );
    let parameter = image.program().functions[image.program().entry.0 as usize].parameters[0];
    let context = ExecutionContext::new(&bundle);
    for (flag, actual) in [(true, Scalar::Bool), (false, Scalar::Int8)] {
        let argument = scalar(parameter, Scalar::Bool, serde_json::json!(flag));
        let full = evaluate(
            &image,
            std::slice::from_ref(&argument),
            &context,
            &ExecutionLimits::default(),
        )
        .unwrap_err();
        assert_eq!(full.kind, ExecutionErrorKind::Arithmetic);
        assert_eq!(full.trace.len(), 3);
        assert!(!full.trace_truncated);
        let span = full.span.as_ref().unwrap();
        assert_eq!(span.source, source.id);
        assert_eq!(&source.text[span.start..span.end], "/");
        for (frame, declaration) in full.trace.iter().zip([2, 1, 0]) {
            let FunctionIdentity::Named {
                symbol,
                type_arguments,
            } = &frame.function
            else {
                panic!("named frame")
            };
            assert_eq!(symbol.source, source.id);
            assert_eq!(symbol.declaration, declaration);
            assert_eq!(frame.definition.source, source.id);
            if declaration == 0 {
                assert!(type_arguments.is_empty());
            } else {
                assert_eq!(type_arguments.len(), 1);
                assert_eq!(
                    image.type_node(type_arguments[0]).unwrap(),
                    &TypeNode::Scalar(actual)
                );
            }
        }
        for cap in [0, 1, 3] {
            let error = evaluate(
                &image,
                std::slice::from_ref(&argument),
                &context,
                &ExecutionLimits {
                    max_trace_frames: cap,
                    ..Default::default()
                },
            )
            .unwrap_err();
            assert_eq!(error.kind, full.kind);
            assert_eq!(error.message, full.message);
            assert_eq!(error.span, full.span);
            assert_eq!(error.trace.len(), cap);
            assert_eq!(error.trace_truncated, cap < 3);
            assert_eq!(
                serde_json::to_value(&error.trace).unwrap(),
                serde_json::to_value(&full.trace[..cap]).unwrap()
            );
            assert_eq!(
                serde_json::to_value(&error.consumed).unwrap(),
                serde_json::to_value(&full.consumed).unwrap()
            );
        }
    }
}
#[test]
fn same_lambda_code_reentry_preserves_distinct_capture_values() {
    let (bundle, source) = fixture(
        "fn run(n:Int8)->Int8=make(n)(42);fn make(n:Int8)->(Int8)->Int8=|x:Int8|if n==0 {x} else {make(n-1)(x)};",
    );
    let image = compile(&bundle, &source);
    let lambdas: Vec<_> = image
        .program()
        .functions
        .iter()
        .filter(|f| matches!(f.identity, FunctionIdentity::Lambda { .. }))
        .collect();
    assert_eq!(lambdas.len(), 1);
    let parameter = image.program().functions[image.program().entry.0 as usize].parameters[0];
    let context = ExecutionContext::new(&bundle);
    let result = evaluate(
        &image,
        &[scalar(parameter, Scalar::Int8, serde_json::json!(5))],
        &context,
        &ExecutionLimits::default(),
    )
    .unwrap();
    assert_eq!(number(&result.value), 42);
    assert!(result.consumed.closures >= 6);
    let recovered = evaluate(
        &image,
        &[scalar(parameter, Scalar::Int8, serde_json::json!(0))],
        &context,
        &ExecutionLimits::default(),
    )
    .unwrap();
    assert_eq!(number(&recovered.value), 42);
}
#[test]
fn arithmetic_steps_and_transfer_budgets_are_aggregate_across_calls() {
    let (bundle, source) =
        fixture("fn run()->Int8={let a=add(10,20);add(a,12)};fn add(a:Int8,b:Int8)->Int8=a+b;");
    let image = compile(&bundle, &source);
    let context = ExecutionContext::new(&bundle);
    let full = evaluate(&image, &[], &context, &ExecutionLimits::default()).unwrap();
    assert_eq!(number(&full.value), 42);
    let (one_bundle, one_source) =
        fixture("fn run()->Int8=add(10,20);fn add(a:Int8,b:Int8)->Int8=a+b;");
    let one = evaluate(
        &compile(&one_bundle, &one_source),
        &[],
        &ExecutionContext::new(&one_bundle),
        &ExecutionLimits::default(),
    )
    .unwrap();
    assert!(full.consumed.numeric_work > one.consumed.numeric_work);
    assert!(full.consumed.steps > one.consumed.steps);
    assert!(full.consumed.value_bytes > one.consumed.value_bytes);
    for category in 0..3 {
        let mut exact = ExecutionLimits::default();
        match category {
            0 => exact.scalar.numeric.max_work = full.consumed.numeric_work,
            1 => exact.scalar.max_steps = full.consumed.steps,
            _ => exact.scalar.max_value_bytes = full.consumed.value_bytes,
        }
        let at = evaluate(&image, &[], &context, &exact).unwrap();
        assert_eq!(at.value, full.value);
        assert_eq!(
            serde_json::to_value(at.consumed).unwrap(),
            serde_json::to_value(&full.consumed).unwrap()
        );
        match category {
            0 => exact.scalar.numeric.max_work -= 1,
            1 => exact.scalar.max_steps -= 1,
            _ => exact.scalar.max_value_bytes -= 1,
        }
        let error = evaluate(&image, &[], &context, &exact).unwrap_err();
        assert_eq!(
            error.kind,
            ExecutionErrorKind::ResourceExhausted,
            "{error:?}"
        );
        assert!(
            error.consumed.steps > 0,
            "boundary must reach actual execution"
        );
        assert_eq!(
            number(
                &evaluate(&image, &[], &context, &ExecutionLimits::default())
                    .unwrap()
                    .value
            ),
            42
        );
    }
}
#[test]
fn structural_depth_and_comparison_limits_span_function_boundaries() {
    let ty = format!("{}Int8{}", "Option<".repeat(12), ">".repeat(12));
    let value = format!("{}x{}", "Some(".repeat(12), ")".repeat(12));
    let (bundle, source) = fixture(&format!(
        "fn run()->{ty} =identity(wrap(42));fn wrap(x:Int8)->{ty} ={value};fn identity<T>(x:T)->T=x;"
    ));
    let image = compile(&bundle, &source);
    let context = ExecutionContext::new(&bundle);
    let at = evaluate(
        &image,
        &[],
        &context,
        &ExecutionLimits {
            max_value_depth: 13,
            ..Default::default()
        },
    )
    .unwrap();
    let mut leaf = &at.value;
    for _ in 0..12 {
        let DataWire::Some(inner) = &leaf.data else {
            panic!("Some")
        };
        leaf = inner;
    }
    assert_eq!(number(leaf), 42);
    let error = evaluate(
        &image,
        &[],
        &context,
        &ExecutionLimits {
            max_value_depth: 12,
            ..Default::default()
        },
    )
    .unwrap_err();
    assert_eq!(error.kind, ExecutionErrorKind::ResourceExhausted);
    assert!(error.consumed.steps > 0, "{error:?}");
    assert!(error.message.contains("depth"), "{error:?}");
    assert!(evaluate(&image, &[], &context, &ExecutionLimits::default()).is_ok());

    let (bundle, source) = fixture(
        "fn run()->(OrderedSet<Int8>,OrderedSet<Int8>)=(unique([1,1]),unique([2,2]));fn unique(xs:Sequence<Int8>)->OrderedSet<Int8> =OrderedSet::<Int8>(xs);",
    );
    let image = compile(&bundle, &source);
    let context = ExecutionContext::new(&bundle);
    let full = evaluate(&image, &[], &context, &ExecutionLimits::default()).unwrap();
    assert_eq!(full.consumed.collection_comparisons, 2);
    let DataWire::Tuple(items) = &full.value.data else {
        panic!("tuple")
    };
    for (item, expected) in items.iter().zip([1, 2]) {
        let DataWire::Collection(items) = &item.data else {
            panic!("ordered set")
        };
        assert_eq!(items.len(), 1);
        assert_eq!(number(&items[0]), expected);
    }
    let exact = ExecutionLimits {
        max_collection_comparisons: 2,
        ..Default::default()
    };
    assert_eq!(
        evaluate(&image, &[], &context, &exact).unwrap().value,
        full.value
    );
    let error = evaluate(
        &image,
        &[],
        &context,
        &ExecutionLimits {
            max_collection_comparisons: 1,
            ..Default::default()
        },
    )
    .unwrap_err();
    assert_eq!(error.kind, ExecutionErrorKind::ResourceExhausted);
    assert!(error.message.contains("comparison"), "{error:?}");
    assert_eq!(
        evaluate(&image, &[], &context, &exact).unwrap().value,
        full.value
    );
}
#[test]
#[cfg_attr(not(feature = "milestone-tests"), ignore = "milestone")]
fn retained_model_context_bytes_are_shared_across_exact_target_domains() {
    use intersect_model::core::model::Model;
    use intersect_model::definitions::{
        compiler::materialize_runtime_bundle,
        conversion::{ConversionRegistry, ReferenceContext},
        generic_types::{RuntimeType, Type},
        resources::DeclarationId,
    };
    use intersect_model::model_reference::ReferenceDomain;
    let model_source = intersect_model_acceptance::migration_fixture::migrate_source_fixture(DefinitionSource::new(
        "memory:bytes.ecore",
        SourceKind::Ecore,
        r##"<e:EPackage xmlns:e="http://www.eclipse.org/emf/2002/Ecore" xmlns:x="http://www.w3.org/2001/XMLSchema-instance" name="bytes" nsURI="urn:bytes"><eClassifiers x:type="e:EClass" name="Base"><eStructuralFeatures x:type="e:EAttribute" name="note" eType="e:EDataType http://www.eclipse.org/emf/2002/Ecore#//EString"/></eClassifiers><eClassifiers x:type="e:EClass" name="Child" eSuperTypes="#//Base"/></e:EPackage>"##,
    )).0;
    let runtime = |index| {
        RuntimeType::new(Type::Classifier {
            declaration: DeclarationId {
                source: model_source.id.clone(),
                path: vec![index],
            },
            arguments: vec![],
            raw: false,
        })
        .unwrap()
    };
    let targets = vec![runtime(0), runtime(1)];
    let model_bundle = DefinitionBundle {
        version: 1,
        sources: vec![model_source.clone()],
        namespaces: vec![],
    };
    let schema =
        materialize_runtime_bundle(&model_bundle, &model_source.id, "urn:bytes", &targets, None)
            .unwrap();
    let mut model = Model::default();
    model
        .create(
            &schema,
            "object",
            schema.class_key_for_type(&targets[1]).unwrap(),
            "memory:objects",
            0,
            1,
        )
        .unwrap();
    model
        .edit(
            &schema,
            &[(
                "object".into(),
                "note".into(),
                serde_json::json!("padding".repeat(4096)),
            )],
        )
        .unwrap();
    let reference = ReferenceContext {
        bundle: model_bundle,
        entry: model_source.id.clone(),
        namespace: "urn:bytes".into(),
        conversions: vec![],
        materialized_types: targets.clone(),
        applied_conversions: vec![],
        constructors: vec![],
        model: serde_json::to_value(model).unwrap(),
    };
    let bytes = serde_json::to_vec(&reference).unwrap().len();
    let registry = ConversionRegistry::default();
    let payload = ReferenceDomain::new_runtime(targets[0].clone(), reference.clone(), &registry)
        .unwrap()
        .bind("object")
        .unwrap();
    let mut source = DefinitionSource::new(
        "memory:context-bytes-behavior.rcore",
        SourceKind::RcoreBehavior,
        "rcore 1;package context_bytes version \"1\";import model \"urn:bytes\" as m;fn run(a:m.Base,b:m.Child)->m.Base=relay(a);fn relay(x:m.Base)->m.Base=x;",
    );
    source.dependencies.push(model_source.id.clone());
    let bundle = DefinitionBundle {
        version: 1,
        sources: vec![model_source, source.clone()],
        namespaces: vec![],
    };
    let image = compile(&bundle, &source);
    let arguments = image.program().functions[image.program().entry.0 as usize]
        .parameters
        .iter()
        .map(|ty| ValueWire {
            ty: *ty,
            data: DataWire::Model(payload.clone()),
        })
        .collect::<Vec<_>>();
    let references = [reference];
    let context = ExecutionContext {
        bundle: &bundle,
        conversions: Some(&registry),
        references: &references,
    };
    let exact = ExecutionLimits {
        // Metadata admission has its own byte measure. This ceiling
        // rejected the former two retained copies of the same context.
        max_context_bytes: bytes * 2 - 1,
        ..Default::default()
    };
    let result = evaluate(&image, &arguments, &context, &exact).unwrap();
    assert_eq!(result.consumed.reference_domains, 2);
    assert_eq!(result.consumed.model_contexts, 1);
    assert_eq!(result.consumed.model_elements, 1);
    assert_eq!(result.consumed.membership_checks, 2);
    assert_eq!(result.consumed.context_bytes, bytes);
    assert_eq!(result.value, arguments[0]);
    // Model indexing/validation and membership share the callable work ceiling,
    // even when no custom provider is needed by the model.
    use intersect_model::rcore_behavior_functions::{
        EvaluationSession, ExecutionControl, SessionProgress,
    };
    use std::{cell::Cell, num::NonZeroUsize};
    struct Probe {
        polls: Cell<usize>,
        stop: usize,
    }
    impl ExecutionControl for Probe {
        fn cancellation_requested(&self) -> bool {
            self.polls.set(self.polls.get() + 1);
            self.polls.get() == self.stop
        }
    }
    let run = |limits: &ExecutionLimits, stop: usize, quantum: usize| {
        let mut session = EvaluationSession::new(&image, &arguments, &context, limits).unwrap();
        let control = Probe {
            polls: Cell::new(0),
            stop,
        };
        let outcome = loop {
            match session.resume_with_control(NonZeroUsize::new(quantum).unwrap(), &control) {
                SessionProgress::Pending => {}
                SessionProgress::Complete(result) => break *result,
                SessionProgress::Finished => panic!("missing terminal outcome"),
            }
        };
        assert!(matches!(
            session.resume(NonZeroUsize::MIN),
            SessionProgress::Finished
        ));
        (outcome, control.polls.get())
    };
    let needed = result.consumed.provider_work;
    assert!(needed > 30);
    assert_eq!(result.consumed.provider_invocations, 0);
    for ceiling in [0, needed - 1, needed, needed + 1] {
        let limits = ExecutionLimits {
            max_provider_work: ceiling,
            ..exact.clone()
        };
        let mut expected = None;
        for quantum in [1, 7] {
            let (outcome, _) = run(&limits, usize::MAX, quantum);
            let wire = if ceiling < needed {
                let error = outcome.unwrap_err();
                assert_eq!(error.kind, ExecutionErrorKind::ResourceExhausted);
                assert_eq!(error.consumed.provider_work, ceiling);
                assert_eq!(error.consumed.provider_invocations, 0);
                if ceiling == 0 {
                    assert_eq!(error.consumed.reference_domains, 0);
                }
                serde_json::to_value(error).unwrap()
            } else {
                let wire = serde_json::to_value(outcome.unwrap()).unwrap();
                assert_eq!(wire, serde_json::to_value(&result).unwrap());
                wire
            };
            if let Some(expected) = &expected {
                assert_eq!(&wire, expected);
            } else {
                expected = Some(wire);
            }
        }
    }
    let (_, polls) = run(&exact, usize::MAX, 1);
    let mut interrupted_domain = 0;
    for stop in 1..=polls {
        let mut expected = None;
        for quantum in [1, 7] {
            let (outcome, seen) = run(&exact, stop, quantum);
            let error = outcome.unwrap_err();
            assert_eq!(seen, stop);
            assert_eq!(error.kind, ExecutionErrorKind::Cancelled);
            if error.consumed.provider_work > 0 && error.consumed.reference_domains == 0 {
                interrupted_domain += 1;
            }
            let wire = serde_json::to_value(error).unwrap();
            if let Some(expected) = &expected {
                assert_eq!(&wire, expected);
            } else {
                expected = Some(wire);
            }
        }
    }
    assert!(
        interrupted_domain > 20,
        "must cancel inside construction before any domain is cached"
    );
    assert_eq!(
        serde_json::to_value(evaluate(&image, &arguments, &context, &exact).unwrap()).unwrap(),
        serde_json::to_value(&result).unwrap()
    );
    let error = evaluate(
        &image,
        &arguments,
        &context,
        &ExecutionLimits {
            max_context_bytes: bytes - 1,
            ..Default::default()
        },
    )
    .unwrap_err();
    assert_eq!(error.kind, ExecutionErrorKind::ResourceExhausted);
    assert!(error.message.contains("byte"));
    assert_eq!(error.consumed.reference_domains, 0);
    assert_eq!(error.consumed.context_bytes, 0);
    assert_eq!(
        evaluate(&image, &arguments, &context, &exact)
            .unwrap()
            .value,
        result.value
    );
}

#[test]
fn nonzero_frame_closure_and_value_node_limits_are_exact_across_calls() {
    let calls = (1..=8)
        .map(|n| format!("apply(f,{n})"))
        .collect::<Vec<_>>()
        .join(",");
    let (bundle, source) = fixture(&format!(
        "fn run()->Sequence<Int8> ={{let f=make(40);[{calls}]}};fn make(base:Int8)->(Int8)->Int8=|x:Int8|base+x;fn apply(f:(Int8)->Int8,x:Int8)->Int8=f(x);"
    ));
    let image = compile(&bundle, &source);
    let context = ExecutionContext::new(&bundle);
    let full = evaluate(&image, &[], &context, &ExecutionLimits::default()).unwrap();
    let DataWire::Collection(items) = &full.value.data else {
        panic!("sequence")
    };
    assert_eq!(
        items.iter().map(number).collect::<Vec<_>>(),
        (41..=48).collect::<Vec<_>>()
    );
    assert!(full.consumed.closures > 1);
    assert!(
        full.consumed.value_nodes > 20,
        "enough runtime values to exceed small context preflight"
    );
    // One captured slot at creation, then one transfer for each of eight calls.
    assert_eq!(full.consumed.captures, 9);
    for category in 0..4 {
        let needed = match category {
            0 => 3, // run -> apply -> captured lambda; make has returned.
            1 => full.consumed.closures,
            2 => full.consumed.value_nodes,
            _ => full.consumed.captures,
        };
        for ceiling in [0, needed - 1, needed, needed + 1] {
            let mut limits = ExecutionLimits::default();
            match category {
                0 => limits.max_frames = ceiling,
                1 => limits.max_closures = ceiling,
                2 => limits.max_value_nodes = ceiling,
                _ => limits.max_captures = ceiling,
            }
            let result = evaluate(&image, &[], &context, &limits);
            if ceiling >= needed {
                assert_eq!(
                    serde_json::to_value(result.unwrap()).unwrap(),
                    serde_json::to_value(&full).unwrap(),
                    "category {category}, ceiling {ceiling}",
                );
            } else {
                let error = result.unwrap_err();
                assert_eq!(error.kind, ExecutionErrorKind::ResourceExhausted);
                if ceiling > 0 {
                    assert!(error.consumed.steps > 0, "must reach execution: {error:?}");
                }
                let charged = match category {
                    0 => None, // frames are a live ceiling, not cumulative consumption.
                    1 => Some(error.consumed.closures),
                    2 => Some(error.consumed.value_nodes),
                    _ => Some(error.consumed.captures),
                };
                assert!(charged.is_none_or(|charged| charged <= ceiling));
                assert_eq!(
                    serde_json::to_value(
                        evaluate(&image, &[], &context, &ExecutionLimits::default()).unwrap()
                    )
                    .unwrap(),
                    serde_json::to_value(&full).unwrap(),
                );
            }
        }
    }
}

#[test]
fn decimal_work_cancellation_is_terminal_and_preserves_success_accounting() {
    use intersect_model::rcore_behavior_functions::{
        EvaluationSession, ExecutionControl, SessionProgress,
    };
    use std::{cell::Cell, num::NonZeroUsize};
    struct Control {
        polls: Cell<usize>,
        stop: usize,
    }
    impl ExecutionControl for Control {
        fn cancellation_requested(&self) -> bool {
            self.polls.set(self.polls.get() + 1);
            self.polls.get() == self.stop
        }
    }
    let (bundle, source) = fixture("fn run(a:Decimal,b:Decimal)->Decimal=a/b;");
    let image = compile(&bundle, &source);
    let ty = image.program().functions[image.program().entry.0 as usize].parameters[0];
    let args = ["1", "340282366920938463463374607431768211456"].map(|coefficient| {
        scalar(
            ty,
            Scalar::Decimal,
            serde_json::json!({"kind":"decimal","coefficient":coefficient,"scale":0}),
        )
    });
    let context = ExecutionContext::new(&bundle);
    let limits = ExecutionLimits::default();
    let baseline = evaluate(&image, &args, &context, &limits).unwrap();
    let run = |stop| {
        let control = Control {
            polls: Cell::new(0),
            stop,
        };
        let mut session = EvaluationSession::new(&image, &args, &context, &limits).unwrap();
        loop {
            match session.resume_with_control(NonZeroUsize::new(4096).unwrap(), &control) {
                SessionProgress::Pending => {}
                SessionProgress::Complete(result) => {
                    assert!(matches!(
                        session.resume(NonZeroUsize::MIN),
                        SessionProgress::Finished
                    ));
                    break (*result, control.polls.get());
                }
                SessionProgress::Finished => panic!("missing terminal result"),
            }
        }
    };
    let (completed, polls) = run(usize::MAX);
    assert_eq!(
        serde_json::to_value(completed.unwrap()).unwrap(),
        serde_json::to_value(&baseline).unwrap()
    );
    let mut numeric_cancellations = 0;
    for stop in 1..=polls {
        let (result, seen) = run(stop);
        let error = result.unwrap_err();
        assert_eq!(seen, stop);
        assert_eq!(error.kind, ExecutionErrorKind::Cancelled);
        assert!(error.arithmetic_kind.is_none());
        assert!(error.consumed.numeric_work <= baseline.consumed.numeric_work);
        if error.message.contains("numeric work") {
            numeric_cancellations += 1;
            assert!(error.span.is_some());
        }
    }
    assert!(numeric_cancellations > 128);
    assert_eq!(
        serde_json::to_value(run(usize::MAX).0.unwrap()).unwrap(),
        serde_json::to_value(baseline).unwrap()
    );
}

#[test]
fn decimal_alignment_and_intermediate_limits_apply_inside_source_calls() {
    let (bundle, source) =
        fixture("fn run()->Decimal=add(1.0,0.001);fn add(a:Decimal,b:Decimal)->Decimal=a+b;");
    let image = compile(&bundle, &source);
    let context = ExecutionContext::new(&bundle);
    let baseline = evaluate(&image, &[], &context, &ExecutionLimits::default()).unwrap();
    let DataWire::Scalar(value) = &baseline.value.data else {
        panic!("decimal")
    };
    assert_eq!(
        value.payload,
        serde_json::json!({"kind":"decimal","coefficient":"1001","scale":3})
    );
    // 10 at scale 1 needs two padding places to reach scale 3. The padded
    // coefficient has four digits; addition reserves a fifth for a possible carry.
    for (alignment, needed, message) in [
        (true, 2, "Decimal scale alignment limit exceeded"),
        (false, 5, "Numeric intermediate digit limit exceeded"),
    ] {
        for ceiling in [0, needed - 1, needed, needed + 1] {
            let mut limits = ExecutionLimits::default();
            if alignment {
                limits.scalar.numeric.max_scale_alignment = ceiling;
            } else {
                limits.scalar.numeric.max_intermediate_digits = ceiling;
            }
            let result = evaluate(&image, &[], &context, &limits);
            if ceiling >= needed {
                assert_eq!(
                    serde_json::to_value(result.unwrap()).unwrap(),
                    serde_json::to_value(&baseline).unwrap()
                );
            } else {
                let error = result.unwrap_err();
                assert_eq!(error.kind, ExecutionErrorKind::ResourceExhausted);
                assert_eq!(&*error.message, message);
                assert!(error.consumed.steps > 0);
                assert_eq!(error.trace.len(), 2);
                assert_eq!(
                    serde_json::to_value(
                        evaluate(&image, &[], &context, &ExecutionLimits::default()).unwrap()
                    )
                    .unwrap(),
                    serde_json::to_value(&baseline).unwrap()
                );
            }
        }
    }
}
