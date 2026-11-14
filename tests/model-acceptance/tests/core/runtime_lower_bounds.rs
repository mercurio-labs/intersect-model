#[path = "../definitions/common/runtime_lower_bounds.rs"]
mod common;
use intersect_model::core::model::Model;
use intersect_model::definitions::compiler::materialize_runtime_types;
use serde_json::json;
#[test]
fn runtime_lower_domains_drive_model_edits_and_atomic_source_replay() {
    let f = common::Fixture::new();
    let base = f.atom(0);
    let sub = f.atom(1);
    let other = f.atom(2);
    let raw = f.ty(4, vec![], true);
    let closed = f.ty(4, vec![sub.expression().clone()], false);
    let inherited = f.ty(5, vec![], true);
    let good = f.pair(&base, &sub);
    let narrow = f.pair(&sub, &sub);
    let bad = f.pair(&base, &other);
    let types = vec![
        base,
        sub,
        other,
        f.ty(3, vec![], true),
        raw.clone(),
        closed.clone(),
        inherited.clone(),
        good.clone(),
        narrow.clone(),
        bad.clone(),
    ];
    let schema = materialize_runtime_types(&f.bundle, &types, None).unwrap();
    let mut model = Model::default();
    for (id, ty) in [
        ("raw", &raw),
        ("closed", &closed),
        ("inherited", &inherited),
        ("good", &good),
        ("narrow", &narrow),
        ("bad", &bad),
    ] {
        model
            .create(
                &schema,
                id,
                schema.class_key_for_type(ty).unwrap(),
                "memory:lower",
                0,
                0,
            )
            .unwrap();
    }
    model
        .edit(
            &schema,
            &[
                ("raw".into(), "value".into(), json!("good")),
                ("closed".into(), "value".into(), json!("narrow")),
                ("inherited".into(), "value".into(), json!("good")),
            ],
        )
        .unwrap();
    let before = model.clone();
    for (owner, target) in [
        ("raw", "narrow"),
        ("raw", "bad"),
        ("closed", "bad"),
        ("inherited", "narrow"),
    ] {
        assert!(
            model
                .edit(
                    &schema,
                    &[
                        ("raw".into(), "name".into(), json!("rollback")),
                        (owner.into(), "value".into(), json!(target))
                    ]
                )
                .is_err()
        );
        assert_eq!(model, before);
    }
    model.validate(&schema).unwrap();
    let replay = materialize_runtime_types(&f.bundle, &types, None).unwrap();
    assert_eq!(schema, replay);
    model.validate(&replay).unwrap();
    for domains in schema.runtime_feature_domains.values() {
        assert!(domains.iter().all(|domain| domain.unresolved.is_empty()));
    }
}
