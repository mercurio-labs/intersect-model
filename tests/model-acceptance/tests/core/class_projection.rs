use intersect_model::core::{
    api::{BasicModel, ModelEdit, TransactionalModel, VersionedModel},
    frozen::FrozenStore,
    model::{Element, Model},
    storage::{
        ModelRead, ModelStore, ModelView, ObjectView, Query, Row, StorageResult,
        StructuralPreparation,
    },
};
use intersect_model::definitions::conversion::{InvocationContext, InvocationObserver, ProviderId};
use serde_json::json;
use std::{cell::Cell, rc::Rc};

fn projected(reader: &impl ModelRead, id: &str, expected: Option<&str>) {
    let revision = reader.revision().unwrap();
    let mut revision_calls = 0;
    reader
        .inspect_revision(&mut |borrowed| {
            revision_calls += 1;
            assert_eq!(borrowed, revision);
            Ok(())
        })
        .unwrap();
    assert_eq!(revision_calls, 1);
    assert_eq!(
        reader
            .inspect_revision(&mut |_| Err("revision callback rejected".into()))
            .unwrap_err(),
        "revision callback rejected"
    );
    let mut calls = 0;
    assert_eq!(
        reader
            .inspect_class(id, &mut |class| {
                calls += 1;
                assert_eq!(Some(class), expected);
                Ok(())
            })
            .unwrap(),
        expected.is_some()
    );
    assert_eq!(calls, usize::from(expected.is_some()));
    if expected.is_some() {
        assert_eq!(
            reader
                .inspect_class(id, &mut |_| Err("callback rejected".into()))
                .unwrap_err(),
            "callback rejected"
        );
    }
}

#[test]
fn class_callbacks_follow_frozen_transactions_and_versioned_forks() {
    let schema = intersect_model_acceptance::migration_fixture::compile_migrated_schema_fixture(r#"<e:EPackage xmlns:e="http://www.eclipse.org/emf/2002/Ecore" xmlns:x="http://www.w3.org/2001/XMLSchema-instance" name="s" nsURI="urn:class-projection"><eClassifiers x:type="e:EClass" name="A"/></e:EPackage>"#).unwrap();
    let mut model = Model::default();
    model.create(&schema, "a", "A", "r", 0, 0).unwrap();
    let view = ModelView::new(&model).unwrap();
    projected(&view, "a", Some("A"));
    projected(&view, "missing", None);
    let mut frozen = FrozenStore::from_materialized(&schema, &model).unwrap();
    projected(&frozen, "a", Some("A"));
    let frame = frozen.begin().unwrap();
    frozen.remove("a").unwrap();
    projected(&frozen, "a", None);
    frozen.rollback(&frame).unwrap();
    projected(&frozen, "a", Some("A"));
    let basic = BasicModel::from_store(schema, frozen).unwrap();
    projected(&basic, "a", Some("A"));
    let mut transactional = TransactionalModel::new(basic);
    projected(&transactional, "a", Some("A"));
    let error = transactional
        .transaction::<()>(|session| {
            session.create("b", "A", "r")?;
            projected(session, "b", Some("A"));
            Err("rollback".into())
        })
        .unwrap_err();
    assert_eq!(error, "rollback");
    projected(&transactional, "b", None);
    let versioned = VersionedModel::new(transactional.into_basic()).unwrap();
    let snapshot = versioned.snapshot().unwrap();
    projected(&snapshot, "a", Some("A"));
    let mut fork = versioned.fork(&snapshot).unwrap();
    fork.create("b", "A", "r").unwrap();
    projected(&fork, "a", Some("A"));
    projected(&fork, "b", Some("A"));
    projected(&snapshot, "b", None);
}

// Structural consumers must use the borrowed class projection. Whole element
// reads and the owned class API intentionally reject in this adapter control.
struct BorrowedOnly<'a>(ModelView<'a>);
impl ModelRead for BorrowedOnly<'_> {
    fn resource_records(
        &self,
    ) -> Result<
        std::collections::BTreeMap<String, intersect_model::core::resources::ResourceRecord>,
        String,
    > {
        self.0.resource_records()
    }
    fn resource_record(
        &self,
        uri: &str,
    ) -> Result<Option<intersect_model::core::resources::ResourceRecord>, String> {
        self.0.resource_record(uri)
    }

    fn revision(&self) -> StorageResult<String> {
        panic!("owned revision lookup")
    }
    fn inspect_revision(&self, f: &mut dyn FnMut(&str) -> StorageResult<()>) -> StorageResult<()> {
        self.0.inspect_revision(f)
    }
    fn get(&self, _: &str) -> StorageResult<Option<Element>> {
        panic!("owned element lookup")
    }
    fn class(&self, _: &str) -> StorageResult<Option<String>> {
        panic!("owned class lookup")
    }
    fn inspect_class(
        &self,
        id: &str,
        f: &mut dyn FnMut(&str) -> StorageResult<()>,
    ) -> StorageResult<bool> {
        self.0.inspect_class(id, f)
    }
    fn page(&self, after: Option<u64>, limit: usize) -> StorageResult<Vec<Row>> {
        self.0.page(after, limit)
    }
    fn query(&self, query: Query<'_>) -> StorageResult<Vec<String>> {
        self.0.query(query)
    }
    fn matches_observed(
        &self,
        id: &str,
        query: intersect_model::core::storage::Query<'_>,
        observation: &intersect_model::core::storage::InvocationContext<'_>,
    ) -> Result<bool, String> {
        self.0.matches_observed(id, query, observation)
    }
    fn query_candidates_observed(
        &self,
        query: intersect_model::core::storage::Query<'_>,
        after: Option<u64>,
        limit: usize,
        observation: &intersect_model::core::storage::InvocationContext<'_>,
    ) -> Result<intersect_model::core::storage::QueryCandidates, String> {
        self.0
            .query_candidates_observed(query, after, limit, observation)
    }
    fn id_page_observed(
        &self,
        after: Option<u64>,
        limit: usize,
        observation: &intersect_model::core::storage::InvocationContext<'_>,
    ) -> Result<Vec<(u64, String)>, String> {
        self.0.id_page_observed(after, limit, observation)
    }
    fn matches(&self, id: &str, query: Query<'_>) -> StorageResult<bool> {
        self.0.matches(id, query)
    }
    fn id_page(&self, after: Option<u64>, limit: usize) -> StorageResult<Vec<(u64, String)>> {
        self.0.id_page(after, limit)
    }
    fn inspect(
        &self,
        id: &str,
        f: &mut dyn FnMut(ObjectView<'_>) -> StorageResult<()>,
    ) -> StorageResult<bool> {
        self.0.inspect(id, f)
    }
}
struct Probe {
    bytes: Cell<usize>,
    checks: Cell<usize>,
    limit: usize,
    stop: usize,
}
impl InvocationObserver for Probe {
    fn before_invoke(&self, _: &ProviderId) -> StorageResult<()> {
        panic!("no provider")
    }
    fn checkpoint(&self) -> StorageResult<()> {
        if self.checks.get() == self.stop {
            return Err("cancelled".into());
        }
        self.checks.set(self.checks.get() + 1);
        Ok(())
    }
    fn retain_storage(&self, bytes: usize) -> StorageResult<()> {
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

#[test]
fn reciprocal_validation_uses_borrowed_classes_and_preserves_terminal_failures() {
    let schema = intersect_model_acceptance::migration_fixture::compile_migrated_schema_fixture(r##"<e:EPackage xmlns:e="http://www.eclipse.org/emf/2002/Ecore" xmlns:x="http://www.w3.org/2001/XMLSchema-instance" name="s" nsURI="urn:reciprocal-projection"><eClassifiers x:type="e:EClass" name="A"><eStructuralFeatures x:type="e:EReference" name="forward" eType="#//B" eOpposite="#//B/back"/></eClassifiers><eClassifiers x:type="e:EClass" name="B"><eStructuralFeatures x:type="e:EReference" name="back" eType="#//A" eOpposite="#//A/forward"/></eClassifiers></e:EPackage>"##).unwrap();
    let mut model = Model::default();
    model.create(&schema, "a", "A", "r", 0, 0).unwrap();
    model.create(&schema, "é", "B", "r", 0, 0).unwrap();
    model.elements[0]
        .values
        .insert("forward".into(), json!("é"));
    model.elements[1].values.insert("back".into(), json!("a"));
    let original = model.clone();
    let view = BorrowedOnly(ModelView::new(&model).unwrap());
    let run = |probe: Rc<Probe>| {
        let observation = InvocationContext::new(probe);
        let mut pending = StructuralPreparation::ownership();
        loop {
            match pending.advance(&view, &schema, None, Some(&observation)) {
                Ok(false) => {}
                Ok(true) => return pending.finish().map(|_| ()),
                Err(error) => {
                    assert!(
                        pending
                            .advance(&view, &schema, None, Some(&observation))
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
        assert_eq!(run(probe(usize::MAX, stop)).unwrap_err(), "cancelled");
    }
    for limit in [0, baseline.bytes.get() - 1] {
        assert_eq!(run(probe(limit, usize::MAX)).unwrap_err(), "storage denied");
    }
    run(probe(baseline.bytes.get(), usize::MAX)).unwrap();
    run(probe(baseline.bytes.get() + 1, usize::MAX)).unwrap();
    assert_eq!(model, original);
    drop(view);
    model.elements[1].values.remove("back");
    let view = BorrowedOnly(ModelView::new(&model).unwrap());
    let mut pending = StructuralPreparation::ownership();
    loop {
        match pending.advance(&view, &schema, None, None) {
            Ok(false) => {}
            Ok(true) => panic!("missing reciprocal accepted"),
            Err(error) => {
                assert!(error.contains("opposite"), "{error}");
                break;
            }
        }
    }
}
