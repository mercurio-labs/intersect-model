//! Public C provider-bearing full observation and current-registry controls.
use super::*;
fn drive_c(
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
            OwnedRuntimeSourceArrayPreparation::begin_with_retained_source_inventory_agreement
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
            == GenericAgreementPolling::RetainedSourceInventory,
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
                if job.preparation.holder.with_dependent(|_, state| matches!(state, MaterializationState::Ready(state) if state.source_inventory.as_ref().is_some_and(|preparation|preparation.is_retained_agreement_step()))) {
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
fn retained_source_inventory_provider_full_contract() {
    let fixture: Value = serde_json::from_str(include_str!(
        "generic_source_inventory_complete_digests.json"
    ))
    .unwrap();
    let mut hash = Sha256::new();
    let mut count = 0usize;
    let mut emit = |kind: &str,
                    poll: Option<usize>,
                    callback: Option<usize>,
                    quantum: usize,
                    row: &Value| {
        let value = serde_json::json!({"kind":kind,"inventory_poll":poll,"callback":callback,"quantum":quantum,"complete_observation":row});
        hash.update(serde_json::to_vec(&value).unwrap());
        hash.update(b"\n");
        count += 1;
    };
    let (b, _) = drive(true, 1, Rc::new(Meter::default()), None, None);
    let expected: Value =
        serde_json::from_str(include_str!("generic_legacy_retained_provider_digest.json")).unwrap();
    assert_eq!(
        format!("{:x}", Sha256::digest(serde_json::to_vec(&b).unwrap())),
        expected["complete_canonical_sha256"].as_str().unwrap()
    );
    let (c, added) = drive_c(true, 1, Rc::new(Meter::default()), None, None);
    assert!(!added.is_empty());
    for (key, value) in b.as_object().unwrap() {
        if !["events", "polls"].contains(&key.as_str()) {
            assert_eq!(&c[key], value, "{key}");
        }
    }
    let mut removed = std::collections::BTreeSet::new();
    for (_, start) in &added {
        assert_eq!(
            &c["events"].as_array().unwrap()[*start..*start + 3],
            &[
                serde_json::json!(["c", 0]),
                serde_json::json!(["w", 0]),
                serde_json::json!(["w", 0])
            ]
        );
        for offset in 0..3 {
            assert!(removed.insert(start + offset));
        }
    }
    assert_eq!(
        serde_json::json!(
            c["events"]
                .as_array()
                .unwrap()
                .iter()
                .enumerate()
                .filter(|(i, _)| !removed.contains(i))
                .map(|(_, event)| event.clone())
                .collect::<Vec<_>>()
        ),
        b["events"]
    );
    assert_eq!(
        c["polls"].as_u64().unwrap(),
        b["polls"].as_u64().unwrap() + added.len() as u64
    );
    assert_eq!(
        format!("{:x}", Sha256::digest(serde_json::to_vec(&c).unwrap())),
        fixture["provider_c_main_sha256"].as_str().unwrap()
    );
    assert_eq!(added.len(), 70);
    for q in [1, 3, 256] {
        assert_eq!(drive_c(true, q, Rc::new(Meter::default()), None, None).0, c);
    }
    for (poll, start) in &added {
        let (row, _) = drive_c(true, 1, Rc::new(Meter::default()), Some(*poll), None);
        assert!(
            row["output"]["error"]
                .as_str()
                .unwrap()
                .contains("Compilation provider selections changed")
        );
        emit("stale", Some(*poll), None, 1, &row);
        for boundary in [poll - 1, *poll] {
            let (row, _) = drive_c(true, 1, Rc::new(Meter::default()), None, Some(boundary));
            assert!(row["output"].get("error").is_some());
            emit("drop", Some(boundary), None, 1, &row);
        }
        for delta in 1..=3 {
            let deny = start + delta;
            let (row, _) = drive_c(
                true,
                1,
                Rc::new(Meter {
                    deny: Some(deny),
                    ..Meter::default()
                }),
                None,
                None,
            );
            assert!(
                row["output"]["error"]
                    .as_str()
                    .unwrap()
                    .contains("retained generic agreement denied")
            );
            assert_eq!(
                row["events"],
                serde_json::json!(&c["events"].as_array().unwrap()[..deny])
            );
            emit("deny", Some(*poll), Some(deny), 1, &row);
        }
    }
    for q in [1, 3, 256] {
        for (work, storage) in [(20807, 2272343), (20806, 2272343), (20807, 2272342)] {
            let (row, _) = drive_c(
                true,
                q,
                Rc::new(Meter {
                    max_work: Some(work),
                    max_storage: Some(storage),
                    ..Meter::default()
                }),
                None,
                None,
            );
            if work == 20807 && storage == 2272343 {
                assert_eq!(row, c)
            } else {
                assert!(row["output"].get("error").is_some())
            }
            emit("budget", None, None, q, &row);
        }
    }
    assert_eq!(drive_c(true, 1, Rc::new(Meter::default()), None, None).0, c);
    assert_eq!(
        count as u64,
        fixture["provider_c_controls"]["rows"].as_u64().unwrap()
    );
    assert_eq!(
        format!("{:x}", hash.finalize()),
        fixture["provider_c_controls"]["complete_jsonl_sha256"]
            .as_str()
            .unwrap()
    );
}

#[path = "generics_decoder_provider_d_controls.rs"]
mod source_decoding_d_controls;
