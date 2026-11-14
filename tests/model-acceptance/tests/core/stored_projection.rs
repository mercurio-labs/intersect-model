use intersect_model::core::{
    api::{BasicModel, ModelEdit, VersionedModel},
    storage::{self, ModelRead, ModelStore, StoredFeatureProjection},
};
use serde_json::json;
const SOURCE: &str = include_str!("../../../../fixtures/ecore-navigation/projection.ecore");

#[test]
fn observed_stored_qualification_precharges_inputs_and_cancels_without_selection() {
    use intersect_model::core::storage::StoredFeaturePreparation;
    use intersect_model::definitions::{
        conversion::{InvocationContext, InvocationObserver, ProviderId},
        schema::MemberIdentity,
    };
    use std::{cell::Cell, rc::Rc};
    struct Probe {
        used: Cell<usize>,
        limit: usize,
        checks: Cell<usize>,
        stop: usize,
    }
    impl InvocationObserver for Probe {
        fn before_invoke(&self, _: &ProviderId) -> Result<(), String> {
            panic!("no provider")
        }
        fn checkpoint(&self) -> Result<(), String> {
            if self.checks.get() == self.stop {
                return Err("stored cancelled".into());
            }
            self.checks.set(self.checks.get() + 1);
            Ok(())
        }
        fn retain_storage(&self, bytes: usize) -> Result<(), String> {
            let next = self
                .used
                .get()
                .checked_add(bytes)
                .filter(|n| *n <= self.limit)
                .ok_or("stored storage denied")?;
            self.used.set(next);
            Ok(())
        }
    }
    let probe = |limit, stop| {
        Rc::new(Probe {
            used: Cell::new(0),
            limit,
            checks: Cell::new(0),
            stop,
        })
    };
    let identity = MemberIdentity::Programmatic {
        revision: "r".into(),
        namespace: "n".into(),
        owner: "A".into(),
        name: "f".into(),
    };
    // Tuple + member record + seven string values + five keys: fourteen nodes (1792).
    // Two container delimiters (4), bare struct keys (30), string bytes (32, including the programmatic identity tag).
    for limit in [0, 1857, 1858, 1859] {
        let budget = probe(limit, usize::MAX);
        let observation = InvocationContext::new(budget.clone());
        let result = StoredFeaturePreparation::new_observed("A", &identity, "o", &observation);
        assert_eq!(result.is_ok(), limit >= 1858);
        assert_eq!(budget.used.get(), if limit >= 1858 { 1858 } else { 0 });
    }
    let schema =
        intersect_model_acceptance::migration_fixture::compile_migrated_schema_fixture(SOURCE)
            .unwrap();
    let mut model = BasicModel::new(schema.clone()).unwrap();
    model.create("d", "Diamond", "r").unwrap();
    let member = schema.effective_feature("Base", "label").unwrap().identity;
    let before = storage::export(&model).unwrap();
    let run = |budget: Rc<Probe>| {
        let observation = InvocationContext::new(budget);
        let mut pending =
            StoredFeaturePreparation::new_observed("Base", &member, "d", &observation)?;
        loop {
            match pending.advance_observed(&model, &schema, &observation) {
                Ok(false) => {}
                Ok(true) => {
                    let selected = pending.finish()?;
                    let projection =
                        selected.projection_observed(&model, &schema, "d", &observation)?;
                    assert_eq!(projection.singular()?, json!("seed"));
                    return Ok(());
                }
                Err(error) => {
                    assert!(
                        pending
                            .advance_observed(&model, &schema, &observation)
                            .is_err()
                    );
                    assert!(pending.finish().is_err());
                    return Err(error);
                }
            }
        }
    };
    let baseline = probe(usize::MAX, usize::MAX);
    run(baseline.clone()).unwrap();
    for stop in 0..baseline.checks.get() {
        assert_eq!(
            run(probe(usize::MAX, stop)).unwrap_err(),
            "stored cancelled"
        );
    }
    for limit in [0, baseline.used.get() - 1] {
        assert_eq!(
            run(probe(limit, usize::MAX)).unwrap_err(),
            "stored storage denied"
        );
    }
    run(probe(baseline.used.get(), usize::MAX)).unwrap();
    run(probe(baseline.used.get() + 1, usize::MAX)).unwrap();
    assert_eq!(storage::export(&model).unwrap(), before);
}

#[test]
fn stored_qualification_yields_wide_inventory_and_retains_selection_for_pages() {
    use intersect_model::core::storage::StoredFeaturePreparation;
    let fields = (0..64).map(|n| format!(r##"<eStructuralFeatures x:type="e:EAttribute" name="f{n:02}" eType="e:EDataType http://www.eclipse.org/emf/2002/Ecore#//EString"/>"##)).collect::<String>();
    let schema = intersect_model_acceptance::migration_fixture::compile_migrated_schema_fixture(&format!(r#"<e:EPackage xmlns:e="http://www.eclipse.org/emf/2002/Ecore" xmlns:x="http://www.w3.org/2001/XMLSchema-instance" name="m" nsURI="urn:stored-cursor"><eClassifiers x:type="e:EClass" name="Wide">{fields}</eClassifiers></e:EPackage>"#)).unwrap();
    let mut model = BasicModel::new(schema.clone()).unwrap();
    model.create("wide", "Wide", "r").unwrap();
    model.create("other", "Wide", "r").unwrap();
    model.set("wide", "f63", json!("last")).unwrap();
    let member = schema.effective_feature("Wide", "f63").unwrap().identity;
    assert!(
        StoredFeaturePreparation::new("Wide", &member, "wide")
            .finish()
            .is_err()
    );
    let mut pending = StoredFeaturePreparation::new("Wide", &member, "wide");
    let mut slices = 0;
    loop {
        slices += 1;
        if pending.advance(&model, &schema).unwrap() {
            break;
        }
        assert!(slices < 2000);
    }
    assert!(slices >= 64 * 8);
    let selected = pending.finish().unwrap();
    assert_eq!(
        selected
            .projection(&model, &schema, "wide")
            .unwrap()
            .singular()
            .unwrap(),
        json!("last")
    );
    assert!(selected.projection(&model, &schema, "other").is_err());
    for stop in [0, 1, 64, slices / 2, slices - 1] {
        let mut pending = StoredFeaturePreparation::new("Wide", &member, "wide");
        for _ in 0..stop {
            assert!(!pending.advance(&model, &schema).unwrap());
        }
        assert!(pending.finish().is_err());
    }
    let mut pending = StoredFeaturePreparation::new("Wide", &member, "missing");
    loop {
        match pending.advance(&model, &schema) {
            Ok(false) => {}
            Ok(true) => panic!("missing object accepted"),
            Err(error) => {
                assert_eq!(error, "Missing feature receiver object");
                assert_eq!(
                    pending.advance(&model, &schema).unwrap_err(),
                    "Stored feature preparation has failed"
                );
                break;
            }
        }
    }
    assert!(pending.finish().is_err());
}

#[test]
fn exact_projection_preserves_diamond_identity_and_rejects_foreign_members() {
    let schema =
        intersect_model_acceptance::migration_fixture::compile_migrated_schema_fixture(SOURCE)
            .unwrap();
    let label = schema.effective_feature("Base", "label").unwrap().identity;
    assert_eq!(
        label,
        schema
            .effective_feature("Diamond", "label")
            .unwrap()
            .identity
    );
    let mut model = BasicModel::new(schema.clone()).unwrap();
    model.create("d", "Diamond", "r").unwrap();
    model.create("other", "Other", "r").unwrap();
    let read = StoredFeatureProjection::new(&model, &schema, "Base", &label, "d").unwrap();
    assert_eq!(read.identity(), &label);
    assert_eq!(read.singular().unwrap(), json!("seed"));
    assert!(read.collection_page(0, 1).is_err());
    let foreign = schema.effective_feature("Other", "label").unwrap().identity;
    assert!(StoredFeatureProjection::new(&model, &schema, "Base", &foreign, "d").is_err());
    assert!(StoredFeatureProjection::new(&model, &schema, "Base", &label, "other").is_err());
    assert!(StoredFeatureProjection::new(&model, &schema, "Base", &label, "missing").is_err());
    let derived = schema
        .effective_feature("Base", "computed")
        .unwrap()
        .identity;
    assert!(StoredFeatureProjection::new(&model, &schema, "Base", &derived, "d").is_err());
    let stale = intersect_model_acceptance::migration_fixture::compile_migrated_schema_fixture(
        &SOURCE.replace("seed", "changed"),
    )
    .unwrap();
    assert!(StoredFeatureProjection::new(&model, &stale, "Base", &label, "d").is_err());
    let mut raw = model.into_store();
    storage::atomic(&mut raw, |s| s.set_value("d", "label", None)).unwrap();
    assert_eq!(
        StoredFeatureProjection::new(&raw, &schema, "Base", &label, "d")
            .unwrap()
            .singular()
            .unwrap(),
        json!("seed")
    );
    storage::atomic(&mut raw, |s| s.set_value("d", "label", Some(json!(null)))).unwrap();
    assert_eq!(
        StoredFeatureProjection::new(&raw, &schema, "Base", &label, "d")
            .unwrap()
            .singular()
            .unwrap(),
        json!(null)
    );
    let nullable = intersect_model_acceptance::migration_fixture::compile_migrated_schema_fixture(
        &SOURCE.replace(" defaultValueLiteral=\"seed\"", ""),
    )
    .unwrap();
    let member = nullable
        .effective_feature("Base", "label")
        .unwrap()
        .identity;
    let mut empty = BasicModel::new(nullable.clone()).unwrap();
    empty.create("d", "Diamond", "r").unwrap();
    let mut empty = empty.into_store();
    storage::atomic(&mut empty, |s| s.set_value("d", "label", None)).unwrap();
    assert_eq!(
        StoredFeatureProjection::new(&empty, &nullable, "Base", &member, "d")
            .unwrap()
            .singular()
            .unwrap(),
        json!(null)
    );
}

#[test]
fn projected_pages_preserve_order_and_pinned_snapshot_values() {
    let schema =
        intersect_model_acceptance::migration_fixture::compile_migrated_schema_fixture(SOURCE)
            .unwrap();
    let member = schema
        .effective_feature("Base", "children")
        .unwrap()
        .identity;
    let mut basic = BasicModel::new(schema.clone()).unwrap();
    for id in ["owner", "a", "b", "c"] {
        basic.create(id, "Diamond", "r").unwrap();
    }
    basic
        .set("owner", "children", json!(["c", "a", "b"]))
        .unwrap();
    let mut model = VersionedModel::new(basic).unwrap();
    let old = model.snapshot().unwrap();
    let read = StoredFeatureProjection::new(&old, &schema, "Base", &member, "owner").unwrap();
    assert!(read.singular().is_err());
    assert!(read.collection_page(0, 0).is_err());
    assert_eq!(read.collection_page(1, 1).unwrap(), [json!("a")]);
    assert_eq!(read.collection_page(2, 100).unwrap(), [json!("b")]);
    assert!(read.collection_page(usize::MAX, 1).unwrap().is_empty());
    let mut fork = model.fork(&old).unwrap();
    fork.set("owner", "children", json!(["b"])).unwrap();
    assert_eq!(
        StoredFeatureProjection::new(&fork, &schema, "Base", &member, "owner")
            .unwrap()
            .collection_page(0, 2)
            .unwrap(),
        [json!("b")]
    );
    model.commit(fork).unwrap();
    assert_eq!(
        read.collection_page(0, 3).unwrap(),
        [json!("c"), json!("a"), json!("b")]
    );
    assert_eq!(old.class("owner").unwrap().as_deref(), Some("Diamond"));
}

#[test]
fn owned_and_borrowed_indexed_views_share_zero_based_cursors_and_projected_reads() {
    use intersect_model::core::storage::{ModelView, OwnedModelView, Query};
    let schema =
        intersect_model_acceptance::migration_fixture::compile_migrated_schema_fixture(SOURCE)
            .unwrap();
    let mut basic = BasicModel::new(schema.clone()).unwrap();
    for id in ["z", "a", "b"] {
        basic.create(id, "Diamond", "r").unwrap();
    }
    basic.set("z", "children", json!(["b", "a"])).unwrap();
    let model = storage::export(&basic).unwrap();
    let borrowed = ModelView::new(&model).unwrap();
    assert!(std::ptr::eq(borrowed.model(), &model));
    let owned = OwnedModelView::new(model.clone()).unwrap();
    for read in [&borrowed as &dyn ModelRead, &owned as &dyn ModelRead] {
        let first = read
            .query_candidates(Query::Class("Diamond"), None, 1)
            .unwrap();
        assert!(first.exact && !first.exhausted);
        assert_eq!(first.rows, [(0, "z".into())]);
        let next = read
            .query_candidates(Query::Class("Diamond"), first.after, 1)
            .unwrap();
        assert_eq!(next.rows, [(1, "a".into())]);
        assert!(
            read.query_candidates(Query::Class("Diamond"), Some(u64::MAX), 1)
                .unwrap()
                .exhausted
        );
        assert!(
            read.query_candidates(Query::Class("Diamond"), None, 0)
                .is_err()
        );
        assert_eq!(
            read.collection_page("z", "children", 1, 1).unwrap(),
            [json!("a")]
        );
        assert!(
            read.collection_page("z", "children", usize::MAX, 1)
                .unwrap()
                .is_empty()
        );
        assert!(read.collection_page("z", "label", 0, 1).is_err());
        assert!(read.collection_page("missing", "children", 0, 1).is_err());
    }
}
