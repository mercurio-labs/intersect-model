use intersect_model::core::{
    frozen::FrozenStore,
    model::Element,
    storage::{self, ModelRead, ModelStore, Query},
};
use intersect_model::definitions::{
    dynamic::{ScalarType, SchemaBuilder},
    schema::Schema,
};
use serde_json::json;

fn record(schema: &Schema, id: &str) -> Element {
    Element {
        implicit: false,
        initialization: None,
        assignment_state: None,
        id: id.into(),
        class: "Raw".into(),
        type_identity: schema.identity("Raw"),
        resource: "source:α".into(),
        start: 7,
        end: 42,
        values: serde_json::from_value(json!({
            "null": null, "flag": true, "integer": 123, "float": 1.5,
            "text": "λ\n\"", "strings": ["a", "b", "a"], "empty": [],
            "mixed": [null, true, 42, ["nested"], {"a": 1}], "object": {"z": [1, 2]},
        }))
        .unwrap(),
    }
}

#[test]
fn compact_records_preserve_raw_json_indexes_order_and_nested_rollback() {
    fn send_sync<T: Send + Sync>() {}
    send_sync::<FrozenStore>();
    let mut builder = SchemaBuilder::new("rows", "urn:raw");
    let raw = builder.class("Raw").unwrap();
    for field in ["null", "flag", "integer", "float", "text", "object"] {
        raw.attribute(field, ScalarType::String).optional().unwrap();
    }
    for field in ["strings", "empty", "mixed"] {
        raw.attribute(field, ScalarType::String)
            .many()
            .unique(false)
            .build()
            .unwrap();
    }
    let changed = builder.class("Different").unwrap();
    changed.extends(&raw).unwrap();
    changed
        .attribute("extra", ScalarType::String)
        .optional()
        .unwrap();
    let schema = builder.build().unwrap();
    let schema = schema.as_schema();
    let mut store = FrozenStore::for_schema(schema).unwrap();
    let first = record(schema, "first");
    let second = record(schema, "second");
    storage::atomic(&mut store, |s| {
        s.put(first.clone())?;
        s.put(second.clone())
    })
    .unwrap();
    let snapshot = storage::export(&store).unwrap();
    let outer = store.begin().unwrap();
    store.remove("first").unwrap();
    assert_eq!(
        store.query(Query::Value("strings", &json!("a"))).unwrap(),
        ["second"]
    );
    let inner = store.begin().unwrap();
    let mut changed = second.clone();
    changed.class = "Different".into();
    changed.type_identity = schema.identity("Different");
    changed
        .values
        .insert("extra".into(), json!({"x": [false, null]}));
    store.put(changed).unwrap();
    store.commit(&inner).unwrap();
    store.rollback(&outer).unwrap();
    assert_eq!(storage::export(&store).unwrap(), snapshot);
    assert_eq!(
        store.query(Query::Value("strings", &json!("a"))).unwrap(),
        ["first", "second"]
    );
    assert_eq!(
        store
            .query(Query::Value("mixed", &json!(["nested"])))
            .unwrap(),
        ["first", "second"]
    );
    assert_eq!(
        store.query(Query::Value("null", &json!(null))).unwrap(),
        Vec::<String>::new()
    );
    assert!(store.matches("first", Query::Resource("source:α")).unwrap());
    let result = store.into_model().unwrap();
    assert_eq!(result, snapshot);
    assert_eq!(
        serde_json::to_value(&result.elements[0]).unwrap(),
        serde_json::to_value(first).unwrap()
    );
}

#[test]
fn empty_and_active_consuming_exports_are_explicit() {
    let schema = crate::stores::empty_schema().unwrap();
    assert!(
        FrozenStore::for_schema(&schema)
            .unwrap()
            .into_model()
            .unwrap()
            .elements
            .is_empty()
    );
    let mut store = FrozenStore::for_schema(&schema).unwrap();
    store.begin().unwrap();
    assert!(store.into_model().is_err());
}
