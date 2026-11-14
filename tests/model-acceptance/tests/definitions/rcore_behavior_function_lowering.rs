use intersect_model::definitions::{
    bundle::{DefinitionBundle, DefinitionSource, SourceKind},
    rcore_behavior::{
        function_lowering::compile_function,
        ir_v2::{self, EntrySelection, FunctionIdentity, Limits, TypeId, TypeNode},
        modules::SymbolId,
        types::Scalar,
    },
};
fn source(name: &str, body: &str) -> DefinitionSource {
    DefinitionSource::new(
        format!("memory:{name}-behavior.rcore"),
        SourceKind::RcoreBehavior,
        format!("rcore 1; package {name} version \"1\"; {body}"),
    )
}
fn bundle(sources: Vec<DefinitionSource>) -> DefinitionBundle {
    DefinitionBundle {
        version: 1,
        sources,
        namespaces: vec![],
    }
}
fn compile(body: &str, declaration: usize) -> ir_v2::VerifiedExecutable {
    let entry = source("functions", body);
    let selection = EntrySelection {
        symbol: SymbolId {
            source: entry.id.clone(),
            declaration,
        },
        types: vec![],
        arguments: vec![],
    };
    compile_function(
        &bundle(vec![entry.clone()]),
        &entry.id,
        &selection,
        &Limits::default(),
    )
    .unwrap_or_else(|errors| panic!("{body}: {errors:?}"))
}
#[test]
fn named_imported_and_generic_calls_retain_exact_instances_and_replay() {
    let helper = source("helper", "pub fn keep<T,U>(x:T)->T=x;");
    let mut entry = source(
        "functions",
        "import package helper version \"1\" as h; fn main()->Int8=(h.keep)::<Int8,Bool>(7);",
    );
    entry.dependencies.push(helper.id.clone());
    let selection = EntrySelection {
        symbol: SymbolId {
            source: entry.id.clone(),
            declaration: 1,
        },
        types: vec![],
        arguments: vec![],
    };
    let image = compile_function(
        &bundle(vec![entry.clone(), helper.clone()]),
        &entry.id,
        &selection,
        &Limits::default(),
    )
    .unwrap();
    let function=image.program().functions.iter().find(|f|matches!(&f.identity,FunctionIdentity::Named{symbol,..} if symbol.source==helper.id)).unwrap();
    let FunctionIdentity::Named { type_arguments, .. } = &function.identity else {
        panic!("Expected named instance")
    };
    assert_eq!(type_arguments.len(), 2);
    assert_eq!(
        image.program().types[type_arguments[1].0 as usize],
        TypeNode::Scalar(Scalar::Bool)
    );
    let replayed: DefinitionBundle =
        serde_json::from_value(serde_json::to_value(bundle(vec![helper, entry.clone()])).unwrap())
            .unwrap();
    let fresh = compile_function(&replayed, &entry.id, &selection, &Limits::default()).unwrap();
    assert_eq!(image.program(), fresh.program());
}
#[test]
fn returned_nested_closures_capture_exact_transitive_lexical_values() {
    let image = compile(
        "fn make(x:Int)->(Int)->(Int)->Int=|y:Int| |z:Int|x+y+z; fn main()->Int={let first=make(1);let second=first(2);second(3)};",
        1,
    );
    let lambdas: Vec<_> = image
        .program()
        .functions
        .iter()
        .filter(|f| matches!(f.identity, FunctionIdentity::Lambda { .. }))
        .collect();
    assert_eq!(lambdas.len(), 2);
    let counts: Vec<_> = lambdas.iter().map(|f| f.captures.len()).collect();
    assert_eq!(counts, vec![1, 2]);
    for function in lambdas {
        assert!(
            function
                .captures
                .windows(2)
                .all(|pair| pair[0].declaration.start < pair[1].declaration.start)
        );
    }
    compile(
        "fn main()->Int={let x=1;let outer=|y:Int|x+y;let inner={let x=10;|z:Int|outer(z)+x};inner(2)};",
        0,
    );
}
#[test]
fn structural_constructors_access_and_nested_matches_share_function_images() {
    for body in [
        "fn main()->Int={let x={a:(1,true),b:Some(3)};x.a[0]+(x.b??2)};",
        "fn main(x:Result<Int?,String>)->Int=match x {Ok(Some(value))=>value,Ok(none)=>0,Err(problem)=>if problem==\"x\"{1}else{2}};",
        "fn main()->Int?={let values=OrderedSet([1,1,2]);values[1]};",
        "fn main()->Bag<Int> =Bag(Set([1,2,1]));",
        "fn main(x:{value:Int}?)->Int?=x?.value;",
        "fn main(x:{value:Int?}?)->Int?=x?.value;",
        "fn apply<T>(f:(T)->T,x:T)->T=f(x);fn id<U>(x:U)->U=x;fn main()->Int=apply(id,3);",
    ] {
        compile(body, if body.contains("fn apply") { 2 } else { 0 });
    }
}
#[test]
fn explicit_generic_entry_retains_unused_actuals_and_checks_bounds() {
    let entry = source("entry", "fn keep<T:Int8,U>(x:T)->T=x;");
    let mut selection = EntrySelection {
        symbol: SymbolId {
            source: entry.id.clone(),
            declaration: 0,
        },
        types: vec![
            TypeNode::Scalar(Scalar::Int8),
            TypeNode::Scalar(Scalar::Bool),
        ],
        arguments: vec![TypeId(0), TypeId(1)],
    };
    let definitions = bundle(vec![entry.clone()]);
    let image = compile_function(&definitions, &entry.id, &selection, &Limits::default()).unwrap();
    assert!(
        matches!(&image.program().functions[0].identity,FunctionIdentity::Named{type_arguments,..} if type_arguments.len()==2)
    );
    selection.arguments = vec![TypeId(1), TypeId(0)];
    assert!(compile_function(&definitions, &entry.id, &selection, &Limits::default()).is_err());
    selection.arguments.clear();
    assert!(compile_function(&definitions, &entry.id, &selection, &Limits::default()).is_err());
}
#[test]
fn source_errors_remain_complete_and_dead_unsupported_navigation_rejects() {
    let entry = source("invalid", "fn first()->Int=true;fn second()->Bool=1;");
    let selection = EntrySelection {
        symbol: SymbolId {
            source: entry.id.clone(),
            declaration: 0,
        },
        types: vec![],
        arguments: vec![],
    };
    let errors = compile_function(
        &bundle(vec![entry.clone()]),
        &entry.id,
        &selection,
        &Limits::default(),
    )
    .unwrap_err();
    assert!(errors.len() >= 2, "{errors:?}");
    let model = intersect_model_acceptance::migration_fixture::migrate_source_fixture(DefinitionSource::new(
        "memory:model.ecore",
        SourceKind::Ecore,
        r##"<e:EPackage xmlns:e="http://www.eclipse.org/emf/2002/Ecore" xmlns:x="http://www.w3.org/2001/XMLSchema-instance" name="m" nsURI="urn:functions"><eClassifiers x:type="e:EClass" name="Item"><eStructuralFeatures x:type="e:EAttribute" name="name" derived="true" lowerBound="1" eType="e:EDataType http://www.eclipse.org/emf/2002/Ecore#//EString"/></eClassifiers></e:EPackage>"##,
    )).0;
    let mut entry = source(
        "model_entry",
        "import model \"urn:functions\" as m;fn main(x:m.Item)->String=if true{\"ok\"}else{x.name};",
    );
    entry.dependencies.push(model.id.clone());
    let selection = EntrySelection {
        symbol: SymbolId {
            source: entry.id.clone(),
            declaration: 1,
        },
        types: vec![],
        arguments: vec![],
    };
    let errors = compile_function(
        &bundle(vec![entry.clone(), model]),
        &entry.id,
        &selection,
        &Limits::default(),
    )
    .unwrap_err();
    assert!(
        errors
            .iter()
            .any(|e| e.message.contains("Derived model navigation")),
        "{errors:?}"
    );
}
#[test]
fn function_limits_and_invalid_external_type_graph_fail_before_publication() {
    let entry = source(
        "limits",
        "fn helper(x:Int)->Int=x;fn main()->Int=helper(1);",
    );
    let definitions = bundle(vec![entry.clone()]);
    let mut selection = EntrySelection {
        symbol: SymbolId {
            source: entry.id.clone(),
            declaration: 1,
        },
        types: vec![],
        arguments: vec![],
    };
    let limits = Limits {
        max_functions: 1,
        ..Limits::default()
    };
    assert!(compile_function(&definitions, &entry.id, &selection, &limits).is_err());
    compile_function(&definitions, &entry.id, &selection, &Limits::default()).unwrap();
    selection.types = vec![TypeNode::Option(TypeId(0))];
    assert!(compile_function(&definitions, &entry.id, &selection, &Limits::default()).is_err());
}

#[test]
fn empty_enum_and_uninhabited_variant_branches_have_no_fabricated_return_value() {
    let model = intersect_model_acceptance::migration_fixture::migrate_source_fixture(DefinitionSource::new(
        "memory:empty.ecore",
        SourceKind::Ecore,
        r##"<e:EPackage xmlns:e="http://www.eclipse.org/emf/2002/Ecore" xmlns:x="http://www.w3.org/2001/XMLSchema-instance" name="m" nsURI="urn:empty"><eClassifiers x:type="e:EEnum" name="Empty"/></e:EPackage>"##,
    )).0;
    for body in [
        "fn main(x:m.Empty)->Int=match x {};",
        "fn main(x:m.Empty?)->Int=match x {none=>7};",
        "fn main(x:Result<m.Empty,Int>)->Int=match x {Err(value)=>value};",
        "fn main(x:m.Empty)->Int=1+(match x {});",
    ] {
        let mut entry = source(
            "empty_entry",
            &format!("import model \"urn:empty\" as m;{body}"),
        );
        entry.dependencies.push(model.id.clone());
        let selection = EntrySelection {
            symbol: SymbolId {
                source: entry.id.clone(),
                declaration: 1,
            },
            types: vec![],
            arguments: vec![],
        };
        let image = compile_function(
            &bundle(vec![entry.clone(), model.clone()]),
            &entry.id,
            &selection,
            &Limits::default(),
        )
        .unwrap_or_else(|errors| panic!("{body}: {errors:?}"));
        assert!(
            image.program().functions[0]
                .blocks
                .iter()
                .any(|block| matches!(block.terminator, ir_v2::Terminator::Absurd { .. })),
            "{body}"
        );
    }
}

#[test]
fn grouped_contextual_generic_values_and_bound_collection_index_keep_source_types() {
    compile(
        "fn id<T>(x:T)->T=x;fn main()->Int={let alias:(Int)->Int=((id));alias(3)};",
        1,
    );
    let model = intersect_model_acceptance::migration_fixture::migrate_source_fixture(DefinitionSource::new(
        "memory:bounds.ecore",
        SourceKind::Ecore,
        r##"<e:EPackage xmlns:e="http://www.eclipse.org/emf/2002/Ecore" xmlns:x="http://www.w3.org/2001/XMLSchema-instance" name="m" nsURI="urn:bounds"><eClassifiers x:type="e:EClass" name="Base"/><eClassifiers x:type="e:EClass" name="Child" eSuperTypes="#//Base"/></e:EPackage>"##,
    )).0;
    let mut entry = source(
        "bounds_entry",
        "import model \"urn:bounds\" as m;fn read<T:Sequence<m.Base>>(x:T)->m.Base?=x[0];fn main(x:Sequence<m.Child>)->m.Base?=read(x);",
    );
    entry.dependencies.push(model.id.clone());
    let selection = EntrySelection {
        symbol: SymbolId {
            source: entry.id.clone(),
            declaration: 2,
        },
        types: vec![],
        arguments: vec![],
    };
    compile_function(
        &bundle(vec![entry.clone(), model]),
        &entry.id,
        &selection,
        &Limits::default(),
    )
    .unwrap();
}

#[test]
fn long_strict_binary_chains_lower_on_a_native_sized_stack_in_source_order() {
    std::thread::Builder::new()
        .stack_size(1024 * 1024)
        .spawn(|| {
            let terms = (0..64).map(|n| n.to_string()).collect::<Vec<_>>().join("+");
            let body = format!("fn main()->Int32={terms};");
            let image = compile(&body, 0);
            let source = source("functions", &body);
            let operators = image.program().functions[0]
                .blocks
                .iter()
                .flat_map(|block| &block.instructions)
                .filter_map(|instruction| match instruction.kind {
                    ir_v2::InstructionKind::Binary { op, .. } => Some((op, &instruction.span)),
                    _ => None,
                })
                .collect::<Vec<_>>();
            assert_eq!(operators.len(), 63);
            assert!(
                operators
                    .windows(2)
                    .all(|pair| pair[0].1.start < pair[1].1.start)
            );
            for (op, span) in operators {
                assert_eq!(
                    op,
                    intersect_model::definitions::rcore_behavior::ir::BinaryOp::Add
                );
                assert_eq!(&source.text[span.start..span.end], "+");
            }
            let mixed = compile("fn main()->Int32=(5-2)*(7+1);", 0);
            let operators = mixed.program().functions[0]
                .blocks
                .iter()
                .flat_map(|block| &block.instructions)
                .filter_map(|instruction| match instruction.kind {
                    ir_v2::InstructionKind::Binary { op, .. } => Some(op),
                    _ => None,
                })
                .collect::<Vec<_>>();
            use intersect_model::definitions::rcore_behavior::ir::BinaryOp::{
                Add, Multiply, Subtract,
            };
            assert_eq!(operators, [Subtract, Add, Multiply]);
        })
        .unwrap()
        .join()
        .unwrap();
}

#[test]
fn long_lazy_binary_chains_preserve_branches_on_a_native_sized_stack() {
    std::thread::Builder::new()
        .stack_size(1024 * 1024)
        .spawn(|| {
            for operator in ["&&", "||"] {
                for count in [32, 64, 96] {
                    let terms = vec!["true"; count].join(operator);
                    let body = format!("fn main()->Bool={terms};");
                    let entry = source("functions", &body);
                    let bundle = bundle(vec![entry.clone()]);
                    let selection = EntrySelection {
                        symbol: SymbolId {
                            source: entry.id.clone(),
                            declaration: 0,
                        },
                        types: vec![],
                        arguments: vec![],
                    };
                    // Larger chains legitimately exceed the default aggregate
                    // block-parameter budget; do not change production defaults.
                    let mut limits = Limits::default();
                    limits.scalar.max_parameters = 65536;
                    let image = compile_function(&bundle, &entry.id, &selection, &limits)
                        .unwrap_or_else(|errors| panic!("{count} {operator}: {errors:?}"));
                    let branches = image.program().functions[0]
                        .blocks
                        .iter()
                        .filter(|block| {
                            matches!(block.terminator, ir_v2::Terminator::Branch { .. })
                        })
                        .count();
                    assert_eq!(branches, count - 1);
                    assert_eq!(
                        image.program(),
                        compile_function(&bundle, &entry.id, &selection, &limits)
                            .unwrap()
                            .program()
                    );
                    if count == 64 {
                        let errors =
                            compile_function(&bundle, &entry.id, &selection, &Limits::default())
                                .unwrap_err();
                        assert!(
                            errors
                                .iter()
                                .any(|e| e.message == "Function IR work limit exceeded")
                        );
                    }
                }
            }
        })
        .unwrap()
        .join()
        .unwrap();
}

#[test]
fn nested_expression_families_lower_with_original_limits_on_one_mib_stack() {
    std::thread::Builder::new()
        .stack_size(1024 * 1024)
        .spawn(|| {
            for count in [32, 60, 63, 64] {
                for shape in [
                    "group", "unary", "call", "block", "if", "bound_if", "else_if", "match",
                    "list", "option", "tuple", "record",
                ] {
                    let mut expression = "true".to_string();
                    let mut ty = "Bool".to_string();
                    for _ in 0..count {
                        expression = match shape {
                            "group" => format!("({expression})"),
                            "unary" => format!("!{expression}"),
                            "call" => format!("keep({expression})"),
                            "block" => format!("{{let x=true;{expression}}}"),
                            "if" => format!("if true {{{expression}}} else {{false}}"),
                            "bound_if" => {
                                format!("if true {{let x=true;{expression}}} else {{false}}")
                            }
                            "else_if" => format!("if false {{false}} else {{{expression}}}"),
                            "match" => {
                                format!("match true {{true => {expression},false => false}}")
                            }
                            "list" => {
                                ty = format!("Sequence<{ty}>");
                                format!("[{expression}]")
                            }
                            "option" => {
                                ty = format!("Option<{ty}>");
                                format!("Some({expression})")
                            }
                            "tuple" => {
                                ty = format!("(Bool,{ty})");
                                format!("(true,{expression})")
                            }
                            "record" => {
                                ty = format!("{{x:{ty}}}");
                                format!("{{x:{expression}}}")
                            }
                            _ => unreachable!(),
                        };
                    }
                    let entry = source(
                        "nested",
                        &format!("fn main()->{ty} ={expression};fn keep(x:Bool)->Bool=x;"),
                    );
                    let bundle = bundle(vec![entry.clone()]);
                    let selection = EntrySelection {
                        symbol: SymbolId {
                            source: entry.id.clone(),
                            declaration: 0,
                        },
                        types: vec![],
                        arguments: vec![],
                    };
                    let mut limits = Limits::default();
                    limits.scalar.max_parameters = 65536;
                    let result = compile_function(&bundle, &entry.id, &selection, &limits);
                    let structural = matches!(shape, "list" | "option" | "tuple" | "record");
                    if count == 64 || (count == 63 && structural) {
                        let errors = result.unwrap_err();
                        assert!(
                            errors.iter().any(|e| e.message.contains("nesting")),
                            "{shape} {count}: {errors:?}"
                        );
                    } else {
                        let image =
                            result.unwrap_or_else(|errors| panic!("{shape} {count}: {errors:?}"));
                        assert_eq!(
                            image.program(),
                            compile_function(&bundle, &entry.id, &selection, &limits)
                                .unwrap()
                                .program()
                        );
                        if count == 60 && matches!(shape, "if" | "bound_if" | "else_if" | "match") {
                            let errors = compile_function(
                                &bundle,
                                &entry.id,
                                &selection,
                                &Limits::default(),
                            )
                            .unwrap_err();
                            assert!(
                                errors
                                    .iter()
                                    .any(|e| e.message == "Function IR work limit exceeded"),
                                "{shape}: {errors:?}"
                            );
                        }
                    }
                }
            }
        })
        .unwrap()
        .join()
        .unwrap();
}

#[path = "../../shared/rcore_behavior_parser_stack.rs"]
mod parser_stack;

#[test]
fn public_parser_complete_baselines_fit_the_original_one_mib_stack() {
    std::thread::Builder::new()
        .stack_size(1024 * 1024)
        .spawn(|| {
            assert_eq!(parser_stack::verify(), 8);
        })
        .unwrap()
        .join()
        .unwrap();
}
