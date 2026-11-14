use intersect_model::core::model::Model;
use intersect_model::definitions::{
    bundle::{DefinitionBundle, DefinitionSource, NamespaceBinding, SourceKind},
    compiler::compile_structure_bundle,
};
use serde_json::json;

fn package(uri: &str, body: &str) -> String {
    format!(
        "<e:EPackage xmlns:e='http://www.eclipse.org/emf/2002/Ecore' xmlns:xsi='http://www.w3.org/2001/XMLSchema-instance' name='test' nsURI='{uri}'>{body}</e:EPackage>"
    )
}
fn fixture() -> (DefinitionBundle, DefinitionSource, DefinitionSource) {
    let mut a = DefinitionSource::new(
        "file:///model/a.ecore",
        SourceKind::Ecore,
        package(
            "urn:a",
            "<eClassifiers xsi:type='e:EClass' name='Item'><eStructuralFeatures xsi:type='e:EReference' name='parts' upperBound='-1' containment='true' eType='b.ecore#//nested/Item' eOpposite='b.ecore#//nested/Item/owner'/></eClassifiers>",
        ),
    );
    let mut b = DefinitionSource::new(
        "file:///model/b.ecore",
        SourceKind::Ecore,
        package(
            "urn:b",
            "<eSubpackages name='nested' nsURI='urn:nested'><eClassifiers xsi:type='e:EClass' name='Item'><eStructuralFeatures xsi:type='e:EReference' name='owner' eType='a.ecore#//Item' eOpposite='a.ecore#//Item/parts'/></eClassifiers><eClassifiers xsi:type='e:EClass' name='Special' eSuperTypes='#//nested/Item'/></eSubpackages>",
        ),
    );
    a.dependencies.push(b.id.clone());
    b.dependencies.push(a.id.clone());
    let bundle = DefinitionBundle {
        version: 1,
        sources: vec![a.clone(), b.clone()],
        namespaces: vec![NamespaceBinding {
            namespace: "urn:nested".into(),
            source: b.id.clone(),
        }],
    };
    let report = intersect_model_acceptance::migration_fixture::migrate_resources_fixture(bundle);
    let converted = report.converted.unwrap();
    let target = |id: &intersect_model::definitions::bundle::SourceId| {
        report
            .reports
            .iter()
            .find(|r| r.original.id == *id)
            .unwrap()
            .converted
            .clone()
            .unwrap()
    };
    (converted, target(&a.id), target(&b.id))
}

#[test]
fn secondary_parent_features_opposites_and_interface_flags_execute() {
    let text = package("urn:graph", "<eClassifiers xsi:type='e:EClass' name='Root' abstract='true'><eStructuralFeatures xsi:type='e:EAttribute' name='name' eType='ecore:EDataType http://www.eclipse.org/emf/2002/Ecore#//EString'/></eClassifiers><eClassifiers xsi:type='e:EClass' name='Left' eSuperTypes='#//Root'/><eClassifiers xsi:type='e:EClass' name='Right' interface='true' eSuperTypes='#//Root'><eStructuralFeatures xsi:type='e:EReference' name='owner' eType='#//Owner' eOpposite='#//Owner/items'/></eClassifiers><eClassifiers xsi:type='e:EClass' name='Leaf' eSuperTypes='#//Left #//Right'/><eClassifiers xsi:type='e:EClass' name='Both' abstract='true' interface='true'/><eClassifiers xsi:type='e:EClass' name='Owner'><eStructuralFeatures xsi:type='e:EReference' name='items' eType='#//Right' containment='true' upperBound='-1' eOpposite='#//Right/owner'/></eClassifiers>").replace("ecore:EDataType", "e:EDataType");
    let schema =
        intersect_model_acceptance::migration_fixture::compile_migrated_schema_fixture(&text)
            .unwrap();
    let mut model = Model::default();
    model
        .create(&schema, "owner", "Owner", "graph", 0, 0)
        .unwrap();
    model
        .create(&schema, "leaf", "Leaf", "graph", 0, 0)
        .unwrap();
    // Ecore's interface flag does not imply abstract dynamic construction.
    model
        .create(&schema, "interface", "Right", "graph", 0, 0)
        .unwrap();
    for class in ["Root", "Both"] {
        let before = model.clone();
        assert!(
            model
                .create(&schema, "abstract", class, "graph", 0, 0)
                .unwrap_err()
                .contains("abstract")
        );
        assert_eq!(model, before);
    }
    model
        .set_with_opposite(&schema, "owner", "items", json!(["leaf", "interface"]))
        .unwrap();
    assert_eq!(model.elements[1].values["owner"], "owner");
    assert_eq!(schema.feature_owner("Leaf", "owner").unwrap(), "Right");
    model.validate(&schema).unwrap();
    let loaded: Model = serde_json::from_str(&serde_json::to_string(&model).unwrap()).unwrap();
    loaded.validate(&schema).unwrap();
    assert_eq!(loaded, model);
    let mut forged = Model::default();
    forged
        .create(&schema, "forged", "Left", "graph", 0, 0)
        .unwrap();
    forged.elements[0].class = "Root".into();
    forged.elements[0].type_identity = schema.identity("Root");
    assert_eq!(forged.validate(&schema).unwrap_err(), "Abstract instance");
}

#[test]
fn resource_classes_share_core_inheritance_opposites_and_atomic_validation() {
    let (bundle, a, b) = fixture();
    let schema = compile_structure_bundle(&bundle, &a.id, "urn:a").unwrap();
    let outer = schema.class_key(&a.id, "urn:a", "Item").unwrap();
    let inner = schema.class_key(&b.id, "urn:nested", "Item").unwrap();
    let special = schema.class_key(&b.id, "urn:nested", "Special").unwrap();
    assert_ne!(outer, inner);
    assert_ne!(schema.identity(outer), schema.identity(inner));
    assert!(schema.conforms(special, inner));
    assert!(!schema.conforms(outer, inner));
    let mut model = Model::default();
    model.create(&schema, "outer", outer, "one", 0, 0).unwrap();
    model
        .create(&schema, "inner", special, "two", 0, 0)
        .unwrap();
    model
        .set_with_opposite(&schema, "outer", "parts", json!(["inner"]))
        .unwrap();
    assert_eq!(model.elements[1].values["owner"], "outer");
    model.validate(&schema).unwrap();
    let before = model.clone();
    assert!(
        model
            .edit(
                &schema,
                &[("outer".into(), "parts".into(), json!(["outer"]))]
            )
            .is_err()
    );
    assert_eq!(model, before);
    let mut reordered = bundle.clone();
    reordered.sources.reverse();
    assert_eq!(
        schema,
        compile_structure_bundle(&reordered, &a.id, "urn:a").unwrap()
    );
    assert_eq!(
        serde_json::from_str::<Model>(&serde_json::to_string(&model).unwrap()).unwrap(),
        model
    );
}

#[test]
fn resource_resolution_errors_identify_the_referring_source_attribute() {
    let (mut bundle, a, b) = fixture();
    bundle.sources[0].dependencies.clear();
    let error = compile_structure_bundle(&bundle, &a.id, "urn:a").unwrap_err();
    assert_eq!(error.span.source, a.id);
    assert!(a.text[error.span.start..error.span.end].contains("external"));
    assert!(error.message.contains("dependency"));
    let (mut bundle, a, _) = fixture();
    bundle.sources[1].text = format!("{} ", b.text);
    // A changed text without its new exact identity must be rejected before use.
    assert!(
        compile_structure_bundle(&bundle, &a.id, "urn:a")
            .unwrap_err()
            .message
            .contains("digest")
    );
}

#[test]
fn cross_resource_inheritance_cycles_reject_without_recursive_resource_loading() {
    let mut a = DefinitionSource::new(
        "file:///a.ecore",
        SourceKind::Ecore,
        package(
            "urn:a",
            "<eClassifiers xsi:type='e:EClass' name='A' eSuperTypes='b.ecore#//B'/>",
        ),
    );
    let mut b = DefinitionSource::new(
        "file:///b.ecore",
        SourceKind::Ecore,
        package(
            "urn:b",
            "<eClassifiers xsi:type='e:EClass' name='B' eSuperTypes='a.ecore#//A'/>",
        ),
    );
    a.dependencies.push(b.id.clone());
    b.dependencies.push(a.id.clone());
    let bundle = DefinitionBundle {
        version: 1,
        sources: vec![a.clone(), b],
        namespaces: vec![],
    };
    let report = intersect_model_acceptance::migration_fixture::migrate_resources_fixture(bundle);
    assert!(report.converted.is_none());
    assert!(
        report
            .reports
            .iter()
            .flat_map(|r| &r.issues)
            .any(|issue| issue.message.contains("Cyclic inheritance"))
    );
}

#[test]
fn multiple_xmi_packages_and_package_ids_compile_to_distinct_runtime_classes() {
    let text = format!(
        "<xmi:XMI xmlns:xmi='http://www.omg.org/XMI' xmi:version='2.0'>{}{}</xmi:XMI>",
        package("urn:a", "<eClassifiers xsi:type='e:EClass' name='Item'/>")
            .replace("name='test'", "name='a' xmi:id='package-a'"),
        package("urn:b", "<eClassifiers xsi:type='e:EClass' name='Item'/>")
    );
    let source = DefinitionSource::new("file:///multi.ecore", SourceKind::Ecore, text);
    let (source, _) = intersect_model_acceptance::migration_fixture::migrate_source_fixture(source);
    let bundle = DefinitionBundle {
        version: 1,
        sources: vec![source.clone()],
        namespaces: vec![],
    };
    let schema = compile_structure_bundle(&bundle, &source.id, "urn:a").unwrap();
    let a = schema.class_key(&source.id, "urn:a", "Item").unwrap();
    let b = schema.class_key(&source.id, "urn:b", "Item").unwrap();
    assert_ne!(a, b);
    let mut model = Model::default();
    model.create(&schema, "a", a, "example", 0, 0).unwrap();
    model.create(&schema, "b", b, "example", 0, 0).unwrap();
    model.validate(&schema).unwrap();
    assert_ne!(
        model.elements[0].type_identity,
        model.elements[1].type_identity
    );
}

#[test]
fn href_elements_and_untyped_datatype_uris_use_the_same_model_contract() {
    let text = package(
        "urn:a",
        "<eClassifiers xsi:type='e:EClass' name='Base'><eStructuralFeatures xsi:type='e:EAttribute' name='name'><eType xsi:type='e:EDataType' href='http://www.eclipse.org/emf/2002/Ecore#//EString'/></eStructuralFeatures></eClassifiers><eClassifiers xsi:type='e:EClass' name='Item'><eSuperTypes href='#//Base'/><eStructuralFeatures xsi:type='e:EReference' name='next'><eType xsi:type='e:EClass' href='#//Item'/><eOpposite href='#//Item/previous'/></eStructuralFeatures><eStructuralFeatures xsi:type='e:EReference' name='previous'><eType href='#//Item'/><eOpposite href='#//Item/next'/></eStructuralFeatures><eStructuralFeatures xsi:type='e:EAttribute' name='count' eType='http://www.eclipse.org/emf/2002/Ecore#//EInt'/></eClassifiers>",
    );
    let source = DefinitionSource::new("file:///a.ecore", SourceKind::Ecore, text);
    let original_text = source.text.clone();
    let (source, _) = intersect_model_acceptance::migration_fixture::migrate_source_fixture(source);
    let bundle = DefinitionBundle {
        version: 1,
        sources: vec![source.clone()],
        namespaces: vec![],
    };
    let schema = compile_structure_bundle(&bundle, &source.id, "urn:a").unwrap();
    let mut model = Model::default();
    model
        .create(&schema, "one", "Item", "example", 0, 0)
        .unwrap();
    model
        .create(&schema, "two", "Item", "example", 0, 0)
        .unwrap();
    model
        .set_with_opposite(&schema, "one", "next", json!("two"))
        .unwrap();
    assert_eq!(model.elements[1].values["previous"], "one");
    assert_eq!(model.elements[0].values["count"], 0);
    model.validate(&schema).unwrap();
    let bad = DefinitionSource::new(
        "file:///bad.ecore",
        SourceKind::Ecore,
        original_text.replace("href='#//Item'", "href='#//Missing'"),
    );
    let report =
        intersect_model::migration::migrate_ecore(bad.clone(), "file:///bad.rcore").unwrap();
    assert!(report.converted.is_none());
    let error = report
        .issues
        .iter()
        .find(|issue| issue.blocks_conversion)
        .unwrap();
    assert_eq!(
        &bad.text[error.span.start..error.span.end],
        "href='#//Missing'"
    );
}
