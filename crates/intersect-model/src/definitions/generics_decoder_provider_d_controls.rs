//! NewD provider controls over actual production Owned public consumers.
use super::*;
use crate::definitions::generics_decoder_new_contract_capture::{Capture, positive_events};
fn drive_d(
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
            OwnedRuntimeSourceArrayPreparation::begin_with_retained_generic_source_decoding
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
        job.preparation.generic_agreement_polling
            == GenericAgreementPolling::RetainedGenericSourceDecoding,
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
                if job.preparation.holder.with_dependent(|_, state| matches!(state, MaterializationState::Ready(state) if state.source_inventory.as_ref().is_some_and(|preparation|preparation.is_source_decoding_step()) || state.reports().is_some_and(|preparation|preparation.is_source_decoding_step()))) {
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
fn retained_source_decoding_provider_first_full_transcripts_all_added_poll_controls() {
    let (old, _) = drive_c(true, 1, Rc::new(Meter::default()), None, None);
    let (full, added) = drive_d(true, 1, Rc::new(Meter::default()), None, None);
    assert!(!added.is_empty());
    for (key, value) in old.as_object().unwrap() {
        if !["events", "polls"].contains(&key.as_str()) {
            assert_eq!(&full[key], value, "{key}");
        }
    }
    assert_eq!(positive_events(&full), positive_events(&old));
    assert_eq!(
        full["polls"].as_u64().unwrap(),
        old["polls"].as_u64().unwrap() + added.len() as u64,
        "all additional polls are actual retained source/ID conversion states"
    );
    let mut main = Capture::new("provider-d-main");
    let mut controls = Capture::new("provider-d-controls");
    for quantum in [1, 3, 256] {
        let (row, boundaries) = drive_d(true, quantum, Rc::new(Meter::default()), None, None);
        assert_eq!(row, full, "q{quantum}");
        assert_eq!(boundaries, added);
        main.row(&serde_json::json!({"kind":"main","quantum":quantum,"complete_observation":row,"source_boundaries":boundaries}));
    }
    let events = full["events"].as_array().unwrap();
    for (poll, start) in &added {
        assert_eq!(events[*start], serde_json::json!(["c", 0]));
        assert_eq!(events[*start + 1], serde_json::json!(["w", 0]));
        assert!(
            events[*start + 2] == serde_json::json!(["w", 0])
                || events[*start + 2] == serde_json::json!(["c", 0])
        );
        // Current registry comparison precedes this cursor's advancement, even
        // on macrosteps whose nonzero policy fee was already admitted earlier.
        let (row, _) = drive_d(true, 1, Rc::new(Meter::default()), Some(*poll), None);
        assert_eq!(row["events"], serde_json::json!(&events[..*start + 2]));
        controls.row(&serde_json::json!({"kind":"stale_every_source_poll","source_poll":poll,"complete_observation":row}));
        for drop_at in [poll - 1, *poll] {
            let (row, _) = drive_d(true, 1, Rc::new(Meter::default()), None, Some(drop_at));
            controls.row(&serde_json::json!({"kind":"drop_every_source_poll","source_poll":poll,"drop_at":drop_at,"complete_observation":row}));
        }
        for offset in 1..=3 {
            let meter = Rc::new(Meter {
                deny: Some(start + offset),
                ..Meter::default()
            });
            let (row, _) = drive_d(true, 1, meter, None, None);
            assert_eq!(row["events"], serde_json::json!(&events[..start + offset]));
            controls.row(&serde_json::json!({"kind":"deny_each_added_control","source_poll":poll,"callback":start+offset,"complete_observation":row}));
        }
    }
    let work = full["admitted_work"].as_u64().unwrap() as usize;
    let storage = full["admitted_storage"].as_u64().unwrap() as usize;
    for quantum in [1, 3, 256] {
        for (max_work, max_storage) in [
            (Some(work), Some(storage)),
            (Some(work.saturating_sub(1)), Some(storage)),
            (Some(work), Some(storage.saturating_sub(1))),
        ] {
            let meter = Rc::new(Meter {
                max_work,
                max_storage,
                ..Meter::default()
            });
            let (row, _) = drive_d(true, quantum, meter, None, None);
            controls.row(
                &serde_json::json!({"kind":"budget","quantum":quantum,"max_work":max_work,"max_storage":max_storage,"complete_observation":row}),
            );
        }
    }
    assert_eq!(
        drive_d(true, 1, Rc::new(Meter::default()), None, None).0,
        full,
        "fresh retry after every failed/dropped attempt"
    );
    main.finish();
    controls.finish();
}
