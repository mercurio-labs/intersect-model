//! New-descriptor unit controls. Crate-private physical_value is used only to
//! isolate callback accounting; public authority is tested with final snapshots.
use super::*;
use crate::core::frozen::FrozenStore;
use crate::definitions::preparation::PreparedModelDefinition;
use crate::{
    core::model::Element,
    definitions::{
        bundle::{DefinitionBundle, DefinitionSource, SourceKind},
        ecore::ParseLimits,
        ecore_migration::BundleMigrationReport,
        model_definition::ModelCompilation,
    },
    storage::{ModelRead, ModelStore, ObjectView, Row},
};
use serde_json::json;

const ECORE: &str = include_str!("../../../fixtures/xtext/pinned-kerml-operands/SysML.ecore");
fn inputs() -> QueryResult<(ModelCompilation, BundleMigrationReport)> {
    let ecore = DefinitionSource::new("org.omg.sysml/model/SysML.ecore", SourceKind::Ecore, ECORE);
    let mut behavior = DefinitionSource::new(
        "memory:n4a-unit.rcore",
        SourceKind::RcoreBehavior,
        "rcore 1; package n4a version \"1\";",
    );
    behavior.dependencies = vec![ecore.id.clone()];
    let original = ModelCompilation {
        bundle: DefinitionBundle {
            version: 1,
            sources: vec![ecore.clone(), behavior.clone()],
            namespaces: vec![],
        },
        structure: ecore.id.clone(),
        behavior: behavior.id,
        namespace: "https://www.omg.org/spec/SysML/20250201".into(),
        materialized_types: vec![],
    };
    let migration = crate::migration::migrate_ecore_bundle_with_limits(
        original.bundle.clone(),
        &[(ecore.id, "org.omg.sysml/model/SysML.ecore.rcore".into())].into(),
        ParseLimits {
            source_bytes: ECORE.len(),
        },
    )
    .map_err(|error| error.to_string())?;
    Ok((migration.remap_model_compilation(&original)?, migration))
}
fn prepare<'a>(
    input: &'a ModelCompilation,
    migration: &'a BundleMigrationReport,
) -> QueryResult<PreparedModelDefinition<'a>> {
    let limits = ParseLimits {
        source_bytes: ECORE.len(),
    };
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

#[derive(Clone, Copy, Debug, PartialEq)]
enum Mode {
    Old,
    Cases,
    Empty,
    Duplicate,
    Cycle,
    Domain,
    Getter,
    Missing,
    WrongRole,
}
struct Policy(Mode);
const OLD: &[ClassifiedRead<'static>] = &[ClassifiedRead {
    key: "owner",
    reference: "owningRelationship",
    nominal: "Membership",
    admitted_actual: "OwningMembership",
}];
const CASES: &[ClassifiedCasesRead<'static>] = &[ClassifiedCasesRead {
    key: "owner",
    reference: "owningRelationship",
    nominal: "Membership",
    admitted_actuals: &["OwningMembership"],
}];
const EMPTY: &[ClassifiedCasesRead<'static>] = &[ClassifiedCasesRead {
    key: "",
    reference: "owningRelationship",
    nominal: "Membership",
    admitted_actuals: &["OwningMembership"],
}];
const DUPLICATE: &[ClassifiedCasesRead<'static>] = &[CASES[0], CASES[0]];
const DOMAIN: &[ClassifiedCasesRead<'static>] = &[ClassifiedCasesRead {
    key: "owner",
    reference: "owningRelationship",
    nominal: "Namespace",
    admitted_actuals: &["Package"],
}];
const MISSING: &[ClassifiedCasesRead<'static>] = &[ClassifiedCasesRead {
    key: "owner",
    reference: "owningRelationship",
    nominal: "Membership",
    admitted_actuals: &["Missing"],
}];
const WRONG: &[ClassifiedCasesRead<'static>] = &[ClassifiedCasesRead {
    key: "owner",
    reference: "declaredName",
    nominal: "Membership",
    admitted_actuals: &["OwningMembership"],
}];
const BAD_GETTER: &[PhysicalRelatedRead<'static>] = &[PhysicalRelatedRead {
    key: "owner",
    reference: "owningRelationship",
    null: RelatedNull::ReturnNull,
    cases: &[PhysicalRelatedCase {
        actual: "OwningMembership",
        feature: "missingGetter",
    }],
}];
impl SemanticReadPolicy for Policy {
    fn validate(&self, _: &PreparedModelDefinition<'_>) -> QueryResult<()> {
        Ok(())
    }
    fn plan(&self, member: ReadMember<'_>) -> ReadPlan {
        if member.feature.derived {
            ReadPlan::Derived
        } else {
            ReadPlan::Stored
        }
    }
    fn dependencies(&self, member: ReadMember<'_>) -> &[&str] {
        if self.0 == Mode::Cycle && member.feature.name == "owningMembership" {
            &["owningMembership"]
        } else {
            &[]
        }
    }
    fn classified_dependencies(&self, member: ReadMember<'_>) -> &[ClassifiedRead<'_>] {
        if self.0 == Mode::Old && member.feature.name == "owningMembership" {
            OLD
        } else {
            &[]
        }
    }
    fn classified_cases_dependencies(&self, member: ReadMember<'_>) -> &[ClassifiedCasesRead<'_>] {
        if member.feature.name != "owningMembership" {
            return &[];
        }
        match self.0 {
            Mode::Old | Mode::Getter => &[],
            Mode::Empty => EMPTY,
            Mode::Duplicate => DUPLICATE,
            Mode::Domain => DOMAIN,
            Mode::Missing => MISSING,
            Mode::WrongRole => WRONG,
            _ => CASES,
        }
    }
    fn physical_related_dependencies(&self, member: ReadMember<'_>) -> &[PhysicalRelatedRead<'_>] {
        if self.0 == Mode::Getter && member.feature.name == "owningMembership" {
            BAD_GETTER
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
        match deps.read_classified("owner", 0, budget)? {
            ClassifiedValue::Match(value) => Ok(value),
            ClassifiedValue::NonMatch | ClassifiedValue::Absent => Ok(Value::Null),
        }
    }
}
struct TraceStore {
    inner: FrozenStore,
    trace: RefCell<Vec<String>>,
}
impl TraceStore {
    fn note(&self, value: String) {
        self.trace.borrow_mut().push(value);
    }
    fn reset(&self) {
        self.trace.borrow_mut().clear();
    }
}
impl ModelRead for TraceStore {
    fn revision(&self) -> QueryResult<String> {
        self.note("revision".into());
        self.inner.revision()
    }
    fn get(&self, id: &str) -> QueryResult<Option<Element>> {
        self.note(format!("get:{id}"));
        self.inner.get(id)
    }
    fn class(&self, id: &str) -> QueryResult<Option<String>> {
        self.note(format!("class:{id}"));
        self.inner.class(id)
    }
    fn page(&self, after: Option<u64>, limit: usize) -> QueryResult<Vec<Row>> {
        self.note(format!("page:{after:?}:{limit}"));
        self.inner.page(after, limit)
    }
    fn query(&self, query: storage::Query<'_>) -> QueryResult<Vec<String>> {
        self.note("query".into());
        self.inner.query(query)
    }
    fn matches(&self, id: &str, query: storage::Query<'_>) -> QueryResult<bool> {
        self.note(format!("matches:{id}"));
        self.inner.matches(id, query)
    }
    fn inspect(
        &self,
        id: &str,
        callback: &mut dyn FnMut(ObjectView<'_>) -> QueryResult<()>,
    ) -> QueryResult<bool> {
        self.note(format!("inspect:{id}"));
        self.inner.inspect(id, callback)
    }
}
fn store(schema: &Schema) -> QueryResult<TraceStore> {
    let mut inner = FrozenStore::for_schema(schema).map_err(|error| error.to_string())?;
    storage::atomic(&mut inner, |store| {
        store.create_element(schema, "p", "Package", "memory:n4a-unit", 0, 1)?;
        store.create_element(schema, "m", "OwningMembership", "memory:n4a-unit", 0, 1)?;
        store.set_value("p", "owningRelationship", Some(json!("m")))
    })?;
    Ok(TraceStore {
        inner,
        trace: RefCell::new(Vec::new()),
    })
}

#[test]
#[cfg_attr(not(feature = "milestone-tests"), ignore = "milestone")]
fn n4a_descriptor_preflight_rejects_bad_keys_cycles_domains_and_getters() -> QueryResult<()> {
    let (input, migration) = inputs()?;
    let definition = prepare(&input, &migration)?;
    let store = store(definition.schema())?;
    for mode in [
        Mode::Empty,
        Mode::Duplicate,
        Mode::Cycle,
        Mode::Domain,
        Mode::Getter,
        Mode::Missing,
        Mode::WrongRole,
    ] {
        let policy = Policy(mode);
        let session = SemanticReadSession::new(&definition, &store, &policy)?;
        let mut budget = QueryBudget::new(100_000);
        let error = session
            .physical_value("Package", "p", "owningMembership", &mut budget)
            .unwrap_err();
        assert_eq!(budget.reads, 0, "{mode:?}: {error}");
        assert!(store.trace.borrow().is_empty(), "{mode:?}: {error}");
        println!(
            "N4A13 {}",
            json!({"mode":format!("{mode:?}"),"error":error,"work":budget.reads,"callbacks":*store.trace.borrow()})
        );
    }
    Ok(())
}

#[test]
#[cfg_attr(not(feature = "milestone-tests"), ignore = "milestone")]
fn n4a_new_boundary_work_and_callbacks_share_old_selector() -> QueryResult<()> {
    let (input, migration) = inputs()?;
    let definition = prepare(&input, &migration)?;
    let store = store(definition.schema())?;
    let mut results = Vec::new();
    for mode in [Mode::Old, Mode::Cases] {
        let policy = Policy(mode);
        let session = SemanticReadSession::new(&definition, &store, &policy)?;
        let mut budget = QueryBudget::new(100_000);
        store.reset();
        assert_eq!(
            session.physical_value("Package", "p", "owningMembership", &mut budget)?,
            json!("m")
        );
        results.push((budget.reads, store.trace.borrow().clone()));
    }
    assert_eq!(results[1].0, results[0].0 + 2);
    // Qualified literal pre-handoff fees were 5318/5320. Reusing only this
    // call's immutable metadata must reduce both, while the new descriptor
    // still costs exactly two more units and retains the literal callbacks.
    assert!(results[0].0 < 5318 && results[1].0 < 5320);
    assert_eq!(results[1].1, results[0].1);
    let literal_callbacks = vec![
        "revision",
        "inspect:p",
        "revision",
        "inspect:p",
        "revision",
        "class:p",
        "get:p",
        "revision",
        "inspect:m",
    ];
    assert_eq!(results[1].1, literal_callbacks);
    let policy = Policy(Mode::Cases);
    let session = SemanticReadSession::new(&definition, &store, &policy)?;
    for limit in [0, results[1].0 - 1, results[1].0] {
        store.reset();
        let mut budget = QueryBudget::new(limit);
        let result = session.physical_value("Package", "p", "owningMembership", &mut budget);
        assert_eq!(result.is_ok(), limit == results[1].0);
        assert_eq!(budget.reads, limit);
        if limit == 0 {
            assert!(store.trace.borrow().is_empty());
        } else {
            assert_eq!(*store.trace.borrow(), literal_callbacks);
        }
        println!(
            "N4A14_BOUNDARY {}",
            json!({"limit":limit,"work":budget.reads,"result":result,"callbacks":*store.trace.borrow()})
        );
    }
    println!(
        "N4A14_SELECTOR {}",
        json!({"old_work":results[0].0,"new_work":results[1].0,"old_callbacks":results[0].1,"new_callbacks":results[1].1})
    );
    Ok(())
}

#[test]
fn n4a_new_descriptors_do_not_grow_legacy_read_holders() -> QueryResult<()> {
    use std::mem::{align_of, size_of};
    let word = size_of::<usize>();
    assert_eq!(size_of::<ClassifiedRead<'_>>(), 8 * word);
    assert_eq!(size_of::<PhysicalRead<'_>>(), 4 * word);
    // One optional immutable metadata-owner reference; dependency holders do
    // not grow, because they already borrow the SAME session.
    assert_eq!(size_of::<SemanticReadSession<'_, Policy>>(), 5 * word);
    assert_eq!(size_of::<Dependencies<'_, '_, Policy>>(), 7 * word);
    assert_eq!(size_of::<RelatedDependencies<'_, '_, Policy>>(), 13 * word);
    assert_eq!(size_of::<PhysicalDependencies<'_, '_, Policy>>(), 13 * word);
    assert_eq!(size_of::<ClassifiedCasesRead<'_>>(), 8 * word);
    assert_eq!(size_of::<PhysicalRelatedCase<'_>>(), 4 * word);
    assert_eq!(size_of::<PhysicalRelatedRead<'_>>(), 7 * word);
    assert_eq!(align_of::<PhysicalRelatedRead<'_>>(), align_of::<usize>());
    println!(
        "N4A15_LAYOUT {}",
        json!({"word":word,"session":size_of::<SemanticReadSession<'_,Policy>>(),
        "dependencies":size_of::<Dependencies<'_,'_,Policy>>(),"physical":size_of::<PhysicalDependencies<'_,'_,Policy>>(),
        "old_classified":size_of::<ClassifiedRead<'_>>(),"new_classified":size_of::<ClassifiedCasesRead<'_>>(),
        "new_related":size_of::<PhysicalRelatedRead<'_>>(),"new_related_case":size_of::<PhysicalRelatedCase<'_>>(),
        "classified_value":size_of::<ClassifiedValue>()})
    );
    Ok(())
}
