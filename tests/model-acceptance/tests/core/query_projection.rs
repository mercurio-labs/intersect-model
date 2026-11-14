use intersect_model::core::{
    api::{BasicModel, ModelEdit, TransactionalModel, VersionedModel},
    frozen::FrozenStore,
    model::Model,
    storage::{self, IndexPolicy, InvocationContext, ModelRead, ModelView, Query},
};
use intersect_model::definitions::conversion::{InvocationObserver, ProviderId};
use serde_json::json;
use std::{cell::Cell, rc::Rc};

struct Probe {
    bytes: Cell<usize>,
    checks: Cell<usize>,
    limit: usize,
    stop: usize,
}
impl InvocationObserver for Probe {
    fn before_invoke(&self, _: &ProviderId) -> Result<(), String> {
        panic!("query dispatched a provider")
    }
    fn checkpoint(&self) -> Result<(), String> {
        if self.checks.get() == self.stop {
            return Err("cancelled".into());
        }
        self.checks.set(self.checks.get() + 1);
        Ok(())
    }
    fn retain_storage(&self, bytes: usize) -> Result<(), String> {
        let next = self
            .bytes
            .get()
            .checked_add(bytes)
            .filter(|n| *n <= self.limit)
            .ok_or("storage denied")?;
        self.bytes.set(next);
        Ok(())
    }
}
fn probe(limit: usize, stop: usize) -> Rc<Probe> {
    Rc::new(Probe {
        bytes: Cell::new(0),
        checks: Cell::new(0),
        limit,
        stop,
    })
}
fn guarded<T: std::fmt::Debug + PartialEq>(
    bytes: usize,
    expected: T,
    operation: impl Fn(&InvocationContext<'_>) -> Result<T, String>,
) {
    let baseline = probe(bytes, usize::MAX);
    assert_eq!(
        operation(&InvocationContext::new(baseline.clone())).unwrap(),
        expected
    );
    assert_eq!(baseline.bytes.get(), bytes);
    for limit in [0, bytes.saturating_sub(1)]
        .into_iter()
        .filter(|_| bytes != 0)
    {
        assert_eq!(
            operation(&InvocationContext::new(probe(limit, usize::MAX))).unwrap_err(),
            "storage denied"
        );
    }
    assert_eq!(
        operation(&InvocationContext::new(probe(bytes + 1, usize::MAX))).unwrap(),
        expected
    );
    for stop in 0..baseline.checks.get() {
        assert_eq!(
            operation(&InvocationContext::new(probe(usize::MAX, stop))).unwrap_err(),
            "cancelled"
        );
    }
    assert_eq!(operation(&InvocationContext::default()).unwrap(), expected);
}

#[test]
fn key_and_identity_guards_count_utf8_and_json_escapes_before_copying() {
    // Two string slots = 128; "value:f" = 7; encoded "é\n\"" = 8 bytes.
    let value = json!("é\n\"");
    guarded(143, ("value:f".into(), value.to_string()), |o| {
        storage::query_key_observed(Query::Value("f", &value), o)
    });
    guarded(134, ("class".into(), "A".into()), |o| {
        storage::query_key_observed(Query::Class("A"), o)
    });
    guarded(137, ("resource".into(), "r".into()), |o| {
        storage::query_key_observed(Query::Resource("r"), o)
    });
    guarded(98, vec![(7, "é".into())], |o| {
        let mut rows = Vec::new();
        storage::push_identity_observed(&mut rows, 7, "é", o)?;
        Ok(rows)
    });
    let mut rows = vec![(0, "existing".into())];
    assert!(
        storage::push_identity_observed(
            &mut rows,
            1,
            "é",
            &InvocationContext::new(probe(97, usize::MAX))
        )
        .is_err()
    );
    assert_eq!(rows, [(0, "existing".into())]);
}

#[test]
fn observed_query_shape_limits_do_not_change_unobserved_compatibility() {
    let mut deep = json!(0);
    for _ in 0..66 {
        deep = json!([deep]);
    }
    let wide = json!(vec![0; 65_537]);
    for value in [&deep, &wide] {
        assert!(
            storage::query_key_observed(
                Query::Value("f", value),
                &InvocationContext::new(probe(usize::MAX, usize::MAX))
            )
            .is_err()
        );
        assert_eq!(
            storage::query_key_observed(Query::Value("f", value), &InvocationContext::default())
                .unwrap(),
            storage::query_key(Query::Value("f", value))
        );
    }
}

#[test]
fn indexed_frozen_and_versioned_pages_preserve_cursors_and_exact_guards() {
    let schema = intersect_model_acceptance::migration_fixture::compile_migrated_schema_fixture(r#"<e:EPackage xmlns:e="http://www.eclipse.org/emf/2002/Ecore" xmlns:x="http://www.w3.org/2001/XMLSchema-instance" name="s" nsURI="urn:query-projection"><eClassifiers x:type="e:EClass" name="A"/></e:EPackage>"#).unwrap();
    let mut model = Model::default();
    for id in ["é", "z"] {
        model.create(&schema, id, "A", "r", 0, 0).unwrap();
    }
    let before = model.clone();
    let view = ModelView::new(&model).unwrap();
    let frozen = FrozenStore::from_materialized(&schema, &model).unwrap();
    for (reader, key_bytes) in [(&view as &dyn ModelRead, 134), (&frozen, 0)] {
        guarded(98, reader.id_page(None, 1).unwrap(), |o| {
            reader.id_page_observed(None, 1, o)
        });
        guarded(key_bytes, true, |o| {
            reader.matches_observed("é", Query::Class("A"), o)
        });
        guarded(
            key_bytes + 98,
            reader.query_candidates(Query::Class("A"), None, 1).unwrap(),
            |o| reader.query_candidates_observed(Query::Class("A"), None, 1, o),
        );
        let first = reader.query_candidates(Query::Class("A"), None, 1).unwrap();
        assert!(!first.exhausted);
        let second = reader
            .query_candidates_observed(
                Query::Class("A"),
                first.after,
                1,
                &InvocationContext::default(),
            )
            .unwrap();
        assert_eq!(second.rows[0].1, "z");
        assert!(second.exhausted);
        assert_eq!(
            reader
                .id_page_observed(Some(u64::MAX), 1, &InvocationContext::new(probe(0, 0)))
                .unwrap_err(),
            "cancelled"
        );
        assert_eq!(
            reader
                .matches_observed(
                    "missing",
                    Query::Class("A"),
                    &InvocationContext::new(probe(0, 0))
                )
                .unwrap_err(),
            "cancelled"
        );
    }
    let mut basic = BasicModel::from_store(
        schema.clone(),
        FrozenStore::with_index_policy(
            frozen.metamodel().clone(),
            IndexPolicy::selected([] as [&str; 0]),
        ),
    )
    .unwrap();
    for id in ["é", "z"] {
        basic.create(id, "A", "r").unwrap();
    }
    let huge = json!(vec![0; 65_537]);
    guarded(
        98,
        basic
            .query_candidates(Query::Value("unindexed", &huge), None, 1)
            .unwrap(),
        |o| basic.query_candidates_observed(Query::Value("unindexed", &huge), None, 1, o),
    );
    let versioned = VersionedModel::new(BasicModel::from_store(schema, frozen).unwrap()).unwrap();
    let snapshot = versioned.snapshot().unwrap();
    guarded(98, snapshot.id_page(None, 1).unwrap(), |o| {
        snapshot.id_page_observed(None, 1, o)
    });
    // Frozen lookahead checks the next posting without retaining its identity.
    guarded(
        98,
        snapshot
            .query_candidates(Query::Class("A"), None, 1)
            .unwrap(),
        |o| snapshot.query_candidates_observed(Query::Class("A"), None, 1, o),
    );
    let fork = versioned.fork(&snapshot).unwrap();
    guarded(98, fork.id_page(None, 1).unwrap(), |o| {
        fork.id_page_observed(None, 1, o)
    });
    assert_eq!(model, before);
}

#[test]
fn stored_scalar_and_collection_pages_preflight_borrowed_values_across_facades() {
    let schema = intersect_model_acceptance::migration_fixture::compile_migrated_schema_fixture(r#"<e:EPackage xmlns:e="http://www.eclipse.org/emf/2002/Ecore" xmlns:x="http://www.w3.org/2001/XMLSchema-instance" name="s" nsURI="urn:value-projection"><eClassifiers x:type="e:EClass" name="A"><eStructuralFeatures x:type="e:EAttribute" name="title" eType="e:EDataType http://www.eclipse.org/emf/2002/Ecore#//EString" defaultValueLiteral="seed"/><eStructuralFeatures x:type="e:EAttribute" name="tags" upperBound="-1" eType="e:EDataType http://www.eclipse.org/emf/2002/Ecore#//EString"/></eClassifiers></e:EPackage>"#).unwrap();
    let mut model = Model::default();
    model.create(&schema, "a", "A", "r", 0, 0).unwrap();
    model.elements[0].values.insert("title".into(), json!("é"));
    model.elements[0]
        .values
        .insert("tags".into(), json!(["é", "z"]));
    let original = model.clone();
    let view = ModelView::new(&model).unwrap();
    let frozen = FrozenStore::from_materialized(&schema, &model).unwrap();
    let basic = BasicModel::from_store(
        schema.clone(),
        FrozenStore::from_materialized(&schema, &model).unwrap(),
    )
    .unwrap();
    let versioned = VersionedModel::new(
        BasicModel::from_store(
            schema.clone(),
            FrozenStore::from_materialized(&schema, &model).unwrap(),
        )
        .unwrap(),
    )
    .unwrap();
    let snapshot = versioned.snapshot().unwrap();
    let mut fork = versioned.fork(&snapshot).unwrap();
    let mut transactional = TransactionalModel::new(
        BasicModel::from_store(
            schema.clone(),
            FrozenStore::from_materialized(&schema, &model).unwrap(),
        )
        .unwrap(),
    );
    for reader in [
        &view as &dyn ModelRead,
        &frozen,
        &basic,
        &snapshot,
        &fork,
        &transactional,
    ] {
        // One retained string node is 128 + UTF-8 length + two metadata bytes.
        guarded(132, Some(json!("é")), |o| {
            reader.value_observed("a", "title", o)
        });
        // Each page adds one 32-byte vector slot before its retained value.
        guarded(164, vec![json!("é")], |o| {
            reader.collection_page_observed("a", "tags", 0, 1, o)
        });
        guarded(163, vec![json!("z")], |o| {
            reader.collection_page_observed("a", "tags", 1, 1, o)
        });
        guarded(327, vec![json!("é"), json!("z")], |o| {
            reader.collection_page_observed("a", "tags", 0, 2, o)
        });
        assert!(
            reader
                .collection_page_observed("a", "title", 0, 1, &InvocationContext::default())
                .is_err()
        );
        assert_eq!(
            reader
                .value_observed("missing", "title", &InvocationContext::new(probe(0, 0)))
                .unwrap_err(),
            "cancelled"
        );
        assert_eq!(
            reader
                .collection_page_observed(
                    "a",
                    "tags",
                    usize::MAX,
                    1,
                    &InvocationContext::new(probe(0, 0))
                )
                .unwrap_err(),
            "cancelled"
        );
        let empty = InvocationContext::new(probe(0, usize::MAX));
        assert_eq!(
            reader.value_observed("missing", "title", &empty).unwrap(),
            None
        );
        assert!(
            reader
                .collection_page_observed("a", "tags", usize::MAX, 1, &empty)
                .unwrap()
                .is_empty()
        );
    }
    assert_eq!(
        transactional
            .transaction::<()>(|session| {
                guarded(132, Some(json!("é")), |o| {
                    session.value_observed("a", "title", o)
                });
                guarded(164, vec![json!("é")], |o| {
                    session.collection_page_observed("a", "tags", 0, 1, o)
                });
                Err("rollback observed read".into())
            })
            .unwrap_err(),
        "rollback observed read"
    );
    fork.set("a", "title", json!("changed")).unwrap();
    fork.set("a", "tags", json!(["X"])).unwrap();
    guarded(137, Some(json!("changed")), |o| {
        fork.value_observed("a", "title", o)
    });
    guarded(163, vec![json!("X")], |o| {
        fork.collection_page_observed("a", "tags", 0, 1, o)
    });
    guarded(132, Some(json!("é")), |o| {
        snapshot.value_observed("a", "title", o)
    });
    let member = schema.effective_feature("A", "title").unwrap().identity;
    let selection =
        storage::StoredFeatureProjection::new(&view, &schema, "A", &member, "a").unwrap();
    guarded(132, json!("é"), |o| selection.singular_observed(o));
    assert_eq!(model, original);
    // Missing stored override uses the same cooked schema default, without an
    // owned class lookup or a different default interpretation.
    drop(view);
    model.elements[0].values.remove("title");
    let view = ModelView::new(&model).unwrap();
    let selection =
        storage::StoredFeatureProjection::new(&view, &schema, "A", &member, "a").unwrap();
    guarded(134, json!("seed"), |o| selection.singular_observed(o));
}

#[test]
fn encoded_json_preflight_preserves_serde_results_and_literal_copy_budgets() {
    // Two parser slots and encoded byte bounds, then shared retained JSON policy.
    guarded(268, json!("é"), |o| {
        storage::decode_json_observed(r#""é""#, o)
    });
    guarded(276, json!("é"), |o| {
        storage::decode_json_observed(r#""\u00e9""#, o)
    });
    guarded(286, json!("é\n\""), |o| {
        storage::decode_json_observed(r#""\u00e9\n\"""#, o)
    });
    guarded(268, serde_json::Value::Null, |o| {
        storage::decode_json_observed("null", o)
    });
    guarded(296, json!(true), |o| {
        storage::decode_json_observed("true", o)
    });
    guarded(290, json!(7), |o| storage::decode_json_observed("7", o));
    let object = r#"{"s":"é","n":true,"items":[null,7]}"#;
    assert_eq!(object.len(), 36);
    // Nine nodes and 89 metadata bytes = 1241, plus 2*(64+36) = 200 scratch.
    guarded(1441, json!({"s":"é","n":true,"items":[null,7]}), |o| {
        storage::decode_json_observed(object, o)
    });
    // Duplicate entries are transient copies too; final semantics stay with serde.
    // Five visited nodes (640), metadata 72 and parser scratch 154 = 866.
    guarded(866, json!({"k":2}), |o| {
        storage::decode_json_observed(r#"{"k":1,"k":2}"#, o)
    });
    for text in [
        "-9223372036854775808",
        "18446744073709551615",
        "-0.0",
        "1.25",
        "[]",
        "{}",
    ] {
        let expected = serde_json::from_str::<serde_json::Value>(text).unwrap();
        let observation = InvocationContext::new(probe(usize::MAX, usize::MAX));
        assert_eq!(
            storage::decode_json_observed(text, &observation).unwrap(),
            expected
        );
    }
    for text in ["", "[", "{\"k\":}", "null true", "1e400"] {
        assert!(
            storage::decode_json_observed(
                text,
                &InvocationContext::new(probe(usize::MAX, usize::MAX))
            )
            .is_err()
        );
        assert!(storage::decode_json_observed(text, &InvocationContext::default()).is_err());
    }
    let deep = format!("{}0{}", "[".repeat(66), "]".repeat(66));
    let wide = serde_json::to_string(&vec![0; 65_537]).unwrap();
    for text in [&deep, &wide] {
        assert!(
            storage::decode_json_observed(
                text,
                &InvocationContext::new(probe(usize::MAX, usize::MAX))
            )
            .is_err()
        );
        assert!(storage::decode_json_observed(text, &InvocationContext::default()).is_ok());
    }
}

#[test]
fn separately_stored_json_items_share_whole_array_node_and_depth_limits() {
    // Array root 130; two child decode budgets 268 and 265 = 663.
    guarded(663, json!(["é", "z"]), |o| {
        let mut budget = storage::JsonDecodeBudget::array(o)?;
        Ok(serde_json::Value::Array(vec![
            budget.decode(r#""é""#, o)?,
            budget.decode(r#""z""#, o)?,
        ]))
    });
    let observation = InvocationContext::new(probe(usize::MAX, usize::MAX));
    let nested = format!("{}0{}", "[".repeat(64), "]".repeat(64));
    assert!(storage::decode_json_observed(&nested, &observation).is_ok());
    let mut array = storage::JsonDecodeBudget::array(&observation).unwrap();
    assert!(
        array
            .decode(&nested, &observation)
            .unwrap_err()
            .contains("depth limit")
    );
    let almost_full = serde_json::to_string(&vec![0; 65_534]).unwrap();
    let mut array = storage::JsonDecodeBudget::array(&observation).unwrap();
    // Outer root + inner array + 65534 atoms consumes all 65536 nodes.
    assert_eq!(
        array
            .decode(&almost_full, &observation)
            .unwrap()
            .as_array()
            .unwrap()
            .len(),
        65_534
    );
    assert!(
        array
            .decode("null", &observation)
            .unwrap_err()
            .contains("node limit")
    );
    let mut compatibility =
        storage::JsonDecodeBudget::array(&InvocationContext::default()).unwrap();
    assert!(
        compatibility
            .decode(&nested, &InvocationContext::default())
            .is_ok()
    );
}
