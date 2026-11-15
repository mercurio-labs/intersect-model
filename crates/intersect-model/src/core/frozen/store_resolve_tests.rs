use super::*;
use crate::definitions::schema::Schema;
use serde_json::json;

fn fixture(indexed: bool) -> StorageResult<(Schema, FrozenStore)> {
    let package = crate::compile_rcore(
        r#"rcore 1; package resolve version "1";
      metamodel "urn:resolve" prefix r { class Node {
        attr name: String [0,1] default "seed";
        attr labels: String [0,*];
        ref links: Node [0,*]; ref second: Node [0,*];
        ref children: Node [0,*] containment; ref one: Node [0,1];
      } }"#,
    )
    .map_err(|e| e.to_string())?;
    let schema = package.schema().clone();
    let mm = Arc::new(Metamodel::freeze(&schema).map_err(|e| e.to_string())?);
    let mut store = FrozenStore::with_index_policy(
        mm,
        if indexed {
            IndexPolicy::AllValues
        } else {
            IndexPolicy::SelectedValues(BTreeSet::from(["name".into()]))
        },
    );
    store.archive_defaults = true;
    let class = schema.classes.keys().next().ok_or("Missing class")?;
    storage::atomic(&mut store, |store| {
        for id in ["owner", "a", "b", "c", "?literal"] {
            store.create_element(&schema, id, class, "fixture", 0, 0)?;
        }
        Ok(())
    })?;
    Ok((schema, store))
}

fn pending(store: &mut FrozenStore) -> StorageResult<()> {
    storage::atomic(store, |store| {
        for id in ["a", "b"] {
            store.set_unresolved_reference("owner", "links", id, true)?;
        }
        Ok(())
    })
}

fn states(store: &FrozenStore, field: &str) -> StorageResult<Vec<&'static str>> {
    let mut states = Vec::new();
    store.inspect("owner", &mut |object| {
        if let Some(items) = object.value(field).and_then(|value| value.collection()) {
            for item in items.iter() {
                states.push(match item.reference() {
                    Some(storage::ReferenceView::Resolved(_)) => "resolved",
                    Some(storage::ReferenceView::Unresolved { .. }) => "unresolved",
                    Some(storage::ReferenceView::Unavailable { .. }) => "unavailable",
                    None => "other",
                });
            }
        }
        Ok(())
    })?;
    Ok(states)
}

#[test]
fn repeated_resolution_keeps_one_owned_list_and_original_pending_proxies() -> StorageResult<()> {
    let (schema, mut store) = fixture(false)?;
    let class = schema.classes.keys().next().ok_or("Missing class")?;
    let n = 128;
    storage::atomic(&mut store, |store| {
        for i in 0..n {
            let id = format!("target{i}");
            store.create_element(&schema, &id, class, "fixture", 0, 0)?;
            store.set_unresolved_reference("owner", "links", &id, true)?;
        }
        Ok(())
    })?;
    let (lists, proxies) = (
        store.builder.data.lists.len(),
        store.builder.data.proxies.len(),
    );
    let mut oracle = store.fork_arena()?;
    storage::atomic(&mut store, |store| {
        for i in 0..n {
            store.resolve_reference("owner", "links", Some(i), &format!("target{i}"))?;
        }
        Ok(())
    })?;
    storage::atomic(&mut oracle, |store| {
        for i in 0..n {
            storage::resolve_reference_compat(
                store,
                "owner",
                "links",
                Some(i),
                &format!("target{i}"),
            )?;
        }
        Ok(())
    })?;
    assert_eq!(store.builder.data.lists.len(), lists);
    assert_eq!(store.builder.data.proxies.len(), proxies);
    assert_eq!(storage::export(&store)?, storage::export(&oracle)?);
    assert_eq!(states(&store, "links")?, vec!["resolved"; n]);
    assert_eq!(
        store.query(Query::Value("links", &json!("target127")))?,
        ["owner"]
    );
    assert_eq!(
        store.query(Query::Value("name", &json!("seed")))?,
        oracle.query(Query::Value("name", &json!("seed")))?
    );
    storage::validate(&store, &schema)
}

#[test]
fn replacement_and_append_journals_restore_nested_commits_and_rollbacks() -> StorageResult<()> {
    let (_, original) = fixture(false)?;
    for commit in [false, true] {
        let mut store = original.fork_arena()?;
        pending(&mut store)?;
        let before = storage::export(&store)?;
        let outer = store.begin()?;
        store.resolve_reference("owner", "links", Some(0), "a")?;
        let after_outer = storage::export(&store)?;
        let inner = store.begin()?;
        store.resolve_reference("owner", "links", Some(0), "b")?;
        store.set_unresolved_reference("owner", "links", "c", true)?;
        store.resolve_reference("owner", "links", Some(2), "c")?;
        let deepest = store.begin()?;
        store.resolve_reference("owner", "links", Some(0), "c")?;
        store.commit(&deepest)?;
        store.set_resolved_value("owner", "links", Some(json!(["b", "c"])))?;
        store.resolve_reference("owner", "links", Some(0), "a")?;
        if commit {
            store.commit(&inner)?;
        } else {
            store.rollback(&inner)?;
            assert_eq!(storage::export(&store)?, after_outer);
        }
        store.rollback(&outer)?;
        assert_eq!(storage::export(&store)?, before);
        assert_eq!(states(&store, "links")?, ["unresolved", "unresolved"]);
        storage::atomic(&mut store, |store| {
            store.resolve_reference("owner", "links", Some(1), "b")?;
            Ok(())
        })?;
        assert_eq!(states(&store, "links")?, ["unresolved", "resolved"]);
    }
    Ok(())
}

#[test]
fn caught_revision_failure_and_noop_keep_an_active_transaction_usable() -> StorageResult<()> {
    let (_, mut store) = fixture(false)?;
    pending(&mut store)?;
    let outer = store.begin()?;
    store.resolve_reference("owner", "links", Some(0), "a")?;
    let record = store.resource_record("fixture")?.ok_or("resource")?;
    let mut maximum = record.clone();
    maximum.content_revision = u64::MAX;
    store.set_resource_record(maximum)?;
    let before = storage::export(&store)?;
    // Identical values have no revision effect, even at the revision ceiling.
    store.resolve_reference("owner", "links", Some(0), "a")?;
    assert_eq!(storage::export(&store)?, before);
    assert!(
        store
            .resolve_reference("owner", "links", Some(1), "b")
            .is_err()
    );
    assert_eq!(storage::export(&store)?, before);
    assert_eq!(states(&store, "links")?, ["resolved", "unresolved"]);
    store.set_resource_record(record)?;
    store.resolve_reference("owner", "links", Some(1), "b")?;
    store.commit(&outer)?;
    assert_eq!(states(&store, "links")?, ["resolved", "resolved"]);
    Ok(())
}

#[test]
fn compatibility_fallbacks_keep_errors_indexes_and_wire_normalization() -> StorageResult<()> {
    for indexed in [false, true] {
        for case in 0..7 {
            let (schema, mut store) = fixture(indexed)?;
            pending(&mut store)?;
            storage::atomic(&mut store, |store| {
                match case {
                    1 => store.set_resolved_value(
                        "owner",
                        "links",
                        Some(json!(["?literal", "b"])),
                    )?,
                    2 => {
                        store.set_resolved_value("owner", "links", Some(json!(["late", "b"])))?;
                        store.create_element(
                            &schema,
                            "late",
                            schema.classes.keys().next().ok_or("class")?,
                            "fixture",
                            0,
                            0,
                        )?;
                    }
                    3 => store.set_value("owner", "children", Some(json!(["?a", "?b"])))?,
                    4 => store.set_value("owner", "labels", Some(json!(["x", "y"])))?,
                    5 => {
                        let mut record = store.resource_record("fixture")?.ok_or("resource")?;
                        record.content_revision = u64::MAX;
                        store.set_resource_record(record)?;
                    }
                    _ => {}
                }
                Ok(())
            })?;
            let mut oracle = store.fork_arena()?;
            let (field, target) = match case {
                1 => ("links", "a"),
                2 => ("links", "a"),
                3 => ("children", "a"),
                4 => ("labels", "a"),
                6 => ("links", "?literal"),
                _ => ("links", "a"),
            };
            let index = Some(if case == 1 || case == 2 { 1 } else { 0 });
            let actual = storage::atomic(&mut store, |store| {
                store.resolve_reference("owner", field, index, target)
            });
            let expected = storage::atomic(&mut oracle, |store| {
                storage::resolve_reference_compat(store, "owner", field, index, target)
            });
            assert_eq!(actual, expected, "case {case}, indexed {indexed}");
            assert_eq!(storage::export(&store)?, storage::export(&oracle)?);
            assert_eq!(states(&store, field)?, states(&oracle, field)?);
            for value in ["a", "b", "?a", "?literal"] {
                assert_eq!(
                    store.query(Query::Value(field, &json!(value)))?,
                    oracle.query(Query::Value(field, &json!(value)))?
                );
            }
        }
    }
    let (_, mut store) = fixture(false)?;
    pending(&mut store)?;
    for (id, field, index) in [
        ("missing", "links", Some(0)),
        ("owner", "missing", Some(0)),
        ("owner", "links", Some(99)),
        ("owner", "one", Some(0)),
        ("owner", "one", None),
    ] {
        let mut oracle = store.fork_arena()?;
        let actual = store.resolve_reference(id, field, index, "a");
        let expected = storage::resolve_reference_compat(&mut oracle, id, field, index, "a");
        assert_eq!(actual, expected);
        let actual = storage::atomic(&mut store, |store| {
            store.resolve_reference(id, field, index, "a")
        });
        let expected = storage::atomic(&mut oracle, |store| {
            storage::resolve_reference_compat(store, id, field, index, "a")
        });
        assert_eq!(actual, expected);
        assert_eq!(storage::export(&store)?, storage::export(&oracle)?);
    }
    Ok(())
}

#[test]
fn imported_aliases_are_copied_once_and_forks_keep_their_cells() -> StorageResult<()> {
    let (schema, mut store) = fixture(false)?;
    storage::atomic(&mut store, |store| {
        store.set_resolved_value("owner", "links", Some(json!(["a", "b"])))
    })?;
    let owner = store.object("owner").ok_or("owner")?;
    let header = store.builder.data.header(owner).ok_or("header")?;
    let mm = store.builder.metamodel.clone();
    let links = header.slots as usize
        + mm.slot(
            header.class,
            mm.feature(header.class, "links").ok_or("links")?,
        )
        .ok_or("slot")?;
    let second = header.slots as usize
        + mm.slot(
            header.class,
            mm.feature(header.class, "second").ok_or("second")?,
        )
        .ok_or("slot")?;
    store.builder.data.slots[second] = store.builder.data.slots[links];
    let before = storage::export(&store)?;
    let fork = store.fork_arena()?;
    let lists = store.builder.data.lists.len();
    storage::atomic(&mut store, |store| {
        store.resolve_reference("owner", "links", Some(1), "c")?;
        store.resolve_reference("owner", "links", Some(0), "b")?;
        Ok(())
    })?;
    assert_eq!(store.builder.data.lists.len(), lists + 1);
    assert_eq!(store.value("owner", "links")?, Some(json!(["b", "c"])));
    assert_eq!(store.value("owner", "second")?, Some(json!(["a", "b"])));
    assert_eq!(storage::export(&fork)?, before);
    storage::validate(&store, &schema)
}
