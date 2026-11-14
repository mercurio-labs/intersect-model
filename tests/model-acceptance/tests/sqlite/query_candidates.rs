#![cfg(not(target_arch = "wasm32"))]
use intersect_model::core::{
    frozen::FrozenStore,
    model::Element,
    storage::{self, ModelRead, ModelStore, Query},
};
use intersect_model::definitions::{
    dynamic::{ScalarType, SchemaBuilder},
    schema::Schema,
};
use intersect_model::sqlite::SqliteStore;
use serde_json::json;

fn collect(store: &dyn ModelRead, query: Query<'_>, limit: usize) -> Vec<String> {
    let mut after = None;
    let mut ids = Vec::new();
    loop {
        let batch = store.query_candidates(query, after, limit).unwrap();
        assert!(batch.exact);
        assert!(batch.rows.len() <= limit);
        ids.extend(batch.rows.into_iter().map(|(_, id)| id));
        if batch.exhausted {
            return ids;
        }
        assert!(batch.after > after);
        after = batch.after;
    }
}

fn exercise(store: &mut dyn ModelStore, schema: &Schema) {
    storage::atomic(store, |s| {
        for id in ["z", "a", "m", "q"] {
            s.put(Element {
                implicit: false,
                initialization: None,
                assignment_state: None,
                id: id.into(),
                class: "Node".into(),
                type_identity: schema.identity("Node"),
                resource: "r".into(),
                start: 0,
                end: 0,
                values: serde_json::from_value(json!({"refs":["target", "target"], "name":id}))
                    .map_err(|e| e.to_string())?,
            })?;
        }
        Ok(())
    })
    .unwrap();
    for limit in [1, 2, 4, 30] {
        for query in [
            Query::Class("Node"),
            Query::Resource("r"),
            Query::Value("refs", &json!("target")),
        ] {
            assert_eq!(collect(store, query, limit), ["z", "a", "m", "q"]);
        }
        assert_eq!(
            collect(store, Query::Value("name", &json!("m")), limit),
            ["m"]
        );
        assert!(collect(store, Query::Value("refs", &json!("absent")), limit).is_empty());
    }
    let query = Query::Value("refs", &json!("target"));
    assert!(store.query_candidates(query, None, 0).is_err());
    let end = store.query_candidates(query, Some(u64::MAX), 1).unwrap();
    assert!(end.exhausted && end.rows.is_empty());
    let tx = store.begin().unwrap();
    store.remove("a").unwrap();
    store.set_value("z", "refs", Some(json!([]))).unwrap();
    assert_eq!(collect(store, query, 1), ["m", "q"]);
    store.rollback(&tx).unwrap();
    assert_eq!(collect(store, query, 2), ["z", "a", "m", "q"]);
    storage::atomic(store, |s| s.remove("a")).unwrap();
    assert_eq!(collect(store, query, 1), ["z", "m", "q"]);
}

#[test]
fn indexed_candidates_match_literal_graph_expectations_in_frozen_and_sqlite() {
    let mut builder = SchemaBuilder::new("test", "urn:test");
    let node = builder.class("Node").unwrap();
    node.attribute("name", ScalarType::String)
        .optional()
        .unwrap();
    node.reference("refs", &node)
        .many()
        .unique(false)
        .build()
        .unwrap();
    let schema = builder.build().unwrap();
    let schema = schema.as_schema();
    exercise(&mut FrozenStore::for_schema(schema).unwrap(), schema);
    exercise(&mut SqliteStore::in_memory().unwrap(), schema);
}

#[test]
fn exact_feature_projection_uses_sqlite_pages_and_scalar_defaults() {
    use intersect_model::core::{
        api::{BasicModel, ModelEdit},
        storage::StoredFeatureProjection,
    };
    let schema = intersect_model_acceptance::migration_fixture::compile_migrated_schema_fixture(
        include_str!("../../../../fixtures/ecore-navigation/projection.ecore"),
    )
    .unwrap();
    let label = schema.effective_feature("Base", "label").unwrap().identity;
    let children = schema
        .effective_feature("Base", "children")
        .unwrap()
        .identity;
    let mut model =
        BasicModel::from_store(schema.clone(), SqliteStore::in_memory().unwrap()).unwrap();
    for id in ["owner", "a", "b"] {
        model.create(id, "Diamond", "r").unwrap();
    }
    model.set("owner", "children", json!(["b", "a"])).unwrap();
    assert_eq!(
        StoredFeatureProjection::new(&model, &schema, "Base", &label, "owner")
            .unwrap()
            .singular()
            .unwrap(),
        json!("seed")
    );
    let read = StoredFeatureProjection::new(&model, &schema, "Base", &children, "owner").unwrap();
    assert_eq!(read.collection_page(1, 1).unwrap(), [json!("a")]);
    assert!(read.collection_page(2, 1).unwrap().is_empty());
    assert!(model.collection_page("missing", "children", 0, 1).is_err());
    assert!(model.collection_page("owner", "label", 0, 1).is_err());
}

#[test]
fn sqlite_late_collection_pages_preserve_raw_storage_shape_errors() {
    let mut store = SqliteStore::in_memory().unwrap();
    storage::atomic(&mut store, |s| {
        s.put(Element {
            implicit: false,
            initialization: None,
            assignment_state: None,
            id: "raw".into(),
            class: "Raw".into(),
            type_identity: "raw".into(),
            resource: "r".into(),
            start: 0,
            end: 0,
            values: [
                ("items".into(), json!((0..10000).collect::<Vec<_>>())),
                ("scalar".into(), json!(0)),
            ]
            .into(),
        })
    })
    .unwrap();
    assert_eq!(
        store.collection_page("raw", "items", 9998, 1).unwrap(),
        [json!(9998)]
    );
    assert_eq!(
        store.collection_page("raw", "items", 9999, 100).unwrap(),
        [json!(9999)]
    );
    assert!(
        store
            .collection_page("raw", "items", 10000, 1)
            .unwrap()
            .is_empty()
    );
    assert!(store.collection_page("raw", "scalar", 0, 1).is_err());
    assert!(store.collection_page("raw", "missing", 0, 1).is_err());
}
