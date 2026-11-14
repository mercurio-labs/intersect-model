//! Frozen original A/current B inventory provider macrosteps.
use super::*;
fn inventory_before_drive(
    retained: bool,
    quantum: usize,
    meter: Rc<Meter>,
    stale_at: Option<usize>,
    drop_at: Option<usize>,
) -> (Value, Vec<(usize, usize)>) {
    let fixture = Fixture::new();
    let bundle = fixture.bundle.clone();
    let before = serde_json::to_value(&bundle).unwrap();
    let pointer = bundle.sources[0].text.as_ptr();
    let entry = bundle.sources[0].id.clone();
    let lease = Rc::new(Lease(meter.clone()));
    let weak = Rc::downgrade(&lease);
    let mut job = {
        let context = InvocationContext::new(lease.clone());
        let registry = fixture.registry();
        let begin = if retained {
            OwnedRuntimeSourceArrayPreparation::begin_with_retained_generic_agreement
        } else {
            OwnedRuntimeSourceArrayPreparation::begin_with_retained_canonical_sources
        };
        begin(
            bundle,
            entry,
            "urn:legacy-freeze".into(),
            &fixture.requested,
            Some(&registry),
            &context,
        )
        .unwrap_or_else(|_| panic!("fixture initialization failed"))
    };
    drop(lease);
    assert!(weak.upgrade().is_none());
    assert_eq!(
        job.preparation.generic_agreement_polling == GenericAgreementPolling::Retained,
        retained
    );
    let mut polls = 0;
    let mut quiet = vec![];
    let mut error = None;
    'outer: loop {
        job = std::hint::black_box(job);
        let lease = Rc::new(Lease(meter.clone()));
        let weak = Rc::downgrade(&lease);
        let mut done = false;
        {
            let context = InvocationContext::new(lease.clone());
            for _ in 0..quantum {
                if drop_at == Some(polls) {
                    done = true;
                    break;
                }
                if job.preparation.holder.with_dependent(|_, state| matches!(state, MaterializationState::Ready(state) if state.source_inventory.is_some())) {
                    quiet.push((polls + 1, meter.events.borrow().len()))
                }
                polls += 1;
                assert!(polls < 100_000);
                if stale_at == Some(polls) {
                    fixture.provider.stale.set(true)
                }
                let registry = fixture.registry();
                match job.advance(Some(&registry), &context) {
                    Ok(true) => {
                        done = true;
                        break;
                    }
                    Ok(false) => {}
                    Err(failure) => {
                        error = Some(failure.to_string());
                        done = true;
                        break;
                    }
                }
            }
        }
        drop(lease);
        assert!(weak.upgrade().is_none(), "public job retained a lease");
        if done {
            break 'outer;
        }
    }
    if error.is_some() {
        fixture.provider.stale.set(false);
        let registry = fixture.registry();
        let quiet_meter = Rc::new(Meter::default());
        let lease = Rc::new(Lease(quiet_meter.clone()));
        let context = InvocationContext::new(lease);
        assert!(job.advance(Some(&registry), &context).is_err());
        assert!(
            quiet_meter.events.borrow().is_empty(),
            "failed attempt repeated observations"
        );
    }
    let events_before = meter.events.borrow().len();
    let result = job.finish();
    assert_eq!(meter.events.borrow().len(), events_before);
    assert_eq!(
        result.materialization.bundle.sources[0].text.as_ptr(),
        pointer
    );
    assert_eq!(
        serde_json::to_value(&result.materialization.bundle).unwrap(),
        before
    );
    if error.is_some() || drop_at.is_some() {
        assert!(
            result.materialization.result.is_err(),
            "partial schema published"
        )
    }
    let output = match result.materialization.result {
        Ok(schema) => serde_json::json!({"ok":schema}),
        Err(error) => serde_json::json!({"error":format!("{error:?}")}),
    };
    (
        serde_json::json!({"fixture":"provider-bearing original public A","source":before,"output":output,"entry_source":result.entry_source,"namespace":result.namespace,"events":*meter.events.borrow(),"admitted_work":meter.work.get(),"admitted_storage":meter.storage.get(),"polls":polls,"exact_input_buffer_recovered":true}),
        quiet,
    )
}

#[test]
fn source_inventory_original_provider_complete_observations() {
    let fixture: Value = serde_json::from_str(include_str!(
        "generic_source_inventory_complete_digests.json"
    ))
    .unwrap();
    let mut hash = Sha256::new();
    let mut count = 0usize;
    let mut emit = |kind: &str,
                    retained: bool,
                    q: usize,
                    control: Value,
                    row: Value,
                    steps: Vec<(usize, usize)>| {
        hash.update(serde_json::to_vec(&serde_json::json!({"kind":kind,"retained_current_b":retained,"quantum":q,"control":control,"complete_observation":row,"inventory_macrosteps":steps})).unwrap());
        hash.update(b"\n");
        count += 1;
    };
    for retained in [false, true] {
        let (full, steps) =
            inventory_before_drive(retained, 1, Rc::new(Meter::default()), None, None);
        let literal: Value = serde_json::from_str(if retained {
            include_str!("generic_legacy_retained_provider_digest.json")
        } else {
            include_str!("generic_legacy_provider_original_digest.json")
        })
        .unwrap();
        assert_eq!(
            format!("{:x}", Sha256::digest(serde_json::to_vec(&full).unwrap())),
            literal["complete_canonical_sha256"].as_str().unwrap()
        );
        for q in [1, 3, 256] {
            let (row, steps) =
                inventory_before_drive(retained, q, Rc::new(Meter::default()), None, None);
            assert_eq!(row, full);
            emit("full", retained, q, serde_json::json!({}), row, steps)
        }
        for position in [0, steps.len() / 2, steps.len() - 1] {
            let (poll, start) = steps[position];
            let end = steps.get(position + 1).map_or(start + 3, |s| s.1);
            for deny in [start + 1, start + (end - start).max(1) / 2, end] {
                let (row, steps) = inventory_before_drive(
                    retained,
                    1,
                    Rc::new(Meter {
                        deny: Some(deny),
                        ..Meter::default()
                    }),
                    None,
                    None,
                );
                assert_eq!(
                    row["events"],
                    serde_json::json!(&full["events"].as_array().unwrap()[..deny])
                );
                emit(
                    "deny",
                    retained,
                    1,
                    serde_json::json!({"callback":deny,"inventory_poll":poll}),
                    row,
                    steps,
                )
            }
            let (row, steps) =
                inventory_before_drive(retained, 1, Rc::new(Meter::default()), Some(poll), None);
            emit(
                "stale",
                retained,
                1,
                serde_json::json!({"inventory_poll":poll}),
                row,
                steps,
            );
            let (row, steps) =
                inventory_before_drive(retained, 1, Rc::new(Meter::default()), None, Some(poll));
            emit(
                "drop",
                retained,
                1,
                serde_json::json!({"inventory_poll":poll}),
                row,
                steps,
            );
        }
        for q in [1, 3, 256] {
            for (work, storage) in [(20807, 2272343), (20806, 2272343), (20807, 2272342)] {
                let (row, steps) = inventory_before_drive(
                    retained,
                    q,
                    Rc::new(Meter {
                        max_work: Some(work),
                        max_storage: Some(storage),
                        ..Meter::default()
                    }),
                    None,
                    None,
                );
                emit(
                    "budget",
                    retained,
                    q,
                    serde_json::json!({"work":work,"storage":storage}),
                    row,
                    steps,
                )
            }
        }
    }
    assert_eq!(
        count as u64,
        fixture["original_public_provider"]["rows"]
            .as_u64()
            .unwrap()
    );
    assert_eq!(
        format!("{:x}", hash.finalize()),
        fixture["original_public_provider"]["complete_jsonl_sha256"]
            .as_str()
            .unwrap()
    );
}
