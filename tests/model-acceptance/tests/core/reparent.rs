use intersect_model::core::{
    api::{BasicModel, ModelEdit, TransactionalModel, VersionedModel},
    model::Model,
    query::QueryBudget,
    storage::{self, ContainmentDestination, ModelRead, Query, ReparentRequest},
};
use intersect_model::definitions::dynamic::SchemaBuilder;
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

fn move_to<'a>(
    object: &'a str,
    owner: &'a str,
    feature: &'a str,
    position: Option<usize>,
) -> ReparentRequest<'a> {
    ReparentRequest {
        object,
        destination: Some(ContainmentDestination {
            owner,
            feature,
            position,
        }),
    }
}
fn fixture() -> BasicModel {
    let mut b = SchemaBuilder::new("tree", "urn:tree");
    let n = b.class("Node").unwrap();
    n.reference("left", &n)
        .many()
        .unsettable(true)
        .containment()
        .unwrap();
    n.reference("right", &n)
        .bounds(0, 2)
        .unsettable(true)
        .containment()
        .unwrap();
    n.reference("slot", &n)
        .unsettable(true)
        .containment()
        .unwrap();
    let mut model = BasicModel::new(b.build().unwrap().as_schema().clone()).unwrap();
    for id in ["a", "b", "c", "x", "y"] {
        model
            .create(id, "Node", if id == "x" { "second" } else { "first" })
            .unwrap();
    }
    model.set("a", "left", json!(["x", "y"])).unwrap();
    model
}
#[test]
fn no_opposite_cross_feature_ordered_and_singular_moves_keep_identities() {
    let mut model = fixture();
    let original = model.get("x").unwrap().unwrap();
    assert!(
        !model
            .reparent(
                move_to("x", "a", "left", None),
                None,
                &mut QueryBudget::new(10000)
            )
            .unwrap()
    );
    assert!(
        model
            .reparent(
                move_to("x", "a", "left", Some(1)),
                None,
                &mut QueryBudget::new(10000)
            )
            .unwrap()
    );
    assert_eq!(model.value("a", "left").unwrap(), Some(json!(["y", "x"])));
    model
        .reparent(
            move_to("x", "b", "right", Some(0)),
            None,
            &mut QueryBudget::new(10000),
        )
        .unwrap();
    assert_eq!(model.value("a", "left").unwrap(), Some(json!(["y"])));
    model.set("b", "slot", json!("c")).unwrap();
    model
        .reparent(
            move_to("x", "b", "slot", None),
            None,
            &mut QueryBudget::new(10000),
        )
        .unwrap();
    assert!(model.get("c").unwrap().is_some());
    assert_eq!(model.value("b", "right").unwrap(), Some(json!([])));
    assert!(model.is_set("b", "right").unwrap());
    model
        .reparent(
            ReparentRequest {
                object: "x",
                destination: None,
            },
            None,
            &mut QueryBudget::new(10000),
        )
        .unwrap();
    assert!(!model.is_set("b", "slot").unwrap());
    let after = model.get("x").unwrap().unwrap();
    assert_eq!(after.resource, original.resource);
    assert_eq!(after.type_identity, original.type_identity);
    assert!(
        model
            .query(Query::Value("slot", &json!("x")))
            .unwrap()
            .is_empty()
    );
    model.validate().unwrap();
}
#[test]
fn cycles_bounds_bad_endpoints_and_every_budget_failure_restore_the_graph() {
    let mut model = fixture();
    let before = storage::export(&model).unwrap();
    for req in [
        move_to("a", "x", "left", None),
        move_to("x", "x", "left", None),
        move_to("x", "b", "right", Some(usize::MAX)),
        move_to("x", "b", "slot", Some(0)),
        move_to("missing", "b", "left", None),
        move_to("x", "missing", "left", None),
        move_to("x", "b", "unknown", None),
    ] {
        assert!(
            model
                .reparent(req, None, &mut QueryBudget::new(10000))
                .is_err()
        );
        assert_eq!(storage::export(&model).unwrap(), before);
    }
    let mut budget = QueryBudget::new(10000);
    model
        .reparent(move_to("x", "b", "left", None), None, &mut budget)
        .unwrap();
    for steps in 0..budget.reads {
        let mut retry = fixture();
        assert!(
            retry
                .reparent(
                    move_to("x", "b", "left", None),
                    None,
                    &mut QueryBudget::new(steps)
                )
                .is_err()
        );
        assert_eq!(storage::export(&retry).unwrap(), before, "budget {steps}");
    }
    let mut exact = fixture();
    exact
        .reparent(
            move_to("x", "b", "left", None),
            None,
            &mut QueryBudget::new(budget.reads),
        )
        .unwrap();
    assert_eq!(
        storage::export(&exact).unwrap(),
        storage::export(&model).unwrap()
    );
}
#[test]
fn moves_share_session_rollback_and_versioned_publication() {
    let mut managed = TransactionalModel::new(fixture());
    let before = storage::export(&managed).unwrap();
    let result: Result<(), String> = managed.transaction(|edit| {
        edit.reparent(
            move_to("x", "b", "left", None),
            None,
            &mut QueryBudget::new(10000),
        )?;
        Err("veto".into())
    });
    assert!(result.is_err());
    assert_eq!(storage::export(&managed).unwrap(), before);
    managed
        .transaction(|edit| {
            edit.reparent(
                move_to("x", "b", "left", None),
                None,
                &mut QueryBudget::new(10000),
            )
        })
        .unwrap();
    let mut versioned = VersionedModel::new(fixture()).unwrap();
    let old = versioned.snapshot().unwrap();
    let mut fork = versioned.fork(&old).unwrap();
    fork.reparent(
        move_to("x", "b", "left", None),
        None,
        &mut QueryBudget::new(10000),
    )
    .unwrap();
    versioned.commit(fork).unwrap();
    assert_eq!(storage::export(&old).unwrap(), before);
    assert_same_model_content(
        storage::export(&versioned.snapshot().unwrap()).unwrap(),
        &storage::export(&managed).unwrap(),
    );
}
#[test]
fn pinned_pilot_ownership_projection_matches_valid_moves_and_rejects_both_cycles() {
    let controls: Value = serde_json::from_str(include_str!(
        "../../../../fixtures/ecore-structure/mercurio-containment-moves.json"
    ))
    .unwrap();
    // Structural projection only: no full Pilot import or language algorithms.
    let mut b = SchemaBuilder::new(
        "pilot_ownership_projection",
        "urn:pilot:ownership-projection",
    );
    let element = b.class("Element").unwrap();
    element.abstract_type().unwrap();
    let relationship = b.class("Relationship").unwrap();
    relationship.extends(&element).unwrap();
    relationship.abstract_type().unwrap();
    for name in ["Package", "Class"] {
        b.class(name).unwrap().extends(&element).unwrap();
    }
    b.class("OwningMembership")
        .unwrap()
        .extends(&relationship)
        .unwrap();
    let owned = element
        .reference("ownedRelationship", &relationship)
        .many()
        .containment()
        .unwrap();
    let owner = relationship
        .reference("owningRelatedElement", &element)
        .optional()
        .unwrap();
    b.opposites(&owned, &owner).unwrap();
    let related = relationship
        .reference("ownedRelatedElement", &element)
        .many()
        .containment()
        .unwrap();
    let membership = element
        .reference("owningRelationship", &relationship)
        .optional()
        .unwrap();
    b.opposites(&related, &membership).unwrap();
    let mut model = BasicModel::new(b.build().unwrap().as_schema().clone()).unwrap();
    for node in controls["controls"]["initial"].as_array().unwrap() {
        model
            .create(
                node["id"].as_str().unwrap(),
                node["class"].as_str().unwrap(),
                "model",
            )
            .unwrap();
    }
    for node in controls["controls"]["initial"].as_array().unwrap() {
        for (name, value) in node["fields"].as_object().unwrap() {
            model
                .set(node["id"].as_str().unwrap(), name, value.clone())
                .unwrap();
        }
    }
    let mut matched = 0;
    let mut rejected = 0;
    for step in controls["controls"]["steps"].as_array().unwrap() {
        let before = storage::export(&model).unwrap();
        let result = model.reparent(
            move_to(
                step["child"].as_str().unwrap(),
                step["owner"].as_str().unwrap(),
                step["field"].as_str().unwrap(),
                None,
            ),
            None,
            // Includes newly observed inherited source/member preparation.
            &mut QueryBudget::new(100_000),
        );
        if step["recursive_destination"] == true {
            assert!(result.is_err());
            assert_eq!(storage::export(&model).unwrap(), before);
            rejected += 1;
            // The forward EMF list write accepted the invalid cycle: explicit disagreement.
            assert_eq!(step["accepted"], step["write_side"] == "forward");
            continue;
        }
        result.unwrap();
        for node in step["after"].as_array().unwrap() {
            let object = model.get(node["id"].as_str().unwrap()).unwrap().unwrap();
            assert_eq!(
                serde_json::to_value(object.values).unwrap(),
                node["fields"],
                "{step}"
            );
        }
        matched += 1;
    }
    assert_eq!((matched, rejected), (6, 2));
}

#[test]
fn readonly_unordered_derived_and_full_destinations_reject_without_mutation() {
    for (name, flag) in [
        ("left", "readonly"),
        ("right", "readonly"),
        ("right", "unordered"),
        ("right", "derived"),
    ] {
        let original = fixture();
        let mut schema = original.schema().clone();
        let field = schema
            .classes
            .get_mut("Node")
            .unwrap()
            .features
            .get_mut(name)
            .unwrap();
        match flag {
            "readonly" => field.changeable = false,
            "unordered" => field.ordered = false,
            "derived" => field.derived = true,
            _ => unreachable!(),
        }
        // A derived Many has no stored effective value, unlike stored defaults.
        let mut store = original.into_store();
        if flag == "derived" {
            use intersect_model::core::storage::ModelStore;
            storage::atomic(&mut store, |s| {
                for id in ["a", "b", "c", "x", "y"] {
                    s.set_value(id, "right", None)?;
                }
                Ok(())
            })
            .unwrap();
        }
        let mut model = BasicModel::from_store(schema, store).unwrap();
        let before = storage::export(&model).unwrap();
        assert!(
            model
                .reparent(
                    move_to("x", "b", "right", Some(0)),
                    None,
                    &mut QueryBudget::new(10000)
                )
                .is_err()
        );
        assert_eq!(storage::export(&model).unwrap(), before, "{name} {flag}");
        if flag == "unordered" {
            model
                .reparent(
                    move_to("x", "b", "right", None),
                    None,
                    &mut QueryBudget::new(10000),
                )
                .unwrap();
        }
    }
    let mut model = fixture();
    model.set("b", "right", json!(["a", "c"])).unwrap();
    let before = storage::export(&model).unwrap();
    assert!(
        model
            .reparent(
                move_to("x", "b", "right", None),
                None,
                &mut QueryBudget::new(10000)
            )
            .unwrap_err()
            .contains("upper bound")
    );
    assert_eq!(storage::export(&model).unwrap(), before);
}

#[test]
fn unrelated_same_name_containment_uses_each_owner_declaration() {
    use intersect_model::core::storage::{DeleteRequest, DeleteSelection, IncomingReferences};
    let mut builder = SchemaBuilder::new("owners", "urn:unrelated-owners");
    let value = builder.class("Value").unwrap();
    let other = builder.class("Other").unwrap();
    let left = builder.class("Left").unwrap();
    let right = builder.class("Right").unwrap();
    let wrong = builder.class("Wrong").unwrap();
    left.reference("children", &value)
        .many()
        .unsettable(true)
        .containment()
        .unwrap();
    right
        .reference("children", &value)
        .unsettable(true)
        .containment()
        .unwrap();
    wrong
        .reference("children", &other)
        .many()
        .containment()
        .unwrap();
    let schema = builder.build().unwrap().as_schema().clone();
    assert_ne!(
        schema
            .effective_feature("Left", "children")
            .unwrap()
            .identity,
        schema
            .effective_feature("Right", "children")
            .unwrap()
            .identity
    );
    let mut model = BasicModel::new(schema).unwrap();
    for (id, class) in [
        ("left", "Left"),
        ("right", "Right"),
        ("wrong", "Wrong"),
        ("a", "Value"),
        ("b", "Value"),
    ] {
        model.create(id, class, "owners.model").unwrap();
    }
    model.set("left", "children", json!(["a"])).unwrap();
    model.set("right", "children", json!("b")).unwrap();
    let before = storage::export(&model).unwrap();
    assert!(
        model
            .reparent(
                move_to("a", "wrong", "children", None),
                None,
                &mut QueryBudget::new(10_000)
            )
            .is_err()
    );
    assert_eq!(storage::export(&model).unwrap(), before);
    model
        .reparent(
            move_to("a", "right", "children", None),
            None,
            &mut QueryBudget::new(10_000),
        )
        .unwrap();
    assert_eq!(model.value("left", "children").unwrap(), Some(json!([])));
    assert_eq!(model.value("right", "children").unwrap(), Some(json!("a")));
    assert!(model.get("b").unwrap().is_some());
    assert!(model.is_set("left", "children").unwrap());
    model
        .delete_objects(
            DeleteRequest {
                roots: &["a"],
                selection: DeleteSelection::Objects,
                incoming: IncomingReferences::Reject,
            },
            None,
            &mut QueryBudget::new(10_000),
        )
        .unwrap();
    assert_eq!(model.value("right", "children").unwrap(), Some(json!(null)));
    assert!(!model.is_set("right", "children").unwrap());
    assert_eq!(model.value("wrong", "children").unwrap(), Some(json!([])));
    model.validate().unwrap();
}

#[test]
fn structural_context_edits_typed_imports_and_preserves_cross_resource_roots() {
    use intersect_model::core::{
        frozen::FrozenStore,
        storage::{DeleteRequest, DeleteSelection, IncomingReferences, StructuralContext},
    };
    let mut model = fixture();
    let original = storage::export(&model).unwrap();
    model.create("branch", "Node", "script.py").unwrap();
    model.create("child", "Node", "script.py").unwrap();
    let before = storage::export(&model).unwrap();
    let context = StructuralContext {
        schema: model.schema(),
        conversions: None,
    };
    let mut store = FrozenStore::from_materialized(model.schema(), &before).unwrap();
    assert_eq!(store.metamodel().schema(), model.schema());
    let mut budget = QueryBudget::new(10_000);
    assert!(
        context
            .reparent(
                &mut store,
                move_to("child", "a", "left", Some(0)),
                &mut budget,
            )
            .unwrap()
    );
    assert_eq!(store.metamodel().schema(), model.schema());
    assert_eq!(
        store.value("a", "left").unwrap(),
        Some(json!(["child", "x", "y"]))
    );
    assert_eq!(store.get("child").unwrap().unwrap().resource, "script.py");
    let moved = storage::export(&store).unwrap();
    assert_eq!(moved.resources["script.py"].roots.as_slice(), ["branch"]);
    storage::validate_draft(&store, model.schema()).unwrap();

    // A final-validation budget failure rolls the exact canonical resource
    // metadata back together with the graph writes.
    let mut retry = FrozenStore::from_materialized(model.schema(), &before).unwrap();
    assert!(
        context
            .reparent(
                &mut retry,
                move_to("child", "a", "left", Some(0)),
                &mut QueryBudget::new(budget.reads - 1),
            )
            .is_err()
    );
    assert_eq!(retry.metamodel().schema(), model.schema());
    assert_eq!(storage::export(&retry).unwrap(), before);

    let mut store = FrozenStore::from_materialized(model.schema(), &moved).unwrap();
    assert_eq!(store.metamodel().schema(), model.schema());
    context
        .reparent(
            &mut store,
            move_to("child", "branch", "left", None),
            &mut QueryBudget::new(10_000),
        )
        .unwrap();
    let nested = storage::export(&store).unwrap();
    assert_eq!(nested.resources["script.py"].roots.as_slice(), ["branch"]);
    assert_eq!(store.value("a", "left").unwrap(), Some(json!(["x", "y"])));
    assert_eq!(
        store.value("branch", "left").unwrap(),
        Some(json!(["child"]))
    );

    // Deletion also works over a separately imported snapshot.
    let mut store = FrozenStore::from_materialized(model.schema(), &nested).unwrap();
    assert_eq!(store.metamodel().schema(), model.schema());
    let request = DeleteRequest {
        roots: &["branch"],
        selection: DeleteSelection::Subtrees,
        incoming: IncomingReferences::Reject,
    };
    let mut budget = QueryBudget::new(10_000);
    assert_eq!(
        context.delete(&mut store, request, &mut budget).unwrap(),
        ["branch", "child"]
    );
    let deleted = storage::export(&store).unwrap();
    assert!(deleted.resources["script.py"].roots.is_empty());
    assert!(store.get("branch").unwrap().is_none());
    assert!(store.get("child").unwrap().is_none());
    for element in &original.elements {
        assert_eq!(store.get(&element.id).unwrap().as_ref(), Some(element));
    }
    storage::validate_draft(&store, model.schema()).unwrap();

    let mut retry = FrozenStore::from_materialized(model.schema(), &nested).unwrap();
    assert!(
        context
            .delete(&mut retry, request, &mut QueryBudget::new(budget.reads - 1))
            .is_err()
    );
    assert_eq!(retry.metamodel().schema(), model.schema());
    assert_eq!(storage::export(&retry).unwrap(), nested);
}
