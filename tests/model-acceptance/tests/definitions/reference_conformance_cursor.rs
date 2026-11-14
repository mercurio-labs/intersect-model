use intersect_model::definitions::{
    compiler,
    schema::{ReferenceConformancePreparation, Schema},
};

fn drain(schema: &Schema, receiver: &str, field: &str, actual: &str) -> (usize, bool) {
    let mut pending = ReferenceConformancePreparation::new(receiver, field, actual);
    for steps in 1..100000 {
        if pending.advance(schema).unwrap() {
            assert!(pending.advance(schema).unwrap());
            let accepted = pending.finish().unwrap();
            assert_eq!(
                schema.reference_conforms(receiver, field, actual).unwrap(),
                accepted
            );
            return (steps, accepted);
        }
    }
    panic!("reference qualification did not terminate")
}

#[test]
fn inherited_reference_conformance_yields_ancestry_and_each_raw_bound() {
    let mut body = String::new();
    for n in 0..64 {
        let parents = if n == 0 {
            String::new()
        } else {
            format!(r##" eSuperTypes="#//C{:02}""##, n - 1)
        };
        let field = if n == 0 {
            r##"<eStructuralFeatures x:type="e:EReference" name="next" eType="#//C00"/>"##
        } else {
            ""
        };
        body.push_str(&format!(
            r#"<eClassifiers x:type="e:EClass" name="C{n:02}"{parents}>{field}</eClassifiers>"#
        ));
    }
    body.push_str(r#"<eClassifiers x:type="e:EClass" name="Other"/>"#);
    let mut schema = intersect_model_acceptance::migration_fixture::compile_migrated_schema_fixture(&format!(r#"<e:EPackage xmlns:e="http://www.eclipse.org/emf/2002/Ecore" xmlns:x="http://www.w3.org/2001/XMLSchema-instance" name="m" nsURI="urn:reference-cursor">{body}</e:EPackage>"#)).unwrap();
    assert!(
        ReferenceConformancePreparation::new("C63", "next", "C63")
            .finish()
            .is_err()
    );
    let (base, accepted) = drain(&schema, "C63", "next", "C63");
    assert!(accepted);
    assert!(base >= 64 * 8);
    assert!(!drain(&schema, "C63", "next", "Other").1);
    assert!(!drain(&schema, "C63", "next", "Missing").1);
    // Synthetic repeated policies exercise scheduling and receiver precedence;
    // they do not qualify source generation of repeated raw constraints.
    schema.raw_reference_bounds.insert(
        "C00".into(),
        std::collections::BTreeMap::from([("next".into(), vec!["C32".into(); 64])]),
    );
    let (bounded, accepted) = drain(&schema, "C63", "next", "C63");
    assert!(accepted);
    assert!(bounded >= base + 64 * 64 * 4);
    schema
        .raw_reference_bounds
        .get_mut("C00")
        .unwrap()
        .get_mut("next")
        .unwrap()[63] = "Other".into();
    assert!(!drain(&schema, "C63", "next", "C63").1);
    schema.raw_reference_bounds.insert(
        "C63".into(),
        std::collections::BTreeMap::from([("next".into(), vec![])]),
    );
    assert!(drain(&schema, "C63", "next", "C63").1);
    let mut invalid = schema;
    invalid.classes.get_mut("C00").unwrap().parents = vec!["C63".into()];
    let mut pending = ReferenceConformancePreparation::new("C63", "next", "C63");
    loop {
        match pending.advance(&invalid) {
            Ok(false) => {}
            Ok(true) => panic!("invalid receiver ancestry accepted"),
            Err(error) => {
                assert_eq!(error, "Cyclic inheritance at C63");
                assert_eq!(
                    pending.advance(&invalid).unwrap_err(),
                    "Reference conformance preparation has failed"
                );
                break;
            }
        }
    }
    assert!(pending.finish().is_err());
}

#[test]
fn runtime_policy_alternatives_and_unresolved_candidates_yield_individually() {
    use intersect_model::definitions::{
        bundle::{DefinitionBundle, DefinitionSource, SourceKind},
        generic_types::{RuntimeType, Type},
        resources::DeclarationId,
        schema::UnresolvedRuntimeFeatureCandidate,
    };
    let source = DefinitionSource::new(
        "memory:reference-domain-cursor",
        SourceKind::Ecore,
        include_str!("../../../../fixtures/ecore-generics/raw-feature-domains.ecore"),
    );
    let (source, _) = intersect_model_acceptance::migration_fixture::migrate_source_fixture(source);
    let ty = |n, arguments, raw| {
        RuntimeType::new(Type::Classifier {
            declaration: DeclarationId {
                source: source.id.clone(),
                path: vec![n],
            },
            arguments,
            raw,
        })
        .unwrap()
    };
    let a = ty(1, vec![], false);
    let b = ty(2, vec![], false);
    let other = ty(3, vec![], false);
    let pair = ty(
        4,
        vec![a.expression().clone(), b.expression().clone()],
        false,
    );
    let raw = ty(5, vec![], true);
    let mut schema = compiler::materialize_runtime_types(
        &DefinitionBundle {
            version: 1,
            sources: vec![source.clone()],
            namespaces: vec![],
        },
        &[a, b, other.clone(), pair.clone(), raw.clone()],
        None,
    )
    .unwrap();
    let receiver = schema.class_key_for_type(&raw).unwrap().to_owned();
    let target = schema.class_key_for_type(&pair).unwrap().to_owned();
    let foreign = schema.class_key_for_type(&other).unwrap().to_owned();
    let field = schema.runtime_feature_domains[&receiver][0]
        .source
        .projection
        .source
        .name
        .clone();
    assert!(drain(&schema, &receiver, &field, &target).1);
    assert!(!drain(&schema, &receiver, &field, &foreign).1);
    let base = drain(&schema, &receiver, &field, &target).0;
    // Deliberately repeated metadata is a scheduling control over an immutable
    // schema, not evidence that this mutated schema passes source validation.
    let policy = &mut schema.runtime_feature_domains.get_mut(&receiver).unwrap()[0];
    policy.alternatives = vec![foreign.clone(); 64];
    policy.alternatives.push(target.clone());
    let (wide, accepted) = drain(&schema, &receiver, &field, &target);
    assert!(accepted);
    assert!(wide >= base + 64);
    let policy = &mut schema.runtime_feature_domains.get_mut(&receiver).unwrap()[0];
    policy.alternatives.pop();
    policy.unresolved = vec![
        UnresolvedRuntimeFeatureCandidate {
            candidate: other,
            target: foreign,
            reason: "unrelated".into()
        };
        64
    ];
    policy.unresolved.push(UnresolvedRuntimeFeatureCandidate {
        candidate: pair,
        target: target.clone(),
        reason: "pinned uncertainty".into(),
    });
    let mut pending = ReferenceConformancePreparation::new(&receiver, &field, &target);
    let mut steps = 0;
    loop {
        steps += 1;
        match pending.advance(&schema) {
            Ok(false) => assert!(steps < 1000),
            Ok(true) => panic!("unresolved target accepted"),
            Err(error) => {
                assert_eq!(
                    error,
                    "Unresolved runtime feature membership: pinned uncertainty"
                );
                assert!(steps >= base + 128);
                assert_eq!(
                    pending.advance(&schema).unwrap_err(),
                    "Reference conformance preparation has failed"
                );
                break;
            }
        }
    }
    assert!(pending.finish().is_err());
    assert_eq!(
        schema
            .reference_conforms(&receiver, &field, &target)
            .unwrap_err(),
        "Unresolved runtime feature membership: pinned uncertainty"
    );
}
