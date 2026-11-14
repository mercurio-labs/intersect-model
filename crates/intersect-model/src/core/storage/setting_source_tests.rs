//! Small authored metadata/service oracle, not original KerML/K01 qualification.
//! Metadata preparation is a separate fixture component. Each invocation below
//! binds, constructs, admits its lease and applies edits on ONE finite 100k-or-
//! shorter QueryBudget; no work is refunded or silently replenished.
use super::*;
use crate::core::frozen::FrozenStore;
use crate::core::storage::pending_source::tests::{definition, inputs_for};
use crate::definitions::preparation::{PreparedModelDefinition, PreparedPhysicalMetadata};
use serde_json::json;
use std::cell::{Cell, RefCell};

const XML: &str = r##"<e:EPackage xmlns:e="http://www.eclipse.org/emf/2002/Ecore" xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance" name="reuse" nsURI="urn:source-reuse"><eClassifiers xsi:type="e:EClass" name="A"><eStructuralFeatures xsi:type="e:EAttribute" name="label" defaultValueLiteral="base" eType="e:EDataType http://www.eclipse.org/emf/2002/Ecore#//EString"/><eStructuralFeatures xsi:type="e:EReference" name="children" containment="true" upperBound="-1" eType="#//A"/><eStructuralFeatures xsi:type="e:EAttribute" name="computed" derived="true" changeable="true" upperBound="-1" eType="e:EDataType http://www.eclipse.org/emf/2002/Ecore#//EString"/><eStructuralFeatures xsi:type="e:EAttribute" name="f00" upperBound="-1" eType="e:EDataType http://www.eclipse.org/emf/2002/Ecore#//EString"/><eStructuralFeatures xsi:type="e:EAttribute" name="f01" upperBound="-1" eType="e:EDataType http://www.eclipse.org/emf/2002/Ecore#//EString"/><eStructuralFeatures xsi:type="e:EAttribute" name="f02" upperBound="-1" eType="e:EDataType http://www.eclipse.org/emf/2002/Ecore#//EString"/><eStructuralFeatures xsi:type="e:EAttribute" name="f03" upperBound="-1" eType="e:EDataType http://www.eclipse.org/emf/2002/Ecore#//EString"/><eStructuralFeatures xsi:type="e:EAttribute" name="f04" upperBound="-1" eType="e:EDataType http://www.eclipse.org/emf/2002/Ecore#//EString"/><eStructuralFeatures xsi:type="e:EAttribute" name="f05" upperBound="-1" eType="e:EDataType http://www.eclipse.org/emf/2002/Ecore#//EString"/><eStructuralFeatures xsi:type="e:EAttribute" name="f06" upperBound="-1" eType="e:EDataType http://www.eclipse.org/emf/2002/Ecore#//EString"/><eStructuralFeatures xsi:type="e:EAttribute" name="f07" upperBound="-1" eType="e:EDataType http://www.eclipse.org/emf/2002/Ecore#//EString"/></eClassifiers></e:EPackage>"##;
const URI: &str = "memory:setting-source";

struct Native {
    rows: Vec<DraftClassContract>,
    equal: Vec<DraftClassContract>,
    changed: Vec<DraftClassContract>,
    alternate: bool,
    calls: Cell<usize>,
    drift_at: Cell<Option<usize>>,
}
impl DraftPhysicalDefaults for Native {
    fn contracts(&self) -> &[DraftClassContract] {
        let count = self.calls.get() + 1;
        self.calls.set(count);
        if self.drift_at.get().is_some_and(|at| count >= at) {
            &self.changed
        } else if self.alternate && count.is_multiple_of(2) {
            &self.equal
        } else {
            &self.rows
        }
    }
}
fn native(d: &PreparedModelDefinition<'_>, alternate: bool) -> StorageResult<Native> {
    let row = DraftClassContract {
        version: 1,
        schema_revision: d.schema().revision.clone(),
        class: "A".into(),
        class_identity: d.schema().identity("A"),
        sources: d.sources().sources.iter().map(|s| s.id.clone()).collect(),
        members: d
            .schema()
            .effective_features("A")?
            .into_iter()
            .filter(|f| !f.feature.derived)
            .map(|f| DraftMemberDisposition {
                name: f.feature.name.clone(),
                identity: f.identity,
                disposition: if f.feature.name != "label" {
                    DraftDisposition::DelegatedAbsent
                } else {
                    DraftDisposition::StoredDefault
                },
            })
            .collect(),
    };
    let rows = vec![row];
    let equal = rows.clone();
    let mut changed = rows.clone();
    changed[0].schema_revision = "changed-live-revision".into();
    Ok(Native {
        rows,
        equal,
        changed,
        alternate,
        calls: Cell::new(0),
        drift_at: Cell::new(None),
    })
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Case {
    Valid,
    ReadError,
    Drift,
    Range,
    ForeignMetadata,
    ForeignContext,
    OwnerSource,
    Unwind,
}
struct Provider<'a> {
    schema: &'a Schema,
    case: Case,
    events: RefCell<Vec<&'static str>>,
}
impl DerivedValues for Provider<'_> {
    fn schema_revision(&self) -> Option<&str> {
        Some(&self.schema.revision)
    }
    fn supports(&self, class: &str, feature: &str) -> bool {
        class == "A" && feature == "computed"
    }
    fn read(
        &self,
        store: &dyn ModelRead,
        id: &str,
        _: &str,
        budget: &mut QueryBudget,
    ) -> StorageResult<Value> {
        budget.charge()?;
        self.events.borrow_mut().push("read");
        if self.case == Case::ReadError {
            return Err("Setting oracle readback denied".into());
        }
        if self.case == Case::Unwind {
            panic!("Setting oracle readback unwind");
        }
        let row = store.get(id)?.ok_or("Missing setting oracle receiver")?;
        Ok(json!([row
            .values
            .get("label")
            .ok_or("Missing stored oracle label")?]))
    }
}
impl SettingValues for Provider<'_> {
    fn plan(
        &self,
        _: &dyn ModelRead,
        id: &str,
        _: &str,
        edit: &ComputedEdit,
        budget: &mut QueryBudget,
    ) -> StorageResult<Vec<SettingEdit>> {
        budget.charge()?;
        self.events.borrow_mut().push("plan");
        if !matches!(edit, ComputedEdit::Append(Value::String(value)) if value == "ok") {
            return Err("Setting oracle requires exact append".into());
        }
        let mut plan = Vec::new();
        for index in 0..3 {
            plan.push(SettingEdit::Create {
                id: format!("child{index}"),
                class: "A".into(),
                resource: URI.into(),
                start: if self.case == Case::Range { 9 } else { 0 },
                end: if self.case == Case::Range { 10 } else { 1 },
            });
        }
        plan.push(SettingEdit::Set {
            id: id.into(),
            feature: "label".into(),
            value: json!("ok"),
        });
        Ok(plan)
    }
}

fn invoke(
    d: &PreparedModelDefinition<'_>,
    metadata: &PreparedPhysicalMetadata<'_>,
    leased: bool,
    registry: bool,
    alternate: bool,
    case: Case,
    limit: usize,
) -> StorageResult<Value> {
    invoke_on(
        FrozenStore::for_schema(d.schema()).map_err(|error| error.to_string())?,
        (d, metadata),
        leased,
        registry,
        alternate,
        case,
        limit,
    )
}

fn invoke_on(
    mut store: impl DraftConstructionStore,
    (d, metadata): (&PreparedModelDefinition<'_>, &PreparedPhysicalMetadata<'_>),
    leased: bool,
    registry: bool,
    alternate: bool,
    case: Case,
    limit: usize,
) -> StorageResult<Value> {
    let n = native(d, alternate)?;
    let mut budget = QueryBudget::new(limit);
    let context = DraftConstructionContext::bind(d.schema(), d.sources(), &n, &mut budget)?;
    let transaction = store.begin()?;
    bind_schema(&mut store, d.schema())?;
    context.create_pending(
        &mut store,
        Construction {
            id: "owner",
            resource: URI,
            authored: &[],
        },
        "A",
        0,
        1,
        metadata,
        &mut budget,
    )?;
    let before = export(&store)?;
    let foreign_context = if case == Case::ForeignContext {
        Some(DraftConstructionContext::bind(
            d.schema(),
            d.sources(),
            &n,
            &mut budget,
        )?)
    } else {
        None
    };
    let foreign_metadata = if case == Case::ForeignMetadata {
        Some(
            d.prepare_physical_metadata(&InvocationContext::default())
                .map_err(|e| e.to_string())?,
        )
    } else {
        None
    };
    let sources = PendingSourceInputs::new(metadata, &mut budget)?;
    let lease = context.source_lease(metadata, &sources, &mut budget)?;
    n.calls.set(0);
    if case == Case::Drift {
        n.drift_at.set(Some(3));
    }
    let provider = Provider {
        schema: d.schema(),
        case,
        events: RefCell::new(Vec::new()),
    };
    let conversions = ConversionRegistry::default();
    let settings = ComputedSettings {
        schema: d.schema(),
        provider: &provider,
        conversions: registry.then_some(&conversions),
    };
    let current_context = foreign_context.as_ref().unwrap_or(&context);
    let current_metadata = foreign_metadata.as_ref().unwrap_or(metadata);
    let resource = if case == Case::OwnerSource {
        "memory:foreign-setting-source"
    } else {
        URI
    };
    let returned = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        if leased {
            settings.apply_with_draft_construction_source(
                &mut store,
                "owner",
                "computed",
                ComputedEdit::Append(json!("ok")),
                &mut budget,
                current_context,
                current_metadata,
                &lease,
                Some(16),
                (resource, "abc"),
            )
        } else {
            settings.apply_with_draft_construction(
                &mut store,
                "owner",
                "computed",
                ComputedEdit::Append(json!("ok")),
                &mut budget,
                current_context,
                current_metadata,
                Some(16),
                (resource, "abc"),
            )
        }
    }));
    let unwound = returned.is_err();
    let result = match returned {
        Ok(result) => result,
        Err(_) => Err("Setting oracle caught unwind".into()),
    };
    let model = export(&store)?;
    let callbacks = n.calls.get();
    let work = budget.reads;
    let mut retry = None;
    if leased && result.is_err() && budget.remaining() > 32 {
        let count = n.calls.get();
        let retried = lease.create_pending(
            &mut store,
            Construction {
                id: "retry",
                resource: URI,
                authored: &[],
            },
            "A",
            0,
            1,
            &mut budget,
        );
        assert!(retried.is_err(), "Failed setting lease revived");
        assert_eq!(
            n.calls.get(),
            count,
            "Poisoned retry invoked native provider"
        );
        assert_eq!(
            serde_json::to_value(export(&store)?).map_err(|e| e.to_string())?,
            serde_json::to_value(&model).map_err(|e| e.to_string())?
        );
        retry = Some(retried.err().ok_or("Missing rejected retry cause")?);
    }
    let output = json!({"result": result, "unwound": unwound, "before": before, "model": model,
        "events": *provider.events.borrow(), "callbacks": callbacks, "work":work,
        "logical":sources.logical_bytes(), "lease_logical":lease.logical_bytes(), "retry":retry});
    store.rollback(&transaction)?;
    Ok(output)
}

fn whole(result: &Value) -> Value {
    json!({"result":result["result"], "unwound":result["unwound"],
        "model":result["model"], "events":result["events"], "callbacks":result["callbacks"]})
}
macro_rules! fixture {
    ($d:ident,$m:ident) => {
        let (input, migration) = inputs_for(XML)?;
        let $d = definition(&input, &migration)?;
        let $m = $d
            .prepare_physical_metadata(&InvocationContext::default())
            .map_err(|e| e.to_string())?;
    };
}

#[test]
fn leased_computed_settings_whole_callbacks_and_conversion_routes() -> StorageResult<()> {
    fixture!(d, metadata);
    for registry in [false, true] {
        for alternate in [false, true] {
            let old = invoke(
                &d,
                &metadata,
                false,
                registry,
                alternate,
                Case::Valid,
                100_000,
            )?;
            let new = invoke(
                &d,
                &metadata,
                true,
                registry,
                alternate,
                Case::Valid,
                100_000,
            )?;
            assert_eq!(new["result"], json!({"Ok":null}));
            assert_eq!(whole(&new), whole(&old));
            assert_eq!(new["events"], json!(["plan", "read"]));
            if !alternate {
                assert!(new["work"].as_u64() < old["work"].as_u64());
            }
            println!(
                "LEASED_COMPUTED_SETTINGS_PARITY_RAW {}",
                json!({"registry":registry,
                "equal_distinct_slices":alternate,"old":old,"new":new,
                "whole_graph_audits_preserved":true,"no_total_memory_claim":true})
            );
        }
    }
    Ok(())
}

#[test]
fn leased_computed_settings_exact_short_and_atomic_poison() -> StorageResult<()> {
    fixture!(d, metadata);
    let full = invoke(&d, &metadata, true, false, false, Case::Valid, 100_000)?;
    let needed = usize::try_from(
        full["work"]
            .as_u64()
            .ok_or("Missing measured setting work")?,
    )
    .map_err(|_| "Setting work overflow")?;
    for limit in [needed - 1, needed] {
        let actual = invoke(&d, &metadata, true, false, false, Case::Valid, limit)?;
        assert_eq!(actual["work"], json!(limit));
        if limit == needed {
            assert_eq!(whole(&actual), whole(&full));
        } else {
            assert_eq!(
                actual["result"],
                json!({"Err":"Query work budget exhausted; no complete result"})
            );
            assert_eq!(actual["model"], actual["before"]);
        }
    }
    assert!(invoke(&d, &metadata, true, false, false, Case::Valid, 0).is_err());
    for case in [Case::ReadError, Case::Drift, Case::Range] {
        let old = invoke(&d, &metadata, false, false, false, case, 100_000)?;
        let new = invoke(&d, &metadata, true, false, false, case, 100_000)?;
        assert_eq!(whole(&new), whole(&old));
        assert!(new["result"].get("Err").is_some());
        assert_eq!(new["model"], new["before"]);
        assert!(new["retry"].as_str().is_some_and(|e| e.contains("failed")));
    }
    Ok(())
}

#[test]
fn leased_computed_settings_foreign_associations_owner_and_unwind() -> StorageResult<()> {
    fixture!(d, metadata);
    for case in [
        Case::ForeignMetadata,
        Case::ForeignContext,
        Case::OwnerSource,
    ] {
        let actual = invoke(&d, &metadata, true, false, false, case, 100_000)?;
        let error = actual["result"]["Err"]
            .as_str()
            .ok_or("Missing foreign setting rejection")?;
        let expected = match case {
            Case::ForeignMetadata => "source metadata association changed",
            Case::ForeignContext => "source context changed or failed",
            _ => "owner belongs to another source",
        };
        assert!(
            error.contains(expected),
            "Unexpected setting rejection: {error}"
        );
        assert_eq!(actual["events"], json!([]));
        assert_eq!(actual["callbacks"], json!(0));
        assert_eq!(actual["model"], actual["before"]);
        assert!(
            actual["retry"]
                .as_str()
                .is_some_and(|e| e.contains("failed"))
        );
    }
    let old = invoke(&d, &metadata, false, false, false, Case::Unwind, 100_000)?;
    let new = invoke(&d, &metadata, true, false, false, Case::Unwind, 100_000)?;
    assert_eq!(whole(&new), whole(&old));
    assert_eq!(new["unwound"], json!(true));
    assert_eq!(new["model"], new["before"]);
    assert!(new["retry"].as_str().is_some_and(|e| e.contains("failed")));
    Ok(())
}
