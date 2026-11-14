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
        "memory:membership-behavior.rcore",
        SourceKind::RcoreBehavior,
        format!("rcore 1;package membership version \"1\";{body}"),
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
fn membership_and_counts_respect_occurrences_and_empty_identities() {
    for (kind, count) in [("Sequence", 2), ("Bag", 2), ("Set", 1), ("OrderedSet", 1)] {
        for expression in [
            "contains(xs,1)".to_owned(),
            "excludes(xs,3)".to_owned(),
            format!("count(xs,1)=={count}"),
            "includesAll(xs,Bag([1,1,1,2]))".to_owned(),
            "includesAny(xs,Set([3,2]))".to_owned(),
            "includesAll(xs,Sequence::<Int>([]))".to_owned(),
            "!includesAny(xs,Sequence::<Int>([]))".to_owned(),
        ] {
            assert_true(&format!(
                "fn run()->Bool={{let xs={kind}([1,1,2]);{expression}}};"
            ));
        }
        for expression in [
            "!contains(xs,1)",
            "excludes(xs,1)",
            "count(xs,1)==0",
            "!includesAll(xs,[1])",
            "!includesAny(xs,[1])",
            "includesAll(xs,Sequence::<Int>([]))",
        ] {
            assert_true(&format!(
                "fn run()->Bool={{let xs={kind}::<Int>([]);{expression}}};"
            ));
        }
    }
}
#[test]
fn total_equivalence_handles_nested_structures_bag_counts_and_private_closures() {
    for body in [
        "fn run()->Bool=contains([[1,2],[3]],[1,2]);",
        "fn run()->Bool=!contains([[1,2]],[2,1]);",
        "fn run()->Bool=contains([Bag([1,2,1])],Bag([2,1,1]));",
        "fn run()->Bool=!contains([Bag([1,1,2])],Bag([1,2,2]));",
        "fn run()->Bool=contains([Set([1,2])],Set([2,1,2]));",
        "fn run()->Bool=contains([Some((1,true)),None::<(Int,Bool)>()],None);",
        "fn run()->Bool=count([Ok::<Int,String>(1),Err::<Int,String>(\"x\"),Err::<Int,String>(\"x\")],Err::<Int,String>(\"x\"))==2;",
        "fn run()->Bool={let f=|x:Int|x;contains([f],f)};",
        "fn run()->Bool={let f=|x:Int|x;let g=|x:Int|x;excludes([f],g)};",
        "fn run()->Bool=contains([1,2],if true {2} else {3});",
        "fn run()->Bool=has([1,2],2);fn has<T>(xs:Sequence<T>,x:T)->Bool=contains(xs,x);",
        "fn run()->Bool={let contains=|x:Int|x+1;contains(2)==3};",
        "fn run()->Bool=count(2)==3;fn count(x:Int)->Int=x+1;",
    ] {
        assert_true(body)
    }
}
#[test]
fn incompatible_domains_arity_and_generic_arguments_reject_statically() {
    for body in [
        "fn run()->Bool=contains([1],true);",
        "fn run()->Bool=contains(true,1);",
        "fn run()->Bool=includesAll([1],2);",
        "fn run()->Bool=includesAny([1],[true]);",
        "fn run()->Bool=contains::<Int>([1],1);",
        "fn run()->Bool=contains([1]);",
        "fn run()->Int=count([1],1,2);",
        "fn run()->Int8=count([1],1);",
        "fn run()->Bool=if true {true} else {contains([1],\"wrong\")};",
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
fn ieee_nan_and_signed_zero_use_collection_equivalence_without_rewriting_values() {
    use intersect_model::definitions::rcore_behavior::{ir, types::Scalar};
    let (bundle, source) = fixture("fn run(a:Float64,b:Float64)->Bool=contains([a],b);");
    let image = compile(&bundle, &source);
    let program = image.program();
    let ty = program.functions[program.entry.0 as usize].parameters[0];
    let value = |bits: &str| ir_v2::ValueWire {
        ty,
        data: DataWire::Scalar(ir::Value {
            ty: ir::ValueType::Scalar(Scalar::Float64),
            payload: serde_json::json!({"kind":"float64","bits":bits}),
        }),
    };
    for (a, b, expected) in [
        ("7ff8000000000001", "7ff8000000000002", true),
        ("0000000000000000", "8000000000000000", true),
        ("7ff8000000000001", "0000000000000000", false),
    ] {
        let values = [value(a), value(b)];
        let before = values.clone();
        let result = evaluate(
            &image,
            &values,
            &ExecutionContext::new(&bundle),
            &ExecutionLimits::default(),
        )
        .unwrap();
        let DataWire::Scalar(result) = result.value.data else {
            panic!("Expected Bool")
        };
        assert_eq!(result.payload, serde_json::json!(expected));
        assert_eq!(values, before);
    }
}
#[test]
fn membership_ir_rejects_forged_comparison_domains_and_operation_results() {
    use intersect_model::definitions::rcore_behavior::types::Scalar;
    use ir_v2::{MembershipOp, TypeNode};
    let (bundle, source) = fixture("fn run()->Bool=contains([1,2],2);");
    let image = compile(&bundle, &source);
    for domain in [false, true] {
        let mut program = image.program().clone();
        let bool_ty = program
            .types
            .iter()
            .position(|t| *t == TypeNode::Scalar(Scalar::Bool))
            .unwrap();
        let instruction = program
            .functions
            .iter_mut()
            .flat_map(|f| &mut f.blocks)
            .flat_map(|b| &mut b.instructions)
            .find(|i| matches!(i.kind, InstructionKind::CollectionMembership { .. }))
            .unwrap();
        let InstructionKind::CollectionMembership {
            operation, element, ..
        } = &mut instruction.kind
        else {
            unreachable!()
        };
        if domain {
            *element = ir_v2::TypeId(bool_ty as u32)
        } else {
            *operation = MembershipOp::Count
        }
        assert!(ir_v2::verify(program, &bundle, &Limits::default()).is_err());
    }
}
#[test]
fn short_circuit_and_comparison_limits_are_exact_and_recoverable() {
    for (body, comparisons) in [
        ("fn run()->Bool=contains([1,2,3],1);", 1),
        ("fn run()->Bool=excludes([1,2,3],1);", 1),
        ("fn run()->Bool=includesAll([1],[1,1,1]);", 3),
        ("fn run()->Bool=includesAny([1,2,3],[1,4]);", 1),
        ("fn run()->Bool=includesAll([1],Sequence::<Int>([]));", 0),
    ] {
        let (bundle, source) = fixture(body);
        let image = compile(&bundle, &source);
        let context = ExecutionContext::new(&bundle);
        let baseline = evaluate(&image, &[], &context, &ExecutionLimits::default()).unwrap();
        assert_eq!(baseline.consumed.collection_comparisons, comparisons);
        for limit in [
            0,
            comparisons.saturating_sub(1),
            comparisons,
            comparisons + 1,
        ] {
            let limits = ExecutionLimits {
                max_collection_comparisons: limit,
                ..Default::default()
            };
            let result = evaluate(&image, &[], &context, &limits);
            assert_eq!(result.is_ok(), limit >= comparisons);
            if let Err(error) = result {
                assert_eq!(error.kind, ExecutionErrorKind::ResourceExhausted)
            }
        }
        assert_eq!(
            serde_json::to_value(
                evaluate(&image, &[], &context, &ExecutionLimits::default()).unwrap()
            )
            .unwrap(),
            serde_json::to_value(baseline).unwrap()
        );
    }
}
#[test]
fn membership_slices_cancellation_and_steps_share_one_continuation() {
    struct Cancel;
    impl ExecutionControl for Cancel {
        fn cancellation_requested(&self) -> bool {
            true
        }
    }
    let (bundle, source) = fixture(
        "fn run()->Bool=includesAll([Bag([1,2,1]),Bag([3,4])],[Bag([2,1,1]),Bag([4,3]),Bag([5])]);",
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
        let mut job = EvaluationSession::new(&image, &[], &context, &limits).unwrap();
        for _ in 0..boundary {
            assert!(matches!(
                job.resume(NonZeroUsize::MIN),
                SessionProgress::Pending
            ));
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
    for bound in [
        0,
        baseline.consumed.steps - 1,
        baseline.consumed.steps,
        baseline.consumed.steps + 1,
    ] {
        let mut limits = ExecutionLimits::default();
        limits.scalar.max_steps = bound;
        assert_eq!(
            evaluate(&image, &[], &context, &limits).is_ok(),
            bound >= baseline.consumed.steps
        );
    }
    assert_eq!(
        serde_json::to_value(evaluate(&image, &[], &context, &limits).unwrap()).unwrap(),
        expected
    );
}

#[test]
fn membership_accepts_many_and_preserves_argument_validation() {
    use intersect_model::definitions::rcore_behavior::{ir, types::Scalar};
    use ir_v2::{TypeId, TypeNode, ValueWire};
    for (ordered, unique, kind) in [
        (false, false, "Bag"),
        (false, true, "Set"),
        (true, false, "Sequence"),
        (true, true, "OrderedSet"),
    ] {
        let (bundle, source) = fixture(&format!("fn run<T:{kind}<Int>>(xs:T)->Int=count(xs,1);"));
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
        let result = evaluate(&image, &[values(&[1, 2])], &context, &limits).unwrap();
        let DataWire::Scalar(value) = result.value.data else {
            panic!("Int")
        };
        assert_eq!(
            value.payload,
            serde_json::json!({"kind":"int64","value":"1"})
        );
        for numbers in [&[][..], &[1, 2, 3][..]] {
            assert!(evaluate(&image, &[values(numbers)], &context, &limits).is_err())
        }
        assert_eq!(
            evaluate(&image, &[values(&[1, 1])], &context, &limits).is_err(),
            unique
        );
    }
}

#[test]
fn enum_membership_preserves_exact_literal_alias_identity() {
    let model = intersect_model_acceptance::migration_fixture::migrate_source_fixture(DefinitionSource::new(
        "memory:enum.ecore",
        SourceKind::Ecore,
        "<e:EPackage xmlns:e='http://www.eclipse.org/emf/2002/Ecore' xmlns:x='http://www.w3.org/2001/XMLSchema-instance' name='enums' nsURI='urn:membership-enum'><eClassifiers x:type='e:EEnum' name='Mode'><eLiterals name='First' value='7'/><eLiterals name='Alias' value='7'/></eClassifiers><eClassifiers x:type='e:EEnum' name='Other'><eLiterals name='First' value='7'/></eClassifiers></e:EPackage>",
    )).0;
    for (body, valid) in [
        ("contains([m.Mode.First],m.Mode.First)", true),
        ("excludes([m.Mode.First],m.Mode.Alias)", true),
        (
            "count([m.Mode.First,m.Mode.Alias,m.Mode.First],m.Mode.First)==2",
            true,
        ),
        (
            "includesAll([m.Mode.First,m.Mode.Alias],[m.Mode.Alias,m.Mode.First])",
            true,
        ),
        ("contains([m.Mode.First],m.Other.First)", false),
    ] {
        let mut source = DefinitionSource::new(
            "memory:enum-membership-behavior.rcore",
            SourceKind::RcoreBehavior,
            format!(
                "rcore 1;package enumeration version \"1\";import model \"urn:membership-enum\" as m;fn run()->Bool={body};"
            ),
        );
        source.dependencies.push(model.id.clone());
        let bundle = DefinitionBundle {
            version: 1,
            sources: vec![model.clone(), source.clone()],
            namespaces: vec![],
        };
        let compiled = compile_function(
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
        );
        assert_eq!(compiled.is_ok(), valid, "{body}: {compiled:?}");
        if valid {
            let image = compiled.unwrap();
            let result = evaluate(
                &image,
                &[],
                &ExecutionContext::new(&bundle),
                &ExecutionLimits::default(),
            )
            .unwrap();
            let DataWire::Scalar(value) = result.value.data else {
                panic!("Bool")
            };
            assert_eq!(value.payload, serde_json::json!(true));
        }
    }
}
