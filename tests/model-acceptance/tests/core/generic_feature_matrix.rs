use intersect_model::core::model::Model;
use intersect_model::definitions::{
    bundle::{DefinitionBundle, DefinitionSource, SourceKind},
    compiler::materialize_closed_types,
    generic_types::{ClosedType, Type},
    schema::{MemberIdentity, Schema},
};
use serde_json::{Value, json};
// Successful write strategies can issue different resource revision tokens.
// Compare every other field here; rollback and snapshot assertions remain exact.
fn assert_same_model_content(mut actual: Model, expected: &Model) {
    let mut expected = expected.clone();
    for record in actual.resources.values_mut() {
        record.content_revision = 0;
    }
    for record in expected.resources.values_mut() {
        record.content_revision = 0;
    }
    assert_eq!(actual, expected);
}

const FIXTURE: &str =
    include_str!("../../../../fixtures/ecore-generics/effective-feature-matrix.ecore");

fn context(text: &str) -> (DefinitionBundle, Vec<ClosedType>) {
    let source = DefinitionSource::new("memory:effective-feature-matrix", SourceKind::Ecore, text);
    let (source, declarations) =
        intersect_model_acceptance::migration_fixture::migrate_source_fixture(source);
    let types = (0..7)
        .map(|index| {
            ClosedType::new(Type::Classifier {
                declaration: declarations[&vec![index]].clone(),
                arguments: vec![],
                raw: false,
            })
            .unwrap()
        })
        .collect();
    (
        DefinitionBundle {
            version: 1,
            sources: vec![source],
            namespaces: vec![],
        },
        types,
    )
}
fn boxed(types: &[ClosedType], argument: &ClosedType) -> ClosedType {
    let Type::Classifier { declaration, .. } = types[2].expression() else {
        panic!("classifier");
    };
    ClosedType::new(Type::Classifier {
        declaration: declaration.clone(),
        arguments: vec![argument.expression().clone()],
        raw: false,
    })
    .unwrap()
}
fn enum_value(schema: &Schema, class: &str, spelling: &str) -> Value {
    let feature = schema.effective_feature(class, "selected").unwrap();
    serde_json::to_value(
        schema.enumerations[&feature.feature.target]
            .parse(spelling)
            .unwrap(),
    )
    .unwrap()
}

#[test]
fn substituted_enum_diamonds_keep_alias_defaults_owner_identity_and_atomic_edits() {
    let (bundle, types) = context(FIXTURE);
    let schema = materialize_closed_types(&bundle, &[types[5].clone(), types[6].clone()]).unwrap();
    let diamond = schema.class_key_for_type(&types[5]).unwrap();
    let other = schema.class_key_for_type(&types[6]).unwrap();
    let owner_type = boxed(&types, &types[0]);
    let other_owner_type = boxed(&types, &types[1]);
    let owner = schema.class_key_for_type(&owner_type).unwrap();
    let other_owner = schema.class_key_for_type(&other_owner_type).unwrap();
    assert_ne!(owner, other_owner);
    assert_eq!(schema.features(diamond).unwrap().len(), 5);
    let selected = schema.effective_feature(diamond, "selected").unwrap();
    assert_eq!(selected.owner, owner);
    assert_eq!(
        selected.identity,
        MemberIdentity::Source {
            declaration: intersect_model_acceptance::migration_fixture::named_declaration_fixture(
                &bundle.sources[0],
                "Record/selected"
            )
        }
    );
    assert_eq!(
        schema.effective_feature(other, "selected").unwrap().owner,
        other_owner
    );
    assert_eq!(
        schema
            .effective_feature(other, "selected")
            .unwrap()
            .identity,
        selected.identity
    );
    let primary = enum_value(&schema, diamond, "primary");
    let alias = enum_value(&schema, diamond, "alias");
    let wrong_enum = enum_value(&schema, other, "alias");
    assert_ne!(primary, alias); // equal numeric value does not merge literal identities
    let mut model = Model::default();
    model
        .create(&schema, "d", diamond, "memory:model", 0, 0)
        .unwrap();
    model
        .create(&schema, "o", other, "memory:model", 0, 0)
        .unwrap();
    assert_eq!(model.elements[0].values["selected"], alias);
    assert_eq!(model.elements[0].values["first"], primary);
    assert_eq!(model.elements[1].values["selected"], wrong_enum);
    model.validate(&schema).unwrap();
    model
        .edit(
            &schema,
            &[
                ("d".into(), "choices".into(), json!([primary, alias])),
                ("d".into(), "history".into(), json!([alias, primary, alias])),
            ],
        )
        .unwrap();
    assert_eq!(
        model.elements[0].values["history"],
        json!([alias, primary, alias])
    );
    let before = model.clone();
    for (field, invalid) in [
        ("selected", wrong_enum),
        ("first", Value::Null),
        ("selected", json!(7)),
        ("choices", json!([alias, alias])),
        ("choices", json!([primary, alias, primary])),
    ] {
        assert!(
            model
                .edit(
                    &schema,
                    &[
                        ("d".into(), "name".into(), json!("must roll back")),
                        ("d".into(), field.into(), invalid)
                    ]
                )
                .is_err()
        );
        assert_eq!(model, before);
    }
    let restored: Schema = serde_json::from_str(&serde_json::to_string(&schema).unwrap()).unwrap();
    let restored_model: Model =
        serde_json::from_str(&serde_json::to_string(&model).unwrap()).unwrap();
    restored_model.validate(&restored).unwrap();
    assert_eq!(model, restored_model);
    assert_eq!(
        schema,
        materialize_closed_types(&bundle, &[types[6].clone(), types[5].clone()]).unwrap()
    );
    let (changed_bundle, changed_types) =
        context(&FIXTURE.replace("urn:generic-features", "urn:changed-generic-features"));
    let changed = materialize_closed_types(&changed_bundle, &[changed_types[5].clone()]).unwrap();
    let stale = enum_value(
        &changed,
        changed.class_key_for_type(&changed_types[5]).unwrap(),
        "alias",
    );
    assert!(
        model
            .edit(&schema, &[("d".into(), "selected".into(), stale)])
            .is_err()
    );
    assert_eq!(model, before);
}

#[test]
fn inherited_generic_multiplicity_and_derived_flags_reach_model_validation() {
    let text = FIXTURE
        .replace(
            "name=\"choices\" upperBound=\"2\"",
            "name=\"choices\" lowerBound=\"1\" upperBound=\"2\"",
        )
        .replace(
            "name=\"history\" upperBound=\"-1\"",
            "name=\"history\" derived=\"true\" upperBound=\"-1\"",
        );
    let (bundle, types) = context(&text);
    let schema = materialize_closed_types(&bundle, &[types[5].clone()]).unwrap();
    let class = schema.class_key_for_type(&types[5]).unwrap();
    let choices = schema.effective_feature(class, "choices").unwrap().feature;
    assert_eq!(
        (
            choices.lower,
            choices.upper,
            choices.ordered,
            choices.unique
        ),
        (1, 2, false, true)
    );
    assert!(
        schema
            .effective_feature(class, "history")
            .unwrap()
            .feature
            .derived
    );
    let primary = enum_value(&schema, class, "primary");
    let alias = enum_value(&schema, class, "alias");
    let mut model = Model::default();
    model
        .create(&schema, "d", class, "memory:model", 0, 0)
        .unwrap();
    assert!(!model.elements[0].values.contains_key("history"));
    assert!(model.validate(&schema).is_err()); // required collection has not been populated
    model
        .edit(
            &schema,
            &[("d".into(), "choices".into(), json!([primary, alias]))],
        )
        .unwrap();
    model.validate(&schema).unwrap();
    let before = model.clone();
    for (field, value) in [("choices", json!([])), ("history", json!([primary]))] {
        assert!(
            model
                .edit(&schema, &[("d".into(), field.into(), value)])
                .is_err()
        );
        assert_eq!(model, before);
    }
}

#[test]
fn invalid_substituted_enum_defaults_kinds_and_feature_diamonds_reject_before_model_use() {
    let incompatible = FIXTURE.replace("name=\"Right\"><eGenericSuperTypes eClassifier=\"#//Record\"><eTypeArguments eClassifier=\"#//Color\"", "name=\"Right\"><eGenericSuperTypes eClassifier=\"#//Record\"><eTypeArguments eClassifier=\"#//OtherColor\"");
    let redeclared = FIXTURE.replace("name=\"Diamond\" eSuperTypes=\"#//Left #//Right\"/>", "name=\"Diamond\" eSuperTypes=\"#//Left #//Right\"><eStructuralFeatures x:type=\"e:EAttribute\" name=\"selected\" eType=\"#//Color\"/></eClassifiers>");
    let wrong_kind = FIXTURE.replace(
        "x:type=\"e:EAttribute\" name=\"selected\" defaultValueLiteral=\"alias\"",
        "x:type=\"e:EReference\" name=\"selected\"",
    );
    for text in [
        FIXTURE.replace(
            "defaultValueLiteral=\"alias\"",
            "defaultValueLiteral=\"Alias\"",
        ),
        FIXTURE.replace(
            "name=\"choices\" upperBound=\"2\"",
            "name=\"choices\" lowerBound=\"3\" upperBound=\"2\"",
        ),
        incompatible,
        redeclared,
        wrong_kind,
    ] {
        assert_ne!(text, FIXTURE);
        let report = intersect_model::migration::migrate_ecore(
            DefinitionSource::new("memory:invalid", SourceKind::Ecore, &text),
            "memory:invalid.rcore",
        )
        .unwrap();
        if report.converted.is_none() {
            assert!(report.issues.iter().any(|issue| issue.blocks_conversion));
            continue;
        }
        let (bundle, types) = context(&text);
        assert!(
            materialize_closed_types(&bundle, &[types[5].clone()]).is_err(),
            "{text}"
        );
    }
    let (bundle, types) = context(FIXTURE);
    let wrong_attribute = boxed(&types, &types[5]);
    assert!(materialize_closed_types(&bundle, &[wrong_attribute, types[5].clone()]).is_err());
}

#[test]
fn inherited_generic_opposites_reparent_through_diamond_and_rollback_argument_mismatch() {
    let original = include_str!("../../../../fixtures/ecore-generics/materialize-opposites.ecore");
    let text = original.replace("</e:EPackage>", r##"<eClassifiers x:type="e:EClass" name="Left" eSuperTypes="#//ValueBox"/><eClassifiers x:type="e:EClass" name="Right" eSuperTypes="#//ValueBox"/><eClassifiers x:type="e:EClass" name="Diamond" eSuperTypes="#//Left #//Right"/></e:EPackage>"##);
    let (bundle, types) = context(&text);
    let good_box = boxed(&types, &types[0]);
    let wrong_box = boxed(&types, &types[1]);
    let schema = materialize_closed_types(
        &bundle,
        &[
            types[0].clone(),
            types[1].clone(),
            types[6].clone(),
            good_box.clone(),
            wrong_box.clone(),
        ],
    )
    .unwrap();
    let diamond = schema.class_key_for_type(&types[6]).unwrap();
    let owner = schema.class_key_for_type(&good_box).unwrap();
    assert_eq!(schema.feature_owner(diamond, "children").unwrap(), owner);
    assert_eq!(
        schema
            .effective_feature(diamond, "children")
            .unwrap()
            .identity,
        MemberIdentity::Source {
            declaration: intersect_model_acceptance::migration_fixture::named_declaration_fixture(
                &bundle.sources[0],
                "Box/children"
            )
        }
    );
    let mut model = Model::default();
    for (id, ty) in [
        ("v", &types[0]),
        ("p", &types[6]),
        ("c", &types[6]),
        ("q", &good_box),
        ("wrong", &wrong_box),
    ] {
        model
            .create(
                &schema,
                id,
                schema.class_key_for_type(ty).unwrap(),
                "memory:model",
                0,
                0,
            )
            .unwrap();
    }
    model
        .edit(&schema, &[("p".into(), "children".into(), json!(["c"]))])
        .unwrap();
    assert_eq!(model.elements[2].values["parent"], json!("p"));
    model
        .edit(&schema, &[("c".into(), "parent".into(), json!("q"))])
        .unwrap();
    assert_eq!(model.elements[1].values["children"], json!([]));
    assert_eq!(model.elements[3].values["children"], json!(["c"]));
    let before = model.clone();
    for edit in [("c", "parent", json!("wrong")), ("q", "parent", json!("c"))] {
        assert!(
            model
                .edit(
                    &schema,
                    &[
                        ("p".into(), "value".into(), json!("v")),
                        (edit.0.into(), edit.1.into(), edit.2)
                    ]
                )
                .is_err()
        );
        assert_eq!(model, before);
    }
    model.validate(&schema).unwrap();
}

#[test]
fn per_edge_formal_composition_reaches_enum_model_defaults_in_either_parent_order() {
    let left = r##"name="Left"><eGenericSuperTypes eClassifier="#//Record"><eTypeArguments eClassifier="#//Color"/></eGenericSuperTypes>"##;
    let right = r##"name="Right"><eGenericSuperTypes eClassifier="#//Record"><eTypeArguments eClassifier="#//Color"/></eGenericSuperTypes>"##;
    let generic_left = r##"name="Left"><eTypeParameters name="T"/><eGenericSuperTypes eClassifier="#//Record"><eTypeArguments eTypeParameter="#//Left/T"/></eGenericSuperTypes>"##;
    let generic_right = r##"name="Right"><eTypeParameters name="T"/><eGenericSuperTypes eClassifier="#//Record"><eTypeArguments eTypeParameter="#//Right/T"/></eGenericSuperTypes>"##;
    let base = FIXTURE
        .replace(left, generic_left)
        .replace(right, generic_right);
    assert_ne!(base, FIXTURE);
    for parents in [["Left", "Right"], ["Right", "Left"]] {
        let projected = parents.iter().map(|parent| format!(r##"<eGenericSuperTypes eClassifier="#//{parent}"><eTypeArguments eClassifier="#//Color"/></eGenericSuperTypes>"##)).collect::<String>();
        let text = base.replace(
            r##"name="Diamond" eSuperTypes="#//Left #//Right"/>"##,
            &format!(r#"name="Diamond">{projected}</eClassifiers>"#),
        );
        let (bundle, types) = context(&text);
        let schema = materialize_closed_types(&bundle, &[types[5].clone()]).unwrap();
        let class = schema.class_key_for_type(&types[5]).unwrap();
        let owner = schema
            .class_key_for_type(&boxed(&types, &types[0]))
            .unwrap();
        assert_eq!(schema.feature_owner(class, "selected").unwrap(), owner);
        let mut model = Model::default();
        model
            .create(&schema, "diamond", class, "memory:model", 0, 0)
            .unwrap();
        assert_eq!(
            model.elements[0].values["selected"],
            enum_value(&schema, class, "alias")
        );
        model.validate(&schema).unwrap();
        let inconsistent = text.replace(
            r##"eClassifier="#//Right"><eTypeArguments eClassifier="#//Color"/>"##,
            r##"eClassifier="#//Right"><eTypeArguments eClassifier="#//OtherColor"/>"##,
        );
        assert_ne!(inconsistent, text);
        let (bundle, types) = context(&inconsistent);
        assert!(materialize_closed_types(&bundle, &[types[5].clone()]).is_err());
    }
}

#[test]
fn generic_diamond_defaults_and_readonly_collections_preserve_assignment_history() {
    use intersect_model::core::{
        api::{BasicModel, ModelEdit},
        storage::{self, CollectionEdit, ModelRead},
    };
    let text = FIXTURE
        .replace(
            "name=\"selected\"",
            "name=\"selected\" unsettable=\"true\" volatile=\"true\"",
        )
        .replace("name=\"choices\"", "name=\"choices\" unsettable=\"true\"")
        .replace(
            "name=\"history\"",
            "name=\"history\" unsettable=\"true\" changeable=\"false\" transient=\"true\"",
        );
    let (bundle, types) = context(&text);
    let schema = materialize_closed_types(&bundle, &[types[5].clone(), types[6].clone()]).unwrap();
    let diamond = schema.class_key_for_type(&types[5]).unwrap().to_owned();
    let other = schema.class_key_for_type(&types[6]).unwrap().to_owned();
    let selected = schema.effective_feature(&diamond, "selected").unwrap();
    assert!(selected.feature.unsettable && selected.feature.volatile);
    assert_eq!(
        selected.identity,
        schema
            .effective_feature(&other, "selected")
            .unwrap()
            .identity
    );
    let alias = enum_value(&schema, &diamond, "alias");
    let primary = enum_value(&schema, &diamond, "primary");
    let foreign = enum_value(&schema, &other, "alias");
    let mut model = BasicModel::new(schema).unwrap();
    model.create("d", &diamond, "model").unwrap();
    let original = storage::export(&model).unwrap();
    assert!(!model.is_set("d", "selected").unwrap());
    model.set("d", "selected", alias.clone()).unwrap();
    assert!(model.is_set("d", "selected").unwrap());
    model.unset("d", "selected").unwrap();
    assert_same_model_content(storage::export(&model).unwrap(), &original);
    model.set("d", "choices", json!([alias, primary])).unwrap();
    assert!(model.is_set("d", "choices").unwrap());
    let before = storage::export(&model).unwrap();
    assert!(
        model
            .set_many(&[
                ("d", "selected", primary.clone()),
                ("d", "choices", json!([foreign]))
            ])
            .is_err()
    );
    assert_eq!(storage::export(&model).unwrap(), before);
    model.unset("d", "choices").unwrap();
    assert_same_model_content(storage::export(&model).unwrap(), &original);
    assert!(
        model
            .edit_collection("d", "history", CollectionEdit::Add(alias.clone()))
            .unwrap()
    );
    assert!(
        model
            .edit_collection("d", "history", CollectionEdit::Add(alias))
            .unwrap()
    );
    assert!(model.is_set("d", "history").unwrap());
    let before = storage::export(&model).unwrap();
    assert!(model.unset("d", "history").is_err());
    assert!(model.set("d", "history", json!([])).is_err());
    assert_eq!(storage::export(&model).unwrap(), before);
    assert!(
        model
            .edit_collection("d", "history", CollectionEdit::Clear)
            .unwrap()
    );
    assert_eq!(model.value("d", "history").unwrap(), Some(json!([])));
    assert!(model.is_set("d", "history").unwrap());
    model.validate().unwrap();
}
