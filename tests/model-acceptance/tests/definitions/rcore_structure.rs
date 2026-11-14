use intersect_model::definitions::{
    bundle::{DefinitionBundle, DefinitionSource, SourceKind},
    rcore::lower_structure,
    resources::ResourceSet,
};

fn source(text: &str) -> DefinitionSource {
    DefinitionSource::new("memory:model.rcore", SourceKind::Rcore, text)
}
fn lower(body: &str) -> intersect_model::definitions::rcore::StructuralLowering {
    lower_structure(
        source(&format!(
            "rcore 1; package demo version \"1\"; metamodel \"urn:demo\" prefix d {{ {body} }}"
        )),
        "memory:generated.ecore",
    )
    .unwrap()
}
#[test]
fn both_daily_models_match_existing_ecore_contracts_with_distinct_identities() {
    for (rcore, ecore) in [
        (
            include_str!("../../../../fixtures/rcore/day03/organization.rcore"),
            include_str!("../../../../fixtures/rcore/day01/organization.ecore"),
        ),
        (
            include_str!("../../../../fixtures/rcore/day03/kerml-specialization.rcore"),
            include_str!("../../../../fixtures/rcore/day01/kerml-specialization.ecore"),
        ),
    ] {
        let original = source(rcore);
        let lowered = lower_structure(original.clone(), "memory:generated.ecore").unwrap();
        assert_eq!(lowered.original, original);
        assert_ne!(lowered.original.id, lowered.generated.id);
        let expected =
            intersect_model_acceptance::migration_fixture::original_schema_fixture(ecore);
        let canonical_bundle = DefinitionBundle {
            version: 1,
            sources: vec![lowered.original.clone()],
            namespaces: vec![],
        };
        let actual = intersect_model::definitions::rcore::compile_structure(
            &canonical_bundle,
            &lowered.original.id,
        )
        .unwrap()
        .schema;
        assert_eq!((&actual.name, &actual.uri), (&expected.name, &expected.uri));
        assert_ne!(actual.revision, expected.revision);
        assert_eq!(actual.classes.len(), expected.classes.len());
        for (name, class) in &actual.classes {
            let expected = &expected.classes[name];
            assert_eq!(class.features, expected.features);
            assert_eq!(class.parents, expected.parents);
            assert_eq!(class.abstract_type, expected.abstract_type);
            assert_eq!(class.feature_order, expected.feature_order);
            if !class.feature_declarations.is_empty() {
                assert_ne!(class.feature_declarations, expected.feature_declarations);
            }
        }
        let bundle = DefinitionBundle {
            version: 1,
            sources: vec![lowered.original.clone(), lowered.generated.clone()],
            namespaces: vec![],
        };
        let resources = ResourceSet::new(&bundle).unwrap();
        for classifier in &lowered.classifiers {
            resources.node(&classifier.declaration).unwrap();
            assert!(actual.classes.contains_key(&classifier.name));
        }
        for origin in &lowered.origins {
            resources.node(&origin.declaration).unwrap();
            assert_eq!(origin.original.source, original.id);
            assert_eq!(origin.generated.source, lowered.generated.id);
            assert!(original.text.is_char_boundary(origin.original.start));
            assert!(original.text.is_char_boundary(origin.original.end));
            assert!(!original.text[origin.original.start..origin.original.end].is_empty());
        }
        let replay = lower_structure(original, "memory:generated.ecore").unwrap();
        assert_eq!(lowered, replay);
        let decoded =
            serde_json::from_str::<intersect_model::definitions::rcore::StructuralLowering>(
                &serde_json::to_string(&lowered).unwrap(),
            )
            .unwrap();
        assert_eq!(lowered, decoded);
    }
}
#[test]
fn inheritance_attributes_containment_and_opposites_use_shared_contracts() {
    let lowered = lower(
        "abstract class Base { attr label: String [1,1]; attr active: Bool [0,1]; } class Parent extends Base { ref children: Child [0,*] containment opposite Child.parent; } class Child { ref parent: Parent [0,1] opposite Parent.children; attr count: Int32 [0,1]; }",
    );
    let schema = intersect_model_acceptance::migration_fixture::compile_migrated_schema_fixture(
        &lowered.generated.text,
    )
    .unwrap();
    assert!(schema.classes["Base"].abstract_type);
    assert_eq!(
        schema.classes["Parent"].parents.first().map(String::as_str),
        Some("Base")
    );
    let children = &schema.classes["Parent"].features["children"];
    assert!(children.containment);
    assert_eq!(children.opposite.as_deref(), Some("Child/parent"));
    assert_eq!(children.upper, -1);
    assert!(children.ordered && children.unique);
    assert_eq!(schema.classes["Base"].features["label"].lower, 1);
}
#[test]
fn malformed_unsupported_and_invalid_structure_report_original_spans() {
    for body in [
        "class A { ref x: Missing [0,1]; }",
        "class A extends Missing {}",
        "class A {} class A {}",
        "class A { attr a:String [0,1]; attr a:String [0,1]; }",
        "class A { attr x:JavaObject [0,1]; }",
        "class A { attr x:String [2,1]; }",
        "class A { attr x:String [0,2147483648]; }",
        "class A { attr x:String [0,1] containment; }",
        "class A { ref x:A [0,1] derived derived; }",
        "class A { ref x:A [0,1] nonunique nonunique; }",
        "class A { ref x:A [0,1] opposite A.missing; }",
        "class A extends A {}",
        "class A { attr x:String [0,1]",
        "class A<T, T> {}",
    ] {
        let original = source(&format!(
            "// café😀\nrcore 1; package demo version \"1\"; metamodel \"urn:demo\" prefix d {{ {body} }}"
        ));
        let errors = lower_structure(original.clone(), "memory:generated.ecore").unwrap_err();
        assert!(!errors.is_empty(), "{body}");
        for error in errors {
            assert_eq!(error.span.source, original.id);
            assert!(original.text.is_char_boundary(error.span.start));
            assert!(original.text.is_char_boundary(error.span.end));
        }
    }
}
#[test]
fn admission_budgets_digest_and_generated_identity_are_checked() {
    let text = "rcore 1; package demo version \"1\"; metamodel \"urn:demo\" prefix d {class A {}}";
    let mut invalid = source(text);
    invalid.text.push(' ');
    assert!(lower_structure(invalid, "memory:generated.ecore").is_err());
    assert!(lower_structure(source(text), "").is_err());
    assert!(lower_structure(source(&text.replace("rcore 1", "rcore 2")), "memory:g").is_err());
    assert!(
        lower_structure(
            source(&format!("{}{}", " ".repeat(256_000), text)),
            "memory:g"
        )
        .is_err()
    );
    let a = lower_structure(source(text), "memory:g").unwrap();
    let b = lower_structure(source(&text.replace("class A", "class B")), "memory:g").unwrap();
    assert_ne!(a.generated.id, b.generated.id);
    assert!(a.generated.dependencies.is_empty());
}

#[test]
fn browser_projection_preserves_endpoint_contracts_and_declares_synthetic_members() {
    let lowered = lower_structure(
        source(include_str!(
            "../../../../fixtures/rcore/web/specialization.rcore"
        )),
        "memory:web.ecore",
    )
    .unwrap();
    let canonical_bundle = DefinitionBundle {
        version: 1,
        sources: vec![lowered.original.clone()],
        namespaces: vec![],
    };
    let actual = intersect_model::definitions::rcore::compile_structure(
        &canonical_bundle,
        &lowered.original.id,
    )
    .unwrap()
    .schema;
    let expected = intersect_model_acceptance::migration_fixture::compile_migrated_schema_fixture(
        include_str!("../../../../fixtures/rcore/day01/kerml-specialization.ecore"),
    )
    .unwrap();
    for endpoint in ["general", "specific"] {
        assert_eq!(
            actual.classes["Specialization"].features[endpoint],
            expected.classes["Specialization"].features[endpoint]
        );
    }
    assert_ne!(actual.uri, expected.uri);
    assert!(actual.classes.contains_key("Model"));
    assert!(actual.classes["Type"].features.contains_key("name"));
    assert_eq!(
        lowered
            .classifiers
            .iter()
            .map(|c| c.name.as_str())
            .collect::<Vec<_>>(),
        vec!["Model", "Type", "Specialization"]
    );
}

#[test]
fn scalar_profile_and_multiple_inheritance_are_consumed_by_existing_importer() {
    let lowered = lower(
        "abstract class A {} abstract class B {} class C extends A, B { attr a:String [0,1]; attr b:Bool [0,1]; attr c:Int32 [0,1]; attr d:Int64 [0,1]; attr e:Float32 [0,1]; attr f:Float64 [0,1]; }",
    );
    let schema = intersect_model_acceptance::migration_fixture::compile_migrated_schema_fixture(
        &lowered.generated.text,
    )
    .unwrap();
    assert_eq!(
        schema.classes["C"].parents.first().map(String::as_str),
        Some("A")
    );
    assert_eq!(schema.classes["C"].parents, vec!["A", "B"]);
    assert_eq!(schema.classes["C"].features.len(), 6);
}

#[test]
fn library_rcore_preserves_ecore_defaults_derived_fields_and_relationships() {
    let authored = source(include_str!("../../../../fixtures/library/library.rcore"));
    let lowered = lower_structure(authored.clone(), "memory:library.generated.ecore").unwrap();
    let canonical_bundle = DefinitionBundle {
        version: 1,
        sources: vec![lowered.original.clone()],
        namespaces: vec![],
    };
    let actual = intersect_model::definitions::rcore::compile_structure(
        &canonical_bundle,
        &lowered.original.id,
    )
    .unwrap()
    .schema;
    let expected = intersect_model_acceptance::migration_fixture::compile_migrated_schema_fixture(
        include_str!("../../../../fixtures/library/library.ecore"),
    )
    .unwrap();
    assert_eq!(actual.uri, expected.uri);
    assert_eq!(actual.classes.len(), expected.classes.len());
    for (name, class) in &actual.classes {
        let expected = &expected.classes[name];
        assert_eq!(class.features, expected.features, "{name}");
        assert_eq!(class.parents, expected.parents);
        assert_eq!(class.abstract_type, expected.abstract_type);
        assert_eq!(class.feature_order, expected.feature_order);
    }
    assert_eq!(lowered.original, authored);
    for body in [
        "class A { attr x:Int32 [0,1] default nope; }",
        "class A { attr x:Int32 [0,1] default 1 default 2; }",
        "class A { ref x:A [0,1] default 1; }",
        "class A { attr x:Int32 [0,1] default \"nope\"; }",
    ] {
        let text = source(&format!(
            "rcore 1;package p version \"1\";metamodel \"urn:p\" prefix p {{{body}}}"
        ));
        assert!(lower_structure(text, "memory:bad.ecore").is_err(), "{body}");
    }
}

#[test]
fn combined_behavior_preserves_exact_source_offsets_and_uses_existing_frontend() {
    let text = "rcore 1; package demo version \"1\";\r\nmetamodel \"urn:d\" prefix d { class A { attr label: String [0,1] default \"é😀\"; } }\r\nbehavior {\r\nfn main(x:Int)->Int { x + 1 }\r\n}\r\n";
    let original = source(text);
    let lowered = lower_structure(original.clone(), "memory:generated.ecore").unwrap();
    let behavior = lowered.behavior.as_ref().unwrap();
    assert_eq!(lowered.original, original);
    assert_eq!(behavior.source.text.len(), text.len());
    assert_eq!(
        behavior.source.dependencies,
        vec![lowered.generated.id.clone()]
    );
    let parsed = intersect_model::definitions::rcore_behavior::parse(behavior.source.clone());
    assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
    let start = text.find("fn main").unwrap();
    assert_eq!(&behavior.source.text[start..start + 7], "fn main");
    assert_eq!(
        behavior.source.text.matches("\r\n").count(),
        text.matches("\r\n").count()
    );
    let broken = source(&text.replace("x + 1", "x +"));
    let errors = lower_structure(broken.clone(), "memory:g").unwrap_err();
    assert!(
        errors
            .iter()
            .all(|e| e.span.source == broken.id && e.span.start >= start)
    );
    let bundle = DefinitionBundle {
        version: 1,
        sources: vec![lowered.original, lowered.generated, behavior.source.clone()],
        namespaces: vec![],
    };
    bundle.validate().unwrap();
}

#[test]
fn legacy_behavior_and_malformed_sections_are_rejected() {
    let rules = include_str!("../../../../fixtures/library/library-behavior.rcore");
    let text = format!(
        "{}\nbehavior legacy {}",
        include_str!("../../../../fixtures/library/library.rcore"),
        rules.trim()
    );
    assert!(lower_structure(source(&text), "memory:g").is_err());
    for suffix in [
        "behavior { fn main()->Int { 1 }",
        "behavior {} trailing",
        "behavior {} behavior {}",
    ] {
        assert!(lower_structure(source(&format!("rcore 1; package d version \"1\"; metamodel \"urn:d\" prefix d {{ class A {{}} }} {suffix}")), "memory:g").is_err());
    }
}

#[test]
fn rcore_enums_preserve_literal_origins_defaults_and_typed_operation_signatures() {
    let lowered = lower(
        "enum Condition { GOOD = 0; WORN = -1; DAMAGED = 2147483647; } class Copy { attr condition: Condition [0,1] default \"GOOD\"; op inspect() -> Condition [1,1]; }",
    );
    let canonical = DefinitionBundle {
        version: 1,
        sources: vec![lowered.original.clone()],
        namespaces: vec![],
    };
    let schema =
        intersect_model::definitions::rcore::compile_structure(&canonical, &lowered.original.id)
            .unwrap()
            .schema;
    let enumeration = &schema.enumerations["Condition"];
    assert_eq!(enumeration.literals()[1].value, -1);
    assert_eq!(enumeration.literals()[2].value, i32::MAX);
    assert_eq!(
        enumeration
            .format(&enumeration.default_value().unwrap())
            .unwrap(),
        "GOOD"
    );
    assert_eq!(schema.enumerations.len(), 1);
    let bundle = DefinitionBundle {
        version: 1,
        sources: vec![lowered.original.clone(), lowered.generated.clone()],
        namespaces: vec![],
    };
    let resources = ResourceSet::new(&bundle).unwrap();
    for origin in &lowered.origins {
        resources.node(&origin.declaration).unwrap();
        assert_eq!(origin.original.source, lowered.original.id);
    }
    for literal in enumeration.literals() {
        let origin = lowered
            .origins
            .iter()
            .find(|o| o.original == resources.node(&literal.declaration).unwrap().span)
            .unwrap();
        assert!(
            lowered.original.text[origin.original.start..origin.original.end]
                .starts_with(&literal.name)
        );
    }
    assert!(
        lowered
            .generated
            .text
            .contains("name=\"condition\" eType=\"#//Condition\"")
    );
    for body in [
        "enum E { A = 0; A = 1; }",
        "enum E { A = 2147483648; }",
        "enum E { A = -2147483649; }",
        "enum E { A = 0; } class E {}",
        "enum E { A = 0; } class C { ref e: E [0,1]; }",
        "enum E { A = 0; } class C { attr e: E [0,1] default \"MISSING\"; }",
    ] {
        let original = source(&format!(
            "rcore 1; package demo version \"1\"; metamodel \"urn:demo\" prefix d {{ {body} }}"
        ));
        let errors = lower_structure(original.clone(), "memory:g").unwrap_err();
        assert!(
            errors.iter().all(|e| e.span.source == original.id),
            "{body}"
        );
    }
}

#[test]
fn qualified_enum_defaults_resolve_types_and_literals_without_losing_source() {
    // Forward declarations work; choosing WORN verifies this is an explicit
    // default rather than the enum's implicit first literal.
    let body = "class Copy { attr condition: Condition [0,1] default Condition.WORN; } enum Condition { GOOD = 0; WORN = 1; }";
    let lowered = lower(body);
    assert!(lowered.original.text.contains("default Condition.WORN"));
    let schema = intersect_model_acceptance::migration_fixture::compile_migrated_schema_fixture(
        &lowered.generated.text,
    )
    .unwrap();
    let enumeration = &schema.enumerations["Condition"];
    let value =
        serde_json::from_value(schema.classes["Copy"].features["condition"].default.clone())
            .unwrap();
    assert_eq!(enumeration.format(&value).unwrap(), "WORN");
    for (declaration, expected) in [
        ("Condition.MISSING", "Unknown enum default literal"),
        ("Missing.GOOD", "Unknown enum default type"),
        ("Other.GOOD", "does not match attribute type"),
    ] {
        let original = source(&format!(
            "rcore 1; package demo version \"1\"; metamodel \"urn:demo\" prefix d {{ enum Condition {{ GOOD = 0; }} enum Other {{ GOOD = 0; }} class Copy {{ attr condition: Condition [0,1] default {declaration}; }} }}"
        ));
        let errors = lower_structure(original.clone(), "memory:g").unwrap_err();
        assert!(
            errors.iter().any(|e| e.message.contains(expected)),
            "{errors:?}"
        );
        assert!(errors.iter().all(|e| e.span.source == original.id));
    }
}

#[test]
fn library_starter_enum_uses_the_existing_enum_default_contract() {
    let original = source(include_str!(
        "../../../../fixtures/library/library-project.rcore"
    ));
    let lowered = lower_structure(original.clone(), "memory:library.generated.ecore").unwrap();
    assert_eq!(lowered.original, original);
    let schema = intersect_model_acceptance::migration_fixture::compile_migrated_schema_fixture(
        &lowered.generated.text,
    )
    .unwrap();
    let enumeration = &schema.enumerations["BookCondition"];
    assert_eq!(
        enumeration
            .literals()
            .iter()
            .map(|l| (l.name.as_str(), l.value))
            .collect::<Vec<_>>(),
        vec![("GOOD", 0), ("WORN", 1), ("DAMAGED", 2)]
    );
    let feature = &schema.classes["BookCopy"].features["condition"];
    assert_eq!(feature.datatype.as_ref(), Some(enumeration.declaration()));
    let value = serde_json::from_value(feature.default.clone()).unwrap();
    assert_eq!(enumeration.format(&value).unwrap(), "GOOD");
    assert_eq!(
        schema.classes["BookCopy"].features["edition"].default,
        serde_json::json!(1)
    );
}
