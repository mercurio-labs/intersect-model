use intersect_model::definitions::{
    compiler,
    schema::{EffectiveFeaturePreparation, Schema},
};

fn source(body: &str) -> String {
    format!(
        r#"<e:EPackage xmlns:e="http://www.eclipse.org/emf/2002/Ecore" xmlns:x="http://www.w3.org/2001/XMLSchema-instance" name="m" nsURI="urn:member-cursor">{body}</e:EPackage>"#
    )
}
fn drain(schema: &Schema, kind: &str) -> (usize, Vec<(String, String, usize)>) {
    let mut pending = EffectiveFeaturePreparation::new(kind);
    for slices in 1..10_000 {
        if pending.advance(schema).unwrap() {
            assert!(pending.advance(schema).unwrap());
            return (
                slices,
                pending
                    .finish()
                    .unwrap()
                    .into_iter()
                    .map(|member| {
                        let resolved = member.resolve(schema).unwrap();
                        (
                            resolved.owner.into(),
                            resolved.feature.name.clone(),
                            resolved.position,
                        )
                    })
                    .collect(),
            );
        }
    }
    panic!("member preparation did not complete")
}
fn attribute(name: &str) -> String {
    format!(
        r#"<eStructuralFeatures x:type="e:EAttribute" name="{name}" eType="e:EDataType http://www.eclipse.org/emf/2002/Ecore#//EString"/>"#
    )
}

#[test]
fn diamond_preserves_literal_declaration_order_and_identity() {
    let text = source(&format!(
        r##"<eClassifiers x:type="e:EClass" name="A">{}{}</eClassifiers>
        <eClassifiers x:type="e:EClass" name="B" eSuperTypes="#//A">{}</eClassifiers>
        <eClassifiers x:type="e:EClass" name="C" eSuperTypes="#//A">{}</eClassifiers>
        <eClassifiers x:type="e:EClass" name="D" eSuperTypes="#//B #//C">{}</eClassifiers>"##,
        attribute("z"),
        attribute("a"),
        attribute("b"),
        attribute("c"),
        attribute("d")
    ));
    let schema =
        intersect_model_acceptance::migration_fixture::compile_migrated_schema_fixture(&text)
            .unwrap();
    assert_eq!(schema.ancestry("D").unwrap(), ["A", "B", "C", "D"]);
    let (_, members) = drain(&schema, "D");
    assert_eq!(
        members,
        [
            ("A".into(), "z".into(), 0),
            ("A".into(), "a".into(), 1),
            ("B".into(), "b".into(), 2),
            ("C".into(), "c".into(), 3),
            ("D".into(), "d".into(), 4),
        ]
    );
    let inherited = schema.effective_feature("D", "z").unwrap();
    assert_eq!(
        inherited.identity,
        schema.effective_feature("A", "z").unwrap().identity
    );
    assert!(EffectiveFeaturePreparation::new("D").finish().is_err());
    let mut legacy = schema;
    for class in legacy.classes.values_mut() {
        class.feature_order.clear();
        class.feature_declarations.clear();
    }
    assert!(legacy.effective_feature("D", "z").is_err());
}

#[test]
fn deep_inheritance_and_wide_member_inventories_yield_before_publishing() {
    let mut body = String::new();
    for n in 0..64 {
        let parents = if n == 0 {
            String::new()
        } else {
            format!(r##" eSuperTypes="#//C{:02}""##, n - 1)
        };
        body.push_str(&format!(
            r#"<eClassifiers x:type="e:EClass" name="C{n:02}"{parents}>{}</eClassifiers>"#,
            attribute(&format!("f{n:02}"))
        ));
    }
    let schema = intersect_model_acceptance::migration_fixture::compile_migrated_schema_fixture(
        &source(&body),
    )
    .unwrap();
    let (slices, actual) = drain(&schema, "C63");
    assert!(
        slices >= 64 * 6,
        "inheritance/inventory/member work must yield"
    );
    let expected = (0..64)
        .map(|n| (format!("C{n:02}"), format!("f{n:02}"), n))
        .collect::<Vec<_>>();
    assert_eq!(actual, expected);
    let members = (0..64)
        .rev()
        .map(|n| attribute(&format!("f{n:02}")))
        .collect::<String>();
    let wide =
        intersect_model_acceptance::migration_fixture::compile_migrated_schema_fixture(&source(
            &format!(r#"<eClassifiers x:type="e:EClass" name="Wide">{members}</eClassifiers>"#),
        ))
        .unwrap();
    let (slices, actual) = drain(&wide, "Wide");
    assert!(slices >= 64 * 4);
    assert_eq!(
        actual
            .iter()
            .map(|(_, name, _)| name.clone())
            .collect::<Vec<_>>(),
        (0..64)
            .rev()
            .map(|n| format!("f{n:02}"))
            .collect::<Vec<_>>()
    );
}

#[test]
fn inventory_and_graph_error_precedence_match_and_poison_reuse() {
    let original = intersect_model_acceptance::migration_fixture::compile_migrated_schema_fixture(&source(&format!(
        r##"<eClassifiers x:type="e:EClass" name="A">{}{}</eClassifiers><eClassifiers x:type="e:EClass" name="B" eSuperTypes="#//A"/>"##,
        attribute("a"), attribute("b")
    ))).unwrap();
    for case in 0..5 {
        let mut schema = original.clone();
        match case {
            0 => {
                schema.classes.get_mut("B").unwrap().parents =
                    vec!["Missing".into(), "Missing".into()];
            }
            1 => {
                schema.classes.get_mut("A").unwrap().parents = vec!["B".into()];
            }
            2 => {
                let class = schema.classes.get_mut("A").unwrap();
                class.feature_order[1] = "a".into();
                class.features.get_mut("a").unwrap().name = "wrong".into();
            }
            3 => {
                schema
                    .classes
                    .get_mut("A")
                    .unwrap()
                    .features
                    .get_mut("a")
                    .unwrap()
                    .name = "wrong".into();
            }
            _ => {
                schema.classes.get_mut("B").unwrap().parents = vec!["Missing".into()];
            }
        }
        let expected = schema.effective_features("B").unwrap_err();
        if case == 0 {
            assert_eq!(expected, "Duplicate supertype Missing of B");
        }
        if case == 2 {
            assert_eq!(expected, "Invalid feature declaration inventory for A");
        }
        let mut pending = EffectiveFeaturePreparation::new("B");
        let mut failed = false;
        for _ in 0..1000 {
            match pending.advance(&schema) {
                Ok(false) => {}
                Ok(true) => panic!("invalid schema accepted"),
                Err(error) => {
                    assert_eq!(error, expected);
                    assert_eq!(
                        pending.advance(&schema).unwrap_err(),
                        "Effective feature preparation has failed"
                    );
                    failed = true;
                    break;
                }
            }
        }
        assert!(failed);
        assert!(pending.finish().is_err());
    }
}

#[test]
fn receiver_default_policies_yield_and_conflicts_reject_without_a_result() {
    use intersect_model::definitions::{
        bundle::{DefinitionBundle, DefinitionSource, SourceKind},
        generic_types::{RuntimeType, Type},
        schema::FeatureDefaultPreparation,
    };
    let source = DefinitionSource::new(
        "memory:default-cursor",
        SourceKind::Ecore,
        include_str!("../../../../fixtures/ecore-generics/raw-feature-domains.ecore"),
    );
    let (source, migration) =
        intersect_model_acceptance::migration_fixture::migrate_source_fixture(source);
    let raw = RuntimeType::new(Type::Classifier {
        declaration: migration[&vec![8]].clone(),
        arguments: vec![],
        raw: true,
    })
    .unwrap();
    let mut schema = compiler::materialize_runtime_types(
        &DefinitionBundle {
            version: 1,
            sources: vec![source],
            namespaces: vec![],
        },
        &[raw],
        None,
    )
    .unwrap();
    let (receiver, domain) = schema
        .runtime_feature_domains
        .iter()
        .find_map(|(receiver, domains)| {
            domains
                .first()
                .map(|domain| (receiver.clone(), domain.clone()))
        })
        .unwrap();
    let field = schema
        .effective_feature(&receiver, "value")
        .unwrap()
        .feature
        .clone();
    schema
        .runtime_feature_domains
        .insert(receiver.clone(), vec![domain; 64]);
    let mut pending = FeatureDefaultPreparation::new(&receiver, &field);
    for _ in 0..64 {
        assert!(!pending.advance(&schema).unwrap());
    }
    assert!(pending.advance(&schema).unwrap());
    assert_eq!(
        pending.finish().unwrap(),
        schema.features(&receiver).unwrap()["value"]
    );
    schema.runtime_feature_domains.get_mut(&receiver).unwrap()[63].default =
        serde_json::json!("conflict");
    let mut pending = FeatureDefaultPreparation::new(&receiver, &field);
    for _ in 0..63 {
        assert!(!pending.advance(&schema).unwrap());
    }
    assert_eq!(
        pending.advance(&schema).unwrap_err(),
        "Conflicting receiver-specific source defaults"
    );
    assert_eq!(
        pending.advance(&schema).unwrap_err(),
        "Feature default preparation has failed"
    );
    assert!(pending.finish().is_err());
    assert_eq!(
        schema.features(&receiver).unwrap_err(),
        "Conflicting receiver-specific source defaults"
    );
    // Defaults are fully checked before policy identity headers, even when the
    // first header is also invalid. Preserve the synchronous error precedence.
    schema.runtime_feature_domains.get_mut(&receiver).unwrap()[0]
        .source
        .projection
        .source
        .declaration
        .path
        .push(99);
    let mut pending = intersect_model::definitions::schema::FeatureMapPreparation::new(&receiver);
    loop {
        match pending.advance(&schema) {
            Ok(false) => (),
            Ok(true) => panic!("conflicting defaults accepted"),
            Err(error) => {
                assert_eq!(error, "Conflicting receiver-specific source defaults");
                assert_eq!(
                    pending.advance(&schema).unwrap_err(),
                    "Feature map preparation has failed"
                );
                break;
            }
        }
    }
    assert!(pending.finish().is_err());
    let mut pending = intersect_model::definitions::schema::InstantiationValidation::new();
    loop {
        match pending.advance(&schema) {
            Ok(false) => (),
            Ok(true) => panic!("invalid policy header accepted"),
            Err(error) => {
                assert_eq!(error, "Conflicting receiver-specific source defaults");
                break;
            }
        }
    }
}
