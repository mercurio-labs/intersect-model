use intersect_model::core::{
    frozen::FrozenStore,
    model::{AssignmentState, Element, Model},
    storage::{self, ModelRead, ModelStore, ObjectHandle, ObjectView, Query},
};
use intersect_model::definitions::{
    dynamic::{ScalarType, SchemaBuilder},
    schema::Schema,
};
use serde_json::json;

fn raw() -> Element {
    serde_json::from_value(json!({"id":"object","class":"Item","type_identity":"urn:raw#Item",
        "resource":"memory:state","start":0,"end":1,"values":{"value":"seed","items":["a"],"null":null}})).unwrap()
}

fn schema() -> Schema {
    let mut builder = SchemaBuilder::new("state", "urn:raw");
    let item = builder.class("Item").unwrap();
    item.attribute("value", ScalarType::String)
        .unsettable(true)
        .default("seed")
        .unwrap();
    item.attribute("items", ScalarType::String)
        .many()
        .unsettable(true)
        .build()
        .unwrap();
    item.attribute("null", ScalarType::String)
        .unsettable(true)
        .optional()
        .unwrap();
    builder.build().unwrap().as_schema().clone()
}

fn typed_element(schema: &Schema) -> Element {
    let mut element = raw();
    element.type_identity = schema.identity("Item");
    element
}

fn with_view<T>(
    store: &FrozenStore,
    handle: &ObjectHandle,
    mut read: impl FnMut(ObjectView<'_>) -> T,
) -> T {
    let mut result = None;
    store
        .inspect_handle(handle, &mut |view| {
            result = Some(read(view));
            Ok(())
        })
        .unwrap();
    result.unwrap()
}

#[test]
fn assignment_wire_is_versioned_strict_and_keeps_legacy_absence_distinct() {
    let legacy = raw();
    let wire = serde_json::to_value(&legacy).unwrap();
    assert!(wire.get("assignment_state").is_none());
    let mut explicit = legacy.clone();
    explicit.assignment_state = Some(AssignmentState::new(Vec::<String>::new()).unwrap());
    assert_ne!(explicit, legacy);
    assert_eq!(
        serde_json::to_value(&explicit).unwrap()["assignment_state"],
        json!({"version":1,"set_features":[]})
    );
    assert_eq!(
        serde_json::from_value::<Element>(serde_json::to_value(&explicit).unwrap()).unwrap(),
        explicit
    );
    let state = AssignmentState::new(["z", "a"]).unwrap();
    assert_eq!(state.set_features().collect::<Vec<_>>(), ["a", "z"]);
    assert!(state.explicitly_set("a"));
    assert!(!state.explicitly_set("missing"));
    for wire in [
        json!({"version":0,"set_features":[]}),
        json!({"version":2,"set_features":[]}),
        json!({"version":1,"set_features":["a","a"]}),
        json!({"version":1,"set_features":[""]}),
        json!({"version":1,"set_features":[],"extra":false}),
        json!({"set_features":[]}),
    ] {
        assert!(serde_json::from_value::<AssignmentState>(wire).is_err());
    }
    assert!(AssignmentState::new(["a", "a"]).is_err());
}

#[test]
fn canonical_and_compatibility_state_survive_nested_rollback_without_changing_values() {
    let schema = schema();
    let mut element = typed_element(&schema);
    element.assignment_state = Some(AssignmentState::new(["value", "items"]).unwrap());
    let mut store = FrozenStore::for_schema(&schema).unwrap();
    storage::atomic(&mut store, |s| s.put(element.clone())).unwrap();
    let original = storage::export(&store).unwrap();
    let handle = store.handle("object").unwrap();
    with_view(&store, &handle, |view| {
        assert_eq!(view.assignment_state(), element.assignment_state.as_ref());
        assert_eq!(
            ObjectView::from_element(&element).assignment_state(),
            view.assignment_state()
        );
        assert_eq!(view.to_element(), element);
    });
    let outer = store.begin().unwrap();
    store
        .set_value("object", "value", Some(json!("changed")))
        .unwrap();
    assert_eq!(
        store.get("object").unwrap().unwrap().assignment_state,
        element.assignment_state
    );
    let inner = store.begin().unwrap();
    let mut replacement = store.get("object").unwrap().unwrap();
    replacement.assignment_state = Some(AssignmentState::new(Vec::<String>::new()).unwrap());
    store.put(replacement).unwrap();
    store.commit(&inner).unwrap();
    assert_eq!(
        store
            .query(Query::Value("value", &json!("changed")))
            .unwrap(),
        ["object"]
    );
    assert_eq!(
        with_view(&store, &handle, |view| view
            .assignment_state()
            .unwrap()
            .set_features()
            .len()),
        0
    );
    store.rollback(&outer).unwrap();
    assert_eq!(storage::export(&store).unwrap(), original);
    assert_eq!(
        with_view(&store, &handle, |view| view.to_element()),
        element
    );
    let model: Model = serde_json::from_value(serde_json::to_value(&original).unwrap()).unwrap();
    assert_eq!(
        storage::export(&FrozenStore::from_materialized(&schema, &model).unwrap()).unwrap(),
        original
    );
}

#[test]
fn assignment_preservation_does_not_silently_enable_unsettable_schema_semantics() {
    let schema = intersect_model_acceptance::migration_fixture::compile_migrated_schema_fixture(r#"<e:EPackage xmlns:e="http://www.eclipse.org/emf/2002/Ecore" xmlns:x="http://www.w3.org/2001/XMLSchema-instance" name="p" nsURI="urn:p"><eClassifiers x:type="e:EClass" name="Item"><eStructuralFeatures x:type="e:EAttribute" name="value" eType="e:EDataType http://www.eclipse.org/emf/2002/Ecore#//EString" defaultValueLiteral="seed"/></eClassifiers></e:EPackage>"#).unwrap();
    let mut model = Model::default();
    model
        .create(&schema, "object", "Item", "memory:state", 0, 1)
        .unwrap();
    model.validate(&schema).unwrap();
    model.elements[0].assignment_state = Some(AssignmentState::new(["value"]).unwrap());
    assert!(
        model
            .validate(&schema)
            .unwrap_err()
            .contains("unsettable schema support")
    );
    let store = FrozenStore::from_materialized(&schema, &model).unwrap();
    assert!(
        storage::validate(&store, &schema)
            .unwrap_err()
            .contains("unsettable schema support")
    );
    model.elements[0].assignment_state = None;
    model.validate(&schema).unwrap();
}

#[test]
fn raw_assignment_updates_preserve_payload_and_follow_transaction_watermarks() {
    let schema = schema();
    let mut store = FrozenStore::for_schema(&schema).unwrap();
    assert!(
        store
            .set_assignment_state("object", None)
            .unwrap_err()
            .contains("transaction")
    );
    storage::atomic(&mut store, |s| s.put(typed_element(&schema))).unwrap();
    let handle = store.handle("object").unwrap();
    let original = storage::export(&store).unwrap();
    let outer = store.begin().unwrap();
    assert!(
        store
            .set_assignment_state("missing", None)
            .unwrap_err()
            .contains("Missing element")
    );
    store
        .set_assignment_state("object", Some(AssignmentState::new(["value"]).unwrap()))
        .unwrap();
    let inner = store.begin().unwrap();
    store.set_assignment_state("object", None).unwrap();
    store
        .set_value("object", "value", Some(json!("changed")))
        .unwrap();
    store.rollback(&inner).unwrap();
    assert!(with_view(&store, &handle, |view| view
        .assignment_state()
        .unwrap()
        .explicitly_set("value")));
    assert_eq!(store.value("object", "value").unwrap(), Some(json!("seed")));
    let inner = store.begin().unwrap();
    store
        .set_assignment_state("object", Some(AssignmentState::new(["items"]).unwrap()))
        .unwrap();
    store.remove("object").unwrap();
    store.put(typed_element(&schema)).unwrap();
    store
        .set_assignment_state("object", Some(AssignmentState::new(["null"]).unwrap()))
        .unwrap();
    store.commit(&inner).unwrap();
    store.rollback(&outer).unwrap();
    assert_eq!(storage::export(&store).unwrap(), original);
    assert_eq!(
        with_view(&store, &handle, |view| view.to_element()),
        typed_element(&schema)
    );
    assert_eq!(
        store.query(Query::Value("items", &json!("a"))).unwrap(),
        ["object"]
    );
    let outer = store.begin().unwrap();
    let mut added = typed_element(&schema);
    added.id = "new".into();
    store.put(added).unwrap();
    let inner = store.begin().unwrap();
    store
        .set_assignment_state("new", Some(AssignmentState::new(["value"]).unwrap()))
        .unwrap();
    store.commit(&inner).unwrap();
    store.rollback(&outer).unwrap();
    assert!(store.get("new").unwrap().is_none());
    storage::atomic(&mut store, |s| {
        s.set_assignment_state("object", Some(AssignmentState::new(["value"])?))
    })
    .unwrap();
    assert!(with_view(&store, &handle, |view| view
        .assignment_state()
        .unwrap()
        .explicitly_set("value")));
    assert_eq!(
        with_view(&store, &handle, |view| view.to_element()).values,
        typed_element(&schema).values
    );
}
