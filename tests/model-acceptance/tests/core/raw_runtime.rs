use intersect_model::core::model::Model;
use intersect_model::definitions::{
    bundle::{DefinitionBundle, DefinitionSource, SourceKind},
    compiler::materialize_runtime_types,
    generic_types::{ClosedType, RuntimeType, Type},
    resources::DeclarationId,
    schema::Schema,
};
use serde_json::json;
#[test]
fn raw_models_preserve_identity_erasure_assignment_and_replay() {
    let source = DefinitionSource::new(
        "memory:raw",
        SourceKind::Ecore,
        include_str!("../../../../fixtures/ecore-generics/raw-runtime.ecore"),
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
    let value = ty(0, vec![], false);
    let child = ty(1, vec![], false);
    let other = ty(2, vec![], false);
    let raw = ty(3, vec![], true);
    let applied = ty(3, vec![child.expression().clone()], false);
    let holder = ty(4, vec![], false);
    let derived = ty(5, vec![], true);
    assert!(ClosedType::new(raw.expression().clone()).is_err());
    assert_ne!(raw.identity(), applied.identity());
    let bundle = DefinitionBundle {
        version: 1,
        sources: vec![source],
        namespaces: vec![],
    };
    let types = vec![
        value.clone(),
        child.clone(),
        other.clone(),
        raw.clone(),
        applied.clone(),
        holder.clone(),
        derived.clone(),
    ];
    let schema = materialize_runtime_types(&bundle, &types, None).unwrap();
    let key = |ty: &RuntimeType| schema.class_key_for_type(ty).unwrap();
    assert!(schema.conforms(key(&applied), key(&raw)));
    assert!(!schema.conforms(key(&raw), key(&applied)));
    assert!(schema.conforms(key(&derived), key(&raw)));
    let mut model = Model::default();
    for (id, ty) in [
        ("v", &value),
        ("c", &child),
        ("o", &other),
        ("r", &raw),
        ("a", &applied),
        ("h", &holder),
        ("d", &derived),
    ] {
        model
            .create(&schema, id, key(ty), "memory:model", 0, 0)
            .unwrap();
    }
    model
        .edit(
            &schema,
            &[
                ("r".into(), "value".into(), json!("v")),
                ("a".into(), "value".into(), json!("c")),
                ("d".into(), "value".into(), json!("v")),
                ("h".into(), "rawBox".into(), json!("a")),
                ("h".into(), "childBox".into(), json!("a")),
            ],
        )
        .unwrap();
    model.validate(&schema).unwrap();
    let before = serde_json::to_value(&model).unwrap();
    for (id, field, target) in [
        ("r", "value", "o"),
        ("a", "value", "v"),
        ("h", "childBox", "r"),
        ("h", "rawBox", "o"),
    ] {
        assert!(
            model
                .edit(
                    &schema,
                    &[
                        ("v".into(), "name".into(), json!("rollback")),
                        (id.into(), field.into(), json!(target))
                    ]
                )
                .is_err()
        );
        assert_eq!(serde_json::to_value(&model).unwrap(), before);
    }
    let saved: Schema = serde_json::from_value(serde_json::to_value(&schema).unwrap()).unwrap();
    model.validate(&saved).unwrap();
    assert_eq!(saved.identity(key(&raw)), raw.identity());
    let replay: Vec<RuntimeType> =
        serde_json::from_value(serde_json::to_value(&types).unwrap()).unwrap();
    assert_eq!(
        schema,
        materialize_runtime_types(&bundle, &replay, None).unwrap()
    );
    let mut forged = raw.expression().clone();
    if let Type::Classifier { raw, .. } = &mut forged {
        *raw = false;
    }
    assert!(
        materialize_runtime_types(&bundle, &[RuntimeType::new(forged).unwrap()], None).is_err()
    );
}

#[test]
fn raw_reference_intersections_enforce_every_bound() {
    let source = DefinitionSource::new(
        "memory:raw",
        SourceKind::Ecore,
        include_str!("../../../../fixtures/ecore-generics/raw-runtime.ecore"),
    );
    let (source, _) = intersect_model_acceptance::migration_fixture::migrate_source_fixture(source);
    let ty = |n, raw| {
        RuntimeType::new(Type::Classifier {
            declaration: DeclarationId {
                source: source.id.clone(),
                path: vec![n],
            },
            arguments: vec![],
            raw,
        })
        .unwrap()
    };
    let value = ty(0, false);
    let other = ty(2, false);
    let both = ty(6, false);
    let raw = ty(7, true);
    let schema = materialize_runtime_types(
        &DefinitionBundle {
            version: 1,
            sources: vec![source],
            namespaces: vec![],
        },
        &[value.clone(), other.clone(), both.clone(), raw.clone()],
        None,
    )
    .unwrap();
    let key = schema.class_key_for_type(&raw).unwrap();
    assert_eq!(schema.raw_reference_bounds[key]["value"].len(), 2);
    assert!(schema.runtime_warnings.iter().any(|w| w.code == "raw_type"));
    let mut model = Model::default();
    for (id, ty) in [("v", &value), ("o", &other), ("b", &both), ("r", &raw)] {
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
        .edit(&schema, &[("r".into(), "value".into(), json!("b"))])
        .unwrap();
    let before = serde_json::to_value(&model).unwrap();
    for bad in ["v", "o"] {
        assert!(
            model
                .edit(&schema, &[("r".into(), "value".into(), json!(bad))])
                .is_err()
        );
        assert_eq!(serde_json::to_value(&model).unwrap(), before);
    }
    let replay: Schema = serde_json::from_value(serde_json::to_value(&schema).unwrap()).unwrap();
    model.validate(&replay).unwrap();
}

#[test]
fn raw_subclass_keeps_narrower_inherited_reference_bound() {
    let source = DefinitionSource::new(
        "memory:raw",
        SourceKind::Ecore,
        include_str!("../../../../fixtures/ecore-generics/raw-runtime.ecore"),
    );
    let (source, _) = intersect_model_acceptance::migration_fixture::migrate_source_fixture(source);
    let ty = |n, raw| {
        RuntimeType::new(Type::Classifier {
            declaration: DeclarationId {
                source: source.id.clone(),
                path: vec![n],
            },
            arguments: vec![],
            raw,
        })
        .unwrap()
    };
    let value = ty(0, false);
    let child = ty(1, false);
    let narrow = ty(8, true);
    let schema = materialize_runtime_types(
        &DefinitionBundle {
            version: 1,
            sources: vec![source],
            namespaces: vec![],
        },
        &[value.clone(), child.clone(), narrow.clone()],
        None,
    )
    .unwrap();
    let mut model = Model::default();
    for (id, ty) in [("v", &value), ("c", &child), ("n", &narrow)] {
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
        .edit(&schema, &[("n".into(), "value".into(), json!("c"))])
        .unwrap();
    assert!(
        model
            .edit(&schema, &[("n".into(), "value".into(), json!("v"))])
            .is_err()
    );
    model.validate(&schema).unwrap();
}
