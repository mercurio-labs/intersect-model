use intersect_model::core::model::Model;
use intersect_model::definitions::{
    bundle::{DefinitionBundle, DefinitionSource, SourceKind},
    conversion::{ConversionRegistry, ReferenceContext},
    rcore_behavior::{
        function_lowering::compile_function,
        ir_v2::{self, DataWire, EntrySelection, InstructionKind, Limits, TypeNode, ValueWire},
        modules::SymbolId,
    },
};

use intersect_model::{
    model_reference::ReferenceDomain,
    rcore_behavior_functions::{ExecutionContext, ExecutionLimits, evaluate},
};
use serde_json::json;

fn setup(body: &str) -> (DefinitionBundle, DefinitionSource, ReferenceContext) {
    setup_model(
        include_str!("../../../../fixtures/ecore-navigation/projection.ecore"),
        body,
        None,
    )
}

#[test]
fn model_storage_reservations_are_aggregate_exact_and_reused_by_target_domains() {
    use intersect_model::rcore_behavior_functions::{
        EvaluationSession, ExecutionErrorKind, SessionProgress,
    };
    use std::num::NonZeroUsize;
    let mut retained = None;
    let mut admission_retained = None;
    // Two retained declaration identities each include the six-byte migration URI suffix.
    let target_admission_bytes = 21085 + 2 * ".rcore".len();
    for body in [
        "fn main(x:m.Base)->Option<String> =x.label;",
        "fn main(x:m.Base,y:m.Diamond)->(Option<String>,Option<String>)=(x.label,y.label);",
    ] {
        let (bundle, source, reference) = setup(body);
        let image = compile_navigation(&bundle, &source);
        let first = ownership_argument(&image, &reference, "d");
        let parameters = &image.program().functions[image.program().entry.0 as usize].parameters;
        let arguments = parameters
            .iter()
            .map(|ty| ValueWire {
                ty: *ty,
                data: first.data.clone(),
            })
            .collect::<Vec<_>>();
        let references = [reference.clone()];
        let mut context = ExecutionContext::new(&bundle);
        context.references = &references;
        let baseline = evaluate(&image, &arguments, &context, &ExecutionLimits::default()).unwrap();
        let needed = baseline.consumed.model_storage_bytes;
        assert!(needed > 0);
        assert_eq!(baseline.consumed.reference_domains, parameters.len());
        // Isolate target-domain admission from the newly observed forward reads.
        let idle_body = format!("{}->Int=0;", body.split_once("->").unwrap().0);
        let (idle_bundle, idle_source, idle_reference) = setup(&idle_body);
        let idle_image = compile_navigation(&idle_bundle, &idle_source);
        let idle_first = ownership_argument(&idle_image, &idle_reference, "d");
        let idle_parameters =
            &idle_image.program().functions[idle_image.program().entry.0 as usize].parameters;
        let idle_arguments = idle_parameters
            .iter()
            .map(|ty| ValueWire {
                ty: *ty,
                data: idle_first.data.clone(),
            })
            .collect::<Vec<_>>();
        let idle_references = [idle_reference];
        let mut idle_context = ExecutionContext::new(&idle_bundle);
        idle_context.references = &idle_references;
        let idle = evaluate(
            &idle_image,
            &idle_arguments,
            &idle_context,
            &ExecutionLimits::default(),
        )
        .unwrap();
        assert!(needed > idle.consumed.model_storage_bytes);
        if let Some(expected) = admission_retained {
            assert_eq!(
                idle.consumed.model_storage_bytes,
                expected + target_admission_bytes,
                "target admission retains its independently pinned scratch while sharing the snapshot"
            );
        } else {
            admission_retained = Some(idle.consumed.model_storage_bytes);
        }
        if let Some(expected) = retained {
            // A second target owns declaration/Type copies (1428+2527),
            // schema validation (14039: includes the new128-byte validation state;
            // children default [] costs two fewer
            // metadata bytes than null), declaration cursors/selected Diamond
            // key (416), and membership scratch (2533 ancestry+142 keys).
            // Structural revision/identity/object pages copy "d" three times
            // during shared snapshot admission (3*97 bytes), not per target.
            // That common cost cancels here; independent extra was 21085 before adding the canonical source URI suffix.
            assert!(
                needed > expected + target_admission_bytes,
                "the second forward qualification adds storage beyond target admission"
            );
        } else {
            retained = Some(needed);
        }
        for limit in [0, needed - 1, needed, needed + 1] {
            let limits = ExecutionLimits {
                max_model_storage_bytes: limit,
                ..Default::default()
            };
            let mut previous = None;
            for quantum in [1, 3, 4096] {
                let mut session =
                    EvaluationSession::new(&image, &arguments, &context, &limits).unwrap();
                let result = loop {
                    match session.resume(NonZeroUsize::new(quantum).unwrap()) {
                        SessionProgress::Pending => {}
                        SessionProgress::Complete(result) => break *result,
                        SessionProgress::Finished => panic!("missing result"),
                    }
                };
                let wire = if limit < needed {
                    let error = result.unwrap_err();
                    assert_eq!(error.kind, ExecutionErrorKind::ResourceExhausted);
                    assert_eq!(&*error.message, "Retained model storage limit exceeded");
                    assert!(error.consumed.model_storage_bytes <= limit);
                    assert_eq!(
                        error.consumed.reference_domains,
                        if limit == 0 { 0 } else { parameters.len() }
                    );
                    assert_eq!(error.consumed.provider_invocations, 0);
                    if limit == 0 {
                        assert_eq!(error.consumed.model_storage_bytes, 0);
                    }
                    serde_json::to_value(error).unwrap()
                } else {
                    let result = result.unwrap();
                    assert_eq!(
                        serde_json::to_value(&result).unwrap(),
                        serde_json::to_value(&baseline).unwrap()
                    );
                    serde_json::to_value(result).unwrap()
                };
                if let Some(expected) = &previous {
                    assert_eq!(&wire, expected);
                }
                previous = Some(wire);
                assert!(matches!(
                    session.resume(NonZeroUsize::MIN),
                    SessionProgress::Finished
                ));
            }
        }
        assert_eq!(references[0], reference);
    }
    let defaults = ExecutionLimits::default();
    let mut legacy = serde_json::to_value(&defaults).unwrap();
    legacy
        .as_object_mut()
        .unwrap()
        .remove("max_model_storage_bytes");
    assert_eq!(
        serde_json::from_value::<ExecutionLimits>(legacy)
            .unwrap()
            .max_model_storage_bytes,
        defaults.max_model_storage_bytes
    );
}
fn setup_model(
    ecore_text: &str,
    body: &str,
    label: Option<serde_json::Value>,
) -> (DefinitionBundle, DefinitionSource, ReferenceContext) {
    let ecore = intersect_model_acceptance::migration_fixture::migrate_source_fixture(
        DefinitionSource::new("memory:navigation.ecore", SourceKind::Ecore, ecore_text),
    )
    .0;
    let model_bundle = DefinitionBundle {
        version: 1,
        sources: vec![ecore.clone()],
        namespaces: vec![],
    };
    let schema = intersect_model::definitions::compiler::compile_structure_bundle_with_conversions(
        &model_bundle,
        &ecore.id,
        "urn:intersect:navigation",
        &ConversionRegistry::default(),
    )
    .unwrap();
    let mut model = Model::default();
    model.create(&schema, "d", "Diamond", "r", 0, 0).unwrap();
    if let Some(label) = label {
        model.elements[0].values.insert("label".into(), label);
    }
    model.validate(&schema).unwrap();
    let reference = ReferenceContext {
        bundle: model_bundle,
        entry: ecore.id.clone(),
        namespace: "urn:intersect:navigation".into(),
        conversions: vec![],
        materialized_types: vec![],
        applied_conversions: vec![],
        constructors: vec![],
        model: serde_json::to_value(model).unwrap(),
    };
    let mut source = DefinitionSource::new(
        "memory:navigation-behavior.rcore",
        SourceKind::RcoreBehavior,
        format!(
            "rcore 1; package navigation version \"1\"; import model \"urn:intersect:navigation\" as m; {body}"
        ),
    );
    source.dependencies.push(ecore.id.clone());
    (
        DefinitionBundle {
            version: 1,
            sources: vec![ecore, source.clone()],
            namespaces: vec![],
        },
        source,
        reference,
    )
}

#[test]
fn stored_scalar_member_executes_through_calls_and_preserves_context_identity() {
    let (bundle, source, reference) =
        setup("fn main(x:m.Base)->Option<String> ={let f=|unused:Int|x.label;f(0)};");
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
    let TypeNode::Model(target) = &image.program().types[ty.0 as usize] else {
        panic!("model type")
    };
    let domain = ReferenceDomain::new_runtime(
        target.clone(),
        reference.clone(),
        &ConversionRegistry::default(),
    )
    .unwrap();
    let argument = ValueWire {
        ty,
        data: DataWire::Model(domain.bind("d").unwrap()),
    };
    let references = [reference];
    let mut context = ExecutionContext::new(&bundle);
    context.references = &references;
    let result = evaluate(
        &image,
        std::slice::from_ref(&argument),
        &context,
        &ExecutionLimits::default(),
    )
    .unwrap();
    let DataWire::Some(value) = result.value.data else {
        panic!("optional default")
    };
    let DataWire::Scalar(value) = value.data else {
        panic!("scalar")
    };
    assert_eq!(value.payload, json!("seed"));
    assert!(
        evaluate(
            &image,
            std::slice::from_ref(&argument),
            &ExecutionContext::new(&bundle),
            &ExecutionLimits::default()
        )
        .is_err()
    );
    let mut forged = image.program().clone();
    let instruction = forged
        .functions
        .iter_mut()
        .flat_map(|f| &mut f.blocks)
        .flat_map(|b| &mut b.instructions)
        .find(|i| matches!(i.kind, InstructionKind::StoredFeature { .. }))
        .unwrap();
    if let InstructionKind::StoredFeature { declaration, .. } = &mut instruction.kind {
        declaration.path.push(999);
    }
    assert!(ir_v2::verify(forged, &bundle, &Limits::default()).is_err());
    let mut wrong_result = image.program().clone();
    wrong_result
        .functions
        .iter_mut()
        .flat_map(|f| &mut f.blocks)
        .flat_map(|b| &mut b.instructions)
        .find(|i| matches!(i.kind, InstructionKind::StoredFeature { .. }))
        .unwrap()
        .ty = ty;
    assert!(ir_v2::verify(wrong_result, &bundle, &Limits::default()).is_err());
    assert!(evaluate(&image, &[argument], &context, &ExecutionLimits::default()).is_ok());
}

#[test]
fn source_navigation_separates_deleted_recreated_and_pinned_snapshot_identities() {
    use intersect_model::core::{
        api::{BasicModel, ModelEdit},
        frozen::FrozenStore,
        query::QueryBudget,
        storage::{DeleteRequest, DeleteSelection, IncomingReferences},
    };
    use intersect_model::rcore_behavior_functions::{DependencyCause, ExecutionErrorKind};
    let (bundle, source, original) = setup("fn main(x:m.Base)->Option<String> =x.label;");
    let image = compile_navigation(&bundle, &source);
    let ty = image.program().functions[image.program().entry.0 as usize].parameters[0];
    let argument = |reference: &ReferenceContext| ValueWire {
        ty,
        data: DataWire::Model(json!({
            "kind": "model_reference", "context": reference.digest().unwrap(), "element": "d"
        })),
    };
    let run = |reference: &ReferenceContext, argument: &ValueWire| {
        let mut context = ExecutionContext::new(&bundle);
        context.references = std::slice::from_ref(reference);
        evaluate(
            &image,
            std::slice::from_ref(argument),
            &context,
            &ExecutionLimits::default(),
        )
    };
    let schema = intersect_model::definitions::compiler::compile_structure_bundle_with_conversions(
        &original.bundle,
        &original.entry,
        &original.namespace,
        &ConversionRegistry::default(),
    )
    .unwrap();
    let model: Model = serde_json::from_value(original.model.clone()).unwrap();
    let store = FrozenStore::from_materialized(&schema, &model).unwrap();
    let mut live = BasicModel::from_store(schema.clone(), store).unwrap();
    assert_eq!(
        live.delete_objects(
            DeleteRequest {
                roots: &["d"],
                selection: DeleteSelection::Objects,
                incoming: IncomingReferences::Reject,
            },
            None,
            &mut QueryBudget::new(100_000)
        )
        .unwrap(),
        vec!["d"]
    );
    live.validate().unwrap();
    let deleted_model = live.into_store().into_model().unwrap();
    let mut deleted = original.clone();
    deleted.model = serde_json::to_value(&deleted_model).unwrap();
    let store = FrozenStore::from_materialized(&schema, &deleted_model).unwrap();
    let mut live = BasicModel::from_store(schema, store).unwrap();
    live.create("d", "Diamond", "r").unwrap();
    live.set("d", "label", json!("replacement")).unwrap();
    live.validate().unwrap();
    let mut recreated = original.clone();
    recreated.model = serde_json::to_value(live.into_store().into_model().unwrap()).unwrap();
    assert_ne!(original.digest().unwrap(), deleted.digest().unwrap());
    assert_ne!(original.digest().unwrap(), recreated.digest().unwrap());
    assert_ne!(deleted.digest().unwrap(), recreated.digest().unwrap());
    let original_argument = argument(&original);
    for reference in [&deleted, &recreated] {
        let error = run(reference, &original_argument).unwrap_err();
        assert_eq!(error.kind, ExecutionErrorKind::MissingDependency);
        assert_eq!(
            error.dependency.as_deref(),
            Some(&DependencyCause::ModelContextRevision)
        );
    }
    let missing = run(&deleted, &argument(&deleted)).unwrap_err();
    assert_eq!(missing.kind, ExecutionErrorKind::Nominal);
    assert!(missing.message.contains("d"), "{missing:?}");
    for (reference, expected) in [
        (&original, "seed"),
        (&recreated, "replacement"),
        (&original, "seed"),
    ] {
        let before = reference.clone();
        let value = run(reference, &argument(reference)).unwrap().value;
        let DataWire::Some(inner) = value.data else {
            panic!("present label")
        };
        let DataWire::Scalar(scalar) = inner.data else {
            panic!("string label")
        };
        assert_eq!(scalar.payload, json!(expected));
        assert_eq!(*reference, before);
    }
    let mut changed_source = DefinitionSource::new(
        &source.id.uri,
        SourceKind::RcoreBehavior,
        source
            .text
            .replace("=x.label", "=Some(\"changed program\")"),
    );
    changed_source.dependencies = source.dependencies.clone();
    let mut changed_bundle = bundle.clone();
    *changed_bundle
        .sources
        .iter_mut()
        .find(|s| s.id == source.id)
        .unwrap() = changed_source;
    let mut changed_context = ExecutionContext::new(&changed_bundle);
    changed_context.references = std::slice::from_ref(&original);
    let error = evaluate(
        &image,
        &[original_argument],
        &changed_context,
        &ExecutionLimits::default(),
    )
    .unwrap_err();
    assert_eq!(error.kind, ExecutionErrorKind::MissingDependency);
    let manifest_index = image
        .program()
        .sources
        .iter()
        .position(|s| s.source == source.id)
        .unwrap();
    assert_eq!(
        error.dependency.as_deref(),
        Some(&DependencyCause::ExecutableContext(
            ir_v2::ContextDependency::SourceRevision { manifest_index }
        ))
    );
    let saved = serde_json::to_vec(image.program()).unwrap();
    let restored = ir_v2::verify(
        serde_json::from_slice(&saved).unwrap(),
        &bundle,
        &Limits::default(),
    )
    .unwrap();
    assert!(
        ir_v2::verify(
            serde_json::from_slice(&saved).unwrap(),
            &changed_bundle,
            &Limits::default()
        )
        .is_err()
    );
    let mut pinned_context = ExecutionContext::new(&bundle);
    pinned_context.references = std::slice::from_ref(&original);
    assert!(
        evaluate(
            &restored,
            &[argument(&original)],
            &pinned_context,
            &ExecutionLimits::default()
        )
        .is_ok()
    );
}

#[test]
fn absent_stored_scalar_reads_use_schema_defaults_or_nullable_absence() {
    for default in [true, false] {
        let text = include_str!("../../../../fixtures/ecore-navigation/projection.ecore");
        let text = if default {
            text.into()
        } else {
            text.replace(" defaultValueLiteral=\"seed\"", "")
        };
        let (bundle, source, mut reference) =
            setup_model(&text, "fn main(x:m.Base)->Option<String> =x.label;", None);
        reference.model["elements"][0]["values"]
            .as_object_mut()
            .unwrap()
            .remove("label");
        let before = reference.clone();
        let image = compile_navigation(&bundle, &source);
        let argument = navigation_argument(&image, &reference);
        let mut context = ExecutionContext::new(&bundle);
        context.references = std::slice::from_ref(&reference);
        let result = evaluate(&image, &[argument], &context, &ExecutionLimits::default()).unwrap();
        if default {
            let DataWire::Some(value) = result.value.data else {
                panic!("default value")
            };
            let DataWire::Scalar(value) = value.data else {
                panic!("string")
            };
            assert_eq!(value.payload, json!("seed"));
        } else {
            assert_eq!(result.value.data, DataWire::None);
        }
        assert_eq!(reference, before);
    }
}

#[test]
fn safe_absent_receiver_skips_model_context_and_read() {
    let (bundle, source, reference) = setup("fn main(x:Option<m.Base>)->Option<String> =x?.label;");
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
    let result = evaluate(
        &image,
        &[ValueWire {
            ty,
            data: DataWire::None,
        }],
        &ExecutionContext::new(&bundle),
        &ExecutionLimits::default(),
    )
    .unwrap();
    assert!(matches!(result.value.data, DataWire::None));
    let TypeNode::Option(inner) = image.program().types[ty.0 as usize] else {
        panic!("optional receiver")
    };
    let TypeNode::Model(target) = &image.program().types[inner.0 as usize] else {
        panic!("model receiver")
    };
    let domain = ReferenceDomain::new_runtime(
        target.clone(),
        reference.clone(),
        &ConversionRegistry::default(),
    )
    .unwrap();
    let references = [reference];
    let mut context = ExecutionContext::new(&bundle);
    context.references = &references;
    let result = evaluate(
        &image,
        &[ValueWire {
            ty,
            data: DataWire::Some(Box::new(ValueWire {
                ty: inner,
                data: DataWire::Model(domain.bind("d").unwrap()),
            })),
        }],
        &context,
        &ExecutionLimits::default(),
    )
    .unwrap();
    let DataWire::Some(value) = result.value.data else {
        panic!("present optional field")
    };
    let DataWire::Scalar(value) = value.data else {
        panic!("no double Option wrapping")
    };
    assert_eq!(value.payload, json!("seed"));
}

#[test]
fn derived_member_rejects_even_in_a_dead_branch() {
    let (bundle, source, _) =
        setup("fn main(x:m.Base)->Bool=if true {true} else {x.computed==Some(\"seed\")};");
    let errors = compile_function(
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
    .unwrap_err();
    assert!(
        errors
            .iter()
            .any(|e| e.message.contains("Derived model navigation")),
        "{errors:?}"
    );
}

#[test]
fn every_ecore_scalar_representation_is_read_without_narrowing_or_alias_conversion() {
    let cases = [
        ("EBoolean", "Bool", json!(true)),
        ("EString", "String", json!("e\u{301}🙂")),
        (
            "EByteArray",
            "Bytes",
            json!({"kind":"bytes","hex":"00ABFF"}),
        ),
        ("EChar", "Char16", json!({"kind":"char16","unit":55296})),
        (
            "EDate",
            "DateInstant",
            json!({"kind":"date","milliseconds":"-9223372036854775808"}),
        ),
        ("EByte", "Int8", json!(-128)),
        ("EShort", "Int16", json!(-32768)),
        ("EInt", "Int32", json!(-2147483648_i64)),
        (
            "ELong",
            "Int64",
            json!({"kind":"int64","value":"-9223372036854775808"}),
        ),
        (
            "EBigInteger",
            "BigInt",
            json!({"kind":"bigint","value":"123456789012345678901234567890"}),
        ),
        (
            "EBigDecimal",
            "Decimal",
            json!({"kind":"decimal","coefficient":"1234567890123456789012300","scale":4}),
        ),
        (
            "EFloat",
            "Float32",
            json!({"kind":"float32","bits":"7fc00000"}),
        ),
        (
            "EDouble",
            "Float64",
            json!({"kind":"float64","bits":"8000000000000000"}),
        ),
    ];
    for (ecore, scalar, payload) in cases {
        for optional in [false, true] {
            let lower = if optional { 0 } else { 1 };
            let text = format!(
                r##"<e:EPackage xmlns:e="http://www.eclipse.org/emf/2002/Ecore" xmlns:x="http://www.w3.org/2001/XMLSchema-instance" name="n" nsURI="urn:intersect:navigation"><eClassifiers x:type="e:EClass" name="Base"><eStructuralFeatures x:type="e:EAttribute" name="label" lowerBound="{lower}" eType="e:EDataType http://www.eclipse.org/emf/2002/Ecore#//{ecore}"/></eClassifiers><eClassifiers x:type="e:EClass" name="Diamond" eSuperTypes="#//Base"/></e:EPackage>"##
            );
            let output = if optional {
                format!("Option<{scalar}>")
            } else {
                scalar.to_owned()
            };
            let (bundle, source, reference) = setup_model(
                &text,
                &format!("fn main(x:m.Base)->{output} =x.label;"),
                Some(if optional {
                    json!(null)
                } else {
                    payload.clone()
                }),
            );
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
            let TypeNode::Model(target) = &image.program().types[ty.0 as usize] else {
                panic!("model")
            };
            let domain = ReferenceDomain::new_runtime(
                target.clone(),
                reference.clone(),
                &ConversionRegistry::default(),
            )
            .unwrap();
            let argument = ValueWire {
                ty,
                data: DataWire::Model(domain.bind("d").unwrap()),
            };
            let references = [reference];
            let mut context = ExecutionContext::new(&bundle);
            context.references = &references;
            let result =
                evaluate(&image, &[argument], &context, &ExecutionLimits::default()).unwrap();
            if optional {
                assert!(matches!(result.value.data, DataWire::None));
            } else {
                let DataWire::Scalar(value) = result.value.data else {
                    panic!("{scalar}")
                };
                assert_eq!(value.payload, payload, "{scalar}");
            }
        }
    }
}

#[test]
fn singular_nominal_reads_preserve_enum_and_reference_identities() {
    for kind in ["enum", "reference", "datatype"] {
        let reference_member = kind == "reference";
        for absent in [false, true] {
            let member = if reference_member {
                r##"<eStructuralFeatures x:type="e:EReference" name="label" eType="#//Base"/>"##
            } else if kind == "datatype" {
                r##"<eStructuralFeatures x:type="e:EAttribute" name="label" eType="#//Text" defaultValueLiteral="seed"/>"##
            } else {
                r##"<eStructuralFeatures x:type="e:EAttribute" name="label" eType="#//Choice" defaultValueLiteral="first"/>"##
            };
            let text = format!(
                r##"<e:EPackage xmlns:e="http://www.eclipse.org/emf/2002/Ecore" xmlns:x="http://www.w3.org/2001/XMLSchema-instance" name="n" nsURI="urn:intersect:navigation"><eClassifiers x:type="e:EDataType" name="Text" instanceClassName="java.lang.String"/><eClassifiers x:type="e:EEnum" name="Choice"><eLiterals name="First" literal="first"/><eLiterals name="Alias" literal="alias" value="0"/></eClassifiers><eClassifiers x:type="e:EClass" name="Base">{member}</eClassifiers><eClassifiers x:type="e:EClass" name="Diamond" eSuperTypes="#//Base"/></e:EPackage>"##
            );
            let output = if reference_member {
                "m.Base"
            } else if kind == "datatype" {
                "m.Text"
            } else {
                "m.Choice"
            };
            let (bundle, source, reference) = setup_model(
                &text,
                &format!("fn main(x:m.Base)->Option<{output}> =x.label;"),
                if absent {
                    Some(json!(null))
                } else if reference_member {
                    Some(json!("d"))
                } else {
                    None
                },
            );
            let expected = reference.model["elements"][0]["values"]["label"].clone();
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
            let TypeNode::Model(target) = &image.program().types[ty.0 as usize] else {
                panic!("model")
            };
            let domain = ReferenceDomain::new_runtime(
                target.clone(),
                reference.clone(),
                &ConversionRegistry::default(),
            )
            .unwrap();
            let bound = domain.bind("d").unwrap();
            let references = [reference];
            let mut context = ExecutionContext::new(&bundle);
            context.references = &references;
            let result = evaluate(
                &image,
                &[ValueWire {
                    ty,
                    data: DataWire::Model(bound.clone()),
                }],
                &context,
                &ExecutionLimits::default(),
            )
            .unwrap();
            if absent {
                assert!(matches!(result.value.data, DataWire::None));
            } else {
                let DataWire::Some(value) = result.value.data else {
                    panic!("present nominal")
                };
                if reference_member {
                    let DataWire::Model(value) = value.data else {
                        panic!("reference")
                    };
                    assert_eq!(value, bound);
                } else if kind == "datatype" {
                    let DataWire::Datatype(value) = value.data else {
                        panic!("datatype")
                    };
                    assert_eq!(value, expected);
                } else {
                    let DataWire::Enum(value) = value.data else {
                        panic!("enum")
                    };
                    assert_eq!(serde_json::to_value(value).unwrap(), expected);
                }
            }
        }
    }
}

fn compile_navigation(
    bundle: &DefinitionBundle,
    source: &DefinitionSource,
) -> ir_v2::VerifiedExecutable {
    compile_function(
        bundle,
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
    .unwrap()
}
fn navigation_argument(
    image: &ir_v2::VerifiedExecutable,
    reference: &ReferenceContext,
) -> ValueWire {
    let ty = image.program().functions[image.program().entry.0 as usize].parameters[0];
    let TypeNode::Model(target) = &image.program().types[ty.0 as usize] else {
        panic!("model")
    };
    let domain = ReferenceDomain::new_runtime(
        target.clone(),
        reference.clone(),
        &ConversionRegistry::default(),
    )
    .unwrap();
    ValueWire {
        ty,
        data: DataWire::Model(domain.bind("d").unwrap()),
    }
}

#[test]
fn singular_scalar_reads_yield_member_qualification_and_preserve_quantum_independent_results() {
    use intersect_model::rcore_behavior_functions::{
        EvaluationSession, ExecutionControl, ExecutionErrorKind, SessionProgress,
    };
    use std::num::NonZeroUsize;
    struct Cancel;
    impl ExecutionControl for Cancel {
        fn cancellation_requested(&self) -> bool {
            true
        }
    }
    let fields = (0..64).map(|n| format!(r#"<eStructuralFeatures x:type="e:EAttribute" name="f{n:02}" defaultValueLiteral="last" eType="e:EDataType http://www.eclipse.org/emf/2002/Ecore#//EString"/>"#)).collect::<String>();
    let text = format!(
        r#"<e:EPackage xmlns:e="http://www.eclipse.org/emf/2002/Ecore" xmlns:x="http://www.w3.org/2001/XMLSchema-instance" name="m" nsURI="urn:intersect:navigation"><eClassifiers x:type="e:EClass" name="Diamond">{fields}</eClassifiers></e:EPackage>"#
    );
    let mut read_steps = 0;
    let mut unused_steps = 0;
    for (body, reads) in [
        ("fn main(x:m.Diamond)->Option<String> =x.f63;", true),
        ("fn main(x:m.Diamond)->Int =0;", false),
    ] {
        let (bundle, source, reference) = setup_model(&text, body, None);
        let image = compile_navigation(&bundle, &source);
        let arguments = [navigation_argument(&image, &reference)];
        let references = [reference];
        let mut context = ExecutionContext::new(&bundle);
        context.references = &references;
        let limits = ExecutionLimits::default();
        let baseline = evaluate(&image, &arguments, &context, &limits).unwrap();
        if !reads {
            unused_steps = baseline.consumed.steps;
            continue;
        }
        read_steps = baseline.consumed.steps;
        let DataWire::Some(value) = &baseline.value.data else {
            panic!("optional scalar")
        };
        let DataWire::Scalar(value) = &value.data else {
            panic!("scalar")
        };
        assert_eq!(value.payload, json!("last"));
        let expected = serde_json::to_value(&baseline).unwrap();
        for quantum in [1, 3, 4096] {
            let mut session =
                EvaluationSession::new(&image, &arguments, &context, &limits).unwrap();
            let mut pending = 0;
            loop {
                match session.resume(NonZeroUsize::new(quantum).unwrap()) {
                    SessionProgress::Pending => {
                        pending += 1;
                        assert!(pending < 10_000);
                    }
                    SessionProgress::Complete(outcome) => {
                        assert_eq!(serde_json::to_value(outcome.unwrap()).unwrap(), expected);
                        break;
                    }
                    SessionProgress::Finished => panic!("missing scalar result"),
                }
            }
            if quantum == 1 {
                for boundary in [0, pending / 2, pending - 1] {
                    let mut cancelled =
                        EvaluationSession::new(&image, &arguments, &context, &limits).unwrap();
                    for _ in 0..boundary {
                        assert!(matches!(
                            cancelled.resume(NonZeroUsize::MIN),
                            SessionProgress::Pending
                        ));
                    }
                    let SessionProgress::Complete(outcome) =
                        cancelled.resume_with_control(NonZeroUsize::MIN, &Cancel)
                    else {
                        panic!("missing cancellation")
                    };
                    assert_eq!(outcome.unwrap_err().kind, ExecutionErrorKind::Cancelled);
                    assert!(matches!(
                        cancelled.resume(NonZeroUsize::MAX),
                        SessionProgress::Finished
                    ));
                }
            }
        }
    }
    // Identical model admission occurs for both entries. Qualification must add
    // per-member work beyond the unused-model function's admission charges.
    assert!(read_steps >= unused_steps + 64 * 8);
}

#[test]
fn wide_inverse_qualification_yields_and_cancels_across_host_quanta() {
    use intersect_model::rcore_behavior_functions::{
        EvaluationSession, ExecutionControl, ExecutionErrorKind, SessionProgress,
    };
    use std::num::NonZeroUsize;
    struct Cancel;
    impl ExecutionControl for Cancel {
        fn cancellation_requested(&self) -> bool {
            true
        }
    }
    let mut fields = (0..63).map(|n| format!(r#"<eStructuralFeatures x:type="e:EAttribute" name="f{n:02}" eType="e:EDataType http://www.eclipse.org/emf/2002/Ecore#//EString"/>"#)).collect::<String>();
    fields.push_str(r##"<eStructuralFeatures x:type="e:EReference" name="links" upperBound="-1" unique="false" eType="#//Diamond"/>"##);
    let text = format!(
        r#"<e:EPackage xmlns:e="http://www.eclipse.org/emf/2002/Ecore" xmlns:x="http://www.w3.org/2001/XMLSchema-instance" name="m" nsURI="urn:intersect:navigation"><eClassifiers x:type="e:EClass" name="Diamond">{fields}</eClassifiers></e:EPackage>"#
    );
    let mut query_steps = 0;
    let mut unused_steps = 0;
    for (body, queries) in [
        (
            "fn main(x:m.Diamond)->OrderedSet<m.Diamond> =incoming::<m.Diamond>(x,\"links\");",
            true,
        ),
        ("fn main(x:m.Diamond)->Int =0;", false),
    ] {
        let (bundle, source, mut reference) = setup_model(&text, body, None);
        reference.model["elements"][0]["values"]["links"] = json!(["d", "d"]);
        let image = compile_navigation(&bundle, &source);
        let arguments = [navigation_argument(&image, &reference)];
        let references = [reference];
        let mut context = ExecutionContext::new(&bundle);
        context.references = &references;
        let limits = ExecutionLimits::default();
        let baseline = evaluate(&image, &arguments, &context, &limits).unwrap();
        if !queries {
            unused_steps = baseline.consumed.steps;
            continue;
        }
        query_steps = baseline.consumed.steps;
        let DataWire::Collection(values) = &baseline.value.data else {
            panic!("inverse collection")
        };
        assert_eq!(values.len(), 1);
        let DataWire::Model(value) = &values[0].data else {
            panic!("inverse model")
        };
        assert_eq!(value["element"], json!("d"));
        let expected = serde_json::to_value(&baseline).unwrap();
        for quantum in [1, 3, 4096] {
            let mut session =
                EvaluationSession::new(&image, &arguments, &context, &limits).unwrap();
            let mut pending = 0;
            loop {
                match session.resume(NonZeroUsize::new(quantum).unwrap()) {
                    SessionProgress::Pending => {
                        pending += 1;
                        assert!(pending < 20000);
                    }
                    SessionProgress::Complete(result) => {
                        assert_eq!(serde_json::to_value(result.unwrap()).unwrap(), expected);
                        break;
                    }
                    SessionProgress::Finished => panic!("missing inverse result"),
                }
            }
            if quantum == 1 {
                for boundary in [0, pending / 2, pending - 1] {
                    let mut session =
                        EvaluationSession::new(&image, &arguments, &context, &limits).unwrap();
                    for _ in 0..boundary {
                        assert!(matches!(
                            session.resume(NonZeroUsize::MIN),
                            SessionProgress::Pending
                        ));
                    }
                    let SessionProgress::Complete(result) =
                        session.resume_with_control(NonZeroUsize::MIN, &Cancel)
                    else {
                        panic!("missing inverse cancellation")
                    };
                    assert_eq!(result.unwrap_err().kind, ExecutionErrorKind::Cancelled);
                    assert!(matches!(
                        session.resume(NonZeroUsize::MAX),
                        SessionProgress::Finished
                    ));
                }
            }
        }
    }
    assert!(query_steps >= unused_steps + 64 * 16);
}

#[test]
fn many_reads_preserve_all_order_uniqueness_flags_and_leaf_families() {
    use intersect_model::definitions::{datatypes::DatatypeValue, enums::EnumValue};
    for ordered in [false, true] {
        for unique in [false, true] {
            for kind in ["scalar", "enum", "datatype", "reference"] {
                let collection = if ordered { "Sequence" } else { "Bag" };
                let (feature_kind, ecore_type, result_type) = match kind {
                    "scalar" => (
                        "EAttribute",
                        "e:EDataType http://www.eclipse.org/emf/2002/Ecore#//EString",
                        "String",
                    ),
                    "enum" => ("EAttribute", "#//Choice", "m.Choice"),
                    "datatype" => ("EAttribute", "#//Text", "m.Text"),
                    _ => ("EReference", "#//Base", "m.Base"),
                };
                let text = format!(
                    r##"<e:EPackage xmlns:e="http://www.eclipse.org/emf/2002/Ecore" xmlns:x="http://www.w3.org/2001/XMLSchema-instance" name="n" nsURI="urn:intersect:navigation"><eClassifiers x:type="e:EEnum" name="Choice"><eLiterals name="First" literal="first"/><eLiterals name="Second" literal="second" value="1"/></eClassifiers><eClassifiers x:type="e:EDataType" name="Text" instanceClassName="java.lang.String"/><eClassifiers x:type="e:EClass" name="Base"><eStructuralFeatures x:type="e:{feature_kind}" name="label" eType="{ecore_type}" ordered="{ordered}" unique="{unique}" upperBound="-1"/></eClassifiers><eClassifiers x:type="e:EClass" name="Diamond" eSuperTypes="#//Base"/></e:EPackage>"##
                );
                let (_, declarations) =
                    intersect_model_acceptance::migration_fixture::migrate_source_fixture(
                        DefinitionSource::new("memory:navigation.ecore", SourceKind::Ecore, &text),
                    );
                let nominal = |path: Vec<usize>| declarations[&path].clone();
                let values: Vec<serde_json::Value> = match kind {
                    "scalar" => vec![json!("second"), json!("first")],
                    "enum" => [1, 0]
                        .into_iter()
                        .map(|n| {
                            serde_json::to_value(EnumValue::Enum {
                                enumeration: nominal(vec![0]),
                                literal: nominal(vec![0, n]),
                            })
                            .unwrap()
                        })
                        .collect(),
                    "datatype" => ["second", "first"]
                        .into_iter()
                        .map(|v| {
                            serde_json::to_value(DatatypeValue::Datatype {
                                datatype: nominal(vec![1]),
                                value: json!(v),
                            })
                            .unwrap()
                        })
                        .collect(),
                    _ => vec![json!("d")],
                };
                for empty in [false, true] {
                    let mut stored = if empty { vec![] } else { values.clone() };
                    if !empty && !unique {
                        stored.push(values[0].clone());
                    }
                    let (bundle, source, reference) = setup_model(
                        &text,
                        &format!(
                            "fn main(x:m.Base)->{collection}<{result_type}> =map(x.label,|v:{result_type}|v);"
                        ),
                        Some(json!(stored)),
                    );
                    let image = compile_navigation(&bundle, &source);
                    let instruction = image
                        .program()
                        .functions
                        .iter()
                        .flat_map(|f| &f.blocks)
                        .flat_map(|b| &b.instructions)
                        .find(|i| matches!(i.kind, InstructionKind::StoredFeature { .. }))
                        .unwrap();
                    assert!(
                        matches!(image.program().types[instruction.ty.0 as usize], TypeNode::Many { ordered: o, unique: u, .. } if o == ordered && u == unique)
                    );
                    let argument = navigation_argument(&image, &reference);
                    let DataWire::Model(bound) = &argument.data else {
                        panic!("model")
                    };
                    let expected = if kind == "reference" {
                        vec![bound.clone(); stored.len()]
                    } else {
                        stored
                    };
                    let references = [reference];
                    let mut context = ExecutionContext::new(&bundle);
                    context.references = &references;
                    let result =
                        evaluate(&image, &[argument], &context, &ExecutionLimits::default())
                            .unwrap();
                    let DataWire::Collection(items) = result.value.data else {
                        panic!("collection")
                    };
                    let actual: Vec<_> = items
                        .into_iter()
                        .map(|v| match v.data {
                            DataWire::Scalar(v) => v.payload,
                            DataWire::Enum(v) => serde_json::to_value(v).unwrap(),
                            DataWire::Datatype(v) | DataWire::Model(v) => v,
                            other => panic!("unexpected {other:?}"),
                        })
                        .collect();
                    assert_eq!(
                        actual, expected,
                        "{kind}: ordered={ordered}, unique={unique}, empty={empty}"
                    );
                }
            }
        }
    }
}

fn many_strings(
    values: Vec<serde_json::Value>,
    body: &str,
    lower: usize,
    upper: i32,
) -> (DefinitionBundle, DefinitionSource, ReferenceContext) {
    let text = format!(
        r##"<e:EPackage xmlns:e="http://www.eclipse.org/emf/2002/Ecore" xmlns:x="http://www.w3.org/2001/XMLSchema-instance" name="n" nsURI="urn:intersect:navigation"><eClassifiers x:type="e:EClass" name="Base"><eStructuralFeatures x:type="e:EAttribute" name="label" eType="e:EDataType http://www.eclipse.org/emf/2002/Ecore#//EString" lowerBound="{lower}" upperBound="{upper}" unique="false"/></eClassifiers><eClassifiers x:type="e:EClass" name="Diamond" eSuperTypes="#//Base"/></e:EPackage>"##
    );
    setup_model(&text, body, Some(json!(values)))
}

#[test]
fn many_read_sessions_yield_cancel_and_keep_resource_accounting_independent_of_quantum() {
    use intersect_model::rcore_behavior_functions::{
        EvaluationSession, ExecutionControl, ExecutionErrorKind, SessionProgress,
    };
    use std::num::NonZeroUsize;
    struct Cancel;
    impl ExecutionControl for Cancel {
        fn cancellation_requested(&self) -> bool {
            true
        }
    }
    let expected_values: Vec<_> = (0..8).map(|n| json!(format!("entry-{n}"))).collect();
    let (bundle, source, reference) = many_strings(
        expected_values.clone(),
        "fn main(x:m.Base)->Sequence<String> =filter(x.label,|v:String|true);",
        2,
        8,
    );
    let image = compile_navigation(&bundle, &source);
    let arguments = [navigation_argument(&image, &reference)];
    let references = [reference];
    let mut context = ExecutionContext::new(&bundle);
    context.references = &references;
    let limits = ExecutionLimits::default();
    let baseline = evaluate(&image, &arguments, &context, &limits).unwrap();
    let DataWire::Collection(values) = &baseline.value.data else {
        panic!("collection")
    };
    let actual: Vec<_> = values
        .iter()
        .map(|v| match &v.data {
            DataWire::Scalar(v) => v.payload.clone(),
            _ => panic!("scalar"),
        })
        .collect();
    assert_eq!(actual, expected_values);
    let expected = serde_json::to_value(&baseline).unwrap();
    let mut boundaries = 0;
    for quantum in [1, 3, 4096] {
        let mut session = EvaluationSession::new(&image, &arguments, &context, &limits).unwrap();
        let mut pending = 0;
        loop {
            match session.resume(NonZeroUsize::new(quantum).unwrap()) {
                SessionProgress::Pending => {
                    pending += 1;
                    assert!(pending < 10000);
                }
                SessionProgress::Complete(outcome) => {
                    assert_eq!(serde_json::to_value(outcome.unwrap()).unwrap(), expected);
                    break;
                }
                SessionProgress::Finished => panic!("missing result"),
            }
        }
        if quantum == 1 {
            boundaries = pending;
        }
    }
    assert!(boundaries > expected_values.len());
    for boundary in 0..=boundaries {
        let mut session = EvaluationSession::new(&image, &arguments, &context, &limits).unwrap();
        for _ in 0..boundary {
            assert!(matches!(
                session.resume(NonZeroUsize::MIN),
                SessionProgress::Pending
            ));
        }
        let SessionProgress::Complete(outcome) =
            session.resume_with_control(NonZeroUsize::MIN, &Cancel)
        else {
            panic!("missing cancellation")
        };
        assert_eq!(outcome.unwrap_err().kind, ExecutionErrorKind::Cancelled);
        assert!(matches!(
            session.resume(NonZeroUsize::MAX),
            SessionProgress::Finished
        ));
    }
    for dimension in 0..3 {
        let amount = match dimension {
            0 => baseline.consumed.steps,
            1 => baseline.consumed.value_bytes,
            _ => baseline.consumed.value_nodes,
        };
        // max_value_nodes also bounds context metadata admission, which can
        // exceed this small fixture's runtime arena count.
        let bounds = if dimension == 2 {
            vec![0, amount - 1, limits.max_value_nodes]
        } else {
            vec![0, amount - 1, amount, amount + 1]
        };
        for bound in bounds {
            let mut limits = ExecutionLimits::default();
            match dimension {
                0 => limits.scalar.max_steps = bound,
                1 => limits.scalar.max_value_bytes = bound,
                _ => limits.max_value_nodes = bound,
            };
            let outcome = evaluate(&image, &arguments, &context, &limits);
            assert_eq!(
                outcome.is_ok(),
                bound >= amount,
                "dimension {dimension}, bound {bound}: {outcome:?}"
            );
            if let Err(error) = outcome {
                assert_eq!(error.kind, ExecutionErrorKind::ResourceExhausted);
            }
        }
    }
    assert_eq!(
        serde_json::to_value(evaluate(&image, &arguments, &context, &limits).unwrap()).unwrap(),
        expected
    );
}

#[test]
fn many_reads_compose_in_callbacks_and_safe_access_skips_missing_context() {
    let expected = vec![json!("b"), json!("a"), json!("b")];
    let (bundle, source, reference) = many_strings(
        expected.clone(),
        "fn main(x:m.Base)->Sequence<String> =flatMap([x,x],|v:m.Base|v.label);",
        0,
        -1,
    );
    let image = compile_navigation(&bundle, &source);
    let arguments = [navigation_argument(&image, &reference)];
    let references = [reference];
    let mut context = ExecutionContext::new(&bundle);
    context.references = &references;
    let result = evaluate(&image, &arguments, &context, &ExecutionLimits::default()).unwrap();
    let DataWire::Collection(values) = result.value.data else {
        panic!("collection")
    };
    let actual: Vec<_> = values
        .into_iter()
        .map(|v| match v.data {
            DataWire::Scalar(v) => v.payload,
            _ => panic!("scalar"),
        })
        .collect();
    assert_eq!(actual, [expected.clone(), expected].concat());

    let (bundle, source, _) = many_strings(
        vec![],
        "fn main(x:Option<m.Base>)->Bool =match x?.label {Some(xs)=>isEmpty(xs),None=>true};",
        0,
        -1,
    );
    let image = compile_navigation(&bundle, &source);
    let ty = image.program().functions[image.program().entry.0 as usize].parameters[0];
    let result = evaluate(
        &image,
        &[ValueWire {
            ty,
            data: DataWire::None,
        }],
        &ExecutionContext::new(&bundle),
        &ExecutionLimits::default(),
    )
    .unwrap();
    let DataWire::Scalar(value) = result.value.data else {
        panic!("boolean")
    };
    assert_eq!(value.payload, json!(true));
}

#[test]
fn many_reads_reject_forged_multiplicity_and_malformed_stored_values() {
    let (bundle, source, reference) = many_strings(
        vec![json!("a"), json!("b")],
        "fn main(x:m.Base)->Sequence<String> =map(x.label,|v:String|v);",
        2,
        3,
    );
    let image = compile_navigation(&bundle, &source);
    for dimension in 0..4 {
        let mut forged = image.program().clone();
        let node = forged
            .types
            .iter_mut()
            .find(|t| matches!(t, TypeNode::Many { .. }))
            .unwrap();
        let TypeNode::Many {
            lower,
            upper,
            ordered,
            unique,
            ..
        } = node
        else {
            unreachable!()
        };
        match dimension {
            0 => *lower = 0,
            1 => *upper = -1,
            2 => *ordered = !*ordered,
            _ => *unique = !*unique,
        };
        assert!(ir_v2::verify(forged, &bundle, &Limits::default()).is_err());
    }
    let valid_argument = navigation_argument(&image, &reference);
    for values in [
        json!([]),
        json!(["a", "b", "c", "d"]),
        json!([42, 43]),
        json!("not an array"),
    ] {
        let mut broken = reference.clone();
        broken.model["elements"][0]["values"]["label"] = values;
        let argument = ValueWire {
            ty: valid_argument.ty,
            data: DataWire::Model(json!({
                "kind":"model_reference", "context": broken.digest().unwrap(), "element":"d",
            })),
        };
        let references = [broken];
        let mut context = ExecutionContext::new(&bundle);
        context.references = &references;
        assert!(evaluate(&image, &[argument], &context, &ExecutionLimits::default()).is_err());
        let references = [reference.clone()];
        context.references = &references;
        assert!(
            evaluate(
                &image,
                std::slice::from_ref(&valid_argument),
                &context,
                &ExecutionLimits::default()
            )
            .is_ok()
        );
    }
}

fn incoming_fixture(body: &str) -> (DefinitionBundle, DefinitionSource, ReferenceContext) {
    let text = include_str!("../../../../fixtures/ecore-navigation/projection.ecore").replace(
        "name=\"Other\">", "name=\"Other\"><eStructuralFeatures xsi:type=\"ecore:EReference\" name=\"children\" upperBound=\"-1\" unique=\"false\" eType=\"#//Base\"/>");
    let (bundle, source, mut reference) = setup_model(&text, body, None);
    let schema = intersect_model::definitions::compiler::compile_structure_bundle_with_conversions(
        &reference.bundle,
        &reference.entry,
        &reference.namespace,
        &ConversionRegistry::default(),
    )
    .unwrap();
    let mut model: Model = serde_json::from_value(reference.model).unwrap();
    for (id, class) in [
        ("z", "Other"),
        ("a", "Other"),
        ("unrelated", "Base"),
        ("empty", "Other"),
    ] {
        model.create(&schema, id, class, "r", 0, 0).unwrap();
        if id != "empty" {
            model.elements.last_mut().unwrap().values.insert(
                "children".into(),
                if class == "Other" {
                    json!(["d", "d"])
                } else {
                    json!(["d"])
                },
            );
        }
    }
    model.validate(&schema).unwrap();
    reference.model = serde_json::to_value(model).unwrap();
    (bundle, source, reference)
}

#[test]
fn incoming_executes_cross_type_queries_and_composes_in_callbacks() {
    for body in [
        "fn main(x:m.Base)->OrderedSet<m.Other> =walk::<m.Other,m.Base>(x);fn walk<S:m.Other,T:m.Base>(x:T)->OrderedSet<S> =incoming::<S>(x,\"children\");",
        "fn main(x:m.Base)->OrderedSet<m.Other> =walk::<m.Other,m.Base>(x);fn walk<S:m.Other,T:m.Base>(x:T)->OrderedSet<S> ={let query=|n:T|incoming::<S>(n,\"children\");query(x)};",
        "fn main(x:m.Base)->OrderedSet<m.Other> =incoming::<m.Other>(x,\"children\");",
        "fn main(x:m.Base)->Sequence<m.Other> =flatMap([x],|n:m.Base|incoming::<m.Other>(n,\"children\"));",
    ] {
        let (bundle, source, reference) = incoming_fixture(body);
        let image = compile_navigation(&bundle, &source);
        assert_eq!(
            image.function_effect(image.program().entry).unwrap(),
            ir_v2::Effect::Query
        );
        let arguments = [navigation_argument(&image, &reference)];
        let references = [reference];
        let mut context = ExecutionContext::new(&bundle);
        context.references = &references;
        let result = evaluate(&image, &arguments, &context, &ExecutionLimits::default()).unwrap();
        let DataWire::Collection(values) = result.value.data else {
            panic!("collection")
        };
        let ids: Vec<_> = values
            .iter()
            .map(|v| {
                let DataWire::Model(p) = &v.data else {
                    panic!("model")
                };
                p["element"].as_str().unwrap()
            })
            .collect();
        assert_eq!(ids, ["z", "a"]);
        assert_eq!(result.consumed.model_contexts, 1);
        assert_eq!(result.consumed.reference_domains, 2);
    }
}

#[test]
fn incoming_rejects_dynamic_selectors_dead_invalid_calls_and_forged_ir() {
    for body in [
        "fn main(x:m.Base)->OrderedSet<m.Other> =walk::<m.Other>(x);fn walk<S>(x:m.Base)->OrderedSet<S> =incoming::<S>(x,\"children\");",
        "fn main(x:m.Base)->OrderedSet<m.Other> =walk(x);fn walk<T>(x:T)->OrderedSet<m.Other> =incoming::<m.Other>(x,\"children\");",
        "fn main(x:m.Other)->OrderedSet<m.Other> =walk(x);fn walk<T:m.Other>(x:T)->OrderedSet<m.Other> =incoming::<m.Other>(x,\"children\");",
        "fn main(x:m.Base)->OrderedSet<m.Base> =walk::<m.Base>(x);fn walk<S:m.Base>(x:m.Base)->OrderedSet<S> =incoming::<S>(x,\"label\");",
        "fn main(x:m.Base)->OrderedSet<m.Base> =incoming::<m.Base>(x,\"label\");",
        "fn main(x:m.Base)->OrderedSet<m.Base> =incoming::<m.Base>(x,\"computed\");",
        "fn main(x:m.Base)->OrderedSet<m.Base> ={let field=\"children\";incoming::<m.Base>(x,field)};",
        "fn main(x:m.Other)->OrderedSet<m.Base> =incoming::<m.Base>(x,\"children\");",
        "fn main(x:m.Base)->Int=if false {size(incoming::<m.Base>(x,\"absent\"))} else {0};",
        "fn main(x:m.Base)->OrderedSet<m.Base> =incoming(x,\"children\");",
        "fn main(x:m.Base)->OrderedSet<m.Base> =incoming::<m.Base>(x);",
    ] {
        let (bundle, source, _) = incoming_fixture(body);
        let selection = EntrySelection {
            symbol: SymbolId {
                source: source.id.clone(),
                declaration: 1,
            },
            types: vec![],
            arguments: vec![],
        };
        assert!(
            compile_function(&bundle, &source.id, &selection, &Limits::default()).is_err(),
            "{body}"
        );
    }
    let (bundle, source, _) = incoming_fixture(
        "fn main(x:m.Base)->OrderedSet<m.Other> =incoming::<m.Other>(x,\"children\");",
    );
    let image = compile_navigation(&bundle, &source);
    for mutation in 0..4 {
        let mut program = image.program().clone();
        let instruction = program
            .functions
            .iter_mut()
            .flat_map(|f| &mut f.blocks)
            .flat_map(|b| &mut b.instructions)
            .find(|i| matches!(i.kind, InstructionKind::Incoming { .. }))
            .unwrap();
        let InstructionKind::Incoming {
            source,
            field,
            declaration,
            ..
        } = &mut instruction.kind
        else {
            unreachable!()
        };
        match mutation {
            0 => *field = "label".into(),
            1 => declaration.path.push(99),
            2 => declaration.path = vec![0, 1], // same-named Base.children is not Other.children
            _ => *source = ir_v2::TypeId(u32::MAX),
        }
        assert!(ir_v2::verify(program, &bundle, &Limits::default()).is_err());
    }
}

#[test]
fn incoming_sessions_yield_cancel_and_obey_exact_resource_ceilings() {
    use intersect_model::rcore_behavior_functions::{
        EvaluationSession, ExecutionControl, ExecutionErrorKind, SessionProgress,
    };
    use std::num::NonZeroUsize;
    struct Cancel;
    impl ExecutionControl for Cancel {
        fn cancellation_requested(&self) -> bool {
            true
        }
    }
    let expected_values = vec![json!("z"), json!("a")];
    let (bundle, source, reference) = incoming_fixture(
        "fn main(x:m.Base)->OrderedSet<m.Other> =incoming::<m.Other>(x,\"children\");",
    );
    let image = compile_navigation(&bundle, &source);
    let arguments = [navigation_argument(&image, &reference)];
    let references = [reference];
    let mut context = ExecutionContext::new(&bundle);
    context.references = &references;
    let limits = ExecutionLimits::default();
    let baseline = evaluate(&image, &arguments, &context, &limits).unwrap();
    let DataWire::Collection(values) = &baseline.value.data else {
        panic!("collection")
    };
    let actual: Vec<_> = values
        .iter()
        .map(|v| match &v.data {
            DataWire::Model(v) => v["element"].clone(),
            _ => panic!("scalar"),
        })
        .collect();
    assert_eq!(actual, expected_values);
    let expected = serde_json::to_value(&baseline).unwrap();
    let mut boundaries = 0;
    for quantum in [1, 3, 4096] {
        let mut session = EvaluationSession::new(&image, &arguments, &context, &limits).unwrap();
        let mut pending = 0;
        loop {
            match session.resume(NonZeroUsize::new(quantum).unwrap()) {
                SessionProgress::Pending => {
                    pending += 1;
                    assert!(pending < 10000);
                }
                SessionProgress::Complete(outcome) => {
                    assert_eq!(serde_json::to_value(outcome.unwrap()).unwrap(), expected);
                    break;
                }
                SessionProgress::Finished => panic!("missing result"),
            }
        }
        if quantum == 1 {
            boundaries = pending;
        }
    }
    assert!(boundaries > expected_values.len());
    for boundary in 0..=boundaries {
        let mut session = EvaluationSession::new(&image, &arguments, &context, &limits).unwrap();
        for _ in 0..boundary {
            assert!(matches!(
                session.resume(NonZeroUsize::MIN),
                SessionProgress::Pending
            ));
        }
        let SessionProgress::Complete(outcome) =
            session.resume_with_control(NonZeroUsize::MIN, &Cancel)
        else {
            panic!("missing cancellation")
        };
        assert_eq!(outcome.unwrap_err().kind, ExecutionErrorKind::Cancelled);
        assert!(matches!(
            session.resume(NonZeroUsize::MAX),
            SessionProgress::Finished
        ));
    }
    for dimension in 0..3 {
        let amount = match dimension {
            0 => baseline.consumed.steps,
            1 => baseline.consumed.value_bytes,
            _ => baseline.consumed.value_nodes,
        };
        // max_value_nodes also bounds context metadata admission, which can
        // exceed this small fixture's runtime arena count.
        let bounds = if dimension == 2 {
            vec![0, amount - 1, limits.max_value_nodes]
        } else {
            vec![0, amount - 1, amount, amount + 1]
        };
        for bound in bounds {
            let mut limits = ExecutionLimits::default();
            match dimension {
                0 => limits.scalar.max_steps = bound,
                1 => limits.scalar.max_value_bytes = bound,
                _ => limits.max_value_nodes = bound,
            };
            let outcome = evaluate(&image, &arguments, &context, &limits);
            assert_eq!(
                outcome.is_ok(),
                bound >= amount,
                "dimension {dimension}, bound {bound}: {outcome:?}"
            );
            if let Err(error) = outcome {
                assert_eq!(error.kind, ExecutionErrorKind::ResourceExhausted);
            }
        }
    }
    assert_eq!(
        serde_json::to_value(evaluate(&image, &arguments, &context, &limits).unwrap()).unwrap(),
        expected
    );
}

fn ownership_fixture(body: &str) -> (DefinitionBundle, DefinitionSource, ReferenceContext) {
    ownership_fixture_ecore(
        body,
        include_str!("../../../../fixtures/ecore-navigation/ownership.ecore"),
    )
}
fn ownership_fixture_ecore(
    body: &str,
    ecore_text: &str,
) -> (DefinitionBundle, DefinitionSource, ReferenceContext) {
    let ecore = intersect_model_acceptance::migration_fixture::migrate_source_fixture(
        DefinitionSource::new("memory:ownership.ecore", SourceKind::Ecore, ecore_text),
    )
    .0;
    let model_bundle = DefinitionBundle {
        version: 1,
        sources: vec![ecore.clone()],
        namespaces: vec![],
    };
    let schema = intersect_model::definitions::compiler::compile_structure_bundle_with_conversions(
        &model_bundle,
        &ecore.id,
        "urn:intersect:ownership",
        &ConversionRegistry::default(),
    )
    .unwrap();
    let mut model = Model::default();
    for (id, class) in [
        ("d", "Diamond"),
        ("root", "Root"),
        ("bridge", "Other"),
        ("sibling", "Base"),
        ("tail", "Base"),
        ("local", "Base"),
        ("leaf", "Base"),
    ] {
        model.create(&schema, id, class, "r", 0, 0).unwrap();
    }
    for (id, field, value) in [
        ("root", "zChildren", json!(["bridge", "sibling"])),
        ("root", "aChild", json!("tail")),
        ("root", "local", json!(["local"])),
        ("bridge", "zChildren", json!(["d"])),
        ("d", "zChildren", json!(["leaf"])),
    ] {
        model
            .elements
            .iter_mut()
            .find(|e| e.id == id)
            .unwrap()
            .values
            .insert(field.into(), value);
    }
    model.resources =
        intersect_model::core::resources::records_for_elements(&schema, &model.elements).unwrap();
    model.validate(&schema).unwrap();
    let reference = ReferenceContext {
        bundle: model_bundle,
        entry: ecore.id.clone(),
        namespace: "urn:intersect:ownership".into(),
        conversions: vec![],
        materialized_types: vec![],
        applied_conversions: vec![],
        constructors: vec![],
        model: serde_json::to_value(model).unwrap(),
    };
    let mut source = DefinitionSource::new(
        "memory:ownership-behavior.rcore",
        SourceKind::RcoreBehavior,
        format!(
            "rcore 1;package ownership version \"1\";import model \"urn:intersect:ownership\" as m;{body}"
        ),
    );
    source.dependencies.push(ecore.id.clone());
    (
        DefinitionBundle {
            version: 1,
            sources: vec![ecore, source.clone()],
            namespaces: vec![],
        },
        source,
        reference,
    )
}
fn ownership_argument(
    image: &ir_v2::VerifiedExecutable,
    reference: &ReferenceContext,
    id: &str,
) -> ValueWire {
    let ty = image.program().functions[image.program().entry.0 as usize].parameters[0];
    let TypeNode::Model(target) = &image.program().types[ty.0 as usize] else {
        panic!("model")
    };
    let domain = ReferenceDomain::new_runtime(
        target.clone(),
        reference.clone(),
        &ConversionRegistry::default(),
    )
    .unwrap();
    ValueWire {
        ty,
        data: DataWire::Model(domain.bind(id).unwrap()),
    }
}
fn ownership_ids(value: &ValueWire) -> Vec<String> {
    match &value.data {
        DataWire::None => vec![],
        DataWire::Some(inner) => ownership_ids(inner),
        DataWire::Collection(items) => items.iter().flat_map(ownership_ids).collect(),
        DataWire::Model(payload) => vec![payload["element"].as_str().unwrap().into()],
        _ => panic!("ownership result"),
    }
}
#[test]
fn ownership_navigation_preserves_effective_order_and_traverses_filtered_ancestors() {
    for (operation, selected, id, expected) in [
        ("container", "Base", "d", vec!["bridge"]),
        ("container", "Diamond", "d", vec![]),
        ("container", "Base", "root", vec![]),
        (
            "contents",
            "Base",
            "root",
            vec!["bridge", "sibling", "tail", "local"],
        ),
        ("contents", "Diamond", "root", vec![]),
        ("contents", "Base", "leaf", vec![]),
        ("ancestors", "Base", "d", vec!["bridge", "root"]),
        ("ancestors", "Diamond", "d", vec!["root"]),
        ("ancestors", "Base", "root", vec![]),
    ] {
        let result = if operation == "container" {
            "Option"
        } else {
            "OrderedSet"
        };
        let (bundle, source, reference) = ownership_fixture(&format!(
            "fn main(x:m.Base)->{result}<m.{selected}> ={operation}::<m.{selected}>(x);"
        ));
        let image = compile_navigation(&bundle, &source);
        assert_eq!(
            image.function_effect(image.program().entry).unwrap(),
            ir_v2::Effect::Query
        );
        let arguments = [ownership_argument(&image, &reference, id)];
        let references = [reference];
        let mut context = ExecutionContext::new(&bundle);
        context.references = &references;
        let value = evaluate(&image, &arguments, &context, &ExecutionLimits::default()).unwrap();
        assert_eq!(
            ownership_ids(&value.value),
            expected,
            "{operation} {selected} {id}"
        );
        assert_eq!(value.consumed.model_contexts, 1);
    }
}
#[test]
fn invalid_ownership_cannot_be_hidden_by_result_type_filters() {
    use intersect_model::rcore_behavior_functions::ExecutionErrorKind;
    for mutation in 0..3 {
        let (bundle, source, mut reference) =
            ownership_fixture("fn main(x:m.Base)->Option<m.Diamond> =container::<m.Diamond>(x);");
        let image = compile_navigation(&bundle, &source);
        let valid_reference = reference.clone();
        let mut model: Model = serde_json::from_value(reference.model).unwrap();
        let field = match mutation {
            0 => "aChild",
            _ => "zChildren",
        };
        let value = match mutation {
            0 => json!("d"),
            1 => json!(["root"]),
            _ => json!(["missing"]),
        };
        model
            .elements
            .iter_mut()
            .find(|e| e.id == "d")
            .unwrap()
            .values
            .insert(field.into(), value);
        reference.model = serde_json::to_value(model).unwrap();
        let ty = image.program().functions[image.program().entry.0 as usize].parameters[0];
        let arguments = [ValueWire {
            ty,
            data: DataWire::Model(
                json!({"kind":"model_reference","context":reference.digest().unwrap(),"element":"d"}),
            ),
        }];
        let references = [reference];
        let before = references.clone();
        let mut context = ExecutionContext::new(&bundle);
        context.references = &references;
        let error =
            evaluate(&image, &arguments, &context, &ExecutionLimits::default()).unwrap_err();
        assert_eq!(
            error.kind,
            ExecutionErrorKind::Nominal,
            "mutation {mutation}"
        );
        let expected = [
            "Multiple containment owners",
            "Containment cycle",
            "Missing reference target missing",
        ][mutation];
        assert_eq!(error.message.as_ref(), expected, "mutation {mutation}");
        assert_eq!(
            error.span, None,
            "context admission precedes source execution"
        );
        assert_eq!(error.consumed.provider_invocations, 0);
        assert!(error.consumed_providers.is_empty());
        let repeated =
            evaluate(&image, &arguments, &context, &ExecutionLimits::default()).unwrap_err();
        assert_eq!(
            serde_json::to_value(error).unwrap(),
            serde_json::to_value(repeated).unwrap()
        );
        assert_eq!(references, before);

        let arguments = [ownership_argument(&image, &valid_reference, "d")];
        let references = [valid_reference];
        let before = references.clone();
        context.references = &references;
        let recovered =
            evaluate(&image, &arguments, &context, &ExecutionLimits::default()).unwrap();
        assert!(ownership_ids(&recovered.value).is_empty());
        assert_eq!(references, before);
    }
}

#[test]
fn ownership_sessions_yield_cancel_and_obey_exact_resource_ceilings() {
    use intersect_model::rcore_behavior_functions::{
        EvaluationSession, ExecutionControl, ExecutionErrorKind, SessionProgress,
    };
    use std::num::NonZeroUsize;
    struct Cancel;
    impl ExecutionControl for Cancel {
        fn cancellation_requested(&self) -> bool {
            true
        }
    }
    for (operation, id, expected_values) in [
        (
            "contents",
            "root",
            vec![
                json!("bridge"),
                json!("sibling"),
                json!("tail"),
                json!("local"),
            ],
        ),
        ("ancestors", "d", vec![json!("bridge"), json!("root")]),
    ] {
        let (bundle, source, reference) = ownership_fixture(&format!(
            "fn main(x:m.Base)->OrderedSet<m.Base> ={operation}::<m.Base>(x);"
        ));
        let image = compile_navigation(&bundle, &source);
        let arguments = [ownership_argument(&image, &reference, id)];
        let references = [reference];
        let mut context = ExecutionContext::new(&bundle);
        context.references = &references;
        let limits = ExecutionLimits::default();
        let baseline = evaluate(&image, &arguments, &context, &limits).unwrap();
        let DataWire::Collection(values) = &baseline.value.data else {
            panic!("collection")
        };
        let actual: Vec<_> = values
            .iter()
            .map(|v| match &v.data {
                DataWire::Model(v) => v["element"].clone(),
                _ => panic!("scalar"),
            })
            .collect();
        assert_eq!(actual, expected_values);
        let expected = serde_json::to_value(&baseline).unwrap();
        let mut boundaries = 0;
        for quantum in [1, 3, 4096] {
            let mut session =
                EvaluationSession::new(&image, &arguments, &context, &limits).unwrap();
            let mut pending = 0;
            loop {
                match session.resume(NonZeroUsize::new(quantum).unwrap()) {
                    SessionProgress::Pending => {
                        pending += 1;
                        assert!(pending < 10000);
                    }
                    SessionProgress::Complete(outcome) => {
                        assert_eq!(serde_json::to_value(outcome.unwrap()).unwrap(), expected);
                        break;
                    }
                    SessionProgress::Finished => panic!("missing result"),
                }
            }
            if quantum == 1 {
                boundaries = pending;
            }
        }
        assert!(boundaries > expected_values.len());
        for boundary in 0..=boundaries {
            let mut session =
                EvaluationSession::new(&image, &arguments, &context, &limits).unwrap();
            for _ in 0..boundary {
                assert!(matches!(
                    session.resume(NonZeroUsize::MIN),
                    SessionProgress::Pending
                ));
            }
            let SessionProgress::Complete(outcome) =
                session.resume_with_control(NonZeroUsize::MIN, &Cancel)
            else {
                panic!("missing cancellation")
            };
            assert_eq!(outcome.unwrap_err().kind, ExecutionErrorKind::Cancelled);
            assert!(matches!(
                session.resume(NonZeroUsize::MAX),
                SessionProgress::Finished
            ));
        }
        for dimension in 0..3 {
            let amount = match dimension {
                0 => baseline.consumed.steps,
                1 => baseline.consumed.value_bytes,
                _ => baseline.consumed.value_nodes,
            };
            // max_value_nodes also bounds context metadata admission, which can
            // exceed this small fixture's runtime arena count.
            let bounds = if dimension == 2 {
                vec![0, amount - 1, limits.max_value_nodes]
            } else {
                vec![0, amount - 1, amount, amount + 1]
            };
            for bound in bounds {
                let mut limits = ExecutionLimits::default();
                match dimension {
                    0 => limits.scalar.max_steps = bound,
                    1 => limits.scalar.max_value_bytes = bound,
                    _ => limits.max_value_nodes = bound,
                };
                let outcome = evaluate(&image, &arguments, &context, &limits);
                assert_eq!(
                    outcome.is_ok(),
                    bound >= amount,
                    "dimension {dimension}, bound {bound}: {outcome:?}"
                );
                if let Err(error) = outcome {
                    assert_eq!(error.kind, ExecutionErrorKind::ResourceExhausted);
                }
            }
        }
        assert_eq!(
            serde_json::to_value(evaluate(&image, &arguments, &context, &limits).unwrap()).unwrap(),
            expected
        );
    }
}

#[test]
fn ownership_types_reject_invalid_calls_and_forged_results() {
    for body in [
        "fn main(x:m.Base)->Option<m.Base> =container(x);",
        "fn main(x:m.Base)->Option<String> =container::<String>(x);",
        "fn main(x:m.Base)->OrderedSet<m.Base> =contents::<m.Base>(7);",
        "fn main(x:m.Base)->Int=if false {size(ancestors::<String>(x))} else {0};",
        "fn main(x:m.Base)->OrderedSet<m.Base> =contents::<m.Base>(x,x);",
    ] {
        let (bundle, source, _) = ownership_fixture(body);
        let selection = EntrySelection {
            symbol: SymbolId {
                source: source.id.clone(),
                declaration: 1,
            },
            types: vec![],
            arguments: vec![],
        };
        assert!(
            compile_function(&bundle, &source.id, &selection, &Limits::default()).is_err(),
            "{body}"
        );
    }
    let (bundle, source, _) =
        ownership_fixture("fn main(x:m.Base)->Option<m.Base> =container::<m.Base>(x);");
    let image = compile_navigation(&bundle, &source);
    for mutation in 0..2 {
        let mut program = image.program().clone();
        let instruction = program
            .functions
            .iter_mut()
            .flat_map(|f| &mut f.blocks)
            .flat_map(|b| &mut b.instructions)
            .find(|i| matches!(i.kind, InstructionKind::Ownership { .. }))
            .unwrap();
        let InstructionKind::Ownership {
            operation,
            selected,
            ..
        } = &mut instruction.kind
        else {
            unreachable!()
        };
        if mutation == 0 {
            *operation = ir_v2::OwnershipOp::Contents;
        } else {
            *selected = ir_v2::TypeId(u32::MAX);
        }
        assert!(ir_v2::verify(program, &bundle, &Limits::default()).is_err());
    }
    let (bundle, source, _) =
        ownership_fixture("fn main(x:m.Base)->Int=container(x);fn container(x:m.Base)->Int=7;");
    let image = compile_navigation(&bundle, &source);
    assert_eq!(
        image.function_effect(image.program().entry).unwrap(),
        ir_v2::Effect::Pure
    );
}

#[test]
fn derived_containment_requires_a_provider_before_ownership_filtering() {
    use intersect_model::rcore_behavior_functions::ExecutionErrorKind;
    let text=include_str!("../../../../fixtures/ecore-navigation/ownership.ecore").replace("name=\"Root\" eSuperTypes=\"#//Diamond\">", "name=\"Root\" eSuperTypes=\"#//Diamond\"><eStructuralFeatures xsi:type=\"ecore:EReference\" name=\"derivedChildren\" derived=\"true\" containment=\"true\" upperBound=\"-1\" eType=\"#//Base\"/>");
    for operation in ["container", "contents", "ancestors"] {
        let result = if operation == "container" {
            "Option"
        } else {
            "OrderedSet"
        };
        let call = format!("{operation}::<m.Diamond>(x)");
        let body = format!("fn main(x:m.Base)->{result}<m.Diamond> ={call};");
        let (bundle, source, reference) = ownership_fixture_ecore(&body, &text);
        let image = compile_navigation(&bundle, &source);
        let arguments = [ownership_argument(
            &image,
            &reference,
            if operation == "contents" { "root" } else { "d" },
        )];
        let references = [reference];
        let before = references.clone();
        let mut context = ExecutionContext::new(&bundle);
        context.references = &references;
        let error =
            evaluate(&image, &arguments, &context, &ExecutionLimits::default()).unwrap_err();
        assert_eq!(error.kind, ExecutionErrorKind::InvalidProgram);
        assert_eq!(
            error.message.as_ref(),
            "Derived containment navigation requires an executable provider"
        );
        let span = error.span.as_ref().unwrap();
        assert_eq!(span.source, source.id);
        assert_eq!(&source.text[span.start..span.end], call);
        assert_eq!(span.start, source.text.find(&call).unwrap());
        assert_eq!(error.consumed.provider_invocations, 0);
        assert!(error.consumed_providers.is_empty());
        let repeated =
            evaluate(&image, &arguments, &context, &ExecutionLimits::default()).unwrap_err();
        assert_eq!(
            serde_json::to_value(&error).unwrap(),
            serde_json::to_value(repeated).unwrap()
        );
        assert_eq!(references, before);

        // Recompile the supported stored-containment fixture after the failed
        // invocation; a different source revision needs its own verified image.
        let (bundle, source, reference) = ownership_fixture(&body);
        let image = compile_navigation(&bundle, &source);
        let arguments = [ownership_argument(
            &image,
            &reference,
            if operation == "contents" { "root" } else { "d" },
        )];
        let references = [reference];
        let before = references.clone();
        let mut context = ExecutionContext::new(&bundle);
        context.references = &references;
        let recovered =
            evaluate(&image, &arguments, &context, &ExecutionLimits::default()).unwrap();
        let expected = if operation == "ancestors" {
            vec!["root"]
        } else {
            vec![]
        };
        assert_eq!(ownership_ids(&recovered.value), expected);
        assert_eq!(references, before);
    }
}

fn closure_fixture(body: &str) -> (DefinitionBundle, DefinitionSource, ReferenceContext) {
    let text = include_str!("../../../../fixtures/ecore-navigation/projection.ecore").replace(
        "name=\"children\" upperBound=\"-1\"",
        "name=\"children\" upperBound=\"-1\" unique=\"false\"",
    );
    let (bundle, source, mut reference) = setup_model(&text, body, None);
    let schema = intersect_model::definitions::compiler::compile_structure_bundle_with_conversions(
        &reference.bundle,
        &reference.entry,
        &reference.namespace,
        &ConversionRegistry::default(),
    )
    .unwrap();
    let mut model: Model = serde_json::from_value(reference.model).unwrap();
    for (id, class) in [
        ("b", "Base"),
        ("c", "Diamond"),
        ("e", "Base"),
        ("q", "Base"),
    ] {
        model.create(&schema, id, class, "r", 0, 0).unwrap();
    }
    for (id, children) in [
        ("d", json!(["b", "c", "b"])),
        ("b", json!(["e"])),
        ("c", json!(["e", "d", "c"])),
        ("e", json!(["b"])),
    ] {
        model
            .elements
            .iter_mut()
            .find(|n| n.id == id)
            .unwrap()
            .values
            .insert("children".into(), children);
    }
    model.validate(&schema).unwrap();
    reference.model = serde_json::to_value(model).unwrap();
    (bundle, source, reference)
}
#[test]
fn graph_closure_is_seed_inclusive_breadth_first_and_cycle_safe() {
    for (body, expected) in [
        (
            "fn main(x:m.Base)->OrderedSet<m.Base> =closure([x],|n|n.children);",
            vec!["d", "b", "c", "e"],
        ),
        (
            "fn main(x:m.Base)->OrderedSet<m.Base> =closure([x,x],|n:m.Base|n.children);",
            vec!["d", "b", "c", "e"],
        ),
        (
            "fn main(x:m.Base)->OrderedSet<m.Base> =closure(x.children,|n|n.children);",
            vec!["b", "c", "e", "d"],
        ),
        (
            "fn main(x:m.Base)->OrderedSet<m.Base> =closure([],|n:m.Base|n.children);",
            vec![],
        ),
        (
            "fn main(x:m.Base)->OrderedSet<m.Base> =closure([x],|n|[]);",
            vec!["d"],
        ),
        (
            "fn main(x:m.Base)->OrderedSet<m.Base> =filter(closure([x],|n|n.children),|n:m.Base|true);",
            vec!["d", "b", "c", "e"],
        ),
        (
            "fn main(x:m.Base)->Sequence<m.Base> =map(closure([x],|n|n.children),|n|n);",
            vec!["d", "b", "c", "e"],
        ),
        (
            "fn main(x:m.Base)->OrderedSet<m.Base> =filter(closure([x],|n|n.children),|n|n!=x);",
            vec!["b", "c", "e"],
        ),
        (
            "fn main(x:m.Base)->Sequence<m.Base> =flatMap(closure([x],|n|n.children),|n|n.children);",
            vec!["b", "c", "b", "e", "e", "d", "c", "b"],
        ),
        (
            "fn main(x:m.Base)->Sequence<m.Base> =fold(closure([x],|n|n.children),Sequence::<m.Base>([]),|a,n|union(a,[n]));",
            vec!["d", "b", "c", "e"],
        ),
        (
            "fn main(x:m.Base)->Sequence<m.Base> =[for n in closure([x],|n|n.children), child in n.children where child!=x yield child];",
            vec!["b", "c", "b", "e", "e", "c", "b"],
        ),
        (
            "fn main(x:m.Base)->OrderedSet<m.Base> =closure([x],|n|closure(n.children,|child|child.children));",
            vec!["d", "b", "c", "e"],
        ),
        (
            "fn main(x:m.Base)->OrderedSet<m.Base> =walk([x],|n:m.Base|Sequence(n.children));fn walk<T:m.Base>(xs:Sequence<T>,f:(T)->Sequence<T>)->OrderedSet<T> =closure(xs,f);",
            vec!["d", "b", "c", "e"],
        ),
        (
            "fn main(x:m.Base)->OrderedSet<m.Base> ={let root=x;closure([x],|n:m.Base|if n==root {n.children} else {n.children})};",
            vec!["d", "b", "c", "e"],
        ),
        (
            "fn main(x:m.Base)->OrderedSet<m.Base> =closure(Set([x,x]),|n|n.children);",
            vec!["d", "b", "c", "e"],
        ),
        (
            "fn main(x:m.Base)->OrderedSet<m.Base> =closure(OrderedSet([x,x]),|n|n.children);",
            vec!["d", "b", "c", "e"],
        ),
        (
            "fn main(x:m.Base)->OrderedSet<m.Base> =closure(Bag([x,x]),|n|Set(n.children));",
            vec!["d", "b", "c", "e"],
        ),
    ] {
        let (bundle, source, reference) = closure_fixture(body);
        let image = compile_navigation(&bundle, &source);
        assert_eq!(
            image.function_effect(image.program().entry).unwrap(),
            ir_v2::Effect::Query
        );
        let arguments = [navigation_argument(&image, &reference)];
        let references = [reference];
        let mut context = ExecutionContext::new(&bundle);
        context.references = &references;
        let value = evaluate(&image, &arguments, &context, &ExecutionLimits::default()).unwrap();
        assert_eq!(ownership_ids(&value.value), expected, "{body}");
    }
}
#[test]
fn graph_composition_preserves_callback_error_spans_and_empty_short_circuiting() {
    use intersect_model::rcore_behavior_functions::ExecutionErrorKind;
    for expression in [
        "map(closure($seeds,|n:m.Base|n.children),|n|{let fail:Int8=1/0;n})",
        "Sequence(filter(closure($seeds,|n:m.Base|n.children),|n|{let fail:Int8=1/0;true}))",
        "flatMap(closure($seeds,|n:m.Base|n.children),|n|{let fail:Int8=1/0;n.children})",
        "fold(closure($seeds,|n:m.Base|n.children),Sequence::<m.Base>([]),|a,n|{let fail:Int8=1/0;union(a,[n])})",
        "[for n in closure($seeds,|n:m.Base|n.children) yield {let fail:Int8=1/0;n}]",
        "Sequence(closure($seeds,|n:m.Base|closure([n],|child|{let fail:Int8=1/0;child.children})))",
    ] {
        for empty in [false, true] {
            let expression = expression.replace("$seeds", if empty { "[]" } else { "[x]" });
            let body = format!("fn main(x:m.Base)->Sequence<m.Base> ={expression};");
            let (bundle, source, reference) = closure_fixture(&body);
            let before = reference.clone();
            let image = compile_navigation(&bundle, &source);
            let arguments = [navigation_argument(&image, &reference)];
            let references = [reference];
            let mut context = ExecutionContext::new(&bundle);
            context.references = &references;
            let result = evaluate(&image, &arguments, &context, &ExecutionLimits::default());
            if empty {
                assert!(ownership_ids(&result.unwrap().value).is_empty(), "{body}");
            } else {
                let error = result.unwrap_err();
                assert_eq!(error.kind, ExecutionErrorKind::Arithmetic, "{body}");
                let span = error.span.unwrap();
                assert_eq!(span.source, source.id);
                assert_eq!(&source.text[span.start..span.end], "/");
                assert_eq!(span.start, source.text.find("1/0").unwrap() + 1);
            }
            assert_eq!(references[0], before, "{body}");
        }
    }
}

#[test]
fn graph_closure_keeps_identical_ids_from_distinct_contexts() {
    let (bundle, source, reference) = closure_fixture(
        "fn main(xs:Sequence<m.Base>)->OrderedSet<m.Base> =closure(xs,|n|n.children);",
    );
    let image = compile_navigation(&bundle, &source);
    let mut other = reference.clone();
    other.model["elements"][0]["values"]["label"] = json!("alternate");
    let ty = image.program().functions[image.program().entry.0 as usize].parameters[0];
    let TypeNode::Collection { element, .. } = &image.program().types[ty.0 as usize] else {
        panic!("collection")
    };
    let TypeNode::Model(target) = &image.program().types[element.0 as usize] else {
        panic!("model")
    };
    let references = [reference, other];
    let domains: Vec<_> = references
        .iter()
        .map(|r| {
            ReferenceDomain::new_runtime(target.clone(), r.clone(), &ConversionRegistry::default())
                .unwrap()
        })
        .collect();
    let arguments = [ValueWire {
        ty,
        data: DataWire::Collection(
            [(0, "d"), (1, "d"), (0, "q")]
                .into_iter()
                .map(|(index, id)| ValueWire {
                    ty: *element,
                    data: DataWire::Model(domains[index].bind(id).unwrap()),
                })
                .collect(),
        ),
    }];
    let mut context = ExecutionContext::new(&bundle);
    context.references = &references;
    let result = evaluate(&image, &arguments, &context, &ExecutionLimits::default()).unwrap();
    let DataWire::Collection(values) = result.value.data else {
        panic!("collection")
    };
    let actual: Vec<_> = values
        .into_iter()
        .map(|v| {
            let DataWire::Model(p) = v.data else {
                panic!("model")
            };
            p
        })
        .collect();
    let expected: Vec<_> = [
        (0, "d"),
        (1, "d"),
        (0, "q"),
        (0, "b"),
        (0, "c"),
        (1, "b"),
        (1, "c"),
        (0, "e"),
        (1, "e"),
    ]
    .into_iter()
    .map(|(context, id)| domains[context].bind(id).unwrap())
    .collect();
    assert_eq!(actual, expected);
    assert_eq!(result.consumed.model_contexts, 2);
}
#[test]
fn graph_closure_rejects_invalid_successors_and_preserves_callback_errors() {
    for body in [
        "fn main(x:m.Base)->OrderedSet<Int> =closure([1],|n:Int|[n]);",
        "fn main(x:m.Base)->OrderedSet<m.Base> =closure([x],|n:m.Base|n);",
        "fn main(x:m.Base)->OrderedSet<m.Base> =closure([x],|n:m.Base|[\"bad\"]);",
        "fn main(x:m.Base)->OrderedSet<m.Base> =closure([x],|n:m.Base,k:Int|[n]);",
        "fn main(x:m.Base)->Bool=if false {isEmpty(closure([1],|n:Int|[n]))} else {true};",
    ] {
        let (bundle, source, _) = closure_fixture(body);
        let selection = EntrySelection {
            symbol: SymbolId {
                source: source.id.clone(),
                declaration: 1,
            },
            types: vec![],
            arguments: vec![],
        };
        assert!(
            compile_function(&bundle, &source.id, &selection, &Limits::default()).is_err(),
            "{body}"
        );
    }
    for empty in [false, true] {
        let seeds = if empty { "[]" } else { "[x]" };
        let (bundle, source, reference) = closure_fixture(&format!(
            "fn main(x:m.Base)->OrderedSet<m.Base> =closure({seeds},|n:m.Base|{{let fail:Int8=1/0;n.children}});"
        ));
        let image = compile_navigation(&bundle, &source);
        let arguments = [navigation_argument(&image, &reference)];
        let references = [reference];
        let mut context = ExecutionContext::new(&bundle);
        context.references = &references;
        let outcome = evaluate(&image, &arguments, &context, &ExecutionLimits::default());
        if empty {
            assert!(ownership_ids(&outcome.unwrap().value).is_empty());
        } else {
            assert_eq!(
                outcome.unwrap_err().kind,
                intersect_model::rcore_behavior_functions::ExecutionErrorKind::Arithmetic
            );
        }
    }
}

#[test]
fn graph_closure_sessions_yield_cancel_and_obey_exact_resource_ceilings() {
    use intersect_model::rcore_behavior_functions::{
        EvaluationSession, ExecutionControl, ExecutionErrorKind, SessionProgress,
    };
    use std::num::NonZeroUsize;
    struct Cancel;
    impl ExecutionControl for Cancel {
        fn cancellation_requested(&self) -> bool {
            true
        }
    }
    let expected_values = vec![json!("d"), json!("b"), json!("c"), json!("e")];
    let (bundle, source, reference) =
        closure_fixture("fn main(x:m.Base)->OrderedSet<m.Base> =closure([x],|n|n.children);");
    let image = compile_navigation(&bundle, &source);
    let arguments = [navigation_argument(&image, &reference)];
    let references = [reference];
    let mut context = ExecutionContext::new(&bundle);
    context.references = &references;
    let limits = ExecutionLimits::default();
    let baseline = evaluate(&image, &arguments, &context, &limits).unwrap();
    let DataWire::Collection(values) = &baseline.value.data else {
        panic!("collection")
    };
    let actual: Vec<_> = values
        .iter()
        .map(|v| match &v.data {
            DataWire::Model(v) => v["element"].clone(),
            _ => panic!("scalar"),
        })
        .collect();
    assert_eq!(actual, expected_values);
    let expected = serde_json::to_value(&baseline).unwrap();
    let mut boundaries = 0;
    for quantum in [1, 3, 4096] {
        let mut session = EvaluationSession::new(&image, &arguments, &context, &limits).unwrap();
        let mut pending = 0;
        loop {
            match session.resume(NonZeroUsize::new(quantum).unwrap()) {
                SessionProgress::Pending => {
                    pending += 1;
                    assert!(pending < 10000);
                }
                SessionProgress::Complete(outcome) => {
                    assert_eq!(serde_json::to_value(outcome.unwrap()).unwrap(), expected);
                    break;
                }
                SessionProgress::Finished => panic!("missing result"),
            }
        }
        if quantum == 1 {
            boundaries = pending;
        }
    }
    assert!(boundaries > expected_values.len());
    for boundary in 0..=boundaries {
        let mut session = EvaluationSession::new(&image, &arguments, &context, &limits).unwrap();
        for _ in 0..boundary {
            assert!(matches!(
                session.resume(NonZeroUsize::MIN),
                SessionProgress::Pending
            ));
        }
        let SessionProgress::Complete(outcome) =
            session.resume_with_control(NonZeroUsize::MIN, &Cancel)
        else {
            panic!("missing cancellation")
        };
        assert_eq!(outcome.unwrap_err().kind, ExecutionErrorKind::Cancelled);
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
        // max_value_nodes also bounds context metadata admission, which can
        // exceed this small fixture's runtime arena count.
        let bounds = if dimension == 2 {
            vec![0, amount - 1, limits.max_value_nodes]
        } else {
            vec![0, amount - 1, amount, amount + 1]
        };
        for bound in bounds {
            let mut limits = ExecutionLimits::default();
            match dimension {
                0 => limits.scalar.max_steps = bound,
                1 => limits.scalar.max_value_bytes = bound,
                2 => limits.max_value_nodes = bound,
                _ => limits.max_collection_comparisons = bound,
            };
            let outcome = evaluate(&image, &arguments, &context, &limits);
            assert_eq!(
                outcome.is_ok(),
                bound >= amount,
                "dimension {dimension}, bound {bound}: {outcome:?}"
            );
            if let Err(error) = outcome {
                assert_eq!(error.kind, ExecutionErrorKind::ResourceExhausted);
            }
        }
    }
    assert_eq!(
        serde_json::to_value(evaluate(&image, &arguments, &context, &limits).unwrap()).unwrap(),
        expected
    );
}

#[test]
fn graph_closure_ir_is_reverified_and_factories_remain_pure() {
    let (bundle, source, _) = closure_fixture(
        "fn main(xs:Sequence<m.Base>)->OrderedSet<m.Base> =closure(xs,|n|n.children);",
    );
    let image = compile_navigation(&bundle, &source);
    for mutation in 0..3 {
        let mut program = image.program().clone();
        let input = program.functions[program.entry.0 as usize].parameters[0];
        let instruction = program
            .functions
            .iter_mut()
            .flat_map(|f| &mut f.blocks)
            .flat_map(|b| &mut b.instructions)
            .find(|i| {
                matches!(
                    i.kind,
                    InstructionKind::CollectionCall {
                        operation: ir_v2::CollectionOp::Closure,
                        ..
                    }
                )
            })
            .unwrap();
        let InstructionKind::CollectionCall {
            operation,
            value,
            callback,
            ..
        } = &mut instruction.kind
        else {
            unreachable!()
        };
        match mutation {
            0 => instruction.ty = input,
            1 => *operation = ir_v2::CollectionOp::Fold,
            _ => *callback = *value,
        }
        assert!(ir_v2::verify(program, &bundle, &Limits::default()).is_err());
    }
    let (bundle, source, _) =
        closure_fixture("fn main()->(m.Base)->OrderedSet<m.Base> =|x:m.Base|closure([x],|n|[]);");
    let image = compile_navigation(&bundle, &source);
    assert_eq!(
        image.function_effect(image.program().entry).unwrap(),
        ir_v2::Effect::Pure
    );
    assert!(image.effects().contains(&ir_v2::Effect::Query));
}

#[test]
fn graph_closure_long_chain_does_not_consume_recursive_call_frames() {
    let (bundle, source, mut reference) =
        closure_fixture("fn main(x:m.Base)->OrderedSet<m.Base> =closure([x],|n|n.children);");
    let schema = intersect_model::definitions::compiler::compile_structure_bundle_with_conversions(
        &reference.bundle,
        &reference.entry,
        &reference.namespace,
        &ConversionRegistry::default(),
    )
    .unwrap();
    let mut model = Model::default();
    let ids: Vec<_> = (0..130)
        .map(|i| {
            if i == 0 {
                "d".into()
            } else {
                format!("node{i}")
            }
        })
        .collect();
    for id in &ids {
        model.create(&schema, id, "Base", "r", 0, 0).unwrap();
    }
    for i in 0..ids.len() - 1 {
        model.elements[i]
            .values
            .insert("children".into(), json!([ids[i + 1]]));
    }
    reference.model = serde_json::to_value(model).unwrap();
    let image = compile_navigation(&bundle, &source);
    let arguments = [navigation_argument(&image, &reference)];
    let references = [reference];
    let mut context = ExecutionContext::new(&bundle);
    context.references = &references;
    let limits = ExecutionLimits {
        max_frames: 3,
        ..Default::default()
    };
    let result = evaluate(&image, &arguments, &context, &limits).unwrap();
    assert_eq!(ownership_ids(&result.value), ids);
}

#[test]
fn graph_closure_matches_pinned_mercurio_explicit_redefinition_membership() {
    let fixture: serde_json::Value = serde_json::from_str(include_str!(
        "../../../../fixtures/ecore-navigation/mercurio-closure.json"
    ))
    .unwrap();
    let excerpt =
        include_str!("../../../../fixtures/ecore-navigation/mercurio-closure-source.rs.txt");
    let excerpt_digest =
        DefinitionSource::new("memory:source-excerpt", SourceKind::RcoreBehavior, excerpt)
            .id
            .sha256;
    assert_eq!(fixture["provenance"]["excerpt_sha256"], excerpt_digest);
    let ecore = intersect_model_acceptance::migration_fixture::migrate_source_fixture(
        DefinitionSource::new(
            "memory:mercurio-closure.ecore",
            SourceKind::Ecore,
            include_str!("../../../../fixtures/ecore-navigation/mercurio-closure.ecore"),
        ),
    )
    .0;
    let model_bundle = DefinitionBundle {
        version: 1,
        sources: vec![ecore.clone()],
        namespaces: vec![],
    };
    let namespace = "urn:intersect:mercurio-redefinition-projection";
    let schema = intersect_model::definitions::compiler::compile_structure_bundle_with_conversions(
        &model_bundle,
        &ecore.id,
        namespace,
        &ConversionRegistry::default(),
    )
    .unwrap();
    let mut source = DefinitionSource::new(
        "memory:mercurio-closure-behavior.rcore",
        SourceKind::RcoreBehavior,
        format!(
            "rcore 1;package redefinition_projection version \"1\";import model \"{namespace}\" as m;fn main(x:m.Feature)->OrderedSet<m.Feature> =closure([x],|f|f.redefined);"
        ),
    );
    source.dependencies.push(ecore.id.clone());
    let bundle = DefinitionBundle {
        version: 1,
        sources: vec![ecore.clone(), source.clone()],
        namespaces: vec![],
    };
    let image = compile_navigation(&bundle, &source);
    for case in fixture["cases"].as_array().unwrap() {
        let mut model = Model::default();
        for feature in case["features"].as_array().unwrap() {
            model
                .create(
                    &schema,
                    feature["id"].as_str().unwrap(),
                    "Feature",
                    "r",
                    0,
                    0,
                )
                .unwrap();
            model
                .elements
                .last_mut()
                .unwrap()
                .values
                .insert("redefined".into(), feature["redefined"].clone());
        }
        model.validate(&schema).unwrap();
        let reference = ReferenceContext {
            bundle: model_bundle.clone(),
            entry: ecore.id.clone(),
            namespace: namespace.into(),
            conversions: vec![],
            materialized_types: vec![],
            applied_conversions: vec![],
            constructors: vec![],
            model: serde_json::to_value(model).unwrap(),
        };
        let arguments = [ownership_argument(
            &image,
            &reference,
            case["seed"].as_str().unwrap(),
        )];
        let references = [reference];
        let mut context = ExecutionContext::new(&bundle);
        context.references = &references;
        let result = evaluate(&image, &arguments, &context, &ExecutionLimits::default()).unwrap();
        let mut membership = ownership_ids(&result.value);
        membership.sort();
        assert_eq!(
            json!(membership),
            case["expected_membership"],
            "{}",
            case["name"]
        );
    }
}

#[test]
fn graph_closure_preserves_pinned_mercurio_required_endpoint_failures_and_repairs() {
    use intersect_model::rcore_behavior_functions::ExecutionErrorKind;
    let fixture: serde_json::Value = serde_json::from_str(include_str!(
        "../../../../fixtures/ecore-navigation/mercurio-prerequisite.json"
    ))
    .unwrap();
    let parent: serde_json::Value = serde_json::from_str(include_str!(
        "../../../../fixtures/ecore-navigation/mercurio-closure.json"
    ))
    .unwrap();
    let excerpt =
        include_str!("../../../../fixtures/ecore-navigation/mercurio-closure-source.rs.txt");
    let digest = DefinitionSource::new("memory:source-excerpt", SourceKind::RcoreBehavior, excerpt)
        .id
        .sha256;
    assert_eq!(fixture["provenance"]["excerpt_sha256"], digest);
    assert_eq!(
        fixture["provenance"]["revision"],
        parent["provenance"]["revision"]
    );
    assert_eq!(
        fixture["provenance"]["excerpt_sha256"],
        parent["provenance"]["excerpt_sha256"]
    );
    assert!(excerpt.contains(
        "for _ in 0..2 {assert!(materialized_redefinition_closure_query(&query,feature).is_err());}"
    ));
    assert_eq!(fixture["missing_endpoint"], "redefined_feature");
    assert!(fixture["relationship"].get("redefined_feature").is_none());
    let ecore = intersect_model_acceptance::migration_fixture::migrate_source_fixture(
        DefinitionSource::new(
            "memory:mercurio-prerequisite.ecore",
            SourceKind::Ecore,
            include_str!("../../../../fixtures/ecore-navigation/mercurio-prerequisite.ecore"),
        ),
    )
    .0;
    let model_bundle = DefinitionBundle {
        version: 1,
        sources: vec![ecore.clone()],
        namespaces: vec![],
    };
    let namespace = "urn:intersect:mercurio-prerequisite-projection";
    let schema = intersect_model::definitions::compiler::compile_structure_bundle_with_conversions(
        &model_bundle,
        &ecore.id,
        namespace,
        &ConversionRegistry::default(),
    )
    .unwrap();
    let mut source = DefinitionSource::new(
        "memory:mercurio-prerequisite-behavior.rcore",
        SourceKind::RcoreBehavior,
        include_str!("../../../../fixtures/ecore-navigation/mercurio-prerequisite-behavior.rcore"),
    );
    source.dependencies.push(ecore.id.clone());
    let bundle = DefinitionBundle {
        version: 1,
        sources: vec![ecore.clone(), source.clone()],
        namespaces: vec![],
    };
    let image = compile_navigation(&bundle, &source);
    let mut model = Model::default();
    for feature in fixture["features"].as_array().unwrap() {
        let index = model
            .create(
                &schema,
                feature["id"].as_str().unwrap(),
                "Feature",
                "r",
                0,
                0,
            )
            .unwrap();
        model.elements[index].values.insert(
            "owned_relationship".into(),
            feature["owned_relationship"].clone(),
        );
    }
    let relation = model
        .create(&schema, "cache_relation", "Redefinition", "r", 0, 0)
        .unwrap();
    model.elements[relation].values.insert(
        "redefining_feature".into(),
        fixture["relationship"]["redefining_feature"].clone(),
    );
    model.elements[relation].values.remove("redefined_feature");
    model.resources =
        intersect_model::core::resources::records_for_elements(&schema, &model.elements).unwrap();
    let reference = |model: &Model| ReferenceContext {
        bundle: model_bundle.clone(),
        entry: ecore.id.clone(),
        namespace: namespace.into(),
        conversions: vec![],
        materialized_types: vec![],
        applied_conversions: vec![],
        constructors: vec![],
        model: serde_json::to_value(model).unwrap(),
    };
    let before = model.clone();
    let references = [reference(&model)];
    let ty = image.program().functions[image.program().entry.0 as usize].parameters[0];
    // Transport a correctly tagged seed without pre-admitting the invalid model.
    let arguments = [ValueWire {
        ty,
        data: DataWire::Model(json!({"kind":"model_reference",
        "context": references[0].digest().unwrap(), "element": fixture["seed"]})),
    }];
    let mut context = ExecutionContext::new(&bundle);
    context.references = &references;
    for _ in 0..fixture["rejected_attempts"].as_u64().unwrap() {
        let error =
            evaluate(&image, &arguments, &context, &ExecutionLimits::default()).unwrap_err();
        assert_eq!(error.kind, ExecutionErrorKind::Nominal);
        assert_eq!(
            error.message.as_ref(),
            fixture["expected_native_error"].as_str().unwrap()
        );
        assert!(error.consumed_providers.is_empty());
        assert_eq!(model, before);
    }
    for repair in fixture["repairs"].as_array().unwrap() {
        let mut repaired = before.clone();
        repaired.elements[relation].values.insert(
            "redefined_feature".into(),
            repair["redefined_feature"].clone(),
        );
        for reload in [false, true] {
            if reload {
                repaired =
                    serde_json::from_value(serde_json::to_value(&repaired).unwrap()).unwrap();
                repaired.elements.reverse();
            }
            repaired.validate(&schema).unwrap();
            let references = [reference(&repaired)];
            let arguments = [ownership_argument(
                &image,
                &references[0],
                fixture["seed"].as_str().unwrap(),
            )];
            let mut context = ExecutionContext::new(&bundle);
            context.references = &references;
            let result =
                evaluate(&image, &arguments, &context, &ExecutionLimits::default()).unwrap();
            let mut membership = ownership_ids(&result.value);
            membership.sort();
            assert_eq!(
                json!(membership),
                repair["expected_membership"],
                "reload={reload}"
            );
        }
    }
}

#[test]
fn ownership_generic_wrappers_preserve_selected_type_and_receiver_bounds() {
    for (operation, selected, id, expected) in [
        ("container", "Base", "d", vec!["bridge"]),
        (
            "contents",
            "Base",
            "root",
            vec!["bridge", "sibling", "tail", "local"],
        ),
        ("ancestors", "Diamond", "d", vec!["root"]),
    ] {
        let result = if operation == "container" {
            "Option"
        } else {
            "OrderedSet"
        };
        let body = format!(
            "fn main(x:m.Base)->{result}<m.{selected}> =walk::<m.{selected},m.Base>(x);fn walk<S:m.Base,T:m.Base>(x:T)->{result}<S> ={operation}::<S>(x);"
        );
        let (bundle, source, reference) = ownership_fixture(&body);
        let image = compile_navigation(&bundle, &source);
        let arguments = [ownership_argument(&image, &reference, id)];
        let references = [reference];
        let mut context = ExecutionContext::new(&bundle);
        context.references = &references;
        let value = evaluate(&image, &arguments, &context, &ExecutionLimits::default()).unwrap();
        assert_eq!(ownership_ids(&value.value), expected, "{body}");
    }
    for body in [
        "fn main(x:m.Base)->Option<m.Base> =walk(x);fn walk<T>(x:T)->Option<m.Base> =container::<m.Base>(x);",
        "fn main(x:m.Base)->Option<Int> =walk::<Int>(x);fn walk<T>(x:m.Base)->Option<T> =container::<T>(x);",
        "fn main(x:m.Base)->Option<Int> =walk::<Int>(x);fn walk<T:Int>(x:m.Base)->Option<T> =container::<T>(x);",
    ] {
        let (bundle, source, _) = ownership_fixture(body);
        assert!(
            compile_function(
                &bundle,
                &source.id,
                &EntrySelection {
                    symbol: SymbolId {
                        source: source.id.clone(),
                        declaration: 1
                    },
                    types: vec![],
                    arguments: vec![],
                },
                &Limits::default()
            )
            .is_err(),
            "{body}"
        );
    }
}

fn generic_navigation_fixture() -> (DefinitionSource, ReferenceContext) {
    use intersect_model::definitions::{
        compiler::materialize_runtime_bundle,
        generic_types::{RuntimeType, Type},
        resources::DeclarationId,
    };
    let ecore = intersect_model_acceptance::migration_fixture::migrate_source_fixture(
        DefinitionSource::new(
            "memory:generic-navigation.ecore",
            SourceKind::Ecore,
            include_str!("../../../../fixtures/ecore-navigation/generic.ecore"),
        ),
    )
    .0;
    let expression = |ordinal, arguments| Type::Classifier {
        declaration: DeclarationId {
            source: ecore.id.clone(),
            path: vec![ordinal],
        },
        arguments,
        raw: false,
    };
    let base = expression(0, vec![]);
    let leaf = expression(1, vec![]);
    let requested: Vec<_> = [
        base.clone(),
        leaf.clone(),
        expression(2, vec![leaf.clone()]),
        expression(3, vec![leaf]),
        expression(2, vec![base]),
        Type::Classifier {
            declaration: DeclarationId {
                source: ecore.id.clone(),
                path: vec![2],
            },
            arguments: vec![],
            raw: true,
        },
    ]
    .into_iter()
    .map(|t| RuntimeType::new(t).unwrap())
    .collect();
    let model_bundle = DefinitionBundle {
        version: 1,
        sources: vec![ecore.clone()],
        namespaces: vec![],
    };
    let schema = materialize_runtime_bundle(
        &model_bundle,
        &ecore.id,
        "urn:generic-navigation",
        &requested,
        None,
    )
    .unwrap();
    let mut model = Model::default();
    for (id, ordinal) in [
        ("leaf", 1),
        ("child", 3),
        ("box", 2),
        ("foreign", 4),
        ("empty", 2),
        ("raw", 5),
    ] {
        model
            .create(
                &schema,
                id,
                schema.class_key_for_type(&requested[ordinal]).unwrap(),
                "r",
                0,
                0,
            )
            .unwrap();
        if ["child", "box", "foreign", "raw"].contains(&id) {
            let values = &mut model.elements.last_mut().unwrap().values;
            values.insert("value".into(), json!("leaf"));
            values.insert("items".into(), json!(["leaf", "leaf"]));
        }
    }
    model.validate(&schema).unwrap();
    let reference = ReferenceContext {
        bundle: model_bundle,
        entry: ecore.id.clone(),
        namespace: "urn:generic-navigation".into(),
        conversions: vec![],
        materialized_types: requested,
        applied_conversions: vec![],
        constructors: vec![],
        model: serde_json::to_value(model).unwrap(),
    };
    (ecore, reference)
}

#[test]
fn symbolic_inverse_queries_execute_exact_closed_and_inherited_domains() {
    let (ecore, reference) = generic_navigation_fixture();
    for (selected, expected) in [("Box", vec!["child", "box"]), ("Child", vec!["child"])] {
        for field in ["value", "items"] {
            let mut source = DefinitionSource::new(
                "memory:generic-navigation-behavior.rcore",
                SourceKind::RcoreBehavior,
                format!(
                    "rcore 1;package navigation version \"1\";import model \"urn:generic-navigation\" as m;fn main(x:m.Leaf)->OrderedSet<m.{selected}<m.Leaf>> =walk(x);fn walk<T:m.Base>(x:T)->OrderedSet<m.{selected}<T>> =incoming::<m.{selected}<T>>(x,\"{field}\");"
                ),
            );
            source.dependencies.push(ecore.id.clone());
            let bundle = DefinitionBundle {
                version: 1,
                sources: vec![ecore.clone(), source.clone()],
                namespaces: vec![],
            };
            let image = compile_navigation(&bundle, &source);
            let arguments = [ownership_argument(&image, &reference, "leaf")];
            let references = [reference.clone()];
            let mut context = ExecutionContext::new(&bundle);
            context.references = &references;
            let result =
                evaluate(&image, &arguments, &context, &ExecutionLimits::default()).unwrap();
            assert_eq!(ownership_ids(&result.value), expected);
        }
    }
}

#[test]
fn generic_forward_and_raw_inverse_navigation_execute_shared_projections() {
    let (ecore, reference) = generic_navigation_fixture();
    for (body, id, expected) in [
        (
            "fn main(x:m.Child<m.Leaf>)->Option<m.Leaf> =read(x);fn read<T:m.Base>(x:m.Child<T>)->Option<T> =x.value;",
            "child",
            vec!["leaf"],
        ),
        (
            "fn main(x:m.Child<m.Leaf>)->Sequence<m.Leaf> =read(x);fn read<T:m.Base>(x:m.Child<T>)->Sequence<T> =Sequence(x.items);",
            "child",
            vec!["leaf", "leaf"],
        ),
        (
            "fn main(x:m.Box)->Option<m.Base> =x.value;",
            "raw",
            vec!["leaf"],
        ),
        (
            "fn main(x:m.Box)->Sequence<m.Base> =Sequence(x.items);",
            "raw",
            vec!["leaf", "leaf"],
        ),
        (
            "fn main(x:m.Box)->Option<m.Base> =x.value;",
            "child",
            vec!["leaf"],
        ),
        (
            "fn main(x:m.Leaf)->OrderedSet<m.Box> =incoming::<m.Box>(x,\"items\");",
            "leaf",
            vec!["child", "box", "foreign", "raw"],
        ),
        (
            "fn main(x:m.Leaf)->OrderedSet<m.Box> =incoming::<m.Box>(x,\"value\");",
            "leaf",
            vec!["child", "box", "foreign", "raw"],
        ),
    ] {
        let mut source = DefinitionSource::new(
            "memory:generic-navigation-behavior.rcore",
            SourceKind::RcoreBehavior,
            format!(
                "rcore 1;package navigation version \"1\";import model \"urn:generic-navigation\" as m;{body}"
            ),
        );
        source.dependencies.push(ecore.id.clone());
        let bundle = DefinitionBundle {
            version: 1,
            sources: vec![ecore.clone(), source.clone()],
            namespaces: vec![],
        };
        let image = compile_navigation(&bundle, &source);
        let arguments = [ownership_argument(&image, &reference, id)];
        let references = [reference.clone()];
        let mut context = ExecutionContext::new(&bundle);
        context.references = &references;
        let result = evaluate(&image, &arguments, &context, &ExecutionLimits::default()).unwrap();
        assert_eq!(ownership_ids(&result.value), expected, "{body}");
    }
}

#[test]
fn raw_navigation_cannot_claim_a_narrower_result_or_receiver() {
    let (ecore, reference) = generic_navigation_fixture();
    for body in [
        "fn main(x:m.Box)->Option<m.Leaf> =x.value;",
        "fn main(x:m.Box)->Sequence<m.Leaf> =Sequence(x.items);",
    ] {
        let mut source = DefinitionSource::new(
            "memory:generic-navigation-behavior.rcore",
            SourceKind::RcoreBehavior,
            format!(
                "rcore 1;package navigation version \"1\";import model \"urn:generic-navigation\" as m;{body}"
            ),
        );
        source.dependencies.push(ecore.id.clone());
        let bundle = DefinitionBundle {
            version: 1,
            sources: vec![ecore.clone(), source.clone()],
            namespaces: vec![],
        };
        assert!(
            compile_function(
                &bundle,
                &source.id,
                &EntrySelection {
                    symbol: SymbolId {
                        source: source.id.clone(),
                        declaration: 1
                    },
                    types: vec![],
                    arguments: vec![]
                },
                &Limits::default()
            )
            .is_err()
        );
    }
    let narrow = reference.materialized_types[2].clone();
    let domain =
        ReferenceDomain::new_runtime(narrow, reference, &ConversionRegistry::default()).unwrap();
    assert!(domain.bind("raw").is_err());
    assert!(domain.bind("foreign").is_err());
    assert!(domain.bind("child").is_ok());
}
