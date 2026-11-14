use intersect_model::definitions::{
    bundle::{DefinitionBundle, DefinitionSource, SourceKind},
    rcore_behavior::{
        function_lowering::compile_function, ir_v2::*, modules::SymbolId, types::Scalar,
    },
};

fn fixture(body: &str) -> (DefinitionBundle, Program) {
    let source = DefinitionSource::new(
        "memory:adversarial-behavior.rcore",
        SourceKind::RcoreBehavior,
        format!("rcore 1; package adversarial version \"1\"; {body}"),
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
    .unwrap_or_else(|e| panic!("{body}: {e:?}"));
    (bundle, image.program().clone())
}
fn rejects(bundle: &DefinitionBundle, program: Program, diagnostic: &str) {
    let error = verify(program, bundle, &Limits::default()).unwrap_err();
    assert!(
        error.message.contains(diagnostic),
        "Expected {diagnostic:?}, got {error:?}"
    );
}
fn scalar(program: &Program, value: Scalar) -> TypeId {
    TypeId(
        program
            .types
            .iter()
            .position(|t| *t == TypeNode::Scalar(value))
            .unwrap() as u32,
    )
}
fn instruction(
    program: &mut Program,
    predicate: impl Fn(&InstructionKind) -> bool,
) -> &mut Instruction {
    program
        .functions
        .iter_mut()
        .flat_map(|f| &mut f.blocks)
        .flat_map(|b| &mut b.instructions)
        .find(|i| predicate(&i.kind))
        .unwrap()
}

#[test]
fn calls_and_operand_indices_reject_at_their_own_validation_boundary() {
    let (bundle, baseline) =
        fixture("fn main(x:Int8,b:Bool)->Int8=keep(x);fn keep(x:Int8)->Int8=x;");
    let mut bad = baseline.clone();
    if let InstructionKind::MakeClosure { function, .. } = &mut instruction(&mut bad, |k| {
        matches!(k, InstructionKind::MakeClosure { .. })
    })
    .kind
    {
        *function = FunctionId(u32::MAX);
    }
    rejects(&bundle, bad, "Unknown closure function");
    let mut bad = baseline.clone();
    if let InstructionKind::Call { arguments, .. } =
        &mut instruction(&mut bad, |k| matches!(k, InstructionKind::Call { .. })).kind
    {
        arguments.clear();
    }
    rejects(&bundle, bad, "Call argument arity mismatch");
    let mut bad = baseline.clone();
    if let InstructionKind::Call { callee, .. } =
        &mut instruction(&mut bad, |k| matches!(k, InstructionKind::Call { .. })).kind
    {
        *callee = Operand::Parameter(0);
    }
    rejects(&bundle, bad, "Call operand is not callable");
    for operand in [
        Operand::Parameter(999),
        Operand::Register(u32::MAX),
        Operand::Capture(0),
    ] {
        let mut bad = baseline.clone();
        if let InstructionKind::Call { arguments, .. } =
            &mut instruction(&mut bad, |k| matches!(k, InstructionKind::Call { .. })).kind
        {
            arguments[0] = operand;
        }
        rejects(&bundle, bad, "Unknown or not-yet-defined function operand");
    }
    let mut bad = baseline.clone();
    if let InstructionKind::Call { arguments, .. } =
        &mut instruction(&mut bad, |k| matches!(k, InstructionKind::Call { .. })).kind
    {
        arguments[0] = Operand::Parameter(1);
    }
    rejects(&bundle, bad, "Incompatible function value types");
    let mut bad = baseline.clone();
    let boolean = scalar(&bad, Scalar::Bool);
    instruction(&mut bad, |k| matches!(k, InstructionKind::Call { .. })).ty = boolean;
    rejects(&bundle, bad, "Incompatible function value types");
    verify(baseline, &bundle, &Limits::default()).unwrap();
}

#[test]
fn capture_arity_types_order_and_lexical_owners_are_checked_independently() {
    let (bundle, baseline) =
        fixture("fn main(x:Int8,b:Bool)->Int8={let f=|y:Int8|if b {x+y} else {x};f(1)};");
    let mut bad = baseline.clone();
    if let InstructionKind::MakeClosure { captures, .. } = &mut instruction(&mut bad, |k| {
        matches!(k, InstructionKind::MakeClosure { .. })
    })
    .kind
    {
        assert_eq!(captures.len(), 2);
        captures.pop();
    }
    rejects(&bundle, bad, "Closure signature or capture arity mismatch");
    let mut bad = baseline.clone();
    if let InstructionKind::MakeClosure { captures, .. } = &mut instruction(&mut bad, |k| {
        matches!(k, InstructionKind::MakeClosure { .. })
    })
    .kind
    {
        captures.swap(0, 1);
    }
    rejects(&bundle, bad, "Incompatible function value types");
    let mut bad = baseline.clone();
    let lambda = bad
        .functions
        .iter_mut()
        .find(|f| matches!(f.identity, FunctionIdentity::Lambda { .. }))
        .unwrap();
    lambda.captures.swap(0, 1);
    // Preserve creation type checking so this mutation reaches the fresh-source capture order check.
    if let InstructionKind::MakeClosure { captures, .. } = &mut instruction(&mut bad, |k| {
        matches!(k, InstructionKind::MakeClosure { .. })
    })
    .kind
    {
        captures.swap(0, 1);
    }
    rejects(
        &bundle,
        bad,
        "Function signature or captures differ from fresh source",
    );
    let mut bad = baseline.clone();
    let (index, lambda) = bad
        .functions
        .iter_mut()
        .enumerate()
        .find(|(_, f)| matches!(f.identity, FunctionIdentity::Lambda { .. }))
        .unwrap();
    if let FunctionIdentity::Lambda { enclosing, .. } = &mut lambda.identity {
        *enclosing = FunctionId(index as u32);
    }
    rejects(&bundle, bad, "Lambda enclosing function must precede it");
    let (nested_bundle, nested) =
        fixture("fn main(x:Int8)->Int8={let f=|y:Int8| |z:Int8|x+y+z;f(1)(2)};");
    let mut bad = nested.clone();
    let entry = bad.entry;
    let inner = bad
        .functions
        .iter_mut()
        .rev()
        .find(|f| matches!(f.identity, FunctionIdentity::Lambda { .. }))
        .unwrap();
    if let FunctionIdentity::Lambda { enclosing, .. } = &mut inner.identity {
        assert_ne!(*enclosing, entry);
        *enclosing = entry;
    }
    rejects(
        &nested_bundle,
        bad,
        "Lambda does not have the exact lexical origin",
    );
    verify(baseline, &bundle, &Limits::default()).unwrap();
    verify(nested, &nested_bundle, &Limits::default()).unwrap();
}

#[test]
fn source_signatures_duplicate_instances_and_type_graphs_cannot_be_forged() {
    let (bundle, baseline) = fixture("fn main(x:Int8,b:Bool)->Int8=x;");
    for result in [false, true] {
        let mut bad = baseline.clone();
        let boolean = scalar(&bad, Scalar::Bool);
        let entry = bad.entry.0 as usize;
        if result {
            bad.functions[entry].result = boolean;
        } else {
            bad.functions[entry].parameters[0] = boolean;
            bad.functions[entry].blocks[0].parameters[0] = boolean;
        }
        rejects(
            &bundle,
            bad,
            "Function signature or captures differ from fresh source",
        );
    }
    let mut bad = baseline.clone();
    bad.functions.push(bad.functions[0].clone());
    rejects(&bundle, bad, "Duplicate function identity");
    let mut bad = baseline.clone();
    bad.types.push(TypeNode::Option(TypeId(u32::MAX)));
    rejects(&bundle, bad, "Type children must precede their parent");
    let mut bad = baseline.clone();
    let next = TypeId(bad.types.len() as u32);
    bad.types.push(TypeNode::Option(next));
    rejects(&bundle, bad, "Type children must precede their parent");
    let mut bad = baseline.clone();
    bad.entry = FunctionId(u32::MAX);
    rejects(&bundle, bad, "Unknown entry function");
    let mut bad = baseline.clone();
    bad.version = 999;
    rejects(&bundle, bad, "Unsupported function IR version");
    let mut wire = serde_json::to_value(&baseline).unwrap();
    wire["functions"][0]["effect"] = serde_json::json!("HostIo");
    assert!(serde_json::from_value::<Program>(wire).is_err());
    verify(baseline, &bundle, &Limits::default()).unwrap();
}

#[test]
fn forward_edges_arity_and_implicit_option_payloads_are_verified() {
    let (bundle, baseline) = fixture("fn main(x:Int8,b:Bool)->Int8=if b {x} else {1};");
    let mut bad = baseline.clone();
    let entry = bad.entry.0 as usize;
    bad.functions[entry].blocks[0].terminator = Terminator::Jump(Edge {
        block: 0,
        arguments: vec![],
    });
    rejects(&bundle, bad, "Function block edges must be forward");
    let mut bad = baseline.clone();
    bad.functions[entry].blocks[0].terminator = Terminator::Jump(Edge {
        block: u32::MAX,
        arguments: vec![],
    });
    rejects(&bundle, bad, "Unknown function block");
    let mut bad = baseline.clone();
    if let Terminator::Branch { then_edge, .. } = &mut bad.functions[entry].blocks[0].terminator {
        then_edge.arguments.push(Operand::Parameter(0));
    } else {
        panic!("Expected branch")
    }
    rejects(&bundle, bad, "Function edge arity mismatch");
    let (option_bundle, option) =
        fixture("fn main(x:Int8?,b:Bool)->Int8=match x {Some(y)=>y,None=>0};");
    let mut bad = option.clone();
    let boolean = scalar(&bad, Scalar::Bool);
    let entry = bad.entry.0 as usize;
    let target = bad.functions[entry]
        .blocks
        .iter()
        .find_map(|b| match &b.terminator {
            Terminator::Option { some, .. } => Some(some.block),
            _ => None,
        })
        .unwrap();
    bad.functions[entry].blocks[target as usize].parameters[0] = boolean;
    rejects(&option_bundle, bad, "Incompatible function value types");
    verify(baseline, &bundle, &Limits::default()).unwrap();
    verify(option, &option_bundle, &Limits::default()).unwrap();
}

#[test]
fn serialized_cancellation_and_effect_claims_cannot_extend_executable_authority() {
    let (bundle, baseline) = fixture("fn main()->Int8=42;");
    assert!(!baseline.functions[0].blocks[0].instructions.is_empty());
    for level in 0..3 {
        for (field, claim) in [
            ("cancelled", serde_json::json!(false)),
            (
                "cancellation_token",
                serde_json::json!("trusted-host-token"),
            ),
            ("effects", serde_json::json!(["HostIo"])),
        ] {
            let mut wire = serde_json::to_value(&baseline).unwrap();
            let target = match level {
                0 => &mut wire,
                1 => &mut wire["functions"][0],
                _ => &mut wire["functions"][0]["blocks"][0]["instructions"][0],
            };
            target[field] = claim;
            let error = serde_json::from_value::<Program>(wire).unwrap_err();
            assert!(
                error.to_string().contains("unknown field"),
                "{level}/{field}: {error}"
            );
            assert!(
                error.to_string().contains(field),
                "{level}/{field}: {error}"
            );
        }
    }
    let decoded: Program =
        serde_json::from_value(serde_json::to_value(&baseline).unwrap()).unwrap();
    assert_eq!(
        verify(decoded, &bundle, &Limits::default())
            .unwrap()
            .program(),
        &baseline
    );
}

#[test]
fn every_collection_callback_operation_rejects_independent_hostile_ir_mutations() {
    for (name, callback, output) in [
        ("map", "(Int)->Int", "Sequence<Int>"),
        ("filter", "(Int)->Bool", "Sequence<Int>"),
        ("flatMap", "(Int)->Sequence<Int>", "Sequence<Int>"),
        ("fold", "(Int,Int)->Int", "Int"),
        ("exists", "(Int)->Bool", "Bool"),
        ("forAll", "(Int)->Bool", "Bool"),
        ("one", "(Int)->Bool", "Bool"),
        ("find", "(Int)->Bool", "Option<Int>"),
        ("groupBy", "(Int)->Int", "Sequence<(Int,Sequence<Int>)>"),
        ("sortedBy", "(Int)->Int", "Sequence<Int>"),
        ("sortedWith", "(Int,Int)->Int", "Sequence<Int>"),
    ] {
        let arguments = if name == "fold" { "xs,n,f" } else { "xs,f" };
        let domain = callback.replacen("(Int", "(Bool", 1);
        let result = format!("{}->(Int,Int)", callback.split("->").next().unwrap());
        let (bundle, baseline) = fixture(&format!(
            "fn main(b:Bool,xs:Sequence<Int>,f:{callback},n:Int,wrong:(Int,Int,Int)->Int,domain:{domain},result:{result})->{output} ={name}({arguments});"
        ));
        verify(baseline.clone(), &bundle, &Limits::default()).unwrap();
        let locate = |k: &InstructionKind| matches!(k, InstructionKind::CollectionCall { .. });
        for mutation in 0..10 {
            let mut bad = baseline.clone();
            let wrong_result = TypeId(bad.types.len() as u32);
            bad.types.push(TypeNode::Scalar(Scalar::String));
            let call = instruction(&mut bad, locate);
            let InstructionKind::CollectionCall {
                value,
                callback,
                seed,
                ..
            } = &mut call.kind
            else {
                unreachable!()
            };
            let message = match mutation {
                0 => {
                    *value = Operand::Parameter(0);
                    "Collection call requires collection input"
                }
                1 => {
                    *value = Operand::Register(u32::MAX);
                    "Unknown or not-yet-defined function operand"
                }
                2 => {
                    *callback = Operand::Parameter(0);
                    "Collection callback must be callable"
                }
                3 => {
                    *callback = Operand::Parameter(4);
                    "Collection callback or seed arity mismatch"
                }
                4 => {
                    *seed = if seed.is_some() {
                        None
                    } else {
                        Some(Operand::Parameter(3))
                    };
                    "Collection callback or seed arity mismatch"
                }
                5 => {
                    call.ty = wrong_result;
                    if name == "groupBy" {
                        "Grouping requires a concrete outer collection"
                    } else {
                        "Collection operation result contract mismatch"
                    }
                }
                6 => {
                    call.span.end = usize::MAX;
                    "Invalid function IR span"
                }
                7 => {
                    call.span.source.uri = "memory:foreign-behavior.rcore".into();
                    "Unknown instruction source"
                }
                8 => {
                    *callback = Operand::Parameter(5);
                    "Incompatible function value types"
                }
                _ => {
                    *callback = Operand::Parameter(6);
                    match name {
                        "filter" | "exists" | "forAll" | "one" | "find" => {
                            "Collection predicate must return Bool"
                        }
                        "flatMap" => "FlatMap callback must return collection",
                        "fold" => "Incompatible function value types",
                        "groupBy" => "Grouping key/member contract mismatch",
                        "sortedBy" => "Sorting key must be a scalar",
                        "sortedWith" => "Sorting comparator must return Int",
                        _ => "Collection operation result contract mismatch",
                    }
                }
            };
            let error = verify(bad, &bundle, &Limits::default()).unwrap_err();
            assert!(
                error.message.contains(message),
                "{name} mutation {mutation}: expected {message}, got {error:?}"
            );
        }
        let mut wire = serde_json::to_value(&baseline).unwrap();
        let call = wire["functions"]
            .as_array_mut()
            .unwrap()
            .iter_mut()
            .flat_map(|f| f["blocks"].as_array_mut().unwrap())
            .flat_map(|b| b["instructions"].as_array_mut().unwrap())
            .find(|i| i["kind"].get("CollectionCall").is_some())
            .unwrap();
        call["kind"]["CollectionCall"]["operation"] =
            serde_json::json!("UnknownCollectionOperation");
        assert!(
            serde_json::from_value::<Program>(wire)
                .unwrap_err()
                .to_string()
                .contains("unknown variant")
        );
        let mut wire = serde_json::to_value(&baseline).unwrap();
        wire["functions"][0]["blocks"][0]["instructions"][0]["unexpected"] =
            serde_json::json!(true);
        assert!(
            serde_json::from_value::<Program>(wire)
                .unwrap_err()
                .to_string()
                .contains("unknown field")
        );
        assert_eq!(
            verify(baseline.clone(), &bundle, &Limits::default())
                .unwrap()
                .program(),
            &baseline
        );
    }
}

#[test]
fn collection_type_verification_does_not_claim_exact_source_replay() {
    for (body, replacement) in [
        (
            "fn main()->Bool=exists([1,2],|x:Int|x==1);",
            CollectionOp::ForAll,
        ),
        (
            "fn main()->Sequence<Int> =map([3,1,2],|x:Int|0-x);",
            CollectionOp::SortedBy,
        ),
    ] {
        let (bundle, baseline) = fixture(body);
        let mut altered = baseline.clone();
        let call = instruction(&mut altered, |k| {
            matches!(k, InstructionKind::CollectionCall { .. })
        });
        let InstructionKind::CollectionCall { operation, .. } = &mut call.kind else {
            unreachable!()
        };
        *operation = replacement;
        // This is deliberately still well-typed IR. Exact source replay is a
        // distinct host contract, exercised by the native/WASM browser control.
        assert_ne!(baseline, altered);
        assert!(verify(altered, &bundle, &Limits::default()).is_ok());
    }
}

#[test]
fn binary_collection_callbacks_validate_second_parameter_and_fold_seed() {
    for name in ["fold", "sortedWith"] {
        let output = if name == "fold" {
            "Int"
        } else {
            "Sequence<Int>"
        };
        let arguments = if name == "fold" { "xs,n,f" } else { "xs,f" };
        let (bundle, baseline) = fixture(&format!(
            "fn main(xs:Sequence<Int>,n:Int,b:Bool,f:(Int,Int)->Int,wrong:(Int,Bool)->Int)->{output} ={name}({arguments});"
        ));
        let mut bad = baseline.clone();
        let call = instruction(&mut bad, |k| {
            matches!(k, InstructionKind::CollectionCall { .. })
        });
        let InstructionKind::CollectionCall { callback, .. } = &mut call.kind else {
            unreachable!()
        };
        *callback = Operand::Parameter(4);
        rejects(&bundle, bad, "Incompatible function value types");
        if name == "fold" {
            let mut bad = baseline.clone();
            let call = instruction(&mut bad, |k| {
                matches!(k, InstructionKind::CollectionCall { .. })
            });
            let InstructionKind::CollectionCall { seed, .. } = &mut call.kind else {
                unreachable!()
            };
            *seed = Some(Operand::Parameter(2));
            rejects(&bundle, bad, "Incompatible function value types");
        }
        assert!(verify(baseline, &bundle, &Limits::default()).is_ok());
    }
}

#[test]
fn comprehension_predicate_and_in_bounds_span_changes_need_exact_replay() {
    let (bundle, baseline) =
        fixture("fn main()->Sequence<Int> =[for x in [1,2] where true yield x];");
    for change_span in [false, true] {
        let mut altered = baseline.clone();
        let constant = instruction(
            &mut altered,
            |kind| matches!(kind,InstructionKind::Constant(ValueWire {data:DataWire::Scalar(value),..}) if value.payload==serde_json::json!(true)),
        );
        if change_span {
            constant.span.start = 0;
            constant.span.end = 0;
        } else {
            let InstructionKind::Constant(ValueWire {
                data: DataWire::Scalar(value),
                ..
            }) = &mut constant.kind
            else {
                unreachable!()
            };
            value.payload = serde_json::json!(false);
        }
        assert_ne!(altered, baseline);
        // Both changes satisfy typed-IR authority; only fresh source replay can
        // establish the exact original predicate and its instruction provenance.
        assert!(verify(altered, &bundle, &Limits::default()).is_ok());
        assert!(verify(baseline.clone(), &bundle, &Limits::default()).is_ok());
    }
}
