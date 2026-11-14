#![cfg(not(target_arch = "wasm32"))]
use intersect_model::core::api::{BasicModel, ModelEdit};
use intersect_model::navigation::{IncomingCursor, IncomingStep};
use intersect_model::sqlite::SqliteStore;
use serde_json::json;

#[test]
fn sqlite_uses_the_shared_typed_cursor() {
    let schema = intersect_model_acceptance::migration_fixture::compile_migrated_schema_fixture(
        include_str!("../../../../fixtures/ecore-navigation/inverse.ecore"),
    )
    .unwrap();
    let mut model =
        BasicModel::from_store(schema.clone(), SqliteStore::in_memory().unwrap()).unwrap();
    for (id, class) in [
        ("target", "Node"),
        ("foreign", "Other"),
        ("z", "Child"),
        ("a", "Node"),
    ] {
        model.create(id, class, "r").unwrap();
    }
    for id in ["foreign", "z", "a"] {
        model.set(id, "links", json!(["target", "target"])).unwrap();
    }
    let member = schema.effective_feature("Node", "links").unwrap().identity;
    let mut cursor = IncomingCursor::new(&model, &schema, "Node", member, "target").unwrap();
    let mut matches = Vec::new();
    // Qualification now yields independently of adapter candidate pages.
    for _ in 0..10000 {
        match cursor.advance(&model, &schema).unwrap() {
            IncomingStep::Pending => (),
            IncomingStep::Match(id) => matches.push(id),
            IncomingStep::Done => {
                assert_eq!(matches, ["z", "a"]);
                return;
            }
        }
    }
    panic!("SQLite inverse query did not terminate");
}

#[test]
fn sqlite_retains_shared_validated_ownership_in_effective_feature_order() {
    let schema = intersect_model_acceptance::migration_fixture::compile_migrated_schema_fixture(
        include_str!("../../../../fixtures/ecore-navigation/ownership.ecore"),
    )
    .unwrap();
    let mut model =
        BasicModel::from_store(schema.clone(), SqliteStore::in_memory().unwrap()).unwrap();
    for (id, class) in [
        ("root", "Root"),
        ("b", "Base"),
        ("a", "Base"),
        ("single", "Base"),
        ("last", "Base"),
    ] {
        model.create(id, class, "r").unwrap();
    }
    model.set("root", "zChildren", json!(["b", "a"])).unwrap();
    model.set("root", "aChild", json!("single")).unwrap();
    model.set("root", "local", json!(["last"])).unwrap();
    let index = intersect_model::core::storage::validate_ownership_with_conversions(
        &model,
        &schema,
        &intersect_model::definitions::conversion::ConversionRegistry::default(),
    )
    .unwrap();
    assert_eq!(index.children("root"), ["b", "a", "single", "last"]);
    assert_eq!(index.parent("a"), Some("root"));
}
