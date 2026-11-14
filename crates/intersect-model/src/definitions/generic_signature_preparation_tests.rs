use super::super::tests::{cache, fixture, prepared};
use super::*;
use crate::definitions::{
    conversion::{InvocationContext, InvocationObserver, ProviderId},
    ecore::ParserObservation,
};
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
            Err("signature job denied".into())
        } else {
            Ok(())
        }
    }
}
impl InvocationObserver for Lease {
    fn before_invoke(&self, _: &ProviderId) -> Result<(), String> {
        panic!("signature preparation cannot invoke providers")
    }
    fn work(&self, n: usize) -> Result<(), String> {
        self.record('w', n)
    }
    fn retain_storage(&self, n: usize) -> Result<(), String> {
        self.record('s', n)
    }
}
fn setup(profile: usize) -> (Resolver, DeclarationId, Substitution) {
    let (bundle, types) = fixture();
    let mut resolver = prepared(&bundle, profile != 5);
    let owner = DeclarationId {
        source: bundle.sources[0].id.clone(),
        path: vec![0, 1],
    };
    let environment = Substitution::new(if matches!(profile, 1 | 4) {
        vec![ParameterBinding {
            parameter: DeclarationId {
                source: owner.source.clone(),
                path: vec![0, 0],
            },
            argument: types[1].expression().clone(),
        }]
    } else {
        vec![]
    })
    .unwrap();
    if profile == 3 {
        resolver
            .compiled_feature_signature_inputs(
                &owner,
                &environment,
                &ParserObservation::new(&InvocationContext::default()),
            )
            .unwrap();
    }
    if profile == 6 {
        let table = cache(&resolver);
        for i in 0..4096 {
            table.borrow_mut().signatures.insert(
                DeclarationId {
                    source: owner.source.clone(),
                    path: vec![999, i],
                },
                Rc::new(vec![]),
            );
        }
    }
    if profile == 7 {
        let id = DeclarationId {
            source: owner.source.clone(),
            path: vec![1],
        };
        resolver.provider_relations.insert(
            (id.clone(), id),
            crate::definitions::conversion::ClassifierRelation::Equivalent,
        );
    }
    (resolver, owner, environment)
}
fn output(rows: ErasedSignatures) -> serde_json::Value {
    serde_json::json!({"kind":if matches!(rows, ErasedSignatures::Shared(_)) {"shared"} else {"owned"}, "rows":serde_json::to_value(&rows).unwrap()})
}
fn drive(
    profile: usize,
    quantum: usize,
    deny: Option<usize>,
) -> (serde_json::Value, Vec<(char, usize)>, usize) {
    let (resolver, owner, environment) = setup(profile);
    let events = Rc::new(RefCell::new(vec![]));
    let pointer = resolver.resource_sets().0.document(&owner.source).unwrap() as *const _;
    let mut job = Some(SignaturePreparation::begin(&resolver, profile >= 2));
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
                assert!(advances < 1_000);
                match observation.finish(job.as_mut().unwrap().advance(
                    &owner,
                    &environment,
                    &observation,
                )) {
                    Ok(false) => {
                        if profile == 2 {
                            assert!(!cache(&resolver).borrow().signatures.contains_key(&owner));
                        }
                    }
                    Ok(true) => {
                        result = Some(output(job.take().unwrap().finish(&owner).unwrap()));
                        break;
                    }
                    Err(error) => {
                        assert!(job.take().unwrap().finish(&owner).is_err());
                        if profile == 2 {
                            assert!(!cache(&resolver).borrow().signatures.contains_key(&owner));
                        }
                        result = Some(serde_json::json!({"error":format!("{error:?}")}));
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
        resolver.resource_sets().0.document(&owner.source).unwrap() as *const _,
        pointer
    );
    let receipts = events.borrow().clone();
    (result, receipts, advances)
}
#[test]
fn retained_signatures_preserve_literal_types_cache_policies_and_moved_fresh_leases() {
    for profile in 0..8 {
        let (expected, receipts, advances) = drive(profile, 1, None);
        assert_eq!(
            expected["kind"],
            if matches!(profile, 2 | 3) {
                "shared"
            } else {
                "owned"
            }
        );
        assert_eq!(expected["rows"].as_array().unwrap().len(), 1);
        let row = &expected["rows"][0];
        assert_eq!(row["owner"]["path"], serde_json::json!([0, 1]));
        assert_eq!(row["feature"], "eType");
        assert_eq!(row["declared"].as_array().unwrap().len(), 1);
        assert_eq!(row["declared"][0]["kind"], "parameter");
        assert_eq!(
            row["declared"][0]["declaration"]["path"],
            serde_json::json!([0, 0])
        );
        let actual = &row["specialized"][0];
        if matches!(profile, 1 | 4) {
            assert_eq!(actual["kind"], "classifier");
            assert_eq!(actual["declaration"]["path"], serde_json::json!([1]));
            assert_eq!(actual["arguments"], serde_json::json!([]));
            assert_eq!(actual["raw"], false);
        } else {
            assert_eq!(*actual, row["declared"][0]);
        }
        if profile != 3 {
            assert!(advances > 8);
        }
        for quantum in [3, 256] {
            let (actual, events, count) = drive(profile, quantum, None);
            assert_eq!(actual, expected);
            assert_eq!(events, receipts);
            assert_eq!(count, advances);
        }
    }
}
#[test]
fn retained_signatures_deny_every_callback_without_partial_cache_and_recover() {
    for profile in [0, 2] {
        let (expected, full, _) = drive(profile, 1, None);
        for callback in 1..=full.len() {
            let (error, prefix, _) = drive(profile, 3, Some(callback));
            assert_eq!(error["error"], "Observation(\"signature job denied\")");
            assert_eq!(prefix, full[..callback]);
        }
        assert_eq!(drive(profile, 256, None).0, expected);
    }
}
#[test]
fn retained_signatures_drop_every_frontier_and_fail_terminally_on_borrow_conflicts() {
    for profile in [0, 2] {
        let (_, _, advances) = drive(profile, 1, None);
        for prefix in 0..advances {
            let (resolver, owner, environment) = setup(profile);
            let context = InvocationContext::default();
            let observation = ParserObservation::new(&context);
            let prior = resolver
                .specialize_signatures_observed(&owner, &environment, &observation)
                .unwrap();
            let mut job = SignaturePreparation::begin(&resolver, profile >= 2);
            for _ in 0..prefix {
                assert!(!job.advance(&owner, &environment, &observation).unwrap());
            }
            assert!(job.finish(&owner).is_err());
            assert!(!cache(&resolver).borrow().signatures.contains_key(&owner));
            assert_eq!(
                resolver
                    .specialize_signatures_observed(&owner, &environment, &observation)
                    .unwrap(),
                prior
            );
            let mut fresh = SignaturePreparation::begin(&resolver, profile >= 2);
            while !fresh.advance(&owner, &environment, &observation).unwrap() {}
            assert_eq!(
                serde_json::to_value(fresh.finish(&owner).unwrap()).unwrap(),
                serde_json::to_value(prior).unwrap()
            );
        }
    }
    let (resolver, owner, environment) = setup(2);
    let context = InvocationContext::default();
    let observation = ParserObservation::new(&context);
    let table = cache(&resolver);
    let borrow = table.borrow_mut();
    let mut job = SignaturePreparation::begin(&resolver, true);
    assert!(
        job.advance(&owner, &environment, &observation)
            .unwrap_err()
            .message
            .contains("already borrowed")
    );
    drop(borrow);
    assert!(
        job.advance(&owner, &environment, &observation)
            .unwrap_err()
            .message
            .contains("failed")
    );
    assert!(job.finish(&owner).is_err());
    let mut fresh = SignaturePreparation::begin(&resolver, true);
    while !fresh.advance(&owner, &environment, &observation).unwrap() {}
    assert!(matches!(
        fresh.finish(&owner).unwrap(),
        ErasedSignatures::Shared(_)
    ));
}
