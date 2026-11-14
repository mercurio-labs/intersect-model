use intersect_model::core::{
    api::{BasicModel, ModelEdit, TransactionalModel, VersionedModel},
    frozen::FrozenStore,
    model::{AssignmentState, Model},
    query::QueryBudget,
    storage::{
        self, DeleteRequest, DeleteSelection, IncomingReferences, ModelRead, ModelStore, Query,
    },
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

// Include individually charged reference qualification in generous success runs.
const SUCCESS_WORK: usize = 10_000;

fn fixture(unsettable: bool, readonly: bool, required: bool) -> BasicModel {
    let mut builder = SchemaBuilder::new("deletion", "urn:deletion");
    let node = builder.class("Node").unwrap();
    let children = node
        .reference("children", &node)
        .many()
        .unsettable(unsettable)
        .containment()
        .unwrap();
    let parent = node
        .reference("parent", &node)
        .unsettable(unsettable)
        .optional()
        .unwrap();
    node.reference("link", &node)
        .unsettable(unsettable)
        .changeable(!readonly)
        .bounds(usize::from(required), 1)
        .build()
        .unwrap();
    builder.opposites(&children, &parent).unwrap();
    let schema = builder.build().unwrap().as_schema().clone();
    let mut store = FrozenStore::for_schema(&schema).unwrap();
    storage::atomic(&mut store, |s| {
        for id in ["a", "b", "c", "d", "e"] {
            s.create_element(
                &schema,
                id,
                "Node",
                if id == "e" { "two.xmi" } else { "one.xmi" },
                0,
                0,
            )?;
        }
        storage::set_with_opposite(s, &schema, "a", "children", json!(["b"]))?;
        storage::set_with_opposite(s, &schema, "b", "children", json!(["c"]))?;
        for (id, target) in [("d", "b"), ("e", "c")] {
            s.set_value(id, "link", Some(json!(target)))?;
            if unsettable {
                s.set_assignment_state(id, Some(AssignmentState::new(["link"])?))?;
            }
        }
        if required {
            for id in ["a", "b", "c"] {
                s.set_value(id, "link", Some(json!("a")))?;
            }
        }
        Ok(())
    })
    .unwrap();
    BasicModel::from_store(schema, store).unwrap()
}
fn request<'a>(
    roots: &'a [&'a str],
    selection: DeleteSelection,
    incoming: IncomingReferences,
) -> DeleteRequest<'a> {
    DeleteRequest {
        roots,
        selection,
        incoming,
    }
}
fn subtree() -> DeleteRequest<'static> {
    request(
        &["b"],
        DeleteSelection::Subtrees,
        IncomingReferences::Detach,
    )
}
fn observe(
    model: &impl ModelRead,
    schema: &intersect_model::definitions::schema::Schema,
    id: &str,
) -> Value {
    json!({"children":model.value(id,"children").unwrap(),"parent":model.value(id,"parent").unwrap(),"link":model.value(id,"link").unwrap(),"children_set":storage::is_set(model,schema,id,"children").unwrap(),"parent_set":storage::is_set(model,schema,id,"parent").unwrap(),"link_set":storage::is_set(model,schema,id,"link").unwrap(),"resource":model.get(id).unwrap().unwrap().resource})
}
#[test]
fn subtree_survivors_match_independent_emf_including_singular_unset_history() {
    let oracle: Value = serde_json::from_str(include_str!(
        "../../../../fixtures/ecore-structure/deletion.json"
    ))
    .unwrap();
    let mut checked = 0;
    for case in oracle["cases"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|c| c["operation"] == "delete-subtree")
    {
        let mut model = fixture(case["unsettable"] == true, false, false);
        let deleted = model
            .delete_objects(
                request(
                    &["b", "b"],
                    DeleteSelection::Subtrees,
                    IncomingReferences::Detach,
                ),
                None,
                &mut QueryBudget::new(SUCCESS_WORK),
            )
            .unwrap();
        assert_eq!(deleted, vec!["b", "c"]);
        assert!(model.get("b").unwrap().is_none());
        assert!(model.get("c").unwrap().is_none());
        for id in ["a", "d", "e"] {
            assert_eq!(observe(&model, model.schema(), id), case["objects"][id]);
            checked += 1;
        }
        model.validate().unwrap();
        assert!(
            model
                .query(Query::Value("link", &json!("b")))
                .unwrap()
                .is_empty()
        );
    }
    assert_eq!(checked, 6);
}
#[test]
fn explicit_selection_incoming_policy_and_readonly_failures_are_atomic() {
    let mut model = fixture(true, false, false);
    let before = storage::export(&model).unwrap();
    for req in [
        request(&["b"], DeleteSelection::Objects, IncomingReferences::Detach),
        request(
            &["b"],
            DeleteSelection::Subtrees,
            IncomingReferences::Reject,
        ),
        request(
            &["missing"],
            DeleteSelection::Objects,
            IncomingReferences::Detach,
        ),
    ] {
        assert!(
            model
                .delete_objects(req, None, &mut QueryBudget::new(SUCCESS_WORK))
                .is_err()
        );
        assert_eq!(storage::export(&model).unwrap(), before);
    }
    model
        .delete_objects(
            request(&["c"], DeleteSelection::Objects, IncomingReferences::Detach),
            None,
            &mut QueryBudget::new(SUCCESS_WORK),
        )
        .unwrap();
    assert_eq!(model.value("d", "link").unwrap(), Some(json!("b")));
    assert!(!model.is_set("e", "link").unwrap());
    assert!(model.is_set("b", "children").unwrap());
    let mut readonly = fixture(false, true, false);
    let before = storage::export(&readonly).unwrap();
    assert!(
        readonly
            .delete_objects(subtree(), None, &mut QueryBudget::new(SUCCESS_WORK))
            .unwrap_err()
            .contains("non-changeable")
    );
    assert_eq!(storage::export(&readonly).unwrap(), before);
}
#[test]
fn every_budget_failure_restores_values_state_and_indexes() {
    let mut complete = fixture(true, false, false);
    let mut budget = QueryBudget::new(SUCCESS_WORK);
    complete
        .delete_objects(subtree(), None, &mut budget)
        .unwrap();
    for limit in 0..budget.reads {
        let mut model = fixture(true, false, false);
        let before = storage::export(&model).unwrap();
        assert!(
            model
                .delete_objects(subtree(), None, &mut QueryBudget::new(limit))
                .is_err(),
            "{limit}"
        );
        assert_eq!(storage::export(&model).unwrap(), before, "{limit}");
        assert_eq!(
            model.query(Query::Value("link", &json!("b"))).unwrap(),
            vec!["d"]
        );
    }
    let mut exact = fixture(true, false, false);
    exact
        .delete_objects(subtree(), None, &mut QueryBudget::new(budget.reads))
        .unwrap();
    assert_eq!(
        storage::export(&exact).unwrap(),
        storage::export(&complete).unwrap()
    );
}
#[test]
fn sessions_and_forks_share_deletion_with_rollback_and_snapshot_isolation() {
    let basic = fixture(true, false, false);
    let before = storage::export(&basic).unwrap();
    let mut managed = TransactionalModel::new(basic);
    let result: Result<(), String> = managed.transaction(|edit| {
        edit.delete_objects(subtree(), None, &mut QueryBudget::new(SUCCESS_WORK))?;
        Err("veto".into())
    });
    assert!(result.is_err());
    assert_eq!(storage::export(&managed).unwrap(), before);
    managed
        .transaction(|edit| {
            edit.delete_objects(subtree(), None, &mut QueryBudget::new(SUCCESS_WORK))
        })
        .unwrap();
    let expected = storage::export(&managed).unwrap();
    let mut versioned = VersionedModel::new(fixture(true, false, false)).unwrap();
    let original = versioned.snapshot().unwrap();
    let mut candidate = versioned.fork(&original).unwrap();
    let mut stale = versioned.fork(&original).unwrap();
    candidate
        .delete_objects(subtree(), None, &mut QueryBudget::new(SUCCESS_WORK))
        .unwrap();
    versioned.commit(candidate).unwrap();
    let published = storage::export(&versioned.snapshot().unwrap()).unwrap();
    assert_same_model_content(published.clone(), &expected);
    stale
        .delete_objects(subtree(), None, &mut QueryBudget::new(SUCCESS_WORK))
        .unwrap();
    assert!(versioned.commit(stale).is_err());
    assert_eq!(
        storage::export(&versioned.snapshot().unwrap()).unwrap(),
        published,
    );
    assert_eq!(storage::export(&original).unwrap(), before);
}
#[test]
fn required_survivors_can_be_drafts_but_cannot_publish_incomplete() {
    let mut basic = fixture(false, false, true);
    basic
        .delete_objects(subtree(), None, &mut QueryBudget::new(SUCCESS_WORK))
        .unwrap();
    assert!(basic.validate().is_err());
    let mut managed = TransactionalModel::new(fixture(false, false, true));
    let before = storage::export(&managed).unwrap();
    assert!(
        managed
            .transaction(|edit| edit.delete_objects(
                subtree(),
                None,
                &mut QueryBudget::new(SUCCESS_WORK)
            ))
            .is_err()
    );
    assert_eq!(storage::export(&managed).unwrap(), before);
    managed
        .transaction(|edit| {
            edit.delete_objects(subtree(), None, &mut QueryBudget::new(SUCCESS_WORK))?;
            edit.set("d", "link", json!("a"))?;
            edit.set("e", "link", json!("a"))
        })
        .unwrap();
    managed.validate().unwrap();
}
