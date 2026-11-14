use intersect_model::definitions::{
    bundle::{DefinitionBundle, DefinitionSource, SourceKind},
    rcore::compile_structure,
    rcore_behavior,
};
fn direct(
    text: &str,
) -> (
    DefinitionSource,
    intersect_model::definitions::rcore::DirectStructure,
) {
    let source = DefinitionSource::new("memory:direct.rcore", SourceKind::Rcore, text);
    let bundle = DefinitionBundle {
        version: 1,
        sources: vec![source.clone()],
        namespaces: vec![],
    };
    let result = compile_structure(&bundle, &source.id).unwrap();
    (source, result)
}

#[test]
fn quoted_type_names_distinguish_local_classifiers_from_builtins() {
    let (source, compiled) = direct(
        r#"rcore 1; package names version "1";
        metamodel "urn:names" prefix n {
            class String {}
            enum EInt { One = 1; }
            class Item {
                ref target: `String` [0,1];
                attr text: String [0,1];
                attr choice: `EInt` [0,1];
                attr number: EInt [0,1];
            }
        }"#,
    );
    let fields = &compiled.schema.classes["Item"].features;
    assert!(fields["target"].reference);
    assert_eq!(fields["target"].target, "String");
    assert_eq!(fields["text"].target, "EString");
    assert_eq!(fields["choice"].default["kind"], "enum");
    assert_eq!(fields["number"].default, 0);
    assert_eq!(
        compiled.schema.enumerations["EInt"].declaration().source,
        source.id
    );
}
#[test]
fn direct_structure_and_behavior_preserve_authored_identity() {
    let text = include_str!("../../../../fixtures/rcore/api/organization.rcore");
    let (source, compiled) = direct(text);
    assert_eq!(compiled.classifiers[0].declaration.source, source.id);
    assert_eq!(compiled.schema.classes.len(), 1);
    assert_eq!(compiled.operations[0].original.source, source.id);
    let bundle = DefinitionBundle {
        version: 1,
        sources: vec![source.clone()],
        namespaces: vec![],
    };
    rcore_behavior::checking::check(&bundle, &source.id).unwrap();
}
#[test]
fn direct_and_ecore_frontends_share_structural_results() {
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
        let (source, direct) = direct(rcore);
        let expected =
            intersect_model_acceptance::migration_fixture::compile_migrated_schema_fixture(ecore)
                .unwrap();
        assert_eq!(direct.schema.classes.len(), expected.classes.len());
        for (name, actual) in &direct.schema.classes {
            let expected = &expected.classes[name];
            assert_eq!(actual.features, expected.features);
            assert_eq!(actual.parents, expected.parents);
            assert_eq!(actual.abstract_type, expected.abstract_type);
            assert_eq!(actual.feature_order, expected.feature_order);
            assert_eq!(direct.schema.declarations[name].source, source.id);
        }
    }
}
#[test]
fn defaults_enums_opposites_and_inheritance_use_shared_checks() {
    let (source, compiled) = direct(
        r#"rcore 1; package demo version "1"; metamodel "urn:demo" prefix d {
      enum Status { New = 0; Done = 1; }
      abstract class Base { attr name: String [0,1] default "caf\u00e9<&"; }
      class Parent extends Base { ref children: Child [0,*] containment opposite Child.parent; }
      class Child { ref parent: Parent [0,1] opposite Parent.children; attr status: Status [1,1] default Status.Done; }
    }"#,
    );
    let schema = compiled.schema;
    assert!(schema.classes["Base"].abstract_type);
    assert_eq!(
        schema.classes["Parent"].parents.first().map(String::as_str),
        Some("Base")
    );
    assert!(schema.classes["Parent"].features["children"].containment);
    assert_eq!(
        schema.classes["Parent"].features["children"]
            .opposite
            .as_deref(),
        Some("Child/parent")
    );
    assert_eq!(
        schema.classes["Base"].features["name"].default,
        serde_json::json!("caf\u{e9}<&")
    );
    assert_eq!(
        compiled.classifiers.last().unwrap().declaration.source,
        source.id
    );
}
#[test]
fn invalid_definitions_and_behavior_report_authored_spans() {
    for body in [
        "class A { ref x: Missing [0,1]; }",
        "class A extends Missing {}",
        "class A {} class A {}",
        "class A extends A {}",
        "class A<T, T> {}",
        "class A { attr x:JavaObject [0,1]; }",
        "class A { attr x:String [2,1]; }",
        "class A { ref x:A [0,1] opposite A.missing; }",
        "class A { attr x:Int32 [0,1] default 999999999999; }",
        "class A { attr x:String [0,1] containment; }",
    ] {
        let text = format!(
            "// caf\u{e9}\u{1f600}\nrcore 1; package demo version \"1\"; metamodel \"urn:demo\" prefix d {{ {body} }}"
        );
        let source = DefinitionSource::new("memory:invalid.rcore", SourceKind::Rcore, text);
        let bundle = DefinitionBundle {
            version: 1,
            sources: vec![source.clone()],
            namespaces: vec![],
        };
        let errors = compile_structure(&bundle, &source.id).unwrap_err();
        for error in errors {
            assert_eq!(error.span.source, source.id, "{body}");
            assert!(source.text.is_char_boundary(error.span.start));
            assert!(source.text.is_char_boundary(error.span.end));
        }
    }
    let text = include_str!("../../../../fixtures/rcore/api/organization.rcore")
        .replace("n.links", "n.missing");
    let (source, _) = direct(&text);
    let bundle = DefinitionBundle {
        version: 1,
        sources: vec![source.clone()],
        namespaces: vec![],
    };
    let errors = rcore_behavior::checking::check(&bundle, &source.id).unwrap_err();
    assert!(errors.iter().all(|e| e.span.source == source.id));
    assert!(
        errors
            .iter()
            .any(|e| text[e.span.start..e.span.end].contains("missing"))
    );
}
