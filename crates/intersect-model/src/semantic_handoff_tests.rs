//! Same-call immutable metadata handoff; no model or semantic-result reuse.
use super::*;
use crate::core::frozen::FrozenStore;
use crate::definitions::preparation::PreparedModelDefinition;
use crate::{
    core::model::Element,
    definitions::{
        bundle::{DefinitionBundle, DefinitionSource, SourceKind},
        ecore_migration::BundleMigrationReport,
        model_definition::ModelCompilation,
        schema::Semantics,
    },
    storage::{ModelRead, ModelStore, ObjectView, Row},
};
use serde_json::json;

const XML: &str = r##"<e:EPackage xmlns:e="http://www.eclipse.org/emf/2002/Ecore" xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance" name="handoff" nsURI="urn:handoff">
<eClassifiers xsi:type="e:EClass" name="Actor">
<eStructuralFeatures xsi:type="e:EAttribute" name="backing" eType="e:EDataType http://www.eclipse.org/emf/2002/Ecore#//EString"/>
<eStructuralFeatures xsi:type="e:EAttribute" name="echo" derived="true" eType="e:EDataType http://www.eclipse.org/emf/2002/Ecore#//EString"/>
<eStructuralFeatures xsi:type="e:EAttribute" name="secret" eType="e:EDataType http://www.eclipse.org/emf/2002/Ecore#//EString"/>
<eStructuralFeatures xsi:type="e:EAttribute" name="volatileLeaf" volatile="true" eType="e:EDataType http://www.eclipse.org/emf/2002/Ecore#//EString"/>
<eStructuralFeatures xsi:type="e:EAttribute" name="transientLeaf" transient="true" eType="e:EDataType http://www.eclipse.org/emf/2002/Ecore#//EString"/>
</eClassifiers><eClassifiers xsi:type="e:EClass" name="Other"/></e:EPackage>"##;

fn inputs() -> QueryResult<(ModelCompilation, BundleMigrationReport)> {
    let source = DefinitionSource::new("memory:handoff.ecore", SourceKind::Ecore, XML);
    let mut rules = DefinitionSource::new(
        "memory:handoff.rules",
        SourceKind::RcoreBehavior,
        "rcore 1; package handoff version \"1\";",
    );
    rules.dependencies = vec![source.id.clone()];
    let input = ModelCompilation {
        bundle: DefinitionBundle {
            version: 1,
            sources: vec![source.clone(), rules.clone()],
            namespaces: vec![],
        },
        structure: source.id.clone(),
        behavior: rules.id,
        namespace: "urn:handoff".into(),
        materialized_types: vec![],
    };
    let migration = crate::migration::migrate_ecore_bundle(
        input.bundle.clone(),
        &[(source.id, "memory:handoff.rcore".into())].into(),
    )
    .map_err(|e| e.to_string())?;
    Ok((migration.remap_model_compilation(&input)?, migration))
}
fn definition<'a>(
    input: &'a ModelCompilation,
    migration: &'a BundleMigrationReport,
) -> QueryResult<PreparedModelDefinition<'a>> {
    let limits = crate::definitions::ecore::ParseLimits::default();
    crate::definitions::model_test_support::prepare(
        crate::definitions::model_definition::StructureSelection {
            bundle: &input.bundle,
            entry: &input.structure,
            namespace: &input.namespace,
            materialized_types: &input.materialized_types,
        },
        &input.behavior,
        migration,
        limits,
    )
}

#[derive(Default)]
struct Policy {
    trace: RefCell<Vec<String>>,
    unavailable_dependency: bool,
}
impl SemanticReadPolicy for Policy {
    fn validate(&self, _: &PreparedModelDefinition<'_>) -> QueryResult<()> {
        Ok(())
    }
    fn plan(&self, member: ReadMember<'_>) -> ReadPlan {
        self.trace
            .borrow_mut()
            .push(format!("plan:{}", member.feature.name));
        match member.feature.name.as_str() {
            "backing" => ReadPlan::Stored,
            "echo" => ReadPlan::Derived,
            _ => ReadPlan::Unsupported,
        }
    }
    fn dependencies(&self, member: ReadMember<'_>) -> &[&str] {
        self.trace
            .borrow_mut()
            .push(format!("dependencies:{}", member.feature.name));
        if member.feature.name == "echo" {
            if self.unavailable_dependency {
                &["secret"]
            } else {
                &["backing"]
            }
        } else {
            &[]
        }
    }
    fn read(
        &self,
        _: ReadMember<'_>,
        deps: &dyn ReadDependencies,
        budget: &mut QueryBudget,
    ) -> QueryResult<Value> {
        self.trace.borrow_mut().push("read:echo".into());
        deps.read("backing", budget)
    }
}
// Interior changes are intentional controls: the reader must not retain a row,
// current class, value, or successful previous result between read calls.
struct LiveRows {
    inner: RefCell<FrozenStore>,
    raw_row: RefCell<Option<Element>>,
    trace: RefCell<Vec<String>>,
}
impl LiveRows {
    fn new(schema: &Schema) -> QueryResult<Self> {
        let mut inner = FrozenStore::for_schema(schema).map_err(|error| error.to_string())?;
        storage::atomic(&mut inner, |store| {
            store.create_element(schema, "a", "Actor", "memory:handoff", 0, 1)?;
            store.set_value("a", "backing", Some(json!("first")))
        })?;
        Ok(Self {
            inner: RefCell::new(inner),
            raw_row: RefCell::new(None),
            trace: RefCell::new(Vec::new()),
        })
    }
    fn change(&self, f: impl FnOnce(&mut Element)) -> QueryResult<()> {
        let mut store = self.inner.borrow_mut();
        let mut element = store.get("a")?.ok_or("Missing handoff fixture object")?;
        f(&mut element);
        storage::atomic(&mut *store, |store| store.put(element))
    }
}
impl ModelRead for LiveRows {
    fn revision(&self) -> QueryResult<String> {
        self.trace.borrow_mut().push("revision".into());
        self.inner.borrow().revision()
    }
    fn get(&self, id: &str) -> QueryResult<Option<Element>> {
        self.inner.borrow().get(id)
    }
    fn class(&self, id: &str) -> QueryResult<Option<String>> {
        self.inner.borrow().class(id)
    }
    fn page(&self, after: Option<u64>, limit: usize) -> QueryResult<Vec<Row>> {
        self.inner.borrow().page(after, limit)
    }
    fn query(&self, query: storage::Query<'_>) -> QueryResult<Vec<String>> {
        self.inner.borrow().query(query)
    }
    fn matches(&self, id: &str, query: storage::Query<'_>) -> QueryResult<bool> {
        self.inner.borrow().matches(id, query)
    }
    fn inspect(
        &self,
        id: &str,
        f: &mut dyn FnMut(ObjectView<'_>) -> QueryResult<()>,
    ) -> QueryResult<bool> {
        self.trace.borrow_mut().push(format!("inspect:{id}"));
        if let Some(raw) = self.raw_row.borrow().as_ref().filter(|raw| raw.id == id) {
            f(ObjectView::from_element(raw))?;
            return Ok(true);
        }
        self.inner.borrow().inspect(id, f)
    }
}
fn read(
    session: &SemanticReadSession<'_, Policy>,
    physical: bool,
    field: &str,
    budget: &mut QueryBudget,
) -> QueryResult<Value> {
    if physical {
        session.physical_value("Actor", "a", field, budget)
    } else {
        session.value("Actor", "a", field, budget)
    }
}

#[test]
fn physical_handoff_preserves_ordinary_callbacks_and_zero_exact_short_work() -> QueryResult<()> {
    let (input, migration) = inputs()?;
    let definition = definition(&input, &migration)?;
    let rows = LiveRows::new(definition.schema())?;
    let policy = Policy::default();
    let session = SemanticReadSession::new(&definition, &rows, &policy)?;
    for field in ["backing", "echo"] {
        let mut observations = Vec::new();
        for physical in [false, true] {
            rows.trace.borrow_mut().clear();
            policy.trace.borrow_mut().clear();
            let mut budget = QueryBudget::new(100_000);
            assert_eq!(
                read(&session, physical, field, &mut budget)?,
                json!("first")
            );
            observations.push((
                budget.reads,
                rows.trace.borrow().clone(),
                policy.trace.borrow().clone(),
            ));
        }
        assert_eq!(observations[0].1, observations[1].1);
        assert_eq!(observations[0].2, observations[1].2);
        if field == "backing" {
            assert_eq!(observations[1].0, observations[0].0);
        } else {
            // Ordinary computed root qualification remains literal.
            assert!(observations[1].0 < observations[0].0);
        }
        for (physical, observation) in [false, true].into_iter().zip(&observations) {
            let work = observation.0;
            assert!(work > 0);
            for limit in [0, work - 1, work] {
                rows.trace.borrow_mut().clear();
                policy.trace.borrow_mut().clear();
                let mut budget = QueryBudget::new(limit);
                let result = read(&session, physical, field, &mut budget);
                assert_eq!(result.is_ok(), limit == work);
                assert_eq!(budget.reads, limit);
                if limit == 0 {
                    assert!(rows.trace.borrow().is_empty());
                }
                if limit == work {
                    assert_eq!(rows.trace.borrow().as_slice(), observation.1.as_slice());
                    assert_eq!(policy.trace.borrow().as_slice(), observation.2.as_slice());
                }
            }
        }
    }
    Ok(())
}

#[test]
fn physical_handoff_keeps_fresh_rows_classes_identities_and_raw_values() -> QueryResult<()> {
    let (input, migration) = inputs()?;
    let definition = definition(&input, &migration)?;
    let rows = LiveRows::new(definition.schema())?;
    let policy = Policy::default();
    let session = SemanticReadSession::new(&definition, &rows, &policy)?;
    assert_eq!(
        read(&session, true, "echo", &mut QueryBudget::new(100_000))?,
        json!("first")
    );
    rows.change(|row| {
        row.values.insert("backing".into(), json!("second"));
    })?;
    for physical in [false, true] {
        assert_eq!(
            read(&session, physical, "echo", &mut QueryBudget::new(100_000))?,
            json!("second")
        );
    }
    rows.change(|row| {
        row.values.insert("backing".into(), json!(["wrong-shape"]));
    })?;
    // Singular projection preserves raw payloads; codec/domain validation is
    // a separate existing contract. The optimization must not normalize them.
    for physical in [false, true] {
        assert_eq!(
            read(&session, physical, "echo", &mut QueryBudget::new(100_000))?,
            json!(["wrong-shape"])
        );
    }
    let mut raw = rows.get("a")?.ok_or("Missing handoff fixture object")?;
    raw.type_identity = "foreign".into();
    assert!(
        rows.change(|row| row.type_identity.clone_from(&raw.type_identity))
            .is_err()
    );
    rows.raw_row.replace(Some(raw));
    let ordinary = read(&session, false, "echo", &mut QueryBudget::new(100_000)).unwrap_err();
    assert!(ordinary.contains("identity mismatch"));
    assert_eq!(
        read(&session, true, "echo", &mut QueryBudget::new(100_000)).unwrap_err(),
        ordinary
    );
    rows.raw_row.replace(None);
    let other = definition.schema().identity("Other");
    rows.change(|row| {
        row.class = "Other".into();
        row.type_identity = other;
        row.values.clear();
    })?;
    let ordinary = read(&session, false, "echo", &mut QueryBudget::new(100_000)).unwrap_err();
    assert!(ordinary.contains("does not conform"));
    assert_eq!(
        read(&session, true, "echo", &mut QueryBudget::new(100_000)).unwrap_err(),
        ordinary
    );
    Ok(())
}

#[test]
fn physical_handoff_keeps_preflight_metadata_and_dependency_refusal_before_store() -> QueryResult<()>
{
    let (input, migration) = inputs()?;
    let definition = definition(&input, &migration)?;
    let rows = LiveRows::new(definition.schema())?;
    let policy = Policy {
        unavailable_dependency: true,
        ..Policy::default()
    };
    let session = SemanticReadSession::new(&definition, &rows, &policy)?;
    let mut expected = None;
    for physical in [false, true] {
        policy.trace.borrow_mut().clear();
        let mut budget = QueryBudget::new(100_000);
        let error = read(&session, physical, "echo", &mut budget).unwrap_err();
        assert!(error.contains("Unsupported semantic read"));
        assert_eq!(budget.reads, 0);
        assert!(rows.trace.borrow().is_empty());
        let actual = (error, policy.trace.borrow().clone());
        if let Some(expected) = &expected {
            assert_eq!(&actual, expected);
        } else {
            expected = Some(actual);
        }
    }
    // Intentionally malformed immutable metadata must still fail complete
    // canonical selection, even though the requested field is locally declared.
    let mut schema = definition.schema().clone();
    schema
        .classes
        .get_mut("Actor")
        .ok_or("Missing fixture Actor")?
        .parents
        .push("Missing".into());
    let expected = schema.effective_feature("Actor", "backing").unwrap_err();
    let invalid = PreparedModelDefinition::inspection_for_test(
        crate::definitions::model_definition::StructureSelection {
            bundle: &input.bundle,
            entry: &input.structure,
            namespace: &input.namespace,
            materialized_types: &input.materialized_types,
        },
        &input.behavior,
        &migration,
        schema,
        Semantics {
            scope_bindings: vec![],
            rule_conversions: vec![],
            version: 1,
            scope: "handoff".into(),
            derived: vec![],
            initializers: vec![],
            validations: vec![],
        },
    );
    let policy = Policy::default();
    let session = SemanticReadSession::new(&invalid, &rows, &policy)?;
    for physical in [false, true] {
        let mut budget = QueryBudget::new(100_000);
        assert_eq!(
            read(&session, physical, "backing", &mut budget).unwrap_err(),
            expected
        );
        assert_eq!(budget.reads, 0);
        assert!(rows.trace.borrow().is_empty());
        assert!(policy.trace.borrow().is_empty());
    }
    Ok(())
}

#[test]
fn physical_preflight_keeps_real_nonzero_position_and_rejects_foreign_schema_metadata()
-> QueryResult<()> {
    let (input, migration) = inputs()?;
    let definition = definition(&input, &migration)?;
    let rows = LiveRows::new(definition.schema())?;
    let policy = Policy::default();
    let session = SemanticReadSession::new(&definition, &rows, &policy)?;
    let (_, selected) = session.preflight_selected_accounted(
        "Actor",
        "echo",
        &mut Vec::new(),
        true,
        &ReadPreflight::legacy(),
        false,
        true,
    )?;
    let real = definition.schema().effective_feature("Actor", "echo")?;
    assert!(real.position > 0);
    assert_eq!(selected.position, Some(real.position));
    assert_eq!(&*selected.identity, &real.identity);
    assert!(std::ptr::eq(selected.feature, real.feature));
    let foreign = definition.schema().clone();
    let foreign_member = foreign.effective_feature("Actor", "echo")?;
    assert_eq!(foreign_member.position, real.position);
    let mut pending = storage::SemanticFeaturePreparation::new("Actor", &selected.identity, "a");
    assert_eq!(
        pending.advance_preflighted_exact(
            &rows,
            definition.schema(),
            &InvocationContext::default(),
            foreign_member.owner,
            foreign_member.feature,
            foreign_member.position,
        ),
        Err("Physical preflight member/schema owner changed".into())
    );
    assert_eq!(rows.trace.borrow().as_slice(), ["revision"]);
    assert!(
        pending
            .advance_preflighted_exact(
                &rows,
                definition.schema(),
                &InvocationContext::default(),
                selected.owner,
                selected.feature,
                real.position,
            )
            .is_err()
    );
    Ok(())
}

#[test]
fn observed_preflight_ordinal_request_keeps_literal_accounting_and_no_handoff() -> QueryResult<()> {
    struct Meter {
        work: RefCell<QueryBudget>,
        bytes: std::cell::Cell<usize>,
    }
    impl InvocationObserver for Meter {
        fn before_invoke(&self, _: &ProviderId) -> QueryResult<()> {
            Ok(())
        }
        fn work(&self, units: usize) -> QueryResult<()> {
            self.work.borrow_mut().charge_many(units)
        }
        fn retain_storage(&self, bytes: usize) -> QueryResult<()> {
            self.bytes.set(
                self.bytes
                    .get()
                    .checked_add(bytes)
                    .ok_or("Test storage overflow")?,
            );
            Ok(())
        }
    }
    let (input, migration) = inputs()?;
    let definition = definition(&input, &migration)?;
    let rows = LiveRows::new(definition.schema())?;
    let policy = Policy::default();
    let session = SemanticReadSession::new(&definition, &rows, &policy)?;
    let mut complete = Vec::new();
    for request_position in [false, true] {
        let meter = Rc::new(Meter {
            work: RefCell::new(QueryBudget::new(100_000)),
            bytes: std::cell::Cell::new(0),
        });
        let accounting = ReadPreflight::observed(Some(InvocationContext::new(meter.clone())))?;
        let selected = if request_position {
            accounting.selected_with_position(&session, "Actor", "echo")?
        } else {
            accounting.selected_with_hint(&session, "Actor", "echo")?
        };
        assert_eq!(selected.position, None);
        assert_eq!(
            &*selected.identity,
            &definition
                .schema()
                .effective_feature("Actor", "echo")?
                .identity
        );
        complete.push((meter.work.borrow().reads, meter.bytes.get()));
    }
    assert_eq!(complete[0], complete[1]);
    assert!(complete[0].0 > 0 && complete[0].1 > 0);
    for limit in [0, complete[0].0 - 1, complete[0].0] {
        for request_position in [false, true] {
            let meter = Rc::new(Meter {
                work: RefCell::new(QueryBudget::new(limit)),
                bytes: std::cell::Cell::new(0),
            });
            let result = ReadPreflight::observed(Some(InvocationContext::new(meter.clone())))
                .and_then(|accounting| {
                    if request_position {
                        accounting.selected_with_position(&session, "Actor", "echo")
                    } else {
                        accounting.selected_with_hint(&session, "Actor", "echo")
                    }
                });
            assert_eq!(result.is_ok(), limit == complete[0].0);
            assert_eq!(meter.work.borrow().reads, limit);
        }
    }
    assert!(rows.trace.borrow().is_empty());
    assert!(policy.trace.borrow().is_empty());
    Ok(())
}

#[test]
fn ordinary_stored_handoff_keeps_derived_and_policy_flag_refusals_before_store() -> QueryResult<()>
{
    #[derive(Default)]
    struct FlagPolicy(RefCell<Vec<(bool, bool, bool)>>);
    impl SemanticReadPolicy for FlagPolicy {
        fn validate(&self, _: &PreparedModelDefinition<'_>) -> QueryResult<()> {
            Ok(())
        }
        fn plan(&self, member: ReadMember<'_>) -> ReadPlan {
            let field = member.feature;
            self.0
                .borrow_mut()
                .push((field.derived, field.volatile, field.transient));
            if field.volatile || field.transient {
                ReadPlan::Unsupported
            } else {
                ReadPlan::Stored
            }
        }
        fn dependencies(&self, _: ReadMember<'_>) -> &[&str] {
            &[]
        }
        fn read(
            &self,
            _: ReadMember<'_>,
            _: &dyn ReadDependencies,
            _: &mut QueryBudget,
        ) -> QueryResult<Value> {
            Err("Refused/stored policy must never invoke a getter".into())
        }
    }
    let (input, migration) = inputs()?;
    let definition = definition(&input, &migration)?;
    let rows = LiveRows::new(definition.schema())?;
    let policy = FlagPolicy::default();
    let session = SemanticReadSession::new(&definition, &rows, &policy)?;
    for (field, flags, expected) in [
        (
            "echo",
            (true, false, false),
            "Derived metadata cannot select Stored reads",
        ),
        (
            "volatileLeaf",
            (false, true, false),
            "Unsupported semantic read Actor:",
        ),
        (
            "transientLeaf",
            (false, false, true),
            "Unsupported semantic read Actor:",
        ),
    ] {
        let mut expected_error = None;
        for physical in [false, true] {
            policy.0.borrow_mut().clear();
            let mut budget = QueryBudget::new(100_000);
            let result = if physical {
                session.physical_value("Actor", "a", field, &mut budget)
            } else {
                session.value("Actor", "a", field, &mut budget)
            };
            let error = result.unwrap_err();
            assert!(error.starts_with(expected), "{error}");
            if let Some(expected_error) = &expected_error {
                assert_eq!(&error, expected_error);
            } else {
                expected_error = Some(error);
            }
            assert_eq!(policy.0.borrow().as_slice(), [flags]);
            assert!(rows.trace.borrow().is_empty());
            assert_eq!(budget.reads, 0);
        }
    }
    Ok(())
}
