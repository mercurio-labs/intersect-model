use intersect_model::definitions::{
    bundle::{DefinitionBundle, DefinitionSource, SourceKind},
    enums::EnumValue,
    rcore_behavior::{
        function_lowering::compile_function,
        ir,
        ir_v2::{DataWire, EntrySelection, Limits, TypeId, TypeNode, ValueWire},
        modules::SymbolId,
        types::Scalar,
    },
    resources::DeclarationId,
};
use intersect_model::rcore_behavior_functions::{
    ExecutionContext, ExecutionErrorKind, ExecutionLimits, evaluate,
};

#[test]
fn enum_calls_and_captures_preserve_alias_identity_and_reject_foreign_values() {
    let model = intersect_model_acceptance::migration_fixture::migrate_source_fixture(DefinitionSource::new(
        "memory:function-enums.ecore",
        SourceKind::Ecore,
        r##"<e:EPackage xmlns:e="http://www.eclipse.org/emf/2002/Ecore" xmlns:x="http://www.w3.org/2001/XMLSchema-instance" name="m" nsURI="urn:function-enums"><eClassifiers x:type="e:EEnum" name="Mode"><eLiterals name="First" value="7" literal="first"/><eLiterals name="Alias" value="7" literal="alias"/></eClassifiers><eClassifiers x:type="e:EEnum" name="Other"><eLiterals name="First" value="7" literal="first"/></eClassifiers></e:EPackage>"##,
    )).0;
    let mut source = DefinitionSource::new(
        "memory:function-enums-behavior.rcore",
        SourceKind::RcoreBehavior,
        "rcore 1; package enums version \"1\"; import model \"urn:function-enums\" as m; fn run(x:m.Mode)->m.Mode={let f=|dummy:Int8|keep(x);f(0)}; fn keep(x:m.Mode)->m.Mode=x;",
    );
    source.dependencies.push(model.id.clone());
    let bundle = DefinitionBundle {
        version: 1,
        sources: vec![model.clone(), source.clone()],
        namespaces: vec![],
    };
    let image = compile_function(
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
    )
    .unwrap();
    let ty = image.program().functions[image.program().entry.0 as usize].parameters[0];
    let id = |path| DeclarationId {
        source: model.id.clone(),
        path,
    };
    let value = |enumeration, literal| ValueWire {
        ty,
        data: DataWire::Enum(EnumValue::Enum {
            enumeration,
            literal,
        }),
    };
    let context = ExecutionContext::new(&bundle);
    let first = value(id(vec![0]), id(vec![0, 0]));
    let alias = value(id(vec![0]), id(vec![0, 1]));
    for argument in [&first, &alias] {
        let result = evaluate(
            &image,
            std::slice::from_ref(argument),
            &context,
            &ExecutionLimits::default(),
        )
        .unwrap();
        assert_eq!(&result.value, argument);
    }
    assert_ne!(first, alias);
    let mut stale = id(vec![0, 0]);
    stale.source.sha256 = "0".repeat(64);
    let mut foreign = id(vec![0, 0]);
    foreign.source.uri = "memory:foreign.ecore".into();
    for rejected in [
        value(id(vec![1]), id(vec![1, 0])),
        value(id(vec![0]), id(vec![1, 0])),
        value(id(vec![0]), id(vec![0, 99])),
        value(id(vec![0]), stale),
        value(id(vec![0]), foreign),
    ] {
        assert_eq!(
            evaluate(&image, &[rejected], &context, &ExecutionLimits::default())
                .unwrap_err()
                .kind,
            ExecutionErrorKind::Arguments
        );
    }
    let mut wire = serde_json::to_value(&alias).unwrap();
    wire["data"]["Enum"]["literal"]["ignored"] = serde_json::json!(true);
    assert!(serde_json::from_value::<ValueWire>(wire).is_err());
    assert_eq!(
        evaluate(
            &image,
            std::slice::from_ref(&alias),
            &context,
            &ExecutionLimits::default()
        )
        .unwrap()
        .value,
        alias
    );
}

#[test]
fn all_many_kinds_capture_exact_bounds_and_reject_invalid_shapes_then_recover() {
    let source = DefinitionSource::new(
        "memory:function-many-behavior.rcore",
        SourceKind::RcoreBehavior,
        "rcore 1; package many version \"1\"; fn identity<T>(x:T)->T={let f=|dummy:Int8|x;f(0)};",
    );
    let bundle = DefinitionBundle {
        version: 1,
        sources: vec![source.clone()],
        namespaces: vec![],
    };
    let context = ExecutionContext::new(&bundle);
    for ordered in [false, true] {
        for unique in [false, true] {
            for (lower, upper) in [(1, 2), (0, -1)] {
                let image = compile_function(
                    &bundle,
                    &source.id,
                    &EntrySelection {
                        symbol: SymbolId {
                            source: source.id.clone(),
                            declaration: 0,
                        },
                        types: vec![
                            TypeNode::Scalar(Scalar::Int8),
                            TypeNode::Many {
                                element: TypeId(0),
                                lower,
                                upper,
                                ordered,
                                unique,
                            },
                        ],
                        arguments: vec![TypeId(1)],
                    },
                    &Limits::default(),
                )
                .unwrap_or_else(|e| panic!("{ordered}/{unique}/{lower}/{upper}: {e:?}"));
                let ty = image.program().functions[image.program().entry.0 as usize].parameters[0];
                let TypeNode::Many {
                    element,
                    lower: checked_lower,
                    upper: checked_upper,
                    ordered: checked_ordered,
                    unique: checked_unique,
                } = image.type_node(ty).unwrap()
                else {
                    panic!("Expected exact Many specialization")
                };
                assert_eq!(
                    (
                        *checked_lower,
                        *checked_upper,
                        *checked_ordered,
                        *checked_unique
                    ),
                    (lower, upper, ordered, unique)
                );
                let scalar = |n| ValueWire {
                    ty: *element,
                    data: DataWire::Scalar(ir::Value {
                        ty: ir::ValueType::Scalar(Scalar::Int8),
                        payload: serde_json::json!(n),
                    }),
                };
                let collection = |items: &[i8]| ValueWire {
                    ty,
                    data: DataWire::Collection(items.iter().copied().map(scalar).collect()),
                };
                for items in [vec![1], vec![1, 2]] {
                    let argument = collection(&items);
                    assert_eq!(
                        evaluate(
                            &image,
                            std::slice::from_ref(&argument),
                            &context,
                            &ExecutionLimits::default()
                        )
                        .unwrap()
                        .value,
                        argument
                    );
                }
                for items in [vec![], vec![1, 2, 3]] {
                    let argument = collection(&items);
                    let result = evaluate(
                        &image,
                        std::slice::from_ref(&argument),
                        &context,
                        &ExecutionLimits::default(),
                    );
                    if lower == 0 {
                        assert_eq!(result.unwrap().value, argument);
                    } else {
                        assert_eq!(result.unwrap_err().kind, ExecutionErrorKind::Arguments);
                    }
                }
                let duplicates = collection(&[1, 1]);
                let result = evaluate(
                    &image,
                    std::slice::from_ref(&duplicates),
                    &context,
                    &ExecutionLimits::default(),
                );
                if unique {
                    assert_eq!(result.unwrap_err().kind, ExecutionErrorKind::Arguments);
                } else {
                    assert_eq!(result.unwrap().value, duplicates);
                }
                for malformed in [
                    ValueWire {
                        ty,
                        data: DataWire::Tuple(vec![scalar(1)]),
                    },
                    ValueWire {
                        ty,
                        data: DataWire::None,
                    },
                    ValueWire {
                        ty,
                        data: DataWire::Collection(vec![ValueWire {
                            ty: *element,
                            data: DataWire::Scalar(ir::Value {
                                ty: ir::ValueType::Scalar(Scalar::Bool),
                                payload: serde_json::json!(true),
                            }),
                        }]),
                    },
                ] {
                    assert_eq!(
                        evaluate(&image, &[malformed], &context, &ExecutionLimits::default())
                            .unwrap_err()
                            .kind,
                        ExecutionErrorKind::Arguments
                    );
                }
                let recovery = collection(&[2, 1]);
                assert_eq!(
                    evaluate(
                        &image,
                        std::slice::from_ref(&recovery),
                        &context,
                        &ExecutionLimits::default()
                    )
                    .unwrap()
                    .value,
                    recovery
                );
            }
        }
    }
}
