use intersect_model::definitions::{
    bundle::{DefinitionBundle, DefinitionSource, SourceKind},
    schema::{Class, Schema},
};
use std::collections::BTreeMap;

fn package(body: &str) -> String {
    format!(
        "<e:EPackage xmlns:e='http://www.eclipse.org/emf/2002/Ecore' xmlns:xsi='http://www.w3.org/2001/XMLSchema-instance' name='test' nsURI='urn:test'>{body}</e:EPackage>"
    )
}
fn diamond() -> String {
    package("<eClassifiers xsi:type='e:EClass' name='Root' abstract='true' interface='true'><eStructuralFeatures xsi:type='e:EAttribute' name='name' eType='ecore:EDataType http://www.eclipse.org/emf/2002/Ecore#//EString'/></eClassifiers><eClassifiers xsi:type='e:EClass' name='Left' eSuperTypes='#//Root'/><eClassifiers xsi:type='e:EClass' name='Right' eSuperTypes='#//Root' interface='true'><eStructuralFeatures xsi:type='e:EAttribute' name='count' eType='ecore:EDataType http://www.eclipse.org/emf/2002/Ecore#//EInt'/></eClassifiers><eClassifiers xsi:type='e:EClass' name='Leaf' eSuperTypes='#//Left #//Right'/>").replace("ecore:EDataType", "e:EDataType")
}

#[test]
fn diamond_keeps_declared_order_and_shared_feature_identity() {
    let schema =
        intersect_model_acceptance::migration_fixture::compile_migrated_schema_fixture(&diamond())
            .unwrap();
    assert_eq!(
        schema.ancestry("Leaf").unwrap(),
        ["Root", "Left", "Right", "Leaf"]
    );
    assert_eq!(
        schema.classes["Leaf"].super_types().collect::<Vec<_>>(),
        ["Left", "Right"]
    );
    assert_eq!(schema.features("Leaf").unwrap().len(), 2);
    assert_eq!(schema.feature_owner("Leaf", "name").unwrap(), "Root");
    assert_eq!(schema.feature_owner("Leaf", "count").unwrap(), "Right");
    for ancestor in ["Root", "Left", "Right", "Leaf"] {
        assert!(schema.conforms("Leaf", ancestor));
    }
    assert!(!schema.conforms("Left", "Right"));
    assert!(!schema.conforms("Missing", "Missing"));
    assert!(schema.classes["Root"].abstract_type);
    assert!(schema.classes["Root"].interface_type);
    assert!(!schema.classes["Right"].abstract_type);
    assert!(schema.classes["Right"].interface_type);
    assert_eq!(
        serde_json::from_str::<Schema>(&serde_json::to_string(&schema).unwrap()).unwrap(),
        schema
    );
}

#[test]
fn href_children_and_redundant_ancestry_share_the_graph() {
    let source = diamond().replace("eSuperTypes='#//Left #//Right'/>", "><eSuperTypes xsi:type='e:EClass' href='#//Left'/><eSuperTypes href='#//Right'/><eSuperTypes href='#//Root'/></eClassifiers>");
    let schema =
        intersect_model_acceptance::migration_fixture::compile_migrated_schema_fixture(&source)
            .unwrap();
    assert_eq!(
        schema.ancestry("Leaf").unwrap(),
        ["Root", "Left", "Right", "Leaf"]
    );
    assert_eq!(schema.features("Leaf").unwrap().len(), 2);
    let error = intersect_model::definitions::ecore_document::validate_structure(
        &source.replace("href='#//Right'", "href='#//Absent'"),
    )
    .unwrap_err();
    let bad = source.replace("href='#//Right'", "href='#//Absent'");
    assert_eq!(&bad[error.span.start..error.span.end], "href='#//Absent'");
}

#[test]
fn malformed_graphs_and_ambiguous_declarations_reject() {
    for (parents, expected) in [
        ("#//Leaf", "Cyclic inheritance"),
        ("#//Left #//Left", "Duplicate supertype"),
        ("#//Left #//Absent", "Missing named fragment"),
    ] {
        let error = intersect_model_acceptance::migration_fixture::compile_migrated_schema_fixture(
            &diamond().replace("#//Left #//Right", parents),
        )
        .unwrap_err();
        assert!(error.contains(expected), "{error}");
    }
    let cyclic = diamond().replace(
        "name='Root' abstract",
        "name='Root' eSuperTypes='#//Leaf' abstract",
    );
    assert!(
        intersect_model_acceptance::migration_fixture::compile_migrated_schema_fixture(&cyclic)
            .unwrap_err()
            .contains("Cyclic inheritance")
    );
    let conflict = diamond().replace("name='count'", "name='name'");
    assert!(
        intersect_model_acceptance::migration_fixture::compile_migrated_schema_fixture(&conflict)
            .unwrap_err()
            .contains("inherited feature redefinition")
    );
}

#[test]
fn same_name_parents_in_different_resources_retain_both_identities() {
    let a = DefinitionSource::new(
        "file:///a.ecore",
        SourceKind::Ecore,
        package("<eClassifiers xsi:type='e:EClass' name='Base'/>").replace("urn:test", "urn:a"),
    );
    let b = DefinitionSource::new(
        "file:///b.ecore",
        SourceKind::Ecore,
        package("<eClassifiers xsi:type='e:EClass' name='Base'/>").replace("urn:test", "urn:b"),
    );
    let mut entry = DefinitionSource::new(
        "file:///entry.ecore",
        SourceKind::Ecore,
        package(
            "<eClassifiers xsi:type='e:EClass' name='Leaf' eSuperTypes='a.ecore#//Base b.ecore#//Base'/>",
        ),
    );
    entry.dependencies = vec![a.id.clone(), b.id.clone()];
    let bundle = DefinitionBundle {
        version: 1,
        sources: vec![entry.clone(), a.clone(), b.clone()],
        namespaces: vec![],
    };
    let report = intersect_model_acceptance::migration_fixture::migrate_resources_fixture(bundle);
    let a = intersect_model_acceptance::migration_fixture::converted_source_fixture(&report, &a.id);
    let b = intersect_model_acceptance::migration_fixture::converted_source_fixture(&report, &b.id);
    let entry =
        intersect_model_acceptance::migration_fixture::converted_source_fixture(&report, &entry.id);
    let mut bundle = report.converted.unwrap();
    let schema = intersect_model::definitions::compiler::compile_structure_bundle(
        &bundle, &entry.id, "urn:test",
    )
    .unwrap();
    let left = schema.class_key(&a.id, "urn:a", "Base").unwrap();
    let right = schema.class_key(&b.id, "urn:b", "Base").unwrap();
    assert_ne!(schema.identity(left), schema.identity(right));
    assert_eq!(
        schema.classes["Leaf"].super_types().collect::<Vec<_>>(),
        [left, right]
    );
    assert!(schema.conforms("Leaf", left));
    assert!(schema.conforms("Leaf", right));
    assert!(!schema.conforms(left, right));
    bundle.sources.reverse();
    assert_eq!(
        schema,
        intersect_model::definitions::compiler::compile_structure_bundle(
            &bundle, &entry.id, "urn:test"
        )
        .unwrap()
    );
}

#[test]
fn canonical_class_format_and_deep_graph_are_stack_independent() {
    assert!(
        serde_json::from_str::<Class>(
            r#"{"name":"Base","abstract_type":false,"parent":null,"features":{}}"#
        )
        .is_err()
    );
    assert!(
        serde_json::from_str::<Class>(
            r#"{"name":"Base","abstract_type":false,"parents":[],"parent":"Old","features":{}}"#
        )
        .is_err()
    );
    let template: Class =
        serde_json::from_str(r#"{"name":"Base","abstract_type":false,"parents":[],"features":{}}"#)
            .unwrap();
    assert!(!template.interface_type);
    assert!(template.parents.is_empty());
    let mut schema = Schema {
        runtime_feature_domains: BTreeMap::new(),
        reference_top: None,
        raw_reference_bounds: BTreeMap::new(),
        runtime_warnings: vec![],
        declared_operation_selections: None,
        instantiations: BTreeMap::new(),
        generic_signatures: None,
        conversion_contracts: vec![],
        applied_conversion_contracts: vec![],
        constructor_contracts: vec![],
        revision: "test".into(),
        uri: "urn:test".into(),
        name: "test".into(),
        classes: BTreeMap::new(),
        enumerations: BTreeMap::new(),
        datatypes: BTreeMap::new(),
        declarations: BTreeMap::new(),
    };
    for i in 0..10_000 {
        let mut class = template.clone();
        class.name = format!("C{i}");
        class.parents = (i > 0).then(|| format!("C{}", i - 1)).into_iter().collect();
        schema.classes.insert(class.name.clone(), class);
    }
    assert_eq!(schema.ancestry("C9999").unwrap().len(), 10_000);
    assert!(schema.conforms("C9999", "C0"));
    schema.classes.get_mut("C0").unwrap().parents = vec!["C9999".into()];
    assert!(schema.ancestry("C9999").unwrap_err().contains("Cyclic"));
    assert!(!schema.conforms("C9999", "C9999"));
}

#[test]
fn typed_resource_parents_and_nonclass_targets_have_explicit_boundaries() {
    let base = package(
        "<eClassifiers xsi:type='e:EClass' name='Base'/><eClassifiers xsi:type='e:EClass' name='Leaf' eSuperTypes='ecore:EClass #//Base'/>",
    ).replace("<e:EPackage", "<e:EPackage xmlns:ecore='http://www.eclipse.org/emf/2002/Ecore'");
    for (text, expected) in [
        (base.clone(), None),
        (
            base.replace("ecore:EClass #//Base", "ecore:EClass"),
            Some("Unsupported or external Ecore type"),
        ),
        (
            base.replace("ecore:EClass #//Base", "#//"),
            Some("Unsupported or external Ecore type"),
        ),
    ] {
        let source = DefinitionSource::new("file:///typed.ecore", SourceKind::Ecore, text);
        if let Some(message) = expected {
            let error =
                intersect_model::definitions::ecore_document::validate_structure(&source.text)
                    .unwrap_err();
            assert!(error.message.contains(message), "{}", error.message);
            assert_eq!(error.span.source.sha256, source.id.sha256);
            assert!(source.text[error.span.start..error.span.end].starts_with("eSuperTypes="));
        } else {
            assert!(
                intersect_model_acceptance::migration_fixture::compile_migrated_schema_fixture(
                    &source.text
                )
                .unwrap()
                .conforms("Leaf", "Base")
            );
        }
    }
}
