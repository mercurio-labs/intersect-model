use intersect_model::definitions::{
    bundle::{DefinitionBundle, DefinitionSource, SourceKind},
    rcore_behavior::{
        function_lowering::compile_function,
        ir_v2::{self, DataWire, EntrySelection, InstructionKind, Limits, Operand, TypeNode},
        modules::SymbolId,
        types::Scalar,
    },
};
use intersect_model::rcore_behavior_functions::{
    ExecutionContext, ExecutionErrorKind, ExecutionLimits, evaluate,
};

fn fixture(body: &str) -> (DefinitionBundle, DefinitionSource) {
    let source = DefinitionSource::new(
        "memory:measures-behavior.rcore",
        SourceKind::RcoreBehavior,
        format!("rcore 1; package measures version \"1\"; {body}"),
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
fn run(body: &str) -> serde_json::Value {
    let (bundle, source) = fixture(body);
    let image = compile(&bundle, &source);
    let result = evaluate(
        &image,
        &[],
        &ExecutionContext::new(&bundle),
        &ExecutionLimits::default(),
    )
    .unwrap();
    let DataWire::Scalar(value) = result.value.data else {
        panic!("Expected scalar")
    };
    value.payload
}
#[test]
fn cardinality_observes_occurrences_and_unique_representatives_for_each_kind() {
    for (kind, size) in [("Sequence", 3), ("Bag", 3), ("Set", 2), ("OrderedSet", 2)] {
        assert_eq!(
            run(&format!("fn run()->Int=size({kind}([1,1,2]));")),
            serde_json::json!({"kind":"int64","value":size.to_string()})
        );
        assert_eq!(
            run(&format!("fn run()->Bool=isEmpty({kind}::<Int>([]));")),
            true
        );
        assert_eq!(run(&format!("fn run()->Bool=isEmpty({kind}([1]));")), false);
    }
    // Measure the outer collection, not nested values, and do not coerce None.
    assert_eq!(
        run(
            "fn run()->Int={let xs:Sequence<Option<Sequence<Int>>> =[Some([1,2]),None,Some([])];size(xs)};"
        ),
        serde_json::json!({"kind":"int64","value":"3"})
    );
}
#[test]
fn generic_functions_closures_and_lexical_shadowing_keep_normal_binding() {
    for body in [
        "fn run()->Int=(size)([1,2,3]);",
        "fn run()->Int=count([1,2,3]);fn count<T>(xs:Sequence<T>)->Int=size(xs);",
        "fn run()->Int={let xs=[1,2,3];let f=|ignored:Bool|size(xs);f(true)};",
        "fn run()->Int={let size=|x:Int|x;size(3)};",
        "fn run()->Int=size(3);fn size(x:Int)->Int=x;",
        "fn run()->Int=if isEmpty(Sequence::<Int>([])) {size([1,2,3])} else {1 / 0};",
    ] {
        assert_eq!(run(body), serde_json::json!({"kind":"int64","value":"3"}));
    }
}
#[test]
fn invalid_measures_are_located_static_errors_even_in_dead_branches() {
    for body in [
        "fn run()->Int=size(1);",
        "fn run()->Bool=isEmpty(\"abc\");",
        "fn run()->Int=size((1,2));",
        "fn run()->Int=size(None::<Int>());",
        "fn run()->Int=size();",
        "fn run()->Int=size([1],[2]);",
        "fn run()->Int=size::<Int>([1]);",
        "fn run()->Bool=isEmpty::<Int>([1]);",
        "fn run()->Int8=size([1]);",
        "fn run()->Int=size([]);",
        "fn run()->Int=if true {0} else {size(false)};",
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
                .all(|e| e.span.source == source.id && e.span.end <= source.text.len()),
            "{errors:?}"
        );
    }
}
#[test]
fn measure_ir_rejects_wrong_operands_and_result_types() {
    let (bundle, source) = fixture("fn run()->Int=size([1,2]);");
    let image = compile(&bundle, &source);
    for wrong_operand in [true, false] {
        let mut program = image.program().clone();
        let instruction = program
            .functions
            .iter_mut()
            .flat_map(|f| &mut f.blocks)
            .flat_map(|b| &mut b.instructions)
            .find(|i| matches!(i.kind, InstructionKind::CollectionSize { .. }))
            .unwrap();
        if wrong_operand {
            instruction.kind = InstructionKind::CollectionSize {
                value: Operand::Parameter(42),
            };
        } else {
            instruction.kind = match instruction.kind {
                InstructionKind::CollectionSize { value } => {
                    InstructionKind::CollectionIsEmpty { value }
                }
                _ => unreachable!(),
            };
        }
        assert!(ir_v2::verify(program, &bundle, &Limits::default()).is_err());
    }
    assert!(
        image
            .program()
            .types
            .contains(&TypeNode::Scalar(Scalar::Int64))
    );
}
#[test]
fn measures_use_common_execution_limits_and_recover_after_exhaustion() {
    let (bundle, source) = fixture("fn run()->Int=size([1,2,3]);");
    let image = compile(&bundle, &source);
    let mut limits = ExecutionLimits::default();
    limits.scalar.max_steps = 0;
    assert_eq!(
        evaluate(&image, &[], &ExecutionContext::new(&bundle), &limits)
            .unwrap_err()
            .kind,
        ExecutionErrorKind::ResourceExhausted
    );
    let result = evaluate(
        &image,
        &[],
        &ExecutionContext::new(&bundle),
        &ExecutionLimits::default(),
    )
    .unwrap();
    assert!(matches!(result.value.data, DataWire::Scalar(_)));
    for steps in [
        result.consumed.steps - 1,
        result.consumed.steps,
        result.consumed.steps + 1,
    ] {
        limits.scalar.max_steps = steps;
        let attempt = evaluate(&image, &[], &ExecutionContext::new(&bundle), &limits);
        assert_eq!(attempt.is_ok(), steps >= result.consumed.steps);
    }
}

#[test]
fn verified_many_operands_preserve_bounds_and_all_order_unique_contracts() {
    for ordered in [false, true] {
        for unique in [false, true] {
            for (operation, expected) in [
                ("size", serde_json::json!({"kind":"int64","value":"2"})),
                ("isEmpty", serde_json::json!(false)),
            ] {
                let result_type = if operation == "size" { "Int" } else { "Bool" };
                let collection = match (ordered, unique) {
                    (true, false) => "Sequence",
                    (true, true) => "OrderedSet",
                    (false, false) => "Bag",
                    (false, true) => "Set",
                };
                let (bundle, source) = fixture(&format!(
                    "fn run<T:{collection}<Int>>(xs:T)->{result_type}={operation}(xs);"
                ));
                // Many is a concrete generic entry type, retaining fresh source evidence.
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
                                element: ir_v2::TypeId(0),
                                lower: 1,
                                upper: 2,
                                ordered,
                                unique,
                            },
                        ],
                        arguments: vec![ir_v2::TypeId(1)],
                    },
                    &Limits::default(),
                )
                .unwrap();
                let program = image.program();
                let parameter = program.functions[program.entry.0 as usize].parameters[0];
                let TypeNode::Many { element, .. } = program.types[parameter.0 as usize] else {
                    panic!("Expected Many")
                };
                let values = |numbers: &[i64]| {
                    ir_v2::ValueWire { ty: parameter, data: DataWire::Collection(numbers.iter().map(|n| ir_v2::ValueWire {
                    ty: element, data: DataWire::Scalar(intersect_model::definitions::rcore_behavior::ir::Value {
                        ty: intersect_model::definitions::rcore_behavior::ir::ValueType::Scalar(Scalar::Int64), payload: serde_json::json!({"kind":"int64","value":n.to_string()}),
                    }),
                }).collect()) }
                };
                let result = evaluate(
                    &image,
                    &[values(&[1, 2])],
                    &ExecutionContext::new(&bundle),
                    &ExecutionLimits::default(),
                )
                .unwrap();
                let DataWire::Scalar(value) = result.value.data else {
                    panic!("Expected scalar")
                };
                assert_eq!(value.payload, expected);
                for bad in [&[][..], &[1, 2, 3][..]] {
                    assert!(
                        evaluate(
                            &image,
                            &[values(bad)],
                            &ExecutionContext::new(&bundle),
                            &ExecutionLimits::default()
                        )
                        .is_err()
                    );
                }
                let duplicates = evaluate(
                    &image,
                    &[values(&[1, 1])],
                    &ExecutionContext::new(&bundle),
                    &ExecutionLimits::default(),
                );
                assert_eq!(duplicates.is_err(), unique);
            }
        }
    }
}
