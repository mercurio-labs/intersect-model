//! Exact canonical-index counts; no language/schema validation claim.
use intersect_model::{
    core::{
        frozen::{FrozenStore, Metamodel},
        model::Element,
    },
    definitions::dynamic::{ScalarType, SchemaBuilder},
    storage::{self, IndexPolicy, ModelRead, ModelStore, Query, StorageResult},
};
use serde_json::json;
use std::{collections::BTreeMap, sync::Arc};

fn element(store: &FrozenStore, id: &str, resource: &str) -> Element {
    Element {
        implicit: false,
        initialization: None,
        assignment_state: None,
        id: id.into(),
        class: "CountControl".into(),
        type_identity: store.metamodel().schema().identity("CountControl"),
        resource: resource.into(),
        start: 0,
        end: 1,
        values: BTreeMap::from([("duplicates".into(), json!(["x", "x", "x"]))]),
    }
}
fn policies() -> [IndexPolicy; 2] {
    [
        IndexPolicy::AllValues,
        IndexPolicy::selected(Vec::<String>::new()),
    ]
}
fn parity(store: &FrozenStore, expected: &[(&str, usize)]) -> StorageResult<()> {
    let before = storage::export(store)?;
    for &(resource, count) in expected {
        let indexed = store.query(Query::Resource(resource))?;
        assert_eq!(indexed.len(), count);
        assert_eq!(
            indexed,
            before
                .elements
                .iter()
                .filter(|element| element.resource == resource)
                .map(|element| element.id.clone())
                .collect::<Vec<_>>()
        );
    }
    assert_eq!(
        storage::export(store)?,
        before,
        "count must not mutate rows, revision or resource metadata"
    );
    Ok(())
}
fn seeded(policy: IndexPolicy) -> StorageResult<FrozenStore> {
    let mut schema = SchemaBuilder::new("counts", "urn:count-control");
    schema
        .class("CountControl")?
        .attribute("duplicates", ScalarType::String)
        .many()
        .unique(false)
        .build()?;
    let schema = schema.build()?;
    let metamodel = Metamodel::freeze(schema.as_schema()).map_err(|error| error.to_string())?;
    let mut store = FrozenStore::with_index_policy(Arc::new(metamodel), policy);
    storage::atomic(&mut store, |s| {
        s.set_revision("count-source-v1")?;
        s.put(element(s, "a", "r1"))?;
        s.put(element(s, "b", "r1"))?;
        s.put(element(s, "c", "r2"))
    })?;
    Ok(store)
}
#[test]
fn exact_resource_membership_is_independent_of_value_index_policy() -> StorageResult<()> {
    for policy in policies() {
        let mut store = seeded(policy)?;
        parity(&store, &[("r1", 2), ("r2", 1), ("missing", 0), ("", 0)])?;
        let handle = store.handle("a").ok_or("Missing original handle")?;
        let tx = store.begin()?;
        store.put(element(&store, "d", "r1"))?;
        parity(&store, &[("r1", 3), ("r2", 1)])?;
        store.rollback(&tx)?;
        parity(&store, &[("r1", 2), ("r2", 1)])?;
        assert_eq!(store.handle("a"), Some(handle));
    }
    Ok(())
}
#[test]
fn current_view_tracks_replace_move_delete_and_recreated_identity() -> StorageResult<()> {
    for policy in policies() {
        let mut store = seeded(policy)?;
        let old = store.handle("a").ok_or("Missing original handle")?;
        let tx = store.begin()?;
        let mut replacement = element(&store, "a", "r1");
        replacement
            .values
            .insert("duplicates".into(), json!(["x", "x"]));
        store.put(replacement)?;
        parity(&store, &[("r1", 2), ("r2", 1)])?;
        store.put(element(&store, "a", "r2"))?;
        parity(&store, &[("r1", 1), ("r2", 2)])?;
        store.remove("a")?;
        parity(&store, &[("r1", 1), ("r2", 1)])?;
        store.put(element(&store, "a", "r1"))?;
        parity(&store, &[("r1", 2), ("r2", 1)])?;
        assert_ne!(store.handle("a"), Some(old.clone()));
        assert!(store.inspect_handle(&old, &mut |_| Ok(())).is_err());
        store.commit(&tx)?;
        parity(&store, &[("r1", 2), ("r2", 1)])?;
    }
    Ok(())
}
#[test]
fn nested_commit_and_rollback_preserve_exact_live_cardinality() -> StorageResult<()> {
    for policy in policies() {
        let mut store = seeded(policy)?;
        let baseline = storage::export(&store)?;
        let outer = store.begin()?;
        store.put(element(&store, "outer", "r1"))?;
        let inner = store.begin()?;
        store.put(element(&store, "a", "r2"))?;
        store.put(element(&store, "inner", "r2"))?;
        parity(&store, &[("r1", 2), ("r2", 3)])?;
        store.commit(&inner)?;
        parity(&store, &[("r1", 2), ("r2", 3)])?;
        store.rollback(&outer)?;
        assert_eq!(storage::export(&store)?, baseline);
        parity(&store, &[("r1", 2), ("r2", 1)])?;
        let outer = store.begin()?;
        store.put(element(&store, "outer", "r1"))?;
        let inner = store.begin()?;
        store.remove("b")?;
        parity(&store, &[("r1", 2), ("r2", 1)])?;
        store.rollback(&inner)?;
        parity(&store, &[("r1", 3), ("r2", 1)])?;
        store.commit(&outer)?;
        parity(&store, &[("r1", 3), ("r2", 1)])?;
    }
    Ok(())
}
#[test]
fn late_atomic_error_restores_counts_metadata_and_existing_handle() -> StorageResult<()> {
    for policy in policies() {
        let mut store = seeded(policy)?;
        let baseline = storage::export(&store)?;
        let handle = store.handle("a").ok_or("Missing original handle")?;
        let result: StorageResult<()> = storage::atomic(&mut store, |s| {
            s.remove("a")?;
            s.put(element(s, "a", "r2"))?;
            s.put(element(s, "transient", "r3"))?;
            parity(s, &[("r1", 1), ("r2", 2), ("r3", 1)])?;
            Err("late count control denial".into())
        });
        assert_eq!(result, Err("late count control denial".into()));
        assert_eq!(storage::export(&store)?, baseline);
        assert_eq!(store.handle("a"), Some(handle.clone()));
        assert!(store.inspect_handle(&handle, &mut |_| Ok(())).is_ok());
        assert!(store.handle("transient").is_none());
        parity(&store, &[("r1", 2), ("r2", 1), ("r3", 0)])?;
    }
    Ok(())
}
