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
        "memory:grouping-behavior.rcore",
        SourceKind::RcoreBehavior,
        format!("rcore 1;package grouping version \"1\";{body}"),
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

fn int(value: &ir_v2::ValueWire) -> i64 {
    let DataWire::Scalar(value) = &value.data else {
        panic!("Int")
    };
    value.payload["value"].as_str().unwrap().parse().unwrap()
}
fn groups(value: ir_v2::ValueWire) -> Vec<(ir_v2::ValueWire, Vec<ir_v2::ValueWire>)> {
    let DataWire::Collection(groups) = value.data else {
        panic!("Collection")
    };
    groups
        .into_iter()
        .map(|group| {
            let DataWire::Tuple(fields) = group.data else {
                panic!("Pair")
            };
            let [key, members]: [ir_v2::ValueWire; 2] = fields.try_into().unwrap();
            let DataWire::Collection(members) = members.data else {
                panic!("Members")
            };
            (key, members)
        })
        .collect()
}
#[test]
fn grouping_preserves_first_key_order_member_order_and_all_collection_kinds() {
    for kind in ["Sequence", "OrderedSet", "Set", "Bag"] {
        let outer = if matches!(kind, "Sequence" | "OrderedSet") {
            "Sequence"
        } else {
            "Bag"
        };
        let value = run(&format!(
            "fn run()->{outer}<(Int,{kind}<Int>)> =groupBy({kind}([3,1,2,3,4]),|x:Int|x%2);"
        ));
        let groups = groups(value);
        assert_eq!(
            groups.iter().map(|(key, _)| int(key)).collect::<Vec<_>>(),
            [1, 0]
        );
        assert_eq!(
            groups[0].1.iter().map(int).collect::<Vec<_>>(),
            if matches!(kind, "Sequence" | "Bag") {
                vec![3, 1, 3]
            } else {
                vec![3, 1]
            }
        );
        assert_eq!(groups[1].1.iter().map(int).collect::<Vec<_>>(), [2, 4]);
        assert_true(&format!(
            "fn run()->Bool=isEmpty(groupBy({kind}::<Int>([]),|x:Int|1/0));"
        ));
    }
}
#[test]
fn grouping_callbacks_share_generics_captures_recursion_and_nested_operations() {
    for body in [
        "fn run()->Bool=size(groupBy([1,2,1],keep))==2;fn keep<T>(x:T)->T=x;",
        "fn run()->Bool={let groups:Sequence<(Int,Sequence<Int>)> =groupBy([],keep);isEmpty(groups)};fn keep<T>(x:T)->T=x;",
        "fn run()->Bool={let n=2;size(groupBy([1,2,3],|x:Int|x%n))==2};",
        "fn run()->Bool=size(groupBy([5,4,3],key))==2;fn key(x:Int)->Int=if x<2 {x} else {key(x-2)};",
        "fn run()->Bool=size(groupBy([1,2],|x:Int|size(groupBy([x,x+1],|y:Int|y%2))))==1;",
        "fn run()->Bool={let groups:Sequence<(Option<Int>,Sequence<Int>)> =groupBy([1,2],|x:Int|None);size(groups)==1};",
        "fn run()->Bool={let groupBy=|x:Int|x;groupBy(2)==2};",
        "fn run()->Bool=groupBy(2)==2;fn groupBy(x:Int)->Int=x;",
    ] {
        assert_true(body)
    }
}
#[test]
fn grouping_keys_use_total_equivalence_for_structures_results_and_private_functions() {
    for body in [
        "fn run()->Bool=size(groupBy([1,2],|x:Int|if x==1 {Bag([1,2])} else {Bag([2,1])}))==1;",
        "fn run()->Bool=size(groupBy([1,2,3],|x:Int|(x%2,Some(true))))==2;",
        "fn run()->Bool=size(groupBy([1,2],|x:Int|Err::<Int,String>(\"same\")))==1;",
        "fn run()->Bool={let f=|x:Int|x;size(groupBy([1,2],|x:Int|f))==1};",
        "fn run()->Bool=size(groupBy([1,2],|x:Int| |y:Int|y))==2;",
    ] {
        assert_true(body)
    }
}
#[test]
fn grouping_retains_first_ieee_key_representation() {
    use intersect_model::definitions::rcore_behavior::{ir, types::Scalar};
    let (bundle, source) = fixture(
        "fn run(xs:Sequence<Float64>)->Sequence<(Float64,Sequence<Float64>)> =groupBy(xs,|x:Float64|x);",
    );
    let image = compile(&bundle, &source);
    let program = image.program();
    let ty = program.functions[program.entry.0 as usize].parameters[0];
    let ir_v2::TypeNode::Collection { element, .. } = program.types[ty.0 as usize] else {
        panic!("Sequence")
    };
    let bits = [
        "7ff8000000000001",
        "7ff8000000000002",
        "8000000000000000",
        "0000000000000000",
    ];
    let values = bits
        .iter()
        .map(|bits| ir_v2::ValueWire {
            ty: element,
            data: DataWire::Scalar(ir::Value {
                ty: ir::ValueType::Scalar(Scalar::Float64),
                payload: serde_json::json!({"kind":"float64","bits":bits}),
            }),
        })
        .collect();
    let result = evaluate(
        &image,
        &[ir_v2::ValueWire {
            ty,
            data: DataWire::Collection(values),
        }],
        &ExecutionContext::new(&bundle),
        &ExecutionLimits::default(),
    )
    .unwrap();
    let groups = groups(result.value);
    assert_eq!(groups.len(), 2);
    for (index, (key, members)) in groups.iter().enumerate() {
        let DataWire::Scalar(key) = &key.data else {
            panic!("Key")
        };
        assert_eq!(key.payload["bits"], bits[index * 2]);
        assert_eq!(members.len(), 2);
        for (offset, member) in members.iter().enumerate() {
            let DataWire::Scalar(member) = &member.data else {
                panic!("Member")
            };
            assert_eq!(member.payload["bits"], bits[index * 2 + offset]);
        }
    }
}
#[test]
fn grouping_rejects_wrong_callback_and_output_contracts_and_preserves_failures() {
    for body in [
        "fn run()->Sequence<(Int,Sequence<Int>)> =groupBy([1],|x:Bool|1);",
        "fn run()->Sequence<(Int,Sequence<Int>)> =groupBy([1],|x:Int,y:Int|1);",
        "fn run()->Sequence<(Int,Set<Int>)> =groupBy([1],|x:Int|x);",
        "fn run()->Sequence<(Int,Bag<Int>)> =groupBy(Bag([1]),|x:Int|x);",
        "fn run()->Sequence<(Bool,Sequence<Int>)> =groupBy([1],|x:Int|x);",
        "fn run()->Sequence<(Int,Sequence<Int>)> =groupBy::<Int>([1],|x:Int|x);",
        "fn run()->Sequence<(Int,Sequence<Int>)> =groupBy([1]);",
    ] {
        let (bundle, source) = fixture(body);
        let errors = compile_function(
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
        .unwrap_err();
        assert!(!errors.is_empty());
        assert!(
            errors
                .iter()
                .all(|e| e.span.source == source.id && e.span.end <= source.text.len())
        );
    }
    let (bundle, source) =
        fixture("fn run()->Sequence<(Int,Sequence<Int>)> =groupBy([1,0],|x:Int|10/x);");
    let image = compile(&bundle, &source);
    let error = evaluate(
        &image,
        &[],
        &ExecutionContext::new(&bundle),
        &ExecutionLimits::default(),
    )
    .unwrap_err();
    assert_eq!(error.kind, ExecutionErrorKind::Arithmetic);
    assert_eq!(error.trace.len(), 2);
    assert_true("fn run()->Bool=size(groupBy([1,2],|x:Int|x))==2;");
}
#[test]
fn grouping_ir_rejects_wrong_operation_and_group_shape() {
    use ir_v2::CollectionOp;
    let (bundle, source) =
        fixture("fn run()->Sequence<(Int,Sequence<Int>)> =groupBy([1,2],|x:Int|x);");
    let image = compile(&bundle, &source);
    for operation in [
        CollectionOp::Map,
        CollectionOp::Filter,
        CollectionOp::FlatMap,
    ] {
        let mut program = image.program().clone();
        let instruction = program
            .functions
            .iter_mut()
            .flat_map(|f| &mut f.blocks)
            .flat_map(|b| &mut b.instructions)
            .find(|i| {
                matches!(
                    i.kind,
                    InstructionKind::CollectionCall {
                        operation: CollectionOp::GroupBy,
                        ..
                    }
                )
            })
            .unwrap();
        let InstructionKind::CollectionCall {
            operation: actual, ..
        } = &mut instruction.kind
        else {
            unreachable!()
        };
        *actual = operation;
        assert!(ir_v2::verify(program, &bundle, &Limits::default()).is_err());
    }
}
#[test]
fn grouping_slices_limits_and_every_cancellation_boundary_preserve_shared_state() {
    struct Cancel;
    impl ExecutionControl for Cancel {
        fn cancellation_requested(&self) -> bool {
            true
        }
    }
    let (bundle, source) = fixture(
        "fn run()->Sequence<(Int,Sequence<Int>)> ={let n=3;groupBy([3,1,2,3,4,5,6,7,8,9,10,11],|x:Int|x%n)};",
    );
    let image = compile(&bundle, &source);
    let context = ExecutionContext::new(&bundle);
    let limits = ExecutionLimits::default();
    let baseline = evaluate(&image, &[], &context, &limits).unwrap();
    let expected = serde_json::to_value(&baseline).unwrap();
    let mut boundaries = 0;
    for quantum in [1, 3, 4096] {
        let mut job = EvaluationSession::new(&image, &[], &context, &limits).unwrap();
        let mut pending = 0;
        loop {
            match job.resume(NonZeroUsize::new(quantum).unwrap()) {
                SessionProgress::Pending => {
                    pending += 1;
                    assert!(pending < 10000)
                }
                SessionProgress::Complete(result) => {
                    assert_eq!(serde_json::to_value(result.unwrap()).unwrap(), expected);
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
        let mut job = EvaluationSession::new(&image, &[], &context, &limits).unwrap();
        for _ in 0..boundary {
            assert!(matches!(
                job.resume(NonZeroUsize::MIN),
                SessionProgress::Pending
            ))
        }
        let SessionProgress::Complete(result) = job.resume_with_control(NonZeroUsize::MIN, &Cancel)
        else {
            panic!("Missing cancellation")
        };
        assert_eq!(result.unwrap_err().kind, ExecutionErrorKind::Cancelled);
        assert!(matches!(
            job.resume(NonZeroUsize::MAX),
            SessionProgress::Finished
        ));
    }
    for dimension in 0..5 {
        let amount = match dimension {
            0 => baseline.consumed.steps,
            1 => baseline.consumed.value_bytes,
            2 => baseline.consumed.value_nodes,
            3 => baseline.consumed.collection_comparisons,
            _ => baseline.consumed.captures,
        };
        for bound in [0, amount - 1, amount, amount + 1] {
            let mut limits = ExecutionLimits::default();
            match dimension {
                0 => limits.scalar.max_steps = bound,
                1 => limits.scalar.max_value_bytes = bound,
                2 => limits.max_value_nodes = bound,
                3 => limits.max_collection_comparisons = bound,
                _ => limits.max_captures = bound,
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
fn grouping_many_inputs_validate_bounds_and_shed_member_cardinality() {
    use intersect_model::definitions::rcore_behavior::{
        ir,
        types::{CollectionKind, Scalar},
    };
    use ir_v2::{TypeId, TypeNode, ValueWire};
    for (ordered, unique, name, kind) in [
        (false, false, "Bag", CollectionKind::Bag),
        (false, true, "Set", CollectionKind::Set),
        (true, false, "Sequence", CollectionKind::Sequence),
        (true, true, "OrderedSet", CollectionKind::OrderedSet),
    ] {
        let outer = if ordered { "Sequence" } else { "Bag" };
        let (bundle, source) = fixture(&format!(
            "fn run<T:{name}<Int>>(xs:T)->{outer}<(Int,{name}<Int>)> =groupBy(xs,|x:Int|x);"
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
                        lower: 2,
                        upper: 3,
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
        let outer_kind = if ordered {
            CollectionKind::Sequence
        } else {
            CollectionKind::Bag
        };
        assert!(
            matches!(&program.types[output.ty.0 as usize], TypeNode::Collection {kind, ..} if *kind == outer_kind)
        );
        let DataWire::Collection(groups) = output.data else {
            panic!("Groups")
        };
        assert_eq!(groups.len(), 2);
        for (index, group) in groups.iter().enumerate() {
            let DataWire::Tuple(fields) = &group.data else {
                panic!("Pair")
            };
            assert_eq!(fields.len(), 2);
            let DataWire::Scalar(key) = &fields[0].data else {
                panic!("Key")
            };
            assert_eq!(key.payload["value"], (index + 1).to_string());
            assert!(
                matches!(&program.types[fields[1].ty.0 as usize], TypeNode::Collection {kind: actual, ..} if *actual == kind)
            );
            let DataWire::Collection(members) = &fields[1].data else {
                panic!("Members")
            };
            // Each group is valid with one member although the input lower bound is two.
            assert_eq!(members.len(), 1);
            assert_eq!(
                serde_json::to_value(&members[0]).unwrap(),
                serde_json::to_value(&fields[0]).unwrap()
            );
        }
        for invalid in [&[][..], &[1][..], &[1, 2, 3, 4][..]] {
            assert!(evaluate(&image, &[values(invalid)], &context, &limits).is_err());
        }
        assert_eq!(
            evaluate(&image, &[values(&[1, 1])], &context, &limits).is_err(),
            unique
        );
    }
}
