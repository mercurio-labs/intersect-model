//! Retained inventory observations and cancellation/recovery controls.
use super::*;
fn run_c(c: &Case, mode: &str, quantum: usize, control: Control) -> Value {
    let bundle = bundle(c);
    let before = serde_json::to_value(&bundle).unwrap();
    let buffers: Vec<_> = bundle.sources.iter().map(|s| s.text.as_ptr()).collect();
    let resolver = match super::Resolver::new(&bundle) {
        Ok(r) => r,
        Err(e) => {
            return json!({"id":c.id,"mode":mode,"quantum":quantum,"control":control,"source":before,"setup_error":format!("{e:?}"),"stage":"resolver","events":[]});
        }
    };
    let root = if mode == "subtree" {
        match resolver
            .resource_sets()
            .0
            .resolve(&bundle.sources[0].id, &c.owner)
        {
            Ok(id) => Some(id),
            Err(e) => {
                return json!({"id":c.id,"mode":mode,"quantum":quantum,"control":control,"source":before,"setup_error":format!("{e:?}"),"stage":"root_selection","events":[]});
            }
        }
    } else {
        None
    };
    let document = resolver
        .resource_sets()
        .0
        .document(&bundle.sources[0].id)
        .unwrap() as *const _;
    let meter = Rc::new(Meter {
        control,
        ..Meter::default()
    });
    let mut steps = vec![];
    let mut polls = 0;
    let mut failure = None;
    let mut poison = None;
    let output = if mode == "sync" {
        let lease = Rc::new(Lease(meter.clone()));
        let weak = Rc::downgrade(&lease);
        let result = {
            let ctx = InvocationContext::new(lease.clone());
            encode(resolver.resolve_all_observed(&ctx))
        };
        drop(lease);
        assert!(weak.upgrade().is_none());
        result
    } else {
        let lease = Rc::new(Lease(meter.clone()));
        let weak = Rc::downgrade(&lease);
        let start = {
            let ctx = InvocationContext::new(lease.clone());
            let obs = ParserObservation::new(&ctx);
            obs.finish(if let Some(id) = &root {
                SourceInventoryPreparation::subtree_with_retained_source_inventory_agreement(
                    &resolver, id, &obs,
                )
            } else {
                SourceInventoryPreparation::new_with_retained_source_inventory_agreement(
                    &resolver, &obs,
                )
            })
        };
        drop(lease);
        assert!(weak.upgrade().is_none());
        match start {
            Err(e) => json!({"error":format!("{e:?}"),"stage":"initialization"}),
            Ok(mut job) => {
                let mut abandoned_leases = Vec::new();
                'all: loop {
                    let lease = Rc::new(Lease(meter.clone()));
                    let weak = Rc::downgrade(&lease);
                    abandoned_leases.push(weak.clone());
                    {
                        let ctx = InvocationContext::new(lease.clone());
                        let obs = ParserObservation::new(&ctx);
                        for _ in 0..quantum {
                            if control.drop_at == Some(polls) {
                                break 'all;
                            }
                            let mut relocation = vec![job];
                            job = relocation.pop().unwrap();
                            let begin = meter.events.borrow().len();
                            let continuation = job.is_retained_agreement_step();
                            let oldphase = match &job.state {
                                State::Walking { phase: p, .. } => phase(*p),
                                State::Agreement { .. } => json!("retained_agreement"),
                                State::Publishing { .. } => json!("retained_publication"),
                                State::Source { .. } => {
                                    panic!("Legacy C driver unexpectedly entered D source decoding")
                                }
                                State::Transition => {
                                    panic!("unpublished transition escaped advance")
                                }
                            };
                            let oldcursor = job.cursor.clone();
                            polls += 1;
                            assert!(polls < 100_000);
                            let result = obs.finish(job.advance(&obs));
                            steps.push(json!({"poll":polls,"continuation":continuation,"phase":oldphase,"cursor":oldcursor,"event_start":begin,"event_end":meter.events.borrow().len(),"result":format!("{result:?}")}));
                            match result {
                                Ok(true) => {
                                    drop(ctx);
                                    drop(lease);
                                    assert!(weak.upgrade().is_none());
                                    break 'all;
                                }
                                Ok(false) => {}
                                Err(e) => {
                                    failure = Some(format!("{e:?}"));
                                    drop(ctx);
                                    drop(lease);
                                    assert!(weak.upgrade().is_none());
                                    break 'all;
                                }
                            }
                        }
                    }
                    drop(lease);
                    assert!(weak.upgrade().is_none());
                }
                assert!(
                    abandoned_leases
                        .iter()
                        .all(|lease| lease.upgrade().is_none()),
                    "abandoned attempt retained observer"
                );
                if failure.is_some() {
                    let quiet = Rc::new(Meter::default());
                    let ctx = InvocationContext::new(Rc::new(Lease(quiet.clone())));
                    let obs = ParserObservation::new(&ctx);
                    let again = obs.finish(job.advance(&obs));
                    assert!(again.is_err());
                    assert!(quiet.events.borrow().is_empty());
                    poison = Some(format!("{again:?}"));
                }
                let before_finish = meter.events.borrow().len();
                let ctx = InvocationContext::default();
                let obs = ParserObservation::new(&ctx);
                let finish = encode(obs.finish(job.finish()));
                assert_eq!(before_finish, meter.events.borrow().len());
                if failure.is_some() || control.drop_at.is_some() {
                    assert!(finish.get("error").is_some())
                }
                finish
            }
        }
    };
    assert_eq!(serde_json::to_value(&bundle).unwrap(), before);
    assert_eq!(
        bundle
            .sources
            .iter()
            .map(|s| s.text.as_ptr())
            .collect::<Vec<_>>(),
        buffers
    );
    assert_eq!(
        resolver
            .resource_sets()
            .0
            .document(&bundle.sources[0].id)
            .unwrap() as *const _,
        document
    );
    json!({"id":c.id,"mode":mode,"quantum":quantum,"control":control,"source":before,"selected_subtree":root,"output":output,"failure":failure,"poison":poison,"events":*meter.events.borrow(),"admitted_work":meter.work.get(),"admitted_storage":meter.storage.get(),"polls":polls,"steps":steps,"fresh_leases_released":true,"source_buffers_unchanged":true,"parsed_document_identity_unchanged":true})
}
fn original_literal(case: &Case, mode: &str) -> String {
    let rows: Value = serde_json::from_str(include_str!(
        "generic_source_inventory_original_main_digests.json"
    ))
    .unwrap();
    rows.as_array()
        .unwrap()
        .iter()
        .find(|r| r["id"] == case.id && r["mode"] == mode && r["quantum"] == 1)
        .unwrap()["complete_sha256"]
        .as_str()
        .unwrap()
        .into()
}
fn assert_c_contract(case: &Case, mode: &str, c: &Value) -> Vec<(usize, usize)> {
    let old = run(case, mode, 1, Control::default());
    assert_eq!(
        format!("{:x}", Sha256::digest(serde_json::to_vec(&old).unwrap())),
        original_literal(case, mode)
    );
    if old.get("setup_error").is_some() {
        assert_eq!(c, &old);
        return vec![];
    }
    for key in [
        "source",
        "selected_subtree",
        "output",
        "failure",
        "poison",
        "admitted_work",
        "admitted_storage",
        "fresh_leases_released",
        "source_buffers_unchanged",
        "parsed_document_identity_unchanged",
    ] {
        assert_eq!(c[key], old[key], "{key} {} {mode}", case.id)
    }
    let events = c["events"].as_array().unwrap();
    let extras: Vec<_> = c["steps"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|step| step["continuation"] == true)
        .map(|step| {
            (
                step["poll"].as_u64().unwrap() as usize,
                step["event_start"].as_u64().unwrap() as usize,
            )
        })
        .collect();
    let positions: std::collections::BTreeSet<_> = extras.iter().map(|pair| pair.1).collect();
    for position in &positions {
        assert_eq!(events[*position], json!(["w", 0]));
    }
    assert_eq!(
        json!(
            events
                .iter()
                .enumerate()
                .filter(|(index, _)| !positions.contains(index))
                .map(|(_, event)| event.clone())
                .collect::<Vec<_>>()
        ),
        old["events"]
    );
    assert_eq!(
        c["polls"].as_u64().unwrap(),
        old["polls"].as_u64().unwrap() + extras.len() as u64
    );
    extras
}
#[test]
#[cfg_attr(not(feature = "milestone-tests"), ignore = "milestone")]
fn source_inventory_retained_complete_transcripts_and_controls() {
    let cases: Vec<Case> =
        serde_json::from_str(include_str!("generic_source_inventory_cases.json")).unwrap();
    let mut main = CompleteStream::new("c_main");
    let mut controls = CompleteStream::new("c_controls");
    for case in &cases {
        for mode in ["whole", "subtree"] {
            let full = run_c(case, mode, 1, Control::default());
            let extras = assert_c_contract(case, mode, &full);
            for q in [1, 3, 256] {
                let row = run_c(case, mode, q, Control::default());
                let mut comparable = row.clone();
                comparable["quantum"] = json!(1);
                assert_eq!(comparable, full);
                main.row(row);
            }
            if ![
                "empty_many",
                "exception_order",
                "duplicate_long_targets",
                "inventory_multisource",
            ]
            .contains(&case.id.as_str())
                || full.get("setup_error").is_some()
            {
                continue;
            }
            let events = full["events"].as_array().unwrap();
            for deny in 1..=events.len() {
                let row = run_c(
                    case,
                    mode,
                    1,
                    Control {
                        deny: Some(deny),
                        ..Control::default()
                    },
                );
                assert_eq!(row["events"], json!(&events[..deny]));
                assert!(row["output"].get("error").is_some());
                controls.row(json!({"kind":"deny_every_callback","complete_observation":row}));
            }
            for (poll, _) in extras {
                for drop_at in [poll - 1, poll] {
                    let row = run_c(
                        case,
                        mode,
                        1,
                        Control {
                            drop_at: Some(drop_at),
                            ..Control::default()
                        },
                    );
                    assert!(row["output"].get("error").is_some());
                    controls.row(json!({"kind":"drop_added_boundary","complete_observation":row}));
                }
            }
            let work = full["admitted_work"].as_u64().unwrap() as usize;
            let storage = full["admitted_storage"].as_u64().unwrap() as usize;
            for q in [1, 3, 256] {
                for control in [
                    Control {
                        max_work: Some(work),
                        max_storage: Some(storage),
                        ..Control::default()
                    },
                    Control {
                        max_work: Some(work.saturating_sub(1)),
                        max_storage: Some(storage),
                        ..Control::default()
                    },
                    Control {
                        max_work: Some(work),
                        max_storage: Some(storage.saturating_sub(1)),
                        ..Control::default()
                    },
                ] {
                    let row = run_c(case, mode, q, control);
                    controls.row(json!({"kind":"budget","complete_observation":row}));
                }
            }
        }
    }
    main.finish();
    controls.finish();
}

#[test]
fn source_inventory_retained_same_resolver_drop_retry() {
    let cases: Vec<Case> =
        serde_json::from_str(include_str!("generic_source_inventory_cases.json")).unwrap();
    let case = cases
        .iter()
        .find(|c| c.id == "inventory_multisource")
        .unwrap();
    let input = bundle(case);
    let resolver = Resolver::new(&input).unwrap();
    let expected = resolver.resolve_all().unwrap();
    let source_ptr = input.sources[0].text.as_ptr();
    let expected_cache = resolver.legacy_owner_cache_capture();
    let full = run_c(case, "whole", 1, Control::default());
    let context = InvocationContext::default();
    let observation = ParserObservation::new(&context);
    for boundary in 0..full["polls"].as_u64().unwrap() as usize {
        let mut job = SourceInventoryPreparation::new_with_retained_source_inventory_agreement(
            &resolver,
            &observation,
        )
        .unwrap();
        for _ in 0..boundary {
            assert!(!job.advance(&observation).unwrap());
        }
        drop(job);
        assert_eq!(resolver.legacy_owner_cache_capture(), expected_cache);
        assert_eq!(resolver.resolve_all().unwrap(), expected);
        assert_eq!(source_ptr, input.sources[0].text.as_ptr());
    }
}
