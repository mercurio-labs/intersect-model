#![cfg(not(target_arch = "wasm32"))]
use intersect_model::core::{
    api::{BasicModel, ModelEdit, TransactionalModel},
    frozen::FrozenStore,
    query::QueryBudget,
    storage::{
        self, DeleteRequest, DeleteSelection, IncomingReferences, ModelRead, ModelStore, Query,
    },
};
use intersect_model::definitions::schema::Schema;
use intersect_model::sqlite::SqliteStore;
use serde_json::json;

// Include individually charged reference qualification in generous success runs.
const SUCCESS_WORK: usize = 10_000;
fn schema() -> Schema {
    intersect_model_acceptance::migration_fixture::compile_migrated_schema_fixture(include_str!(
        "../../../../fixtures/ecore-structure/deletion.ecore"
    ))
    .unwrap()
}

fn exercise(schema: &Schema, store: impl ModelStore) -> intersect_model::core::model::Model {
    let mut basic = BasicModel::from_store(schema.clone(), store).unwrap();
    for id in ["a", "b", "c", "d"] {
        basic.create(id, "Node", "model.xmi").unwrap();
    }
    basic.set("a", "children", json!(["b"])).unwrap();
    basic.set("b", "children", json!(["c"])).unwrap();
    basic.set("d", "link", json!("c")).unwrap();
    let before = storage::export(&basic).unwrap();
    let mut managed = TransactionalModel::new(basic);
    let request = DeleteRequest {
        roots: &["b"],
        selection: DeleteSelection::Subtrees,
        incoming: IncomingReferences::Detach,
    };
    let failed: Result<(), String> = managed.transaction(|edit| {
        edit.delete_objects(request, None, &mut QueryBudget::new(SUCCESS_WORK))?;
        assert!(edit.get("b")?.is_none());
        Err("outer veto".into())
    });
    assert!(failed.is_err());
    assert_eq!(storage::export(&managed).unwrap(), before);
    assert_eq!(
        managed.query(Query::Value("link", &json!("c"))).unwrap(),
        vec!["d"]
    );
    managed
        .transaction(|edit| edit.delete_objects(request, None, &mut QueryBudget::new(SUCCESS_WORK)))
        .unwrap();
    assert_eq!(managed.query(Query::Class("Node")).unwrap(), vec!["a", "d"]);
    assert!(
        managed
            .query(Query::Value("link", &json!("c")))
            .unwrap()
            .is_empty()
    );
    assert_eq!(managed.value("a", "children").unwrap(), Some(json!([])));
    let basic = managed.into_basic();
    assert!(basic.is_set("a", "children").unwrap());
    assert!(!basic.is_set("d", "link").unwrap());
    basic.validate().unwrap();
    storage::export(&basic).unwrap()
}
#[test]
fn sqlite_and_frozen_share_deletion_indexes_history_and_outer_rollback() {
    let schema = schema();
    let sqlite = SqliteStore::in_memory().unwrap();
    let frozen = FrozenStore::for_schema(&schema).unwrap();
    assert_eq!(
        semantic_model(exercise(&schema, sqlite)),
        semantic_model(exercise(&schema, frozen))
    );
}

#[test]
fn sqlite_and_frozen_share_explicit_containment_transfer_and_rollback() {
    use intersect_model::core::storage::{ContainmentDestination, ReparentRequest};
    fn exercise_move(
        schema: &Schema,
        store: impl ModelStore,
    ) -> intersect_model::core::model::Model {
        let mut basic = BasicModel::from_store(schema.clone(), store).unwrap();
        for id in ["a", "b", "c", "d"] {
            basic.create(id, "Node", "model.xmi").unwrap();
        }
        basic.set("a", "children", json!(["b"])).unwrap();
        basic.set("b", "children", json!(["c"])).unwrap();
        let before = storage::export(&basic).unwrap();
        let mut managed = TransactionalModel::new(basic);
        let transfer = ReparentRequest {
            object: "c",
            destination: Some(ContainmentDestination {
                owner: "d",
                feature: "children",
                position: None,
            }),
        };
        let failed: Result<(), String> = managed.transaction(|edit| {
            edit.reparent(transfer, None, &mut QueryBudget::new(SUCCESS_WORK))?;
            Err("veto".into())
        });
        assert!(failed.is_err());
        assert_eq!(storage::export(&managed).unwrap(), before);
        managed
            .transaction(|edit| edit.reparent(transfer, None, &mut QueryBudget::new(SUCCESS_WORK)))
            .unwrap();
        managed
            .transaction(|edit| {
                edit.reparent(
                    ReparentRequest {
                        object: "b",
                        destination: Some(ContainmentDestination {
                            owner: "d",
                            feature: "children",
                            position: Some(0),
                        }),
                    },
                    None,
                    &mut QueryBudget::new(SUCCESS_WORK),
                )
            })
            .unwrap();
        assert_eq!(
            managed.value("d", "children").unwrap(),
            Some(json!(["b", "c"]))
        );
        assert_eq!(
            managed.query(Query::Value("parent", &json!("d"))).unwrap(),
            vec!["b", "c"]
        );
        managed.validate().unwrap();
        storage::export(&managed).unwrap()
    }
    let schema = schema();
    let sqlite = SqliteStore::in_memory().unwrap();
    let frozen = FrozenStore::for_schema(&schema).unwrap();
    assert_eq!(
        semantic_model(exercise_move(&schema, sqlite)),
        semantic_model(exercise_move(&schema, frozen))
    );
}

#[test]
fn durable_structural_edits_reopen_with_receipts_history_and_independent_indexes() {
    use intersect_model::core::storage::{ContainmentDestination, ReparentRequest};
    use intersect_model::definitions::{
        bundle::{DefinitionSource, SourceKind},
        conversion::ProviderId,
        initialization::{ClassInitializationContract, InitializationReceipt},
    };
    let schema = schema();
    let mut seed = BasicModel::new(schema.clone()).unwrap();
    for (id, resource) in [
        ("a", "one.xmi"),
        ("b", "one.xmi"),
        ("c", "one.xmi"),
        ("d", "two.xmi"),
    ] {
        seed.create(id, "Node", resource).unwrap();
    }
    seed.set("a", "children", json!(["b"])).unwrap();
    seed.set("b", "children", json!(["c"])).unwrap();
    seed.set("a", "link", json!("c")).unwrap();
    let mut original = storage::export(&seed).unwrap();
    let source = DefinitionSource::new(
        "test:retained-construction",
        SourceKind::RcoreBehavior,
        "no-op",
    );
    for element in &mut original.elements {
        element.start = 10;
        element.end = 20;
        element.initialization = Some(InitializationReceipt {
            object: element.id.clone(),
            authored_members: vec![],
            contract: ClassInitializationContract {
                version: 1,
                schema_revision: schema.revision.clone(),
                class: "Node".into(),
                class_identity: schema.identity("Node"),
                provider: ProviderId {
                    name: "retained-test".into(),
                    version: "1".into(),
                },
                sources: vec![source.id.clone()],
                members: vec![],
            },
        });
    }
    let path = std::env::temp_dir().join(format!(
        "intersect-e10-structural-{}.db",
        std::process::id()
    ));
    assert!(!path.exists());
    let mut sqlite = SqliteStore::open(&path).unwrap();
    storage::atomic(&mut sqlite, |store| {
        store.set_revision(&schema.revision)?;
        for element in &original.elements {
            store.put(element.clone())?;
        }
        for record in original.resources.values() {
            store.set_resource_record(record.clone())?;
        }
        Ok(())
    })
    .unwrap();
    let mut disk = TransactionalModel::new(BasicModel::from_store(schema.clone(), sqlite).unwrap());
    let frozen = FrozenStore::from_materialized(&schema, &original).unwrap();
    let mut frozen =
        TransactionalModel::new(BasicModel::from_store(schema.clone(), frozen).unwrap());
    fn edits(edit: &mut impl ModelEdit) -> Result<(), String> {
        edit.reparent(
            ReparentRequest {
                object: "b",
                destination: Some(ContainmentDestination {
                    owner: "d",
                    feature: "children",
                    position: None,
                }),
            },
            None,
            &mut QueryBudget::new(10_000),
        )?;
        edit.delete_objects(
            DeleteRequest {
                roots: &["c"],
                selection: DeleteSelection::Objects,
                incoming: IncomingReferences::Detach,
            },
            None,
            &mut QueryBudget::new(10_000),
        )?;
        Ok(())
    }
    assert!(
        disk.transaction(|edit| {
            edits(edit)?;
            Err::<(), String>("outer veto".into())
        })
        .is_err()
    );
    assert_eq!(storage::export(&disk).unwrap(), original);
    drop(disk);
    let mut disk = TransactionalModel::new(
        BasicModel::from_store(schema.clone(), SqliteStore::open(&path).unwrap()).unwrap(),
    );
    assert_eq!(storage::export(&disk).unwrap(), original);
    disk.transaction(|edit| edits(edit)).unwrap();
    frozen.transaction(|edit| edits(edit)).unwrap();
    let expected = storage::export(&frozen).unwrap();
    let disk_expected = storage::export(&disk).unwrap();
    assert_eq!(
        semantic_model(disk_expected.clone()),
        semantic_model(expected.clone())
    );
    drop(disk);
    let disk = BasicModel::from_store(schema, SqliteStore::open(&path).unwrap()).unwrap();
    assert_eq!(storage::export(&disk).unwrap(), disk_expected);
    let restored = FrozenStore::from_materialized(disk.schema(), &disk_expected).unwrap();
    assert_eq!(restored.into_model().unwrap(), disk_expected);
    assert!(disk.get("c").unwrap().is_none());
    assert!(!disk.is_set("a", "link").unwrap());
    assert!(disk.is_set("b", "children").unwrap());
    for object in &expected.elements {
        let retained = original
            .elements
            .iter()
            .find(|e| e.id == object.id)
            .unwrap();
        assert_eq!(object.initialization, retained.initialization);
        assert_eq!(
            (&object.resource, object.start, object.end),
            (&retained.resource, 10, 20)
        );
        let by_resource = expected
            .elements
            .iter()
            .filter(|e| e.resource == object.resource)
            .map(|e| e.id.clone())
            .collect::<Vec<_>>();
        assert_eq!(
            disk.query(Query::Resource(&object.resource)).unwrap(),
            by_resource
        );
        for (feature, value) in &object.values {
            let targets = value
                .as_array()
                .map(|v| v.as_slice())
                .unwrap_or(std::slice::from_ref(value));
            for target in targets.iter().filter(|v| !v.is_null()) {
                let matches = expected
                    .elements
                    .iter()
                    .filter(|e| {
                        let value = &e.values[feature];
                        value == target
                            || value.as_array().is_some_and(|items| items.contains(target))
                    })
                    .map(|e| e.id.clone())
                    .collect::<Vec<_>>();
                assert_eq!(disk.query(Query::Value(feature, target)).unwrap(), matches);
            }
        }
    }
    assert_eq!(
        disk.query(Query::Class("Node")).unwrap(),
        vec!["a", "b", "d"]
    );
    assert!(
        disk.query(Query::Value("link", &json!("c")))
            .unwrap()
            .is_empty()
    );
    disk.validate().unwrap();
    drop(disk);
    std::fs::remove_file(path).unwrap();
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
