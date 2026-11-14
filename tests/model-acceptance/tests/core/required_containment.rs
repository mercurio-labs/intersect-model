use intersect_model::core::{
    api::{BasicModel, ModelEdit, TransactionalModel, VersionedModel},
    model::Model,
    query::QueryBudget,
    storage::{self, ContainmentDestination, ModelRead, ReparentRequest},
};
use intersect_model::definitions::dynamic::SchemaBuilder;
use serde_json::json;

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

fn fixture(many: bool) -> BasicModel {
    let mut builder = SchemaBuilder::new("required", "urn:required-containment");
    let child = builder.class("Child").unwrap();
    let owner = builder.class("Owner").unwrap();
    owner
        .reference("children", &child)
        .bounds(1, if many { 2 } else { 1 })
        .unsettable(true)
        .containment()
        .unwrap();
    let mut model = BasicModel::new(builder.build().unwrap().as_schema().clone()).unwrap();
    for (id, class) in [
        ("a", "Owner"),
        ("b", "Owner"),
        ("x", "Child"),
        ("y", "Child"),
    ] {
        model.create(id, class, "required.model").unwrap();
    }
    model
        .set(
            "a",
            "children",
            if many { json!(["x"]) } else { json!("x") },
        )
        .unwrap();
    model
        .set(
            "b",
            "children",
            if many { json!(["y"]) } else { json!("y") },
        )
        .unwrap();
    model.validate().unwrap();
    model
}
fn move_to(model: &mut impl ModelEdit, child: &str, owner: &str) -> Result<bool, String> {
    model.reparent(
        ReparentRequest {
            object: child,
            destination: Some(ContainmentDestination {
                owner,
                feature: "children",
                position: None,
            }),
        },
        None,
        &mut QueryBudget::new(10_000),
    )
}
#[test]
fn required_source_containment_is_a_draft_obligation_and_a_publication_gate() {
    for many in [false, true] {
        let mut basic = fixture(many);
        move_to(&mut basic, "x", "b").unwrap();
        assert!(basic.validate().is_err());
        assert_eq!(basic.is_set("a", "children").unwrap(), many);
        move_to(&mut basic, "y", "a").unwrap();
        basic.validate().unwrap();
        let expected = storage::export(&basic).unwrap();
        let mut managed = TransactionalModel::new(fixture(many));
        let before = storage::export(&managed).unwrap();
        assert!(managed.transaction(|edit| move_to(edit, "x", "b")).is_err());
        assert_eq!(storage::export(&managed).unwrap(), before);
        managed
            .transaction(|edit| {
                move_to(edit, "x", "b")?;
                move_to(edit, "y", "a")?;
                Ok(())
            })
            .unwrap();
        assert_same_model_content(storage::export(&managed).unwrap(), &expected);
        let mut versioned = VersionedModel::new(fixture(many)).unwrap();
        let snapshot = versioned.snapshot().unwrap();
        let mut rejected = versioned.fork(&snapshot).unwrap();
        move_to(&mut rejected, "x", "b").unwrap();
        assert!(versioned.commit(rejected).is_err());
        assert_eq!(
            storage::export(&versioned.snapshot().unwrap()).unwrap(),
            before
        );
        let mut repaired = versioned.fork(&snapshot).unwrap();
        move_to(&mut repaired, "x", "b").unwrap();
        move_to(&mut repaired, "y", "a").unwrap();
        versioned.commit(repaired).unwrap();
        assert_same_model_content(
            storage::export(&versioned.snapshot().unwrap()).unwrap(),
            &expected,
        );
        assert_eq!(storage::export(&snapshot).unwrap(), before);
        assert_eq!(
            basic.value("b", "children").unwrap(),
            Some(if many { json!(["x"]) } else { json!("x") })
        );
    }
}
