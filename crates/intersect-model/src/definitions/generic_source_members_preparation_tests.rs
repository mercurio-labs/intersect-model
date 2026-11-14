use super::super::source_members_capture_tests::inputs;
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
            Err("source member job denied".into())
        } else {
            Ok(())
        }
    }
}
impl InvocationObserver for Lease {
    fn before_invoke(&self, _: &ProviderId) -> Result<(), String> {
        panic!("projection cannot invoke a provider")
    }
    fn work(&self, n: usize) -> Result<(), String> {
        self.record('w', n)
    }
    fn retain_storage(&self, n: usize) -> Result<(), String> {
        self.record('s', n)
    }
}
fn output(report: SourceMembers<'_>) -> serde_json::Value {
    serde_json::json!({"kind":if matches!(report,SourceMembers::Views(_)){"views"}else{"owned"},"members":(0..report.len()).map(|i|serde_json::to_value(report.get(i).unwrap()).unwrap()).collect::<Vec<_>>()})
}
fn drive(
    case: usize,
    profile: usize,
    quantum: usize,
    deny: Option<usize>,
) -> (serde_json::Value, Events, usize) {
    let (bundle, expression, span) = inputs(case);
    let runtime = profile % 2 == 1;
    let views = (profile / 2) % 2 == 1;
    let prepared = profile / 4;
    let mut resolver = Resolver::new(&bundle).unwrap();
    if prepared > 0
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
    if prepared == 2 {
        let context = InvocationContext::default();
        let observation = ParserObservation::new(&context);
        drop(resolver.project_member_sources_observed(
            &expression,
            &span,
            runtime,
            views,
            &observation,
        ));
    }
    let pointer = resolver
        .resource_sets()
        .0
        .document(&bundle.sources[0].id)
        .unwrap() as *const _;
    let events = Rc::new(RefCell::new(vec![]));
    let mut job = Some(SourceMembersPreparation::begin(&resolver, runtime, views));
    let mut advances = 0;
    let result = loop {
        job = std::hint::black_box(job);
        let lease = Rc::new(Lease {
            events: events.clone(),
            deny,
        });
        let weak = Rc::downgrade(&lease);
        let outcome = {
            let context = InvocationContext::new(lease.clone());
            let observation = ParserObservation::new(&context);
            let mut result = None;
            for _ in 0..quantum {
                advances += 1;
                assert!(advances < 100_000);
                match observation.finish(job.as_mut().unwrap().advance(
                    &expression,
                    &span,
                    &observation,
                )) {
                    Ok(false) => {}
                    Ok(true) => {
                        result = Some(
                            job.take()
                                .unwrap()
                                .finish(&span)
                                .map(output)
                                .map_err(ObservedImportError::Import),
                        );
                        break;
                    }
                    Err(error) => {
                        assert!(job.take().unwrap().finish(&span).is_err());
                        result = Some(Err(error));
                        break;
                    }
                }
            }
            result
        };
        drop(lease);
        assert!(weak.upgrade().is_none());
        if let Some(result) = outcome {
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
        Ok(report) => report,
        Err(error) => serde_json::json!({"error":format!("{error:?}")}),
    };
    let row = {
        let events = events.borrow();
        serde_json::json!({"case":case,"profile":profile,"output":output,"events_sha256":format!("{:x}",Sha256::digest(serde_json::to_vec(&*events).unwrap())),"work":events.iter().filter(|e|e.0=='w').map(|e|e.1).sum::<usize>(),"storage":events.iter().filter(|e|e.0=='s').map(|e|e.1).sum::<usize>(),"erasure_cache":format!("{:?}",resolver.prepared_erased_signatures.get().map(|v|v.borrow())),"ancestry_cache":format!("{:?}",resolver.prepared_ancestry.get().map(|v|v.borrow()))})
    };
    (row, events, advances)
}
#[test]
fn retained_source_members_match_all_468_frozen_outputs_errors_fees_caches_and_quanta() {
    let baseline: serde_json::Value =
        serde_json::from_str(include_str!("generic_source_members_baseline.json")).unwrap();
    for case in 0..39 {
        for profile in 0..12 {
            let mut count = None;
            for quantum in [1, 3, 256] {
                let (row, _, advances) = drive(case, profile, quantum, None);
                assert_eq!(
                    row,
                    baseline[case * 12 + profile],
                    "case {case} profile {profile} q{quantum}"
                );
                assert_eq!(*count.get_or_insert(advances), advances);
            }
        }
    }
}
#[test]
fn retained_source_members_deny_every_callback_with_exact_prefixes_and_fresh_recovery() {
    for (case, profile) in [(0, 0), (35, 7)] {
        let (expected, events, _) = drive(case, profile, 1, None);
        assert!(expected["output"]["members"].is_array());
        let full = events.borrow().clone();
        for callback in 1..=full.len() {
            let (row, prefix, _) = drive(case, profile, 3, Some(callback));
            assert_eq!(
                row["output"]["error"],
                "Observation(\"source member job denied\")"
            );
            assert_eq!(*prefix.borrow(), full[..callback]);
        }
        assert_eq!(drive(case, profile, 256, None).0, expected);
    }
}
#[test]
fn retained_source_members_drop_every_operation_frontier_without_mutating_prior_inputs() {
    let (bundle, expression, span) = inputs(35);
    let resolver = Resolver::new(&bundle).unwrap();
    let context = InvocationContext::default();
    let observation = ParserObservation::new(&context);
    let prior = output(
        resolver
            .project_member_sources_observed(&expression, &span, true, true, &observation)
            .unwrap(),
    );
    let pointer = resolver
        .resource_sets()
        .0
        .document(&bundle.sources[0].id)
        .unwrap() as *const _;
    let (_, _, advances) = drive(35, 3, 1, None);
    for prefix in 0..advances {
        let mut job = SourceMembersPreparation::begin(&resolver, true, true);
        for _ in 0..prefix {
            assert!(!job.advance(&expression, &span, &observation).unwrap());
        }
        assert!(job.finish(&span).is_err());
        assert_eq!(
            resolver
                .resource_sets()
                .0
                .document(&bundle.sources[0].id)
                .unwrap() as *const _,
            pointer
        );
        assert_eq!(
            output(
                resolver
                    .project_member_sources_observed(&expression, &span, true, true, &observation)
                    .unwrap()
            ),
            prior
        );
    }
}
