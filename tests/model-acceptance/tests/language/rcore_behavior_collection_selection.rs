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
        "memory:selection-behavior.rcore",
        SourceKind::RcoreBehavior,
        format!("rcore 1;package selection version \"1\";{body}"),
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

#[test]
fn flatten_uses_both_order_contracts_preserves_occurrences_and_only_removes_one_level() {
    for outer in ["Sequence", "OrderedSet", "Set", "Bag"] {
        for inner in ["Sequence", "OrderedSet", "Set", "Bag"] {
            let result = if matches!(outer, "Sequence" | "OrderedSet")
                && matches!(inner, "Sequence" | "OrderedSet")
            {
                "Sequence"
            } else {
                "Bag"
            };
            assert_true(&format!(
                "fn run()->Bool={{let xs:{result}<Int> =flatten({outer}([{inner}([1,2]),{inner}([2,3])]));size(xs)==4}};"
            ));
        }
    }
    for body in [
        "fn run()->Bool=fold(flatten([[1,2],Sequence::<Int>([]),[3,4]]),0,|a:Int,x:Int|a*10+x)==1234;",
        "fn run()->Bool=isEmpty(flatten(Sequence::<Sequence<Int>>([])));",
        "fn run()->Bool={let xs:Sequence<Sequence<Int>> =flatten([[[1]],[[2,3]]]);size(xs)==2};",
        "fn run()->Bool=size(flatten([[1,1],[1]]))==3;",
        "fn run()->Bool={let flatten=|x:Int|x+1;flatten(1)==2};",
        "fn run()->Bool=flatten(1)==2;fn flatten(x:Int)->Int=x+1;",
    ] {
        assert_true(body)
    }
}
#[test]
fn ordered_selection_preserves_option_shape_and_index_boundaries() {
    for kind in ["Sequence", "OrderedSet"] {
        for (expression, expected) in [
            ("first(xs)", "Some(3)"),
            ("last(xs)", "Some(1)"),
            ("at(xs,0)", "Some(3)"),
            ("at(xs,1)", "Some(2)"),
            ("at(xs,-1)", "None::<Int>()"),
            ("at(xs,3)", "None::<Int>()"),
            ("at(xs,9223372036854775807)", "None::<Int>()"),
        ] {
            assert_true(&format!(
                "fn run()->Bool={{let xs={kind}([3,2,1]);{expression}=={expected}}};"
            ));
        }
        for operation in ["first(xs)", "last(xs)", "at(xs,0)"] {
            assert_true(&format!(
                "fn run()->Bool={{let xs={kind}::<Int>([]);{operation}==None::<Int>()}};"
            ));
        }
    }
    for body in [
        "fn run()->Bool={let xs:Sequence<Option<Int>> =[None,Some(2)];first(xs)==Some(None::<Int>())};",
        "fn run()->Bool=at([10,20],1)==[10,20][1];",
        "fn run()->Bool=at([10,20],if true {1} else {0})==Some(20);",
        "fn run()->Bool=at([10,20],match Some(1) {Some(x)=>x,None=>0})==Some(20);",
        "fn run()->Bool=first([Some(1)])==last([Some(1)]);",
        "fn run()->Bool={let first=|x:Int|x;first(3)==3};",
        "fn run()->Bool=(last)([1,2])==Some(2);",
    ] {
        assert_true(body)
    }
}
#[test]
fn invalid_flatten_and_selection_contracts_are_located_static_errors() {
    for body in [
        "fn run()->Int=flatten([1,2]);",
        "fn run()->Sequence<Int> =flatten([Some([1])]);",
        "fn run()->Sequence<Int> =flatten(Bag([[1]]));",
        "fn run()->Sequence<Int> =flatten([Set([1])]);",
        "fn run()->Sequence<Int> =flatten::<Int>([[1]]);",
        "fn run()->Option<Int> =first(Set([1]));",
        "fn run()->Option<Int> =last(Bag([1]));",
        "fn run()->Option<Int> =at([1],true);",
        "fn run()->Option<Int> =at([1],small());fn small()->Int8=0;",
        "fn run()->Option<Int> =first([1],[2]);",
        "fn run()->Option<Int> =at([1]);",
        "fn run()->Option<Int> =first::<Int>([1]);",
        "fn run()->Int=if true {0} else {size(flatten([1]))};",
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
        assert!(!errors.is_empty(), "{body}");
        assert!(
            errors
                .iter()
                .all(|e| e.span.source == source.id && e.span.end <= source.text.len())
        );
    }
}
#[test]
fn flatten_and_selection_reject_forged_ir_shapes_and_operands() {
    for body in [
        "fn run()->Sequence<Int> =flatten([[1],[2]]);",
        "fn run()->Option<Int> =last([1,2]);",
    ] {
        let (bundle, source) = fixture(body);
        let image = compile(&bundle, &source);
        for wrong_operand in [false, true] {
            let mut program = image.program().clone();
            let instruction = program
                .functions
                .iter_mut()
                .flat_map(|f| &mut f.blocks)
                .flat_map(|b| &mut b.instructions)
                .find(|i| {
                    matches!(
                        i.kind,
                        InstructionKind::CollectionFlatten { .. } | InstructionKind::Index { .. }
                    )
                })
                .unwrap();
            if wrong_operand {
                match &mut instruction.kind {
                    InstructionKind::CollectionFlatten { value }
                    | InstructionKind::Index { value, .. } => {
                        *value = ir_v2::Operand::Parameter(999)
                    }
                    _ => unreachable!(),
                }
            } else {
                instruction.kind = InstructionKind::CollectionSize {
                    value: match instruction.kind {
                        InstructionKind::CollectionFlatten { value }
                        | InstructionKind::Index { value, .. } => value,
                        _ => unreachable!(),
                    },
                }
            }
            assert!(ir_v2::verify(program, &bundle, &Limits::default()).is_err());
        }
    }
}
#[test]
fn flatten_yields_between_outputs_with_exact_budgets_and_terminal_cancellation() {
    // Runtime nodes exceed the independently bounded context metadata floor,
    // so the node boundary below exercises output execution rather than setup.
    struct Cancel;
    impl ExecutionControl for Cancel {
        fn cancellation_requested(&self) -> bool {
            true
        }
    }
    let (bundle, source) = fixture(
        "fn run()->Sequence<Int> =flatten([Sequence::<Int>([]),[1,2,3,4,5,6,7,8,9,10,11,12,13,14,15,16],Sequence::<Int>([]),[17,18,19,20,21,22,23,24,25,26,27,28,29,30,31,32]]);",
    );
    let image = compile(&bundle, &source);
    let context = ExecutionContext::new(&bundle);
    let limits = ExecutionLimits::default();
    let baseline = evaluate(&image, &[], &context, &limits).unwrap();
    let expected = serde_json::to_value(&baseline).unwrap();
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
                SessionProgress::Finished => panic!("Lost result"),
            }
        }
        if quantum == 1 {
            for boundary in 0..=pending {
                let mut job = EvaluationSession::new(&image, &[], &context, &limits).unwrap();
                for _ in 0..boundary {
                    assert!(matches!(
                        job.resume(NonZeroUsize::MIN),
                        SessionProgress::Pending
                    ))
                }
                let SessionProgress::Complete(outcome) =
                    job.resume_with_control(NonZeroUsize::MIN, &Cancel)
                else {
                    panic!("Missing cancellation")
                };
                assert_eq!(outcome.unwrap_err().kind, ExecutionErrorKind::Cancelled);
                assert!(matches!(
                    job.resume(NonZeroUsize::MAX),
                    SessionProgress::Finished
                ));
            }
        }
    }
    for dimension in 0..3 {
        let amount = match dimension {
            0 => baseline.consumed.steps,
            1 => baseline.consumed.value_bytes,
            _ => baseline.consumed.value_nodes,
        };
        for bound in [0, amount - 1, amount, amount + 1] {
            let mut limits = ExecutionLimits::default();
            match dimension {
                0 => limits.scalar.max_steps = bound,
                1 => limits.scalar.max_value_bytes = bound,
                _ => limits.max_value_nodes = bound,
            };
            let outcome = evaluate(&image, &[], &context, &limits);
            assert_eq!(
                outcome.is_ok(),
                bound >= amount,
                "dimension {dimension}, bound {bound}: {outcome:?}"
            );
            if let Err(error) = outcome {
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
fn flatten_accepts_nested_many_contracts_without_retaining_invalid_output_bounds() {
    use intersect_model::definitions::rcore_behavior::{
        ir,
        types::{CollectionKind, Scalar},
    };
    use ir_v2::{TypeId, TypeNode, ValueWire};
    for (outer_ordered, outer_unique, outer) in [
        (false, false, "Bag"),
        (false, true, "Set"),
        (true, false, "Sequence"),
        (true, true, "OrderedSet"),
    ] {
        for (inner_ordered, inner_unique, inner) in [
            (false, false, "Bag"),
            (false, true, "Set"),
            (true, false, "Sequence"),
            (true, true, "OrderedSet"),
        ] {
            let kind = if outer_ordered && inner_ordered {
                "Sequence"
            } else {
                "Bag"
            };
            let (bundle, source) = fixture(&format!(
                "fn run<T:{outer}<{inner}<Int>>>(xs:T)->{kind}<Int> =flatten(xs);"
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
                            ordered: inner_ordered,
                            unique: inner_unique,
                        },
                        TypeNode::Many {
                            element: TypeId(1),
                            lower: 1,
                            upper: 2,
                            ordered: outer_ordered,
                            unique: outer_unique,
                        },
                    ],
                    arguments: vec![TypeId(2)],
                },
                &Limits::default(),
            )
            .unwrap();
            let program = image.program();
            let outer_ty = program.functions[program.entry.0 as usize].parameters[0];
            let TypeNode::Many {
                element: inner_ty, ..
            } = program.types[outer_ty.0 as usize]
            else {
                panic!("Outer Many")
            };
            let TypeNode::Many { element, .. } = program.types[inner_ty.0 as usize] else {
                panic!("Inner Many")
            };
            let scalar = |n: i64| ValueWire {
                ty: element,
                data: DataWire::Scalar(ir::Value {
                    ty: ir::ValueType::Scalar(Scalar::Int64),
                    payload: serde_json::json!({"kind":"int64","value":n.to_string()}),
                }),
            };
            let collection = |numbers: &[i64]| ValueWire {
                ty: inner_ty,
                data: DataWire::Collection(numbers.iter().copied().map(scalar).collect()),
            };
            let value = ValueWire {
                ty: outer_ty,
                data: DataWire::Collection(vec![collection(&[1, 2]), collection(&[2, 3])]),
            };
            let context = ExecutionContext::new(&bundle);
            let limits = ExecutionLimits::default();
            let result = evaluate(&image, std::slice::from_ref(&value), &context, &limits)
                .unwrap()
                .value;
            let expected_kind = if outer_ordered && inner_ordered {
                CollectionKind::Sequence
            } else {
                CollectionKind::Bag
            };
            assert!(
                matches!(&program.types[result.ty.0 as usize],TypeNode::Collection{kind,..} if *kind==expected_kind)
            );
            let DataWire::Collection(items) = result.data else {
                panic!("Expected flattened values")
            };
            assert_eq!(items.len(), 4);
            for inner_values in [&[][..], &[1, 2, 3][..]] {
                let bad = ValueWire {
                    ty: outer_ty,
                    data: DataWire::Collection(vec![collection(inner_values)]),
                };
                assert!(evaluate(&image, &[bad], &context, &limits).is_err());
            }
            let empty = ValueWire {
                ty: outer_ty,
                data: DataWire::Collection(vec![]),
            };
            assert!(evaluate(&image, &[empty], &context, &limits).is_err());
        }
    }
}

#[test]
fn selection_and_flatten_preserve_private_functions_and_structural_values() {
    for body in [
        "fn run()->Bool=match first([|x:Int|x+1]) {Some(f)=>f(3)==4,None=>false};",
        "fn run()->Bool=match last(flatten([[|x:Int|x+1],[|x:Int|x+2]])) {Some(f)=>f(3)==5,None=>false};",
        "fn run()->Bool=first(flatten([[Some((1,true))],[None::<(Int,Bool)>()]]))==Some(Some((1,true)));",
        "fn run()->Bool=last(flatten([[Ok::<Int,String>(1)],[Err::<Int,String>(\"missing\")]]))==Some(Err::<Int,String>(\"missing\"));",
    ] {
        assert_true(body)
    }
}
