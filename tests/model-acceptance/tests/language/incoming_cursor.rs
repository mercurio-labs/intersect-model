use intersect_model::core::{
    api::{BasicModel, ModelEdit, VersionedModel},
    frozen::{FrozenStore, Metamodel},
    storage::{IndexPolicy, ModelRead},
};
use intersect_model::definitions::schema::Schema;
use intersect_model::navigation::{IncomingCursor, IncomingStep};
use serde_json::json;
const SOURCE: &str = include_str!("../../../../fixtures/ecore-navigation/inverse.ecore");

struct BorrowedClasses<'a> {
    read: &'a dyn ModelRead,
    inexact_candidates: bool,
}
impl ModelRead for BorrowedClasses<'_> {
    fn resource_records(
        &self,
    ) -> Result<
        std::collections::BTreeMap<String, intersect_model::core::resources::ResourceRecord>,
        String,
    > {
        self.read.resource_records()
    }
    fn resource_record(
        &self,
        uri: &str,
    ) -> Result<Option<intersect_model::core::resources::ResourceRecord>, String> {
        self.read.resource_record(uri)
    }

    fn revision(&self) -> Result<String, String> {
        panic!("owned revision lookup")
    }
    fn inspect_revision(
        &self,
        f: &mut dyn FnMut(&str) -> Result<(), String>,
    ) -> Result<(), String> {
        self.read.inspect_revision(f)
    }
    fn get(&self, _: &str) -> Result<Option<intersect_model::core::model::Element>, String> {
        panic!("owned element lookup")
    }
    fn class(&self, _: &str) -> Result<Option<String>, String> {
        panic!("owned class lookup")
    }
    fn inspect_class(
        &self,
        id: &str,
        f: &mut dyn FnMut(&str) -> Result<(), String>,
    ) -> Result<bool, String> {
        self.read.inspect_class(id, f)
    }
    fn page(
        &self,
        after: Option<u64>,
        limit: usize,
    ) -> Result<Vec<intersect_model::core::storage::Row>, String> {
        self.read.page(after, limit)
    }
    fn query(
        &self,
        query: intersect_model::core::storage::Query<'_>,
    ) -> Result<Vec<String>, String> {
        self.read.query(query)
    }
    fn query_candidates(
        &self,
        query: intersect_model::core::storage::Query<'_>,
        after: Option<u64>,
        limit: usize,
    ) -> Result<intersect_model::core::storage::QueryCandidates, String> {
        self.read.query_candidates(query, after, limit)
    }
    fn matches_observed(
        &self,
        id: &str,
        query: intersect_model::core::storage::Query<'_>,
        observation: &intersect_model::core::storage::InvocationContext<'_>,
    ) -> Result<bool, String> {
        self.read.matches_observed(id, query, observation)
    }
    fn query_candidates_observed(
        &self,
        query: intersect_model::core::storage::Query<'_>,
        after: Option<u64>,
        limit: usize,
        observation: &intersect_model::core::storage::InvocationContext<'_>,
    ) -> Result<intersect_model::core::storage::QueryCandidates, String> {
        let mut batch = self
            .read
            .query_candidates_observed(query, after, limit, observation)?;
        batch.exact &= !self.inexact_candidates;
        Ok(batch)
    }
    fn id_page_observed(
        &self,
        after: Option<u64>,
        limit: usize,
        observation: &intersect_model::core::storage::InvocationContext<'_>,
    ) -> Result<Vec<(u64, String)>, String> {
        self.read.id_page_observed(after, limit, observation)
    }
    fn matches(
        &self,
        id: &str,
        query: intersect_model::core::storage::Query<'_>,
    ) -> Result<bool, String> {
        self.read.matches(id, query)
    }
    fn value_observed(
        &self,
        id: &str,
        field: &str,
        observation: &intersect_model::core::storage::InvocationContext<'_>,
    ) -> Result<Option<serde_json::Value>, String> {
        self.read.value_observed(id, field, observation)
    }
    fn collection_page_observed(
        &self,
        id: &str,
        field: &str,
        offset: usize,
        limit: usize,
        observation: &intersect_model::core::storage::InvocationContext<'_>,
    ) -> Result<Vec<serde_json::Value>, String> {
        self.read
            .collection_page_observed(id, field, offset, limit, observation)
    }
    fn value(&self, id: &str, field: &str) -> Result<Option<serde_json::Value>, String> {
        self.read.value(id, field)
    }
    fn value_len(&self, id: &str, field: &str) -> Result<Option<usize>, String> {
        self.read.value_len(id, field)
    }
    fn collection_page(
        &self,
        id: &str,
        field: &str,
        offset: usize,
        limit: usize,
    ) -> Result<Vec<serde_json::Value>, String> {
        self.read.collection_page(id, field, offset, limit)
    }
}

#[test]
fn observed_inverse_setup_and_candidates_have_exact_storage_and_terminal_cancellation() {
    use intersect_model::definitions::{
        conversion::{InvocationContext, InvocationObserver, ProviderId},
        schema::MemberIdentity,
    };
    use intersect_model::navigation::IncomingPreparation;
    use std::{cell::Cell, rc::Rc};
    struct Probe {
        bytes: Cell<usize>,
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
                return Err("inverse cancelled".into());
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
                .ok_or("inverse storage denied")?;
            self.bytes.set(next);
            Ok(())
        }
    }
    let probe = |limit, stop| {
        Rc::new(Probe {
            bytes: Cell::new(0),
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
    // Fourteen nodes (1792), thirty bare key bytes, thirty-two string bytes,
    // and four container bytes: 1858. This is constructor policy, not admission.
    for limit in [0, 1857, 1858, 1859] {
        let budget = probe(limit, usize::MAX);
        let observation = InvocationContext::new(budget.clone());
        assert_eq!(
            IncomingPreparation::new_observed("A", &identity, "o", &observation).is_ok(),
            limit >= 1858
        );
        assert_eq!(budget.bytes.get(), if limit >= 1858 { 1858 } else { 0 });
    }
    let schema =
        intersect_model_acceptance::migration_fixture::compile_migrated_schema_fixture(SOURCE)
            .unwrap();
    let member = schema.effective_feature("Node", "links").unwrap().identity;
    for store in [
        FrozenStore::for_schema(&schema).unwrap(),
        unindexed(&schema),
    ] {
        let mut model = BasicModel::from_store(schema.clone(), store).unwrap();
        populate(&mut model);
        let before = intersect_model::core::storage::export(&model).unwrap();
        let view = BorrowedClasses {
            read: &model,
            inexact_candidates: false,
        };
        let run = |budget: Rc<Probe>| {
            let observation = InvocationContext::new(budget);
            let mut setup =
                IncomingPreparation::new_observed("Node", &member, "target", &observation)?;
            loop {
                match setup.advance_observed(&view, &schema, &observation) {
                    Ok(false) => {}
                    Ok(true) => break,
                    Err(error) => {
                        assert!(
                            setup
                                .advance_observed(&view, &schema, &observation)
                                .is_err()
                        );
                        assert!(setup.finish().is_err());
                        return Err(error);
                    }
                }
            }
            let mut cursor = setup.finish()?;
            let mut output = Vec::new();
            loop {
                match cursor.advance_observed(&view, &schema, &observation) {
                    Ok(IncomingStep::Pending) => {}
                    Ok(IncomingStep::Match(id)) => output.push(id),
                    Ok(IncomingStep::Done) => return Ok(output),
                    Err(error) => {
                        assert!(
                            cursor
                                .advance_observed(&view, &schema, &observation)
                                .is_err()
                        );
                        return Err(error);
                    }
                }
            }
        };
        let baseline = probe(usize::MAX, usize::MAX);
        assert_eq!(run(baseline.clone()).unwrap(), ["z", "a"]);
        for stop in 0..baseline.checks.get() {
            assert_eq!(
                run(probe(usize::MAX, stop)).unwrap_err(),
                "inverse cancelled"
            );
        }
        for limit in [0, baseline.bytes.get() - 1] {
            assert_eq!(
                run(probe(limit, usize::MAX)).unwrap_err(),
                "inverse storage denied"
            );
        }
        assert_eq!(
            run(probe(baseline.bytes.get(), usize::MAX)).unwrap(),
            ["z", "a"]
        );
        assert_eq!(
            run(probe(baseline.bytes.get() + 1, usize::MAX)).unwrap(),
            ["z", "a"]
        );
        assert_eq!(
            intersect_model::core::storage::export(&model).unwrap(),
            before
        );
    }
}

fn collect(store: &dyn ModelRead, schema: &Schema, field: &str) -> (Vec<String>, usize) {
    let member = schema.effective_feature("Node", field).unwrap().identity;
    let mut cursor = IncomingCursor::new(store, schema, "Node", member, "target").unwrap();
    let mut output = Vec::new();
    for steps in 1..1000 {
        match cursor.advance(store, schema).unwrap() {
            IncomingStep::Pending => (),
            IncomingStep::Match(id) => output.push(id),
            IncomingStep::Done => {
                assert_eq!(cursor.advance(store, schema).unwrap(), IncomingStep::Done);
                return (output, steps);
            }
        }
    }
    panic!("cursor did not terminate")
}
fn populate(model: &mut dyn ModelEdit) {
    for (id, class) in [
        ("target", "Node"),
        ("foreign", "Other"),
        ("z", "Child"),
        ("a", "Node"),
        ("empty", "Node"),
    ] {
        model.create(id, class, "r").unwrap();
    }
    for id in ["foreign", "z", "a"] {
        model
            .set(id, "links", json!(["empty", "target", "target"]))
            .unwrap();
    }
    model.set("a", "peer", json!("target")).unwrap();
}
#[test]
fn indexed_and_unindexed_results_preserve_row_order_and_exact_source_identity() {
    let schema =
        intersect_model_acceptance::migration_fixture::compile_migrated_schema_fixture(SOURCE)
            .unwrap();
    let mut steps = Vec::new();
    for store in [
        FrozenStore::for_schema(&schema).unwrap(),
        unindexed(&schema),
    ] {
        let mut model = BasicModel::from_store(schema.clone(), store).unwrap();
        populate(&mut model);
        let (ids, count) = collect(&model, &schema, "links");
        assert_eq!(ids, ["z", "a"]);
        assert_eq!(collect(&model, &schema, "peer").0, ["a"]);
        steps.push(count);
        for field in ["label", "computed"] {
            let member = schema.effective_feature("Node", field).unwrap().identity;
            assert!(IncomingCursor::new(&model, &schema, "Node", member, "target").is_err());
        }
        let member = schema.effective_feature("Node", "links").unwrap().identity;
        assert!(IncomingCursor::new(&model, &schema, "Node", member.clone(), "foreign").is_err());
        assert!(IncomingCursor::new(&model, &schema, "Node", member.clone(), "absent").is_err());
        assert!(IncomingCursor::new(&model, &schema, "Other", member, "target").is_err());
    }
    assert!(
        steps[1] > steps[0],
        "unindexed edges must be independently resumable"
    );
}
#[test]
fn versioned_queries_observe_fork_changes_and_preserve_old_snapshot() {
    let schema =
        intersect_model_acceptance::migration_fixture::compile_migrated_schema_fixture(SOURCE)
            .unwrap();
    let mut basic = BasicModel::new(schema.clone()).unwrap();
    populate(&mut basic);
    let mut versioned = VersionedModel::new(basic).unwrap();
    let old = versioned.snapshot().unwrap();
    let mut fork = versioned.fork(&old).unwrap();
    fork.set("z", "links", json!([])).unwrap();
    fork.set("empty", "links", json!(["target"])).unwrap();
    assert_eq!(collect(&fork, &schema, "links").0, ["a", "empty"]);
    versioned.commit(fork).unwrap();
    assert_eq!(collect(&old, &schema, "links").0, ["z", "a"]);
    assert_eq!(
        collect(&versioned.snapshot().unwrap(), &schema, "links").0,
        ["a", "empty"]
    );
}

fn unindexed(schema: &intersect_model::definitions::schema::Schema) -> FrozenStore {
    FrozenStore::with_index_policy(
        std::sync::Arc::new(Metamodel::freeze(schema).unwrap()),
        IndexPolicy::selected([] as [&str; 0]),
    )
}

#[test]
fn raw_malformed_edges_and_stale_schema_are_explicit_errors() {
    use intersect_model::core::storage::{self, ModelStore, ModelView};
    let schema =
        intersect_model_acceptance::migration_fixture::compile_migrated_schema_fixture(SOURCE)
            .unwrap();
    let mut model = BasicModel::from_store(schema.clone(), unindexed(&schema)).unwrap();
    model.create("target", "Node", "r").unwrap();
    let member = schema.effective_feature("Node", "links").unwrap().identity;
    let mut raw = model.into_store();
    let mut wire = storage::export(&raw).unwrap();
    assert!(
        storage::atomic(&mut raw, |s| {
            s.set_value("target", "links", Some(json!([42])))
        })
        .is_err()
    );
    assert_eq!(storage::export(&raw).unwrap(), wire);
    wire.elements[0]
        .values
        .insert("links".into(), json!([42, "target"]));
    let wire_view = ModelView::new(&wire).unwrap();
    let raw = BorrowedClasses {
        read: &wire_view,
        inexact_candidates: true,
    };
    let mut cursor = IncomingCursor::new(&raw, &schema, "Node", member.clone(), "target").unwrap();
    assert_eq!(
        cursor.advance(&raw, &schema).unwrap(),
        IncomingStep::Pending
    );
    let error = loop {
        match cursor.advance(&raw, &schema) {
            Ok(IncomingStep::Pending) => (),
            Ok(other) => panic!("malformed edge published {other:?}"),
            Err(error) => break error,
        }
    };
    assert!(error.contains("object identity"));
    assert_eq!(
        cursor.advance(&raw, &schema).unwrap_err(),
        "Inverse cursor has failed"
    );
    let stale = intersect_model_acceptance::migration_fixture::compile_migrated_schema_fixture(
        &SOURCE.replace("urn:intersect:inverse", "urn:intersect:changed"),
    )
    .unwrap();
    let mut cursor = IncomingCursor::new(&raw, &schema, "Node", member, "target").unwrap();
    assert!(
        cursor
            .advance(&raw, &stale)
            .unwrap_err()
            .contains("revision")
    );
}

#[test]
fn wide_inverse_setup_and_exact_index_hits_yield_full_qualification() {
    use intersect_model::navigation::IncomingPreparation;
    let mut fields = String::new();
    for n in 0..63 {
        fields.push_str(&format!(r#"<eStructuralFeatures x:type="e:EAttribute" name="f{n:02}" eType="e:EDataType http://www.eclipse.org/emf/2002/Ecore#//EString"/>"#));
    }
    fields.push_str(r##"<eStructuralFeatures x:type="e:EReference" name="links" upperBound="-1" unique="false" eType="#//Wide"/>"##);
    let source = format!(
        r#"<e:EPackage xmlns:e="http://www.eclipse.org/emf/2002/Ecore" xmlns:x="http://www.w3.org/2001/XMLSchema-instance" name="m" nsURI="urn:wide-inverse"><eClassifiers x:type="e:EClass" name="Wide">{fields}</eClassifiers></e:EPackage>"#
    );
    let schema =
        intersect_model_acceptance::migration_fixture::compile_migrated_schema_fixture(&source)
            .unwrap();
    for store in [
        FrozenStore::for_schema(&schema).unwrap(),
        unindexed(&schema),
    ] {
        let mut model = BasicModel::from_store(schema.clone(), store).unwrap();
        model.create("target", "Wide", "r").unwrap();
        model.create("source", "Wide", "r").unwrap();
        model
            .set("source", "links", json!(["target", "target"]))
            .unwrap();
        let member = schema.effective_feature("Wide", "links").unwrap().identity;
        assert!(
            IncomingPreparation::new("Wide", member.clone(), "target")
                .finish()
                .is_err()
        );
        let mut preparation = IncomingPreparation::new("Wide", member.clone(), "target");
        let mut setup = 0;
        while !preparation.advance(&model, &schema).unwrap() {
            setup += 1;
            assert!(setup < 10000);
        }
        assert!(setup >= 64 * 8);
        for stop in [0, 1, 64, setup / 2, setup - 1] {
            let mut cancelled = IncomingPreparation::new("Wide", member.clone(), "target");
            for _ in 0..stop {
                assert!(!cancelled.advance(&model, &schema).unwrap());
            }
            assert!(cancelled.finish().is_err());
        }
        let mut cursor = preparation.finish().unwrap();
        let mut ids = Vec::new();
        let mut advances = 0;
        loop {
            advances += 1;
            assert!(advances < 10000);
            match cursor.advance(&model, &schema).unwrap() {
                IncomingStep::Pending => (),
                IncomingStep::Match(id) => ids.push(id),
                IncomingStep::Done => break,
            }
        }
        assert_eq!(ids, ["source"]);
        assert!(advances >= 64 * 8);
        assert_eq!(cursor.advance(&model, &schema).unwrap(), IncomingStep::Done);
        let mut invalid = IncomingPreparation::new("Wide", member, "absent");
        loop {
            match invalid.advance(&model, &schema) {
                Ok(false) => (),
                Ok(true) => panic!("missing inverse target accepted"),
                Err(error) => {
                    assert_eq!(error, "Missing inverse query target");
                    break;
                }
            }
        }
        assert_eq!(
            invalid.advance(&model, &schema).unwrap_err(),
            "Inverse preparation has failed"
        );
        assert!(invalid.finish().is_err());
    }
}
