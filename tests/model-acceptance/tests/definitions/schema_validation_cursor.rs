use intersect_model::definitions::{compiler, schema::InstantiationValidation};

fn schema() -> intersect_model::definitions::schema::Schema {
    intersect_model_acceptance::migration_fixture::compile_migrated_schema_fixture(
        r##"<e:EPackage xmlns:e="http://www.eclipse.org/emf/2002/Ecore" xmlns:x="http://www.w3.org/2001/XMLSchema-instance" name="m" nsURI="urn:validation-cursor">
          <eClassifiers x:type="e:EClass" name="A">
            <eStructuralFeatures x:type="e:EAttribute" name="name" eType="e:EDataType http://www.eclipse.org/emf/2002/Ecore#//EString"/>
            <eStructuralFeatures x:type="e:EReference" name="next" eType="#//B"/>
          </eClassifiers>
          <eClassifiers x:type="e:EClass" name="B">
            <eStructuralFeatures x:type="e:EAttribute" name="value" eType="e:EDataType http://www.eclipse.org/emf/2002/Ecore#//EInt"/>
          </eClassifiers>
        </e:EPackage>"##,
    )
    .unwrap()
}

#[test]
fn schema_validation_advances_by_declaration_and_matches_synchronous_errors() {
    let schema = schema();
    let mut pending = InstantiationValidation::new();
    let mut advances = 0;
    while !pending.advance(&schema).unwrap() {
        advances += 1;
        assert!(advances < 32);
    }
    assert!(advances >= 6);
    schema.validate_instantiations().unwrap();

    let mut invalid = schema;
    invalid
        .classes
        .get_mut("A")
        .unwrap()
        .features
        .get_mut("next")
        .unwrap()
        .opposite = Some("invalid".into());
    let synchronous = invalid.validate_instantiations().unwrap_err();
    let mut pending = InstantiationValidation::new();
    let staged = loop {
        match pending.advance(&invalid) {
            Ok(false) => {}
            Ok(true) => panic!("invalid schema accepted"),
            Err(error) => break error,
        }
    };
    assert_eq!(staged, synchronous);
}

#[test]
fn raw_bound_targets_yield_individually_and_empty_policies_validate_their_owner() {
    fn run(schema: &intersect_model::definitions::schema::Schema) -> (usize, Result<(), String>) {
        let mut cursor = InstantiationValidation::new();
        let mut slices = 0;
        loop {
            slices += 1;
            match cursor.advance(schema) {
                Ok(false) => {}
                Ok(true) => return (slices, Ok(())),
                Err(error) => return (slices, Err(error)),
            }
            assert!(slices < 200);
        }
    }
    let mut schema = schema();
    let baseline = run(&schema).0;
    schema.raw_reference_bounds.insert(
        "A".into(),
        std::collections::BTreeMap::from([("next".into(), vec!["B".into(); 64])]),
    );
    let (slices, result) = run(&schema);
    assert_eq!(result, Ok(()));
    assert!(slices >= baseline + 64);
    schema.validate_instantiations().unwrap();
    schema
        .raw_reference_bounds
        .get_mut("A")
        .unwrap()
        .get_mut("next")
        .unwrap()[63] = "Missing".into();
    let (failed_slices, result) = run(&schema);
    assert!(failed_slices >= 64);
    assert_eq!(
        result.unwrap_err(),
        "Invalid raw reference bound projection"
    );

    schema.raw_reference_bounds.clear();
    schema
        .raw_reference_bounds
        .insert("Missing".into(), Default::default());
    assert!(schema.validate_instantiations().is_err());
    schema.raw_reference_bounds.clear();
    schema
        .runtime_feature_domains
        .insert("Missing".into(), vec![]);
    assert!(schema.validate_instantiations().is_err());
}

#[test]
fn runtime_alternatives_yield_duplicates_targets_and_cleanup_in_order() {
    use intersect_model::definitions::{
        bundle::{DefinitionBundle, DefinitionSource, SourceKind},
        generic_types::{RuntimeType, Type},
        resources::DeclarationId,
    };
    let source = DefinitionSource::new(
        "memory:domain-cursor",
        SourceKind::Ecore,
        include_str!("../../../../fixtures/ecore-generics/raw-feature-domains.ecore"),
    );
    let (source, _) = intersect_model_acceptance::migration_fixture::migrate_source_fixture(source);
    let mut types = (0..4)
        .map(|n| {
            RuntimeType::new(Type::Classifier {
                declaration: DeclarationId {
                    source: source.id.clone(),
                    path: vec![n],
                },
                arguments: vec![],
                raw: false,
            })
            .unwrap()
        })
        .collect::<Vec<_>>();
    types.push(
        RuntimeType::new(Type::Classifier {
            declaration: DeclarationId {
                source: source.id.clone(),
                path: vec![4],
            },
            arguments: vec![types[1].expression().clone(), types[2].expression().clone()],
            raw: false,
        })
        .unwrap(),
    );
    types.push(
        RuntimeType::new(Type::Classifier {
            declaration: DeclarationId {
                source: source.id.clone(),
                path: vec![5],
            },
            arguments: vec![],
            raw: true,
        })
        .unwrap(),
    );
    let mut schema = compiler::materialize_runtime_types(
        &DefinitionBundle {
            version: 1,
            sources: vec![source],
            namespaces: vec![],
        },
        &types,
        None,
    )
    .unwrap();
    let receiver = schema
        .runtime_feature_domains
        .iter()
        .find(|(_, domains)| !domains.is_empty())
        .unwrap()
        .0
        .clone();
    let template = schema
        .classes
        .values()
        .find(|class| class.features.is_empty())
        .unwrap()
        .clone();
    let mut targets = Vec::new();
    for i in 0..64 {
        let name = format!("target{i:02}");
        let mut class = template.clone();
        class.name = name.clone();
        schema.classes.insert(name.clone(), class);
        targets.push(name);
    }
    schema.runtime_feature_domains.get_mut(&receiver).unwrap()[0].alternatives = targets;
    let mut cursor = InstantiationValidation::new();
    let mut slices = 0;
    loop {
        slices += 1;
        if cursor.advance(&schema).unwrap() {
            break;
        }
        assert!(slices < 400);
    }
    // Duplicate scan, target validation and retained-name cleanup each visit 64 alternatives.
    assert!(slices >= 192);
    schema.validate_instantiations().unwrap();

    let domain = &mut schema.runtime_feature_domains.get_mut(&receiver).unwrap()[0];
    domain.alternatives[0] = "Missing".into();
    domain.alternatives[63] = "Missing".into();
    let mut cursor = InstantiationValidation::new();
    loop {
        match cursor.advance(&schema) {
            Ok(false) => {}
            Ok(true) => panic!("duplicate alternatives accepted"),
            Err(error) => {
                assert_eq!(error, "Duplicate runtime domain alternative");
                break;
            }
        }
    }
    assert_eq!(
        cursor.advance(&schema).unwrap_err(),
        "Schema validation preparation has failed"
    );
    schema.runtime_feature_domains.get_mut(&receiver).unwrap()[0].alternatives[63] =
        "target63".into();
    assert_eq!(
        schema.validate_instantiations().unwrap_err(),
        "Unknown runtime domain class"
    );
}

#[test]
fn raw_identity_signature_search_yields_and_rejects_missing_exact_owner() {
    use intersect_model::definitions::{
        bundle::{DefinitionBundle, DefinitionSource, SourceKind},
        generic_types::{RuntimeType, Type},
        resources::DeclarationId,
    };
    fn drain(schema: &intersect_model::definitions::schema::Schema) -> (usize, Result<(), String>) {
        let mut cursor = InstantiationValidation::new();
        for slices in 1..1000 {
            match cursor.advance(schema) {
                Ok(false) => {}
                Ok(true) => {
                    assert!(cursor.advance(schema).unwrap());
                    return (slices, Ok(()));
                }
                Err(error) => {
                    assert_eq!(
                        cursor.advance(schema).unwrap_err(),
                        "Schema validation preparation has failed"
                    );
                    return (slices, Err(error));
                }
            }
        }
        panic!("schema cursor did not terminate")
    }
    let mut text = String::from(
        r#"<e:EPackage xmlns:e="http://www.eclipse.org/emf/2002/Ecore" xmlns:x="http://www.w3.org/2001/XMLSchema-instance" name="m" nsURI="urn:raw-identity-cursor">"#,
    );
    for n in 0..64 {
        text.push_str(&format!(
            r#"<eClassifiers x:type="e:EClass" name="C{n:02}"><eTypeParameters name="T"/></eClassifiers>"#
        ));
    }
    text.push_str("</e:EPackage>");
    let source = DefinitionSource::new("memory:raw-identity-cursor", SourceKind::Ecore, text);
    let (source, _) = intersect_model_acceptance::migration_fixture::migrate_source_fixture(source);
    let raw = RuntimeType::new(Type::Classifier {
        declaration: DeclarationId {
            source: source.id.clone(),
            path: vec![63],
        },
        arguments: vec![],
        raw: true,
    })
    .unwrap();
    let schema = compiler::materialize_runtime_types(
        &DefinitionBundle {
            version: 1,
            sources: vec![source],
            namespaces: vec![],
        },
        &[raw],
        None,
    )
    .unwrap();
    let (last_slices, result) = drain(&schema);
    assert_eq!(result, Ok(()));
    schema.validate_instantiations().unwrap();
    let mut first = schema.clone();
    first
        .generic_signatures
        .as_mut()
        .unwrap()
        .parameters
        .rotate_right(1);
    let (first_slices, result) = drain(&first);
    assert_eq!(result, Ok(()));
    // The exact owner is last in the source inventory. Reordering the unchanged
    // signature records only changes scheduling, not the admitted identity.
    assert_eq!(last_slices - first_slices, 63);

    let mut missing = schema.clone();
    missing
        .generic_signatures
        .as_mut()
        .unwrap()
        .parameters
        .pop();
    assert_eq!(
        drain(&missing).1.unwrap_err(),
        "Raw runtime identity requires a generic classifier"
    );
    assert_eq!(
        missing.validate_instantiations().unwrap_err(),
        "Raw runtime identity requires a generic classifier"
    );
    missing.generic_signatures = None;
    assert_eq!(
        drain(&missing).1.unwrap_err(),
        "Raw runtime identity requires source signature inventory"
    );
}
