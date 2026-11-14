use super::*;

fn retained(
    bundle: DefinitionBundle,
    entry: SourceId,
    types: &[RuntimeType],
    operation: &InvocationContext<'_>,
) -> OwnedRuntimeSourceArrayPreparation {
    OwnedRuntimeSourceArrayPreparation::begin_with_retained_canonical_sources(
        bundle,
        entry,
        "urn:loading:a".into(),
        types,
        None,
        operation,
    )
    .unwrap_or_else(|_| panic!("fixed admission failed"))
}
fn still_scanning(session: &OwnedRuntimeSourceArrayPreparation) -> bool {
    session.preparation.holder.with_dependent(|_,state|matches!(state,MaterializationState::Loading(loading) if matches!(loading.phase,LoadingPhase::Canonical(_)) && loading.resources.is_none()))
}
#[test]
fn canonical_scan_visits_one_source_per_advance_before_resource_admission() {
    let (mut bundle, entry, types) = fixture();
    for index in 0..64 {
        bundle.sources.push(DefinitionSource::new(
            format!("unused:{index}"),
            SourceKind::Xtext,
            "not a consumed grammar",
        ));
    }
    let pointer = bundle.sources.as_ptr();
    let count = bundle.sources.len();
    let mut session = retained(bundle, entry, &types, &InvocationContext::default());
    for _ in 0..count {
        assert!(still_scanning(&session));
        assert!(session.root.is_none() && session.schema.is_none());
        assert!(
            !session
                .advance(None, &InvocationContext::default())
                .unwrap()
        );
        assert!(still_scanning(&session));
    }
    assert!(
        !session
            .advance(None, &InvocationContext::default())
            .unwrap()
    );
    assert!(!still_scanning(&session));
    let failed = session.finish();
    assert!(failed.materialization.result.is_err());
    assert_eq!(failed.materialization.bundle.sources.as_ptr(), pointer);
}
#[test]
fn canonical_errors_remain_exact_and_precede_invalid_bundle_source_and_selection() {
    let (bundle, entry, types) = fixture();
    for position in 0..=bundle.sources.len() {
        let mut bad = bundle.clone();
        bad.version = 0;
        bad.sources[0].id.sha256 = "forged".into();
        let mut first = DefinitionSource::new("ecore:first", SourceKind::Ecore, "<not-validated/>");
        first.id.sha256 = "also forged".into();
        bad.sources.insert(position, first.clone());
        bad.sources.push(DefinitionSource::new(
            "ecore:second",
            SourceKind::Ecore,
            "later",
        ));
        let expected = ImportError::new(
            ErrorKind::Unsupported,
            &SourceSpan {
                source: first.id,
                start: 0,
                end: 16,
            },
            "Ecore sources are migration inputs; migrate the complete bundle before compiling structure",
        );
        let legacy = OwnedRuntimeSourceArrayPreparation::begin_with_compiled_signature_inputs(
            bad.clone(),
            entry.clone(),
            "urn:missing".into(),
            &types,
            None,
            &InvocationContext::default(),
        );
        assert_eq!(
            legacy.err().unwrap().materialization.result.unwrap_err(),
            ObservedImportError::Import(expected.clone())
        );
        let pointer = bad.sources[position].text.as_ptr();
        let mut session = retained(
            bad.clone(),
            entry.clone(),
            &types,
            &InvocationContext::default(),
        );
        for _ in 0..position {
            assert!(
                !session
                    .advance(None, &InvocationContext::default())
                    .unwrap()
            );
        }
        let error = ObservedImportError::Import(expected);
        assert_eq!(
            session.advance(None, &InvocationContext::default()),
            Err(error.clone())
        );
        assert_eq!(
            session.advance(None, &InvocationContext::default()),
            Err(error.clone())
        );
        let result = session.finish();
        assert_eq!(result.materialization.bundle, bad);
        assert_eq!(result.materialization.result.unwrap_err(), error);
        // A separately moved invalid bundle recovers its original source allocation.
        let mut moved = retained(bad, entry.clone(), &types, &InvocationContext::default());
        for _ in 0..=position {
            let _ = moved.advance(None, &InvocationContext::default());
        }
        assert_eq!(
            moved.finish().materialization.bundle.sources[position]
                .text
                .as_ptr(),
            pointer
        );
    }
}
#[test]
fn canonical_traversal_preserves_complete_schema_fees_observers_and_boundary_recovery() {
    let (bundle, entry, types) = fixture();
    let old_meter = Rc::new(Meter::default());
    let old_operation = InvocationContext::new(old_meter.clone());
    let legacy = finish_session(
        begin_session(bundle.clone(), entry.clone(), &types, None, &old_operation),
        None,
        &old_operation,
    );
    let meter = Rc::new(Meter::default());
    let operation = InvocationContext::new(meter.clone());
    let current = finish_session(
        retained(bundle.clone(), entry.clone(), &types, &operation),
        None,
        &operation,
    );
    assert_eq!(
        serde_json::to_value(current.materialization.result.as_ref().unwrap()).unwrap(),
        serde_json::to_value(legacy.materialization.result.as_ref().unwrap()).unwrap()
    );
    assert_eq!(
        (meter.work.get(), meter.bytes.get()),
        (old_meter.work.get(), old_meter.bytes.get())
    );
    assert_eq!(current.materialization.bundle, bundle);
    for boundary in 0..=bundle.sources.len() {
        let moved = bundle.clone();
        let pointer = moved.sources[0].text.as_ptr();
        let mut session = retained(moved, entry.clone(), &types, &InvocationContext::default());
        for _ in 0..boundary {
            assert!(
                !session
                    .advance(None, &InvocationContext::default())
                    .unwrap()
            );
        }
        let control = Rc::new(Meter {
            cancelled: Cell::new(true),
            ..Meter::default()
        });
        let cancelled = InvocationContext::new(control.clone());
        let error = session.advance(None, &cancelled).unwrap_err();
        drop(cancelled);
        assert_eq!(Rc::strong_count(&control), 1);
        assert_eq!(
            session.advance(None, &InvocationContext::default()),
            Err(error.clone())
        );
        let result = session.finish();
        assert_eq!(result.materialization.bundle, bundle);
        assert_eq!(
            result.materialization.bundle.sources[0].text.as_ptr(),
            pointer
        );
        assert_eq!(result.materialization.result.unwrap_err(), error);
    }
    let fresh = finish_session(
        retained(bundle, entry, &types, &InvocationContext::default()),
        None,
        &InvocationContext::default(),
    );
    assert_eq!(
        serde_json::to_value(fresh.materialization.result.as_ref().unwrap()).unwrap(),
        serde_json::to_value(current.materialization.result.as_ref().unwrap()).unwrap()
    );
}
