use super::*;
use crate::definitions::{
    bundle::{DefinitionSource, SourceKind},
    conversion::{
        ConversionContract, ConversionProvider, ConversionRegistry, InvocationContext,
        InvocationObserver, ProviderId,
    },
    ecore::ObservedImportError,
};
use std::{cell::Cell, rc::Rc};
#[path = "materialization_canonical_source_tests.rs"]
mod canonical;

fn fixture() -> (DefinitionBundle, SourceId, Vec<RuntimeType>) {
    let mut bundle: DefinitionBundle = serde_json::from_str(include_str!(
        "../../assets/fixtures/ecore-generics/materialization-source-preparation/bundle.json"
    ))
    .unwrap();
    // The captured bundle's unused Ecore Unknown node deliberately cannot migrate.
    // Subset-loading controls now use an independent canonical unused resource.
    bundle.sources.pop();
    let mut bundle = crate::definitions::migration_test_fixture::bundle(bundle);
    bundle.sources.push(DefinitionSource::new("file:///loading/unselected.rcore", SourceKind::Rcore,
        "rcore 1; package unused version \"1\"; metamodel \"urn:loading:a\" prefix unused { class Unconsumed {} }"));
    let types = bundle.sources[..2]
        .iter()
        .map(|s| {
            RuntimeType::new(Type::Classifier {
                declaration: DeclarationId {
                    source: s.id.clone(),
                    path: vec![0],
                },
                arguments: vec![],
                raw: false,
            })
            .unwrap()
        })
        .collect();
    (bundle.clone(), bundle.sources[0].id.clone(), types)
}
fn begin_session(
    bundle: DefinitionBundle,
    entry: SourceId,
    types: &[RuntimeType],
    registry: Option<&ConversionRegistry<'_>>,
    operation: &InvocationContext<'_>,
) -> OwnedRuntimeSourceArrayPreparation {
    OwnedRuntimeSourceArrayPreparation::begin_with_compiled_signature_inputs(
        bundle,
        entry,
        "urn:loading:a".into(),
        types,
        registry,
        operation,
    )
    .unwrap_or_else(|_| panic!("source begin failed"))
}
fn finish_session(
    mut session: OwnedRuntimeSourceArrayPreparation,
    registry: Option<&ConversionRegistry<'_>>,
    operation: &InvocationContext<'_>,
) -> OwnedRuntimeSourceArrayPreparationResult {
    while !session.advance(registry, operation).unwrap() {}
    session.finish()
}
#[derive(Default)]
struct Meter {
    cancelled: Cell<bool>,
    work: Cell<usize>,
    bytes: Cell<usize>,
    calls: Cell<usize>,
    stop: Option<usize>,
    work_limit: Option<usize>,
    byte_limit: Option<usize>,
}
impl Meter {
    fn touch(&self) -> Result<(), String> {
        let n = self.calls.get();
        self.calls.set(n + 1);
        if self.cancelled.get() || self.stop == Some(n) {
            Err("source load interrupted".into())
        } else {
            Ok(())
        }
    }
}
impl InvocationObserver for Meter {
    fn before_invoke(&self, _: &ProviderId) -> Result<(), String> {
        Err("Unexpected provider call".into())
    }
    fn checkpoint(&self) -> Result<(), String> {
        self.touch()
    }
    fn work(&self, n: usize) -> Result<(), String> {
        self.touch()?;
        let value = self.work.get() + n;
        if self.work_limit.is_some_and(|limit| value > limit) {
            return Err("source work denied".into());
        }
        self.work.set(value);
        Ok(())
    }
    fn retain_storage(&self, n: usize) -> Result<(), String> {
        self.touch()?;
        let value = self.bytes.get() + n;
        if self.byte_limit.is_some_and(|limit| value > limit) {
            return Err("source storage denied".into());
        }
        self.bytes.set(value);
        Ok(())
    }
}
#[test]
fn actual_compiler_retains_source_loading_then_reuses_selected_resources_and_recovers_original_owner()
 {
    let (bundle, entry, types) = fixture();
    let operation = InvocationContext::default();
    let expected =
        materialize_runtime_bundle(&bundle, &entry, "urn:loading:a", &types, None).unwrap();
    let ptr = bundle.sources[0].text.as_ptr();
    let mut session = begin_session(bundle.clone(), entry.clone(), &types, None, &operation);
    assert!(session.root.is_none());
    assert!(matches!(
        session.preparation.holder.borrow_dependent(),
        MaterializationState::Loading(_)
    ));
    assert!(
        session
            .preparation
            .holder
            .borrow_owner()
            .resolver
            .get()
            .is_none()
    );
    let mut loads = 0;
    while session.root.is_none() {
        assert!(!session.advance(None, &operation).unwrap());
        loads += 1;
        assert!(session.schema.is_none());
    }
    assert!(
        loads > 30,
        "resource parsing/indexing must span real advances"
    );
    session.preparation.holder.with_dependent(|owner, state| {
        let MaterializationState::Ready(ready) = state else {
            panic!("not ready")
        };
        assert_eq!(ready.bundle.sources.len(), 2);
        assert_eq!(owner.bundle, bundle);
        assert!(ready.resources.document(&bundle.sources[2].id).is_err());
        assert_eq!(
            ready.resources.document(&entry).unwrap().source,
            bundle.sources[0]
        );
        assert!(ready.source_inventory.is_some());
    });
    let result = finish_session(session, None, &operation);
    assert_eq!(result.materialization.bundle, bundle);
    assert_eq!(result.entry_source, entry);
    assert_eq!(
        serde_json::to_value(result.materialization.result.unwrap()).unwrap(),
        serde_json::to_value(expected).unwrap()
    );
    // Separate move proves exact recovery uses the original allocation.
    let session = begin_session(bundle, entry, &types, None, &operation);
    let result = session.finish();
    assert!(result.materialization.result.is_err());
    assert_eq!(result.materialization.bundle.sources[0].text.as_ptr(), ptr);
}
#[test]
#[cfg_attr(not(feature = "milestone-tests"), ignore = "milestone")]
fn source_loading_cancels_terminally_at_every_boundary_and_releases_each_observer() {
    let (bundle, entry, types) = fixture();
    let operation = InvocationContext::default();
    let mut baseline = begin_session(bundle.clone(), entry.clone(), &types, None, &operation);
    let mut loads = 0;
    while baseline.root.is_none() {
        baseline.advance(None, &operation).unwrap();
        loads += 1;
    }
    for prefix in 0..=loads {
        let original = bundle.clone();
        let ptr = original.sources[0].text.as_ptr();
        let mut session = begin_session(original, entry.clone(), &types, None, &operation);
        for _ in 0..prefix {
            assert!(!session.advance(None, &operation).unwrap());
        }
        let control = Rc::new(Meter {
            cancelled: Cell::new(true),
            ..Default::default()
        });
        let weak = Rc::downgrade(&control);
        let observed = InvocationContext::new(control.clone());
        assert!(matches!(
            session.advance(None, &observed),
            Err(ObservedImportError::Observation(_))
        ));
        control.cancelled.set(false);
        drop(control);
        drop(observed);
        assert!(weak.upgrade().is_none());
        assert!(session.advance(None, &operation).is_err());
        let recovered = session.finish();
        assert!(recovered.materialization.result.is_err());
        assert_eq!(recovered.materialization.bundle, bundle);
        assert_eq!(
            recovered.materialization.bundle.sources[0].text.as_ptr(),
            ptr
        );
    }
    assert!(
        finish_session(
            begin_session(bundle, entry, &types, None, &operation),
            None,
            &operation
        )
        .materialization
        .result
        .is_ok()
    );
}
fn loading_receipt(
    bundle: &DefinitionBundle,
    entry: &SourceId,
    types: &[RuntimeType],
    meter: Rc<Meter>,
) -> Result<(), ObservedImportError> {
    let operation = InvocationContext::new(meter);
    let ptr = bundle.sources[0].text.as_ptr();
    let mut session = match OwnedRuntimeSourceArrayPreparation::begin_with_compiled_signature_inputs(
        bundle.clone(),
        entry.clone(),
        "urn:loading:a".into(),
        types,
        None,
        &operation,
    ) {
        Ok(session) => session,
        Err(failure) => {
            assert_eq!(failure.materialization.bundle, *bundle);
            return failure.materialization.result.map(|_| ());
        }
    };
    let result = loop {
        match session.advance(None, &operation) {
            Ok(false) if session.root.is_none() => {}
            Ok(_) => break Ok(()),
            Err(error) => break Err(error),
        }
    };
    let recovered = session.finish();
    assert_eq!(recovered.materialization.bundle, *bundle);
    assert!(recovered.materialization.result.is_err());
    assert_ne!(
        ptr,
        recovered.materialization.bundle.sources[0].text.as_ptr()
    );
    result
}
#[test]
#[cfg_attr(not(feature = "milestone-tests"), ignore = "milestone")]
fn loading_observation_failures_and_adjacent_limits_preserve_inputs_without_partial_publication() {
    let (bundle, entry, types) = fixture();
    let meter = Rc::new(Meter::default());
    loading_receipt(&bundle, &entry, &types, meter.clone()).unwrap();
    let calls = meter.calls.get();
    let work = meter.work.get();
    let bytes = meter.bytes.get();
    // Every observer interruption in loading, including initialization and Ready installation.
    for stop in 0..calls {
        assert!(
            matches!(
                loading_receipt(
                    &bundle,
                    &entry,
                    &types,
                    Rc::new(Meter {
                        stop: Some(stop),
                        ..Default::default()
                    })
                ),
                Err(ObservedImportError::Observation(_))
            ),
            "observer {stop}"
        );
    }
    for (w, b, ok) in [
        (work, bytes, true),
        (work - 1, bytes, false),
        (work, bytes - 1, false),
    ] {
        assert_eq!(
            loading_receipt(
                &bundle,
                &entry,
                &types,
                Rc::new(Meter {
                    work_limit: Some(w),
                    byte_limit: Some(b),
                    ..Default::default()
                })
            )
            .is_ok(),
            ok
        );
    }
}
struct Provider {
    initial: ConversionContract,
    changed: ConversionContract,
    stale: Cell<bool>,
}
impl ConversionProvider for Provider {
    fn contract(&self) -> &ConversionContract {
        if self.stale.get() {
            &self.changed
        } else {
            &self.initial
        }
    }
    fn parse(&self, _: &str) -> Result<Value, String> {
        Err("No executable calls during loading".into())
    }
    fn format(&self, _: &Value) -> Result<String, String> {
        Err("No executable calls during loading".into())
    }
    fn validate(&self, _: &Value) -> Result<(), String> {
        Err("No executable calls during loading".into())
    }
    fn default_value(&self) -> Result<Option<Value>, String> {
        Err("No executable calls during loading".into())
    }
}
#[test]
fn metadata_captured_before_loading_cannot_be_silently_replaced_and_no_registry_is_retained() {
    for retain_canonical in [false, true] {
        let begin_session = |bundle: DefinitionBundle,
                             entry: SourceId,
                             types: &[RuntimeType],
                             registry: Option<&ConversionRegistry<'_>>,
                             operation: &InvocationContext<'_>| {
            let constructor = if retain_canonical {
                OwnedRuntimeSourceArrayPreparation::begin_with_retained_canonical_sources
            } else {
                OwnedRuntimeSourceArrayPreparation::begin_with_compiled_signature_inputs
            };
            constructor(
                bundle,
                entry,
                "urn:loading:a".into(),
                types,
                registry,
                operation,
            )
            .unwrap_or_else(|_| panic!("source begin failed"))
        };
        let (bundle, entry, types) = fixture();
        let operation = InvocationContext::default();
        // A registered contract need not be executed to seal the attempt's selections.
        let initial = ConversionContract {
            datatype: DeclarationId {
                source: entry.clone(),
                path: vec![1],
            },
            provider: ProviderId {
                name: "test.loading".into(),
                version: "1".into(),
            },
            serializable: true,
            declarations: vec![],
            members: vec![],
        };
        let mut changed = initial.clone();
        changed.provider.version = "2".into();
        let provider = Rc::new(Provider {
            initial,
            changed,
            stale: Cell::new(false),
        });
        let weak = Rc::downgrade(&provider);
        let mut registry = ConversionRegistry::default();
        let datatype = crate::definitions::datatypes::Datatype::new(
            provider.initial.datatype.clone(),
            "Text".into(),
            true,
            Some("java.lang.String".into()),
            None,
        )
        .unwrap();
        registry
            .register_shared(&datatype, provider.clone())
            .unwrap();
        let mut session = begin_session(
            bundle.clone(),
            entry.clone(),
            &types,
            Some(&registry),
            &operation,
        );
        assert!(!session.advance(Some(&registry), &operation).unwrap());
        provider.stale.set(true);
        let error = loop {
            match session.advance(Some(&registry), &operation) {
                Ok(false) => {}
                Ok(true) => panic!("changed metadata accepted"),
                Err(error) => break error,
            }
        };
        assert!(matches!(error, ObservedImportError::Import(_)));
        assert!(session.root.is_none());
        let mut malformed = bundle.clone();
        malformed.sources[2] = DefinitionSource::new(
            "file:///loading/unselected.rcore",
            SourceKind::Rcore,
            "rcore 1; package broken {",
        );
        let expected = materialize_runtime_bundle(
            &malformed,
            &entry,
            "urn:loading:a",
            &types,
            Some(&registry),
        )
        .unwrap_err();
        let mut queued = begin_session(
            malformed.clone(),
            entry.clone(),
            &types,
            Some(&registry),
            &operation,
        );
        let actual = loop {
            match queued.advance(Some(&registry), &operation) {
                Ok(false) => {}
                Ok(true) => panic!("malformed source accepted"),
                Err(error) => break error,
            }
        };
        assert_eq!(actual, ObservedImportError::Import(expected));
        assert_eq!(queued.finish().materialization.bundle, malformed);
        drop(registry);
        drop(provider);
        assert!(weak.upgrade().is_none());
        assert!(session.advance(None, &operation).is_err());
        assert_eq!(session.finish().materialization.bundle, bundle);
    }
}
#[test]
fn source_and_selection_errors_keep_error_priority_and_exact_spans_after_deferred_loading() {
    let (bundle, entry, types) = fixture();
    for kind in 0..4 {
        let mut input = bundle.clone();
        let mut selected = entry.clone();
        let mut namespace = "urn:loading:a".to_string();
        match kind {
            0 => {
                input.sources[2] = DefinitionSource::new(
                    "file:///loading/unselected.rcore",
                    SourceKind::Rcore,
                    "rcore 1; package broken {",
                );
            }
            1 => {
                namespace = "urn:missing".into();
            }
            2 => {
                selected.sha256 = "0".repeat(64);
            }
            _ => {
                input.namespaces[0].source = input.sources[1].id.clone();
                input.namespaces[0].namespace = "urn:loading:b".into();
                namespace = "urn:loading:b".into();
            }
        }
        let operation = InvocationContext::default();
        let expected =
            materialize_runtime_bundle(&input, &selected, &namespace, &types, None).unwrap_err();
        let result = OwnedRuntimeSourceArrayPreparation::begin_with_compiled_signature_inputs(
            input.clone(),
            selected.clone(),
            namespace.clone(),
            &types,
            None,
            &operation,
        );
        let actual = match result {
            Err(failure) => {
                assert_eq!(failure.materialization.bundle, input);
                failure.materialization.result.unwrap_err()
            }
            Ok(mut session) => {
                loop {
                    match session.advance(None, &operation) {
                        Ok(false) => {}
                        Ok(true) => panic!("invalid source accepted"),
                        Err(_) => break,
                    }
                }
                let failed = session.finish();
                assert_eq!(failed.materialization.bundle, input);
                assert_eq!(failed.entry_source, selected);
                assert_eq!(failed.namespace, namespace);
                failed.materialization.result.unwrap_err()
            }
        };
        assert_eq!(
            actual,
            ObservedImportError::Import(expected),
            "variant {kind}"
        );
    }
}

fn in_standard_phase(session: &OwnedRuntimeSourceArrayPreparation) -> bool {
    session.preparation.holder.with_dependent(|_, state| matches!(state, MaterializationState::Loading(loading) if matches!(loading.phase, LoadingPhase::Standard)))
}
#[test]
fn standard_source_parsing_yields_before_readiness_and_moves_exact_documents_into_the_resolver() {
    let (bundle, entry, types) = fixture();
    let operation = InvocationContext::default();
    let expected =
        materialize_runtime_bundle(&bundle, &entry, "urn:loading:a", &types, None).unwrap();
    let mut session = begin_session(bundle.clone(), entry.clone(), &types, None, &operation);
    while !in_standard_phase(&session) {
        assert!(!session.advance(None, &operation).unwrap());
    }
    assert!(session.root.is_none());
    assert!(
        session
            .preparation
            .holder
            .borrow_owner()
            .resolver
            .get()
            .is_none()
    );
    let standard = standard_datatype_source_observed(
        &crate::definitions::ecore::ParserObservation::new(&operation),
    )
    .unwrap();
    assert_eq!(
        standard.id.sha256,
        "4ef00e244eda85b2da8670bb5582eb7c8d762d459e3e2c834b228384733c0aa6"
    );
    let mut advances = 0;
    while in_standard_phase(&session) {
        assert!(!session.advance(None, &operation).unwrap());
        advances += 1;
        assert!(session.root.is_none());
    }
    assert!(
        advances > 100,
        "pinned XML must use real event/index advances"
    );
    let pointer = session.preparation.holder.with_dependent(|_, state| {
        let MaterializationState::Loading(loading) = state else {
            panic!("premature readiness")
        };
        assert!(matches!(loading.phase, LoadingPhase::Install));
        let document = loading
            .standard
            .as_ref()
            .unwrap()
            .document(&standard.id)
            .unwrap();
        assert_eq!(document.source, standard);
        document.source.text.as_ptr()
    });
    assert!(!session.advance(None, &operation).unwrap());
    assert!(session.root.is_some());
    session.preparation.holder.with_dependent(|_, state| {
        let MaterializationState::Ready(ready) = state else {
            panic!("missing complete readiness")
        };
        assert_eq!(
            ready
                .standard
                .document(&standard.id)
                .unwrap()
                .source
                .text
                .as_ptr(),
            pointer
        );
    });
    let finished = finish_session(session, None, &operation);
    assert_eq!(finished.materialization.bundle, bundle);
    assert_eq!(
        serde_json::to_value(finished.materialization.result.unwrap()).unwrap(),
        serde_json::to_value(expected).unwrap()
    );
}
#[test]
fn standard_source_reuse_requires_exact_user_and_standard_inputs_without_reparsing_or_authority() {
    let (mut bundle, _, _) = fixture();
    bundle.sources.truncate(2);
    let operation = InvocationContext::default();
    let observation = crate::definitions::ecore::ParserObservation::new(&operation);
    let standard_bundle = DefinitionBundle {
        version: 1,
        sources: vec![standard_datatype_source_observed(&observation).unwrap()],
        namespaces: vec![],
    };
    let source = &standard_bundle.sources[0];
    let resources = ResourceSet::new(&bundle).unwrap();
    let standard = ResourceSet::new(&standard_bundle).unwrap();
    let user_pointer = resources
        .document(&bundle.sources[0].id)
        .unwrap()
        .source
        .text
        .as_ptr();
    let standard_pointer = standard.document(&source.id).unwrap().source.text.as_ptr();
    for variant in 0..6 {
        let mut changed_user = bundle.clone();
        let mut changed_standard = standard_bundle.clone();
        match variant {
            0 => changed_standard.sources[0].text.push(' '),
            1 => changed_standard.sources[0].id.sha256 = "0".repeat(64),
            2 => changed_standard
                .namespaces
                .push(crate::definitions::bundle::NamespaceBinding {
                    namespace: "urn:unexpected".into(),
                    source: source.id.clone(),
                }),
            3 => changed_standard.sources[0].kind = SourceKind::RcoreBehavior,
            4 => changed_standard.version = 2,
            _ => changed_user.sources[0].dependencies.clear(),
        }
        let error = match Resolver::with_prepared_resource_inputs(
            &changed_user,
            &changed_standard,
            resources.clone(),
            standard.clone(),
            &observation,
        ) {
            Ok(_) => panic!("changed inputs accepted"),
            Err(error) => error,
        };
        assert_eq!(error.kind, ErrorKind::InvalidDefinition);
        assert!(error.message.contains("exact bundles"));
    }
    let resolver = Resolver::with_prepared_resource_inputs(
        &bundle,
        &standard_bundle,
        resources,
        standard,
        &observation,
    )
    .unwrap();
    assert_eq!(
        resolver
            .resource_sets()
            .0
            .document(&bundle.sources[0].id)
            .unwrap()
            .source
            .text
            .as_ptr(),
        user_pointer
    );
    assert_eq!(
        resolver
            .resource_sets()
            .1
            .document(&source.id)
            .unwrap()
            .source
            .text
            .as_ptr(),
        standard_pointer
    );
}
#[test]
fn standard_source_phase_does_not_allow_changed_provider_metadata_to_grant_readiness() {
    let (bundle, entry, types) = fixture();
    let operation = InvocationContext::default();
    let initial = ConversionContract {
        datatype: DeclarationId {
            source: entry.clone(),
            path: vec![1],
        },
        provider: ProviderId {
            name: "test.standard.loading".into(),
            version: "1".into(),
        },
        serializable: true,
        declarations: vec![],
        members: vec![],
    };
    let mut changed = initial.clone();
    changed.provider.version = "2".into();
    let provider = Rc::new(Provider {
        initial,
        changed,
        stale: Cell::new(false),
    });
    let datatype = crate::definitions::datatypes::Datatype::new(
        provider.initial.datatype.clone(),
        "Text".into(),
        true,
        Some("java.lang.String".into()),
        None,
    )
    .unwrap();
    let mut registry = ConversionRegistry::default();
    registry
        .register_shared(&datatype, provider.clone())
        .unwrap();
    let mut session = begin_session(bundle.clone(), entry, &types, Some(&registry), &operation);
    while !in_standard_phase(&session) {
        assert!(!session.advance(Some(&registry), &operation).unwrap());
    }
    provider.stale.set(true);
    let error = loop {
        match session.advance(Some(&registry), &operation) {
            Ok(false) => {}
            Ok(true) => panic!("changed metadata published"),
            Err(error) => break error,
        }
    };
    assert!(matches!(error, ObservedImportError::Import(_)));
    assert!(session.root.is_none());
    assert!(session.advance(Some(&registry), &operation).is_err());
    assert_eq!(session.finish().materialization.bundle, bundle);
}
