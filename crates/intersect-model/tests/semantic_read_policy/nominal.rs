//! Authored nominal dependency controls; no imported-language validity award.
use intersect_model::definitions::preparation::PreparedModelDefinition;
use intersect_model::{
    core::{frozen::FrozenStore, query::QueryBudget, storage::ModelStore},
    definitions::{
        bundle::{DefinitionBundle, DefinitionSource, SourceId, SourceKind},
        ecore_migration::BundleMigrationReport,
        model_definition::ModelCompilation,
        schema::MemberIdentity,
    },
    semantic_reads::{
        ClassifiedValue, NominalRead, ReadDependencies, ReadMember, ReadPlan, SemanticReadPolicy,
        SemanticReadSession,
    },
};
use serde_json::{Value, json};

fn inputs() -> Result<(ModelCompilation, BundleMigrationReport), String> {
    let ecore = DefinitionSource::new(
        "memory:nominal/model.ecore",
        SourceKind::Ecore,
        r##"<ecore:EPackage xmlns:ecore="http://www.eclipse.org/emf/2002/Ecore" xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance" name="nominal" nsURI="urn:nominal" nsPrefix="n">
<eClassifiers xsi:type="ecore:EClass" name="Target"/>
<eClassifiers xsi:type="ecore:EClass" name="Wanted" eSuperTypes="#//Target"/>
<eClassifiers xsi:type="ecore:EClass" name="NestedWanted" eSuperTypes="#//Wanted"/>
<eClassifiers xsi:type="ecore:EClass" name="Other" eSuperTypes="#//Target"/>
<eClassifiers xsi:type="ecore:EClass" name="Holder">
<eStructuralFeatures xsi:type="ecore:EReference" name="related" eType="#//Target" upperBound="-1" containment="true"/>
<eStructuralFeatures xsi:type="ecore:EReference" name="member" eType="#//Target" derived="true"/>
<eStructuralFeatures xsi:type="ecore:EAttribute" name="forbidden" eType="ecore:EDataType http://www.eclipse.org/emf/2002/Ecore#//EString"/>
</eClassifiers></ecore:EPackage>"##,
    );
    let mut behavior = DefinitionSource::new(
        "memory:nominal/behavior.rcore",
        SourceKind::RcoreBehavior,
        "rcore 1; package nominalRules version \"1\";",
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
        namespace: "urn:nominal".into(),
        materialized_types: vec![],
    };
    let origin = intersect_model::migration::migrate_ecore_bundle(
        original.bundle.clone(),
        &[(ecore.id, "memory:nominal/model.rcore".into())].into(),
    )
    .map_err(|e| e.to_string())?;
    Ok((origin.remap_model_compilation(&original)?, origin))
}
const EDGE: &[NominalRead<'static>] = &[NominalRead {
    key: "kind",
    reference: "related",
    nominal: "Wanted",
}];
const BAD: &[NominalRead<'static>] = &[NominalRead {
    key: "kind",
    reference: "forbidden",
    nominal: "Wanted",
}];
struct Policy {
    source: SourceId,
    forbidden_key: bool,
    bad_source_role: bool,
}
impl SemanticReadPolicy for Policy {
    fn validate(&self, definition: &PreparedModelDefinition<'_>) -> Result<(), String> {
        if *definition.structure_selection().entry != self.source {
            return Err("Foreign nominal test source".into());
        }
        Ok(())
    }
    fn plan(&self, member: ReadMember<'_>) -> ReadPlan {
        if !matches!(member.identity, MemberIdentity::Source { declaration } if declaration.source == self.source)
        {
            return ReadPlan::Unsupported;
        }
        match (member.actual, member.feature.name.as_str()) {
            ("Holder", "related" | "forbidden") => ReadPlan::Stored,
            ("Holder", "member") => ReadPlan::Derived,
            _ => ReadPlan::Unsupported,
        }
    }
    fn dependencies(&self, _: ReadMember<'_>) -> &[&str] {
        &[]
    }
    fn nominal_dependencies(&self, member: ReadMember<'_>) -> &[NominalRead<'_>] {
        if member.feature.name != "member" {
            &[]
        } else if self.bad_source_role {
            BAD
        } else {
            EDGE
        }
    }
    fn read(
        &self,
        _: ReadMember<'_>,
        deps: &dyn ReadDependencies,
        budget: &mut QueryBudget,
    ) -> Result<Value, String> {
        match deps.read_nominal(
            if self.forbidden_key {
                "undeclared"
            } else {
                "kind"
            },
            0,
            budget,
        )? {
            ClassifiedValue::Match(value) => Ok(value),
            ClassifiedValue::NonMatch | ClassifiedValue::Absent => Ok(Value::Null),
        }
    }
}

#[test]
fn nominal_filter_uses_first_occurrence_and_real_subtype_without_getter_dispatch()
-> Result<(), String> {
    let (input, origin) = inputs()?;
    let prepared = super::controls::prepare(&input, &origin)?;
    let mut store = FrozenStore::for_schema(prepared.schema()).map_err(|e| e.to_string())?;
    intersect_model::storage::atomic(&mut store, |s| {
        for (id, class) in [
            ("holder", "Holder"),
            ("wanted", "NestedWanted"),
            ("other", "Other"),
        ] {
            s.create_element(prepared.schema(), id, class, "memory:nominal-model", 0, 1)?;
        }
        s.set_value("holder", "related", Some(json!(["other", "wanted"])))
    })?;
    let policy = Policy {
        source: prepared.structure_selection().entry.clone(),
        forbidden_key: false,
        bad_source_role: false,
    };
    // Wrong-kind FIRST target is a legitimate null, not a first-matching scan.
    assert_eq!(
        SemanticReadSession::new(&prepared, &store, &policy)?.value(
            "Holder",
            "holder",
            "member",
            &mut QueryBudget::new(1_000_000)
        )?,
        Value::Null
    );
    intersect_model::storage::atomic(&mut store, |s| {
        s.set_value("holder", "related", Some(json!(["wanted", "other"])))
    })?;
    // NestedWanted is a genuine subtype; no exact-target getter case is selected.
    let session = SemanticReadSession::new(&prepared, &store, &policy)?;
    let mut budget = QueryBudget::new(1_000_000);
    assert_eq!(
        session.value("Holder", "holder", "member", &mut budget)?,
        json!("wanted")
    );
    let used = budget.reads;
    assert_eq!(
        session.value("Holder", "holder", "member", &mut QueryBudget::new(used))?,
        json!("wanted")
    );
    assert!(
        session
            .value(
                "Holder",
                "holder",
                "member",
                &mut QueryBudget::new(used - 1)
            )
            .is_err()
    );
    intersect_model::storage::atomic(&mut store, |s| {
        s.set_value("holder", "related", Some(json!([])))
    })?;
    assert_eq!(
        SemanticReadSession::new(&prepared, &store, &policy)?.value(
            "Holder",
            "holder",
            "member",
            &mut QueryBudget::new(1_000_000)
        )?,
        Value::Null
    );
    Ok(())
}

#[test]
fn nominal_dependencies_reject_forbidden_keys_and_nonreference_sources() -> Result<(), String> {
    let (input, origin) = inputs()?;
    let prepared = super::controls::prepare(&input, &origin)?;
    let mut store = FrozenStore::for_schema(prepared.schema()).map_err(|e| e.to_string())?;
    intersect_model::storage::atomic(&mut store, |s| {
        s.create_element(
            prepared.schema(),
            "holder",
            "Holder",
            "memory:nominal-model",
            0,
            1,
        )
    })?;
    let trace = super::controls::Trace {
        inner: &store,
        calls: Default::default(),
    };
    let bad = Policy {
        source: prepared.structure_selection().entry.clone(),
        forbidden_key: false,
        bad_source_role: true,
    };
    let error = SemanticReadSession::new(&prepared, &trace, &bad)?
        .value(
            "Holder",
            "holder",
            "member",
            &mut QueryBudget::new(1_000_000),
        )
        .err()
        .ok_or("Expected forbidden nominal source role")?;
    assert!(error.contains("ordinary stored reference"), "{error}");
    assert!(trace.calls.borrow().is_empty());
    let forbidden = Policy {
        source: prepared.structure_selection().entry.clone(),
        forbidden_key: true,
        bad_source_role: false,
    };
    let error = SemanticReadSession::new(&prepared, &trace, &forbidden)?
        .value(
            "Holder",
            "holder",
            "member",
            &mut QueryBudget::new(1_000_000),
        )
        .err()
        .ok_or("Expected forbidden nominal key")?;
    assert!(
        error.contains("Undeclared nominal reference dependency key"),
        "{error}"
    );
    assert!(
        trace
            .calls
            .borrow()
            .iter()
            .all(|call| !call.contains("wanted"))
    );
    Ok(())
}
