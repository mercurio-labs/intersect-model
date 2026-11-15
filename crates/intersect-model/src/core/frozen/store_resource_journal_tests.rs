use super::*;
use crate::core::{
    resources::{ResourceLoadState, touch_resource},
    storage::{self, ModelRead, ModelStore},
};
use std::sync::Arc;

fn fixture() -> StorageResult<FrozenStore> {
    let package = crate::compile_rcore(
        r#"rcore 1; package touches version "1";
        metamodel "urn:touches" prefix t { class Node { attr name: String [0,1]; } }"#,
    )
    .map_err(|e| e.to_string())?;
    let mut store = FrozenStore::for_schema(package.schema()).map_err(|e| e.to_string())?;
    storage::atomic(&mut store, |store| {
        for uri in ["urn:a", "urn:b"] {
            // An empty resource may still have an interned URI after its last
            // object was removed. Exercise that state without unrelated writes.
            store
                .builder
                .data
                .strings
                .intern(uri)
                .map_err(|e| e.to_string())?;
            let mut record = ResourceRecord::new(uri)?;
            record.content_revision = 17;
            record.source = Some(SourceCorrespondence {
                uri: "file:///source.sml".into(),
                revision: "original".into(),
            });
            record.diagnostics.push("original diagnostic".into());
            store.set_resource_record(record)?;
        }
        Ok(())
    })?;
    Ok(store)
}

fn touch(store: &mut FrozenStore, uri: &str, canonical: bool) -> StorageResult<()> {
    if canonical {
        touch_resource_compat(store, uri)
    } else {
        touch_resource(store, uri)
    }
}

fn action(store: &mut FrozenStore, action: usize, canonical: bool) -> StorageResult<()> {
    let uri = if action == 1 { "urn:b" } else { "urn:a" };
    match action {
        0 | 1 => touch(store, uri, canonical),
        2 | 4 | 5 => {
            let mut record = ResourceRecord::new(uri)?;
            record.content_revision = if action == 5 { u64::MAX } else { 71 };
            record.roots = Arc::new(vec!["replacement-root".into()]);
            record.diagnostics.push("replacement diagnostic".into());
            if action == 4 {
                record.load_state = ResourceLoadState::Unloaded;
            }
            store.set_resource_record(record)
        }
        3 => store.remove_resource_record(uri),
        _ => Err("Unknown test action".into()),
    }
}

#[test]
fn nested_touches_replacements_and_removals_match_canonical_history() -> StorageResult<()> {
    let initial = fixture()?;
    // Compare live state as well as final rollback for every mixed history.
    // Rejected touches leave the checkpoint usable for subsequent operations.
    for outer_action in 0..6 {
        for inner_action in 0..6 {
            for later_action in 0..6 {
                for disposition in 0..4 {
                    let mut fast = initial.fork_arena()?;
                    let mut canonical = initial.fork_arena()?;
                    let a = fast.begin()?;
                    let b = canonical.begin()?;
                    assert_eq!(
                        action(&mut fast, outer_action, false),
                        action(&mut canonical, outer_action, true)
                    );
                    let c = fast.begin()?;
                    let d = canonical.begin()?;
                    for step in [inner_action, later_action] {
                        assert_eq!(
                            action(&mut fast, step, false),
                            action(&mut canonical, step, true)
                        );
                        assert_eq!(storage::export(&fast)?, storage::export(&canonical)?);
                    }
                    if disposition & 1 == 0 {
                        fast.commit(&c)?;
                        canonical.commit(&d)?;
                    } else {
                        fast.rollback(&c)?;
                        canonical.rollback(&d)?;
                    }
                    // A touch after promotion/merge must preserve earliest undo.
                    assert_eq!(
                        touch(&mut fast, "urn:a", false),
                        touch(&mut canonical, "urn:a", true)
                    );
                    if disposition & 2 == 0 {
                        fast.commit(&a)?;
                        canonical.commit(&b)?;
                    } else {
                        fast.rollback(&a)?;
                        canonical.rollback(&b)?;
                    }
                    assert_eq!(storage::export(&fast)?, storage::export(&canonical)?);
                    assert_eq!(
                        fast.validated_resource_uris,
                        canonical.validated_resource_uris
                    );
                }
            }
        }
    }
    Ok(())
}

#[test]
fn single_resource_touch_journal_is_inline_and_restores_metadata() -> StorageResult<()> {
    let mut store = fixture()?;
    let before = storage::export(&store)?;
    let outer = store.begin()?;
    for _ in 0..32 {
        let inner = store.begin()?;
        touch_resource(&mut store, "urn:a")?;
        let frame = store.frames.last().ok_or("Missing frame")?;
        assert!(frame.records.is_empty());
        assert!(frame.touches.first.is_some());
        assert!(frame.touches.more.is_empty());
        store.commit(&inner)?;
    }
    let record = store.resource_record("urn:a")?.ok_or("Missing resource")?;
    assert_eq!(record.content_revision, 49);
    assert!(record.source.is_none() && record.diagnostics.is_empty());
    touch_resource(&mut store, "urn:b")?;
    assert_eq!(
        store
            .frames
            .last()
            .ok_or("Missing frame")?
            .touches
            .more
            .len(),
        1
    );
    store.rollback(&outer)?;
    assert_eq!(storage::export(&store)?, before);
    Ok(())
}

#[test]
fn touch_fallbacks_preserve_admission_and_error_order() -> StorageResult<()> {
    let initial = fixture()?;
    for active in [false, true] {
        for state in [
            ResourceLoadState::Loaded,
            ResourceLoadState::Unloaded,
            ResourceLoadState::Failed,
        ] {
            for revision in [17, u64::MAX] {
                let mut seed = initial.fork_arena()?;
                storage::atomic(&mut seed, |store| {
                    let mut record = store.resource_record("urn:a")?.ok_or("Missing resource")?;
                    record.load_state = state;
                    record.content_revision = revision;
                    store.set_resource_record(record)
                })?;
                for uri in ["urn:a", "urn:missing", "bad uri", "urn:a#fragment"] {
                    let mut fast = seed.fork_arena()?;
                    let mut canonical = seed.fork_arena()?;
                    if active {
                        fast.begin()?;
                        canonical.begin()?;
                    }
                    assert_eq!(
                        touch(&mut fast, uri, false),
                        touch(&mut canonical, uri, true)
                    );
                    assert_eq!(storage::export(&fast)?, storage::export(&canonical)?);
                }
            }
        }
    }
    // Raw imports cannot borrow URI-validation proof, even with interned URIs.
    let schema = initial.builder.metamodel.schema();
    let mut raw = storage::export(&initial)?;
    let mut invalid = ResourceRecord::new("urn:valid")?;
    invalid.uri = "urn:invalid#fragment".into();
    raw.resources.insert(invalid.uri.clone(), invalid);
    for uri in ["urn:a", "urn:invalid#fragment"] {
        let mut fast = FrozenStore::from_materialized(schema, &raw)?;
        fast.builder
            .data
            .strings
            .intern(uri)
            .map_err(|e| e.to_string())?;
        let mut canonical = fast.fork_arena()?;
        let a = fast.begin()?;
        let b = canonical.begin()?;
        assert_eq!(
            touch(&mut fast, uri, false),
            touch(&mut canonical, uri, true)
        );
        fast.rollback(&a)?;
        canonical.rollback(&b)?;
        assert_eq!(storage::export(&fast)?, raw);
        assert_eq!(storage::export(&canonical)?, raw);
    }
    Ok(())
}

#[test]
fn ordinary_writes_keep_resource_effects_and_nested_rollback() -> StorageResult<()> {
    let mut store = fixture()?;
    let schema = store.builder.metamodel.schema().clone();
    let class = schema.classes.keys().next().ok_or("Missing class")?;
    storage::atomic(&mut store, |store| {
        store.create_element(&schema, "node", class, "urn:a", 0, 1)
    })?;
    let before = storage::export(&store)?;
    let outer = store.begin()?;
    let inner = store.begin()?;
    store.set_value("node", "name", Some(serde_json::json!("changed")))?;
    store.commit(&inner)?;
    let changed = store.resource_record("urn:a")?.ok_or("Missing resource")?;
    assert_eq!(
        changed.content_revision,
        before.resources["urn:a"].content_revision + 1
    );
    // No-op assignments must not touch the resource.
    store.set_value("node", "name", Some(serde_json::json!("changed")))?;
    assert_eq!(store.resource_record("urn:a")?, Some(changed));
    store.rollback(&outer)?;
    assert_eq!(storage::export(&store)?, before);
    Ok(())
}

#[test]
fn compact_journal_preserves_checkpoint_order_and_symbol_lifetimes() -> StorageResult<()> {
    let mut store = fixture()?;
    let before = storage::export(&store)?;
    let outer = store.begin()?;
    let inner = store.begin()?;
    touch_resource(&mut store, "urn:a")?;
    assert!(store.rollback(&outer).is_err());
    assert!(store.commit(&outer).is_err());
    store.commit(&inner)?;
    assert!(store.rollback(&inner).is_err());
    // URI interned after the outer checkpoint: parent full-record undo must
    // dominate a child's compact undo before the symbol suffix is truncated.
    store.set_resource_record(ResourceRecord::new("urn:new")?)?;
    store
        .builder
        .data
        .strings
        .intern("urn:new")
        .map_err(|e| e.to_string())?;
    let inner = store.begin()?;
    touch_resource(&mut store, "urn:new")?;
    store.commit(&inner)?;
    store.rollback(&outer)?;
    assert_eq!(storage::export(&store)?, before);
    assert!(store.builder.data.strings.get("urn:new").is_none());
    assert!(!store.validated_resource_uris.contains("urn:new"));
    Ok(())
}
