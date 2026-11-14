//! Initial D observations: separate new contract, oldC remains frozen.
use super::*;
use crate::definitions::generics_decoder_new_contract_capture::{Capture, positive_events};
fn run_d(c: &Case, mode: &str, quantum: usize, control: Control) -> Value {
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
                SourceInventoryPreparation::subtree_with_retained_generic_source_decoding(
                    &resolver, id, &obs,
                )
            } else {
                SourceInventoryPreparation::new_with_retained_generic_source_decoding(
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
                            let source_decoding = job.is_source_decoding_step();
                            let oldphase = match &job.state {
                                State::Walking { phase: p, .. } => phase(*p),
                                State::Agreement { .. } => json!("retained_agreement"),
                                State::Publishing { .. } => json!("retained_publication"),
                                State::Source { .. } => json!("retained_generic_source_decoding"),
                                State::Transition => {
                                    panic!("unpublished transition escaped advance")
                                }
                            };
                            let oldcursor = job.cursor.clone();
                            polls += 1;
                            assert!(polls < 100_000);
                            let result = obs.finish(job.advance(&obs));
                            steps.push(json!({"poll":polls,"continuation":continuation,"source_decoding":source_decoding,"phase":oldphase,"cursor":oldcursor,"event_start":begin,"event_end":meter.events.borrow().len(),"result":format!("{result:?}")}));
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

#[test]
#[cfg_attr(not(feature = "milestone-tests"), ignore = "milestone")]
fn retained_source_decoding_inventory_first_complete_d_transcripts_and_controls() {
    let cases: Vec<Case> =
        serde_json::from_str(include_str!("generic-source-decoding-before-cases.json")).unwrap();
    let mut main = Capture::new("inventory-d-main");
    let mut controls = Capture::new("inventory-d-controls");
    let selected = [
        "inventory_multisource",
        "decode_leaf",
        "decode_child_href",
        "decode_href_257",
        "decode_wildcard_both",
        "decode_deep_8",
    ];
    for case in &cases {
        for mode in ["whole", "subtree"] {
            let old = run_c(case, mode, 1, Control::default());
            let full = run_d(case, mode, 1, Control::default());
            for key in [
                "id",
                "mode",
                "source",
                "selected_subtree",
                "output",
                "admitted_work",
                "admitted_storage",
                "fresh_leases_released",
                "source_buffers_unchanged",
                "parsed_document_identity_unchanged",
                "setup_error",
                "stage",
            ] {
                assert_eq!(full[key], old[key], "{key} {} {mode}", case.id);
            }
            if full.get("setup_error").is_none() {
                assert_eq!(
                    positive_events(&full),
                    positive_events(&old),
                    "{} {mode}",
                    case.id
                );
            }
            let added: Vec<_> = full["steps"]
                .as_array()
                .into_iter()
                .flatten()
                .filter(|step| step["source_decoding"] == true)
                .map(|step| {
                    (
                        step["poll"].as_u64().unwrap() as usize,
                        step["event_start"].as_u64().unwrap() as usize,
                    )
                })
                .collect();
            if let (Some(new), Some(old)) = (full["polls"].as_u64(), old["polls"].as_u64()) {
                assert_eq!(
                    new,
                    old + added.len() as u64,
                    "exact added source polls {} {mode}",
                    case.id
                );
            }
            for quantum in [1, 3, 256] {
                let row = run_d(case, mode, quantum, Control::default());
                let mut comparable = row.clone();
                comparable["quantum"] = json!(1);
                assert_eq!(comparable, full, "{} {mode} q{quantum}", case.id);
                main.row(&row);
            }
            if !selected.contains(&case.id.as_str()) || full.get("setup_error").is_some() {
                continue;
            }
            let events = full["events"].as_array().unwrap();
            for deny in 1..=events.len() {
                let row = run_d(
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
                controls.row(&json!({"kind":"deny_every_callback","id":case.id,"mode":mode,"callback":deny,"complete_observation":row}));
            }
            for (poll, start) in added {
                assert_eq!(events[start], json!(["w", 0]));
                for drop_at in [poll - 1, poll] {
                    let row = run_d(
                        case,
                        mode,
                        1,
                        Control {
                            drop_at: Some(drop_at),
                            ..Control::default()
                        },
                    );
                    assert!(row["output"].get("error").is_some());
                    controls.row(&json!({"kind":"drop_every_source_boundary","id":case.id,"mode":mode,"source_poll":poll,"drop_at":drop_at,"complete_observation":row}));
                }
            }
            let work = full["admitted_work"].as_u64().unwrap() as usize;
            let storage = full["admitted_storage"].as_u64().unwrap() as usize;
            for quantum in [1, 3, 256] {
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
                    controls.row(&json!({"kind":"budget","id":case.id,"mode":mode,"quantum":quantum,"complete_observation":run_d(case,mode,quantum,control)}));
                }
            }
            assert_eq!(
                run_d(case, mode, 1, Control::default()),
                full,
                "fresh retry {} {mode}",
                case.id
            );
        }
    }
    main.finish();
    controls.finish();
}

#[test]
fn retained_source_decoding_inventory_same_resolver_every_drop_keeps_cache_and_inputs() {
    let cases: Vec<Case> =
        serde_json::from_str(include_str!("generic-source-decoding-before-cases.json")).unwrap();
    let case = cases
        .iter()
        .find(|case| case.id == "inventory_multisource")
        .unwrap();
    let input = bundle(case);
    let resolver = Resolver::new(&input).unwrap();
    let expected = resolver.resolve_all().unwrap();
    let cache = resolver.legacy_owner_cache_capture();
    let pointer = input.sources[0].text.as_ptr();
    let full = run_d(case, "whole", 1, Control::default());
    let context = InvocationContext::default();
    let observation = ParserObservation::new(&context);
    for boundary in 0..full["polls"].as_u64().unwrap() as usize {
        let mut preparation =
            SourceInventoryPreparation::new_with_retained_generic_source_decoding(
                &resolver,
                &observation,
            )
            .unwrap();
        for _ in 0..boundary {
            assert!(!preparation.advance(&observation).unwrap());
        }
        drop(preparation);
        assert_eq!(resolver.legacy_owner_cache_capture(), cache);
        assert_eq!(resolver.resolve_all().unwrap(), expected);
        assert_eq!(pointer, input.sources[0].text.as_ptr());
    }
}
