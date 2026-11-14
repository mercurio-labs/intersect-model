use intersect_model::definitions::{
    bundle::{DefinitionBundle, DefinitionSource, SourceKind},
    rcore_behavior::{
        function_lowering::compile_function,
        ir_v2::{self, DataWire, EntrySelection, InstructionKind, Limits},
        modules::SymbolId,
    },
};
use intersect_model::rcore_behavior_functions::{
    EvaluationSession, ExecutionContext, ExecutionControl, ExecutionErrorKind, ExecutionLimits,
    SessionProgress, evaluate,
};
use std::num::NonZeroUsize;

fn fixture(body: &str) -> (DefinitionBundle, DefinitionSource) {
    let source = DefinitionSource::new(
        "memory:algebra-behavior.rcore",
        SourceKind::RcoreBehavior,
        format!("rcore 1;package algebra version \"1\";{body}"),
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
fn compile(bundle: &DefinitionBundle, source: &DefinitionSource) -> ir_v2::VerifiedExecutable {
    compile_function(
        bundle,
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
    .unwrap_or_else(|e| panic!("{}: {e:?}", source.text))
}
fn assert_true(body: &str) {
    let (bundle, source) = fixture(body);
    let image = compile(&bundle, &source);
    let result = evaluate(
        &image,
        &[],
        &ExecutionContext::new(&bundle),
        &ExecutionLimits::default(),
    )
    .unwrap_or_else(|e| panic!("{body}: {e:?}"));
    let DataWire::Scalar(value) = result.value.data else {
        panic!("Expected Bool")
    };
    assert_eq!(value.payload, serde_json::json!(true), "{body}");
}

fn run(body: &str) -> ir_v2::ValueWire {
    let (bundle, source) = fixture(body);
    let image = compile(&bundle, &source);
    evaluate(
        &image,
        &[],
        &ExecutionContext::new(&bundle),
        &ExecutionLimits::default(),
    )
    .unwrap_or_else(|e| panic!("{body}: {e:?}"))
    .value
}
fn numbers(body: &str) -> Vec<i64> {
    let DataWire::Collection(values) = run(body).data else {
        panic!("Collection")
    };
    values
        .into_iter()
        .map(|v| {
            let DataWire::Scalar(v) = v.data else {
                panic!("Scalar")
            };
            v.payload["value"].as_str().unwrap().parse().unwrap()
        })
        .collect()
}
#[test]
fn union_intersection_difference_and_distinct_observe_each_kind_contract() {
    for kind in ["Sequence", "Bag", "Set", "OrderedSet"] {
        let unique = matches!(kind, "Set" | "OrderedSet");
        for (operation, expected) in [
            (
                "union",
                if unique {
                    vec![3, 1, 2, 4]
                } else {
                    vec![3, 1, 1, 2, 1, 1, 4]
                },
            ),
            ("intersection", if unique { vec![1] } else { vec![1, 1] }),
            ("difference", vec![3, 2]),
        ] {
            assert_eq!(
                numbers(&format!(
                    "fn run()->{kind}<Int> ={operation}({kind}([3,1,1,2]),{kind}([1,1,4]));"
                )),
                expected
            );
        }
        let distinct = if matches!(kind, "Sequence" | "OrderedSet") {
            "OrderedSet"
        } else {
            "Set"
        };
        assert_eq!(
            numbers(&format!(
                "fn run()->{distinct}<Int> =distinct({kind}([3,1,1,2]));"
            )),
            [3, 1, 2]
        );
        for operation in ["union", "intersection", "difference"] {
            assert!(
                numbers(&format!(
                    "fn run()->{kind}<Int> ={operation}({kind}::<Int>([]),{kind}::<Int>([]));"
                ))
                .is_empty()
            );
        }
        assert_eq!(
            numbers(&format!(
                "fn run()->{kind}<Int> =difference({kind}([1,2]),{kind}::<Int>([]));"
            )),
            [1, 2]
        );
        assert!(
            numbers(&format!(
                "fn run()->{kind}<Int> =intersection({kind}([1,2]),{kind}::<Int>([]));"
            ))
            .is_empty()
        );
    }
    for left in ["Sequence", "OrderedSet"] {
        for right in ["Sequence", "OrderedSet"] {
            assert_eq!(
                numbers(&format!(
                    "fn run()->Sequence<Int> =concat({left}([1,2]),{right}([2,3]));"
                )),
                [1, 2, 2, 3]
            );
        }
    }
}
#[test]
fn occurrence_subtraction_retains_earliest_decimal_representatives() {
    for (operation, scales) in [("difference", vec![1]), ("intersection", vec![1, 2])] {
        let DataWire::Collection(values) = run(&format!(
            "fn run()->Sequence<Decimal> ={operation}([1.0,1.00,1.000],[1.0000,1.00000]);"
        ))
        .data
        else {
            panic!("Collection")
        };
        let actual: Vec<_> = values
            .iter()
            .map(|v| {
                let DataWire::Scalar(v) = &v.data else {
                    panic!("Decimal")
                };
                v.payload["scale"].as_i64().unwrap()
            })
            .collect();
        assert_eq!(actual, scales);
    }
    let DataWire::Collection(values) =
        run("fn run()->OrderedSet<Decimal> =union(OrderedSet([1.0]),OrderedSet([1.00,2.00]));")
            .data
    else {
        panic!("Collection")
    };
    let DataWire::Scalar(first) = &values[0].data else {
        panic!("Decimal")
    };
    assert_eq!(first.payload["scale"], 1);
}
#[test]
fn algebra_preserves_structures_closure_identity_and_branching_operands() {
    for body in [
        "fn run()->Bool=size(distinct([Bag([1,2]),Bag([2,1])]))==1;",
        "fn run()->Bool=size(difference([[1,2],[1,2],[3]],[[1,2]]))==2;",
        "fn run()->Bool=first(intersection([None::<Int>(),Some(1)],[Some(1)]))==Some(Some(1));",
        "fn run()->Bool={let f=|x:Int|x;size(distinct([f,f]))==1};",
        "fn run()->Bool={let f=|x:Int|x;let g=|x:Int|x;size(distinct([f,g]))==2};",
        "fn run()->Bool=size(union([1],if true {[2]} else {[3]}))==2;",
        "fn run()->Bool=has([1],[2]);fn has<T>(a:Sequence<T>,b:Sequence<T>)->Bool=size(union(a,b))==2;",
        "fn run()->Bool={let union=|x:Int|x;union(2)==2};",
        "fn run()->Bool=distinct(2)==2;fn distinct(x:Int)->Int=x;",
    ] {
        assert_true(body)
    }
}
#[test]
fn invalid_kinds_domains_arities_and_order_promises_are_static_errors() {
    for body in [
        "fn run()->Sequence<Int> =union([1],Set([2]));",
        "fn run()->Sequence<Int> =intersection([1],[true]);",
        "fn run()->Sequence<Int> =difference([1],1);",
        "fn run()->Sequence<Int> =concat(Bag([1]),[2]);",
        "fn run()->OrderedSet<Int> =concat(OrderedSet([1]),OrderedSet([2]));",
        "fn run()->OrderedSet<Int> =distinct(Bag([1]));",
        "fn run()->Sequence<Int> =union::<Int>([1],[2]);",
        "fn run()->Sequence<Int> =union([1]);",
        "fn run()->OrderedSet<Int> =distinct([1],[2]);",
        "fn run()->Int=if true {0} else {size(union([1],[true]))};",
    ] {
        let (bundle, source) = fixture(body);
        let result = compile_function(
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
        );
        let errors = result.unwrap_err();
        assert!(!errors.is_empty(), "{body}");
        assert!(
            errors
                .iter()
                .all(|e| e.span.source == source.id && e.span.end <= source.text.len())
        );
    }
}
#[test]
fn algebra_ir_rejects_missing_operands_wrong_kinds_and_arity_changes() {
    use ir_v2::AlgebraOp;
    let (bundle, source) = fixture("fn run()->Set<Int> =union(Set([1]),Set([2]));");
    let image = compile(&bundle, &source);
    for mutation in 0..3 {
        let mut program = image.program().clone();
        let instruction = program
            .functions
            .iter_mut()
            .flat_map(|f| &mut f.blocks)
            .flat_map(|b| &mut b.instructions)
            .find(|i| matches!(i.kind, InstructionKind::CollectionAlgebra { .. }))
            .unwrap();
        let InstructionKind::CollectionAlgebra {
            operation, other, ..
        } = &mut instruction.kind
        else {
            unreachable!()
        };
        match mutation {
            0 => *other = None,
            1 => *operation = AlgebraOp::Concat,
            _ => *operation = AlgebraOp::Distinct,
        }
        assert!(ir_v2::verify(program, &bundle, &Limits::default()).is_err());
    }
}
#[test]
fn algebra_slices_budgets_and_cancellation_preserve_receipts_and_recover() {
    struct Cancel;
    impl ExecutionControl for Cancel {
        fn cancellation_requested(&self) -> bool {
            true
        }
    }
    let (bundle, source) = fixture(
        "fn run()->Sequence<Int> =difference([1,2,3,1,4,5,1,6,7,1,8,9,1,10,11,1,12,13,1,14,15,1,16,17,1,18,19,1,20,21,1,22],[1,1,1,1]);",
    );
    let image = compile(&bundle, &source);
    let context = ExecutionContext::new(&bundle);
    let limits = ExecutionLimits::default();
    let baseline = evaluate(&image, &[], &context, &limits).unwrap();
    let expected = serde_json::to_value(&baseline).unwrap();
    let mut boundaries = 0;
    for quantum in [1, 3, 4096] {
        let mut session = EvaluationSession::new(&image, &[], &context, &limits).unwrap();
        let mut pending = 0;
        loop {
            match session.resume(NonZeroUsize::new(quantum).unwrap()) {
                SessionProgress::Pending => {
                    pending += 1;
                    assert!(pending < 10000)
                }
                SessionProgress::Complete(outcome) => {
                    assert_eq!(serde_json::to_value(outcome.unwrap()).unwrap(), expected);
                    break;
                }
                SessionProgress::Finished => panic!("Missing result"),
            }
        }
        if quantum == 1 {
            boundaries = pending;
        }
    }
    for boundary in 0..=boundaries {
        let mut session = EvaluationSession::new(&image, &[], &context, &limits).unwrap();
        for _ in 0..boundary {
            assert!(matches!(
                session.resume(NonZeroUsize::MIN),
                SessionProgress::Pending
            ))
        }
        let SessionProgress::Complete(result) =
            session.resume_with_control(NonZeroUsize::MIN, &Cancel)
        else {
            panic!("Missing cancellation")
        };
        assert_eq!(result.unwrap_err().kind, ExecutionErrorKind::Cancelled);
        assert!(matches!(
            session.resume(NonZeroUsize::MAX),
            SessionProgress::Finished
        ));
    }
    for dimension in 0..4 {
        let amount = match dimension {
            0 => baseline.consumed.steps,
            1 => baseline.consumed.value_bytes,
            2 => baseline.consumed.value_nodes,
            _ => baseline.consumed.collection_comparisons,
        };
        for bound in [0, amount - 1, amount, amount + 1] {
            let mut limits = ExecutionLimits::default();
            match dimension {
                0 => limits.scalar.max_steps = bound,
                1 => limits.scalar.max_value_bytes = bound,
                2 => limits.max_value_nodes = bound,
                _ => limits.max_collection_comparisons = bound,
            };
            let result = evaluate(&image, &[], &context, &limits);
            assert_eq!(
                result.is_ok(),
                bound >= amount,
                "dimension {dimension}, bound {bound}: {result:?}"
            );
            if let Err(error) = result {
                assert_eq!(error.kind, ExecutionErrorKind::ResourceExhausted)
            }
        }
    }
    assert_eq!(
        serde_json::to_value(evaluate(&image, &[], &context, &limits).unwrap()).unwrap(),
        expected
    );
}

#[test]
fn algebra_many_inputs_keep_validation_and_produce_concrete_output_kinds() {
    use intersect_model::definitions::rcore_behavior::{
        ir,
        types::{CollectionKind, Scalar},
    };
    use ir_v2::{TypeId, TypeNode, ValueWire};
    for (ordered, unique, kind) in [
        (false, false, "Bag"),
        (false, true, "Set"),
        (true, false, "Sequence"),
        (true, true, "OrderedSet"),
    ] {
        let distinct = if ordered { "OrderedSet" } else { "Set" };
        for (operation, result, count) in [
            (
                format!("union(xs,{kind}([1]))"),
                kind,
                if unique { 2 } else { 3 },
            ),
            (format!("intersection(xs,{kind}([1]))"), kind, 1),
            (format!("difference(xs,{kind}([1]))"), kind, 1),
            ("distinct(xs)".into(), distinct, 2),
        ] {
            let (bundle, source) = fixture(&format!(
                "fn run<T:{kind}<Int>>(xs:T)->{result}<Int> ={operation};"
            ));
            let image = compile_function(
                &bundle,
                &source.id,
                &EntrySelection {
                    symbol: SymbolId {
                        source: source.id.clone(),
                        declaration: 0,
                    },
                    types: vec![
                        TypeNode::Scalar(Scalar::Int64),
                        TypeNode::Many {
                            element: TypeId(0),
                            lower: 1,
                            upper: 2,
                            ordered,
                            unique,
                        },
                    ],
                    arguments: vec![TypeId(1)],
                },
                &Limits::default(),
            )
            .unwrap();
            let program = image.program();
            let ty = program.functions[program.entry.0 as usize].parameters[0];
            let TypeNode::Many { element, .. } = program.types[ty.0 as usize] else {
                panic!("Many")
            };
            let values = |numbers: &[i64]| ValueWire {
                ty,
                data: DataWire::Collection(
                    numbers
                        .iter()
                        .map(|n| ValueWire {
                            ty: element,
                            data: DataWire::Scalar(ir::Value {
                                ty: ir::ValueType::Scalar(Scalar::Int64),
                                payload: serde_json::json!({"kind":"int64","value":n.to_string()}),
                            }),
                        })
                        .collect(),
                ),
            };
            let context = ExecutionContext::new(&bundle);
            let limits = ExecutionLimits::default();
            let output = evaluate(&image, &[values(&[1, 2])], &context, &limits)
                .unwrap()
                .value;
            let expected = match result {
                "Sequence" => CollectionKind::Sequence,
                "OrderedSet" => CollectionKind::OrderedSet,
                "Set" => CollectionKind::Set,
                _ => CollectionKind::Bag,
            };
            assert!(
                matches!(&program.types[output.ty.0 as usize],TypeNode::Collection{kind,..} if *kind==expected)
            );
            let DataWire::Collection(items) = output.data else {
                panic!("Collection")
            };
            assert_eq!(items.len(), count);
            for numbers in [&[][..], &[1, 2, 3][..]] {
                assert!(evaluate(&image, &[values(numbers)], &context, &limits).is_err())
            }
            assert_eq!(
                evaluate(&image, &[values(&[1, 1])], &context, &limits).is_err(),
                unique
            );
        }
    }
}

#[test]
fn conversions_preserve_first_decimal_representations_and_occurrences() {
    for (kind, expected) in [
        ("Set", vec![1, 2]),
        ("OrderedSet", vec![1, 2]),
        ("Sequence", vec![1, 3, 2, 4]),
        ("Bag", vec![1, 3, 2, 4]),
    ] {
        let DataWire::Collection(values) = run(&format!(
            "fn run()->{kind}<Decimal> ={kind}([1.0,1.000,2.00,2.0000]);"
        ))
        .data
        else {
            panic!("Collection");
        };
        let scales: Vec<_> = values
            .iter()
            .map(|value| {
                let DataWire::Scalar(value) = &value.data else {
                    panic!("Decimal");
                };
                value.payload["scale"].as_i64().unwrap()
            })
            .collect();
        assert_eq!(scales, expected, "{kind}");
    }
}
