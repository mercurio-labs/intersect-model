//! SOURCE-owner completion reuse controls, not original36 admission or model semantics.
use super::*;
use crate::core::frozen::FrozenStore;
use crate::core::storage::pending_source::tests::{
    Meter, definition as ordinary_definition, inputs as ordinary_inputs,
};
use crate::definitions::{
    bundle::{DefinitionSource, SourceKind},
    conversion::{InvocationObserver, ProviderId},
    ecore::ParseLimits,
    ecore_migration::BundleMigrationReport,
    model_definition::ModelCompilation,
    preparation::{PreparedModelDefinition, PreparedPhysicalMetadata},
};
use std::{cell::Cell, rc::Rc, sync::Arc};

fn inputs() -> StorageResult<(ModelCompilation, BundleMigrationReport)> {
    let model = DefinitionSource::new(
        "memory:compiled-proof/Gate.ecore",
        SourceKind::Ecore,
        include_str!("../../../../../fixtures/xtext-whole-iteration-v76/GateModel.ecore"),
    );
    let mut syntax = DefinitionSource::new(
        "memory:compiled-proof/Iteration.xtext",
        SourceKind::Xtext,
        include_str!("../../../../../fixtures/xtext-whole-iteration-v76/IterationX.xtext"),
    );
    syntax.dependencies.push(model.id.clone());
    // Authored valid source padding makes actual canonical hashing work material;
    // this is a component fixture, never a substitute original language source.
    let behavior = format!(
        "rcore 1; package emptyRules version \"1\";\n// {}\n",
        "x".repeat(128_000)
    );
    let mut rules = DefinitionSource::new(
        "memory:compiled-proof/rules",
        SourceKind::RcoreBehavior,
        behavior,
    );
    rules.dependencies.push(model.id.clone());
    let original = ModelCompilation {
        materialized_types: vec![],
        structure: model.id.clone(),
        behavior: rules.id.clone(),
        namespace: "urn:intersect:oracle:WholeIterationGate".into(),
        bundle: DefinitionBundle {
            version: 1,
            sources: vec![model.clone(), syntax, rules],
            namespaces: vec![],
        },
    };
    let origin = crate::migration::migrate_ecore_bundle_with_limits(
        original.bundle.clone(),
        &[(model.id, "memory:compiled-proof/Gate.rcore".into())].into(),
        ParseLimits::default(),
    )
    .map_err(|e| e.to_string())?;
    Ok((origin.remap_model_compilation(&original)?, origin))
}
fn owner(
    input: ModelCompilation,
) -> StorageResult<crate::definitions::model_definition::OwnedModelDefinition> {
    use crate::definitions::model_definition::{PhysicalInputCapture, StructureSelection};
    let bundle = Arc::new(input.bundle);
    crate::definitions::model_test_support::compile(
        StructureSelection {
            bundle: &bundle,
            entry: &input.structure,
            namespace: &input.namespace,
            materialized_types: &input.materialized_types,
        },
        &input.behavior,
        PhysicalInputCapture::Retain,
        ParseLimits::default(),
        true,
    )?
    .into_owned(Arc::clone(&bundle))
}
struct Native<'a> {
    rows: Vec<DraftClassContract>,
    changed: Vec<DraftClassContract>,
    calls: Cell<usize>,
    drift: bool,
    cancel: Option<&'a Cell<bool>>,
}
impl DraftPhysicalDefaults for Native<'_> {
    fn contracts(&self) -> &[DraftClassContract] {
        let call = self.calls.get() + 1;
        self.calls.set(call);
        if call == 2
            && let Some(cancel) = self.cancel
        {
            cancel.set(true);
        }
        if self.drift && call > 1 {
            &self.changed
        } else {
            &self.rows
        }
    }
}
fn native<'a>(definition: &PreparedModelDefinition<'_>) -> StorageResult<Native<'a>> {
    let schema = definition.schema();
    let mut rows = Vec::new();
    for (class, header) in &schema.classes {
        if header.abstract_type {
            continue;
        }
        rows.push(DraftClassContract {
            version: 1,
            schema_revision: schema.revision.clone(),
            class: class.clone(),
            class_identity: schema.identity(class),
            sources: definition
                .sources()
                .sources
                .iter()
                .map(|s| s.id.clone())
                .collect(),
            members: schema
                .effective_features(class)?
                .into_iter()
                .filter(|f| !f.feature.derived)
                .map(|f| DraftMemberDisposition {
                    name: f.feature.name.clone(),
                    identity: f.identity,
                    disposition: DraftDisposition::StoredDefault,
                })
                .collect(),
        });
    }
    let mut changed = rows.clone();
    changed
        .first_mut()
        .ok_or("No compiled fixture native classes")?
        .schema_revision
        .push_str("/drift");
    Ok(Native {
        rows,
        changed,
        calls: Cell::new(0),
        drift: false,
        cancel: None,
    })
}
fn cold<'a>(
    metadata: &'a PreparedPhysicalMetadata<'_>,
    provider: &dyn DraftPhysicalDefaults,
    observation: &InvocationContext<'_>,
) -> StorageResult<DraftConstructionContext<'a>> {
    let definition = metadata.definition();
    let bound = bind_draft_contracts(
        definition.schema(),
        definition.sources(),
        provider,
        None,
        &mut DraftBindWork::Observed(observation),
        None,
    )?;
    observation.checkpoint()?;
    Ok(DraftConstructionContext {
        schema: definition.schema(),
        provider: None,
        contracts: bound.contracts,
        selected: bound.selected,
    })
}
fn graph(
    context: &DraftConstructionContext<'_>,
    metadata: &PreparedPhysicalMetadata<'_>,
) -> StorageResult<FrozenStore> {
    let mut store = FrozenStore::for_schema(metadata.definition().schema())
        .map_err(|error| error.to_string())?;
    atomic(&mut store, |store| {
        bind_schema(store, metadata.definition().schema())?;
        context.create_pending(
            store,
            Construction {
                id: "target",
                resource: "memory:compiled-proof/model",
                authored: &[],
            },
            "Target",
            0,
            1,
            metadata,
            &mut QueryBudget::new(100_000),
        )?;
        Ok(())
    })?;
    Ok(store)
}
#[test]
fn frozen_compiled_bundle_real_owner_removes_duplicate_hash_work_and_preserves_contracts()
-> StorageResult<()> {
    let (input, origin) = inputs()?;
    let owner = owner(input)?;
    // Moving the borrowed view changes no source/schema owner allocation.
    let view = crate::definitions::model_test_support::prepare_owned(
        &owner,
        &origin,
        ParseLimits::default(),
    )
    .map_err(|e| format!("{e:?}"))?;
    let definition = view;
    let metadata = definition
        .prepare_physical_metadata(&InvocationContext::default())
        .map_err(|e| e.to_string())?;
    assert!(metadata.compiled_sources().is_some());
    let provider = native(&definition)?;
    let old = Meter::new(100_000);
    let a = cold(&metadata, &provider, &InvocationContext::new(old.clone()))?;
    assert_eq!(provider.calls.get(), 2);
    provider.calls.set(0);
    let new = Meter::new(100_000);
    let b = DraftConstructionContext::bind_frozen(
        &metadata,
        &provider,
        &InvocationContext::new(new.clone()),
    )?;
    println!(
        "FROZEN_COMPILED_BUNDLE_REUSE_RAW {}",
        serde_json::json!({"old_work":old.work(),"new_work":new.work(),
        "old_bytes":old.bytes.get(),"new_bytes":new.bytes.get(),"actual_source_bytes":definition.sources().sources.iter().map(|s|s.text.len()).sum::<usize>(),
        "grant_each":100_000,"original36_fit":false})
    );
    assert_eq!(provider.calls.get(), 2);
    assert!(
        new.work() < old.work(),
        "Remove actual duplicate complete hash validation"
    );
    assert!(
        old.work() - new.work() > 128,
        "Material paid source-chunk traversal removed"
    );
    assert!(new.bytes.get() < old.bytes.get());
    assert_eq!(
        serde_json::to_value(&a.contracts).map_err(|e| e.to_string())?,
        serde_json::to_value(&b.contracts).map_err(|e| e.to_string())?
    );
    assert_eq!(
        export(&graph(&a, &metadata)?)?,
        export(&graph(&b, &metadata)?)?
    );
    let legacy = DraftConstructionContext::bind(
        definition.schema(),
        definition.sources(),
        &provider,
        &mut QueryBudget::new(100_000),
    )?;
    assert_eq!(
        export(&graph(&legacy, &metadata)?)?,
        export(&graph(&b, &metadata)?)?
    );
    Ok(())
}
#[test]
fn frozen_compiled_bundle_equal_foreign_owner_and_forged_bundle_refuse_before_provider()
-> StorageResult<()> {
    let (input, origin) = inputs()?;
    let first = owner(input.clone())?;
    let second = owner(input)?;
    let a = crate::definitions::model_test_support::prepare_owned(
        &first,
        &origin,
        ParseLimits::default(),
    )
    .map_err(|e| format!("{e:?}"))?;
    let b = crate::definitions::model_test_support::prepare_owned(
        &second,
        &origin,
        ParseLimits::default(),
    )
    .map_err(|e| format!("{e:?}"))?;
    let ma = a
        .prepare_physical_metadata(&InvocationContext::default())
        .map_err(|e| e.to_string())?;
    let provider = native(&b)?;
    let proof = ma
        .compiled_sources()
        .ok_or("No completed compiler source proof")?;
    let clone = a.sources().clone();
    let mut forged = clone.clone();
    forged.sources[0].text.push(' ');
    let schema = a.schema().clone();
    for (shape, sources, expected_schema) in [
        ("foreign", b.sources(), b.schema()),
        ("equal-bundle-clone", &clone, a.schema()),
        ("forged-payload", &forged, a.schema()),
        ("equal-schema-clone", a.sources(), &schema),
    ] {
        let result = bind_draft_contracts(
            expected_schema,
            sources,
            &provider,
            Some(proof),
            &mut DraftBindWork::Observed(&InvocationContext::new(Meter::new(100_000))),
            None,
        );
        assert_eq!(
            result.err().as_deref(),
            Some("Frozen compiled source/schema owner association changed"),
            "{shape}"
        );
        assert_eq!(provider.calls.get(), 0);
    }
    // Even an internally supplied real witness cannot change the legacy path.
    // A changed payload still reaches its original canonical digest error.
    let legacy = bind_draft_contracts(
        a.schema(),
        &forged,
        &provider,
        Some(proof),
        &mut DraftBindWork::Legacy(&mut QueryBudget::new(100_000)),
        None,
    );
    assert!(
        legacy
            .err()
            .is_some_and(|e| e.contains("Source content digest mismatch"))
    );
    assert_eq!(provider.calls.get(), 0);
    let mb = b
        .prepare_physical_metadata(&InvocationContext::default())
        .map_err(|e| e.to_string())?;
    DraftConstructionContext::bind_frozen(
        &mb,
        &provider,
        &InvocationContext::new(Meter::new(100_000)),
    )?;
    Ok(())
}
#[test]
fn frozen_compiled_bundle_ordinary_and_synthetic_metadata_keep_cold_source_validation()
-> StorageResult<()> {
    let (input, origin) = ordinary_inputs()?;
    let definition = ordinary_definition(&input, &origin)?;
    let metadata = definition
        .prepare_physical_metadata(&InvocationContext::default())
        .map_err(|e| e.to_string())?;
    assert!(metadata.compiled_sources().is_none());
    let provider = native(&definition)?;
    let old = Meter::new(100_000);
    let new = Meter::new(100_000);
    cold(&metadata, &provider, &InvocationContext::new(old.clone()))?;
    DraftConstructionContext::bind_frozen(
        &metadata,
        &provider,
        &InvocationContext::new(new.clone()),
    )?;
    assert_eq!((old.work(), old.bytes.get()), (new.work(), new.bytes.get()));
    let mut forged = input.clone();
    forged.bundle.sources[0].text.push(' ');
    let synthetic = PreparedModelDefinition::inspection_for_test(
        crate::definitions::model_definition::StructureSelection {
            bundle: &forged.bundle,
            entry: &forged.structure,
            namespace: &forged.namespace,
            materialized_types: &forged.materialized_types,
        },
        &forged.behavior,
        &origin,
        definition.schema().clone(),
        crate::definitions::schema::Semantics {
            scope_bindings: vec![],
            version: 1,
            scope: "source-validation component".into(),
            derived: vec![],
            initializers: vec![],
            validations: vec![],
            rule_conversions: vec![],
        },
    );
    // Completed structural type metadata alone MUST NOT confer bundle authority.
    let raw = synthetic
        .prepare_physical_metadata(&InvocationContext::default())
        .map_err(|e| e.to_string())?;
    assert!(raw.compiled_sources().is_none());
    let bad = native(&synthetic)?;
    let result = DraftConstructionContext::bind_frozen(
        &raw,
        &bad,
        &InvocationContext::new(Meter::new(100_000)),
    );
    let error = result
        .err()
        .ok_or("Synthetic changed source bypassed canonical validation")?;
    assert!(error.contains("Source content digest mismatch"), "{error}");
    assert_eq!(bad.calls.get(), 0);
    Ok(())
}
struct Control {
    meter: Rc<Meter>,
    cancel: Cell<bool>,
}
impl InvocationObserver for Control {
    fn before_invoke(&self, _: &ProviderId) -> StorageResult<()> {
        Err("No semantic provider".into())
    }
    fn checkpoint(&self) -> StorageResult<()> {
        if self.cancel.get() {
            Err("Compiled source binding cancelled".into())
        } else {
            Ok(())
        }
    }
    fn work(&self, n: usize) -> StorageResult<()> {
        self.checkpoint()?;
        InvocationObserver::work(&*self.meter, n)
    }
    fn retain_storage(&self, n: usize) -> StorageResult<()> {
        self.checkpoint()?;
        self.meter.retain_storage(n)
    }
}
#[test]
fn frozen_compiled_bundle_zero_short_exact_preconsumed_cancel_and_final_drift() -> StorageResult<()>
{
    let (input, origin) = inputs()?;
    let owner = owner(input)?;
    let definition = crate::definitions::model_test_support::prepare_owned(
        &owner,
        &origin,
        ParseLimits::default(),
    )
    .map_err(|e| format!("{e:?}"))?;
    let metadata = definition
        .prepare_physical_metadata(&InvocationContext::default())
        .map_err(|e| e.to_string())?;
    let provider = native(&definition)?;
    let measure = Meter::new(100_000);
    DraftConstructionContext::bind_frozen(
        &metadata,
        &provider,
        &InvocationContext::new(measure.clone()),
    )?;
    let exact = measure.work();
    assert!(exact > 7 && exact < 100_000);
    for (grant, prefix, success) in [
        (0, 0, false),
        (exact - 1, 0, false),
        (exact, 0, true),
        (exact + 7, 7, true),
    ] {
        // Each independent bind-budget case starts with the SAME canonical
        // owner and a fresh cold metadata-input cache, matching the measurement.
        // Metadata setup remains outside this existing bind-only component grant.
        let metadata = definition
            .prepare_physical_metadata(&InvocationContext::default())
            .map_err(|e| e.to_string())?;
        let meter = Meter::new(grant);
        let observation = InvocationContext::new(meter.clone());
        observation.work(prefix)?;
        let result = DraftConstructionContext::bind_frozen(&metadata, &provider, &observation);
        assert_eq!(result.is_ok(), success, "grant={grant}");
        assert_eq!(meter.work(), grant);
        if !success {
            let error = result.err().ok_or("Expected actual bind budget refusal")?;
            assert!(
                error.starts_with("Query work budget exhausted; no complete result [draft-bind ")
            );
            assert!(error.len() <= "Query work budget exhausted; no complete result".len() + 384);
            if grant == 0 {
                assert_eq!(
                    error,
                    "Query work budget exhausted; no complete result [draft-bind stage=entry receiver=- member=- inputs=Pending]"
                );
            } else {
                assert!(error.contains("stage=final_contract"), "{error}");
                assert!(error.contains("inputs=Captured"), "{error}");
            }
        }
    }
    let control = Rc::new(Control {
        meter: Meter::new(100_000),
        cancel: Cell::new(true),
    });
    let error = DraftConstructionContext::bind_frozen(
        &metadata,
        &provider,
        &InvocationContext::new(control.clone()),
    )
    .err()
    .ok_or("Cancelled binding published context")?;
    assert_eq!(error, "Compiled source binding cancelled");
    assert_eq!(control.meter.work(), 0);
    control.cancel.set(false);
    let mut last = native(&definition)?;
    last.cancel = Some(&control.cancel);
    let failed = DraftConstructionContext::bind_frozen(
        &metadata,
        &last,
        &InvocationContext::new(control.clone()),
    );
    assert_eq!(last.calls.get(), 2);
    assert_eq!(
        failed.err().as_deref(),
        Some("Compiled source binding cancelled")
    );
    let mut drift = native(&definition)?;
    drift.drift = true;
    let changed = DraftConstructionContext::bind_frozen(
        &metadata,
        &drift,
        &InvocationContext::new(Meter::new(100_000)),
    );
    assert_eq!(
        changed.err().as_deref(),
        Some("Draft physical capability changed after binding")
    );
    assert_eq!(drift.calls.get(), 2);
    Ok(())
}

// Reuse the existing real Ecore/grammar fixture with a real reciprocal reference,
// so canonical metadata validation produces the OPPOSITE-owner Target map.
fn physical_input_fixture() -> StorageResult<(ModelCompilation, BundleMigrationReport)> {
    let (prior, migration) = inputs()?;
    let mut bundle = migration.original.clone();
    let original = bundle.sources.first().ok_or("Missing real Ecore source")?;
    let old_id = original.id.clone();
    let text = original.text.replace(
        "name=\"children\" upperBound=\"-1\"",
        "name=\"children\" eOpposite=\"#//Target/owner\" upperBound=\"-1\"",
    ).replace(
        "name=\"Target\">",
        "name=\"Target\"><eStructuralFeatures xsi:type=\"ecore:EReference\" name=\"owner\" eType=\"#//Root\" eOpposite=\"#//Root/children\"/>",
    );
    if text == original.text {
        return Err("Actual opposite fixture edit did not apply".into());
    }
    let model = DefinitionSource::new(&old_id.uri, SourceKind::Ecore, text);
    bundle.sources[0] = model.clone();
    for source in &mut bundle.sources {
        for dependency in &mut source.dependencies {
            if dependency == &old_id {
                *dependency = model.id.clone();
            }
        }
    }
    let _retained_syntax = bundle
        .sources
        .iter()
        .find(|s| s.kind == SourceKind::Xtext)
        .ok_or("No real fixture syntax")?
        .id
        .clone();
    let original = ModelCompilation {
        materialized_types: prior.materialized_types,
        structure: model.id.clone(),
        behavior: bundle
            .sources
            .iter()
            .find(|s| s.kind == SourceKind::RcoreBehavior)
            .ok_or("No real fixture rules")?
            .id
            .clone(),
        namespace: prior.namespace,
        bundle,
    };
    let migration = crate::migration::migrate_ecore_bundle_with_limits(
        original.bundle.clone(),
        &[(model.id, "memory:compiled-proof/Gate.rcore".into())].into(),
        ParseLimits::default(),
    )
    .map_err(|e| e.to_string())?;
    Ok((migration.remap_model_compilation(&original)?, migration))
}

#[test]
fn frozen_completed_physical_inputs_actual_hit_and_canonical_contract_failures() -> StorageResult<()>
{
    let (input, migration) = physical_input_fixture()?;
    let owner = owner(input)?;
    let definition = crate::definitions::model_test_support::prepare_owned(
        &owner,
        &migration,
        ParseLimits::default(),
    )
    .map_err(|e| format!("{e:?}"))?;
    let preparation = Meter::new(100_000);
    let metadata = definition
        .prepare_physical_metadata(&InvocationContext::new(preparation.clone()))
        .map_err(|e| e.to_string())?;
    let lookup = InvocationContext::new(Meter::new(100_000));
    let first = metadata
        .frozen_feature_inputs(definition.schema(), "Target", &lookup)?
        .ok_or("Canonical real-opposite validation did not retain actual Target inputs")?;
    let second = metadata
        .frozen_feature_inputs(definition.schema(), "Target", &lookup)?
        .ok_or("Same physical map alias disappeared")?;
    assert!(
        Arc::ptr_eq(&first, &second),
        "Must reuse actual canonical map, not reconstruct equivalent data"
    );
    assert!(first.get("owner").is_some());
    let a = metadata
        .frozen_feature_input_cache(definition.schema(), &lookup)?
        .ok_or("No actual retained source-input cache")?;
    let b = metadata
        .frozen_feature_input_cache(definition.schema(), &lookup)?
        .ok_or("No repeated source-input cache")?;
    assert!(Arc::ptr_eq(&a, &b));
    let foreign = definition.schema().clone();
    assert!(
        metadata
            .frozen_feature_inputs(&foreign, "Target", &lookup)
            .is_err()
    );
    assert!(
        metadata
            .frozen_feature_input_cache(&foreign, &lookup)
            .is_err()
    );
    let provider = native(&definition)?;
    let cold_work = Meter::new(100_000);
    // SAME compiler source proof, without prepared-input reuse: isolate map work.
    let old = bind_draft_contracts(
        definition.schema(),
        definition.sources(),
        &provider,
        metadata.compiled_sources(),
        &mut DraftBindWork::Observed(&InvocationContext::new(cold_work.clone())),
        None,
    )?;
    let old = DraftConstructionContext {
        schema: definition.schema(),
        provider: None,
        contracts: old.contracts,
        selected: old.selected,
    };
    provider.calls.set(0);
    let new_work = Meter::new(100_000);
    let current = DraftConstructionContext::bind_frozen(
        &metadata,
        &provider,
        &InvocationContext::new(new_work.clone()),
    )?;
    assert_eq!(
        provider.calls.get(),
        2,
        "Final live-provider drift check remains"
    );
    assert!(
        new_work.work() < cold_work.work(),
        "Actual completed maps must remove duplicate work with SAME source witness"
    );
    assert_eq!(
        serde_json::to_value(&old.contracts).map_err(|e| e.to_string())?,
        serde_json::to_value(&current.contracts).map_err(|e| e.to_string())?
    );
    assert_eq!(
        export(&graph(&old, &metadata)?)?,
        export(&graph(&current, &metadata)?)?
    );
    println!(
        "COMPLETED_PHYSICAL_INPUT_HIT_RAW {}",
        serde_json::json!({
            "same_actual_Target_map":true,"same_actual_source_cache":true,
            "preparation_work":preparation.work(),"preparation_retained_bytes":preparation.bytes.get(),
            "rebuild_with_same_compiler_proof":cold_work.work(),"reuse_work":new_work.work(),
            "original24_fit":false
        })
    );
    for case in ["member", "coverage", "default", "type", "stale"] {
        let mut bad = native(&definition)?;
        match case {
            "member" => {
                let member = bad
                    .rows
                    .iter_mut()
                    .find(|c| c.class == "Root")
                    .ok_or("No actual Root")?
                    .members
                    .first_mut()
                    .ok_or("No actual Root member")?;
                let MemberIdentity::Source { declaration } = &mut member.identity else {
                    return Err("Fixture member lacks original source identity".into());
                };
                declaration.path.push(999);
            }
            "coverage" => {
                bad.rows
                    .iter_mut()
                    .find(|c| c.class == "Target")
                    .ok_or("No actual Target")?
                    .members
                    .pop();
            }
            "default" => {
                bad.rows
                    .iter_mut()
                    .find(|c| c.class == "Target")
                    .ok_or("No actual Target")?
                    .members
                    .iter_mut()
                    .find(|m| m.name == "tag")
                    .ok_or("No actual tag")?
                    .disposition = DraftDisposition::StoredNullDefault;
            }
            "type" => {
                bad.rows
                    .iter_mut()
                    .find(|c| c.class == "Root")
                    .ok_or("No actual Root")?
                    .members
                    .iter_mut()
                    .find(|m| m.name == "children")
                    .ok_or("No actual reference")?
                    .disposition = DraftDisposition::StoredNullDefault;
            }
            _ => bad
                .rows
                .first_mut()
                .ok_or("No actual contract")?
                .schema_revision
                .push_str("/stale"),
        }
        let expected = bind_draft_contracts(
            definition.schema(),
            definition.sources(),
            &bad,
            metadata.compiled_sources(),
            &mut DraftBindWork::Observed(&InvocationContext::new(Meter::new(100_000))),
            None,
        )
        .err()
        .ok_or("Invalid canonical contract was accepted without reuse")?;
        bad.calls.set(0);
        let actual = DraftConstructionContext::bind_frozen(
            &metadata,
            &bad,
            &InvocationContext::new(Meter::new(100_000)),
        )
        .err()
        .ok_or("Prepared inputs accepted invalid canonical contract")?;
        println!(
            "COMPLETED_PHYSICAL_CONTRACT_REFUSAL_RAW {}",
            serde_json::json!({"case":case,"expected":expected,"actual":actual})
        );
        assert_eq!(
            actual, expected,
            "{case}: same canonical member/default/type/coverage/staleness failure"
        );
    }
    Ok(())
}

#[test]
fn frozen_completed_physical_inputs_short_cancel_and_final_provider_guards() -> StorageResult<()> {
    let (input, migration) = physical_input_fixture()?;
    let owner = owner(input)?;
    let definition = crate::definitions::model_test_support::prepare_owned(
        &owner,
        &migration,
        ParseLimits::default(),
    )
    .map_err(|e| format!("{e:?}"))?;
    let measured = Meter::new(100_000);
    let metadata = definition
        .prepare_physical_metadata(&InvocationContext::new(measured.clone()))
        .map_err(|e| e.to_string())?;
    let used = measured.work();
    assert!(used > 1 && used < 100_000);
    for grant in [0, used - 1, used] {
        let meter = Meter::new(grant);
        let result = definition.prepare_physical_metadata(&InvocationContext::new(meter.clone()));
        assert_eq!(
            result.is_ok(),
            grant == used,
            "No metadata authority on incomplete capture/validation"
        );
        assert_eq!(meter.work(), grant);
        if grant == used - 1 {
            assert!(
                meter.bytes.get() > 0,
                "Failed preparation must not pretend retained storage was refunded"
            );
        }
    }
    let cancel = Rc::new(Control {
        meter: Meter::new(100_000),
        cancel: Cell::new(true),
    });
    assert!(
        definition
            .prepare_physical_metadata(&InvocationContext::new(cancel.clone()))
            .is_err()
    );
    assert_eq!(cancel.meter.work(), 0);
    cancel.cancel.set(false);
    let mut provider = native(&definition)?;
    provider.cancel = Some(&cancel.cancel);
    assert_eq!(
        DraftConstructionContext::bind_frozen(
            &metadata,
            &provider,
            &InvocationContext::new(cancel.clone())
        )
        .err()
        .as_deref(),
        Some("Compiled source binding cancelled"),
    );
    assert_eq!(provider.calls.get(), 2);
    cancel.cancel.set(false);
    let mut drift = native(&definition)?;
    drift.drift = true;
    assert_eq!(
        DraftConstructionContext::bind_frozen(
            &metadata,
            &drift,
            &InvocationContext::new(Meter::new(100_000))
        )
        .err()
        .as_deref(),
        Some("Draft physical capability changed after binding"),
    );
    assert_eq!(drift.calls.get(), 2);
    let provider = native(&definition)?;
    let measure = Meter::new(100_000);
    DraftConstructionContext::bind_frozen(
        &metadata,
        &provider,
        &InvocationContext::new(measure.clone()),
    )?;
    let bind_used = measure.work();
    assert!(bind_used > 1);
    for grant in [bind_used - 1, bind_used] {
        let meter = Meter::new(grant);
        let outcome = DraftConstructionContext::bind_frozen(
            &metadata,
            &provider,
            &InvocationContext::new(meter.clone()),
        );
        assert_eq!(outcome.is_ok(), grant == bind_used);
        assert_eq!(meter.work(), grant);
    }
    Ok(())
}

// Real inherited Ecore declarations; no synthetic schema inventory or warmed proof.
fn inherited_owner_fixture() -> StorageResult<(ModelCompilation, BundleMigrationReport)> {
    let (prior, migration) = inputs()?;
    let mut bundle = migration.original.clone();
    let old = bundle.sources.first().ok_or("No original Ecore")?.clone();
    let fields = (0..16).map(|n| format!(
        "<eStructuralFeatures xsi:type=\"ecore:EAttribute\" name=\"extra{n}\" eType=\"ecore:EDataType http://www.eclipse.org/emf/2002/Ecore#//EString\"/>"
    )).collect::<String>();
    let children = (0..8).map(|n| format!(
        "<eClassifiers xsi:type=\"ecore:EClass\" name=\"Inherited{n}\" eSuperTypes=\"#//Target\"/>"
    )).collect::<String>();
    let text = old
        .text
        .replace("name=\"Target\">", &format!("name=\"Target\">{fields}"))
        .replace("</ecore:EPackage>", &format!("{children}</ecore:EPackage>"));
    assert_ne!(text, old.text);
    let model = DefinitionSource::new(&old.id.uri, SourceKind::Ecore, text);
    bundle.sources[0] = model.clone();
    for source in &mut bundle.sources {
        for dependency in &mut source.dependencies {
            if dependency == &old.id {
                *dependency = model.id.clone();
            }
        }
    }
    let _retained_syntax = bundle
        .sources
        .iter()
        .find(|s| s.kind == SourceKind::Xtext)
        .ok_or("No syntax")?
        .id
        .clone();
    let original = ModelCompilation {
        materialized_types: prior.materialized_types,
        structure: model.id.clone(),
        behavior: bundle
            .sources
            .iter()
            .find(|s| s.kind == SourceKind::RcoreBehavior)
            .ok_or("No rules")?
            .id
            .clone(),
        namespace: prior.namespace,
        bundle,
    };
    let origin = crate::migration::migrate_ecore_bundle_with_limits(
        original.bundle.clone(),
        &[(model.id, "memory:compiled-proof/Gate.rcore".into())].into(),
        ParseLimits::default(),
    )
    .map_err(|e| e.to_string())?;
    Ok((origin.remap_model_compilation(&original)?, origin))
}

#[test]
fn frozen_owner_inventory_actual_inherited_hit_and_canonical_equivalence() -> StorageResult<()> {
    let (input, origin) = inherited_owner_fixture()?;
    let owner = owner(input)?;
    let definition = crate::definitions::model_test_support::prepare_owned(
        &owner,
        &origin,
        ParseLimits::default(),
    )
    .map_err(|e| format!("{e:?}"))?
    .without_completed_compiler_field_data_for_test();
    let metadata = definition
        .prepare_physical_metadata(&InvocationContext::default())
        .map_err(|e| e.to_string())?;
    let lookup = InvocationContext::new(Meter::new(100_000));
    let cache = metadata
        .frozen_feature_input_cache(definition.schema(), &lookup)?
        .ok_or("No actual completed cache")?;
    assert!(
        !crate::definitions::schema::FeatureInputCache::owner_inventory_complete_observed(
            &cache, "Target", &lookup
        )
        .map_err(|e| e.to_string())?
    );
    let provider = native(&definition)?;
    let cold_work = Meter::new(100_000);
    let cold = bind_draft_contracts(
        definition.schema(),
        definition.sources(),
        &provider,
        metadata.compiled_sources(),
        &mut DraftBindWork::Observed(&InvocationContext::new(cold_work.clone())),
        None,
    )?;
    provider.calls.set(0);
    let work = Meter::new(100_000);
    let current = DraftConstructionContext::bind_frozen(
        &metadata,
        &provider,
        &InvocationContext::new(work.clone()),
    )?;
    assert_eq!(provider.calls.get(), 2);
    assert!(
        crate::definitions::schema::FeatureInputCache::owner_inventory_complete_observed(
            &cache, "Target", &lookup
        )
        .map_err(|e| e.to_string())?,
        "Actual canonical parent inventory must have completed"
    );
    let policy = binding::FrozenSourceInputs::new(&metadata, definition.schema(), &cache, &lookup)?
        .ok_or("No source policy")?;
    use crate::definitions::schema::DefaultSourceInputs;
    assert!(
        policy
            .owner_inventory_complete(definition.schema(), &cache, "Target", &lookup)
            .map_err(|e| e.to_string())?
    );
    assert!(
        work.work() < cold_work.work(),
        "Real inherited source reuse must save work after all new guards"
    );
    assert_eq!(
        serde_json::to_value(&cold.contracts).map_err(|e| e.to_string())?,
        serde_json::to_value(&current.contracts).map_err(|e| e.to_string())?
    );
    let old = DraftConstructionContext {
        schema: definition.schema(),
        provider: None,
        contracts: cold.contracts,
        selected: cold.selected,
    };
    assert_eq!(
        export(&graph(&old, &metadata)?)?,
        export(&graph(&current, &metadata)?)?
    );
    println!(
        "FROZEN_OWNER_INVENTORY_RAW {}",
        serde_json::json!({"cold":cold_work.work(),"reuse":work.work(),"actual_Target_completed_hit":true,"inherited_receivers":8,"original24_fit":false})
    );
    Ok(())
}

#[test]
fn frozen_owner_inventory_foreign_short_cancel_and_malformed_order() -> StorageResult<()> {
    use crate::definitions::schema::{
        DefaultSourceInputs, FeatureInputCache, FeatureMapPreparation,
    };
    let (input, origin) = inherited_owner_fixture()?;
    let owner = owner(input)?;
    let definition = crate::definitions::model_test_support::prepare_owned(
        &owner,
        &origin,
        ParseLimits::default(),
    )
    .map_err(|e| format!("{e:?}"))?;
    let make_metadata = || {
        definition
            .prepare_physical_metadata(&InvocationContext::default())
            .map_err(|e| e.to_string())
    };
    let metadata = make_metadata()?;
    let observation = InvocationContext::new(Meter::new(100_000));
    let cache = metadata
        .frozen_feature_input_cache(definition.schema(), &observation)?
        .ok_or("No actual cache")?;
    let foreign_cache = Arc::new(std::sync::Mutex::new(FeatureInputCache::default()));
    assert!(
        binding::FrozenSourceInputs::new(
            &metadata,
            definition.schema(),
            &foreign_cache,
            &observation
        )
        .is_err()
    );
    let mut foreign = definition.schema().clone();
    let policy =
        binding::FrozenSourceInputs::new(&metadata, definition.schema(), &cache, &observation)?
            .ok_or("No policy")?;
    assert!(
        policy
            .owner_inventory_complete(&foreign, &cache, "Target", &observation)
            .is_err()
    );
    // Malformed declaration order cannot acquire this immutable metadata authority.
    foreign
        .classes
        .get_mut("Target")
        .ok_or("No Target")?
        .feature_order
        .pop();
    assert!(binding::FrozenSourceInputs::new(&metadata, &foreign, &cache, &observation).is_err());
    let mut canonical = FeatureMapPreparation::new_observed("Inherited0", &observation)
        .map_err(|e| e.to_string())?;
    let error = loop {
        match canonical.advance_observed(&foreign, &observation) {
            Err(error) => break error.to_string(),
            Ok(true) => return Err("Malformed source order passed canonical worker".into()),
            Ok(false) => {}
        }
    };
    assert!(error.contains("Invalid feature declaration inventory for Target"));
    let provider = native(&definition)?;
    let measured = Meter::new(100_000);
    DraftConstructionContext::bind_frozen(
        &metadata,
        &provider,
        &InvocationContext::new(measured.clone()),
    )?;
    let exact = measured.work();
    for grant in [exact - 1, exact] {
        let fresh = make_metadata()?;
        let meter = Meter::new(grant);
        let result = DraftConstructionContext::bind_frozen(
            &fresh,
            &provider,
            &InvocationContext::new(meter.clone()),
        );
        assert_eq!(result.is_ok(), grant == exact);
        assert_eq!(meter.work(), grant);
    }
    let denied = Meter::new(100_000);
    denied.deny_retention.set(true);
    assert!(
        DraftConstructionContext::bind_frozen(
            &metadata,
            &provider,
            &InvocationContext::new(denied)
        )
        .is_err()
    );
    let control = Rc::new(Control {
        meter: Meter::new(100_000),
        cancel: Cell::new(true),
    });
    assert!(
        DraftConstructionContext::bind_frozen(
            &metadata,
            &provider,
            &InvocationContext::new(control.clone())
        )
        .is_err()
    );
    assert_eq!(control.meter.work(), 0);
    Ok(())
}

// Authored mixed source profile, not a substitute for unchanged original24.
fn physical_projection_sources(
    inherited_derived_collision: bool,
) -> StorageResult<(ModelCompilation, BundleMigrationReport)> {
    let (mut original, migration) = inherited_owner_fixture()?;
    let mut bundle = migration.original.clone();
    let old = bundle.sources.first().ok_or("No original Ecore")?.clone();
    let derived = (0..24).map(|n| format!(
        "<eStructuralFeatures xsi:type=\"ecore:EAttribute\" name=\"derived{n}\" derived=\"true\" transient=\"true\" volatile=\"true\" defaultValueLiteral=\"unused\" eType=\"ecore:EDataType http://www.eclipse.org/emf/2002/Ecore#//EString\"/>"
    )).collect::<String>();
    assert_eq!(old.text.matches("name=\"Target\">").count(), 1);
    let mut text = old
        .text
        .replace("name=\"Target\">", &format!("name=\"Target\">{derived}"));
    if inherited_derived_collision {
        let child = "<eClassifiers xsi:type=\"ecore:EClass\" name=\"Inherited0\" eSuperTypes=\"#//Target\"/>";
        assert_eq!(text.matches(child).count(), 1);
        text = text.replace(child, "<eClassifiers xsi:type=\"ecore:EClass\" name=\"Inherited0\" eSuperTypes=\"#//Target\"><eStructuralFeatures xsi:type=\"ecore:EAttribute\" name=\"derived0\" derived=\"true\" eType=\"ecore:EDataType http://www.eclipse.org/emf/2002/Ecore#//EString\"/></eClassifiers>");
    }
    let model = DefinitionSource::new(&old.id.uri, SourceKind::Ecore, text);
    bundle.sources[0] = model.clone();
    for source in &mut bundle.sources {
        for dependency in &mut source.dependencies {
            if dependency == &old.id {
                *dependency = model.id.clone();
            }
        }
    }
    original.structure = model.id.clone();
    original.bundle = bundle;
    let origin = crate::migration::migrate_ecore_bundle_with_limits(
        original.bundle.clone(),
        &[(model.id, "memory:compiled-proof/Gate.rcore".into())].into(),
        ParseLimits::default(),
    )
    .map_err(|e| e.to_string())?;
    Ok((original, origin))
}
fn physical_projection_fixture(
    inherited_derived_collision: bool,
) -> StorageResult<(ModelCompilation, BundleMigrationReport)> {
    let (original, origin) = physical_projection_sources(inherited_derived_collision)?;
    Ok((origin.remap_model_compilation(&original)?, origin))
}

#[test]
fn frozen_physical_projection_mixed_inheritance_preserves_contracts_and_graph() -> StorageResult<()>
{
    let (input, origin) = physical_projection_fixture(false)?;
    let owner = owner(input)?;
    let definition = crate::definitions::model_test_support::prepare_owned(
        &owner,
        &origin,
        ParseLimits::default(),
    )
    .map_err(|e| format!("{e:?}"))?
    .without_completed_compiler_field_data_for_test();
    let metadata = definition
        .prepare_physical_metadata(&InvocationContext::default())
        .map_err(|e| e.to_string())?;
    let selected = definition.schema().effective_features("Inherited0")?;
    assert_eq!(selected.len(), 42);
    assert_eq!(selected.iter().filter(|f| f.feature.derived).count(), 24);
    let provider = native(&definition)?;
    let full_meter = Meter::new(100_000);
    let full = bind_draft_contracts(
        definition.schema(),
        definition.sources(),
        &provider,
        metadata.compiled_sources(),
        &mut DraftBindWork::Observed(&InvocationContext::new(full_meter.clone())),
        None,
    )?;
    provider.calls.set(0);
    let meter = Meter::new(100_000);
    let context = DraftConstructionContext::bind_frozen(
        &metadata,
        &provider,
        &InvocationContext::new(meter.clone()),
    )?;
    assert_eq!(provider.calls.get(), 2);
    assert_eq!(
        serde_json::to_value(&full.contracts).map_err(|e| e.to_string())?,
        serde_json::to_value(&context.contracts).map_err(|e| e.to_string())?
    );
    let full_context = DraftConstructionContext {
        schema: definition.schema(),
        provider: None,
        contracts: full.contracts,
        selected: full.selected,
    };
    let inherited_graph = |context: &DraftConstructionContext<'_>| -> StorageResult<FrozenStore> {
        let mut store =
            FrozenStore::for_schema(definition.schema()).map_err(|error| error.to_string())?;
        atomic(&mut store, |store| {
            bind_schema(store, definition.schema())?;
            context.create_pending(
                store,
                Construction {
                    id: "inherited",
                    resource: "memory:compiled-proof/mixed",
                    authored: &[],
                },
                "Inherited0",
                0,
                1,
                &metadata,
                &mut QueryBudget::new(100_000),
            )?;
            Ok(())
        })?;
        Ok(store)
    };
    let actual_graph = export(&inherited_graph(&context)?)?;
    assert_eq!(export(&inherited_graph(&full_context)?)?, actual_graph);
    assert_eq!(actual_graph.elements.len(), 1);
    let actual = actual_graph
        .elements
        .first()
        .ok_or("Missing actual inherited object")?;
    assert_eq!(actual.class, "Inherited0");
    assert_eq!(actual.values.len(), 18);
    assert_eq!(actual.values.get("tag"), Some(&serde_json::Value::Null));
    assert_eq!(actual.values.get("extra0"), Some(&serde_json::Value::Null));
    assert!(!actual.values.contains_key("derived0"));
    assert!(
        meter.work() < full_meter.work(),
        "Physical projection must save observed work after all new charges"
    );
    let observation = InvocationContext::new(Meter::new(100_000));
    let mut work = DraftBindWork::Observed(&observation);
    let mut cache = None;
    let physical = work.fields(
        definition.schema(),
        "Inherited0",
        &mut cache,
        Some(&metadata),
    )?;
    assert_eq!(physical.coverage(&work)?.len(), 18);
    let binding::BindFields::Observed(physical) = physical else {
        return Err("Fixture must exercise the physical projection fallback".into());
    };
    assert!(physical.get("derived0").is_none());
    assert!(!physical.get("tag").ok_or("Missing physical tag")?.derived);
    // The generic continuation still produces all fields and derived defaults.
    let mut generic =
        crate::definitions::schema::FeatureMapPreparation::new_observed("Inherited0", &observation)
            .map_err(|e| e.to_string())?;
    while !generic
        .advance_observed(definition.schema(), &observation)
        .map_err(|e| e.to_string())?
    {}
    let generic = generic.finish()?;
    assert_eq!(generic.fields().len(), 42);
    assert_eq!(
        generic
            .fields()
            .get("derived0")
            .ok_or("Missing generic derived field")?
            .default,
        serde_json::json!("unused")
    );
    println!(
        "FROZEN_PHYSICAL_PROJECTION_RAW {}",
        serde_json::json!({
            "full_work":full_meter.work(),"physical_work":meter.work(),
            "all_selected":42,"derived_selected_but_not_projected":24,"physical":18,
            "canonical_generic_unchanged":true,"original24_fit":false
        })
    );
    Ok(())
}

#[test]
fn frozen_physical_projection_exact_short_and_derived_provider_refusal() -> StorageResult<()> {
    let (input, origin) = physical_projection_fixture(false)?;
    let owner = owner(input)?;
    let definition = crate::definitions::model_test_support::prepare_owned(
        &owner,
        &origin,
        ParseLimits::default(),
    )
    .map_err(|e| format!("{e:?}"))?;
    let prepare = || {
        definition
            .prepare_physical_metadata(&InvocationContext::default())
            .map_err(|e| e.to_string())
    };
    let metadata = prepare()?;
    let provider = native(&definition)?;
    let measured = Meter::new(100_000);
    DraftConstructionContext::bind_frozen(
        &metadata,
        &provider,
        &InvocationContext::new(measured.clone()),
    )?;
    let exact = measured.work();
    assert!(exact > 0);
    for grant in [exact - 1, exact] {
        let fresh = prepare()?;
        let meter = Meter::new(grant);
        let result = DraftConstructionContext::bind_frozen(
            &fresh,
            &provider,
            &InvocationContext::new(meter.clone()),
        );
        assert_eq!(result.is_ok(), grant == exact);
        assert_eq!(meter.work(), grant);
    }
    let mut bad = native(&definition)?;
    let derived = definition
        .schema()
        .effective_feature("Inherited0", "derived0")?;
    bad.rows
        .iter_mut()
        .find(|row| row.class == "Inherited0")
        .ok_or("No inherited contract")?
        .members
        .push(DraftMemberDisposition {
            name: "derived0".into(),
            identity: derived.identity,
            disposition: DraftDisposition::StoredDefault,
        });
    let fresh = prepare()?;
    let result = DraftConstructionContext::bind_frozen(
        &fresh,
        &bad,
        &InvocationContext::new(Meter::new(100_000)),
    );
    let Err(error) = result else {
        return Err("Derived physical disposition was admitted".into());
    };
    assert!(error.contains("Unsupported unknown or derived physical member Inherited0.derived0"));
    Ok(())
}

#[test]
fn frozen_physical_projection_still_rejects_inherited_derived_name_collision() -> StorageResult<()>
{
    let (original, origin) = physical_projection_sources(true)?;
    assert!(origin.converted.is_none());
    assert_eq!(origin.reports.len(), 1);
    let report = origin
        .reports
        .first()
        .ok_or("Missing original Ecore report")?;
    assert_eq!(
        &report.original,
        original.bundle.source(&original.structure)?
    );
    assert!(report.converted.is_none());
    let resources = crate::definitions::resources::ResourceSet::new_with_limits(
        &original.bundle,
        ParseLimits::default(),
    )
    .map_err(|e| e.to_string())?;
    let parent = resources.resolve(&original.structure, "#//Target/derived0")?;
    let child = resources.resolve(&original.structure, "#//Inherited0/derived0")?;
    let classifier = resources.resolve(&original.structure, "#//Inherited0")?;
    let expected_span = &resources.node(&classifier)?.opening;
    let expected_message = format!(
        "Unsupported inherited feature redefinition: Inherited0.derived0; declarations {:?} and {:?}",
        crate::definitions::schema::MemberIdentity::Source {
            declaration: parent
        },
        crate::definitions::schema::MemberIdentity::Source { declaration: child },
    );
    let blocking = report
        .issues
        .iter()
        .filter(|issue| issue.blocks_conversion)
        .collect::<Vec<_>>();
    assert_eq!(blocking.len(), 1, "{:?}", report.issues);
    let issue = blocking.first().ok_or("Missing collision issue")?;
    assert_eq!(issue.code, "invalid_original");
    assert_eq!(issue.message, expected_message);
    assert_eq!(&issue.span, expected_span);
    let error = origin
        .remap_model_compilation(&original)
        .err()
        .ok_or("Inherited derived collision acquired remapped compilation authority")?;
    assert_eq!(error, "Migration has blocking issues");
    Ok(())
}

#[test]
fn frozen_borrowed_contracts_keep_actual_rows_and_owned_graph_equivalence() -> StorageResult<()> {
    let (input, origin) = physical_projection_fixture(false)?;
    let owner = owner(input)?;
    let definition = crate::definitions::model_test_support::prepare_owned(
        &owner,
        &origin,
        ParseLimits::default(),
    )
    .map_err(|e| format!("{e:?}"))?;
    let make_metadata = || {
        definition
            .prepare_physical_metadata(&InvocationContext::default())
            .map_err(|e| e.to_string())
    };
    let owned_metadata = make_metadata()?;
    let owned_work = Meter::new(100_000);
    // Existing owned API must still outlive a local native provider.
    let owned = {
        let local = native(&definition)?;
        let context = DraftConstructionContext::bind_frozen(
            &owned_metadata,
            &local,
            &InvocationContext::new(owned_work.clone()),
        )?;
        assert_eq!(local.calls.get(), 2);
        context
    };
    assert!(
        owned
            .contracts
            .values()
            .all(|row| matches!(row, std::borrow::Cow::Owned(_)))
    );
    let borrowed_metadata = make_metadata()?;
    let provider = native(&definition)?;
    let borrowed_work = Meter::new(100_000);
    let borrowed = DraftConstructionContext::bind_frozen_borrowed(
        &borrowed_metadata,
        &provider,
        &InvocationContext::new(borrowed_work.clone()),
    )?;
    assert_eq!(provider.calls.get(), 2);
    assert!(borrowed.is_frozen());
    for (class, captured) in &borrowed.contracts {
        let std::borrow::Cow::Borrowed(actual) = captured else {
            return Err("Borrowed frozen capture cloned a native row".into());
        };
        let original = provider
            .rows
            .iter()
            .find(|row| row.class.as_str() == class.as_str())
            .ok_or("Missing original native row")?;
        assert!(std::ptr::eq(*actual, original));
    }
    assert_eq!(
        serde_json::to_value(&owned.contracts).map_err(|e| e.to_string())?,
        serde_json::to_value(&borrowed.contracts).map_err(|e| e.to_string())?
    );
    let construct = |context: &DraftConstructionContext<'_>,
                     metadata: &PreparedPhysicalMetadata<'_>|
     -> StorageResult<FrozenStore> {
        let mut store =
            FrozenStore::for_schema(definition.schema()).map_err(|error| error.to_string())?;
        atomic(&mut store, |store| {
            bind_schema(store, definition.schema())?;
            let receipt = context.create_pending(
                store,
                Construction {
                    id: "inherited",
                    resource: "memory:borrowed-contracts",
                    authored: &[],
                },
                "Inherited0",
                0,
                1,
                metadata,
                &mut QueryBudget::new(100_000),
            )?;
            assert_eq!(receipt.contract.class, "Inherited0");
            assert_eq!(receipt.contract.members.len(), 18);
            Ok(())
        })?;
        Ok(store)
    };
    let expected = export(&construct(&owned, &owned_metadata)?)?;
    let actual = export(&construct(&borrowed, &borrowed_metadata)?)?;
    assert_eq!(expected, actual);
    assert_eq!(
        actual.elements.first().ok_or("No inherited object")?.class,
        "Inherited0"
    );
    assert_eq!(
        actual
            .elements
            .first()
            .ok_or("No inherited object")?
            .values
            .len(),
        18
    );
    assert_eq!(provider.calls.get(), 2);
    assert!(borrowed_work.work() < owned_work.work());
    assert!(borrowed_work.bytes.get() < owned_work.bytes.get());
    println!(
        "FROZEN_BORROWED_CONTRACTS_RAW {}",
        serde_json::json!({"owned_work":owned_work.work(),"borrowed_work":borrowed_work.work(),"owned_bytes":owned_work.bytes.get(),"borrowed_bytes":borrowed_work.bytes.get(),"rows":borrowed.contracts.len(),"same_actual_rows":true,"owned_api_provider_may_drop":true,"post_capture_callbacks":0,"original24_fit":false})
    );
    Ok(())
}

#[test]
fn frozen_borrowed_contracts_exact_short_cancel_drift_and_stale_refuse() -> StorageResult<()> {
    let (input, origin) = physical_projection_fixture(false)?;
    let owner = owner(input)?;
    let definition = crate::definitions::model_test_support::prepare_owned(
        &owner,
        &origin,
        ParseLimits::default(),
    )
    .map_err(|e| format!("{e:?}"))?;
    let prepare = || {
        definition
            .prepare_physical_metadata(&InvocationContext::default())
            .map_err(|e| e.to_string())
    };
    let provider = native(&definition)?;
    let metadata = prepare()?;
    let measured = Meter::new(100_000);
    DraftConstructionContext::bind_frozen_borrowed(
        &metadata,
        &provider,
        &InvocationContext::new(measured.clone()),
    )?;
    let exact = measured.work();
    assert!(exact > 0);
    for grant in [0, exact - 1, exact] {
        let fresh = prepare()?;
        let work = Meter::new(grant);
        let result = DraftConstructionContext::bind_frozen_borrowed(
            &fresh,
            &provider,
            &InvocationContext::new(work.clone()),
        );
        assert_eq!(result.is_ok(), grant == exact);
        assert_eq!(work.work(), grant);
    }
    let mut drifting = native(&definition)?;
    drifting.drift = true;
    let fresh = prepare()?;
    let result = DraftConstructionContext::bind_frozen_borrowed(
        &fresh,
        &drifting,
        &InvocationContext::new(Meter::new(100_000)),
    );
    let Err(error) = result else {
        return Err("Drifting borrowed provider published a context".into());
    };
    assert!(error.contains("Draft physical capability changed after binding"));
    assert_eq!(drifting.calls.get(), 2);
    let mut stale = native(&definition)?;
    stale
        .rows
        .first_mut()
        .ok_or("No native row")?
        .schema_revision
        .push_str("/stale");
    let fresh = prepare()?;
    let result = DraftConstructionContext::bind_frozen_borrowed(
        &fresh,
        &stale,
        &InvocationContext::new(Meter::new(100_000)),
    );
    let Err(error) = result else {
        return Err("Stale borrowed row published a context".into());
    };
    assert!(error.contains("Invalid or stale draft physical construction contract"));
    let control = Rc::new(Control {
        meter: Meter::new(100_000),
        cancel: Cell::new(false),
    });
    let mut cancel_at_final_provider = native(&definition)?;
    cancel_at_final_provider.cancel = Some(&control.cancel);
    let fresh = prepare()?;
    let result = DraftConstructionContext::bind_frozen_borrowed(
        &fresh,
        &cancel_at_final_provider,
        &InvocationContext::new(control.clone()),
    );
    let Err(error) = result else {
        return Err("Final-canceled borrowed bind published a context".into());
    };
    assert!(error.contains("cancelled"));
    assert_eq!(cancel_at_final_provider.calls.get(), 2);
    let denied = Meter::new(100_000);
    denied.deny_retention.set(true);
    let fresh = prepare()?;
    assert!(
        DraftConstructionContext::bind_frozen_borrowed(
            &fresh,
            &provider,
            &InvocationContext::new(denied)
        )
        .is_err()
    );
    Ok(())
}

// Actual ordinary compiler output, not a DTO or authored cache certificate.
#[test]
fn frozen_compiler_completed_all_source_maps_reuse_inherited_graph() -> StorageResult<()> {
    let (input, origin) = physical_projection_fixture(false)?;
    let owner = owner(input)?;
    let definition = crate::definitions::model_test_support::prepare_owned(
        &owner,
        &origin,
        ParseLimits::default(),
    )
    .map_err(|e| format!("{e:?}"))?;
    assert!(definition.schema().runtime_feature_domains.is_empty());
    let completed = definition
        .compiled_sources()
        .ok_or("No original compiled owner")?;
    let lookup = InvocationContext::new(Meter::new(100_000));
    // This map exists BEFORE physical metadata preparation. Previously the
    // opposite-only retention did not contain any Inherited receiver.
    let original = completed
        .physical_fields_observed(definition.schema(), "Inherited0", &lookup)?
        .ok_or("Ordinary compiler did not retain the actual source receiver")?;
    assert_eq!(original.receiver(), "Inherited0");
    assert!(original.get("derived0").is_some_and(|field| field.derived));
    let metadata = definition
        .prepare_physical_metadata(&InvocationContext::default())
        .map_err(|e| e.to_string())?;
    let actual = metadata
        .frozen_feature_inputs(definition.schema(), "Inherited0", &lookup)?
        .ok_or("No compiler-completed receiver at physical bind")?;
    assert!(Arc::ptr_eq(&original, &actual));
    for (class, header) in &definition.schema().classes {
        if header.feature_declarations.is_empty() && !header.features.is_empty() {
            continue;
        }
        let row = completed
            .physical_fields_observed(definition.schema(), class, &lookup)?
            .ok_or_else(|| format!("Missing completed original class {class}"))?;
        let expected = definition.schema().features(class)?;
        for (name, feature) in &expected {
            assert_eq!(row.get(name), Some(feature));
            assert!(matches!(
                row.member_identity(name),
                Some(MemberIdentity::Source { .. })
            ));
        }
        assert_eq!(row.field_iter().count(), expected.len());
    }
    let mut bad = native(&definition)?;
    let derived = definition
        .schema()
        .effective_feature("Inherited0", "derived0")?;
    bad.rows
        .iter_mut()
        .find(|row| row.class == "Inherited0")
        .ok_or("No inherited hot contract")?
        .members
        .push(DraftMemberDisposition {
            name: "derived0".into(),
            identity: derived.identity,
            disposition: DraftDisposition::StoredDefault,
        });
    let Err(error) = DraftConstructionContext::bind_frozen_borrowed(
        &metadata,
        &bad,
        &InvocationContext::new(Meter::new(100_000)),
    ) else {
        return Err("Hot complete data map granted derived physical authority".into());
    };
    assert!(error.contains("Unsupported unknown or derived physical member Inherited0.derived0"));
    let provider = native(&definition)?;
    let cold_work = Meter::new(100_000);
    let cold = bind_draft_contracts_with_capture::<BorrowedContractCapture>(
        definition.schema(),
        definition.sources(),
        &provider,
        Some(completed),
        &mut DraftBindWork::Observed(&InvocationContext::new(cold_work.clone())),
        None,
    )?;
    let cold = DraftConstructionContext {
        schema: definition.schema(),
        provider: None,
        contracts: cold.contracts,
        selected: cold.selected,
    };
    let warm_work = Meter::new(100_000);
    let warm = DraftConstructionContext::bind_frozen_borrowed(
        &metadata,
        &provider,
        &InvocationContext::new(warm_work.clone()),
    )?;
    let construct = |context: &DraftConstructionContext<'_>| -> StorageResult<FrozenStore> {
        let mut store =
            FrozenStore::for_schema(definition.schema()).map_err(|error| error.to_string())?;
        atomic(&mut store, |store| {
            bind_schema(store, definition.schema())?;
            let receipt = context.create_pending(
                store,
                Construction {
                    id: "inherited",
                    resource: "memory:compiler-completed",
                    authored: &[],
                },
                "Inherited0",
                0,
                1,
                &metadata,
                &mut QueryBudget::new(100_000),
            )?;
            assert_eq!(receipt.contract.class, "Inherited0");
            assert_eq!(receipt.contract.members.len(), 18);
            Ok(())
        })?;
        Ok(store)
    };
    let expected = export(&construct(&cold)?)?;
    let actual = export(&construct(&warm)?)?;
    assert_eq!(actual, expected);
    assert_eq!(
        actual
            .elements
            .first()
            .ok_or("No inherited graph object")?
            .values
            .len(),
        18
    );
    assert!(warm_work.work() < cold_work.work());
    println!(
        "COMPILER_COMPLETED_FIELDS_RAW {}",
        serde_json::json!({
            "cold_bind_work":cold_work.work(),"completed_bind_work":warm_work.work(),
            "completed_bind_bytes":warm_work.bytes.get(),"actual_inherited_map_ptr_equal":true,
            "ordinary_source_classes":definition.schema().classes.len(),
            "runtime_domain_receivers":0,"actual_inherited_graph_fields":18,
            "all_source_fields_preserved":true,"original24_fit":false,"full_K01_verified":false
        })
    );
    Ok(())
}

#[test]
fn frozen_compiler_completed_inputs_source_storage_cancel_and_exact_short() -> StorageResult<()> {
    let (input, origin) = physical_projection_fixture(false)?;
    let owner = owner(input)?;
    let definition = crate::definitions::model_test_support::prepare_owned(
        &owner,
        &origin,
        ParseLimits::default(),
    )
    .map_err(|e| format!("{e:?}"))?;
    let completed = definition
        .compiled_sources()
        .ok_or("No compiled input owner")?;
    let foreign = definition.schema().clone();
    let Err(error) = completed.physical_fields_observed(
        &foreign,
        "Inherited0",
        &InvocationContext::new(Meter::new(100_000)),
    ) else {
        return Err("Foreign schema accepted compiler input data".into());
    };
    assert!(error.contains("another actual schema"));
    let measured = Meter::new(100_000);
    let row = completed
        .physical_fields_observed(
            definition.schema(),
            "Inherited0",
            &InvocationContext::new(measured.clone()),
        )?
        .ok_or("No actual completed map")?;
    let exact = measured.work();
    for grant in [0, exact - 1, exact] {
        let meter = Meter::new(grant);
        let result = completed.physical_fields_observed(
            definition.schema(),
            "Inherited0",
            &InvocationContext::new(meter.clone()),
        );
        assert_eq!(result.is_ok(), grant == exact);
        assert_eq!(meter.work(), grant);
        if let Ok(Some(actual)) = result {
            assert!(Arc::ptr_eq(&row, &actual));
        }
    }
    let denied = Meter::new(100_000);
    denied.deny_retention.set(true);
    assert!(
        completed
            .physical_fields_observed(
                definition.schema(),
                "Inherited0",
                &InvocationContext::new(denied),
            )
            .is_err()
    );
    let control = Rc::new(Control {
        meter: Meter::new(100_000),
        cancel: Cell::new(true),
    });
    assert!(
        completed
            .physical_fields_observed(
                definition.schema(),
                "Inherited0",
                &InvocationContext::new(control.clone()),
            )
            .is_err()
    );
    control.cancel.set(false);
    let recovered = completed
        .physical_fields_observed(
            definition.schema(),
            "Inherited0",
            &InvocationContext::new(control),
        )?
        .ok_or("Canceled data lookup destroyed immutable owner")?;
    assert!(Arc::ptr_eq(&row, &recovered));
    // Missing receiver remains a miss, not a fabricated empty complete map.
    assert!(
        completed
            .physical_fields_observed(
                definition.schema(),
                "Absent",
                &InvocationContext::new(Meter::new(100_000)),
            )?
            .is_none()
    );
    Ok(())
}
#[test]
fn frozen_coverage_cursor_owned_shared_rows_and_paid_work_are_equivalent() -> StorageResult<()> {
    use crate::definitions::schema::{FeatureMapPreparation, PreparedFeatureInputs};
    let (input, origin) = physical_projection_fixture(false)?;
    let owner = owner(input)?;
    let definition = crate::definitions::model_test_support::prepare_owned(
        &owner,
        &origin,
        ParseLimits::default(),
    )
    .map_err(|e| format!("{e:?}"))?;
    let fields = definition
        .compiled_sources()
        .ok_or("No actual completed compiler owner")?
        .physical_fields_observed(
            definition.schema(),
            "Inherited0",
            &InvocationContext::new(Meter::new(100_000)),
        )?
        .ok_or("No actual complete inherited map")?;
    let mut cold = FeatureMapPreparation::new("Inherited0");
    while !cold.advance(definition.schema())? {}
    let owned = PreparedFeatureInputs::Owned(cold.finish()?);
    let expected = definition.schema().features("Inherited0")?;
    assert_eq!(expected.len(), 42);
    assert_eq!(expected.values().filter(|f| f.derived).count(), 24);
    let expected_rows = expected
        .iter()
        .map(|(name, field)| (name.clone(), field.derived, field.default.clone()))
        .collect::<Vec<_>>();
    for view in [&owned, fields.as_ref()] {
        let meter = Meter::new(100_000);
        let mut rows = Vec::new(); // Host oracle only; never schema/provider authority.
        view.visit_fields_observed(&InvocationContext::new(meter.clone()), |name, field| {
            rows.push((name.to_owned(), field.derived, field.default.clone()));
            Ok(())
        })?;
        assert_eq!(rows, expected_rows);
        assert_eq!(meter.work(), 184);
        assert_eq!(meter.bytes.get(), 128);
        // Actual predecessor algorithm's paid ordered probes; no cloned index.
        let predecessor = Meter::new(100_000);
        let observation = InvocationContext::new(predecessor.clone());
        let mut after = None;
        let mut visits = 0;
        loop {
            view.admit_lookup_observed(after.unwrap_or(""), 1, &observation)?;
            let Some((name, _)) = view.next_field(after) else {
                break;
            };
            visits += 1;
            after = Some(name);
        }
        assert_eq!(visits, expected.len());
        assert!(meter.work() < predecessor.work());
        println!(
            "FROZEN_COVERAGE_CURSOR_TRAVERSAL_RAW {}",
            serde_json::json!({"actual_rows":visits,"derived_rows":24,"nonderived_rows":18,"predecessor_ordered_probe_work":predecessor.work(),"cursor_traversal_work":meter.work(),"cursor_logical_bytes":meter.bytes.get(),"all_rows_preserved":true,"original24_fit":false,"full_K01_verified":false})
        );
    }
    Ok(())
}
#[test]
fn frozen_coverage_cursor_preserves_complete_nonderived_sets_exact_short_and_derived_refusal()
-> StorageResult<()> {
    use crate::definitions::schema::{FeatureMapPreparation, PreparedFeatureInputs};
    let (input, origin) = physical_projection_fixture(false)?;
    let owner = owner(input)?;
    let definition = crate::definitions::model_test_support::prepare_owned(
        &owner,
        &origin,
        ParseLimits::default(),
    )
    .map_err(|e| format!("{e:?}"))?;
    let fields = definition
        .compiled_sources()
        .ok_or("No compiler owner")?
        .physical_fields_observed(
            definition.schema(),
            "Inherited0",
            &InvocationContext::new(Meter::new(100_000)),
        )?
        .ok_or("No complete inherited fields")?;
    let metadata = definition
        .prepare_physical_metadata(&InvocationContext::default())
        .map_err(|e| e.to_string())?;
    let provider = native(&definition)?;
    let contract = provider
        .rows
        .iter()
        .find(|r| r.class == "Inherited0")
        .ok_or("No actual inherited native contract")?;
    let expected = contract
        .members
        .iter()
        .map(|m| m.name.clone())
        .collect::<BTreeSet<_>>();
    assert_eq!(expected.len(), 18);
    assert!(!expected.contains("derived0"));
    let mut cold = FeatureMapPreparation::new("Inherited0");
    while !cold.advance(definition.schema())? {}
    let owned = binding::BindFields::Observed(PreparedFeatureInputs::Owned(cold.finish()?));
    let shared = binding::BindFields::Retained(fields);
    for view in [&owned, &shared] {
        let measured = Meter::new(100_000);
        let context = InvocationContext::new(measured.clone());
        assert_eq!(view.coverage(&DraftBindWork::Observed(&context))?, expected);
        let exact = measured.work();
        assert!(exact > 0);
        // Complete predecessor coverage oracle, with its original probes,
        // per-row visit, SAME name clone/storage and ordered-set insert.
        let previous = Meter::new(100_000);
        let previous_context = InvocationContext::new(previous.clone());
        let previous_work = DraftBindWork::Observed(&previous_context);
        let previous_fields = match view {
            binding::BindFields::Observed(fields) => fields,
            binding::BindFields::Retained(fields) => fields.as_ref(),
            binding::BindFields::Legacy(_) => {
                return Err("Expected observed coverage fixture".into());
            }
        };
        let mut previous_set = BTreeSet::new();
        let mut after = None;
        loop {
            previous_fields.admit_lookup_observed(after.unwrap_or(""), 1, &previous_context)?;
            let Some((name, field)) = previous_fields.next_field(after) else {
                break;
            };
            previous_context.work(1)?;
            if !field.derived {
                previous_work.key(name)?;
                previous_set.insert(name.to_owned());
            }
            after = Some(name);
        }
        assert_eq!(previous_set, expected);
        assert!(
            exact < previous.work(),
            "Mixed-source coverage including insertion fees must improve"
        );
        println!(
            "FROZEN_COVERAGE_COMPLETE_WORK_RAW {}",
            serde_json::json!({"derived_rows":24,"nonderived_rows":18,"predecessor_coverage_work":previous.work(),"cursor_plus_explicit_insert_work":exact,"same_complete_coverage":true,"original24_fit":false,"full_K01_verified":false})
        );
        for grant in [exact - 1, exact] {
            let meter = Meter::new(grant);
            let context = InvocationContext::new(meter.clone());
            let result = view.coverage(&DraftBindWork::Observed(&context));
            assert_eq!(result.is_ok(), grant == exact);
            assert_eq!(meter.work(), grant);
            if let Ok(actual) = result {
                assert_eq!(actual, expected);
            }
        }
    }
    // Source data still includes derived rows; the actual physical member guard
    // must refuse them even though coverage visits and filters those rows.
    let derived = definition
        .schema()
        .effective_feature("Inherited0", "derived0")?;
    let invalid = DraftMemberDisposition {
        name: "derived0".into(),
        identity: derived.identity,
        disposition: DraftDisposition::StoredDefault,
    };
    let context = InvocationContext::new(Meter::new(100_000));
    let error = shared
        .member(
            definition.schema(),
            "Inherited0",
            &invalid,
            &DraftBindWork::Observed(&context),
        )
        .err()
        .ok_or("Cursor route admitted a derived physical disposition")?;
    assert_eq!(
        error,
        "Unsupported unknown or derived physical member Inherited0.derived0"
    );
    let actual = metadata
        .frozen_feature_inputs(definition.schema(), "Inherited0", &context)?
        .ok_or("No same-owner metadata map")?;
    assert!(actual.get("derived0").is_some_and(|f| f.derived));
    Ok(())
}
#[test]
fn frozen_coverage_cursor_denied_storage_mid_cancel_and_consumer_error_publish_no_coverage()
-> StorageResult<()> {
    let (input, origin) = physical_projection_fixture(false)?;
    let owner = owner(input)?;
    let definition = crate::definitions::model_test_support::prepare_owned(
        &owner,
        &origin,
        ParseLimits::default(),
    )
    .map_err(|e| format!("{e:?}"))?;
    let fields = definition
        .compiled_sources()
        .ok_or("No compiler owner")?
        .physical_fields_observed(
            definition.schema(),
            "Inherited0",
            &InvocationContext::new(Meter::new(100_000)),
        )?
        .ok_or("No complete inherited fields")?;
    let denied = Meter::new(100_000);
    denied.deny_retention.set(true);
    let visited = Cell::new(0);
    assert_eq!(
        fields
            .visit_fields_observed(&InvocationContext::new(denied.clone()), |_, _| {
                visited.set(visited.get() + 1);
                Ok(())
            })
            .err()
            .as_deref(),
        Some("Source retention denied")
    );
    assert_eq!(visited.get(), 0);
    assert_eq!(denied.bytes.get(), 0);
    assert_eq!(denied.work(), 184);
    let short = Meter::new(183);
    let visited = Cell::new(0);
    assert!(
        fields
            .visit_fields_observed(&InvocationContext::new(short.clone()), |_, _| {
                visited.set(visited.get() + 1);
                Ok(())
            })
            .is_err()
    );
    assert_eq!(visited.get(), 0);
    assert_eq!(short.bytes.get(), 0);
    assert_eq!(short.work(), 183);
    let control = Rc::new(Control {
        meter: Meter::new(100_000),
        cancel: Cell::new(false),
    });
    let visited = Cell::new(0);
    let result = fields.visit_fields_observed(&InvocationContext::new(control.clone()), |_, _| {
        visited.set(visited.get() + 1);
        if visited.get() == 3 {
            control.cancel.set(true);
        }
        Ok(())
    });
    assert_eq!(
        result.err().as_deref(),
        Some("Compiled source binding cancelled")
    );
    assert_eq!(visited.get(), 3);
    assert_eq!(control.meter.work(), 184);
    assert_eq!(control.meter.bytes.get(), 128);
    let final_control = Rc::new(Control {
        meter: Meter::new(100_000),
        cancel: Cell::new(false),
    });
    let final_visits = Cell::new(0);
    let final_result =
        fields.visit_fields_observed(&InvocationContext::new(final_control.clone()), |_, _| {
            final_visits.set(final_visits.get() + 1);
            if final_visits.get() == 42 {
                final_control.cancel.set(true);
            }
            Ok(())
        });
    assert_eq!(
        final_result.err().as_deref(),
        Some("Compiled source binding cancelled")
    );
    assert_eq!(final_visits.get(), 42);
    assert_eq!(final_control.meter.work(), 184);
    assert_eq!(final_control.meter.bytes.get(), 128);
    let meter = Meter::new(100_000);
    let visited = Cell::new(0);
    assert_eq!(
        fields
            .visit_fields_observed(&InvocationContext::new(meter.clone()), |_, _| {
                visited.set(visited.get() + 1);
                Err("actual coverage consumer refusal".into())
            })
            .err()
            .as_deref(),
        Some("actual coverage consumer refusal")
    );
    assert_eq!(visited.get(), 1);
    assert_eq!(meter.work(), 184);
    assert_eq!(meter.bytes.get(), 128);
    Ok(())
}

struct StickyBindContextMeter {
    inner: Rc<Meter>,
    failed: Cell<bool>,
    post_failure_checkpoints: Cell<usize>,
    budget_primary: bool,
}
impl InvocationObserver for StickyBindContextMeter {
    fn before_invoke(&self, _: &ProviderId) -> StorageResult<()> {
        Err("No budget-context semantic provider".into())
    }
    fn checkpoint(&self) -> StorageResult<()> {
        if self.failed.get() {
            self.post_failure_checkpoints
                .set(self.post_failure_checkpoints.get() + 1);
            Err("Budget-context secondary final checkpoint refusal".into())
        } else {
            Ok(())
        }
    }
    fn work(&self, units: usize) -> StorageResult<()> {
        self.checkpoint()?;
        let result = InvocationObserver::work(&*self.inner, units);
        if let Err(error) = result {
            self.failed.set(true);
            return Err(if self.budget_primary {
                error
            } else {
                "Budget-context primary nonbudget observer refusal".into()
            });
        }
        Ok(())
    }
    fn retain_storage(&self, bytes: usize) -> StorageResult<()> {
        self.checkpoint()?;
        self.inner.retain_storage(bytes)
    }
}
#[test]
fn frozen_bind_budget_context_actual_retained_receiver_survives_sticky_final_checkpoint()
-> StorageResult<()> {
    let (input, origin) = physical_projection_fixture(false)?;
    let owner = owner(input)?;
    let definition = crate::definitions::model_test_support::prepare_owned(
        &owner,
        &origin,
        ParseLimits::default(),
    )
    .map_err(|e| format!("{e:?}"))?;
    let metadata = definition
        .prepare_physical_metadata(&InvocationContext::default())
        .map_err(|e| e.to_string())?;
    let mut provider = native(&definition)?;
    provider.rows.retain(|row| row.class == "Inherited0");
    assert_eq!(provider.rows.len(), 1);
    assert_eq!(provider.rows[0].members.len(), 18);
    let measured = Meter::new(100_000);
    DraftConstructionContext::bind_frozen_borrowed(
        &metadata,
        &provider,
        &InvocationContext::new(measured.clone()),
    )?;
    let exact = measured.work();
    assert!(exact > 2);
    let sufficient = Rc::new(StickyBindContextMeter {
        inner: Meter::new(exact),
        failed: Cell::new(false),
        post_failure_checkpoints: Cell::new(0),
        budget_primary: true,
    });
    DraftConstructionContext::bind_frozen_borrowed(
        &metadata,
        &provider,
        &InvocationContext::new(sufficient.clone()),
    )?;
    assert_eq!(sufficient.inner.work(), exact);
    assert_eq!(sufficient.post_failure_checkpoints.get(), 0);
    let denied = Rc::new(StickyBindContextMeter {
        inner: Meter::new(exact / 2),
        failed: Cell::new(false),
        post_failure_checkpoints: Cell::new(0),
        budget_primary: true,
    });
    let error = DraftConstructionContext::bind_frozen_borrowed(
        &metadata,
        &provider,
        &InvocationContext::new(denied.clone()),
    )
    .err()
    .ok_or("Short actual retained bind published a context")?;
    println!(
        "FROZEN_BIND_CAUSAL_CONTEXT_RAW {}",
        serde_json::json!({"error":error,"actual_short_work":denied.inner.work(),"successful_exact_work":exact,"final_failed_checkpoint_observed":denied.post_failure_checkpoints.get(),"original24_fit":false,"full_K01_verified":false})
    );
    assert!(
        error.starts_with("Query work budget exhausted; no complete result [draft-bind "),
        "{error}"
    );
    assert!(error.contains("receiver=Inherited0"), "{error}");
    assert!(error.contains("inputs=Retained"), "{error}");
    assert!(
        error.contains("stage=member_")
            || error.contains("stage=stored_selection_key")
            || error.contains("stage=coverage"),
        "{error}"
    );
    assert!(
        denied.post_failure_checkpoints.get() > 0,
        "Existing final checkpoint must still run"
    );
    assert_eq!(denied.inner.work(), exact / 2);
    // Same cofailure, noncanonical primary: keep the EXISTING postguard priority.
    let nonbudget = Rc::new(StickyBindContextMeter {
        inner: Meter::new(exact / 2),
        failed: Cell::new(false),
        post_failure_checkpoints: Cell::new(0),
        budget_primary: false,
    });
    let error = DraftConstructionContext::bind_frozen_borrowed(
        &metadata,
        &provider,
        &InvocationContext::new(nonbudget.clone()),
    )
    .err()
    .ok_or("Nonbudget observer refusal published a context")?;
    assert_eq!(error, "Budget-context secondary final checkpoint refusal");
    assert!(nonbudget.post_failure_checkpoints.get() > 0);
    let mut invalid = native(&definition)?;
    invalid.rows.retain(|row| row.class == "Inherited0");
    invalid.rows[0].schema_revision.push_str("/foreign");
    let error = DraftConstructionContext::bind_frozen_borrowed(
        &metadata,
        &invalid,
        &InvocationContext::new(Meter::new(100_000)),
    )
    .err()
    .ok_or("Invalid actual contract published a context")?;
    assert_eq!(
        error,
        "Invalid or stale draft physical construction contract"
    );
    Ok(())
}
#[test]
fn frozen_bind_budget_context_has_bounded_utf8_labels_and_preserves_other_refusal_strings()
-> StorageResult<()> {
    let primary = QueryBudget::new(0)
        .charge()
        .err()
        .ok_or("Expected canonical QueryBudget refusal")?;
    let receiver = format!("{}é-tail", "a".repeat(63));
    let member = "🛰️".repeat(100);
    let error = binding::budget_failure_context(
        primary.clone(),
        &receiver,
        &member,
        &"s".repeat(400),
        &"k".repeat(400),
    );
    assert!(error.starts_with(&primary));
    assert!(binding::is_budget_failure(&error));
    assert!(error.len() <= primary.len() + 384);
    assert!(error.contains(&format!("receiver={} member=", "a".repeat(63))));
    assert!(!error.contains("é-tail"));
    assert!(!error.contains("s".repeat(65).as_str()));
    assert!(!error.contains("k".repeat(65).as_str()));
    assert!(error.ends_with(']'));
    for refusal in [
        "Frozen owner inventory schema/cache association changed",
        "Unsupported unknown or derived physical member Inherited0.derived0",
        "Compiled source binding cancelled",
        "Query work budget exhausted; a distinct noncanonical refusal",
    ] {
        assert!(!binding::is_budget_failure(refusal));
        assert_eq!(
            binding::budget_failure_context(
                refusal.into(),
                "receiver",
                "member",
                "stage",
                "Retained"
            ),
            refusal
        );
    }
    Ok(())
}

#[test]
fn frozen_member_row_shared_and_owned_preserve_all_native_members_and_actual_probe_savings()
-> StorageResult<()> {
    use crate::definitions::schema::{FeatureMapPreparation, PreparedFeatureInputs};
    let (input, origin) = physical_projection_fixture(false)?;
    let owner = owner(input)?;
    let definition = crate::definitions::model_test_support::prepare_owned(
        &owner,
        &origin,
        ParseLimits::default(),
    )
    .map_err(|error| format!("{error:?}"))?;
    let provider = native(&definition)?;
    let completed = definition
        .compiled_sources()
        .ok_or("Missing completed owner")?;
    let shared_meter = Meter::new(1_000_000);
    let owned_meter = Meter::new(1_000_000);
    let predecessor_meter = Meter::new(1_000_000);
    let shared_observation = InvocationContext::new(shared_meter.clone());
    let owned_observation = InvocationContext::new(owned_meter.clone());
    let predecessor_observation = InvocationContext::new(predecessor_meter.clone());
    let mut checked = 0usize;
    for contract in &provider.rows {
        let fields = completed
            .physical_fields_observed(
                definition.schema(),
                &contract.class,
                &InvocationContext::default(),
            )?
            .ok_or("Missing complete actual native class map")?;
        assert!(matches!(fields.as_ref(), PreparedFeatureInputs::Shared(_)));
        let mut cold = FeatureMapPreparation::new(&contract.class);
        while !cold.advance(definition.schema())? {}
        let owned = binding::BindFields::Observed(PreparedFeatureInputs::Owned(cold.finish()?));
        let shared = binding::BindFields::Retained(fields.clone());
        for member in &contract.members {
            // Exact predecessor algorithm: two paid probes, then field and identity.
            fields.admit_lookup_observed(&member.name, 2, &predecessor_observation)?;
            let expected = fields
                .get(&member.name)
                .ok_or("Missing predecessor field")?;
            let identity = fields.member_identity(&member.name);
            assert_eq!(identity, Some(&member.identity));
            let actual = shared.member(
                definition.schema(),
                &contract.class,
                member,
                &DraftBindWork::Observed(&shared_observation),
            )?;
            let fallback = owned.member(
                definition.schema(),
                &contract.class,
                member,
                &DraftBindWork::Observed(&owned_observation),
            )?;
            assert!(std::ptr::eq(expected, actual.0));
            assert!(actual.1 && fallback.1);
            assert_eq!(actual.0, fallback.0);
            assert!(!actual.0.derived);
            checked += 1;
        }
    }
    assert!(
        checked >= 8 * 18,
        "Use every actual inherited native contract, not one cheap key"
    );
    assert_eq!(shared_meter.work() * 2, predecessor_meter.work());
    assert_eq!(owned_meter.work(), predecessor_meter.work());
    assert_eq!(shared_meter.bytes.get(), 0);
    assert_eq!(owned_meter.bytes.get(), 0);
    println!(
        "FROZEN_MEMBER_ROW_LOOKUP_RAW {}",
        serde_json::json!({
            "actual_native_members":checked,"predecessor_two_probe_work":predecessor_meter.work(),
            "shared_single_row_work":shared_meter.work(),"owned_two_map_work":owned_meter.work(),
            "same_fields_and_identities":true,"original24_fit":false,"full_K01_verified":false
        })
    );
    Ok(())
}

#[test]
fn frozen_member_row_exact_short_cancel_missing_member_identity_mismatch_and_derived_refusal()
-> StorageResult<()> {
    use crate::definitions::schema::{FeatureMapPreparation, PreparedFeatureInputs};
    let (input, origin) = physical_projection_fixture(false)?;
    let owner = owner(input)?;
    let definition = crate::definitions::model_test_support::prepare_owned(
        &owner,
        &origin,
        ParseLimits::default(),
    )
    .map_err(|error| format!("{error:?}"))?;
    let fields = definition
        .compiled_sources()
        .ok_or("No actual compiled owner")?
        .physical_fields_observed(
            definition.schema(),
            "Inherited0",
            &InvocationContext::default(),
        )?
        .ok_or("Missing actual inherited fields")?;
    assert!(matches!(fields.as_ref(), PreparedFeatureInputs::Shared(_)));
    let provider = native(&definition)?;
    let contract = provider
        .rows
        .iter()
        .find(|row| row.class == "Inherited0")
        .ok_or("Missing actual inherited contract")?;
    let member = contract.members.first().ok_or("Missing physical member")?;
    let mut cold = FeatureMapPreparation::new("Inherited0");
    while !cold.advance(definition.schema())? {}
    let owned = binding::BindFields::Observed(PreparedFeatureInputs::Owned(cold.finish()?));
    let shared = binding::BindFields::Retained(fields);
    for view in [&owned, &shared] {
        let measured = Meter::new(100_000);
        let observation = InvocationContext::new(measured.clone());
        assert!(
            view.member(
                definition.schema(),
                "Inherited0",
                member,
                &DraftBindWork::Observed(&observation)
            )?
            .1
        );
        let exact = measured.work();
        assert!(exact > 0);
        for grant in [0, exact - 1, exact] {
            let meter = Meter::new(grant);
            let observation = InvocationContext::new(meter.clone());
            let result = view.member(
                definition.schema(),
                "Inherited0",
                member,
                &DraftBindWork::Observed(&observation),
            );
            assert_eq!(result.is_ok(), grant == exact);
            assert_eq!(meter.work(), grant);
            if let Ok((field, matches)) = result {
                assert!(matches && !field.derived);
            }
        }
        let denied = Meter::new(100_000);
        denied.deny_retention.set(true);
        let observation = InvocationContext::new(denied.clone());
        assert!(
            view.member(
                definition.schema(),
                "Inherited0",
                member,
                &DraftBindWork::Observed(&observation)
            )?
            .1
        );
        assert_eq!(
            denied.bytes.get(),
            0,
            "Borrowed row does not allocate or retain payload"
        );
        let control = Rc::new(Control {
            meter: Meter::new(100_000),
            cancel: Cell::new(true),
        });
        let observation = InvocationContext::new(control.clone());
        let error = view
            .member(
                definition.schema(),
                "Inherited0",
                member,
                &DraftBindWork::Observed(&observation),
            )
            .err()
            .ok_or("Cancelled row lookup succeeded")?;
        assert_eq!(error, "Compiled source binding cancelled");
        assert_eq!(control.meter.work(), 0);
        control.cancel.set(false);
        assert!(
            view.member(
                definition.schema(),
                "Inherited0",
                member,
                &DraftBindWork::Observed(&observation)
            )?
            .1
        );
        let wrong = DraftMemberDisposition {
            name: member.name.clone(),
            identity: definition
                .schema()
                .effective_feature("Inherited0", "derived0")?
                .identity,
            disposition: member.disposition,
        };
        assert!(
            !view
                .member(
                    definition.schema(),
                    "Inherited0",
                    &wrong,
                    &DraftBindWork::Observed(&InvocationContext::default())
                )?
                .1
        );
        let missing = DraftMemberDisposition {
            name: "missing".into(),
            identity: member.identity.clone(),
            disposition: member.disposition,
        };
        assert_eq!(
            view.member(
                definition.schema(),
                "Inherited0",
                &missing,
                &DraftBindWork::Observed(&InvocationContext::default())
            )
            .err()
            .ok_or("Missing member was admitted")?,
            "Unsupported unknown or derived physical member Inherited0.missing"
        );
    }
    let derived = DraftMemberDisposition {
        name: "derived0".into(),
        identity: definition
            .schema()
            .effective_feature("Inherited0", "derived0")?
            .identity,
        disposition: DraftDisposition::StoredDefault,
    };
    assert_eq!(
        shared
            .member(
                definition.schema(),
                "Inherited0",
                &derived,
                &DraftBindWork::Observed(&InvocationContext::default())
            )
            .err()
            .ok_or("Retained derived disposition admitted")?,
        "Unsupported unknown or derived physical member Inherited0.derived0"
    );
    Ok(())
}
// The per-contract memo of `member_among` selects exactly what the per-member
// `Schema::effective_feature` lookup selects, including its refusals.
#[test]
fn legacy_member_memo_matches_the_per_member_lookup() -> StorageResult<()> {
    const SOURCE: &str = r#"rcore 1;
package memo version "1";
metamodel "urn:intersect:member-memo" prefix m {
  abstract class Named { attr name: String [1,1] default ""; }
  class Node extends Named {
    attr tags: String [0,*];
    ref children: Named [0,*] containment;
    ref first: Named [0,1] derived;
  }
  class Leaf extends Named { attr note: String [0,1]; }
}"#;
    let package = crate::compile_rcore_at("memory:/member-memo.rcore", SOURCE)
        .map_err(|error| error.to_string())?;
    let schema = package.schema();
    let mut budget = QueryBudget::new(100_000);
    let work = DraftBindWork::Legacy(&mut budget);
    let mut compared = 0;
    for class in schema.classes.keys() {
        let fields = binding::BindFields::Legacy(schema.features(class)?);
        let mut requested: Vec<_> = schema
            .effective_features(class)?
            .into_iter()
            .map(|member| (member.feature.name.clone(), member.identity))
            .collect();
        let other = requested
            .first()
            .map(|(_, identity)| identity.clone())
            .ok_or("Class without members")?;
        // An unknown name, and a known name under another member's identity.
        requested.push(("missing".into(), other.clone()));
        if let Some((name, _)) = requested.get(1).cloned() {
            requested.push((name, other));
        }
        let mut effective = None;
        for (name, identity) in requested {
            let expected = schema
                .effective_feature(class, &name)
                .map(|selected| (selected.feature.clone(), selected.identity == identity));
            let member = DraftMemberDisposition {
                name,
                identity,
                disposition: DraftDisposition::StoredDefault,
            };
            let memoized = fields
                .member_among(schema, class, &member, &work, &mut effective)
                .map(|(feature, same)| (feature.clone(), same));
            let single = fields
                .member(schema, class, &member, &work)
                .map(|(feature, same)| (feature.clone(), same));
            assert_eq!(memoized, expected, "{class}.{}", member.name);
            assert_eq!(single, expected, "{class}.{}", member.name);
            compared += 1;
        }
        assert!(effective.is_some());
    }
    assert!(compared >= 9);
    Ok(())
}
