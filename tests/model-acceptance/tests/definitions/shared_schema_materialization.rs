use intersect_model::definitions::{
    bundle::{DefinitionBundle, DefinitionSource, SourceKind},
    compiler,
    conversion::{InvocationContext, InvocationObserver, ProviderId},
    ecore::ObservedImportError,
    generic_types::{RuntimeType, Type},
    resources::DeclarationId,
    schema::RuntimeSchemaInputs,
};
use std::{cell::Cell, rc::Rc};
fn fixture(subset: bool) -> (DefinitionBundle, Vec<RuntimeType>) {
    let source = DefinitionSource::new(
        "memory:cold-shared.ecore",
        SourceKind::Ecore,
        r##"<e:EPackage xmlns:e="http://www.eclipse.org/emf/2002/Ecore" xmlns:x="http://www.w3.org/2001/XMLSchema-instance" name="m" nsURI="urn:cold-shared"><eClassifiers x:type="e:EClass" name="Root"><eTypeParameters name="T"><eBounds eClassifier="#//Root"/></eTypeParameters><eStructuralFeatures x:type="e:EReference" name="r0"><eGenericType eTypeParameter="#//Root/T"/></eStructuralFeatures><eStructuralFeatures x:type="e:EReference" name="r1"><eGenericType eTypeParameter="#//Root/T"/></eStructuralFeatures></eClassifiers><eClassifiers x:type="e:EClass" name="C0" eSuperTypes="#//Root"/><eClassifiers x:type="e:EClass" name="C1" eSuperTypes="#//Root"/><eClassifiers x:type="e:EClass" name="C2" eSuperTypes="#//Root"/></e:EPackage>"##,
    );
    let (source, _) = intersect_model_acceptance::migration_fixture::migrate_source_fixture(source);
    let types = (0..4)
        .map(|i| {
            RuntimeType::new(Type::Classifier {
                declaration: DeclarationId {
                    source: source.id.clone(),
                    path: vec![i],
                },
                arguments: vec![],
                raw: i == 0,
            })
            .unwrap()
        })
        .collect();
    let mut sources = vec![source];
    if subset {
        sources.push(DefinitionSource::new("memory:unselected.rcore", SourceKind::Rcore, "rcore 1; package other version \"1\"; metamodel \"urn:other\" prefix other { class Unused {} }"));
    }
    (
        DefinitionBundle {
            version: 1,
            sources,
            namespaces: vec![],
        },
        types,
    )
}
struct Meter {
    bytes: Cell<usize>,
    checks: Cell<usize>,
    work: Cell<usize>,
    limit: usize,
    stop: usize,
}
impl InvocationObserver for Meter {
    fn before_invoke(&self, _: &ProviderId) -> Result<(), String> {
        panic!("reference-only materialization cannot invoke providers")
    }
    fn checkpoint(&self) -> Result<(), String> {
        if self.checks.get() == self.stop {
            return Err("cold construction cancelled".into());
        }
        self.checks.set(self.checks.get() + 1);
        Ok(())
    }
    fn work(&self, n: usize) -> Result<(), String> {
        self.work.set(self.work.get() + n);
        Ok(())
    }
    fn retain_storage(&self, n: usize) -> Result<(), String> {
        let next = self.bytes.get() + n;
        if next > self.limit {
            return Err("cold construction storage denied".into());
        }
        self.bytes.set(next);
        Ok(())
    }
}
fn run(
    bundle: &DefinitionBundle,
    types: &[RuntimeType],
    limit: usize,
    stop: usize,
) -> (
    Result<RuntimeSchemaInputs, ObservedImportError>,
    usize,
    usize,
) {
    let meter = Rc::new(Meter {
        bytes: Cell::new(0),
        checks: Cell::new(0),
        work: Cell::new(0),
        limit,
        stop,
    });
    let ctx = InvocationContext::new(meter.clone());
    let result = compiler::materialize_runtime_bundle_inputs_observed(
        bundle,
        &bundle.sources[0].id,
        "urn:cold-shared",
        types,
        None,
        &ctx,
    );
    (result, meter.bytes.get(), meter.checks.get())
}
#[test]
fn cold_shared_materialization_preserves_full_owned_output_subset_fallback_and_source_errors() {
    for subset in [false, true] {
        let (bundle, types) = fixture(subset);
        let owned = compiler::materialize_runtime_bundle(
            &bundle,
            &bundle.sources[0].id,
            "urn:cold-shared",
            &types,
            None,
        )
        .unwrap();
        let shared = run(&bundle, &types, usize::MAX, usize::MAX).0.unwrap();
        assert_eq!(
            serde_json::to_vec(&shared).unwrap(),
            serde_json::to_vec(&owned).unwrap()
        );
        assert_eq!(shared.classes.len(), 4);
        let mut pointers = std::collections::BTreeMap::new();
        let mut count = 0;
        for key in shared
            .runtime_feature_domains
            .values()
            .flatten()
            .flat_map(|d| &d.alternatives)
        {
            count += 1;
            if let Some(previous) = pointers.insert(key.as_ref(), key.as_ref().as_ptr()) {
                assert_eq!(previous, key.as_ref().as_ptr());
            }
        }
        assert_eq!(count, 32);
        assert_eq!(pointers.len(), 4);
        assert_eq!(
            shared
                .into_owned_observed(&InvocationContext::default())
                .unwrap(),
            owned
        );
        let mut stale = bundle.clone();
        stale.sources[0].text.push(' ');
        let a = compiler::materialize_runtime_bundle(
            &stale,
            &stale.sources[0].id,
            "urn:cold-shared",
            &types,
            None,
        )
        .unwrap_err();
        let b = run(&stale, &types, usize::MAX, usize::MAX).0.unwrap_err();
        assert!(matches!(b,ObservedImportError::Import(error) if error==a));
    }
}
#[test]
fn cold_shared_materialization_precharges_exact_output_and_cancels_without_publication() {
    let (bundle, types) = fixture(false);
    let (result, needed, checks) = run(&bundle, &types, usize::MAX, usize::MAX);
    let schema = result.unwrap();
    let encoded = serde_json::to_vec(&schema).unwrap();
    for limit in [0, needed - 1, needed] {
        let (result, bytes, _) = run(&bundle, &types, limit, usize::MAX);
        if limit < needed {
            assert!(
                matches!(result,Err(ObservedImportError::Observation(ref e)) if e=="cold construction storage denied")
            );
        } else {
            assert_eq!(bytes, needed);
            assert_eq!(serde_json::to_vec(&result.unwrap()).unwrap(), encoded);
        }
    }
    for stop in [0, 1, checks / 4, checks / 2, checks * 3 / 4, checks - 1] {
        assert!(
            matches!(run(&bundle,&types,usize::MAX,stop).0,Err(ObservedImportError::Observation(ref e)) if e=="cold construction cancelled")
        );
    }
    assert_eq!(
        serde_json::to_vec(&run(&bundle, &types, needed, usize::MAX).0.unwrap()).unwrap(),
        encoded
    );
}

#[test]
fn cold_complete_domain_inputs_preserve_source_metadata_sharing_and_full_output() {
    use intersect_model::definitions::schema::RuntimeDomainSource;
    for subset in [false, true] {
        let (bundle, types) = fixture(subset);
        let owned = compiler::materialize_runtime_bundle(
            &bundle,
            &bundle.sources[0].id,
            "urn:cold-shared",
            &types,
            None,
        )
        .unwrap();
        let shared = compiler::materialize_runtime_domain_inputs_observed(
            &bundle,
            &bundle.sources[0].id,
            "urn:cold-shared",
            &types,
            None,
            &InvocationContext::default(),
        )
        .unwrap();
        assert_eq!(
            serde_json::to_vec(&shared).unwrap(),
            serde_json::to_vec(&owned).unwrap()
        );
        let mut sources = std::collections::BTreeMap::new();
        let mut count = 0;
        let mut shared_count = 0;
        for domain in shared.runtime_feature_domains.values().flatten() {
            count += 1;
            let view = domain.source.view();
            let identity = serde_json::to_vec(&view.projection.source).unwrap();
            let pointer = view.projection.source.declaration as *const DeclarationId;
            if let Some(previous) = sources.insert(identity, pointer) {
                assert_eq!(previous, pointer);
                shared_count += 1;
            }
        }
        assert_eq!(count, 8);
        assert!(shared_count > 0);
        assert_eq!(
            shared
                .into_owned_observed(&InvocationContext::default())
                .unwrap(),
            owned
        );
    }
}

#[test]
fn cold_complete_domain_inputs_keep_storage_and_selected_cancellation_boundaries() {
    use intersect_model::definitions::schema::RuntimeDomainSchemaInputs;
    fn complete(
        bundle: &DefinitionBundle,
        types: &[RuntimeType],
        limit: usize,
        stop: usize,
    ) -> (
        Result<RuntimeDomainSchemaInputs, ObservedImportError>,
        usize,
        usize,
    ) {
        let meter = Rc::new(Meter {
            bytes: Cell::new(0),
            checks: Cell::new(0),
            work: Cell::new(0),
            limit,
            stop,
        });
        let context = InvocationContext::new(meter.clone());
        let result = compiler::materialize_runtime_domain_inputs_observed(
            bundle,
            &bundle.sources[0].id,
            "urn:cold-shared",
            types,
            None,
            &context,
        );
        (result, meter.bytes.get(), meter.checks.get())
    }
    let (bundle, types) = fixture(false);
    let (success, bytes, checks) = complete(&bundle, &types, usize::MAX, usize::MAX);
    let encoded = serde_json::to_vec(&success.unwrap()).unwrap();
    for limit in [0, bytes - 1] {
        assert!(
            matches!(complete(&bundle,&types,limit,usize::MAX).0,Err(ObservedImportError::Observation(message)) if message=="cold construction storage denied")
        );
    }
    assert_eq!(
        serde_json::to_vec(&complete(&bundle, &types, bytes, usize::MAX).0.unwrap()).unwrap(),
        encoded
    );
    for stop in [0, 1, checks / 4, checks / 2, checks * 3 / 4, checks - 1] {
        assert!(
            matches!(complete(&bundle,&types,usize::MAX,stop).0,Err(ObservedImportError::Observation(message)) if message=="cold construction cancelled")
        );
    }
    assert_eq!(
        serde_json::to_vec(&complete(&bundle, &types, usize::MAX, usize::MAX).0.unwrap()).unwrap(),
        encoded
    );
}

fn array_inputs(
    bundle: &DefinitionBundle,
    types: &[RuntimeType],
    limit: usize,
    stop: usize,
) -> (
    Result<intersect_model::definitions::schema::RuntimeArraySchemaInputs, ObservedImportError>,
    usize,
    usize,
) {
    let meter = Rc::new(Meter {
        bytes: Cell::new(0),
        checks: Cell::new(0),
        work: Cell::new(0),
        limit,
        stop,
    });
    let result = compiler::materialize_runtime_array_inputs_observed(
        bundle,
        &bundle.sources[0].id,
        "urn:cold-shared",
        types,
        None,
        &InvocationContext::new(meter.clone()),
    );
    (result, meter.bytes.get(), meter.checks.get())
}
#[test]
fn cold_array_inputs_preserve_complete_output_and_share_only_equal_full_arrays() {
    for subset in [false, true] {
        let (bundle, types) = fixture(subset);
        let owned = compiler::materialize_runtime_bundle(
            &bundle,
            &bundle.sources[0].id,
            "urn:cold-shared",
            &types,
            None,
        )
        .unwrap();
        let (result, _, _) = array_inputs(&bundle, &types, usize::MAX, usize::MAX);
        let arrays = result.unwrap();
        let encoded = serde_json::to_vec(&owned).unwrap();
        assert_eq!(serde_json::to_vec(&arrays).unwrap(), encoded);
        let rows = arrays
            .runtime_feature_domains
            .values()
            .flatten()
            .collect::<Vec<_>>();
        assert_eq!(rows.len(), 8);
        assert!(rows.iter().all(|row| row.alternatives.len() == 4));
        let pointer = rows[0].alternatives.as_ptr();
        assert!(rows.iter().all(|row| row.alternatives.as_ptr() == pointer));
        let decoded: intersect_model::definitions::schema::RuntimeArraySchemaInputs =
            serde_json::from_slice(&encoded).unwrap();
        assert_eq!(serde_json::to_vec(&decoded).unwrap(), encoded);
        assert_eq!(
            arrays
                .into_owned_observed(&InvocationContext::default())
                .unwrap(),
            owned
        );
        let mut stale = bundle.clone();
        stale.sources[0].text.push(' ');
        let expected = compiler::materialize_runtime_bundle(
            &stale,
            &stale.sources[0].id,
            "urn:cold-shared",
            &types,
            None,
        )
        .unwrap_err();
        assert!(
            matches!(array_inputs(&stale, &types, usize::MAX, usize::MAX).0, Err(ObservedImportError::Import(error)) if error == expected)
        );
    }
}
#[test]
fn cold_array_inputs_keep_exact_storage_cancellation_and_fresh_recovery() {
    let (bundle, types) = fixture(false);
    let (result, needed, checks) = array_inputs(&bundle, &types, usize::MAX, usize::MAX);
    let encoded = serde_json::to_vec(&result.unwrap()).unwrap();
    for limit in [0, needed - 1, needed] {
        let (result, bytes, _) = array_inputs(&bundle, &types, limit, usize::MAX);
        if limit < needed {
            assert!(
                matches!(result, Err(ObservedImportError::Observation(message)) if message == "cold construction storage denied")
            );
        } else {
            assert_eq!(bytes, needed);
            assert_eq!(serde_json::to_vec(&result.unwrap()).unwrap(), encoded);
        }
    }
    for stop in [0, 1, checks / 4, checks / 2, checks * 3 / 4, checks - 1] {
        assert!(
            matches!(array_inputs(&bundle, &types, usize::MAX, stop).0, Err(ObservedImportError::Observation(message)) if message == "cold construction cancelled")
        );
    }
    assert_eq!(
        serde_json::to_vec(&array_inputs(&bundle, &types, needed, usize::MAX).0.unwrap()).unwrap(),
        encoded
    );
}

#[test]
fn cold_array_inputs_recheck_changed_source_and_exclude_a_newly_unrelated_candidate() {
    let (original, _) = fixture(false);
    let original_source = &original.sources[0];
    let changed = DefinitionSource::new(
        &original_source.id.uri,
        SourceKind::Rcore,
        original_source
            .text
            .replace("class C2 extends Root", "class C2"),
    );
    assert_ne!(changed.id, original_source.id);
    let types = (0..4)
        .map(|i| {
            RuntimeType::new(Type::Classifier {
                declaration: DeclarationId {
                    source: changed.id.clone(),
                    path: vec![i],
                },
                arguments: vec![],
                raw: i == 0,
            })
            .unwrap()
        })
        .collect::<Vec<_>>();
    let bundle = DefinitionBundle {
        version: 1,
        sources: vec![changed],
        namespaces: vec![],
    };
    let owned = compiler::materialize_runtime_bundle(
        &bundle,
        &bundle.sources[0].id,
        "urn:cold-shared",
        &types,
        None,
    )
    .unwrap();
    let unrelated = owned.class_key_for_type(&types[3]).unwrap();
    let arrays = array_inputs(&bundle, &types, usize::MAX, usize::MAX)
        .0
        .unwrap();
    assert_eq!(
        serde_json::to_vec(&arrays).unwrap(),
        serde_json::to_vec(&owned).unwrap()
    );
    let rows = arrays
        .runtime_feature_domains
        .values()
        .flatten()
        .collect::<Vec<_>>();
    assert_eq!(rows.len(), 6);
    for row in rows {
        assert_eq!(row.alternatives.len(), 3);
        assert!(row.alternatives.iter().all(|key| key.as_ref() != unrelated));
    }
}

#[test]
fn cold_array_views_keep_complete_owned_output_and_validate_unordered_transport_inputs() {
    let (bundle, types) = fixture(false);
    let owned = compiler::materialize_runtime_bundle(
        &bundle,
        &bundle.sources[0].id,
        "urn:cold-shared",
        &types,
        None,
    )
    .unwrap();
    let views = compiler::materialize_runtime_array_view_inputs_observed(
        &bundle,
        &bundle.sources[0].id,
        "urn:cold-shared",
        &types,
        None,
        &InvocationContext::default(),
    )
    .unwrap();
    let encoded = serde_json::to_value(&owned).unwrap();
    assert_eq!(serde_json::to_value(&views).unwrap(), encoded);
    assert_eq!(
        views
            .into_owned_observed(&InvocationContext::default())
            .unwrap(),
        owned
    );
    let mut transport = encoded;
    for rows in transport["runtime_feature_domains"]
        .as_object_mut()
        .unwrap()
        .values_mut()
    {
        for row in rows.as_array_mut().unwrap() {
            row["alternatives"].as_array_mut().unwrap().reverse();
        }
    }
    let unordered: intersect_model::definitions::schema::RuntimeArrayViewSchemaInputs =
        serde_json::from_value(transport.clone()).unwrap();
    unordered.validate_instantiations().unwrap();
    let first = transport["runtime_feature_domains"]
        .as_object_mut()
        .unwrap()
        .values_mut()
        .next()
        .unwrap();
    let alternatives = first[0]["alternatives"].as_array_mut().unwrap();
    alternatives.push(alternatives[0].clone());
    let duplicate: intersect_model::definitions::schema::RuntimeArrayViewSchemaInputs =
        serde_json::from_value(transport).unwrap();
    assert!(
        duplicate
            .validate_instantiations()
            .unwrap_err()
            .to_string()
            .contains("Duplicate runtime domain alternative")
    );
}

#[test]
fn cold_observed_source_tables_keep_complete_owned_output_and_validate_unordered_transport_inputs()
{
    let (bundle, types) = fixture(false);
    let owned = compiler::materialize_runtime_bundle(
        &bundle,
        &bundle.sources[0].id,
        "urn:cold-shared",
        &types,
        None,
    )
    .unwrap();
    let views = compiler::materialize_runtime_source_array_inputs_observed(
        &bundle,
        &bundle.sources[0].id,
        "urn:cold-shared",
        &types,
        None,
        &InvocationContext::default(),
    )
    .unwrap();
    let encoded = serde_json::to_value(&owned).unwrap();
    assert_eq!(serde_json::to_value(&views).unwrap(), encoded);
    assert_eq!(
        views
            .into_owned_observed(&InvocationContext::default())
            .unwrap(),
        owned
    );
    let mut transport = encoded;
    for rows in transport["runtime_feature_domains"]
        .as_object_mut()
        .unwrap()
        .values_mut()
    {
        for row in rows.as_array_mut().unwrap() {
            row["alternatives"].as_array_mut().unwrap().reverse();
        }
    }
    let unordered: intersect_model::definitions::schema::RuntimeSourceArraySchemaInputs =
        serde_json::from_value(transport.clone()).unwrap();
    unordered.validate_instantiations().unwrap();
    let first = transport["runtime_feature_domains"]
        .as_object_mut()
        .unwrap()
        .values_mut()
        .next()
        .unwrap();
    let alternatives = first[0]["alternatives"].as_array_mut().unwrap();
    alternatives.push(alternatives[0].clone());
    let duplicate: intersect_model::definitions::schema::RuntimeSourceArraySchemaInputs =
        serde_json::from_value(transport).unwrap();
    assert!(
        duplicate
            .validate_instantiations()
            .unwrap_err()
            .to_string()
            .contains("Duplicate runtime domain alternative")
    );
}

#[test]
fn owned_source_holder_preserves_complete_output_and_recovers_original_subset_inputs() {
    for subset in [false, true] {
        let (bundle, types) = fixture(subset);
        let entry = bundle.sources[0].id.clone();
        let borrowed = compiler::materialize_runtime_source_array_inputs_observed(
            &bundle,
            &entry,
            "urn:cold-shared",
            &types,
            None,
            &InvocationContext::default(),
        )
        .unwrap();
        let owned = compiler::materialize_owned_runtime_source_array_inputs_observed(
            bundle.clone(),
            &entry,
            "urn:cold-shared",
            &types,
            None,
            &InvocationContext::default(),
        );
        assert_eq!(owned.bundle, bundle);
        let schema = owned.result.unwrap();
        assert_eq!(
            serde_json::to_value(&schema).unwrap(),
            serde_json::to_value(&borrowed).unwrap()
        );
        schema.validate_instantiations().unwrap();
    }
}

#[test]
fn owned_source_holder_precharges_allocation_and_preserves_inputs_on_denial_and_recovery() {
    let (bundle, types) = fixture(false);
    let entry = bundle.sources[0].id.clone();
    let meter = |limit| {
        Rc::new(Meter {
            bytes: Cell::new(0),
            checks: Cell::new(0),
            work: Cell::new(0),
            limit,
            stop: usize::MAX,
        })
    };
    let borrowed_meter = meter(usize::MAX);
    let borrowed = compiler::materialize_runtime_source_array_inputs_observed(
        &bundle,
        &entry,
        "urn:cold-shared",
        &types,
        None,
        &InvocationContext::new(borrowed_meter.clone()),
    )
    .unwrap();
    let owned_meter = meter(usize::MAX);
    let owned = compiler::materialize_owned_runtime_source_array_inputs_observed(
        bundle.clone(),
        &entry,
        "urn:cold-shared",
        &types,
        None,
        &InvocationContext::new(owned_meter.clone()),
    );
    assert_eq!(owned.bundle, bundle);
    assert_eq!(
        serde_json::to_value(owned.result.unwrap()).unwrap(),
        serde_json::to_value(&borrowed).unwrap()
    );
    assert_eq!(owned_meter.bytes.get(), borrowed_meter.bytes.get() + 2048);
    assert_eq!(owned_meter.work.get(), borrowed_meter.work.get());
    let denied_meter = meter(owned_meter.bytes.get() - 1);
    let denied = compiler::materialize_owned_runtime_source_array_inputs_observed(
        bundle.clone(),
        &entry,
        "urn:cold-shared",
        &types,
        None,
        &InvocationContext::new(denied_meter.clone()),
    );
    assert_eq!(denied.bundle, bundle);
    assert!(matches!(
        denied.result,
        Err(ObservedImportError::Observation(_))
    ));
    assert!(denied_meter.bytes.get() < owned_meter.bytes.get());
    let recovered = compiler::materialize_owned_runtime_source_array_inputs_observed(
        denied.bundle,
        &entry,
        "urn:cold-shared",
        &types,
        None,
        &InvocationContext::default(),
    );
    assert_eq!(recovered.bundle, bundle);
    assert_eq!(
        serde_json::to_value(recovered.result.unwrap()).unwrap(),
        serde_json::to_value(&borrowed).unwrap()
    );
}

#[test]
fn owned_source_holder_returns_exact_inputs_on_cancellation_and_source_errors() {
    let (bundle, types) = fixture(true);
    let entry = bundle.sources[0].id.clone();
    let cancelled = Rc::new(Meter {
        bytes: Cell::new(0),
        checks: Cell::new(0),
        work: Cell::new(0),
        limit: usize::MAX,
        stop: 0,
    });
    let stopped = compiler::materialize_owned_runtime_source_array_inputs_observed(
        bundle.clone(),
        &entry,
        "urn:cold-shared",
        &types,
        None,
        &InvocationContext::new(cancelled),
    );
    assert_eq!(stopped.bundle, bundle);
    assert!(matches!(
        stopped.result,
        Err(ObservedImportError::Observation(_))
    ));
    for (namespace, requested) in [("urn:missing", types.as_slice()), ("urn:cold-shared", &[])] {
        let failed = compiler::materialize_owned_runtime_source_array_inputs_observed(
            bundle.clone(),
            &entry,
            namespace,
            requested,
            None,
            &InvocationContext::default(),
        );
        assert_eq!(failed.bundle, bundle);
        assert!(matches!(failed.result, Err(ObservedImportError::Import(_))));
    }
}

fn owned_session(
    bundle: DefinitionBundle,
    entry: intersect_model::definitions::bundle::SourceId,
    namespace: String,
    types: &[RuntimeType],
) -> compiler::OwnedRuntimeSourceArrayPreparation {
    match compiler::OwnedRuntimeSourceArrayPreparation::begin(
        bundle,
        entry,
        namespace,
        types,
        None,
        &InvocationContext::default(),
    ) {
        Ok(session) => session,
        Err(_) => panic!("valid session initialization failed"),
    }
}
#[test]
fn owned_session_matches_synchronous_output_moves_selection_and_releases_observers() {
    for subset in [false, true] {
        let (bundle, types) = fixture(subset);
        let entry = bundle.sources[0].id.clone();
        let namespace = "urn:cold-shared".to_owned();
        let entry_pointer = entry.uri.as_ptr();
        let namespace_pointer = namespace.as_ptr();
        let expected = compiler::materialize_owned_runtime_source_array_inputs_observed(
            bundle.clone(),
            &entry,
            &namespace,
            &types,
            None,
            &InvocationContext::default(),
        )
        .result
        .unwrap();
        let expected_entry = entry.clone();
        let mut session = owned_session(bundle.clone(), entry, namespace, &types);
        let mut advances = 0;
        loop {
            let meter = Rc::new(Meter {
                bytes: Cell::new(0),
                checks: Cell::new(0),
                work: Cell::new(0),
                limit: usize::MAX,
                stop: usize::MAX,
            });
            let weak = Rc::downgrade(&meter);
            let done = {
                let operation = InvocationContext::new(meter);
                session.advance(None, &operation).unwrap()
            };
            assert!(weak.upgrade().is_none());
            advances += 1;
            let mut moved = Some(session);
            session = moved.take().unwrap();
            if done {
                break;
            }
        }
        assert!(advances > types.len());
        assert!(
            session
                .advance(None, &InvocationContext::default())
                .unwrap()
        );
        let outcome = session.finish();
        assert_eq!(outcome.materialization.bundle, bundle);
        assert_eq!(outcome.entry_source, expected_entry);
        // The namespace is transferred unchanged, without a retained string copy.
        assert_eq!(outcome.namespace.as_ptr(), namespace_pointer);
        assert_eq!(outcome.entry_source.uri.as_ptr(), entry_pointer);
        assert_eq!(
            serde_json::to_value(outcome.materialization.result.unwrap()).unwrap(),
            serde_json::to_value(expected).unwrap()
        );
    }
}
#[test]
fn owned_session_cancels_every_advance_and_recovers_exact_inputs_without_partial_publication() {
    let (bundle, types) = fixture(true);
    let entry = bundle.sources[0].id.clone();
    let mut baseline = owned_session(
        bundle.clone(),
        entry.clone(),
        "urn:cold-shared".into(),
        &types,
    );
    let mut advances = 0;
    loop {
        advances += 1;
        if baseline
            .advance(None, &InvocationContext::default())
            .unwrap()
        {
            break;
        }
    }
    let expected = serde_json::to_value(baseline.finish().materialization.result.unwrap()).unwrap();
    for prefix in 0..=advances {
        let mut session = owned_session(
            bundle.clone(),
            entry.clone(),
            "urn:cold-shared".into(),
            &types,
        );
        for _ in 0..prefix {
            session
                .advance(None, &InvocationContext::default())
                .unwrap();
        }
        let meter = Rc::new(Meter {
            bytes: Cell::new(0),
            checks: Cell::new(0),
            work: Cell::new(0),
            limit: usize::MAX,
            stop: 0,
        });
        let error = session
            .advance(None, &InvocationContext::new(meter))
            .unwrap_err();
        assert!(matches!(error, ObservedImportError::Observation(_)));
        assert_eq!(
            session
                .advance(None, &InvocationContext::default())
                .unwrap_err(),
            error
        );
        let outcome = session.finish();
        assert_eq!(outcome.entry_source, entry);
        assert_eq!(outcome.namespace, "urn:cold-shared");
        assert_eq!(outcome.materialization.bundle, bundle);
        assert_eq!(outcome.materialization.result.unwrap_err(), error);
        let mut fresh = owned_session(
            outcome.materialization.bundle,
            entry.clone(),
            outcome.namespace,
            &types,
        );
        while !fresh.advance(None, &InvocationContext::default()).unwrap() {}
        assert_eq!(
            serde_json::to_value(fresh.finish().materialization.result.unwrap()).unwrap(),
            expected
        );
    }
}
#[test]
fn owned_session_recovers_selection_on_initialization_failure_and_incomplete_finish() {
    let (bundle, types) = fixture(true);
    let entry = bundle.sources[0].id.clone();
    for namespace in ["urn:missing", "urn:cold-shared"] {
        let outcome = match compiler::OwnedRuntimeSourceArrayPreparation::begin(
            bundle.clone(),
            entry.clone(),
            namespace.into(),
            &types,
            None,
            &InvocationContext::default(),
        ) {
            Ok(session) => session.finish(),
            Err(outcome) => *outcome,
        };
        assert_eq!(outcome.materialization.bundle, bundle);
        assert_eq!(outcome.entry_source, entry);
        assert_eq!(outcome.namespace, namespace);
        assert!(matches!(
            outcome.materialization.result,
            Err(ObservedImportError::Import(_))
        ));
    }
    let meter = Rc::new(Meter {
        bytes: Cell::new(0),
        checks: Cell::new(0),
        work: Cell::new(0),
        limit: usize::MAX,
        stop: 0,
    });
    let outcome = match compiler::OwnedRuntimeSourceArrayPreparation::begin(
        bundle.clone(),
        entry.clone(),
        "urn:cold-shared".into(),
        &types,
        None,
        &InvocationContext::new(meter),
    ) {
        Ok(_) => panic!("cancelled initialization succeeded"),
        Err(outcome) => outcome,
    };
    assert_eq!(outcome.materialization.bundle, bundle);
    assert_eq!(outcome.entry_source, entry);
    assert!(matches!(
        outcome.materialization.result,
        Err(ObservedImportError::Observation(_))
    ));
}

fn membership_session(
    bundle: DefinitionBundle,
    types: &[RuntimeType],
    observed: bool,
    operation: &InvocationContext<'_>,
) -> compiler::OwnedRuntimeSourceArrayPreparation {
    let entry = bundle.sources[0].id.clone();
    let result = if observed {
        compiler::OwnedRuntimeSourceArrayPreparation::begin_with_membership_observation(
            bundle,
            entry,
            "urn:cold-shared".into(),
            types,
            None,
            operation,
        )
    } else {
        compiler::OwnedRuntimeSourceArrayPreparation::begin(
            bundle,
            entry,
            "urn:cold-shared".into(),
            types,
            None,
            operation,
        )
    };
    result.unwrap_or_else(|failure| {
        panic!(
            "Session initialization failed: {:?}",
            failure.materialization.result
        )
    })
}
#[test]
fn observed_membership_session_keeps_exact_schema_and_observes_more_real_work() {
    let (bundle, types) = fixture(true);
    let mut results = Vec::new();
    for observed in [false, true] {
        let mut session = membership_session(
            bundle.clone(),
            &types,
            observed,
            &InvocationContext::default(),
        );
        let mut work = 0;
        let mut bytes = 0;
        let mut advances = 0;
        loop {
            let meter = Rc::new(Meter {
                bytes: Cell::new(0),
                checks: Cell::new(0),
                work: Cell::new(0),
                limit: usize::MAX,
                stop: usize::MAX,
            });
            let weak = Rc::downgrade(&meter);
            let done = {
                let operation = InvocationContext::new(meter.clone());
                session.advance(None, &operation).unwrap()
            };
            work += meter.work.get();
            bytes += meter.bytes.get();
            drop(meter);
            assert!(weak.upgrade().is_none());
            advances += 1;
            let mut moved = Some(session);
            session = moved.take().unwrap();
            if done {
                break;
            }
        }
        let output = session.finish();
        assert_eq!(output.materialization.bundle, bundle);
        results.push((
            serde_json::to_value(output.materialization.result.unwrap()).unwrap(),
            work,
            bytes,
            advances,
        ));
    }
    assert_eq!(results[0].0, results[1].0);
    assert_eq!(results[0].3, results[1].3);
    assert!(results[1].1 > results[0].1);
    assert!(results[1].2 > results[0].2);
}
#[test]
fn observed_membership_session_cancels_inside_real_candidate_work_and_recovers() {
    let (bundle, types) = fixture(true);
    // Find an advance with substantially more observer checkpoints than the
    // compatibility route. This identifies fresh semantic work, not a dummy yield.
    let mut old = membership_session(bundle.clone(), &types, false, &InvocationContext::default());
    let mut observed =
        membership_session(bundle.clone(), &types, true, &InvocationContext::default());
    let mut prefix = 0;
    let stop = loop {
        let mut checkpoints = Vec::new();
        for session in [&mut old, &mut observed] {
            let meter = Rc::new(Meter {
                bytes: Cell::new(0),
                checks: Cell::new(0),
                work: Cell::new(0),
                limit: usize::MAX,
                stop: usize::MAX,
            });
            let done = session
                .advance(None, &InvocationContext::new(meter.clone()))
                .unwrap();
            assert!(!done, "No observed candidate checkpoint was found");
            checkpoints.push(meter.checks.get());
        }
        if checkpoints[1] > checkpoints[0] + 4 {
            break checkpoints[0] + 2;
        }
        prefix += 1;
    };
    let mut cancelled =
        membership_session(bundle.clone(), &types, true, &InvocationContext::default());
    for _ in 0..prefix {
        cancelled
            .advance(None, &InvocationContext::default())
            .unwrap();
    }
    let meter = Rc::new(Meter {
        bytes: Cell::new(0),
        checks: Cell::new(0),
        work: Cell::new(0),
        limit: usize::MAX,
        stop,
    });
    let error = cancelled
        .advance(None, &InvocationContext::new(meter))
        .unwrap_err();
    assert!(matches!(error, ObservedImportError::Observation(_)));
    assert_eq!(
        cancelled
            .advance(None, &InvocationContext::default())
            .unwrap_err(),
        error
    );
    let output = cancelled.finish();
    assert_eq!(output.materialization.bundle, bundle);
    assert_eq!(output.materialization.result.unwrap_err(), error);
    let mut fresh = membership_session(bundle.clone(), &types, true, &InvocationContext::default());
    while !fresh.advance(None, &InvocationContext::default()).unwrap() {}
    let result = fresh.finish();
    assert_eq!(result.materialization.bundle, bundle);
    result
        .materialization
        .result
        .unwrap()
        .validate_instantiations()
        .unwrap();
}

#[test]
fn buffered_revision_session_preserves_complete_output_and_each_advance_cancellation() {
    let (bundle, types) = fixture(true);
    let begin = || {
        compiler::OwnedRuntimeSourceArrayPreparation::begin_with_buffered_revision(
            bundle.clone(),
            bundle.sources[0].id.clone(),
            "urn:cold-shared".into(),
            &types,
            None,
            &InvocationContext::default(),
        )
        .unwrap_or_else(|failure| panic!("{:?}", failure.materialization.result))
    };
    let run = |mut session: compiler::OwnedRuntimeSourceArrayPreparation| {
        let meter = Rc::new(Meter {
            bytes: Cell::new(0),
            checks: Cell::new(0),
            work: Cell::new(0),
            limit: usize::MAX,
            stop: usize::MAX,
        });
        let operation = InvocationContext::new(meter.clone());
        let mut advances = 0;
        while !session.advance(None, &operation).unwrap() {
            advances += 1;
        }
        let output = session.finish();
        assert_eq!(output.materialization.bundle, bundle);
        (
            serde_json::to_value(output.materialization.result.unwrap()).unwrap(),
            meter.work.get(),
            advances,
        )
    };
    let baseline = run(membership_session(
        bundle.clone(),
        &types,
        true,
        &InvocationContext::default(),
    ));
    let buffered = run(begin());
    assert_eq!(buffered.0, baseline.0);
    assert_eq!(buffered.2, baseline.2);
    assert!(buffered.1 < baseline.1);
    for prefix in 0..=buffered.2 {
        let mut session = begin();
        for _ in 0..prefix {
            assert!(
                !session
                    .advance(None, &InvocationContext::default())
                    .unwrap()
            );
        }
        let meter = Rc::new(Meter {
            bytes: Cell::new(0),
            checks: Cell::new(0),
            work: Cell::new(0),
            limit: usize::MAX,
            stop: 0,
        });
        let operation = InvocationContext::new(meter.clone());
        assert!(matches!(
            session.advance(None, &operation),
            Err(ObservedImportError::Observation(_))
        ));
        assert!(
            session
                .advance(None, &InvocationContext::default())
                .is_err()
        );
        let output = session.finish();
        assert_eq!(output.materialization.bundle, bundle);
        assert!(output.materialization.result.is_err());
        assert_eq!(run(begin()).0, baseline.0);
    }
}

#[test]
#[cfg_attr(not(feature = "milestone-tests"), ignore = "milestone")]
fn incremental_revision_session_keeps_full_schema_private_until_complete_and_recovers_each_advance()
{
    let (bundle, types) = fixture(true);
    let begin = || {
        compiler::OwnedRuntimeSourceArrayPreparation::begin_with_incremental_revision(
            bundle.clone(),
            bundle.sources[0].id.clone(),
            "urn:cold-shared".into(),
            &types,
            None,
            &InvocationContext::default(),
        )
        .unwrap_or_else(|e| panic!("{:?}", e.materialization.result))
    };
    let run = |mut session: compiler::OwnedRuntimeSourceArrayPreparation| {
        let mut advances = 0;
        while !session
            .advance(None, &InvocationContext::default())
            .unwrap()
        {
            advances += 1;
        }
        let output = session.finish();
        assert_eq!(output.materialization.bundle, bundle);
        (
            serde_json::to_value(output.materialization.result.unwrap()).unwrap(),
            advances,
        )
    };
    let old = run(membership_session(
        bundle.clone(),
        &types,
        true,
        &InvocationContext::default(),
    ));
    let new = run(begin());
    assert_eq!(new.0, old.0);
    assert!(new.1 > old.1);
    for prefix in 0..=new.1 {
        let mut incomplete = begin();
        for _ in 0..prefix {
            assert!(
                !incomplete
                    .advance(None, &InvocationContext::default())
                    .unwrap()
            );
        }
        let output = incomplete.finish();
        assert_eq!(output.materialization.bundle, bundle);
        assert!(output.materialization.result.is_err());
        let mut pending = begin();
        for _ in 0..prefix {
            assert!(
                !pending
                    .advance(None, &InvocationContext::default())
                    .unwrap()
            );
        }
        let meter = Rc::new(Meter {
            bytes: Cell::new(0),
            checks: Cell::new(0),
            work: Cell::new(0),
            limit: usize::MAX,
            stop: 0,
        });
        assert!(
            pending
                .advance(None, &InvocationContext::new(meter))
                .is_err()
        );
        assert!(
            pending
                .advance(None, &InvocationContext::default())
                .is_err()
        );
        let output = pending.finish();
        assert_eq!(output.materialization.bundle, bundle);
        assert!(output.materialization.result.is_err());
        assert_eq!(run(begin()).0, old.0);
    }
}
