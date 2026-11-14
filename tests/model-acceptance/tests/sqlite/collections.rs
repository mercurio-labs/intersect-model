#![cfg(not(target_arch = "wasm32"))]
use intersect_model::core::{
    model::Element,
    storage::{self, ModelRead, ModelStore, Query},
};
use intersect_model::sqlite::SqliteStore;
use serde_json::json;

fn record() -> Element {
    Element {
        implicit: false,
        initialization: None,
        assignment_state: None,
        id: "source#stable".into(),
        class: "Raw".into(),
        type_identity: "urn:raw#Raw".into(),
        resource: "source".into(),
        start: 1,
        end: 99,
        values: serde_json::from_value(
            json!({"items":["a","b","a"],"null":null,"mixed":[true,{"x":1},[2]],"default":0}),
        )
        .unwrap(),
    }
}

#[test]
fn collection_edits_preserve_duplicates_sparse_fields_and_nested_rollback() {
    let mut store = SqliteStore::in_memory().unwrap();
    storage::atomic(&mut store, |s| s.put(record())).unwrap();
    let before = storage::export(&store).unwrap();
    let outer = store.begin().unwrap();
    store
        .append_value("source#stable", "items", json!("c"))
        .unwrap();
    assert_eq!(
        store
            .collection_page("source#stable", "items", 1, 2)
            .unwrap(),
        vec![json!("b"), json!("a")]
    );
    assert_eq!(store.value_len("source#stable", "items").unwrap(), Some(4));
    let inner = store.begin().unwrap();
    store.truncate_values("source#stable", "items", 1).unwrap();
    assert_eq!(
        store.query(Query::Value("items", &json!("a"))).unwrap(),
        ["source#stable"]
    );
    assert!(
        store
            .query(Query::Value("items", &json!("b")))
            .unwrap()
            .is_empty()
    );
    store.set_value("source#stable", "null", None).unwrap();
    assert_eq!(store.value("source#stable", "null").unwrap(), None);
    assert_eq!(
        store.value("source#stable", "default").unwrap(),
        Some(json!(0))
    );
    store.commit(&inner).unwrap();
    store.rollback(&outer).unwrap();
    assert_eq!(storage::export(&store).unwrap(), before);
    assert_eq!(
        store.value("source#stable", "null").unwrap(),
        Some(json!(null))
    );
    assert_eq!(store.value("source#stable", "absent").unwrap(), None);
}

#[test]
fn append_does_not_rewrite_parent_or_other_fields() {
    let path = std::env::temp_dir().join(format!("intersect-field-{}.sqlite", std::process::id()));
    {
        let mut store = SqliteStore::open(&path).unwrap();
        storage::atomic(&mut store, |s| s.put(record())).unwrap();
        let connection = rusqlite::Connection::open(&path).unwrap();
        connection.execute_batch("CREATE TRIGGER fail_parent_update BEFORE UPDATE ON intersect_elements BEGIN SELECT RAISE(ABORT,'parent rewritten'); END;").unwrap();
        storage::atomic(&mut store, |s| {
            s.append_value("source#stable", "items", json!("next"))?;
            s.truncate_values("source#stable", "items", 2)?;
            s.set_value("source#stable", "default", Some(json!(42)))
        })
        .unwrap();
        assert_eq!(
            store.value("source#stable", "items").unwrap(),
            Some(json!(["a", "b"]))
        );
        assert_eq!(store.get("source#stable").unwrap().unwrap().end, 99);
    }
    std::fs::remove_file(path).unwrap();
}

fn version_one(path: &std::path::Path, payload: &str) {
    let connection = rusqlite::Connection::open(path).unwrap();
    connection.execute_batch("CREATE TABLE intersect_metadata (key TEXT PRIMARY KEY,value TEXT NOT NULL); INSERT INTO intersect_metadata VALUES ('revision','schema-revision'); CREATE TABLE intersect_elements (seq INTEGER PRIMARY KEY AUTOINCREMENT,id TEXT NOT NULL UNIQUE,payload TEXT NOT NULL); PRAGMA user_version=1;").unwrap();
    connection
        .execute(
            "INSERT INTO intersect_elements VALUES (7,'source#stable',?1)",
            [payload],
        )
        .unwrap();
}

#[test]
fn older_formats_reject_without_modifying_the_file() {
    for version in 1..=4 {
        let path = std::env::temp_dir().join(format!(
            "intersect-rejected-format-{version}-{}.sqlite",
            std::process::id()
        ));
        assert!(!path.exists());
        let payload = serde_json::to_string(&record()).unwrap();
        version_one(&path, &payload);
        let connection = rusqlite::Connection::open(&path).unwrap();
        connection
            .pragma_update(None, "user_version", version)
            .unwrap();
        drop(connection);
        assert!(
            SqliteStore::open(&path)
                .err()
                .unwrap()
                .contains("expected format 5")
        );
        let connection = rusqlite::Connection::open(&path).unwrap();
        assert_eq!(
            connection
                .query_row("PRAGMA user_version", [], |r| r.get::<_, i64>(0))
                .unwrap(),
            version
        );
        assert_eq!(
            connection
                .query_row("SELECT payload FROM intersect_elements", [], |r| r
                    .get::<_, String>(0))
                .unwrap(),
            payload
        );
        drop(connection);
        std::fs::remove_file(path).unwrap();
    }
}

#[test]
fn rejected_invalid_old_payload_leaves_file_unchanged() {
    let path = std::env::temp_dir().join(format!(
        "intersect-upgrade-bad-{}.sqlite",
        std::process::id()
    ));
    version_one(&path, "{invalid");
    assert!(SqliteStore::open(&path).is_err());
    let connection = rusqlite::Connection::open(&path).unwrap();
    assert_eq!(
        connection
            .query_row("PRAGMA user_version", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        1
    );
    assert_eq!(
        connection
            .query_row("SELECT payload FROM intersect_elements", [], |r| r
                .get::<_, String>(0))
            .unwrap(),
        "{invalid"
    );
    assert_eq!(
        connection
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE name='intersect_fields'",
                [],
                |r| r.get::<_, i64>(0)
            )
            .unwrap(),
        0
    );
    drop(connection);
    std::fs::remove_file(path).unwrap();
}
