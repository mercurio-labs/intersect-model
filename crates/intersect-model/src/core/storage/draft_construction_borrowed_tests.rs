//! Actual native source/contract capture controls; no original SysML fit claim.
use super::*;
use crate::core::frozen::FrozenStore;
use crate::core::storage::pending_source::tests::{Meter, definition, inputs};
use crate::definitions::{
    conversion::{InvocationObserver, ProviderId},
    preparation::PreparedModelDefinition,
};
use std::{cell::Cell, rc::Rc};

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
        let fields = schema.features(class)?;
        let members = fields
            .iter()
            .filter(|(_, field)| !field.derived)
            .map(|(name, _)| {
                Ok(DraftMemberDisposition {
                    name: name.clone(),
                    identity: schema.effective_feature(class, name)?.identity,
                    disposition: DraftDisposition::StoredDefault,
                })
            })
            .collect::<StorageResult<Vec<_>>>()?;
        rows.push(DraftClassContract {
            version: 1,
            schema_revision: schema.revision.clone(),
            class: class.clone(),
            class_identity: schema.identity(class),
            sources: definition
                .sources()
                .sources
                .iter()
                .map(|source| source.id.clone())
                .collect(),
            members,
        });
    }
    let mut changed = rows.clone();
    changed
        .first_mut()
        .ok_or("Missing actual class")?
        .class_identity
        .push_str("/drift");
    Ok(Native {
        rows,
        changed,
        calls: Cell::new(0),
        drift: false,
        cancel: None,
    })
}
#[test]
fn borrowed_capture_actual_rows_owned_graph_and_outer_rollback() -> StorageResult<()> {
    let (input, origin) = inputs()?;
    let definition = definition(&input, &origin)?;
    let owned_metadata = definition
        .prepare_physical_metadata(&InvocationContext::default())
        .map_err(|e| e.to_string())?;
    let owned_work = Meter::new(100_000);
    let owned = {
        let local = native(&definition)?;
        let captured = DraftConstructionContext::bind_frozen(
            &owned_metadata,
            &local,
            &InvocationContext::new(owned_work.clone()),
        )?;
        assert_eq!(local.calls.get(), 2);
        captured
    };
    assert!(
        owned
            .contracts
            .values()
            .all(|row| matches!(row, Cow::Owned(_)))
    );
    let metadata = definition
        .prepare_physical_metadata(&InvocationContext::default())
        .map_err(|e| e.to_string())?;
    let provider = native(&definition)?;
    let work = Meter::new(100_000);
    let borrowed = DraftConstructionContext::bind_frozen_borrowed(
        &metadata,
        &provider,
        &InvocationContext::new(work.clone()),
    )?;
    assert!(borrowed.is_frozen());
    assert_eq!(provider.calls.get(), 2);
    for (class, captured) in &borrowed.contracts {
        let Cow::Borrowed(actual) = captured else {
            return Err("Borrowed capture cloned its native row".into());
        };
        let source = provider
            .rows
            .iter()
            .find(|row| row.class == *class)
            .ok_or("Missing native source row")?;
        assert!(std::ptr::eq(*actual, source));
    }
    assert_eq!(
        serde_json::to_value(&owned.contracts).map_err(|e| e.to_string())?,
        serde_json::to_value(&borrowed.contracts).map_err(|e| e.to_string())?
    );
    let construct = |context: &DraftConstructionContext<'_>,
                     metadata: &crate::definitions::preparation::PreparedPhysicalMetadata<'_>|
     -> StorageResult<FrozenStore> {
        let mut store =
            FrozenStore::for_schema(definition.schema()).map_err(|error| error.to_string())?;
        atomic(&mut store, |store| {
            bind_schema(store, definition.schema())?;
            let receipt = context.create_pending(
                store,
                Construction {
                    id: "a",
                    resource: "memory:borrowed-control",
                    authored: &[],
                },
                "A",
                0,
                1,
                metadata,
                &mut QueryBudget::new(100_000),
            )?;
            assert_eq!(receipt.contract.class, "A");
            assert_eq!(receipt.contract.members.len(), 4);
            Ok(())
        })?;
        Ok(store)
    };
    let expected = export(&construct(&owned, &owned_metadata)?)?;
    let actual = export(&construct(&borrowed, &metadata)?)?;
    assert_eq!(expected, actual);
    assert_eq!(
        actual
            .elements
            .first()
            .ok_or("Missing actual object")?
            .values
            .get("label"),
        Some(&serde_json::json!("base"))
    );
    let mut store =
        FrozenStore::for_schema(definition.schema()).map_err(|error| error.to_string())?;
    let before = export(&store)?;
    let failure: StorageResult<()> = atomic(&mut store, |store| {
        bind_schema(store, definition.schema())?;
        borrowed.create_pending(
            store,
            Construction {
                id: "rollback",
                resource: "memory:borrowed-control",
                authored: &[],
            },
            "A",
            0,
            1,
            &metadata,
            &mut QueryBudget::new(100_000),
        )?;
        Err("Intentional outer rollback".into())
    });
    assert_eq!(failure, Err("Intentional outer rollback".into()));
    assert_eq!(export(&store)?, before);
    assert_eq!(provider.calls.get(), 2);
    assert!(work.work() < owned_work.work());
    assert!(work.bytes.get() < owned_work.bytes.get());
    println!(
        "S_BORROWED_CAPTURE_RAW {}",
        serde_json::json!({"owned_work":owned_work.work(),"borrowed_work":work.work(),"owned_bytes":owned_work.bytes.get(),"borrowed_bytes":work.bytes.get(),"actual_rows":provider.rows.len(),"same_source_row_pointers":true,"same_owned_graph":true,"outer_rollback":true,"owned_provider_may_drop":true,"SysML_fit":false})
    );
    Ok(())
}
struct Control {
    meter: Rc<Meter>,
    cancel: Cell<bool>,
}
impl InvocationObserver for Control {
    fn before_invoke(&self, provider: &ProviderId) -> StorageResult<()> {
        self.meter.before_invoke(provider)
    }
    fn work(&self, units: usize) -> StorageResult<()> {
        self.checkpoint()?;
        InvocationObserver::work(self.meter.as_ref(), units)
    }
    fn retain_storage(&self, bytes: usize) -> StorageResult<()> {
        self.checkpoint()?;
        self.meter.retain_storage(bytes)
    }
    fn checkpoint(&self) -> StorageResult<()> {
        if self.cancel.get() {
            Err("Borrowed control cancelled".into())
        } else {
            Ok(())
        }
    }
}
#[test]
fn borrowed_capture_exact_short_drift_stale_cancel_and_storage_refuse() -> StorageResult<()> {
    let (input, origin) = inputs()?;
    let definition = definition(&input, &origin)?;
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
    let mut drift = native(&definition)?;
    drift.drift = true;
    let fresh = prepare()?;
    let result = DraftConstructionContext::bind_frozen_borrowed(
        &fresh,
        &drift,
        &InvocationContext::new(Meter::new(100_000)),
    );
    let Err(error) = result else {
        return Err("Drift published a borrowed context".into());
    };
    assert!(error.contains("Draft physical capability changed after binding"));
    assert_eq!(drift.calls.get(), 2);
    let mut stale = native(&definition)?;
    stale
        .rows
        .first_mut()
        .ok_or("Missing native row")?
        .schema_revision
        .push_str("/stale");
    let fresh = prepare()?;
    let result = DraftConstructionContext::bind_frozen_borrowed(
        &fresh,
        &stale,
        &InvocationContext::new(Meter::new(100_000)),
    );
    let Err(error) = result else {
        return Err("Stale source row published a borrowed context".into());
    };
    assert!(error.contains("Invalid or stale draft physical construction contract"));
    let control = Rc::new(Control {
        meter: Meter::new(100_000),
        cancel: Cell::new(false),
    });
    let mut cancel = native(&definition)?;
    cancel.cancel = Some(&control.cancel);
    let fresh = prepare()?;
    let result = DraftConstructionContext::bind_frozen_borrowed(
        &fresh,
        &cancel,
        &InvocationContext::new(control.clone()),
    );
    let Err(error) = result else {
        return Err("Final cancellation published a borrowed context".into());
    };
    assert!(error.contains("cancelled"));
    assert_eq!(cancel.calls.get(), 2);
    let deny = Meter::new(100_000);
    deny.deny_retention.set(true);
    let fresh = prepare()?;
    assert!(
        DraftConstructionContext::bind_frozen_borrowed(
            &fresh,
            &provider,
            &InvocationContext::new(deny)
        )
        .is_err()
    );
    Ok(())
}

// Shared canonical compiler source-map consumer components. These sources are
// authored controls; they do not replace or qualify the original SysML request.
use crate::definitions::{
    bundle::{DefinitionSource, SourceKind},
    ecore::ParseLimits,
    ecore_migration::BundleMigrationReport,
    model_definition::ModelCompilation,
};
use std::sync::Arc;

fn completed_inputs() -> StorageResult<(ModelCompilation, BundleMigrationReport)> {
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
fn completed_owner(
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

fn completed_inherited_fixture() -> StorageResult<(ModelCompilation, BundleMigrationReport)> {
    let (prior, migration) = completed_inputs()?;
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

fn completed_projection_sources(
    inherited_derived_collision: bool,
) -> StorageResult<(ModelCompilation, BundleMigrationReport)> {
    let (mut original, migration) = completed_inherited_fixture()?;
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
fn completed_projection_fixture(
    inherited_derived_collision: bool,
) -> StorageResult<(ModelCompilation, BundleMigrationReport)> {
    let (original, origin) = completed_projection_sources(inherited_derived_collision)?;
    Ok((origin.remap_model_compilation(&original)?, origin))
}

// Actual ordinary compiler output, not a DTO or authored cache certificate.
#[test]
fn completed_source_maps_preserve_inherited_graph_and_delegated_absence() -> StorageResult<()> {
    let (input, origin) = completed_projection_fixture(false)?;
    let owner = completed_owner(input)?;
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
    let mut provider = native(&definition)?;
    for row in &mut provider.rows {
        for member in &mut row.members {
            if member.name == "tail" || (row.class == "Root" && member.name == "children") {
                member.disposition = DraftDisposition::DelegatedAbsent;
            }
        }
    }
    let cold_work = Meter::new(100_000);
    let cold = bind_draft_contracts_with_capture::<BorrowedContractCapture>(
        definition.schema(),
        definition.sources(),
        &provider,
        None,
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
        17
    );
    assert!(
        !actual
            .elements
            .first()
            .ok_or("No delegated object")?
            .values
            .contains_key("tail")
    );
    assert!(warm_work.work() < cold_work.work());
    println!(
        "S_COMPILER_COMPLETED_FIELDS_RAW {}",
        serde_json::json!({
            "cold_bind_work":cold_work.work(),"completed_bind_work":warm_work.work(),
            "completed_bind_bytes":warm_work.bytes.get(),"actual_inherited_map_ptr_equal":true,
            "ordinary_source_classes":definition.schema().classes.len(),
            "runtime_domain_receivers":0,"actual_inherited_graph_fields":17,"actual_tail_delegated_absent":true,
            "all_source_fields_preserved":true,"SysML_fit":false,"full_K01_verified":false
        })
    );
    Ok(())
}

#[test]
fn completed_source_maps_foreign_storage_cancel_and_exact_short() -> StorageResult<()> {
    let (input, origin) = completed_projection_fixture(false)?;
    let owner = completed_owner(input)?;
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
fn completed_source_maps_reject_equal_foreign_bundle_stale_and_final_provider_changes()
-> StorageResult<()> {
    let (input, origin) = completed_projection_fixture(false)?;
    let owner = completed_owner(input)?;
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
    let foreign_bundle = definition.sources().clone();
    let result = bind_draft_contracts_with_capture::<BorrowedContractCapture>(
        definition.schema(),
        &foreign_bundle,
        &provider,
        metadata.compiled_sources(),
        &mut DraftBindWork::Observed(&InvocationContext::new(Meter::new(100_000))),
        Some(&metadata),
    );
    let Err(error) = result else {
        return Err("Equal copied bundle forged the actual compiler owner".into());
    };
    assert!(error.contains("compiled source/schema owner association changed"));
    let mut stale = native(&definition)?;
    stale
        .rows
        .first_mut()
        .ok_or("No native row")?
        .schema_revision
        .push_str("/stale");
    let Err(error) = DraftConstructionContext::bind_frozen_borrowed(
        &metadata,
        &stale,
        &InvocationContext::new(Meter::new(100_000)),
    ) else {
        return Err("Stale hot physical contract admitted".into());
    };
    assert!(error.contains("Invalid or stale draft physical construction contract"));
    let mut drift = native(&definition)?;
    drift.drift = true;
    let Err(error) = DraftConstructionContext::bind_frozen_borrowed(
        &metadata,
        &drift,
        &InvocationContext::new(Meter::new(100_000)),
    ) else {
        return Err("Final hot provider drift published context".into());
    };
    assert!(error.contains("Draft physical capability changed after binding"));
    assert_eq!(drift.calls.get(), 2);
    let control = Rc::new(Control {
        meter: Meter::new(100_000),
        cancel: Cell::new(false),
    });
    let mut canceled = native(&definition)?;
    canceled.cancel = Some(&control.cancel);
    let Err(error) = DraftConstructionContext::bind_frozen_borrowed(
        &metadata,
        &canceled,
        &InvocationContext::new(control.clone()),
    ) else {
        return Err("Final canceled hot source bind published context".into());
    };
    assert!(error.contains("cancelled"));
    assert_eq!(canceled.calls.get(), 2);
    Ok(())
}
