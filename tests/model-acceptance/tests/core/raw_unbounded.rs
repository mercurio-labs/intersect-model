use intersect_model::core::model::Model;
use intersect_model::definitions::{
    bundle::{DefinitionBundle, DefinitionSource, SourceKind},
    compiler::materialize_runtime_types,
    generic_types::{RuntimeType, Type},
    resources::DeclarationId,
    schema::{EOBJECT_REFERENCE_TOP, Schema, eobject_reference_top_declaration},
};
use serde_json::json;

#[test]
fn raw_unbounded_reference_domain_is_exact_and_replayable() {
    let source = DefinitionSource::new(
        "memory:raw-unbounded",
        SourceKind::Ecore,
        include_str!("../../../../fixtures/ecore-generics/raw-unbounded.ecore"),
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
    let other = ty(1, vec![], false);
    let raw = ty(2, vec![], true);
    let applied = ty(2, vec![value.expression().clone()], false);
    let local = ty(3, vec![], false);
    let holder = ty(4, vec![], false);
    let types = vec![
        value.clone(),
        other.clone(),
        raw.clone(),
        applied.clone(),
        local.clone(),
        holder.clone(),
    ];
    let bundle = DefinitionBundle {
        version: 1,
        sources: vec![source],
        namespaces: vec![],
    };
    let schema = materialize_runtime_types(&bundle, &types, None).unwrap();
    assert_eq!(
        schema.reference_top,
        Some(eobject_reference_top_declaration().unwrap())
    );
    assert!(!schema.classes.contains_key(EOBJECT_REFERENCE_TOP));
    assert!(schema.runtime_warnings.iter().any(|w| w.code == "raw_type"));
    let mut model = Model::default();
    for (id, ty) in [
        ("v", &value),
        ("o", &other),
        ("r", &raw),
        ("a", &applied),
        ("e", &local),
        ("h", &holder),
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
    for id in ["v", "o", "r", "a", "e", "h"] {
        model
            .edit(
                &schema,
                &[
                    ("r".into(), "value".into(), json!(id)),
                    ("h".into(), "top".into(), json!(id)),
                ],
            )
            .unwrap();
    }
    model
        .edit(
            &schema,
            &[
                ("h".into(), "local".into(), json!("e")),
                ("a".into(), "value".into(), json!("v")),
            ],
        )
        .unwrap();
    let before = serde_json::to_value(&model).unwrap();
    for (id, name, value) in [
        ("r", "value", json!("missing")),
        ("r", "value", json!(7)),
        ("h", "local", json!("o")),
        ("a", "value", json!("o")),
    ] {
        assert!(
            model
                .edit(
                    &schema,
                    &[
                        ("v".into(), "name".into(), json!("rollback")),
                        (id.into(), name.into(), value)
                    ]
                )
                .is_err()
        );
        assert_eq!(serde_json::to_value(&model).unwrap(), before);
    }
    assert!(
        model
            .create(&schema, "top", EOBJECT_REFERENCE_TOP, "memory:model", 0, 0)
            .is_err()
    );
    assert!(!schema.conforms("EString", EOBJECT_REFERENCE_TOP));
    assert!(!schema.conforms("missing", EOBJECT_REFERENCE_TOP));
    let replay: Schema = serde_json::from_value(serde_json::to_value(&schema).unwrap()).unwrap();
    model.validate(&replay).unwrap();
    let retained: Vec<RuntimeType> =
        serde_json::from_value(serde_json::to_value(&types).unwrap()).unwrap();
    assert_eq!(
        schema,
        materialize_runtime_types(&bundle, &retained, None).unwrap()
    );
    let mut forged = replay.clone();
    forged.reference_top = Some(match local.expression() {
        Type::Classifier { declaration, .. } => declaration.clone(),
        _ => unreachable!(),
    });
    assert!(!forged.conforms(
        schema.class_key_for_type(&value).unwrap(),
        EOBJECT_REFERENCE_TOP
    ));
    assert!(forged.validate_instantiations().is_err());
    assert!(model.validate(&forged).is_err());
    let mut stale = replay;
    stale.reference_top.as_mut().unwrap().source.sha256 = "0".repeat(64);
    assert!(stale.validate_instantiations().is_err());
    let top = RuntimeType::new(Type::Classifier {
        declaration: eobject_reference_top_declaration().unwrap(),
        arguments: vec![],
        raw: false,
    })
    .unwrap();
    let constructed = materialize_runtime_types(&bundle, std::slice::from_ref(&top), None).unwrap();
    let key = constructed.class_key_for_type(&top).unwrap();
    assert_ne!(key, EOBJECT_REFERENCE_TOP);
    assert_eq!(constructed.classes.len(), 1);
    assert_eq!(constructed.effective_operations(key).unwrap().len(), 15);
    let mut explicit = Model::default();
    explicit
        .create(&constructed, "top", key, "memory:model", 0, 0)
        .unwrap();
    explicit.validate(&constructed).unwrap();
}

#[test]
fn reference_only_top_does_not_synthesize_operation_classes() {
    let source = DefinitionSource::new(
        "memory:top-operation",
        SourceKind::Ecore,
        r#"<e:EPackage xmlns:e="http://www.eclipse.org/emf/2002/Ecore" xmlns:x="http://www.w3.org/2001/XMLSchema-instance" name="top" nsURI="urn:top-operation">
        <eClassifiers x:type="e:EClass" name="Owner"><eOperations name="get" eType="e:EClass http://www.eclipse.org/emf/2002/Ecore#//EObject"/></eClassifiers>
        </e:EPackage>"#,
    );
    let (source, _) = intersect_model_acceptance::migration_fixture::migrate_source_fixture(source);
    let owner = RuntimeType::new(Type::Classifier {
        declaration: DeclarationId {
            source: source.id.clone(),
            path: vec![0],
        },
        arguments: vec![],
        raw: false,
    })
    .unwrap();
    let bundle = DefinitionBundle {
        version: 1,
        sources: vec![source],
        namespaces: vec![],
    };
    let error = materialize_runtime_types(&bundle, &[owner], None).unwrap_err();
    assert!(
        error.to_string().contains("Unknown operation class"),
        "{error}"
    );
}
