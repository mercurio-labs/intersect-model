#![cfg(not(target_arch = "wasm32"))]
use intersect_model::core::{
    api::{BasicModel, ModelEdit, TransactionalModel, VersionedModel},
    frozen::FrozenStore,
    query::QueryBudget,
    storage::{
        self, ContainmentDestination, DeleteRequest, DeleteSelection, IncomingReferences,
        IndexPolicy, ModelRead, ModelStore, ReparentRequest,
    },
};
use intersect_model::definitions::{conversion::ConversionRegistry, schema::Schema};
use intersect_model::navigation::{IncomingCursor, IncomingStep};
use intersect_model::sqlite::SqliteStore;
use serde_json::json;

fn schema() -> Schema {
    intersect_model_acceptance::migration_fixture::compile_migrated_schema_fixture(include_str!(
        "../../../../fixtures/ecore-structure/deletion.ecore"
    ))
    .unwrap()
}

fn incoming(read: &dyn ModelRead, schema: &Schema, target: &str) -> Result<Vec<String>, String> {
    let member = schema.effective_feature("Node", "children")?.identity;
    let mut cursor = IncomingCursor::new(read, schema, "Node", member, target)?;
    let mut result = Vec::new();
    for _ in 0..10000 {
        match cursor.advance(read, schema)? {
            IncomingStep::Pending => {}
            IncomingStep::Match(id) => result.push(id),
            IncomingStep::Done => return Ok(result),
        }
    }
    Err("inverse cursor did not terminate".into())
}

fn check(
    read: &dyn ModelRead,
    schema: &Schema,
    target: &str,
    expected: &[&str],
    parent: Option<&str>,
) {
    let actual = incoming(read, schema, target).unwrap();
    assert_eq!(actual, expected);
    // Independent forward enumeration over the exported records, with neither
    // index queries nor typed projections used to compute the expected answer.
    let model = storage::export(read).unwrap();
    let scanned = model
        .elements
        .iter()
        .filter(|element| {
            element.class == "Node"
                && element
                    .values
                    .get("children")
                    .and_then(|value| value.as_array())
                    .is_some_and(|values| values.iter().any(|value| value.as_str() == Some(target)))
        })
        .map(|element| element.id.clone())
        .collect::<Vec<_>>();
    assert_eq!(actual, scanned);
    let ownership =
        storage::validate_ownership_with_conversions(read, schema, &ConversionRegistry::default())
            .unwrap();
    assert_eq!(ownership.parent(target), parent);
}

fn seed<S: ModelStore>(schema: &Schema, store: S) -> BasicModel<S> {
    let mut model = BasicModel::from_store(schema.clone(), store).unwrap();
    for (id, resource) in [("a", "one"), ("b", "one"), ("c", "two"), ("d", "two")] {
        model.create(id, "Node", resource).unwrap();
    }
    model
}

fn move_to(edit: &mut impl ModelEdit, object: &str, owner: &str) -> Result<(), String> {
    edit.reparent(
        ReparentRequest {
            object,
            destination: Some(ContainmentDestination {
                owner,
                feature: "children",
                position: None,
            }),
        },
        None,
        &mut QueryBudget::new(10_000),
    )?;
    Ok(())
}

fn delete(edit: &mut impl ModelEdit, object: &str) -> Result<(), String> {
    edit.delete_objects(
        DeleteRequest {
            roots: &[object],
            selection: DeleteSelection::Objects,
            incoming: IncomingReferences::Detach,
        },
        None,
        &mut QueryBudget::new(10_000),
    )?;
    Ok(())
}

fn exercise<S: ModelStore>(schema: &Schema, store: S) -> S {
    let mut basic = seed(schema, store);
    // A previously empty answer must become positive after editing.
    check(&basic, schema, "c", &[], None);
    basic.set("a", "children", json!(["b"])).unwrap();
    basic.set("b", "children", json!(["c"])).unwrap();
    basic.set("d", "link", json!("c")).unwrap();
    check(&basic, schema, "c", &["b"], Some("b"));
    // Replacing the stored Many value removes the old posting and ownership.
    basic.set("b", "children", json!([])).unwrap();
    check(&basic, schema, "c", &[], None);
    basic.set("d", "children", json!(["c"])).unwrap();
    check(&basic, schema, "c", &["d"], Some("d"));
    basic.create("e", "Node", "three").unwrap();
    check(&basic, schema, "e", &[], None);
    basic.set("a", "children", json!(["b", "e"])).unwrap();
    check(&basic, schema, "e", &["a"], Some("a"));

    let before = storage::export(&basic).unwrap();
    let mut model = TransactionalModel::new(basic);
    let rejected: Result<(), String> = model.transaction(|edit| {
        move_to(edit, "c", "a")?;
        check(edit, schema, "c", &["a"], Some("a"));
        delete(edit, "e")?;
        assert_eq!(
            incoming(edit, schema, "e").unwrap_err(),
            "Missing inverse query target"
        );
        Err("navigation rollback control".into())
    });
    assert_eq!(rejected.unwrap_err(), "navigation rollback control");
    assert_eq!(storage::export(&model).unwrap(), before);
    check(&model, schema, "c", &["d"], Some("d"));
    check(&model, schema, "e", &["a"], Some("a"));
    model.transaction(|edit| move_to(edit, "c", "a")).unwrap();
    check(&model, schema, "c", &["a"], Some("a"));
    model.transaction(|edit| delete(edit, "c")).unwrap();
    assert_eq!(
        incoming(&model, schema, "c").unwrap_err(),
        "Missing inverse query target"
    );
    assert_eq!(model.value("d", "link").unwrap(), Some(json!(null)));
    // Reusing the external spelling must not revive a removed inverse answer.
    model
        .transaction(|edit| edit.create("c", "Node", "recreated"))
        .unwrap();
    check(&model, schema, "c", &[], None);
    check(&model, schema, "b", &["a"], Some("a"));
    model.into_basic().into_store()
}

#[test]
fn navigation_tracks_frozen_indexed_unindexed_and_sqlite_transaction_transitions() {
    let schema = schema();
    let indexed = FrozenStore::for_schema(&schema).unwrap();
    let unindexed = FrozenStore::with_index_policy(
        indexed.metamodel().clone(),
        IndexPolicy::selected(Vec::<String>::new()),
    );
    let indexed = exercise(&schema, indexed);
    let unindexed = exercise(&schema, unindexed);
    let sqlite = SqliteStore::in_memory().unwrap();
    let sqlite = exercise(&schema, sqlite);
    assert_eq!(
        storage::export(&indexed).unwrap(),
        storage::export(&unindexed).unwrap()
    );
    assert_eq!(
        semantic_model(storage::export(&indexed).unwrap()),
        semantic_model(storage::export(&sqlite).unwrap())
    );
}

#[test]
fn navigation_reopens_with_committed_inverse_and_ownership_answers() {
    let schema = schema();
    let path = std::env::temp_dir().join(format!(
        "intersect-s09-navigation-coherence-{}.sqlite",
        std::process::id()
    ));
    assert!(!path.exists(), "navigation test path already exists");
    let sqlite = SqliteStore::open(&path).unwrap();
    let sqlite = exercise(&schema, sqlite);
    let expected = storage::export(&sqlite).unwrap();
    drop(sqlite);
    let reopened =
        BasicModel::from_store(schema.clone(), SqliteStore::open(&path).unwrap()).unwrap();
    assert_eq!(storage::export(&reopened).unwrap(), expected);
    check(&reopened, &schema, "c", &[], None);
    check(&reopened, &schema, "e", &["a"], Some("a"));
    check(&reopened, &schema, "b", &["a"], Some("a"));
    drop(reopened);
    std::fs::remove_file(path).unwrap();
}

#[test]
fn navigation_pins_old_snapshots_and_discards_rejected_fork_answers() {
    let schema = schema();
    let mut basic = seed(&schema, FrozenStore::for_schema(&schema).unwrap());
    basic.set("a", "children", json!(["b"])).unwrap();
    basic.set("b", "children", json!(["c"])).unwrap();
    basic.set("d", "link", json!("c")).unwrap();
    let mut model = VersionedModel::new(basic).unwrap();
    let before = model.snapshot().unwrap();
    let mut rejected = model.fork(&before).unwrap();
    move_to(&mut rejected, "c", "d").unwrap();
    check(&rejected, &schema, "c", &["d"], Some("d"));
    check(&before, &schema, "c", &["b"], Some("b"));
    assert!(
        model
            .commit_with(rejected, |_| Err("fork veto".into()))
            .is_err()
    );
    check(&model.snapshot().unwrap(), &schema, "c", &["b"], Some("b"));

    let mut accepted = model.fork(&before).unwrap();
    move_to(&mut accepted, "c", "d").unwrap();
    check(&accepted, &schema, "c", &["d"], Some("d"));
    delete(&mut accepted, "c").unwrap();
    assert_eq!(
        incoming(&accepted, &schema, "c").unwrap_err(),
        "Missing inverse query target"
    );
    accepted.create("c", "Node", "recreated").unwrap();
    check(&accepted, &schema, "c", &[], None);
    model.commit(accepted).unwrap();
    let after = model.snapshot().unwrap();
    check(&after, &schema, "c", &[], None);
    check(&before, &schema, "c", &["b"], Some("b"));
    assert_eq!(after.value("d", "link").unwrap(), Some(json!(null)));
    assert_eq!(before.value("d", "link").unwrap(), Some(json!("c")));
}

// Resource content revisions are adapter-local deterministic tokens, not a
// portable edit counter. Preserve every other field in semantic comparisons.
fn semantic_model(
    mut model: intersect_model::core::model::Model,
) -> intersect_model::core::model::Model {
    for record in model.resources.values_mut() {
        record.content_revision = 0;
    }
    model
}
