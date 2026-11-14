use crate::stores::StoreFixture;
use intersect_model::core::{
    api::{BasicModel, ModelEdit, TransactionalModel, VersionedModel},
    frozen::FrozenStore,
    model::Element,
    storage::{self, Checkpoint, ModelRead, ModelStore, ObjectView, Query, Row, StorageResult},
};
use intersect_model::definitions::schema::Schema;
use serde_json::{Value, json};
use std::{cell::RefCell, collections::BTreeSet, rc::Rc};

fn schema() -> Schema {
    let source = include_str!("../../../../fixtures/library/library.ecore").replace(
        "<eStructuralFeatures xsi:type=\"ecore:EAttribute\" name=\"title\"",
        "<eStructuralFeatures xsi:type=\"ecore:EAttribute\" name=\"tags\" upperBound=\"2\" eType=\"ecore:EDataType http://www.eclipse.org/emf/2002/Ecore#//EString\"/>\n    <eStructuralFeatures xsi:type=\"ecore:EAttribute\" name=\"title\"",
    );
    intersect_model_acceptance::migration_fixture::compile_migrated_schema_fixture(&source).unwrap()
}

// This is the previous facade algorithm, deliberately using the public complete
// draft validator rather than any incremental implementation or private helper.
fn full_create(
    store: &mut FrozenStore,
    schema: &Schema,
    id: &str,
    class: &str,
) -> StorageResult<()> {
    storage::atomic(store, |store| {
        store.create_element(schema, id, class, "model", 0, 0)?;
        storage::validate_draft(store, schema)
    })
}

fn full_set(
    store: &mut FrozenStore,
    schema: &Schema,
    edits: &[(&str, &str, Value)],
) -> StorageResult<()> {
    storage::atomic(store, |store| {
        for (id, feature, value) in edits {
            storage::set_with_opposite(store, schema, id, feature, value.clone())?;
        }
        storage::validate_draft(store, schema)
    })
}

fn oracle(schema: &Schema) -> FrozenStore {
    let mut store = FrozenStore::for_schema(schema).unwrap();
    storage::atomic(&mut store, |store| storage::bind_schema(store, schema)).unwrap();
    store
}

fn assert_equivalent(model: &impl ModelRead, oracle: &FrozenStore) {
    assert_eq!(
        storage::export(model).unwrap(),
        storage::export(oracle).unwrap()
    );
}

#[test]
fn checked_operations_match_complete_draft_validation_and_rollback() {
    let schema = schema();
    let mut model = BasicModel::new(schema.clone()).unwrap();
    let mut oracle = oracle(&schema);
    for (id, class) in [
        ("library", "Library"),
        ("branch", "Library"),
        ("copy", "BookCopy"),
        ("reader", "Member"),
        ("loan", "Loan"),
        ("copy", "BookCopy"),
        ("", "Member"),
        ("abstract", "Named"),
        ("unknown", "Missing"),
    ] {
        let expected = full_create(&mut oracle, &schema, id, class);
        let actual = model.create(id, class, "model");
        assert_eq!(
            actual.is_ok(),
            expected.is_ok(),
            "create {id}: {actual:?}, {expected:?}"
        );
        assert_equivalent(&model, &oracle);
    }
    let batches = [
        vec![
            ("copy", "name", json!("Dune")),
            ("copy", "edition", json!(2)),
        ],
        vec![("copy", "edition", json!("invalid"))],
        vec![("copy", "edition", json!(2147483648_i64))],
        vec![("copy", "edition", json!([1]))],
        vec![("copy", "title", json!(null))],
        vec![("copy", "tags", json!(["science", "fiction"]))],
        vec![("copy", "tags", json!(["same", "same"]))],
        vec![("copy", "tags", json!(["one", "two", "three"]))],
        vec![("copy", "tags", json!(null))],
        vec![("copy", "tags", json!([null]))],
        vec![("copy", "tags", json!([3]))],
        vec![("copy", "available", json!(true))],
        vec![("copy", "unknown", json!(true))],
        vec![("missing", "name", json!("absent"))],
        vec![
            ("copy", "title", json!("rolled back")),
            ("copy", "tags", json!(["x", "x"])),
        ],
        vec![("library", "copies", json!(["copy"]))],
        vec![("branch", "copies", json!(["copy"]))],
        vec![
            ("library", "copies", json!([])),
            ("branch", "copies", json!(["copy"])),
        ],
        vec![("library", "members", json!(["copy"]))],
        vec![("library", "members", json!(["missing"]))],
        vec![("library", "members", json!(["reader", "reader"]))],
        vec![("library", "members", json!(["reader"]))],
        vec![("branch", "members", json!(["reader"]))],
        vec![
            ("loan", "copy", json!("copy")),
            ("loan", "member", json!("reader")),
        ],
        vec![],
    ];
    for edits in batches {
        let expected = full_set(&mut oracle, &schema, &edits);
        let actual = model.set_many(&edits);
        assert_eq!(
            actual.is_ok(),
            expected.is_ok(),
            "{edits:?}: {actual:?}, {expected:?}"
        );
        assert_equivalent(&model, &oracle);
        storage::validate_draft(&model, &schema).unwrap();
    }
}

#[test]
fn one_batch_checks_its_final_attribute_values() {
    let mut schema = schema();
    // Legacy scalar declarations have no eager datatype conversion. Their
    // structural type checks historically happen at the end of set_many.
    schema.runtime_feature_domains.clear();
    schema
        .classes
        .get_mut("BookCopy")
        .unwrap()
        .features
        .get_mut("edition")
        .unwrap()
        .datatype = None;
    let mut model = BasicModel::new(schema.clone()).unwrap();
    let mut oracle = oracle(&schema);
    model.create("copy", "BookCopy", "model").unwrap();
    full_create(&mut oracle, &schema, "copy", "BookCopy").unwrap();
    let edits = [
        ("copy", "edition", json!("temporary invalid value")),
        ("copy", "edition", json!(7)),
    ];
    full_set(&mut oracle, &schema, &edits).unwrap();
    model.set_many(&edits).unwrap();
    assert_equivalent(&model, &oracle);
    let edits = [
        ("copy", "tags", json!(["x", "x"])),
        ("copy", "tags", json!(["unique"])),
    ];
    full_set(&mut oracle, &schema, &edits).unwrap();
    model.set_many(&edits).unwrap();
    assert_equivalent(&model, &oracle);
}

#[test]
fn failed_operations_preserve_prior_transaction_edits_and_the_transaction_can_continue() {
    let mut model = TransactionalModel::new(BasicModel::new(schema()).unwrap());
    model
        .transaction(|edit| {
            edit.create("copy", "BookCopy", "model")?;
            edit.set("copy", "name", json!("committed"))?;
            let before = storage::export(edit)?;
            assert!(
                edit.set_many(&[
                    ("copy", "edition", json!(8)),
                    ("copy", "tags", json!(["x", "x"]))
                ])
                .is_err()
            );
            assert_eq!(storage::export(edit)?, before);
            assert!(edit.create("copy", "BookCopy", "model").is_err());
            assert_eq!(storage::export(edit)?, before);
            edit.set("copy", "edition", json!(9))
        })
        .unwrap();
    assert_eq!(
        model.value("copy", "name").unwrap(),
        Some(json!("committed"))
    );
    assert_eq!(model.value("copy", "edition").unwrap(), Some(json!(9)));
    model.validate().unwrap();
}

#[test]
fn commit_still_rejects_untouched_incomplete_objects() {
    let mut basic = BasicModel::new(schema()).unwrap();
    basic.create("incomplete", "Member", "model").unwrap();
    basic.create("copy", "BookCopy", "model").unwrap();
    basic.set("copy", "name", json!("complete")).unwrap();
    let before = storage::export(&basic).unwrap();
    let mut model = TransactionalModel::new(basic);
    assert!(
        model
            .transaction(|edit| edit.set("copy", "edition", json!(2)))
            .is_err()
    );
    assert_eq!(storage::export(&model).unwrap(), before);
    assert!(model.transaction(|_| Ok(())).is_err());
    model
        .transaction(|edit| edit.set("incomplete", "name", json!("completed")))
        .unwrap();
    model.validate().unwrap();
}

#[test]
fn versioned_forks_keep_checked_edits_and_full_publication_validation() {
    let mut basic = BasicModel::new(schema()).unwrap();
    basic.create("copy", "BookCopy", "model").unwrap();
    basic.set("copy", "name", json!("original")).unwrap();
    let mut model = VersionedModel::new(basic).unwrap();
    let original = model.snapshot().unwrap();
    let mut fork = model.fork(&original).unwrap();
    fork.set("copy", "edition", json!(2)).unwrap();
    let before = storage::export(&fork).unwrap();
    assert!(
        fork.set_many(&[
            ("copy", "title", json!("rejected")),
            ("copy", "tags", json!(["x", "x"]))
        ])
        .is_err()
    );
    assert_eq!(storage::export(&fork).unwrap(), before);
    fork.create("incomplete", "Member", "model").unwrap();
    assert!(model.commit(fork).is_err());
    assert_eq!(model.snapshot().unwrap().version(), original.version());
    let mut fork = model.fork(&original).unwrap();
    fork.set("copy", "tags", json!(["new"])).unwrap();
    model.commit(fork).unwrap();
    assert_eq!(
        model.snapshot().unwrap().value("copy", "tags").unwrap(),
        Some(json!(["new"]))
    );
    assert_eq!(original.value("copy", "tags").unwrap(), Some(json!([])));
}

#[test]
fn creation_checks_invalid_attribute_and_reference_defaults_against_full_validation() {
    for (class, feature, value) in [
        ("BookCopy", "edition", json!("bad default")),
        ("BookCopy", "tags", json!(["duplicate", "duplicate"])),
        ("Loan", "member", json!("missing")),
        ("BookCopy", "library", json!("missing")),
    ] {
        let mut schema = schema();
        schema
            .classes
            .get_mut(class)
            .unwrap()
            .features
            .get_mut(feature)
            .unwrap()
            .default = value;
        let mut model = BasicModel::new(schema.clone()).unwrap();
        let mut oracle = oracle(&schema);
        let expected = full_create(&mut oracle, &schema, "invalid", class);
        let actual = model.create("invalid", class, "model");
        assert!(expected.is_err(), "{class}.{feature}");
        assert_eq!(actual.is_ok(), expected.is_ok());
        assert_equivalent(&model, &oracle);
    }
}

#[test]
fn populated_containment_defaults_cannot_claim_an_existing_owner() {
    let mut schema = schema();
    schema
        .classes
        .get_mut("Library")
        .unwrap()
        .features
        .get_mut("members")
        .unwrap()
        .default = json!(["reader"]);
    let mut model = BasicModel::new(schema.clone()).unwrap();
    let mut oracle = oracle(&schema);
    for (id, class, succeeds) in [
        ("reader", "Member", true),
        ("library", "Library", true),
        ("branch", "Library", false),
    ] {
        let expected = full_create(&mut oracle, &schema, id, class);
        let actual = model.create(id, class, "model");
        assert_eq!(expected.is_ok(), succeeds);
        assert_eq!(actual.is_ok(), expected.is_ok());
        assert_equivalent(&model, &oracle);
        if id == "library" {
            assert_eq!(
                model
                    .resource_record("model")
                    .unwrap()
                    .unwrap()
                    .roots
                    .as_ref(),
                &["library"]
            );
            assert_eq!(
                oracle
                    .resource_record("model")
                    .unwrap()
                    .unwrap()
                    .roots
                    .as_ref(),
                &["library"]
            );
        }
    }
    let mut materialized = intersect_model::core::model::Model::default();
    materialized
        .create(&schema, "reader", "Member", "model", 0, 0)
        .unwrap();
    materialized
        .create(&schema, "library", "Library", "model", 0, 0)
        .unwrap();
    assert_eq!(materialized.resources["model"].roots.as_ref(), &["library"]);
    storage::validate_draft(&storage::ModelView::new(&materialized).unwrap(), &schema).unwrap();
}

#[test]
fn scalar_fields_referenced_by_opposite_metadata_keep_relationship_validation() {
    let mut schema = schema();
    // Raw schema adoption historically accepts this shape. Although this field
    // is an attribute, Library.copies still requires it to point back correctly.
    let library = schema
        .classes
        .get_mut("BookCopy")
        .unwrap()
        .features
        .get_mut("library")
        .unwrap();
    library.reference = false;
    library.target = "EString".into();
    library.datatype = None;
    library.opposite = None;
    let mut store = oracle(&schema);
    storage::atomic(&mut store, |store| {
        store.create_element(&schema, "library", "Library", "model", 0, 0)?;
        store.create_element(&schema, "copy", "BookCopy", "model", 0, 0)?;
        store.set_value("library", "copies", Some(json!(["copy"])))?;
        store.set_value("copy", "library", Some(json!("library")))
    })
    .unwrap();
    storage::validate_draft(&store, &schema).unwrap();
    let mut model = BasicModel::from_store(schema.clone(), store).unwrap();
    let before = storage::export(&model).unwrap();
    assert!(model.set("copy", "library", json!("wrong owner")).is_err());
    assert_eq!(storage::export(&model).unwrap(), before);
}

#[test]
fn nonreference_fields_with_opposites_validate_the_objects_they_change() {
    let mut schema = schema();
    let title = schema
        .classes
        .get_mut("BookCopy")
        .unwrap()
        .features
        .get_mut("title")
        .unwrap();
    title.datatype = None;
    title.opposite = Some("Library/members".into());
    let mut model = BasicModel::new(schema.clone()).unwrap();
    let mut oracle = oracle(&schema);
    for (id, class) in [("library", "Library"), ("copy", "BookCopy")] {
        model.create(id, class, "model").unwrap();
        full_create(&mut oracle, &schema, id, class).unwrap();
    }
    // set_with_opposite uses this metadata even though title is an attribute.
    // It would insert a BookCopy into the Member-only collection on Library.
    let edits = [("copy", "title", json!("library"))];
    let expected = full_set(&mut oracle, &schema, &edits);
    let actual = model.set_many(&edits);
    assert!(expected.is_err());
    assert_eq!(actual.is_ok(), expected.is_ok());
    assert_equivalent(&model, &oracle);
}

#[derive(Default)]
struct Reads {
    page_calls: usize,
    id_page_calls: usize,
    inspected: BTreeSet<String>,
    accessed: BTreeSet<String>,
}

struct CountingStore<S> {
    inner: S,
    reads: Rc<RefCell<Reads>>,
    incremental: bool,
}

impl<S: ModelStore> ModelRead for CountingStore<S> {
    fn resource_records(
        &self,
    ) -> Result<
        std::collections::BTreeMap<String, intersect_model::core::resources::ResourceRecord>,
        String,
    > {
        self.inner.resource_records()
    }
    fn resource_record(
        &self,
        uri: &str,
    ) -> Result<Option<intersect_model::core::resources::ResourceRecord>, String> {
        self.inner.resource_record(uri)
    }

    fn revision(&self) -> StorageResult<String> {
        self.inner.revision()
    }
    fn get(&self, id: &str) -> StorageResult<Option<Element>> {
        self.reads.borrow_mut().accessed.insert(id.into());
        self.inner.get(id)
    }
    fn class(&self, id: &str) -> StorageResult<Option<String>> {
        self.reads.borrow_mut().accessed.insert(id.into());
        self.inner.class(id)
    }
    fn page(&self, after: Option<u64>, limit: usize) -> StorageResult<Vec<Row>> {
        self.reads.borrow_mut().page_calls += 1;
        self.inner.page(after, limit)
    }
    fn id_page(&self, after: Option<u64>, limit: usize) -> StorageResult<Vec<(u64, String)>> {
        self.reads.borrow_mut().id_page_calls += 1;
        self.inner.id_page(after, limit)
    }
    fn query(&self, query: Query<'_>) -> StorageResult<Vec<String>> {
        self.inner.query(query)
    }
    fn matches(&self, id: &str, query: Query<'_>) -> StorageResult<bool> {
        self.reads.borrow_mut().accessed.insert(id.into());
        self.inner.matches(id, query)
    }
    fn value(&self, id: &str, field: &str) -> StorageResult<Option<Value>> {
        self.reads.borrow_mut().accessed.insert(id.into());
        self.inner.value(id, field)
    }
    fn inspect(
        &self,
        id: &str,
        f: &mut dyn FnMut(ObjectView<'_>) -> StorageResult<()>,
    ) -> StorageResult<bool> {
        self.reads.borrow_mut().inspected.insert(id.into());
        self.inner.inspect(id, f)
    }
}

impl<S: ModelStore> ModelStore for CountingStore<S> {
    fn set_resource_record(
        &mut self,
        record: intersect_model::core::resources::ResourceRecord,
    ) -> Result<(), String> {
        self.inner.set_resource_record(record)
    }
    fn remove_resource_record(&mut self, uri: &str) -> Result<(), String> {
        self.inner.remove_resource_record(uri)
    }

    fn supports_incremental_validation(&self) -> bool {
        self.incremental && self.inner.supports_incremental_validation()
    }
    fn is_transaction_active(&self) -> bool {
        self.inner.is_transaction_active()
    }
    fn begin(&mut self) -> StorageResult<Checkpoint> {
        self.inner.begin()
    }
    fn commit(&mut self, checkpoint: &Checkpoint) -> StorageResult<()> {
        self.inner.commit(checkpoint)
    }
    fn rollback(&mut self, checkpoint: &Checkpoint) -> StorageResult<()> {
        self.inner.rollback(checkpoint)
    }
    fn set_revision(&mut self, revision: &str) -> StorageResult<()> {
        self.inner.set_revision(revision)
    }
    fn put(&mut self, element: Element) -> StorageResult<()> {
        self.inner.put(element)
    }
    fn remove(&mut self, id: &str) -> StorageResult<()> {
        self.inner.remove(id)
    }
    fn configure_schema(&mut self, schema: &Schema) -> StorageResult<()> {
        self.inner.configure_schema(schema)
    }
    fn create_element(
        &mut self,
        schema: &Schema,
        id: &str,
        class: &str,
        resource: &str,
        start: usize,
        end: usize,
    ) -> StorageResult<()> {
        self.inner
            .create_element(schema, id, class, resource, start, end)
    }
    fn set_value(&mut self, id: &str, field: &str, value: Option<Value>) -> StorageResult<()> {
        self.inner.set_value(id, field, value)
    }
}

fn counting_model<S: StoreFixture>(
    incremental: bool,
) -> (BasicModel<CountingStore<S>>, Rc<RefCell<Reads>>) {
    let schema = schema();
    let mut inner = crate::stores::store::<S>(&schema).unwrap();
    storage::atomic(&mut inner, |store| {
        for (id, class) in [("copy", "BookCopy"), ("library", "Library")] {
            store.create_element(&schema, id, class, "model", 0, 0)?;
            store.set_value(id, "name", Some(json!(id)))?;
        }
        for i in 0..64 {
            let id = format!("unrelated-{i}");
            store.create_element(&schema, &id, "Member", "model", 0, 0)?;
            store.set_value(&id, "name", Some(json!(id)))?;
        }
        Ok(())
    })
    .unwrap();
    let reads = Rc::new(RefCell::new(Reads::default()));
    let model = BasicModel::from_store(
        schema,
        CountingStore {
            inner,
            reads: reads.clone(),
            incremental,
        },
    )
    .unwrap();
    assert!(
        reads.borrow().inspected.contains("unrelated-63"),
        "adoption remains a full check"
    );
    *reads.borrow_mut() = Reads::default();
    (model, reads)
}

fn assert_local(reads: &RefCell<Reads>, allowed: &[&str]) {
    let reads = reads.borrow();
    assert_eq!(reads.page_calls, 0);
    assert_eq!(reads.id_page_calls, 0);
    for id in reads.inspected.iter().chain(&reads.accessed) {
        assert!(allowed.contains(&id.as_str()), "unexpected read of {id}");
    }
}

fn creation_and_attribute_reads<S: StoreFixture>() {
    let (mut model, reads) = counting_model::<S>(true);
    model.create("new", "BookCopy", "model").unwrap();
    assert_local(&reads, &["new"]);
    *reads.borrow_mut() = Reads::default();
    model.set("copy", "edition", json!(2)).unwrap();
    assert_local(&reads, &["copy"]);
    *reads.borrow_mut() = Reads::default();
    model
        .set_many(&[
            ("copy", "tags", json!(["a", "b"])),
            ("new", "name", json!("new")),
        ])
        .unwrap();
    assert_local(&reads, &["copy", "new"]);
    *reads.borrow_mut() = Reads::default();
    model.set_many(&[]).unwrap();
    assert_local(&reads, &[]);
}

fn full_checks_for_relationships_and_publication<S: StoreFixture>() {
    let (mut model, reads) = counting_model::<S>(true);
    model.set("library", "copies", json!(["copy"])).unwrap();
    assert!(reads.borrow().id_page_calls > 0);
    assert!(reads.borrow().inspected.contains("unrelated-63"));
    *reads.borrow_mut() = Reads::default();
    model.validate().unwrap();
    assert!(reads.borrow().inspected.contains("unrelated-63"));
    let mut transactional = TransactionalModel::new(model);
    *reads.borrow_mut() = Reads::default();
    transactional
        .transaction(|edit| {
            edit.set("copy", "edition", json!(3))?;
            // Checking the changed object may read its reference endpoint.
            assert_local(&reads, &["copy", "library"]);
            Ok(())
        })
        .unwrap();
    assert!(reads.borrow().inspected.contains("unrelated-63"));
}

fn full_checks_without_opt_in<S: StoreFixture>() {
    let (mut model, reads) = counting_model::<S>(false);
    model.set("copy", "edition", json!(2)).unwrap();
    assert!(reads.borrow().inspected.contains("unrelated-63"));
    *reads.borrow_mut() = Reads::default();
    model.create("new", "Member", "model").unwrap();
    assert!(reads.borrow().inspected.contains("unrelated-63"));
    *reads.borrow_mut() = Reads::default();
    model.set_many(&[]).unwrap();
    assert!(reads.borrow().inspected.contains("unrelated-63"));
}

#[test]
fn adoption_rejects_an_invalid_unrelated_object() {
    let schema = schema();
    let mut store = oracle(&schema);
    full_create(&mut store, &schema, "valid", "BookCopy").unwrap();
    full_create(&mut store, &schema, "invalid", "BookCopy").unwrap();
    storage::atomic(&mut store, |store| {
        store.set_value("invalid", "tags", Some(json!(["x", "x"])))
    })
    .unwrap();
    assert!(BasicModel::from_store(schema, store).is_err());
}

mod frozen {
    use super::*;

    #[test]
    fn creation_and_attribute_edits_do_not_read_unrelated_objects() {
        creation_and_attribute_reads::<FrozenStore>();
    }

    #[test]
    fn relationship_edits_explicit_validation_and_transaction_commit_retain_full_checks() {
        full_checks_for_relationships_and_publication::<FrozenStore>();
    }

    #[test]
    fn stores_without_opt_in_keep_full_per_operation_checks() {
        full_checks_without_opt_in::<FrozenStore>();
    }

    #[test]
    fn failed_metadata_writes_leave_the_transaction_usable() {
        let schema = schema();
        let mut store = FrozenStore::for_schema(&schema).unwrap();
        let outer = store.begin().unwrap();
        store
            .create_element(&schema, "copy", "BookCopy", "model", 0, 0)
            .unwrap();
        let mut resource = store.resource_record("model").unwrap().unwrap();
        resource.content_revision = u64::MAX;
        store.set_resource_record(resource).unwrap();
        let before = storage::export(&store).unwrap();

        assert!(store.set_value("copy", "edition", Some(json!(9))).is_err());
        assert!(store.set_span("copy", 1, 2).is_err());
        assert!(
            store
                .create_element(&schema, "other", "BookCopy", "model", 0, 0)
                .is_err()
        );
        assert!(
            store
                .put(Element::new(&schema, "another", "Member", "model", 0, 0).unwrap())
                .is_err()
        );
        assert_eq!(storage::export(&store).unwrap(), before);
        assert_eq!(store.query(Query::Class("BookCopy")).unwrap(), ["copy"]);
        store.rollback(&outer).unwrap();
        assert!(store.id_page(None, 10).unwrap().is_empty());
    }
}
