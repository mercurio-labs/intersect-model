use intersect_model::definitions::{
    bundle::{DefinitionBundle, DefinitionSource, SourceKind},
    rcore_behavior::{
        function_lowering::compile_function,
        ir_v2::{self, CollectionOp, DataWire, EntrySelection, InstructionKind, Limits},
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
        "memory:callbacks-behavior.rcore",
        SourceKind::RcoreBehavior,
        format!("rcore 1;package callbacks version \"1\";{body}"),
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
fn map_filter_and_flat_map_preserve_exact_kind_and_occurrences() {
    for kind in ["Sequence", "OrderedSet", "Set", "Bag"] {
        let ordered = matches!(kind, "Sequence" | "OrderedSet");
        let output = if ordered { "Sequence" } else { "Bag" };
        let count = if matches!(kind, "Set" | "OrderedSet") {
            2
        } else {
            3
        };
        assert_true(&format!(
            "fn run()->Bool={{let xs:{output}<Int> =map({kind}([1,1,2]),|x:Int|0);size(xs)=={count}}};"
        ));
        assert_true(&format!(
            "fn run()->Bool={{let xs:{kind}<Int> =filter({kind}([1,1,2]),|x:Int|x==1);size(xs)=={}}};",
            count - 1
        ));
        for inner in ["Sequence", "OrderedSet", "Set", "Bag"] {
            let output = if ordered && matches!(inner, "Sequence" | "OrderedSet") {
                "Sequence"
            } else {
                "Bag"
            };
            assert_true(&format!(
                "fn run()->Bool={{let xs:{output}<Int> =flatMap({kind}([1,2]),|x:Int|{inner}([x,x+10]));size(xs)==4}};"
            ));
        }
    }
    assert_true(
        "fn run()->Bool=fold(flatMap([1,2],|x:Int|[x,x+10]),0,|a:Int,x:Int|a*100+x)==1110212;",
    );
    assert_true("fn run()->Bool=isEmpty(flatMap([1,2],|x:Int|Sequence::<Int>([])));");
    assert_true(
        "fn run()->Bool=fold(flatMap([1,2],|x:Int|[[x]]),0,|a:Int,x:Sequence<Int>|a+size(x))==2;",
    );
}
#[test]
fn callbacks_use_generics_captures_recursion_and_nested_operations() {
    for body in [
        "fn run()->Bool=fold(map([1,2],keep),0,|a:Int,x:Int|a*10+x)==12;fn keep<T>(x:T)->T=x;",
        "fn run()->Bool={let n=4;fold(map([1,2],|x:Int|x+n),0,|a:Int,x:Int|a*10+x)==56};",
        "fn run()->Bool=fold(map([1,2],|x:Int|fold([3,4],x,|a:Int,b:Int|a+b)),0,|a:Int,x:Int|a*10+x)==89;",
        "fn run()->Bool=fold(map([3,4],fact),0,|a:Int,x:Int|a*100+x)==624;fn fact(x:Int)->Int=if x==0 {1} else {x*fact(x-1)};",
        "fn run()->Bool={let map=|x:Int|x+1;map(2)==3};",
        "fn run()->Bool=map(2)==3;fn map(x:Int)->Int=x+1;",
        "fn run()->Bool=find(map([1,2],|x:Int|Some(x)),|x:Option<Int>|x==Some(2))==Some(Some(2));",
    ] {
        assert_true(body)
    }
}
#[test]
fn folds_and_predicates_have_empty_identities_and_short_circuit() {
    for body in [
        "fn run()->Bool=fold([1,2,3],0,|a:Int,x:Int|a*10+x)==123;",
        "fn run()->Bool=fold(Sequence::<Int>([]),7,|a:Int,x:Int|1/0)==7;",
        "fn run()->Bool=exists([1,0],|x:Int|10/x==10);",
        "fn run()->Bool=!forAll([2,0],|x:Int|10/x==10);",
        "fn run()->Bool=!one([1,1,0],|x:Int|10/x==10);",
        "fn run()->Bool=one([2,1,3],|x:Int|x==1);",
        "fn run()->Bool=!exists(Sequence::<Int>([]),|x:Int|1/0==0);",
        "fn run()->Bool=forAll(Sequence::<Int>([]),|x:Int|1/0==0);",
        "fn run()->Bool=!one(Sequence::<Int>([]),|x:Int|1/0==0);",
        "fn run()->Bool=find([2,1,0],|x:Int|10/x==10)==Some(1);",
        "fn run()->Bool=find([1,2],|x:Int|x==3)==None::<Int>();",
    ] {
        assert_true(body)
    }
}
#[test]
fn callback_errors_remain_located_and_dead_branches_are_checked() {
    for body in [
        "fn run()->Sequence<Int> =map([1],|x:Bool|1);",
        "fn run()->Sequence<Int> =map([],|x:Int|true);",
        "fn run()->Bool=isEmpty(map([],keep));fn keep<T>(x:T)->T=x;",
        "fn run()->Sequence<Int8> =map([300],|x:Int8|x);",
        "fn run()->Sequence<Bool> =map(Set::<Int>([]),|x:Bool|x);",
        "fn run()->Sequence<Bool> =map([],keep);fn keep<T:Int>(x:T)->T=x;",
        "fn run()->Sequence<Int> =filter([1],|x:Int|1);",
        "fn run()->Sequence<Int> =flatMap([1],|x:Int|1);",
        "fn run()->Int=fold([1],0,|x:Int|x);",
        "fn run()->Int=fold([1],0,|a:Int,x:Int|true);",
        "fn run()->Bool=exists([1],|x:Int|1);",
        "fn run()->Option<Int> =find(Bag([1]),|x:Int|true);",
        "fn run()->Sequence<Int> =map::<Int>([1],|x:Int|x);",
        "fn run()->Int=if true {0} else {size(map([1],false))};",
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
}
#[test]
fn forged_operation_seed_and_result_types_are_rejected() {
    let (bundle, source) = fixture("fn run()->Sequence<Int> =map([1,2],|x:Int|x+1);");
    let image = compile(&bundle, &source);
    for operation in [
        CollectionOp::Filter,
        CollectionOp::Fold,
        CollectionOp::Exists,
        CollectionOp::FlatMap,
    ] {
        let mut program = image.program().clone();
        let instruction = program
            .functions
            .iter_mut()
            .flat_map(|f| &mut f.blocks)
            .flat_map(|b| &mut b.instructions)
            .find(|i| matches!(i.kind, InstructionKind::CollectionCall { .. }))
            .unwrap();
        let InstructionKind::CollectionCall { operation: op, .. } = &mut instruction.kind else {
            unreachable!()
        };
        *op = operation;
        assert!(ir_v2::verify(program, &bundle, &Limits::default()).is_err());
    }
}
#[test]
fn continuations_preserve_receipts_across_schedules_and_exact_budgets() {
    let (bundle, source) =
        fixture("fn run()->Sequence<Int> =flatMap([1,2,3],|x:Int|map([10,20],|y:Int|x+y));");
    let image = compile(&bundle, &source);
    let context = ExecutionContext::new(&bundle);
    let limits = ExecutionLimits::default();
    let baseline = evaluate(&image, &[], &context, &limits).unwrap();
    let expected = serde_json::to_value(&baseline).unwrap();
    for quantum in [1, 3, 4096] {
        let mut session = EvaluationSession::new(&image, &[], &context, &limits).unwrap();
        loop {
            match session.resume(NonZeroUsize::new(quantum).unwrap()) {
                SessionProgress::Pending => {}
                SessionProgress::Complete(result) => {
                    assert_eq!(serde_json::to_value(result.unwrap()).unwrap(), expected);
                    break;
                }
                SessionProgress::Finished => panic!("Lost result"),
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
fn cancellation_at_every_suspension_is_terminal_and_does_not_affect_other_jobs() {
    struct Cancel;
    impl ExecutionControl for Cancel {
        fn cancellation_requested(&self) -> bool {
            true
        }
    }
    let (bundle, source) =
        fixture("fn run()->Sequence<Int> =flatMap([1,2],|x:Int|map([3,4],|y:Int|x+y));");
    let image = compile(&bundle, &source);
    let context = ExecutionContext::new(&bundle);
    let limits = ExecutionLimits::default();
    let expected = serde_json::to_value(evaluate(&image, &[], &context, &limits).unwrap()).unwrap();
    let mut probe = EvaluationSession::new(&image, &[], &context, &limits).unwrap();
    let mut boundaries = 0;
    while matches!(probe.resume(NonZeroUsize::MIN), SessionProgress::Pending) {
        boundaries += 1;
        assert!(boundaries < 10000)
    }
    assert!(boundaries > 20);
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
            panic!("Cancellation must finish")
        };
        assert_eq!(result.unwrap_err().kind, ExecutionErrorKind::Cancelled);
        assert!(matches!(
            session.resume(NonZeroUsize::MAX),
            SessionProgress::Finished
        ));
    }
    assert_eq!(
        serde_json::to_value(evaluate(&image, &[], &context, &limits).unwrap()).unwrap(),
        expected
    );
}

#[test]
fn literal_inputs_receive_callback_context_without_confusing_input_and_output() {
    for body in [
        "fn run()->Bool=isEmpty(map([],|x:Int|true));",
        "fn run()->Bool=isEmpty(flatMap([],|x:Int|[true]));",
        "fn run()->Bool=isEmpty(filter(Set([]),|x:Int|true));",
        "fn run()->Bool=fold([],7,|a:Int,x:Bool|a)==7;",
        "fn run()->Bool=forAll([],|x:Int|true);",
        "fn run()->Bool=find([],|x:Int|true)==None::<Int>();",
        "fn run()->Bool={let xs:Sequence<Int8> =map([],keep);isEmpty(xs)};fn keep<T>(x:T)->T=x;",
        "fn run()->Bool={let xs:Sequence<Bool> =map([None],|x:Option<Int>|x==None::<Int>());size(xs)==1};",
        "fn run()->Bool={let f=|x:Int|true;isEmpty(map([],f))};",
        "fn run()->Bool=isEmpty(map([],convertValue));fn convertValue(x:Int)->Bool=true;",
        "fn run()->Bool=isEmpty(map(([]),(|x:Int|true)));",
    ] {
        assert_true(body)
    }
}

#[test]
fn many_callbacks_validate_inputs_and_shed_transformed_cardinality_contracts() {
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
        let mapped = if ordered { "Sequence" } else { "Bag" };
        for (result, operation, count) in [
            (mapped.to_string(), "map(xs,|x:Int|0)", 2),
            (kind.to_string(), "filter(xs,|x:Int|false)", 0),
            (mapped.to_string(), "flatMap(xs,|x:Int|[x,x])", 4),
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
                panic!("Expected Many")
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
            let value = evaluate(&image, &[values(&[1, 2])], &context, &limits)
                .unwrap()
                .value;
            let expected_kind = match result.as_str() {
                "Sequence" => CollectionKind::Sequence,
                "Set" => CollectionKind::Set,
                "OrderedSet" => CollectionKind::OrderedSet,
                _ => CollectionKind::Bag,
            };
            assert!(
                matches!(&program.types[value.ty.0 as usize],TypeNode::Collection{kind,..} if *kind==expected_kind)
            );
            let DataWire::Collection(items) = value.data else {
                panic!("Expected collection")
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
fn imported_generic_callbacks_retain_exact_module_identity_and_failure_sites() {
    for fail in [false, true] {
        let helper = DefinitionSource::new(
            "memory:helper-behavior.rcore",
            SourceKind::RcoreBehavior,
            if fail {
                "rcore 1;package helper version \"1\";pub fn convertValue(x:Int)->Int=10/x;"
            } else {
                "rcore 1;package helper version \"1\";pub fn convertValue<T>(x:T)->T=x;"
            },
        );
        let mut entry = DefinitionSource::new(
            "memory:entry-behavior.rcore",
            SourceKind::RcoreBehavior,
            "rcore 1;package entry version \"1\";import package helper version \"1\" as h;fn run()->Sequence<Int> =map([1,0],h.convertValue);",
        );
        entry.dependencies.push(helper.id.clone());
        let mut bundle = DefinitionBundle {
            version: 1,
            sources: vec![entry.clone(), helper.clone()],
            namespaces: vec![],
        };
        let selection = EntrySelection {
            symbol: SymbolId {
                source: entry.id.clone(),
                declaration: 1,
            },
            types: vec![],
            arguments: vec![],
        };
        let image = compile_function(&bundle, &entry.id, &selection, &Limits::default()).unwrap();
        let outcome = evaluate(
            &image,
            &[],
            &ExecutionContext::new(&bundle),
            &ExecutionLimits::default(),
        );
        if fail {
            let error = outcome.as_ref().unwrap_err();
            assert_eq!(error.kind, ExecutionErrorKind::Arithmetic);
            let json = serde_json::to_value(error).unwrap();
            assert_eq!(
                json["trace"][0]["definition"]["source"],
                serde_json::to_value(&helper.id).unwrap()
            );
            assert_eq!(
                json["trace"][0]["call_site"]["source"],
                serde_json::to_value(&entry.id).unwrap()
            );
        } else {
            let DataWire::Collection(items) = &outcome.as_ref().unwrap().value.data else {
                panic!("Expected collection")
            };
            assert_eq!(items.len(), 2);
        }
        let expected = serde_json::to_value(outcome).unwrap();
        bundle.sources.reverse();
        let reordered =
            compile_function(&bundle, &entry.id, &selection, &Limits::default()).unwrap();
        assert_eq!(image.program(), reordered.program());
        assert_eq!(
            serde_json::to_value(evaluate(
                &reordered,
                &[],
                &ExecutionContext::new(&bundle),
                &ExecutionLimits::default()
            ))
            .unwrap(),
            expected
        );
    }
}
