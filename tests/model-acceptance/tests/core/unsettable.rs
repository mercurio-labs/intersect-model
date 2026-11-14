use intersect_model::core::{
    api::{BasicModel, ModelEdit, VersionedModel},
    frozen::FrozenStore,
    model::{AssignmentState, Element},
    storage::{self, ModelRead, ModelStore},
};
use intersect_model::definitions::{
    dynamic::{ScalarType, SchemaBuilder},
    schema::Schema,
};
use serde_json::{Value, json};

fn schema(many: bool) -> Schema {
    let bound = if many { "-1" } else { "1" };
    let default = if many {
        ""
    } else {
        "defaultValueLiteral=\"seed\""
    };
    intersect_model_acceptance::migration_fixture::compile_migrated_schema_fixture(&format!(r##"<e:EPackage xmlns:e="http://www.eclipse.org/emf/2002/Ecore" xmlns:x="http://www.w3.org/2001/XMLSchema-instance" name="p" nsURI="urn:p"><eClassifiers x:type="e:EClass" name="Base"><eStructuralFeatures x:type="e:EAttribute" name="value" eType="e:EDataType http://www.eclipse.org/emf/2002/Ecore#//EString" unsettable="true" upperBound="{bound}" {default}/></eClassifiers><eClassifiers x:type="e:EClass" name="Item" eSuperTypes="#//Base"/></e:EPackage>"##)).unwrap()
}
fn store(schema: &Schema) -> FrozenStore {
    let mut store = FrozenStore::for_schema(schema).unwrap();
    storage::atomic(&mut store, |s| {
        s.create_element(schema, "object", "Item", "memory:model", 0, 0)
    })
    .unwrap();
    store
}
#[test]
fn source_dynamic_inheritance_and_construction_share_assignment_contract() {
    let source = schema(false);
    assert!(source.features("Item").unwrap()["value"].unsettable);
    let mut builder = SchemaBuilder::new("p", "urn:p");
    let base = builder.class("Base").unwrap();
    base.attribute("value", ScalarType::String)
        .unsettable(true)
        .default("seed")
        .unwrap();
    builder.class("Item").unwrap().extends(&base).unwrap();
    let dynamic = builder.build().unwrap();
    assert_eq!(
        source.features("Item").unwrap(),
        dynamic.as_schema().features("Item").unwrap()
    );
    let memory = store(&source);
    let element = Element::new(&source, "object", "Item", "memory:model", 0, 0).unwrap();
    assert_eq!(memory.get("object").unwrap().unwrap(), element);
    assert!(!storage::is_explicitly_set(&memory, &source, "object", "value").unwrap());
    storage::validate(&memory, &source).unwrap();
}
#[test]
fn scalar_and_many_replay_pinned_dynamic_emf_assignment_observations() {
    let oracle: Value = serde_json::from_str(include_str!(
        "../../../../fixtures/ecore-feature-flags/emf-dynamic-observations.json"
    ))
    .unwrap();
    let cases = oracle["cases"].as_array().unwrap();
    let mut checked = 0;
    for case in cases.iter().filter(|c| {
        c["changeable"] == true
            && c["unsettable"] == true
            && c["derived"] == false
            && c["volatile"] == false
            && c["transient"] == false
    }) {
        let many = case["many"].as_bool().unwrap();
        let schema = schema(many);
        let mut memory = store(&schema);
        for step in case["steps"].as_array().unwrap() {
            let operation = step["operation"].as_str().unwrap();
            match operation {
                "fresh" => (),
                "unset" | "unset-again" | "unset-default" => {
                    storage::unset(&mut memory, &schema, "object", "value").unwrap()
                }
                _ => storage::edit(
                    &mut memory,
                    &schema,
                    &[("object".into(), "value".into(), step["value"].clone())],
                )
                .unwrap(),
            }
            assert_eq!(
                memory.value("object", "value").unwrap().unwrap(),
                step["value"],
                "{many} {operation}"
            );
            assert_eq!(
                storage::is_explicitly_set(&memory, &schema, "object", "value").unwrap(),
                step["is_set"],
                "{many} {operation}"
            );
            storage::validate(&memory, &schema).unwrap();
            checked += 1;
        }
    }
    assert_eq!(checked, 16);
}
#[test]
fn assignment_to_default_is_transactional_and_failed_batch_restores_history() {
    let schema = schema(false);
    let mut memory = store(&schema);
    let original = storage::export(&memory).unwrap();
    let outer = memory.begin().unwrap();
    storage::edit(
        &mut memory,
        &schema,
        &[("object".into(), "value".into(), json!("seed"))],
    )
    .unwrap();
    assert!(storage::is_explicitly_set(&memory, &schema, "object", "value").unwrap());
    storage::unset(&mut memory, &schema, "object", "value").unwrap();
    memory.rollback(&outer).unwrap();
    assert_eq!(storage::export(&memory).unwrap(), original);
    assert!(
        storage::edit(
            &mut memory,
            &schema,
            &[
                ("object".into(), "value".into(), json!("seed")),
                ("object".into(), "missing".into(), json!(0))
            ]
        )
        .is_err()
    );
    assert_eq!(storage::export(&memory).unwrap(), original);
}
#[test]
fn invalid_or_missing_assignment_evidence_cannot_be_guessed_from_values() {
    let schema = schema(false);
    for (state, value) in [
        (None, json!("seed")),
        (
            Some(AssignmentState::new(["missing"]).unwrap()),
            json!("seed"),
        ),
        (
            Some(AssignmentState::new(Vec::<String>::new()).unwrap()),
            json!("changed"),
        ),
    ] {
        let mut memory = store(&schema);
        storage::atomic(&mut memory, |s| {
            s.set_assignment_state("object", state.clone())?;
            s.set_value("object", "value", Some(value.clone()))
        })
        .unwrap();
        let before = storage::export(&memory).unwrap();
        assert!(storage::validate(&memory, &schema).is_err());
        assert!(storage::is_explicitly_set(&memory, &schema, "object", "value").is_err());
        assert!(storage::unset(&mut memory, &schema, "object", "value").is_err());
        assert_eq!(storage::export(&memory).unwrap(), before);
    }
}
#[test]
fn default_assignment_survives_versioned_snapshots_without_a_value_delta() {
    let schema = schema(false);
    let mut basic = BasicModel::new(schema.clone()).unwrap();
    basic.create("object", "Item", "memory:model").unwrap();
    let versioned = VersionedModel::new(basic).unwrap();
    let baseline = versioned.snapshot().unwrap();
    let mut fork = versioned.fork(&baseline).unwrap();
    fork.set("object", "value", json!("seed")).unwrap();
    assert!(storage::is_explicitly_set(&fork, &schema, "object", "value").unwrap());
    assert!(!storage::is_explicitly_set(&baseline, &schema, "object", "value").unwrap());
    assert_eq!(
        fork.value("object", "value").unwrap(),
        baseline.value("object", "value").unwrap()
    );
    fork.validate().unwrap();
    fork.unset("object", "value").unwrap();
    assert!(!fork.is_explicitly_set("object", "value").unwrap());
    assert_eq!(fork.get("object").unwrap(), baseline.get("object").unwrap());
}

#[test]
fn all_collection_kinds_preserve_empty_assignment_and_unset_distinction() {
    for ordered in [false, true] {
        for unique in [false, true] {
            let mut builder = SchemaBuilder::new("p", "urn:p");
            builder
                .class("Item")
                .unwrap()
                .attribute("value", ScalarType::String)
                .many()
                .ordered(ordered)
                .unique(unique)
                .unsettable(true)
                .build()
                .unwrap();
            let dynamic = builder.build().unwrap();
            let schema = dynamic.as_schema();
            let mut memory = store(schema);
            storage::edit(
                &mut memory,
                schema,
                &[("object".into(), "value".into(), json!([]))],
            )
            .unwrap();
            assert!(storage::is_explicitly_set(&memory, schema, "object", "value").unwrap());
            let before = storage::export(&memory).unwrap();
            let result = storage::edit(
                &mut memory,
                schema,
                &[("object".into(), "value".into(), json!(["a", "a"]))],
            );
            assert_eq!(result.is_err(), unique);
            if unique {
                assert_eq!(storage::export(&memory).unwrap(), before);
            }
            storage::unset(&mut memory, schema, "object", "value").unwrap();
            assert!(!storage::is_explicitly_set(&memory, schema, "object", "value").unwrap());
            assert_eq!(memory.value("object", "value").unwrap(), Some(json!([])));
        }
    }
}

#[test]
fn reference_assignment_and_reset_share_default_state_and_target_validation() {
    let mut builder = SchemaBuilder::new("p", "urn:p");
    let item = builder.class("Item").unwrap();
    item.reference("next", &item)
        .unsettable(true)
        .optional()
        .unwrap();
    let dynamic = builder.build().unwrap();
    let schema = dynamic.as_schema();
    let mut memory = store(schema);
    storage::edit(
        &mut memory,
        schema,
        &[("object".into(), "next".into(), Value::Null)],
    )
    .unwrap();
    assert!(storage::is_explicitly_set(&memory, schema, "object", "next").unwrap());
    let before = storage::export(&memory).unwrap();
    assert!(
        storage::edit(
            &mut memory,
            schema,
            &[("object".into(), "next".into(), json!("missing"))]
        )
        .is_err()
    );
    assert_eq!(storage::export(&memory).unwrap(), before);
    storage::edit(
        &mut memory,
        schema,
        &[("object".into(), "next".into(), json!("object"))],
    )
    .unwrap();
    storage::unset(&mut memory, schema, "object", "next").unwrap();
    assert_eq!(memory.value("object", "next").unwrap(), Some(Value::Null));
    assert!(!storage::is_explicitly_set(&memory, schema, "object", "next").unwrap());
    storage::validate(&memory, schema).unwrap();
}

#[test]
fn derived_construction_has_no_fabricated_history_and_state_requires_a_provider() {
    let mut derived = schema(false);
    derived
        .classes
        .get_mut("Base")
        .unwrap()
        .features
        .get_mut("value")
        .unwrap()
        .derived = true;
    let object = Element::new(&derived, "object", "Item", "model", 0, 0).unwrap();
    assert!(object.assignment_state.is_none());
    assert!(!object.values.contains_key("value"));
    let store = store(&derived);
    storage::validate(&store, &derived).unwrap();
    assert!(storage::is_set(&store, &derived, "object", "value").is_err());
    assert!(
        storage::is_set_with_derived(
            &store,
            &derived,
            "object",
            "value",
            &intersect_model::core::query::NoDerivedValues,
            &mut intersect_model::core::query::QueryBudget::new(100)
        )
        .is_err()
    );
}

#[test]
fn basic_and_transactional_reset_share_checked_history_and_domain_rollback() {
    let schema = schema(false);
    let mut basic = BasicModel::new(schema.clone()).unwrap();
    basic.create("object", "Item", "model").unwrap();
    basic.set("object", "value", json!("seed")).unwrap();
    basic.unset("object", "value").unwrap();
    assert!(!basic.is_explicitly_set("object", "value").unwrap());
    basic.set("object", "value", json!("seed")).unwrap();
    let mut transactional = intersect_model::core::api::TransactionalModel::new(basic);
    let before = storage::export(&transactional).unwrap();
    let result = transactional.transaction_with(
        |edit| {
            edit.unset("object", "value")?;
            assert!(!edit.is_explicitly_set("object", "value")?);
            Ok(())
        },
        |_| Err("domain veto".into()),
    );
    assert_eq!(result.unwrap_err(), "domain veto");
    assert_eq!(storage::export(&transactional).unwrap(), before);
    transactional
        .transaction(|edit| edit.unset("object", "value"))
        .unwrap();
    assert!(
        !transactional
            .into_basic()
            .is_explicitly_set("object", "value")
            .unwrap()
    );
}
