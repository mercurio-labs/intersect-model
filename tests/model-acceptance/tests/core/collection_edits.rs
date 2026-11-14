use intersect_model::core::{
    api::{BasicModel, ModelEdit, VersionedModel},
    frozen::FrozenStore,
    storage::{self, CollectionEdit, ModelRead, ModelStore},
};
use intersect_model::definitions::{
    dynamic::{ScalarType, SchemaBuilder},
    schema::Schema,
};
use serde_json::{Value, json};
fn schema(case: &Value) -> Schema {
    let mut b = SchemaBuilder::new("p", "urn:p");
    let c = b.class("Item").unwrap();
    c.attribute("value", ScalarType::String)
        .many()
        .changeable(case["changeable"] == true)
        .unsettable(case["unsettable"] == true)
        .unique(case["unique"] == true)
        .ordered(case["ordered"] == true)
        .build()
        .unwrap();
    b.build().unwrap().as_schema().clone()
}
#[test]
fn all_sixteen_dynamic_collection_configurations_match_emf() {
    let oracle: Value = serde_json::from_str(include_str!(
        "../../../../fixtures/ecore-feature-flags/emf-collections-observations.json"
    ))
    .unwrap();
    let mut count = 0;
    for case in oracle["cases"].as_array().unwrap() {
        let schema = schema(case);
        let mut store = FrozenStore::for_schema(&schema).unwrap();
        storage::atomic(&mut store, |s| {
            s.create_element(&schema, "a", "Item", "model", 0, 0)
        })
        .unwrap();
        for step in case["steps"].as_array().unwrap() {
            let operation = step["operation"].as_str().unwrap();
            let accepted = match operation {
                "fresh" => true,
                "reset" => storage::unset(&mut store, &schema, "a", "value").is_ok(),
                _ => {
                    let edit = match operation {
                        "remove-missing-fresh" => CollectionEdit::Remove(json!("missing")),
                        "clear-fresh" | "clear-empty" => CollectionEdit::Clear,
                        "add-a" | "add-a-again" => CollectionEdit::Add(json!("a")),
                        "add-b" => CollectionEdit::Add(json!("b")),
                        "remove-a" | "remove-a-again" => CollectionEdit::Remove(json!("a")),
                        "remove-b" => CollectionEdit::Remove(json!("b")),
                        _ => panic!("Unknown operation"),
                    };
                    storage::edit_collection(&mut store, &schema, "a", "value", edit).is_ok()
                }
            };
            assert_eq!(
                json!(accepted),
                step["accepted"],
                "{} {operation}",
                case["mask"]
            );
            let value = store.value("a", "value").unwrap().unwrap();
            assert_eq!(value, step["value"]);
            let is_set = if case["unsettable"] == true {
                storage::is_explicitly_set(&store, &schema, "a", "value").unwrap()
            } else {
                !value.as_array().unwrap().is_empty()
            };
            assert_eq!(
                json!(is_set),
                step["is_set"],
                "{} {operation}",
                case["mask"]
            );
            count += 1;
        }
    }
    assert_eq!(count, 176);
}
#[test]
fn collection_mutations_preserve_snapshot_and_rollback_invalid_items() {
    let schema =
        schema(&json!({"changeable":false,"unsettable":true,"unique":true,"ordered":true}));
    let mut basic = BasicModel::new(schema).unwrap();
    basic.create("a", "Item", "model").unwrap();
    let owner = VersionedModel::new(basic).unwrap();
    let snapshot = owner.snapshot().unwrap();
    let mut fork = owner.fork(&snapshot).unwrap();
    assert!(
        fork.edit_collection("a", "value", CollectionEdit::Add(json!("x")))
            .unwrap()
    );
    assert!(
        !fork
            .edit_collection("a", "value", CollectionEdit::Add(json!("x")))
            .unwrap()
    );
    let before = storage::export(&fork).unwrap();
    assert!(
        fork.edit_collection("a", "value", CollectionEdit::Add(json!(42)))
            .is_err()
    );
    assert_eq!(storage::export(&fork).unwrap(), before);
    assert_eq!(snapshot.value("a", "value").unwrap(), Some(json!([])));
    assert!(
        !snapshot
            .get("a")
            .unwrap()
            .unwrap()
            .assignment_state
            .unwrap()
            .explicitly_set("value")
    );
    assert!(
        fork.edit_collection("a", "value", CollectionEdit::Clear)
            .unwrap()
    );
    assert!(fork.is_explicitly_set("a", "value").unwrap());
    assert!(fork.set("a", "value", json!([])).is_err());
}

#[test]
fn upper_bounds_nested_rollback_and_wrong_shape_do_not_leak_state() {
    let mut builder = SchemaBuilder::new("p", "urn:p");
    let class = builder.class("Item").unwrap();
    class
        .attribute("value", ScalarType::String)
        .bounds(0, 2)
        .unsettable(true)
        .build()
        .unwrap();
    class
        .attribute("scalar", ScalarType::String)
        .build()
        .unwrap();
    let schema = builder.build().unwrap().as_schema().clone();
    let mut store = FrozenStore::for_schema(&schema).unwrap();
    storage::atomic(&mut store, |s| {
        s.create_element(&schema, "a", "Item", "model", 0, 0)
    })
    .unwrap();
    let before = storage::export(&store).unwrap();
    let outer = store.begin().unwrap();
    for v in ["a", "b"] {
        storage::edit_collection(
            &mut store,
            &schema,
            "a",
            "value",
            CollectionEdit::Add(json!(v)),
        )
        .unwrap();
    }
    let full = storage::export(&store).unwrap();
    assert!(
        storage::edit_collection(
            &mut store,
            &schema,
            "a",
            "value",
            CollectionEdit::Add(json!("c"))
        )
        .is_err()
    );
    assert_eq!(storage::export(&store).unwrap(), full);
    assert!(
        storage::edit_collection(&mut store, &schema, "a", "scalar", CollectionEdit::Clear)
            .is_err()
    );
    store.rollback(&outer).unwrap();
    assert_eq!(storage::export(&store).unwrap(), before);
}

#[test]
fn indexed_edits_match_all_eight_ordered_emf_configurations() {
    let oracle: Value = serde_json::from_str(include_str!(
        "../../../../fixtures/ecore-feature-flags/emf-indexed-collections-observations.json"
    ))
    .unwrap();
    let mut checked = 0;
    for case in oracle["cases"].as_array().unwrap() {
        let schema = schema(case);
        let mut store = FrozenStore::for_schema(&schema).unwrap();
        storage::atomic(&mut store, |s| {
            s.create_element(&schema, "a", "Item", "model", 0, 0)
        })
        .unwrap();
        for step in case["steps"].as_array().unwrap() {
            let len = store
                .value("a", "value")
                .unwrap()
                .unwrap()
                .as_array()
                .unwrap()
                .len();
            let accepted = match step["operation"].as_str().unwrap() {
                "fresh" => true,
                "reset" => storage::unset(&mut store, &schema, "a", "value").is_ok(),
                op => {
                    let edit = match op {
                        "insert-b" | "insert-duplicate" => CollectionEdit::Insert {
                            index: 0,
                            value: json!("b"),
                        },
                        "insert-a" => CollectionEdit::Insert {
                            index: 0,
                            value: json!("a"),
                        },
                        "replace-first" => CollectionEdit::Replace {
                            index: 0,
                            value: json!("c"),
                        },
                        "replace-duplicate" => CollectionEdit::Replace {
                            index: 0,
                            value: json!("b"),
                        },
                        "move-last-first" => CollectionEdit::Move {
                            from: len - 1,
                            to: 0,
                        },
                        "move-same" => CollectionEdit::Move { from: 0, to: 0 },
                        "remove-at" => CollectionEdit::RemoveAt(1),
                        "invalid-insert" => CollectionEdit::Insert {
                            index: len + 1,
                            value: json!("x"),
                        },
                        "invalid-replace" => CollectionEdit::Replace {
                            index: len,
                            value: json!("x"),
                        },
                        "invalid-remove" => CollectionEdit::RemoveAt(len),
                        "invalid-move" => CollectionEdit::Move { from: 0, to: len },
                        "clear" => CollectionEdit::Clear,
                        "empty-remove" => CollectionEdit::RemoveAt(0),
                        _ => panic!("Unknown operation"),
                    };
                    storage::edit_collection(&mut store, &schema, "a", "value", edit).is_ok()
                }
            };
            assert_eq!(
                json!(accepted),
                step["accepted"],
                "{} {}",
                case["mask"],
                step["operation"]
            );
            let value = store.value("a", "value").unwrap().unwrap();
            assert_eq!(value, step["value"]);
            let is_set = if case["unsettable"] == true {
                storage::is_explicitly_set(&store, &schema, "a", "value").unwrap()
            } else {
                !value.as_array().unwrap().is_empty()
            };
            assert_eq!(json!(is_set), step["is_set"]);
            checked += 1;
        }
    }
    assert_eq!(checked, 128);
}
#[test]
fn indexed_unordered_and_hostile_bounds_fail_without_mutation() {
    for ordered in [false, true] {
        let schema =
            schema(&json!({"ordered":ordered,"unique":true,"unsettable":true,"changeable":false}));
        let mut model = BasicModel::new(schema).unwrap();
        model.create("a", "Item", "model").unwrap();
        model
            .edit_collection("a", "value", CollectionEdit::Add(json!("a")))
            .unwrap();
        let before = storage::export(&model).unwrap();
        let index = if ordered { usize::MAX } else { 0 };
        for edit in [
            CollectionEdit::Insert {
                index,
                value: json!("b"),
            },
            CollectionEdit::Replace {
                index,
                value: json!("b"),
            },
            CollectionEdit::RemoveAt(index),
            CollectionEdit::Move { from: index, to: 0 },
            CollectionEdit::Move { from: 0, to: index },
        ] {
            assert!(model.edit_collection("a", "value", edit).is_err());
            assert_eq!(storage::export(&model).unwrap(), before);
        }
    }
}

#[test]
fn moving_equal_duplicate_values_reports_no_content_change() {
    let schema =
        schema(&json!({"ordered":true,"unique":false,"unsettable":true,"changeable":false}));
    let mut model = BasicModel::new(schema).unwrap();
    model.create("a", "Item", "model").unwrap();
    for _ in 0..2 {
        model
            .edit_collection("a", "value", CollectionEdit::Add(json!("a")))
            .unwrap();
    }
    let before = storage::export(&model).unwrap();
    assert!(
        !model
            .edit_collection("a", "value", CollectionEdit::Move { from: 0, to: 1 })
            .unwrap()
    );
    assert_eq!(storage::export(&model).unwrap(), before);
}
