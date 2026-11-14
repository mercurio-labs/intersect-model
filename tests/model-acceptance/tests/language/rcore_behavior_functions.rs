const COLLECTION_ROUNDTRIP: &str = "fn collectionRoundtrip<T>(value:T)->T={let xs=map([value,value],|x:T|x);let kept=filter(xs,|x:T|contains([value],x));let groups=groupBy(kept,|x:T|x);let flat=flatten(map(groups,|g:(T,Sequence<T>)|g[1]));let checked:Int8=if size(groups)==1 && count(flat,value)==2 && size(distinct(flat))==1 && size(intersection(flat,[value]))==1 && size(union(flat,[value]))==3 && contains(difference(flat,[value]),value) {0} else {1/0};fold(Sequence(distinct(flat)),value,|a:T,b:T|b)};";
use intersect_model::definitions::{
    bundle::{DefinitionBundle, DefinitionSource, SourceKind},
    rcore_behavior::{
        function_lowering::compile_function,
        ir,
        ir_v2::{DataWire, EntrySelection, Limits, TypeNode, ValueWire},
        modules::SymbolId,
        types::Scalar,
    },
};
use intersect_model::rcore_behavior_functions::{
    ExecutionContext, ExecutionErrorKind, ExecutionLimits, evaluate,
};
fn fixture(body: &str) -> (DefinitionBundle, DefinitionSource) {
    let source = DefinitionSource::new(
        "memory:runtime.rcore_behavior",
        SourceKind::RcoreBehavior,
        format!("rcore 1; package runtime version \"1\"; {body}"),
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
fn selected(source: &DefinitionSource) -> SymbolId {
    let document = intersect_model::definitions::rcore_behavior::parse(source.clone());
    let declaration = document
        .declarations
        .iter()
        .position(|d| {
            matches!(
                d.value,
                intersect_model::definitions::rcore_behavior::ast::Declaration::Function { .. }
            )
        })
        .unwrap();
    SymbolId {
        source: source.id.clone(),
        declaration,
    }
}
fn compile(
    bundle: &DefinitionBundle,
    source: &DefinitionSource,
) -> intersect_model::definitions::rcore_behavior::ir_v2::VerifiedExecutable {
    compile_function(
        bundle,
        &source.id,
        &EntrySelection {
            symbol: selected(source),
            types: vec![],
            arguments: vec![],
        },
        &Limits::default(),
    )
    .unwrap_or_else(|e| panic!("{}\n{e:?}", source.text))
}
fn run(body: &str) -> ValueWire {
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
fn int(value: &ValueWire) -> i64 {
    let DataWire::Scalar(value) = &value.data else {
        panic!("expected scalar {value:?}")
    };
    value
        .payload
        .as_i64()
        .or_else(|| {
            value
                .payload
                .get("value")
                .and_then(|v| v.as_str())
                .and_then(|v| v.parse().ok())
        })
        .unwrap()
}
#[test]
fn lexical_scopes_transitive_captures_and_creator_lifetimes_execute() {
    for body in [
        "fn run()->Int8={let x:Int8=1;let f=|y:Int8|x+y;{let x:Int8=100;f(41)}};",
        "fn run()->Int8={let f=make(10);let g=f(20);g(12)};fn make(x:Int8)->(Int8)->(Int8)->Int8=|y:Int8| |z:Int8|x+y+z;",
        "fn run()->Int8={let a=make(1);let b=make(2);a(20)+b(19)};fn make(x:Int8)->(Int8)->Int8=|y:Int8|x+y;",
        "fn run()->Int8={let x:Int8=40;{let x:Int8=x+2;x}};",
        "fn run()->Int8=apply::<Int8>(|x:Int8|x+1,41);fn apply<T>(f:(T)->T,x:T)->T=f(x);",
        "fn run()->Int8={let alias:(Int8)->Int8=((id));alias(42)};fn id<T>(x:T)->T=x;",
    ] {
        assert_eq!(int(&run(body)), 42)
    }
}
#[test]
fn structural_values_and_lazy_match_paths_survive_calls() {
    assert_eq!(
        int(&run(
            "fn run()->Int8={let r:{left:Int8,right:Int8}=keep({left:40,right:2});r.left+r.right};fn keep<T>(x:T)->T=x;"
        )),
        42
    );
    assert_eq!(
        int(&run(
            "fn run()->Int8={let pair=keep((40,2));pair[0]+pair[1]};fn keep(x:(Int8,Int8))->(Int8,Int8)=x;"
        )),
        42
    );
    assert_eq!(
        int(&run(
            "fn run()->Int8={let result:Result<Int8,String> =Ok(42);match result {Ok(x)=>keep(x),Err(e)=>fail()}};fn keep(x:Int8)->Int8=x;fn fail()->Int8=1/0;"
        )),
        42
    );
    assert_eq!(
        int(&run(
            "fn run()->Int8={let values=OrderedSet::<Int8>([40,40,2]);(values[0]??0)+(values[1]??0)};"
        )),
        42
    );
    assert_eq!(
        int(&run(
            "fn run()->Int8=if false {fail()} else {42};fn fail()->Int8=1/0;"
        )),
        42
    );
}
#[test]
fn full_collection_equivalence_retains_first_representatives() {
    let result = run("fn run()->OrderedSet<Decimal> =OrderedSet::<Decimal>([1.0,1.00,2.0]);");
    let DataWire::Collection(items) = result.data else {
        panic!("collection")
    };
    assert_eq!(items.len(), 2);
    let DataWire::Scalar(first) = &items[0].data else {
        panic!("decimal")
    };
    assert_eq!(first.payload["scale"], 1);
    let result = run("fn run()->Set<Sequence<Int8>> =Set::<Sequence<Int8>>([[1,2],[1,2],[2,1]]);");
    let DataWire::Collection(items) = result.data else {
        panic!("collection")
    };
    assert_eq!(items.len(), 2);
    let result =
        run("fn run()->Set<Set<Int8>> =Set::<Set<Int8>>([Set::<Int8>([1,2]),Set::<Int8>([2,1])]);");
    let DataWire::Collection(items) = result.data else {
        panic!("collection")
    };
    assert_eq!(items.len(), 1);
    let result = run("fn run()->Bag<Int8> =Bag::<Int8>([1,1,2]);");
    let DataWire::Collection(items) = result.data else {
        panic!("bag")
    };
    assert_eq!(items.len(), 3);
    assert_eq!(
        int(&run(
            "fn run()->Int8={let f:(Int8)->Int8=|x:Int8|x;let values=OrderedSet::<(Int8)->Int8>([f,f]);match values[1]{Some(g)=>0,None=>42}};"
        )),
        42
    );
    assert_eq!(
        int(&run(
            "fn run()->Int8={let a=make();let b=make();let values=OrderedSet::<(Int8)->Int8>([a,b]);match values[1]{Some(g)=>g(42),None=>0}};fn make()->(Int8)->Int8=|x:Int8|x;"
        )),
        42
    );
    // Function collections remain legal; comparison never executes the failing body.
    assert_eq!(
        int(&run(
            "fn run()->Int8={let f:(Int8)->Int8=|x:Int8|1/0;let values=Set::<(Int8)->Int8>([f,f]);42};"
        )),
        42
    );
}
#[test]
fn ieee_collection_equivalence_handles_nan_and_zero_without_rewriting_bits() {
    for scalar in [Scalar::Float32, Scalar::Float64] {
        let name = if scalar == Scalar::Float32 {
            "Float32"
        } else {
            "Float64"
        };
        let (bundle, source) = fixture(&format!(
            "fn run(xs:Sequence<{name}>)->OrderedSet<{name}> =OrderedSet::<{name}>(xs);"
        ));
        let image = compile(&bundle, &source);
        let parameter = image.program().functions[image.program().entry.0 as usize].parameters[0];
        let TypeNode::Collection { element, .. } = image.type_node(parameter).unwrap() else {
            panic!("sequence")
        };
        let (kind, bits) = if scalar == Scalar::Float32 {
            ("float32", ["7fa00001", "7fc00002", "80000000", "00000000"])
        } else {
            (
                "float64",
                [
                    "7ff0000000000001",
                    "7ff8000000000002",
                    "8000000000000000",
                    "0000000000000000",
                ],
            )
        };
        let items = bits
            .iter()
            .map(|bits| ValueWire {
                ty: *element,
                data: DataWire::Scalar(ir::Value {
                    ty: ir::ValueType::Scalar(scalar),
                    payload: serde_json::json!({"kind":kind,"bits":bits}),
                }),
            })
            .collect();
        let result = evaluate(
            &image,
            &[ValueWire {
                ty: parameter,
                data: DataWire::Collection(items),
            }],
            &ExecutionContext::new(&bundle),
            &ExecutionLimits::default(),
        )
        .unwrap();
        let DataWire::Collection(items) = result.value.data else {
            panic!("collection")
        };
        assert_eq!(items.len(), 2);
        for (item, bits) in items.iter().zip([bits[0], bits[2]]) {
            let DataWire::Scalar(value) = &item.data else {
                panic!("float")
            };
            assert_eq!(value.payload["bits"], bits)
        }
    }
}
#[test]
fn explicit_cycle_transport_and_arithmetic_failures_keep_traces() {
    for body in [
        "fn run()->Int8=run();",
        "fn run()->Int8=a();fn a()->Int8=run();",
        "fn run()->(Int8)->Int8=|x:Int8|x;",
    ] {
        let (bundle, source) = fixture(body);
        let image = compile(&bundle, &source);
        let error = evaluate(
            &image,
            &[],
            &ExecutionContext::new(&bundle),
            &ExecutionLimits::default(),
        )
        .unwrap_err();
        assert_eq!(
            error.kind,
            if body.contains("|x:Int8|") {
                ExecutionErrorKind::Unsupported
            } else {
                ExecutionErrorKind::ResourceExhausted
            }
        );
        assert!(!error.trace.is_empty());
    }
    let (bundle, source) =
        fixture("fn run()->Int8={let f=make(0);f(42)};fn make(n:Int8)->(Int8)->Int8=|x:Int8|x/n;");
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
    assert_eq!(error.span.as_ref().unwrap().source, source.id);
    assert!(error.consumed.steps > 0);
}
#[test]
fn budgets_foreign_context_and_duplicate_unique_input_reject_then_recover() {
    let (bundle, source) = fixture("fn run()->Int8={let f=|x:Int8|x;f(42)};");
    let image = compile(&bundle, &source);
    for limits in [
        ExecutionLimits {
            max_frames: 1,
            ..Default::default()
        },
        ExecutionLimits {
            max_closures: 0,
            ..Default::default()
        },
        ExecutionLimits {
            max_value_nodes: 0,
            ..Default::default()
        },
    ] {
        assert_eq!(
            evaluate(&image, &[], &ExecutionContext::new(&bundle), &limits)
                .unwrap_err()
                .kind,
            ExecutionErrorKind::ResourceExhausted
        )
    }
    assert_eq!(
        int(&evaluate(
            &image,
            &[],
            &ExecutionContext::new(&bundle),
            &ExecutionLimits::default()
        )
        .unwrap()
        .value),
        42
    );
    let (foreign, _) = fixture("fn run()->Int8=1;");
    assert!(
        evaluate(
            &image,
            &[],
            &ExecutionContext::new(&foreign),
            &ExecutionLimits::default()
        )
        .is_err()
    );
    let (bundle, source) = fixture("fn run(xs:Set<Int8>)->Set<Int8> =xs;");
    let image = compile(&bundle, &source);
    let ty = image.program().functions[0].parameters[0];
    let TypeNode::Collection { element, .. } = image.type_node(ty).unwrap() else {
        panic!("set")
    };
    let value = ValueWire {
        ty: *element,
        data: DataWire::Scalar(ir::Value {
            ty: ir::ValueType::Scalar(Scalar::Int8),
            payload: serde_json::json!(1),
        }),
    };
    let error = evaluate(
        &image,
        &[ValueWire {
            ty,
            data: DataWire::Collection(vec![value.clone(), value]),
        }],
        &ExecutionContext::new(&bundle),
        &ExecutionLimits::default(),
    )
    .unwrap_err();
    assert_eq!(error.kind, ExecutionErrorKind::Arguments);
    assert!(error.message.contains("Duplicate"));
}
#[test]
fn model_and_nominal_datatype_captures_use_real_checked_contexts() {
    use intersect_model::core::model::Model;
    use intersect_model::definitions::{
        compiler::{import_datatype_bundle, materialize_runtime_bundle},
        conversion::{ConversionRegistry, ReferenceContext},
        generic_types::{RuntimeType, Type},
    };
    use intersect_model::model_reference::ReferenceDomain;
    let model_source = intersect_model_acceptance::migration_fixture::migrate_source_fixture(DefinitionSource::new(
        "memory:objects.ecore",
        SourceKind::Ecore,
        r##"<e:EPackage xmlns:e="http://www.eclipse.org/emf/2002/Ecore" xmlns:x="http://www.w3.org/2001/XMLSchema-instance" name="objects" nsURI="urn:objects"><eClassifiers x:type="e:EClass" name="Base"/><eClassifiers x:type="e:EClass" name="Child" eSuperTypes="#//Base"/><eClassifiers x:type="e:EDataType" name="Text" instanceClassName="java.lang.String"/></e:EPackage>"##,
    )).0;
    let id = |n| {
        intersect_model_acceptance::migration_fixture::named_declaration_fixture(
            &model_source,
            ["Base", "Child", "Text"][n],
        )
    };
    let runtime = |n| {
        RuntimeType::new(Type::Classifier {
            declaration: id(n),
            arguments: vec![],
            raw: false,
        })
        .unwrap()
    };
    let model_bundle = DefinitionBundle {
        version: 1,
        sources: vec![model_source.clone()],
        namespaces: vec![],
    };
    let requested = vec![runtime(0), runtime(1)];
    let schema = materialize_runtime_bundle(
        &model_bundle,
        &model_source.id,
        "urn:objects",
        &requested,
        None,
    )
    .unwrap();
    let mut model = Model::default();
    model
        .create(
            &schema,
            "object",
            schema.class_key_for_type(&runtime(1)).unwrap(),
            "memory:objects",
            0,
            1,
        )
        .unwrap();
    let reference = ReferenceContext {
        bundle: model_bundle.clone(),
        entry: model_source.id.clone(),
        namespace: "urn:objects".into(),
        conversions: vec![],
        materialized_types: requested,
        applied_conversions: vec![],
        constructors: vec![],
        model: serde_json::to_value(&model).unwrap(),
    };
    let registry = ConversionRegistry::default();
    let payload = ReferenceDomain::new_runtime(runtime(0), reference.clone(), &registry)
        .unwrap()
        .bind("object")
        .unwrap();
    for (typename, data) in [
        ("Base", DataWire::Model(payload.clone())),
        (
            "Text",
            DataWire::Datatype(
                serde_json::to_value(
                    import_datatype_bundle(&model_bundle, &id(2))
                        .unwrap()
                        .bind_value(serde_json::json!("text"))
                        .unwrap(),
                )
                .unwrap(),
            ),
        ),
    ] {
        let mut source = DefinitionSource::new(
            "memory:nominal-behavior.rcore",
            SourceKind::RcoreBehavior,
            format!(
                "rcore 1; package nominal version \"1\";import model \"urn:objects\" as m;fn run(value:m.{typename})->m.{typename}={{let f=|dummy:Int8|collectionRoundtrip(value);f(0)}};{COLLECTION_ROUNDTRIP}"
            ),
        );
        source.dependencies.push(model_source.id.clone());
        let bundle = DefinitionBundle {
            version: 1,
            sources: vec![model_source.clone(), source.clone()],
            namespaces: vec![],
        };
        let image = compile(&bundle, &source);
        let ty = image.program().functions[0].parameters[0];
        let refs = vec![reference.clone()];
        let context = ExecutionContext {
            bundle: &bundle,
            conversions: Some(&registry),
            references: &refs,
        };
        let argument = ValueWire { ty, data };
        let result = evaluate(
            &image,
            std::slice::from_ref(&argument),
            &context,
            &ExecutionLimits::default(),
        )
        .unwrap();
        assert_eq!(result.value, argument);
        if typename == "Base" {
            assert_eq!(result.consumed.reference_domains, 1);
            assert_eq!(result.consumed.model_elements, 1);
            let mut stale = argument.clone();
            let DataWire::Model(value) = &mut stale.data else {
                panic!("model")
            };
            value["context"] = serde_json::json!("stale");
            assert_eq!(
                evaluate(&image, &[stale], &context, &ExecutionLimits::default())
                    .unwrap_err()
                    .kind,
                ExecutionErrorKind::MissingDependency
            );
        }
    }
    let mut source = DefinitionSource::new(
        "memory:generic-index-behavior.rcore",
        SourceKind::RcoreBehavior,
        "rcore 1;package generic_index version \"1\";import model \"urn:objects\" as m;fn run(xs:Sequence<m.Child>)->m.Base?=first(union(xs,Sequence::<m.Base>([])));fn first<T:Sequence<m.Base>>(xs:T)->m.Base?=xs[0];",
    );
    source.dependencies.push(model_source.id.clone());
    let bundle = DefinitionBundle {
        version: 1,
        sources: vec![model_source.clone(), source.clone()],
        namespaces: vec![],
    };
    let image = compile(&bundle, &source);
    let parameter = image.program().functions[image.program().entry.0 as usize].parameters[0];
    let TypeNode::Collection { element, .. } = image.type_node(parameter).unwrap() else {
        panic!("sequence")
    };
    let payload = ReferenceDomain::new_runtime(runtime(1), reference.clone(), &registry)
        .unwrap()
        .bind("object")
        .unwrap();
    let argument = ValueWire {
        ty: parameter,
        data: DataWire::Collection(vec![ValueWire {
            ty: *element,
            data: DataWire::Model(payload.clone()),
        }]),
    };
    let refs = vec![reference];
    let context = ExecutionContext {
        bundle: &bundle,
        conversions: Some(&registry),
        references: &refs,
    };
    let result = evaluate(&image, &[argument], &context, &ExecutionLimits::default()).unwrap();
    let DataWire::Some(value) = result.value.data else {
        panic!("present reference")
    };
    let DataWire::Model(actual) = value.data else {
        panic!("reference")
    };
    assert_eq!(actual, payload);
}
#[test]
fn raw_applied_models_and_provider_alias_values_preserve_exact_tags() {
    use intersect_model::core::model::Model;
    use intersect_model::definitions::{
        compiler::materialize_runtime_bundle,
        conversion::{ConversionContract, ProviderId, ReferenceContext},
        generic_types::{RuntimeType, Type},
        rcore_behavior::function_lowering::compile_function_with_conversions,
    };

    use intersect_model::{conversion::with_catalog_policies, model_reference::ReferenceDomain};
    let model_source = intersect_model_acceptance::migration_fixture::migrate_source_fixture(
        DefinitionSource::new(
            "memory:provider-handoff.ecore",
            SourceKind::Ecore,
            include_str!(
                "../../../../fixtures/ecore-generics/rcore-behavior-provider-handoff.ecore"
            ),
        ),
    )
    .0;
    let id = |n| {
        intersect_model_acceptance::migration_fixture::named_declaration_fixture(
            &model_source,
            ["RGB", "Alias", "Box"][n],
        )
    };
    let atom = |n| Type::Classifier {
        declaration: id(n),
        arguments: vec![],
        raw: false,
    };
    let applied = |n| {
        RuntimeType::new(Type::Classifier {
            declaration: id(2),
            arguments: vec![atom(n)],
            raw: false,
        })
        .unwrap()
    };
    let raw = RuntimeType::new(Type::Classifier {
        declaration: id(2),
        arguments: vec![],
        raw: true,
    })
    .unwrap();
    let contracts: Vec<_> = (0..2)
        .map(|n| ConversionContract {
            datatype: id(n),
            provider: ProviderId {
                name: "example.rgb".into(),
                version: "1".into(),
            },
            serializable: true,
            declarations: vec![],
            members: vec![],
        })
        .collect();
    let model_bundle = DefinitionBundle {
        version: 1,
        sources: vec![model_source.clone()],
        namespaces: vec![],
    };
    let requested = vec![applied(0), applied(1), raw.clone()];
    let snapshot = with_catalog_policies(&model_bundle, &contracts, &[], &[], |registry| {
        let schema = materialize_runtime_bundle(
            &model_bundle,
            &model_source.id,
            "urn:rcore-behavior-provider-handoff",
            &requested,
            Some(registry),
        )
        .map_err(|e| e.to_string())?;
        let mut model = Model::default();
        for (name, target, datatype) in [
            ("rgb", applied(0), id(0)),
            ("alias", applied(1), id(1)),
            ("raw", raw.clone(), id(0)),
        ] {
            model.create(
                &schema,
                name,
                schema.class_key_for_type(&target)?,
                "memory:provider-model",
                0,
                1,
            )?;
            let value = serde_json::to_value(registry.parse(&datatype, "#010203")?)
                .map_err(|e| e.to_string())?;
            model.edit_with_conversions(
                &schema,
                &[(name.into(), "value".into(), value)],
                registry,
            )?;
        }
        model.validate_with_conversions(&schema, registry)?;
        serde_json::to_value(model).map_err(|e| e.to_string())
    })
    .unwrap();
    let reference = ReferenceContext {
        bundle: model_bundle,
        entry: model_source.id.clone(),
        namespace: "urn:rcore-behavior-provider-handoff".into(),
        conversions: contracts.clone(),
        materialized_types: requested,
        applied_conversions: vec![],
        constructors: vec![],
        model: snapshot,
    };
    for annotation in ["m.Box<m.Alias>", "m.Box", "m.Alias"] {
        let mut source = DefinitionSource::new(
            "memory:provider-functions-behavior.rcore",
            SourceKind::RcoreBehavior,
            format!(
                "rcore 1;package provider_functions version \"1\";import model \"urn:rcore-behavior-provider-handoff\" as m;fn run(value:{annotation})->{annotation} ={{let f=|unused:Int8|bounce(4,collectionRoundtrip(value));f(0)}};fn bounce<T>(n:Int8,x:T)->T=if n==0 {{x}} else {{bounce::<T>(n-1,x)}};{COLLECTION_ROUNDTRIP}"
            ),
        );
        source.dependencies.push(model_source.id.clone());
        let bundle = DefinitionBundle {
            version: 1,
            sources: vec![model_source.clone(), source.clone()],
            namespaces: vec![],
        };
        with_catalog_policies(&bundle, &contracts, &[], &[], |registry| {
            let image = compile_function_with_conversions(
                &bundle,
                &source.id,
                &EntrySelection {
                    symbol: selected(&source),
                    types: vec![],
                    arguments: vec![],
                },
                registry,
                &Limits::default(),
            )
            .map_err(|e| format!("{e:?}"))?;
            let ty = image.program().functions[image.program().entry.0 as usize].parameters[0];
            let data = if annotation == "m.Alias" {
                DataWire::Datatype(
                    serde_json::to_value(registry.parse(&id(1), "#010203")?)
                        .map_err(|e| e.to_string())?,
                )
            } else {
                DataWire::Model(
                    ReferenceDomain::new_runtime(
                        if annotation == "m.Box" {
                            raw.clone()
                        } else {
                            applied(1)
                        },
                        reference.clone(),
                        registry,
                    )?
                    .bind("alias")?,
                )
            };
            let refs = vec![reference.clone()];
            let context = ExecutionContext {
                bundle: &bundle,
                conversions: Some(registry),
                references: &refs,
            };
            let argument = ValueWire { ty, data };
            let result = evaluate(
                &image,
                std::slice::from_ref(&argument),
                &context,
                &ExecutionLimits::default(),
            )
            .map_err(|e| format!("{e:?}"))?;
            assert_eq!(result.value, argument);
            if annotation == "m.Box<m.Alias>" {
                let wrong = ReferenceDomain::new_runtime(applied(0), reference.clone(), registry)?
                    .bind("rgb")?;
                assert_eq!(
                    evaluate(
                        &image,
                        &[ValueWire {
                            ty,
                            data: DataWire::Model(wrong)
                        }],
                        &context,
                        &ExecutionLimits::default()
                    )
                    .unwrap_err()
                    .kind,
                    ExecutionErrorKind::Nominal
                );
            }
            Ok(())
        })
        .unwrap();
    }
    for body in [
        "fn run(value:m.Alias)->m.RGB=value;",
        "fn run(value:m.Box<m.Alias>)->m.Box<m.RGB> =value;",
    ] {
        let mut source = DefinitionSource::new(
            "memory:retag-behavior.rcore",
            SourceKind::RcoreBehavior,
            format!(
                "rcore 1;package retag version \"1\";import model \"urn:rcore-behavior-provider-handoff\" as m;{body}"
            ),
        );
        source.dependencies.push(model_source.id.clone());
        let bundle = DefinitionBundle {
            version: 1,
            sources: vec![model_source.clone(), source.clone()],
            namespaces: vec![],
        };
        with_catalog_policies(&bundle, &contracts, &[], &[], |registry| {
            assert!(
                compile_function_with_conversions(
                    &bundle,
                    &source.id,
                    &EntrySelection {
                        symbol: selected(&source),
                        types: vec![],
                        arguments: vec![]
                    },
                    registry,
                    &Limits::default()
                )
                .is_err()
            );
            Ok(())
        })
        .unwrap();
    }
}

#[test]
fn applied_datatype_closure_values_require_the_installed_exact_codec() {
    use intersect_model::conversion::with_catalog_policies;
    use intersect_model::definitions::{
        conversion::{
            AppliedConversionContract, ConversionContract, MemberContract, MemberType, ProviderId,
        },
        generic_types::{ClosedType, Type},
        rcore_behavior::function_lowering::compile_function_with_conversions,
        resources::DeclarationId,
    };
    let model = intersect_model_acceptance::migration_fixture::migrate_source_fixture(DefinitionSource::new(
        "memory:applied-values.ecore",
        SourceKind::Ecore,
        r##"<e:EPackage xmlns:e="http://www.eclipse.org/emf/2002/Ecore" xmlns:x="http://www.w3.org/2001/XMLSchema-instance" name="values" nsURI="urn:applied-values"><eClassifiers x:type="e:EDataType" name="Text" instanceClassName="java.lang.String"/><eClassifiers x:type="e:EDataType" name="Sequence" instanceClassName="intersect.Sequence"><eTypeParameters name="T"/></eClassifiers></e:EPackage>"##,
    )).0;
    let id = |n| DeclarationId {
        source: model.id.clone(),
        path: vec![n],
    };
    let application = ClosedType::new(Type::Classifier {
        declaration: id(1),
        arguments: vec![Type::Classifier {
            declaration: id(0),
            arguments: vec![],
            raw: false,
        }],
        raw: false,
    })
    .unwrap();
    let contract = AppliedConversionContract {
        application: application.clone(),
        conversion: ConversionContract {
            datatype: id(1),
            provider: ProviderId {
                name: "intersect.sequence".into(),
                version: "1".into(),
            },
            serializable: true,
            declarations: vec![],
            members: vec![MemberContract {
                nullable: false,
                value: MemberType::Datatype { declaration: id(0) },
            }],
        },
    };
    let mut source = DefinitionSource::new(
        "memory:applied-values-behavior.rcore",
        SourceKind::RcoreBehavior,
        format!(
            "{}{}",
            "rcore 1;package applied_values version \"1\";import model \"urn:applied-values\" as m;fn run(value:m.Sequence<m.Text>)->m.Sequence<m.Text> ={let f=|unused:Int8|collectionRoundtrip(value);f(0)};",
            COLLECTION_ROUNDTRIP
        ),
    );
    source.dependencies.push(model.id.clone());
    let bundle = DefinitionBundle {
        version: 1,
        sources: vec![model.clone(), source.clone()],
        namespaces: vec![],
    };
    let selection = EntrySelection {
        symbol: selected(&source),
        types: vec![],
        arguments: vec![],
    };
    assert!(compile_function(&bundle, &source.id, &selection, &Limits::default()).is_err());
    with_catalog_policies(
        &bundle,
        &[],
        std::slice::from_ref(&contract),
        &[],
        |registry| {
            assert!(registry.applied().contains_contract(&contract)?);
            let mut altered = contract.clone();
            altered.conversion.provider.version = "foreign".into();
            assert!(!registry.applied().contains_contract(&altered)?);
            assert!(
                !intersect_model::definitions::conversion::AppliedConversionRegistry::default()
                    .contains_contract(&contract)?
            );

            let image = compile_function_with_conversions(
                &bundle,
                &source.id,
                &selection,
                registry,
                &Limits::default(),
            )
            .map_err(|e| format!("{e:?}"))?;
            let ty = image.program().functions[image.program().entry.0 as usize].parameters[0];
            let converted = registry
                .applied()
                .parse(&application, "[\"one\",\"two\"]")?;
            let argument = ValueWire {
                ty,
                data: DataWire::Datatype(
                    serde_json::to_value(converted).map_err(|e| e.to_string())?,
                ),
            };
            let context = ExecutionContext {
                bundle: &bundle,
                conversions: Some(registry),
                references: &[],
            };
            let result = evaluate(
                &image,
                std::slice::from_ref(&argument),
                &context,
                &ExecutionLimits::default(),
            )
            .map_err(|e| format!("{e:?}"))?;
            assert_eq!(result.value, argument);
            assert!(
                evaluate(
                    &image,
                    std::slice::from_ref(&argument),
                    &ExecutionContext::new(&bundle),
                    &ExecutionLimits::default()
                )
                .is_err()
            );
            Ok(())
        },
    )
    .unwrap();
}

#[test]
fn repeated_closure_invocation_charges_capture_transfers_before_copying() {
    let (bundle, source) = fixture("fn run()->Int8={let n:Int8=21;let f=|x:Int8|n+x;f(0)+f(0)};");
    let image = compile(&bundle, &source);
    let context = ExecutionContext::new(&bundle);
    let limited = ExecutionLimits {
        max_captures: 2,
        ..Default::default()
    };
    let error = evaluate(&image, &[], &context, &limited).unwrap_err();
    assert_eq!(error.kind, ExecutionErrorKind::ResourceExhausted);
    assert!(error.message.contains("capture transfer"));
    let exact = ExecutionLimits {
        max_captures: 3,
        ..Default::default()
    };
    let result = evaluate(&image, &[], &context, &exact).unwrap();
    assert_eq!(int(&result.value), 42);
    assert_eq!(result.consumed.captures, 3);
}

#[test]
fn wide_record_resource_failure_precedes_result_allocation_and_recovers() {
    let fields = (0..64)
        .map(|i| format!("f{i}:Int8"))
        .collect::<Vec<_>>()
        .join(",");
    let values = (0..64)
        .map(|i| format!("f{i}:value"))
        .collect::<Vec<_>>()
        .join(",");
    let (bundle, source) = fixture(&format!("fn run(value:Int8)->{{{fields}}}={{{values}}};"));
    let image = compile(&bundle, &source);
    let ty = image.program().functions[image.program().entry.0 as usize].parameters[0];
    let argument = ValueWire {
        ty,
        data: DataWire::Scalar(ir::Value {
            ty: ir::ValueType::Scalar(Scalar::Int8),
            payload: serde_json::json!(42),
        }),
    };
    let context = ExecutionContext::new(&bundle);
    let error = evaluate(
        &image,
        std::slice::from_ref(&argument),
        &context,
        &ExecutionLimits {
            scalar: intersect_model::rcore_behavior_execution::ExecutionLimits {
                values: ir::ValueLimits {
                    max_value_bytes: 128,
                    ..Default::default()
                },
                ..Default::default()
            },
            ..Default::default()
        },
    )
    .unwrap_err();
    assert_eq!(error.kind, ExecutionErrorKind::ResourceExhausted);
    assert_eq!(error.consumed.value_nodes, 1);
    assert!(error.message.contains("byte limit"));
    let result = evaluate(&image, &[argument], &context, &ExecutionLimits::default()).unwrap();
    let DataWire::Record(fields) = result.value.data else {
        panic!("record")
    };
    assert_eq!(fields.len(), 64);
}

#[test]
fn exact_trace_byte_boundaries_preserve_arithmetic_cause_and_whole_identities() {
    let (mut bundle, mut source) = fixture("fn run()->Int8=fail(0);fn fail(x:Int8)->Int8=1/x;");
    source.id.uri = format!(
        "memory:{}\"trace-behavior.rcore",
        "long-source-name".repeat(32)
    );
    bundle.sources = vec![source.clone()];
    let image = compile(&bundle, &source);
    let context = ExecutionContext::new(&bundle);
    let full = evaluate(&image, &[], &context, &ExecutionLimits::default()).unwrap_err();
    assert_eq!(full.kind, ExecutionErrorKind::Arithmetic);
    assert!(!full.trace_truncated);
    let bytes = full
        .span
        .as_ref()
        .map_or(0, |s| serde_json::to_vec(s).unwrap().len())
        + full
            .trace
            .iter()
            .map(|f| serde_json::to_vec(f).unwrap().len())
            .sum::<usize>()
        + full
            .selected_providers
            .iter()
            .map(|p| serde_json::to_vec(p).unwrap().len())
            .sum::<usize>();
    for cap in [bytes, bytes + 1] {
        let error = evaluate(
            &image,
            &[],
            &context,
            &ExecutionLimits {
                max_trace_bytes: cap,
                ..Default::default()
            },
        )
        .unwrap_err();
        assert!(!error.trace_truncated);
        assert_eq!(
            serde_json::to_value(&error.trace).unwrap(),
            serde_json::to_value(&full.trace).unwrap()
        );
        assert_eq!(error.message, full.message);
    }
    for cap in [0, bytes - 1] {
        let error = evaluate(
            &image,
            &[],
            &context,
            &ExecutionLimits {
                max_trace_bytes: cap,
                ..Default::default()
            },
        )
        .unwrap_err();
        assert!(error.trace_truncated);
        assert_eq!(error.kind, full.kind);
        assert_eq!(error.arithmetic_kind, full.arithmetic_kind);
        assert_eq!(error.message, full.message);
        let retained = error
            .span
            .as_ref()
            .map_or(0, |s| serde_json::to_vec(s).unwrap().len())
            + error
                .trace
                .iter()
                .map(|f| serde_json::to_vec(f).unwrap().len())
                .sum::<usize>()
            + error
                .selected_providers
                .iter()
                .map(|p| serde_json::to_vec(p).unwrap().len())
                .sum::<usize>();
        assert!(retained <= cap);
        for frame in &error.trace {
            assert_eq!(frame.definition.source, source.id)
        }
    }
    assert!(
        !evaluate(&image, &[], &context, &ExecutionLimits::default())
            .unwrap_err()
            .trace_truncated
    );
}

#[test]
fn recursive_generic_frames_preserve_structural_payloads_and_collection_representatives() {
    for (annotation, expression) in [
        ("Int8?", "Some::<Int8>(42)"),
        ("Int8?", "None::<Int8>()"),
        ("Result<Int8,String>", "Ok(42)"),
        ("Result<Int8,String>", "Err(\"failure\")"),
        ("(Int8,String)", "(42,\"kept\")"),
        ("{left:Int8,right:Sequence<Int8>}", "{left:40,right:[2,2]}"),
        ("Sequence<Int8>", "[1,1,2]"),
        ("Bag<Int8>", "Bag::<Int8>([1,1,2])"),
        ("Set<Int8>", "Set::<Int8>([1,1,2])"),
        (
            "OrderedSet<Decimal>",
            "OrderedSet::<Decimal>([1.0,1.00,2.0])",
        ),
    ] {
        let (bundle, source) = fixture(&format!(
            "fn run(n:Int8)->{annotation} ={{let value:{annotation} ={expression};bounce(n,value)}};fn bounce<T>(n:Int8,x:T)->T=if n==0 {{x}} else {{bounce::<T>(n-1,x)}};"
        ));
        let image = compile(&bundle, &source);
        let ty = image.program().functions[image.program().entry.0 as usize].parameters[0];
        let invoke = |n| {
            evaluate(
                &image,
                &[ValueWire {
                    ty,
                    data: DataWire::Scalar(ir::Value {
                        ty: ir::ValueType::Scalar(Scalar::Int8),
                        payload: serde_json::json!(n),
                    }),
                }],
                &ExecutionContext::new(&bundle),
                &ExecutionLimits::default(),
            )
            .unwrap()
        };
        let base = invoke(0);
        let recursive = invoke(4);
        assert_eq!(recursive.value, base.value, "{annotation}");
        assert!(recursive.consumed.steps > base.consumed.steps);
        if annotation == "OrderedSet<Decimal>" {
            let DataWire::Collection(items) = recursive.value.data else {
                panic!("collection")
            };
            assert_eq!(items.len(), 2);
            let DataWire::Scalar(first) = &items[0].data else {
                panic!("scalar")
            };
            assert_eq!(first.payload["scale"], 1);
        }
    }
}

#[test]
fn typed_errors_propagate_through_recursive_generic_mutual_and_closure_calls() {
    for body in [
        "fn run()->Result<Int8,String> =fail(4);fn fail(n:Int8)->Result<Int8,String> =if n==0 {Err(\"kept\")} else {match fail(n-1){Ok(x)=>Ok(x),Err(e)=>Err(e)}};",
        "fn run()->Result<Int8,String> =a(4);fn a(n:Int8)->Result<Int8,String> =if n==0 {Err(\"kept\")} else {b(n-1)};fn b(n:Int8)->Result<Int8,String> =a(n);",
        "fn run()->Result<Int8,String> =carry(4,Err::<Int8,String>(\"kept\"));fn carry<T>(n:Int8,x:T)->T=if n==0 {x} else {carry::<T>(n-1,x)};",
        "fn run()->Result<Int8,String> =make(4)();fn make(n:Int8)->()->Result<Int8,String> =||if n==0 {Err(\"kept\")} else {make(n-1)()};",
    ] {
        let DataWire::Err(payload) = run(body).data else {
            panic!("Expected typed Err: {body}")
        };
        let DataWire::Scalar(value) = payload.data else {
            panic!("Expected String")
        };
        assert_eq!(value.payload, serde_json::json!("kept"));
    }
    assert_eq!(
        int(&run(
            "fn run()->Int8=match fail(4){Ok(x)=>x,Err(e)=>42};fn fail(n:Int8)->Result<Int8,String> =if n==0 {Err(\"kept\")} else {fail(n-1)};"
        )),
        42
    );
}

#[test]
fn matching_typed_errors_cannot_swallow_recursive_safety_failures() {
    for (body, expected) in [
        (
            "fn run()->Result<Int8,String> =match forever(){Ok(x)=>Ok(x),Err(e)=>Ok(42)};fn forever()->Result<Int8,String> =forever();",
            ExecutionErrorKind::ResourceExhausted,
        ),
        (
            "fn run()->Result<Int8,String> =match fail(4){Ok(x)=>Ok(x),Err(e)=>Ok(42)};fn fail(n:Int8)->Result<Int8,String> =if n==0 {Ok(1/0)} else {fail(n-1)};",
            ExecutionErrorKind::Arithmetic,
        ),
    ] {
        let (bundle, source) = fixture(body);
        let image = compile(&bundle, &source);
        let limits = ExecutionLimits {
            max_frames: 8,
            ..Default::default()
        };
        let failure = evaluate(&image, &[], &ExecutionContext::new(&bundle), &limits).unwrap_err();
        assert_eq!(failure.kind, expected);
        assert!(!failure.trace.is_empty());
        assert!(failure.consumed.steps > 0);
    }
}
