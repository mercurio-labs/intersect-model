//! Cursor controls against complete, immutable pre-edit observations.
use super::super::preparation::{AgreementPreparation, LegacyPreparation};
use super::*;

enum Job<'a> {
    Legacy(LegacyPreparation<'a>),
    Agreement(AgreementPreparation<'a>),
}
impl<'a> Job<'a> {
    fn advance(
        &mut self,
        resolver: &'a Resolver,
        owner: &DeclarationId,
        feature: &str,
        observation: &ParserObservation<'_, '_>,
    ) -> Result<bool, ImportError> {
        match self {
            Self::Legacy(job) => job.advance(resolver, owner, feature, observation),
            Self::Agreement(job) => job.advance(resolver, owner, observation),
        }
    }
    fn finish(self, owner: &DeclarationId, observation: &ParserObservation<'_, '_>) -> Value {
        match self {
            Self::Legacy(job) => encoded(observation.finish(job.finish(owner))),
            Self::Agreement(job) => encoded(observation.finish(job.finish(owner))),
        }
    }
}
// Explicit independent inputs and controls for the verification matrix.
#[allow(clippy::too_many_arguments)]
fn cursor_observation(
    resolver: &Resolver,
    owner: &DeclarationId,
    feature: &str,
    operation: &str,
    control: Control,
    quantum: usize,
    stop: Option<usize>,
    poison: bool,
) -> (Value, usize, bool) {
    let mut job = Some(if operation == "legacy" {
        Job::Legacy(LegacyPreparation::begin())
    } else {
        Job::Agreement(AgreementPreparation::begin())
    });
    let meter = Rc::new(Meter {
        control,
        ..Meter::default()
    });
    let mut output = None;
    let mut steps = 0;
    let mut failed = false;
    while output.is_none() && stop.is_none_or(|stop| steps < stop) {
        job = std::hint::black_box(job);
        let lease = Rc::new(PublicLease(meter.clone()));
        let weak = Rc::downgrade(&lease);
        {
            let context = InvocationContext::new(lease.clone());
            let observation = ParserObservation::new(&context);
            for _ in 0..quantum {
                steps += 1;
                assert!(steps < 100_000, "cursor did not complete");
                match job
                    .as_mut()
                    .unwrap()
                    .advance(resolver, owner, feature, &observation)
                {
                    Ok(true) => {
                        output = Some(job.take().unwrap().finish(owner, &observation));
                        break;
                    }
                    Ok(false) => {}
                    Err(error) => {
                        output = Some(encoded(observation.finish(Err::<Value, _>(error))));
                        failed = true;
                        break;
                    }
                }
                if stop == Some(steps) {
                    break;
                }
            }
        }
        drop(lease);
        assert!(
            weak.upgrade().is_none(),
            "cursor retained an observer lease"
        );
    }
    if failed && poison {
        let quiet = Rc::new(Meter::default());
        let context = InvocationContext::new(quiet.clone());
        let observation = ParserObservation::new(&context);
        assert!(
            job.as_mut()
                .unwrap()
                .advance(resolver, owner, feature, &observation)
                .is_err()
        );
        assert!(
            job.take()
                .unwrap()
                .finish(owner, &observation)
                .get("error")
                .is_some()
        );
        assert!(
            quiet.events.borrow().is_empty(),
            "poisoned cursor repeated admissions"
        );
    }
    let before = meter.events.borrow().clone();
    drop(job);
    assert_eq!(*meter.events.borrow(), before, "drop invoked observer");
    let complete = output.is_some();
    (
        json!({"output":output,"events":*meter.events.borrow(),"admitted_work":meter.work.get(),"admitted_storage":meter.storage.get()}),
        steps,
        complete,
    )
}

fn expected_hash(case: &Case, operation: &str, supplemental: bool) -> String {
    let fixture: Value = serde_json::from_str(if supplemental {
        include_str!("generic_legacy_supplement_digests.json")
    } else {
        include_str!("generic_legacy_baseline_digests.json")
    })
    .unwrap();
    fixture[if supplemental {
        "complete_main_profiles"
    } else {
        "main_rows"
    }]
    .as_array()
    .unwrap()
    .iter()
    .find(|row| row["id"] == case.id && row["operation"] == operation)
    .unwrap()["complete_sha256"]
        .as_str()
        .unwrap()
        .into()
}

#[test]
fn retained_legacy_moved_fresh_leases_all_quanta_match_complete_frozen_rows() {
    for (fixture, supplemental) in [
        (include_str!("generic_legacy_baseline_cases.json"), false),
        (include_str!("generic_legacy_supplement_cases.json"), true),
    ] {
        let cases: Vec<Case> = serde_json::from_str(fixture).unwrap();
        for case in &cases {
            for operation in ["legacy", "agreement"] {
                for quantum in [1, 3, 256] {
                    let mut row = capture_case_cursor(case, operation, Control::default(), quantum);
                    if supplemental {
                        row["input_source_id"] = serde_json::to_value(
                            source(&case.source_kind, &case.uri, &case.text).id,
                        )
                        .unwrap();
                        if let Some(dependency) = &case.dependency {
                            row["input_dependency_source_id"] = serde_json::to_value(
                                source("ecore", &dependency.uri, &dependency.text).id,
                            )
                            .unwrap();
                        }
                    }
                    assert_eq!(
                        format!("{:x}", Sha256::digest(serde_json::to_vec(&row).unwrap())),
                        expected_hash(case, operation, supplemental),
                        "{} {} q{}",
                        case.id,
                        operation,
                        quantum
                    );
                }
            }
        }
    }
}

#[test]
fn retained_legacy_every_refusal_poison_drop_and_same_resolver_recovery() {
    let mut cases: Vec<Case> =
        serde_json::from_str(include_str!("generic_legacy_baseline_cases.json")).unwrap();
    cases.extend(
        serde_json::from_str::<Vec<Case>>(include_str!("generic_legacy_supplement_cases.json"))
            .unwrap(),
    );
    for id in [
        "literal_556",
        "comments_whitespace",
        "long_prefix_shadow_good",
        "ordered_exact",
        "ordered_long_mismatch",
    ] {
        let case = cases.iter().find(|c| c.id == id).unwrap();
        let bundle = bundle(
            &case.source_kind,
            &case.uri,
            &case.text,
            case.dependency.as_ref(),
        );
        let before = serde_json::to_value(&bundle).unwrap();
        let resolver = Resolver::new(&bundle).unwrap();
        let owner = resolver
            .resource_sets()
            .0
            .resolve(&bundle.sources[0].id, &case.owner)
            .unwrap();
        let operation = if id.starts_with("ordered") {
            "agreement"
        } else {
            "legacy"
        };
        let (full, steps, complete) = cursor_observation(
            &resolver,
            &owner,
            &case.feature,
            operation,
            Control::default(),
            1,
            None,
            false,
        );
        assert!(complete);
        let events = full["events"].as_array().unwrap();
        for quantum in [1, 3, 256] {
            for deny in 1..=events.len() {
                let (rejected, _, complete) = cursor_observation(
                    &resolver,
                    &owner,
                    &case.feature,
                    operation,
                    Control {
                        deny: Some(deny),
                        ..Control::default()
                    },
                    quantum,
                    None,
                    true,
                );
                assert!(complete);
                assert!(
                    rejected["output"]["error"]
                        .as_str()
                        .unwrap()
                        .contains("legacy baseline denied")
                );
                assert_eq!(rejected["events"], json!(&events[..deny]));
            }
            let (retry, _, complete) = cursor_observation(
                &resolver,
                &owner,
                &case.feature,
                operation,
                Control::default(),
                quantum,
                None,
                false,
            );
            assert!(complete);
            assert_eq!(retry, full, "fresh observer retry changed original result");
        }
        for at in 0..steps {
            let (_, _, complete) = cursor_observation(
                &resolver,
                &owner,
                &case.feature,
                operation,
                Control::default(),
                1,
                Some(at),
                false,
            );
            assert!(!complete);
        }
        let work = full["admitted_work"].as_u64().unwrap() as usize;
        let storage = full["admitted_storage"].as_u64().unwrap() as usize;
        for quantum in [1, 3, 256] {
            let (exact, _, _) = cursor_observation(
                &resolver,
                &owner,
                &case.feature,
                operation,
                Control {
                    max_work: Some(work),
                    max_storage: Some(storage),
                    deny: None,
                },
                quantum,
                None,
                false,
            );
            assert_eq!(exact, full);
            if work > 0 {
                let (short, _, _) = cursor_observation(
                    &resolver,
                    &owner,
                    &case.feature,
                    operation,
                    Control {
                        max_work: Some(work - 1),
                        max_storage: Some(storage),
                        deny: None,
                    },
                    quantum,
                    None,
                    true,
                );
                assert!(
                    short["output"]["error"]
                        .as_str()
                        .unwrap()
                        .contains("legacy work denied")
                );
            }
            if storage > 0 {
                let (short, _, _) = cursor_observation(
                    &resolver,
                    &owner,
                    &case.feature,
                    operation,
                    Control {
                        max_work: Some(work),
                        max_storage: Some(storage - 1),
                        deny: None,
                    },
                    quantum,
                    None,
                    true,
                );
                assert!(
                    short["output"]["error"]
                        .as_str()
                        .unwrap()
                        .contains("legacy storage denied")
                );
            }
        }
        assert_eq!(serde_json::to_value(&bundle).unwrap(), before);
    }
}
fn capture_case_cursor(case: &Case, operation: &str, control: Control, quantum: usize) -> Value {
    let bundle = bundle(
        &case.source_kind,
        &case.uri,
        &case.text,
        case.dependency.as_ref(),
    );
    let before = serde_json::to_value(&bundle).unwrap();
    let resolver = match Resolver::new(&bundle) {
        Ok(value) => value,
        Err(error) => return early(&case.id, operation, "resolver_setup", error),
    };
    let owner = match resolver
        .resource_sets()
        .0
        .resolve(&bundle.sources[0].id, &case.owner)
    {
        Ok(value) => value,
        Err(error) => return early(&case.id, operation, "owner_selection", error),
    };
    let _selected = if let Some(path) = &case.selected_path {
        Some(DeclarationId {
            source: bundle.sources[0].id.clone(),
            path: path.clone(),
        })
    } else if let Some(fragment) = &case.generic_fragment {
        match resolver
            .resource_sets()
            .0
            .resolve(&bundle.sources[0].id, fragment)
        {
            Ok(value) => Some(value),
            Err(error) => return early(&case.id, operation, "generic_selection", error),
        }
    } else {
        None
    };
    let document = resolver
        .resource_sets()
        .0
        .document(&bundle.sources[0].id)
        .unwrap() as *const _;
    let observation = cursor_observation(
        &resolver,
        &owner,
        &case.feature,
        operation,
        control,
        quantum,
        None,
        false,
    )
    .0;
    assert_eq!(serde_json::to_value(&bundle).unwrap(), before);
    assert_eq!(
        resolver
            .resource_sets()
            .0
            .document(&bundle.sources[0].id)
            .unwrap() as *const _,
        document
    );
    json!({"id":case.id,"operation":operation,"control":control,"observation":observation,"source_bundle_unchanged":true,"parsed_document_identity_unchanged":true})
}
