use super::*;
use crate::definitions::runtime_schema_test_inputs as inputs;
use crate::definitions::{
    conversion::{InvocationContext, InvocationObserver, ProviderId},
    ecore::ParserObservation,
};
use std::{cell::RefCell, rc::Rc};
#[derive(Default)]
struct Meter(RefCell<Vec<(char, usize)>>);
impl InvocationObserver for Meter {
    fn before_invoke(&self, _: &ProviderId) -> Result<(), String> {
        panic!("unconfigured model context cannot invoke providers")
    }
    fn work(&self, n: usize) -> Result<(), String> {
        self.0.borrow_mut().push(('w', n));
        Ok(())
    }
    fn retain_storage(&self, n: usize) -> Result<(), String> {
        self.0.borrow_mut().push(('s', n));
        Ok(())
    }
}
#[test]
fn runtime_schema_capture_complete_owned_materialization_and_ordered_fees() {
    let baseline: serde_json::Value =
        serde_json::from_str(include_str!("runtime_schema_baseline.json")).unwrap();
    for case in 0..12 {
        let (bundle, requested, _) = inputs::fixture(case);
        let requested = requested
            .into_iter()
            .map(RuntimeType::new)
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        let resources = ResourceSet::new(&bundle).unwrap();
        let meter = Rc::new(Meter::default());
        let context = InvocationContext::new(meter.clone());
        let observation = ParserObservation::new(&context);
        let result = observation.finish(materialize_context_with_inputs_observed(
            &bundle,
            &requested,
            None,
            Some(resources),
            &observation,
        ));
        let output = match result {
            Ok(schema) => serde_json::json!({"ok":schema}),
            Err(error) => serde_json::json!({"error":format!("{error:?}")}),
        };
        let events = meter.0.borrow();
        let row = serde_json::json!({"case":case,"output":output,"events_sha256":format!("{:x}",Sha256::digest(serde_json::to_vec(&*events).unwrap())),"work":events.iter().filter(|e|e.0=='w').map(|e|e.1).sum::<usize>(),"storage":events.iter().filter(|e|e.0=='s').map(|e|e.1).sum::<usize>()});
        assert_eq!(row, baseline["owned"][case]);
    }
}

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
            Err("owned schema denied".into())
        } else {
            Ok(())
        }
    }
}
impl InvocationObserver for Lease {
    fn before_invoke(&self, _: &ProviderId) -> Result<(), String> {
        panic!("unconfigured schema metadata cannot invoke providers")
    }
    fn checkpoint(&self) -> Result<(), String> {
        self.record('c', 0)
    }
    fn work(&self, n: usize) -> Result<(), String> {
        self.record('w', n)
    }
    fn retain_storage(&self, n: usize) -> Result<(), String> {
        self.record('s', n)
    }
}
fn frozen_output(
    result: Result<Schema, crate::definitions::ecore::ObservedImportError>,
) -> serde_json::Value {
    match result {
        Ok(schema) => serde_json::json!({"ok":schema}),
        Err(error) => serde_json::json!({"error":format!("{error:?}")}),
    }
}
#[test]
fn retained_owned_schema_moves_and_releases_fresh_observers_with_complete_frozen_results() {
    let baseline: serde_json::Value =
        serde_json::from_str(include_str!("runtime_schema_baseline.json")).unwrap();
    for case in 0..12 {
        let (bundle, requested, _) = inputs::fixture(case);
        let requested = requested
            .into_iter()
            .map(RuntimeType::new)
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        let resources = ResourceSet::new(&bundle).unwrap();
        let mut expected_count = None;
        let mut expected_events = None;
        for quantum in [1, 3, 256] {
            let events = Rc::new(RefCell::new(vec![]));
            let lease = Rc::new(Lease {
                events: events.clone(),
                deny: None,
            });
            let weak = Rc::downgrade(&lease);
            let started = {
                let operation = InvocationContext::new(lease.clone());
                OwnedSchemaPreparation::begin(
                    &bundle,
                    &requested,
                    None,
                    resources.clone(),
                    &operation,
                )
            };
            drop(lease);
            assert!(weak.upgrade().is_none());
            let mut advances = 0;
            let result = match started {
                Err(error) => Err(error),
                Ok(mut job) => loop {
                    let moved = job;
                    job = std::hint::black_box(moved);
                    let lease = Rc::new(Lease {
                        events: events.clone(),
                        deny: None,
                    });
                    let weak = Rc::downgrade(&lease);
                    let result = {
                        let operation = InvocationContext::new(lease.clone());
                        let mut result = Ok(false);
                        for _ in 0..quantum {
                            advances += 1;
                            result = job.advance(None, &operation);
                            if result != Ok(false) {
                                break;
                            }
                        }
                        result
                    };
                    drop(lease);
                    assert!(weak.upgrade().is_none());
                    match result {
                        Ok(false) => {}
                        Ok(true) => break job.finish(),
                        Err(error) => {
                            assert_eq!(
                                job.advance(None, &InvocationContext::default()),
                                Err(error.clone())
                            );
                            assert_eq!(job.finish(), Err(error.clone()));
                            break Err(error);
                        }
                    }
                },
            };
            assert_eq!(frozen_output(result), baseline["owned"][case]["output"]);
            assert_eq!(*expected_count.get_or_insert(advances), advances);
            assert_eq!(
                *expected_events.get_or_insert_with(|| events.borrow().clone()),
                *events.borrow()
            );
        }
    }
}
type CursorOutcome = Result<Schema, crate::definitions::ecore::ObservedImportError>;
fn cursor_attempt(deny: Option<usize>, quantum: usize) -> (CursorOutcome, Events, usize) {
    let (bundle, requested, _) = inputs::fixture(0);
    let requested = requested
        .into_iter()
        .map(RuntimeType::new)
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    let resources = ResourceSet::new(&bundle).unwrap();
    let warm = InvocationContext::default();
    let warm_observation = ParserObservation::new(&warm);
    let InitializedMaterialization {
        resolver,
        inventory,
        fallback,
    } = initialize_materialization::<OwnedAlternatives>(
        &bundle,
        &requested,
        None,
        Some(resources),
        &warm_observation,
    )
    .unwrap();
    let mut state = new_materialization_context(&bundle, &resolver, &warm_observation).unwrap();
    let mut preparation =
        PreparedMaterialization::begin(&mut state, inventory, &requested, &fallback).unwrap();
    let events = Rc::new(RefCell::new(vec![]));
    let mut advances = 0;
    loop {
        let moved = preparation;
        preparation = std::hint::black_box(moved);
        let lease = Rc::new(Lease {
            events: events.clone(),
            deny,
        });
        let weak = Rc::downgrade(&lease);
        let result = {
            let operation = InvocationContext::new(lease.clone());
            let observation = ParserObservation::new(&operation);
            let mut result = Ok(false);
            for _ in 0..quantum {
                advances += 1;
                result = observation.finish(preparation.advance(
                    &mut state,
                    &mut OwnedAlternatives,
                    None,
                    &fallback,
                    &observation,
                ));
                if result != Ok(false) {
                    break;
                }
            }
            result
        };
        drop(lease);
        assert!(weak.upgrade().is_none());
        match result {
            Ok(false) => {}
            Ok(true) => return (Ok(preparation.finish(&fallback).unwrap()), events, advances),
            Err(error) => {
                assert!(preparation.finish(&fallback).is_err());
                return (Err(error), events, advances);
            }
        }
    }
}
#[test]
#[cfg_attr(not(feature = "milestone-tests"), ignore = "milestone")]
fn retained_schema_semantic_cursor_denies_every_callback_without_partial_output() {
    let (expected, events, _) = cursor_attempt(None, 1);
    assert!(expected.is_ok());
    for callback in 1..=events.borrow().len() {
        let (denied, prefix, _) = cursor_attempt(Some(callback), 3);
        assert_eq!(
            denied,
            Err(crate::definitions::ecore::ObservedImportError::Observation(
                "owned schema denied".into()
            ))
        );
        assert_eq!(*prefix.borrow(), events.borrow()[..callback]);
    }
    assert_eq!(cursor_attempt(None, 256).0, expected);
}
#[test]
fn retained_owned_schema_drop_prefixes_ready_cancellation_and_fresh_recovery() {
    let (bundle, requested, _) = inputs::fixture(0);
    let requested = requested
        .into_iter()
        .map(RuntimeType::new)
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    let resources = ResourceSet::new(&bundle).unwrap();
    let original = resources.document(&bundle.sources[0].id).unwrap() as *const _;
    let operation = InvocationContext::default();
    let mut job =
        OwnedSchemaPreparation::begin(&bundle, &requested, None, resources.clone(), &operation)
            .unwrap();
    let mut count = 0;
    loop {
        count += 1;
        if job.advance(None, &operation).unwrap() {
            break;
        }
    }
    let expected = job.finish().unwrap();
    for prefix in 0..count {
        let mut job =
            OwnedSchemaPreparation::begin(&bundle, &requested, None, resources.clone(), &operation)
                .unwrap();
        for _ in 0..prefix {
            assert!(!job.advance(None, &operation).unwrap());
        }
        assert!(job.finish().is_err());
    }
    let mut ready =
        OwnedSchemaPreparation::begin(&bundle, &requested, None, resources.clone(), &operation)
            .unwrap();
    while !ready.advance(None, &operation).unwrap() {}
    let denied = InvocationContext::new(Rc::new(Lease {
        events: Rc::new(RefCell::new(vec![])),
        deny: Some(1),
    }));
    let error = ready.advance(None, &denied).unwrap_err();
    assert!(matches!(
        error,
        crate::definitions::ecore::ObservedImportError::Observation(_)
    ));
    assert_eq!(ready.advance(None, &operation), Err(error.clone()));
    assert_eq!(ready.finish(), Err(error));
    let fresh =
        materialize_runtime_types_from_resources(&bundle, &requested, None, resources.clone())
            .unwrap();
    assert_eq!(fresh, expected);
    assert_eq!(
        resources.document(&bundle.sources[0].id).unwrap() as *const _,
        original
    );
}
