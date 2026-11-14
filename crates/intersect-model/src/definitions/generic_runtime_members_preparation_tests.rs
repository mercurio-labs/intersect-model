use super::super::runtime_members_capture_tests::inputs;
use super::*;
use crate::definitions::{
    conversion::{InvocationContext, InvocationObserver, ProviderId},
    ecore::{ObservedImportError, ParserObservation},
};
use sha2::{Digest, Sha256};
use std::{cell::RefCell, rc::Rc};
type Events = Rc<RefCell<Vec<(char, usize)>>>;
struct Lease {
    events: Events,
    deny: Option<usize>,
}
impl Lease {
    fn record(&self, kind: char, n: usize) -> Result<(), String> {
        let mut events = self.events.borrow_mut();
        events.push((kind, n));
        if self.deny == Some(events.len()) {
            Err("member job denied".into())
        } else {
            Ok(())
        }
    }
}
impl InvocationObserver for Lease {
    fn before_invoke(&self, _: &ProviderId) -> Result<(), String> {
        panic!("unconfigured member preparation cannot invoke providers")
    }
    fn work(&self, n: usize) -> Result<(), String> {
        self.record('w', n)
    }
    fn retain_storage(&self, n: usize) -> Result<(), String> {
        self.record('s', n)
    }
}
fn drive(
    case: usize,
    profile: usize,
    quantum: usize,
    deny: Option<usize>,
) -> (serde_json::Value, Events, usize) {
    let (bundle, receiver) = inputs(case);
    let mut resolver = Resolver::new(&bundle).unwrap();
    if profile >= 2
        && let Err(error) = resolver.resolve_materialization_inventory_observed(
            &ParserObservation::new(&InvocationContext::default()),
        )
    {
        return (
            serde_json::json!({"case":case,"profile":profile,"setup_error":format!("{error:?}")}),
            Rc::new(RefCell::new(vec![])),
            0,
        );
    }
    if profile >= 3 {
        let context = InvocationContext::default();
        let observation = ParserObservation::new(&context);
        drop(resolver.runtime_members_for_materialization_observed(&receiver, &observation));
    }
    let pointer = resolver
        .resource_sets()
        .0
        .document(&bundle.sources[0].id)
        .unwrap() as *const _;
    let events = Rc::new(RefCell::new(vec![]));
    let mut job = Some(RuntimeMembersPreparation::begin(
        &resolver,
        Cow::Owned(receiver),
        profile == 0 || profile == 4,
    ));
    let mut advances = 0;
    let result = loop {
        job = std::hint::black_box(job);
        let lease = Rc::new(Lease {
            events: events.clone(),
            deny,
        });
        let weak = Rc::downgrade(&lease);
        let result = {
            let context = InvocationContext::new(lease.clone());
            let observation = ParserObservation::new(&context);
            let mut result = None;
            for _ in 0..quantum {
                advances += 1;
                assert!(advances < 100_000);
                let progress = observation.finish(job.as_mut().unwrap().advance(&observation));
                match progress {
                    Ok(false) => {}
                    Ok(true) => {
                        let completed = job.take().unwrap();
                        result = Some(if profile == 0 || profile == 4 {
                            observation
                                .finish(completed.finish_owned(&observation))
                                .map(|report| serde_json::to_value(report).unwrap())
                        } else {
                            completed
                                .finish_parts()
                                .map(|(report, _)| serde_json::to_value(report).unwrap())
                                .map_err(ObservedImportError::Import)
                        });
                        break;
                    }
                    Err(error) => {
                        assert!(job.take().unwrap().finish_parts().is_err());
                        result = Some(Err(error));
                        break;
                    }
                }
            }
            result
        };
        drop(lease);
        assert!(weak.upgrade().is_none());
        if let Some(result) = result {
            break result;
        }
    };
    assert_eq!(
        resolver
            .resource_sets()
            .0
            .document(&bundle.sources[0].id)
            .unwrap() as *const _,
        pointer
    );
    let output = match result {
        Ok(report) => serde_json::json!({"ok":report}),
        Err(error) => serde_json::json!({"error":format!("{error:?}")}),
    };
    let row = {
        let events = events.borrow();
        serde_json::json!({"case":case,"profile":profile,"output":output,"events_sha256":format!("{:x}",Sha256::digest(serde_json::to_vec(&*events).unwrap())),"work":events.iter().filter(|e|e.0=='w').map(|e|e.1).sum::<usize>(),"storage":events.iter().filter(|e|e.0=='s').map(|e|e.1).sum::<usize>(),"cache":format!("{:?}",resolver.prepared_erased_signatures.get().map(|v|v.borrow()))})
    };
    (row, events, advances)
}
#[test]
fn retained_runtime_members_match_all_180_frozen_profiles_fees_caches_and_quanta() {
    let baseline: serde_json::Value =
        serde_json::from_str(include_str!("generic_runtime_members_baseline.json")).unwrap();
    for case in 0..36 {
        for profile in 0..5 {
            let mut count = None;
            for quantum in [1, 3, 256] {
                let (row, _, advances) = drive(case, profile, quantum, None);
                assert_eq!(
                    row,
                    baseline[case * 5 + profile],
                    "case {case}, profile {profile}, q{quantum}"
                );
                assert_eq!(*count.get_or_insert(advances), advances);
            }
        }
    }
}
#[test]
#[cfg_attr(not(feature = "milestone-tests"), ignore = "milestone")]
fn retained_runtime_members_deny_every_callback_without_partial_report_and_recover() {
    for (case, profile) in [(0, 0), (20, 2)] {
        let (expected, events, _) = drive(case, profile, 1, None);
        assert!(expected["output"]["ok"].is_object());
        let full = events.borrow().clone();
        for callback in 1..=full.len() {
            let (row, prefix, _) = drive(case, profile, 3, Some(callback));
            assert_eq!(row["output"]["error"], "Observation(\"member job denied\")");
            assert_eq!(*prefix.borrow(), full[..callback]);
        }
        assert_eq!(drive(case, profile, 256, None).0, expected);
    }
}
#[test]
fn retained_runtime_members_every_advance_drop_keeps_source_and_prior_report_usable() {
    let (bundle, receiver) = inputs(0);
    let resolver = Resolver::new(&bundle).unwrap();
    let context = InvocationContext::default();
    let observation = ParserObservation::new(&context);
    let prior = resolver.runtime_members(&receiver).unwrap();
    let pointer = resolver
        .resource_sets()
        .0
        .document(&bundle.sources[0].id)
        .unwrap() as *const _;
    let (_, _, advances) = drive(0, 0, 1, None);
    for prefix in 0..advances {
        let mut job = RuntimeMembersPreparation::begin(&resolver, Cow::Borrowed(&receiver), true);
        for _ in 0..prefix {
            assert!(!job.advance(&observation).unwrap());
        }
        assert!(job.finish_parts().is_err());
        assert_eq!(
            resolver
                .resource_sets()
                .0
                .document(&bundle.sources[0].id)
                .unwrap() as *const _,
            pointer
        );
        assert_eq!(resolver.runtime_members(&receiver).unwrap(), prior);
    }
}
