//! Append-only mutation of slot-owned lists. Snapshots/imported lists are never
//! assumed exclusive. Nested rollback stores lengths, not copied prefixes.
use super::super::{FeatureId, builder::encode_list_item, intern::Sym};
use super::*;

#[derive(Clone, Default)]
struct Members {
    keys: Vec<Sym>,
    counts: FxHashMap<Sym, usize>,
}

impl Members {
    fn push(&mut self, key: Sym) {
        self.keys.push(key);
        *self.counts.entry(key).or_default() += 1;
    }
    fn truncate(&mut self, length: usize) {
        while self.keys.len() > length {
            if let Some(key) = self.keys.pop()
                && let Some(count) = self.counts.get_mut(&key)
            {
                *count -= 1;
                if *count == 0 {
                    self.counts.remove(&key);
                }
            }
        }
    }
}

#[derive(Clone)]
pub(super) struct AppendList {
    pub(super) owner: usize,
    members: Option<Members>,
}
impl AppendList {
    pub(super) fn truncate(&mut self, length: usize) {
        if let Some(members) = &mut self.members {
            members.truncate(length);
        }
    }
}

impl FrozenStore {
    /// The same raw write boundary as set_value; callers still perform checked
    /// structural edits/validation. Only the appended atom is encoded/indexed.
    pub(super) fn append_item(&mut self, id: &str, field: &str, value: Value) -> StorageResult<()> {
        self.active()?;
        storage::atomic(self, |store| {
            let obj = store.object(id).ok_or("Missing element")?;
            let mm = store.builder.metamodel.clone();
            let header = store
                .builder
                .data
                .header(obj)
                .ok_or("Missing object")?
                .clone();
            let feature = mm
                .feature(header.class, field)
                .ok_or_else(|| format!("Unknown feature {}.{field}", mm.class_key(header.class)))?;
            let offset = mm
                .slot(header.class, feature)
                .ok_or("Derived values cannot be stored")?;
            let kind = mm.feature_info(feature).ok_or("Unknown feature")?.kind;
            let index = header.slots as usize + offset;
            let current = *store
                .builder
                .data
                .slots
                .get(index)
                .ok_or("Missing object slots")?;
            let resource = store
                .builder
                .data
                .resources
                .get(header.resource as usize)
                .ok_or("Missing object resource")?;
            let resource = store.builder.data.strings.resolve(*resource).to_owned();
            store.touch(obj)?;
            let list = store.owned_append_list(obj, feature, kind, index, current)?;
            let atom = if kind == FeatureKind::Attribute {
                encode_list_item(&mut store.builder.data, value.clone())
                    .map_err(|e| e.to_string())?
            } else {
                store.reference(obj, feature, &value, ReferenceEncoding::Wire)?
            };
            let key = if kind == FeatureKind::Containment {
                Some(
                    store
                        .builder
                        .data
                        .strings
                        .intern(value.as_str().ok_or("Invalid reference")?)
                        .map_err(|e| e.to_string())?,
                )
            } else {
                None
            };
            let existed = key.is_some_and(|key| {
                store
                    .append_lists
                    .get(&list)
                    .and_then(|owned| owned.members.as_ref())
                    .is_some_and(|m| m.counts.contains_key(&key))
            });
            // An append has exactly one possible new containment member. Using
            // the shared delta hook also preserves revision/load-state checks.
            crate::core::resources::field_delta_inner(
                store,
                &resource,
                kind == FeatureKind::Containment && !existed,
                None,
                Some(&value),
            )?;
            let length = store
                .builder
                .data
                .lists
                .get(list as usize)
                .ok_or("Missing list")?
                .len();
            let frame = store
                .frames
                .last_mut()
                .ok_or("Storage write requires a transaction")?;
            if (list as usize) < frame.lists {
                frame.list_lengths.entry(list).or_insert(length);
            }
            store
                .builder
                .data
                .lists
                .get_mut(list as usize)
                .ok_or("Missing list")?
                .push(atom);
            if let Some(key) = key
                && let Some(members) = store
                    .append_lists
                    .get_mut(&list)
                    .and_then(|o| o.members.as_mut())
            {
                members.push(key);
            }
            *store
                .builder
                .data
                .slots
                .get_mut(index)
                .ok_or("Missing object slots")? = Slot::Many(list);
            if let Some(name) = mm
                .class_info(header.class)
                .and_then(|class| class.stored.get(offset))
                .map(|&feature| mm.feature_name(feature))
                .filter(|name| store.indexes.indexes(name))
            {
                store.indexes.update_member(obj, name, &value, true);
            }
            Ok(())
        })
    }

    pub(super) fn owned_append_list(
        &mut self,
        obj: ObjId,
        feature: FeatureId,
        kind: FeatureKind,
        index: usize,
        current: Slot,
    ) -> StorageResult<u32> {
        let containment = kind == FeatureKind::Containment;
        if let Slot::Many(list) = current
            && self
                .append_lists
                .get(&list)
                .is_some_and(|o| o.owner == index && o.members.is_some() == containment)
        {
            return Ok(list);
        }
        let next = match current {
            Slot::Many(list) => {
                let items = self
                    .builder
                    .data
                    .lists
                    .get(list as usize)
                    .ok_or("Missing list")?
                    .clone();
                new_list(&mut self.builder.data, items).map_err(|e| e.to_string())?
            }
            Slot::Absent => {
                new_list(&mut self.builder.data, Vec::new()).map_err(|e| e.to_string())?
            }
            slot => {
                let value = self
                    .slot_value(feature, slot)
                    .unwrap_or_else(|| Value::Array(Vec::new()));
                if !value.is_array() {
                    return Err("Multiple feature requires a list".into());
                }
                self.encode_slot(obj, feature, kind, &value, ReferenceEncoding::Wire)?
            }
        };
        let Slot::Many(list) = next else {
            return Err("Multiple feature requires a list".into());
        };
        let members = if containment {
            let length = self
                .builder
                .data
                .lists
                .get(list as usize)
                .ok_or("Missing list")?
                .len();
            let mut members = Members::default();
            for i in 0..length {
                let value = self.decode(self.builder.data.lists[list as usize][i]);
                let key = self
                    .builder
                    .data
                    .strings
                    .intern(value.as_str().ok_or("Invalid reference")?)
                    .map_err(|e| e.to_string())?;
                members.push(key);
            }
            Some(members)
        } else {
            None
        };
        self.append_lists.insert(
            list,
            AppendList {
                owner: index,
                members,
            },
        );
        Ok(list)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::definitions::schema::Schema;
    use serde_json::json;

    fn fixture(
        default: Value,
        unsettable: bool,
        archive: bool,
    ) -> StorageResult<(Schema, FrozenStore)> {
        let package = crate::compile_rcore(
            r#"rcore 1; package append version "1";
            metamodel "urn:append" prefix a { class Node {
                attr labels: String [0,*]; attr name: String [0,1] default "seed";
                ref children: Node [0,*] containment; ref links: Node [0,*];
            } }"#,
        )
        .map_err(|e| e.to_string())?;
        let mut schema = package.schema().clone();
        let class = schema
            .classes
            .keys()
            .next()
            .cloned()
            .ok_or("Missing fixture class")?;
        let labels = schema
            .classes
            .get_mut(&class)
            .and_then(|c| c.features.get_mut("labels"))
            .ok_or("Missing fixture labels")?;
        labels.default = default;
        labels.unsettable = unsettable;
        let mut store = FrozenStore::with_index_policy(
            Arc::new(Metamodel::freeze(&schema).map_err(|e| e.to_string())?),
            IndexPolicy::AllValues,
        );
        store.archive_defaults = archive;
        storage::atomic(&mut store, |store| {
            for id in ["owner", "a", "b", "?literal"] {
                store.create_element(&schema, id, &class, "fixture", 0, 0)?;
            }
            Ok(())
        })?;
        Ok((schema, store))
    }
    fn legacy_append(store: &mut FrozenStore, field: &str, value: Value) -> StorageResult<()> {
        let mut before = store.value("owner", field)?.unwrap_or_else(|| json!([]));
        before
            .as_array_mut()
            .ok_or("Expected fixture list")?
            .push(value);
        store.set_value("owner", field, Some(before))
    }

    #[test]
    fn streaming_append_retains_linear_arena_storage() -> StorageResult<()> {
        let (schema, mut store) = fixture(json!([]), false, true)?;
        let n = 256;
        storage::atomic(&mut store, |store| {
            for i in 0..n {
                store.append_value("owner", "labels", json!(format!("v{i}")))?;
            }
            Ok(())
        })?;
        assert_eq!(
            store
                .value("owner", "labels")?
                .and_then(|v| v.as_array().map(Vec::len)),
            Some(n)
        );
        assert!(
            store
                .builder
                .data
                .lists
                .iter()
                .map(Vec::capacity)
                .sum::<usize>()
                <= n * 2 + 8
        );
        assert_eq!(
            store.query(Query::Value("labels", &json!("v255")))?,
            ["owner"]
        );
        storage::validate(&store, &schema)
    }

    #[test]
    fn nested_commit_and_rollback_restore_values_indexes_and_resource_records() -> StorageResult<()>
    {
        let (_, mut store) = fixture(json!([]), false, true)?;
        let initial = storage::export(&store)?;
        let outer = store.begin()?;
        store.append_value("owner", "labels", json!("outer"))?;
        let after_outer = storage::export(&store)?;
        let inner = store.begin()?;
        store.append_value("owner", "labels", json!("discarded"))?;
        store.append_value("owner", "children", json!("a"))?;
        assert!(
            !store
                .resource_record("fixture")?
                .ok_or("Missing resource")?
                .roots
                .iter()
                .any(|s| s == "a")
        );
        store.rollback(&inner)?;
        assert_eq!(storage::export(&store)?, after_outer);
        assert!(
            store
                .query(Query::Value("labels", &json!("discarded")))?
                .is_empty()
        );
        let inner = store.begin()?;
        store.append_value("owner", "labels", json!("committed-child"))?;
        store.commit(&inner)?;
        store.set_value("owner", "labels", Some(json!(["replacement"])))?;
        store.append_value("owner", "labels", json!("tail"))?;
        let tail = store.begin()?;
        store.append_value("owner", "labels", json!("temporary"))?;
        store.rollback(&tail)?;
        assert_eq!(
            store.value("owner", "labels")?,
            Some(json!(["replacement", "tail"]))
        );
        store.rollback(&outer)?;
        assert_eq!(storage::export(&store)?, initial);
        assert!(
            store
                .query(Query::Value("labels", &json!("tail")))?
                .is_empty()
        );
        storage::atomic(&mut store, |store| {
            store.append_value("owner", "labels", json!("reused-symbol"))
        })?;
        assert_eq!(
            store.value("owner", "labels")?,
            Some(json!(["reused-symbol"]))
        );
        Ok(())
    }

    #[test]
    fn defaults_unsettable_and_absent_match_the_generic_write_contract() -> StorageResult<()> {
        for default in [json!([]), json!(["seed"])] {
            for unsettable in [false, true] {
                for archive in [false, true] {
                    let (_, mut store) = fixture(default.clone(), unsettable, archive)?;
                    let mut control = store.fork_arena()?;
                    storage::atomic(&mut store, |s| {
                        s.append_value("owner", "labels", json!("next"))
                    })?;
                    storage::atomic(&mut control, |s| legacy_append(s, "labels", json!("next")))?;
                    assert_eq!(storage::export(&store)?, storage::export(&control)?);
                    for s in [&mut store, &mut control] {
                        storage::atomic(s, |s| s.set_value("owner", "labels", None))?;
                    }
                    storage::atomic(&mut store, |s| {
                        s.append_value("owner", "labels", json!("assigned"))
                    })?;
                    storage::atomic(&mut control, |s| {
                        legacy_append(s, "labels", json!("assigned"))
                    })?;
                    assert_eq!(storage::export(&store)?, storage::export(&control)?);
                }
            }
        }
        Ok(())
    }

    #[test]
    fn raw_json_members_keep_atomic_index_keys() -> StorageResult<()> {
        let (_, mut store) = fixture(json!([]), false, false)?;
        let mut control = store.fork_arena()?;
        // Raw storage can stage invalid values; checked facade validation is a
        // separate boundary. Nested JSON must not be flattened by its indexes.
        for value in [
            json!(["nested"]),
            json!({"key":"value"}),
            Value::Null,
            json!(1.25),
        ] {
            storage::atomic(&mut store, |s| {
                s.append_value("owner", "labels", value.clone())
            })?;
            storage::atomic(&mut control, |s| legacy_append(s, "labels", value.clone()))?;
        }
        assert_eq!(storage::export(&store)?, storage::export(&control)?);
        for value in [
            json!(["nested"]),
            json!({"key":"value"}),
            json!("nested"),
            Value::Null,
        ] {
            assert_eq!(
                store.query(Query::Value("labels", &value))?,
                control.query(Query::Value("labels", &value))?
            );
        }
        Ok(())
    }

    #[test]
    fn append_preserves_existing_resolved_and_lexical_reference_states() -> StorageResult<()> {
        let (_, mut store) = fixture(json!([]), false, false)?;
        storage::atomic(&mut store, |s| {
            s.set_resolved_value("owner", "links", Some(json!(["?literal"])))?;
            s.append_value("owner", "links", json!("a"))?;
            s.append_value("owner", "links", json!("?missing"))
        })?;
        assert_eq!(
            store.value("owner", "links")?,
            Some(json!(["?literal", "a", "?missing"]))
        );
        store.inspect("owner", &mut |object| {
            let list = object
                .value("links")
                .and_then(|v| v.collection())
                .ok_or("Missing links")?;
            assert!(matches!(
                list.get(0).and_then(|v| v.reference()),
                Some(storage::ReferenceView::Resolved(_))
            ));
            assert!(matches!(
                list.get(2).and_then(|v| v.reference()),
                Some(storage::ReferenceView::Unresolved {
                    spelling: "missing"
                })
            ));
            Ok(())
        })?;
        Ok(())
    }

    #[test]
    fn duplicate_containment_members_survive_nested_rollback_without_new_root_delta()
    -> StorageResult<()> {
        let (_, mut store) = fixture(json!([]), false, false)?;
        storage::atomic(&mut store, |s| {
            s.append_value("owner", "children", json!("a"))?;
            s.append_value("owner", "children", json!("a"))
        })?;
        // Raw persistence may retain an invalid draft. A duplicate append must
        // preserve the existing root state, just like the shared set delta.
        let outer = store.begin()?;
        let inner = store.begin()?;
        store.append_value("owner", "children", json!("a"))?;
        store.rollback(&inner)?;
        crate::core::resources::change_root(&mut store, "fixture", "a", true)?;
        let before = store
            .resource_record("fixture")?
            .ok_or("Missing resource")?;
        store.append_value("owner", "children", json!("a"))?;
        let after = store
            .resource_record("fixture")?
            .ok_or("Missing resource")?;
        assert_eq!(before.roots, after.roots);
        assert_eq!(after.content_revision, before.content_revision + 1);
        store.rollback(&outer)?;
        assert_eq!(store.value("owner", "children")?, Some(json!(["a", "a"])));
        Ok(())
    }

    #[test]
    fn imported_aliases_and_forks_do_not_share_mutable_append_storage() -> StorageResult<()> {
        let (_, mut store) = fixture(json!([]), false, false)?;
        storage::atomic(&mut store, |s| {
            for id in ["owner", "a"] {
                s.set_value(id, "labels", Some(json!(["shared"])))?;
            }
            Ok(())
        })?;
        let slot = |s: &FrozenStore, id: &str| -> StorageResult<usize> {
            let object = s.object(id).ok_or("Missing object")?;
            let h = s.builder.data.header(object).ok_or("Missing header")?;
            let feature = s
                .builder
                .metamodel
                .feature(h.class, "labels")
                .ok_or("Missing labels")?;
            Ok(h.slots as usize
                + s.builder
                    .metamodel
                    .slot(h.class, feature)
                    .ok_or("Missing slot")?)
        };
        let owner = slot(&store, "owner")?;
        let other = slot(&store, "a")?;
        // Simulate a compact imported image that reuses an equal list. Lookup
        // postings remain valid because both fields have identical values.
        store.builder.data.slots[other] = store.builder.data.slots[owner];
        storage::atomic(&mut store, |s| {
            s.append_value("owner", "labels", json!("next"))
        })?;
        assert_eq!(store.value("a", "labels")?, Some(json!(["shared"])));
        let before = storage::export(&store)?;
        let mut fork = store.fork_arena()?;
        storage::atomic(&mut fork, |s| {
            s.append_value("owner", "labels", json!("fork"))
        })?;
        assert_eq!(storage::export(&store)?, before);
        assert_eq!(
            fork.value("owner", "labels")?,
            Some(json!(["shared", "next", "fork"]))
        );
        Ok(())
    }

    #[test]
    fn prepared_prefix_and_errors_restore_the_original_graph() -> StorageResult<()> {
        let (_, mut source) = fixture(json!([]), false, true)?;
        storage::atomic(&mut source, |s| {
            s.append_value("owner", "labels", json!("seed"))
        })?;
        let wire = storage::export(&source)?;
        let model = super::super::super::Model::from_model(source.metamodel().clone(), &wire)
            .map_err(|e| e.to_string())?;
        let immutable = model.to_model().map_err(|e| e.to_string())?;
        let mut store = FrozenStore::from_model(model.clone());
        let before = storage::export(&store)?;
        let checkpoint = store.begin()?;
        store.append_value("owner", "labels", json!("discarded"))?;
        assert!(store.prepared_written);
        store.rollback(&checkpoint)?;
        assert!(!store.prepared_written);
        assert_eq!(storage::export(&store)?, before);
        assert_eq!(model.to_model().map_err(|e| e.to_string())?, immutable);
        let checkpoint = store.begin()?;
        assert!(store.append_value("owner", "links", json!(123)).is_err());
        assert_eq!(storage::export(&store)?, before);
        store.rollback(&checkpoint)?;
        storage::atomic(&mut store, |s| {
            let mut record = s.resource_record("fixture")?.ok_or("Missing resource")?;
            record.content_revision = u64::MAX;
            s.set_resource_record(record)
        })?;
        let before = storage::export(&store)?;
        let checkpoint = store.begin()?;
        assert!(
            store
                .append_value("owner", "labels", json!("must-not-publish"))
                .is_err()
        );
        assert_eq!(storage::export(&store)?, before);
        assert!(
            store
                .query(Query::Value("labels", &json!("must-not-publish")))?
                .is_empty()
        );
        store.rollback(&checkpoint)?;
        Ok(())
    }
}
