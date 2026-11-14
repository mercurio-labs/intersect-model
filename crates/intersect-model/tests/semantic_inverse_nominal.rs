//! Current-store inverse witness controls; no language or whole-model validity claim.
use intersect_model::{
    core::{
        frozen::{FrozenStore, Metamodel},
        query::QueryBudget,
        storage::{ModelRead, ModelStore},
    },
    definitions::{
        bundle::{DefinitionBundle, DefinitionSource, SourceId, SourceKind},
        model_definition::ModelCompilation,
        schema::MemberIdentity,
    },
    semantic_reads::{
        ClassifiedValue, InverseNominalRead, NominalRead, ReadDependencies, ReadMember, ReadPlan,
        SemanticReadPolicy, SemanticReadSession,
    },
};
use serde_json::{Value, json};
use std::cell::Cell;
const EDGE: &[InverseNominalRead<'static>] = &[InverseNominalRead {
    key: "owner-kind",
    reference: "owner",
    nominal: "Wanted",
}];
const BAD: &[InverseNominalRead<'static>] = &[InverseNominalRead {
    key: "owner-kind",
    reference: "plain",
    nominal: "Wanted",
}];
const ORDINARY: &[NominalRead<'static>] = &[NominalRead {
    key: "owner-kind",
    reference: "owner",
    nominal: "Wanted",
}];
struct Policy {
    source: SourceId,
    kind: Cell<u8>,
    ordinary: bool,
    bad: bool,
}
impl SemanticReadPolicy for Policy {
    fn validate(
        &self,
        d: &intersect_model::definitions::preparation::PreparedModelDefinition<'_>,
    ) -> Result<(), String> {
        if self.source != *d.structure_selection().entry {
            return Err("foreign test source".into());
        }
        Ok(())
    }
    fn plan(&self, m: ReadMember<'_>) -> ReadPlan {
        if m.actual != "Child"
            || !matches!(m.identity,MemberIdentity::Source{declaration} if declaration.source==self.source)
        {
            return ReadPlan::Unsupported;
        }
        match m.feature.name.as_str() {
            "owner" | "plain" => ReadPlan::Stored,
            "selected" => ReadPlan::Derived,
            _ => ReadPlan::Unsupported,
        }
    }
    fn dependencies(&self, _: ReadMember<'_>) -> &[&str] {
        &[]
    }
    fn nominal_dependencies(&self, m: ReadMember<'_>) -> &[NominalRead<'_>] {
        if self.ordinary && m.feature.name == "selected" {
            ORDINARY
        } else {
            &[]
        }
    }
    fn inverse_nominal_dependencies(&self, m: ReadMember<'_>) -> &[InverseNominalRead<'_>] {
        if self.ordinary || m.feature.name != "selected" {
            &[]
        } else if self.bad {
            BAD
        } else {
            EDGE
        }
    }
    fn read(
        &self,
        _: ReadMember<'_>,
        d: &dyn ReadDependencies,
        b: &mut QueryBudget,
    ) -> Result<Value, String> {
        let value = if self.ordinary {
            d.read_nominal("owner-kind", 0, b)?
        } else {
            d.read_inverse_nominal("owner-kind", 0, b)?
        };
        match value {
            ClassifiedValue::Match(v) => {
                self.kind.set(1);
                Ok(v)
            }
            ClassifiedValue::NonMatch => {
                self.kind.set(2);
                Ok(Value::Null)
            }
            ClassifiedValue::Absent => {
                self.kind.set(3);
                Ok(Value::Null)
            }
        }
    }
}
struct ChangingRevision<'a> {
    store: &'a FrozenStore,
    calls: Cell<usize>,
}
impl ModelRead for ChangingRevision<'_> {
    fn revision(&self) -> Result<String, String> {
        let n = self.calls.get();
        self.calls.set(n + 1);
        if n == 0 {
            self.store.revision()
        } else {
            Ok("changed".into())
        }
    }
    fn get(&self, id: &str) -> Result<Option<intersect_model::core::model::Element>, String> {
        self.store.get(id)
    }
    fn class(&self, id: &str) -> Result<Option<String>, String> {
        self.store.class(id)
    }
    fn page(
        &self,
        after: Option<u64>,
        limit: usize,
    ) -> Result<Vec<intersect_model::storage::Row>, String> {
        self.store.page(after, limit)
    }
    fn query(&self, q: intersect_model::storage::Query<'_>) -> Result<Vec<String>, String> {
        self.store.query(q)
    }
    fn matches(&self, id: &str, q: intersect_model::storage::Query<'_>) -> Result<bool, String> {
        self.store.matches(id, q)
    }
}

#[test]
fn checked_inverse_nominal_requires_current_unique_reciprocal_source_containment()
-> Result<(), String> {
    let ecore = DefinitionSource::new(
        "memory:inverse/model.ecore",
        SourceKind::Ecore,
        r##"<ecore:EPackage xmlns:ecore="http://www.eclipse.org/emf/2002/Ecore" xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance" name="inverse" nsURI="urn:inverse" nsPrefix="n">
<eClassifiers xsi:type="ecore:EClass" name="Parent"><eStructuralFeatures xsi:type="ecore:EReference" name="children" eType="#//Child" upperBound="-1" containment="true" eOpposite="#//Child/owner"/></eClassifiers>
<eClassifiers xsi:type="ecore:EClass" name="Wanted" eSuperTypes="#//Parent"/>
<eClassifiers xsi:type="ecore:EClass" name="Other" eSuperTypes="#//Parent"/>
<eClassifiers xsi:type="ecore:EClass" name="Child"><eStructuralFeatures xsi:type="ecore:EReference" name="owner" eType="#//Parent" eOpposite="#//Parent/children"/><eStructuralFeatures xsi:type="ecore:EReference" name="plain" eType="#//Parent"/><eStructuralFeatures xsi:type="ecore:EReference" name="selected" eType="#//Parent" derived="true"/></eClassifiers></ecore:EPackage>"##,
    );
    let mut behavior = DefinitionSource::new(
        "memory:inverse/behavior.rcore",
        SourceKind::RcoreBehavior,
        "rcore 1; package inverse version \"1\";",
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
        namespace: "urn:inverse".into(),
        materialized_types: vec![],
    };
    let origin = intersect_model::migration::migrate_ecore_bundle(
        original.bundle.clone(),
        &[(ecore.id, "memory:inverse/model.rcore".into())].into(),
    )
    .map_err(|e| e.to_string())?;
    let input = origin.remap_model_compilation(&original)?;
    let prepared = crate::model_fixture::prepare(
        intersect_model::definitions::model_definition::StructureSelection {
            bundle: &input.bundle,
            entry: &input.structure,
            namespace: &input.namespace,
            materialized_types: &input.materialized_types,
        },
        &input.behavior,
        &origin,
        intersect_model::definitions::ecore::ParseLimits::default(),
    )?;
    let metamodel = Metamodel::freeze(prepared.schema()).map_err(|e| e.to_string())?;
    let mut store = FrozenStore::with_index_policy(
        std::sync::Arc::new(metamodel),
        intersect_model::storage::IndexPolicy::selected(std::iter::empty::<&str>()),
    );
    intersect_model::storage::atomic(&mut store, |s| {
        for (id, class) in [("child", "Child"), ("wanted", "Wanted"), ("other", "Other")] {
            s.create_element(prepared.schema(), id, class, "memory:inverse-model", 0, 1)?;
        }
        Ok(())
    })?;
    let policy = Policy {
        source: prepared.structure_selection().entry.clone(),
        kind: Cell::new(0),
        ordinary: false,
        bad: false,
    };
    let read = |s: &FrozenStore, b: &mut QueryBudget| {
        SemanticReadSession::new(&prepared, s, &policy)?.value("Child", "child", "selected", b)
    };
    assert_eq!(read(&store, &mut QueryBudget::new(100_000))?, Value::Null);
    assert_eq!(policy.kind.get(), 3);
    intersect_model::storage::atomic(&mut store, |s| {
        s.set_value("child", "owner", Some(json!("wanted")))?;
        s.set_value("wanted", "children", Some(json!(["child"])))
    })?;
    assert_eq!(
        read(&store, &mut QueryBudget::new(100_000))?,
        json!("wanted")
    );
    assert_eq!(policy.kind.get(), 1);
    let handle = store.handle("child").ok_or("missing handle")?;
    assert!(read(&store, &mut QueryBudget::new(1)).is_err());
    assert_eq!(
        read(&store, &mut QueryBudget::new(100_000))?,
        json!("wanted")
    );
    store.inspect_handle(&handle, &mut |object| {
        assert_eq!(object.id(), "child");
        Ok(())
    })?;
    // Owner exists but lacks the source containment occurrence.
    intersect_model::storage::atomic(&mut store, |s| {
        s.set_value("wanted", "children", Some(json!([])))
    })?;
    assert!(read(&store, &mut QueryBudget::new(100_000)).is_err());
    // A genuine nonmatching inverse owner is distinct from source absence.
    intersect_model::storage::atomic(&mut store, |s| {
        s.set_value("child", "owner", Some(json!("other")))?;
        s.set_value("other", "children", Some(json!(["child"])))
    })?;
    assert_eq!(read(&store, &mut QueryBudget::new(100_000))?, Value::Null);
    assert_eq!(policy.kind.get(), 2);
    // Multiply owned and malformed/duplicate containment never produce an ID.
    intersect_model::storage::atomic(&mut store, |s| {
        s.set_value("wanted", "children", Some(json!(["child"])))
    })?;
    assert!(read(&store, &mut QueryBudget::new(100_000)).is_err());
    intersect_model::storage::atomic(&mut store, |s| {
        s.set_value("wanted", "children", Some(json!([])))?;
        s.set_value("other", "children", Some(json!(["child", "child"])))
    })?;
    assert!(read(&store, &mut QueryBudget::new(100_000)).is_err());
    intersect_model::storage::atomic(&mut store, |s| {
        s.set_value("other", "children", Some(json!("child")))
    })?;
    assert!(read(&store, &mut QueryBudget::new(100_000)).is_err());
    intersect_model::storage::atomic(&mut store, |s| {
        s.set_value("other", "children", Some(json!(["child"])))
    })?;
    let ordinary = Policy {
        source: policy.source.clone(),
        kind: Cell::new(0),
        ordinary: true,
        bad: false,
    };
    assert!(
        SemanticReadSession::new(&prepared, &store, &ordinary)?
            .value("Child", "child", "selected", &mut QueryBudget::new(100_000))
            .is_err()
    );
    let bad = Policy {
        source: policy.source.clone(),
        kind: Cell::new(0),
        ordinary: false,
        bad: true,
    };
    assert!(
        SemanticReadSession::new(&prepared, &store, &bad)?
            .value("Child", "child", "selected", &mut QueryBudget::new(100_000))
            .is_err()
    );
    let changing = ChangingRevision {
        store: &store,
        calls: Cell::new(0),
    };
    assert!(
        SemanticReadSession::new(&prepared, &changing, &policy)?
            .value("Child", "child", "selected", &mut QueryBudget::new(100_000))
            .is_err()
    );
    let mut full = QueryBudget::new(100_000);
    read(&store, &mut full)?;
    assert!(full.reads > 1);
    assert!(read(&store, &mut QueryBudget::new(full.reads - 1)).is_err());
    assert_eq!(read(&store, &mut QueryBudget::new(100_000))?, Value::Null);
    let mut foreign = policy.source.clone();
    foreign.sha256 = "foreign".into();
    let foreign_policy = Policy {
        source: foreign,
        kind: Cell::new(0),
        ordinary: false,
        bad: false,
    };
    assert!(SemanticReadSession::new(&prepared, &store, &foreign_policy).is_err());
    assert_eq!(store.value("child", "owner")?, Some(json!("other")));
    store.inspect_handle(&handle, &mut |object| {
        assert_eq!(object.id(), "child");
        Ok(())
    })?;
    // Fresh XMI candidates use ModelView; its complete collection index is
    // checked independently of the live store's overapproximate candidates.
    let model = store.into_model()?;
    let view = intersect_model::storage::ModelView::new(&model)?;
    assert_eq!(
        SemanticReadSession::new(&prepared, &view, &policy)?.value(
            "Child",
            "child",
            "selected",
            &mut QueryBudget::new(100_000)
        )?,
        Value::Null
    );
    assert_eq!(policy.kind.get(), 2);
    let mut multiply_owned = model.clone();
    multiply_owned
        .elements
        .iter_mut()
        .find(|e| e.id == "wanted")
        .ok_or("missing duplicate owner")?
        .values
        .insert("children".into(), json!(["child"]));
    let bad_view = intersect_model::storage::ModelView::new(&multiply_owned)?;
    assert!(
        SemanticReadSession::new(&prepared, &bad_view, &policy)?
            .value("Child", "child", "selected", &mut QueryBudget::new(100_000))
            .is_err()
    );
    Ok(())
}
