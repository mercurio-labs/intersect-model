use intersect_model::definitions::{
    bundle::{DefinitionBundle, DefinitionSource, SourceKind},
    compiler::compile_structure_bundle,
    resources::DeclarationId,
    schema::{MemberIdentity, Schema},
};

fn package(body: &str) -> String {
    format!(
        "<e:EPackage xmlns:e='http://www.eclipse.org/emf/2002/Ecore' xmlns:xsi='http://www.w3.org/2001/XMLSchema-instance' name='test' nsURI='urn:test'>{body}</e:EPackage>"
    )
}
fn feature(name: &str) -> String {
    format!(
        "<eStructuralFeatures xsi:type='e:EAttribute' name='{name}' eType='e:EDataType http://www.eclipse.org/emf/2002/Ecore#//EString'/>"
    )
}
fn diamond() -> String {
    package(&format!(
        "<eClassifiers xsi:type='e:EClass' name='Root'>{}{}</eClassifiers><eClassifiers xsi:type='e:EClass' name='Left' eSuperTypes='#//Root'>{}</eClassifiers><eClassifiers xsi:type='e:EClass' name='Right' eSuperTypes='#//Root'>{}</eClassifiers><eClassifiers xsi:type='e:EClass' name='Leaf' eSuperTypes='#//Left #//Right #//Root'>{}</eClassifiers>",
        feature("z"),
        feature("a"),
        feature("left"),
        feature("right"),
        feature("leaf")
    ))
}

#[test]
fn exact_feature_identity_order_diamonds_and_receiver_positions() {
    let text = diamond();
    let schema =
        intersect_model_acceptance::migration_fixture::compile_migrated_schema_fixture(&text)
            .unwrap();
    let members = schema.effective_features("Leaf").unwrap();
    assert_eq!(
        schema.feature_by_id("Leaf", &members[0].identity).unwrap(),
        members[0]
    );
    let edited = intersect_model_acceptance::migration_fixture::compile_migrated_schema_fixture(
        &text.replace("name='z'", "name='z' defaultValueLiteral='changed'"),
    )
    .unwrap();
    assert!(
        edited
            .feature_by_id("Leaf", &members[0].identity)
            .unwrap_err()
            .contains("Unknown feature declaration")
    );
    assert_eq!(
        members
            .iter()
            .map(|m| m.feature.name.as_str())
            .collect::<Vec<_>>(),
        ["z", "a", "left", "right", "leaf"]
    );
    assert_eq!(
        members.iter().map(|m| m.position).collect::<Vec<_>>(),
        [0, 1, 2, 3, 4]
    );
    let source = DefinitionSource::new(
        "memory:fixture.rcore",
        SourceKind::Rcore,
        intersect_model_acceptance::migrate_model_fixture(&text).unwrap(),
    );
    assert_eq!(
        members[0].identity,
        MemberIdentity::Source {
            declaration: DeclarationId {
                source: source.id,
                path: vec![0, 0]
            }
        }
    );
    assert_eq!(
        members[0].identity,
        schema.effective_feature("Right", "z").unwrap().identity
    );
    assert_eq!(
        members[3].identity,
        schema.effective_feature("Right", "right").unwrap().identity
    );
    assert_eq!(
        schema.effective_feature("Right", "right").unwrap().position,
        2
    );
    assert_eq!(members[3].owner, "Right");
    assert_eq!(
        schema.features("Leaf").unwrap()["right"],
        *members[3].feature
    );
    let restored: Schema = serde_json::from_str(&serde_json::to_string(&schema).unwrap()).unwrap();
    assert_eq!(members, restored.effective_features("Leaf").unwrap());
}

#[test]
fn bundle_paths_survive_removed_parent_nodes_and_nested_packages() {
    let text = package(&format!(
        "<eClassifiers xsi:type='e:EClass' name='Base'/><eSubpackages name='nested' nsURI='urn:nested'><eClassifiers xsi:type='e:EClass' name='Child'><eSuperTypes href='#//Base'/>{}{}</eClassifiers></eSubpackages>",
        feature("z"),
        feature("a")
    ));
    let source = DefinitionSource::new("file:///members.ecore", SourceKind::Ecore, text);
    let (source, correspondence) =
        intersect_model_acceptance::migration_fixture::migrate_source_fixture(source);
    let bundle = DefinitionBundle {
        version: 1,
        sources: vec![source.clone()],
        namespaces: vec![],
    };
    let schema = compile_structure_bundle(&bundle, &source.id, "urn:nested").unwrap();
    for (index, member) in schema
        .effective_features("Child")
        .unwrap()
        .iter()
        .enumerate()
    {
        assert_eq!(
            member.identity,
            MemberIdentity::Source {
                declaration: correspondence[&vec![1, 0, index + 1]].clone()
            }
        );
    }
}

#[test]
fn conflicts_report_both_declarations_and_parent_order_cannot_hide_them() {
    for parents in ["#//Left #//Right", "#//Right #//Left"] {
        let text = diamond()
            .replace("name='right'", "name='left'")
            .replace("#//Left #//Right #//Root", parents);
        let error =
            intersect_model_acceptance::migration_fixture::compile_migrated_schema_fixture(&text)
                .unwrap_err();
        assert!(error.contains("feature redefinition"), "{error}");
        assert!(error.contains("path: [1, 0]"), "{error}");
        assert!(error.contains("path: [2, 0]"), "{error}");
    }
    let text = diamond().replace("name='leaf'", "name='z'");
    assert!(
        intersect_model_acceptance::migration_fixture::compile_migrated_schema_fixture(&text)
            .unwrap_err()
            .contains("feature redefinition")
    );
    let text = diamond()
        .replace("name='a'", "name='Z'")
        .replace("name='left'", "name='_z'");
    assert_eq!(
        intersect_model_acceptance::migration_fixture::compile_migrated_schema_fixture(&text)
            .unwrap()
            .effective_features("Leaf")
            .unwrap()
            .len(),
        5
    );
}

#[test]
fn removed_legacy_identity_and_incomplete_source_inventories_reject() {
    let schema =
        intersect_model_acceptance::migration_fixture::compile_migrated_schema_fixture(&diamond())
            .unwrap();
    let mut json = serde_json::to_value(&schema).unwrap();
    for class in json["classes"].as_object_mut().unwrap().values_mut() {
        class.as_object_mut().unwrap().remove("feature_order");
        class
            .as_object_mut()
            .unwrap()
            .remove("feature_declarations");
    }
    let incomplete: Schema = serde_json::from_value(json).unwrap();
    assert!(
        incomplete
            .effective_features("Root")
            .unwrap_err()
            .contains("inventory")
    );
    assert!(serde_json::from_value::<MemberIdentity>(serde_json::json!({
        "kind": "legacy", "revision": schema.revision, "namespace": "urn:test", "owner": "Root", "name": "a"
    })).is_err());
    for order in [vec!["a"], vec!["a", "a"], vec!["z", "missing"]] {
        let mut invalid = schema.clone();
        invalid.classes.get_mut("Root").unwrap().feature_order =
            order.into_iter().map(str::to_owned).collect();
        assert!(invalid.features("Leaf").unwrap_err().contains("inventory"));
    }
    let mut invalid = schema.clone();
    invalid
        .classes
        .get_mut("Root")
        .unwrap()
        .feature_declarations
        .remove("a");
    assert!(invalid.features("Leaf").unwrap_err().contains("inventory"));
}

#[test]
fn source_member_inventories_reject_reordered_repeated_and_wrong_owner_ids() {
    let source = DefinitionSource::new("file:///members.ecore", SourceKind::Ecore, diamond());
    let (source, _) = intersect_model_acceptance::migration_fixture::migrate_source_fixture(source);
    let bundle = DefinitionBundle {
        version: 1,
        sources: vec![source.clone()],
        namespaces: vec![],
    };
    let schema = compile_structure_bundle(&bundle, &source.id, "urn:test").unwrap();
    let mut changed = schema.clone();
    changed
        .classes
        .get_mut("Root")
        .unwrap()
        .feature_order
        .reverse();
    assert!(
        changed
            .effective_features("Leaf")
            .unwrap_err()
            .contains("order")
    );
    let mut changed = schema.clone();
    changed
        .classes
        .get_mut("Root")
        .unwrap()
        .feature_order
        .clear();
    assert!(
        changed
            .effective_features("Leaf")
            .unwrap_err()
            .contains("inventory")
    );
    let mut changed = schema.clone();
    let id = changed.classes["Root"].feature_declarations["z"].clone();
    changed
        .classes
        .get_mut("Root")
        .unwrap()
        .feature_declarations
        .insert("a".into(), id);
    assert!(
        changed
            .effective_features("Leaf")
            .unwrap_err()
            .contains("Repeated")
    );
    let mut changed = schema;
    changed
        .classes
        .get_mut("Root")
        .unwrap()
        .feature_declarations
        .get_mut("z")
        .unwrap()
        .path = vec![2, 0];
    assert!(
        changed
            .effective_features("Leaf")
            .unwrap_err()
            .contains("wrong owner")
    );
}
