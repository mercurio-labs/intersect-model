//! Captured against the recursive d02bdc68 implementation before replacement.
use super::*;
use crate::definitions::{
    bundle::{DefinitionBundle, DefinitionSource, SourceKind},
    conversion::{InvocationContext, InvocationObserver, ProviderId},
    ecore::ParserObservation,
};
use sha2::{Digest, Sha256};
use std::{cell::RefCell, rc::Rc};

#[derive(Default)]
struct Meter {
    events: Rc<RefCell<Vec<(char, usize)>>>,
    deny_at: Option<usize>,
}
impl InvocationObserver for Meter {
    fn before_invoke(&self, _: &ProviderId) -> Result<(), String> {
        panic!("ancestry traversal cannot invoke providers")
    }
    fn work(&self, units: usize) -> Result<(), String> {
        self.record('w', units)
    }
    fn retain_storage(&self, bytes: usize) -> Result<(), String> {
        self.record('s', bytes)
    }
}
impl Meter {
    fn record(&self, kind: char, units: usize) -> Result<(), String> {
        let mut events = self.events.borrow_mut();
        events.push((kind, units));
        if self.deny_at == Some(events.len()) {
            return Err("ancestry callback denied".into());
        }
        Ok(())
    }
}
fn fixture(case: usize) -> (DefinitionBundle, usize) {
    let classes = match case {
        0..=2 => {
            let parents = ["#//Raw #//Left #//Right", "#//Left #//Raw #//Right", "#//Left #//Right #//Raw"][case];
            format!("<eClassifiers x:type='e:EClass' name='Root'/><eClassifiers x:type='e:EClass' name='Other'/><eClassifiers x:type='e:EClass' name='Base'><eTypeParameters name='T'/></eClassifiers><eClassifiers x:type='e:EClass' name='Raw' eSuperTypes='#//Base'/><eClassifiers x:type='e:EClass' name='Left'><eGenericSuperTypes eClassifier='#//Base'><eTypeArguments eClassifier='#//Root'/></eGenericSuperTypes></eClassifiers><eClassifiers x:type='e:EClass' name='Right'><eGenericSuperTypes eClassifier='#//Base'><eTypeArguments eClassifier='#//Other'/></eGenericSuperTypes></eClassifiers><eClassifiers x:type='e:EClass' name='Child' eSuperTypes='{parents}'/>")
        }
        3 => "<eClassifiers x:type='e:EClass' name='Root' eSuperTypes='#//Child'/><eClassifiers x:type='e:EClass' name='Child' eSuperTypes='#//Root'/>".into(),
        4 => "<eClassifiers x:type='e:EClass' name='Root'/><eClassifiers x:type='e:EClass' name='Child' eSuperTypes='#//Root #//Root'/>".into(),
        5 => "<eClassifiers x:type='e:EDataType' name='Root'/><eClassifiers x:type='e:EClass' name='Child' eSuperTypes='#//Root'/>".into(),
        6 => "<eClassifiers x:type='e:EClass' name='Root'/><eClassifiers x:type='e:EClass' name='Child' eSuperTypes='#//Missing'/>".into(),
        7 => (0..130).map(|n| if n == 0 { "<eClassifiers x:type='e:EClass' name='C0'/>".into() } else { format!("<eClassifiers x:type='e:EClass' name='C{n}' eSuperTypes='#//C{}'/>", n - 1) }).collect(),
        8 => "<eClassifiers x:type='e:EClass' name='Root'/><eClassifiers x:type='e:EClass' name='Child' eSuperTypes='#//Root'/>".into(),
        _ => unreachable!(),
    };
    let text = format!(
        "<e:EPackage xmlns:e='http://www.eclipse.org/emf/2002/Ecore' xmlns:x='http://www.w3.org/2001/XMLSchema-instance' name='p' nsURI='urn:ancestry-continuation'>{classes}</e:EPackage>"
    );
    let bundle = DefinitionBundle {
        version: 1,
        sources: vec![DefinitionSource::new(
            "memory:ancestry-continuation.ecore",
            SourceKind::Ecore,
            text,
        )],
        namespaces: vec![],
    };
    (
        bundle,
        if case < 3 {
            6
        } else if case == 7 {
            129
        } else {
            1
        },
    )
}
fn inputs(bundle: &DefinitionBundle, index: usize) -> (Type, SourceSpan) {
    (
        Type::Classifier {
            declaration: DeclarationId {
                source: bundle.sources[0].id.clone(),
                path: vec![index],
            },
            arguments: vec![],
            raw: false,
        },
        SourceSpan {
            source: bundle.sources[0].id.clone(),
            start: 19,
            end: 23,
        },
    )
}
#[test]
fn frozen_recursive_outputs_and_ordered_observation_receipts() {
    let baseline: serde_json::Value = serde_json::from_str(include_str!(
        "generic_inheritance_preparation_baseline.json"
    ))
    .unwrap();
    let mut checked = 0;
    for case in 0..8 {
        let (bundle, index) = fixture(case);
        for prepared in [false, true] {
            let mut resolver = Resolver::new(&bundle).unwrap();
            if prepared {
                let invocation = InvocationContext::default();
                // Some invalid fixtures fail inventory preparation first. Keep
                // those as standalone inputs rather than changing error order.
                if resolver
                    .resolve_materialization_inventory_observed(&ParserObservation::new(
                        &invocation,
                    ))
                    .is_err()
                {
                    continue;
                }
            }
            let (receiver, span) = inputs(&bundle, index);
            let meter = Rc::new(Meter::default());
            let invocation = InvocationContext::new(meter.clone());
            let observation = ParserObservation::new(&invocation);
            let result = observation.finish(resolver.ancestry_expression_observed(
                &receiver,
                &span,
                &observation,
            ));
            let payload = format!("{result:?}|{:?}", meter.events.borrow());
            let digest = format!("{:x}", Sha256::digest(payload.as_bytes()));
            let work: usize = meter
                .events
                .borrow()
                .iter()
                .filter(|e| e.0 == 'w')
                .map(|e| e.1)
                .sum();
            let storage: usize = meter
                .events
                .borrow()
                .iter()
                .filter(|e| e.0 == 's')
                .map(|e| e.1)
                .sum();
            let expected = baseline["observations"]
                .as_array()
                .unwrap()
                .iter()
                .find(|row| row["case"] == case && row["prepared"] == prepared)
                .unwrap();
            assert_eq!(
                digest, expected["sha256"],
                "case {case}, prepared {prepared}"
            );
            assert_eq!(work, expected["work"].as_u64().unwrap() as usize);
            assert_eq!(storage, expected["storage"].as_u64().unwrap() as usize);
            checked += 1;
        }
    }
    assert_eq!(checked, 14);
}

type Outcome = Result<Vec<InheritedType>, crate::definitions::ecore::ObservedImportError>;
type Events = Rc<RefCell<Vec<(char, usize)>>>;
fn drive(
    resolver: &Resolver,
    receiver: &Type,
    span: &SourceSpan,
    quantum: usize,
    deny_at: Option<usize>,
) -> (Outcome, Events, usize) {
    let events = Rc::new(RefCell::new(vec![]));
    let mut job = AncestryPreparation::begin(resolver, receiver, span);
    let mut advances = 0;
    loop {
        // Move the state between leases. There are no pointers into a frame.
        let moved = job;
        job = std::hint::black_box(moved);
        let lease = Rc::new(Meter {
            events: events.clone(),
            deny_at,
        });
        let weak = Rc::downgrade(&lease);
        let result = {
            let invocation = InvocationContext::new(lease.clone());
            let observation = ParserObservation::new(&invocation);
            let mut result = Ok(false);
            for _ in 0..quantum {
                advances += 1;
                result = job.advance(&observation);
                if result != Ok(false) {
                    break;
                }
            }
            observation.finish(result)
        };
        drop(lease);
        assert!(weak.upgrade().is_none(), "cursor retained an old observer");
        match result {
            Ok(false) => {}
            Ok(true) => {
                let before = events.borrow().len();
                let invocation = InvocationContext::default();
                assert!(job.advance(&ParserObservation::new(&invocation)).unwrap());
                assert_eq!(events.borrow().len(), before);
                return (Ok(job.finish().unwrap()), events, advances);
            }
            Err(error) => {
                let before = events.borrow().len();
                let invocation = InvocationContext::default();
                assert!(job.advance(&ParserObservation::new(&invocation)).is_err());
                assert!(job.finish().is_err());
                assert_eq!(events.borrow().len(), before);
                return (Err(error), events, advances);
            }
        }
    }
}
#[test]
fn retained_ancestry_matches_all_frozen_receipts_with_fresh_leases_and_quanta() {
    let baseline: serde_json::Value = serde_json::from_str(include_str!(
        "generic_inheritance_preparation_baseline.json"
    ))
    .unwrap();
    for row in baseline["observations"].as_array().unwrap() {
        let (bundle, index) = fixture(row["case"].as_u64().unwrap() as usize);
        let mut resolver = Resolver::new(&bundle).unwrap();
        if row["prepared"].as_bool().unwrap() {
            let invocation = InvocationContext::default();
            resolver
                .resolve_materialization_inventory_observed(&ParserObservation::new(&invocation))
                .unwrap();
        }
        let (receiver, span) = inputs(&bundle, index);
        let mut expected_advances = None;
        for quantum in [1, 3, 256] {
            let (result, events, advances) = drive(&resolver, &receiver, &span, quantum, None);
            let payload = format!("{result:?}|{:?}", events.borrow());
            assert_eq!(
                format!("{:x}", Sha256::digest(payload.as_bytes())),
                row["sha256"]
            );
            assert_eq!(*expected_advances.get_or_insert(advances), advances);
        }
    }
}
#[test]
fn retained_ancestry_denies_every_callback_without_output_or_stale_authority() {
    let (bundle, index) = fixture(8);
    let resolver = Resolver::new(&bundle).unwrap();
    let (receiver, span) = inputs(&bundle, index);
    let (expected, events, _) = drive(&resolver, &receiver, &span, 1, None);
    assert!(expected.is_ok());
    let count = events.borrow().len();
    assert!(count > 0);
    for callback in 1..=count {
        let (denied, prefix, _) = drive(&resolver, &receiver, &span, 3, Some(callback));
        assert_eq!(
            denied,
            Err(crate::definitions::ecore::ObservedImportError::Observation(
                "ancestry callback denied".into()
            ))
        );
        assert_eq!(&*prefix.borrow(), &events.borrow()[..callback]);
    }
    assert_eq!(drive(&resolver, &receiver, &span, 256, None).0, expected);
    assert_eq!(bundle.sources[0].id, span.source);
}
#[test]
fn retained_ancestry_exposes_only_complete_output_and_drop_preserves_the_resolver() {
    let (bundle, index) = fixture(8);
    let resolver = Resolver::new(&bundle).unwrap();
    let (receiver, span) = inputs(&bundle, index);
    let (expected, _, advances) = drive(&resolver, &receiver, &span, 1, None);
    assert!(advances > 2);
    for prefix in 0..advances {
        let mut job = AncestryPreparation::begin(&resolver, &receiver, &span);
        for _ in 0..prefix {
            let invocation = InvocationContext::default();
            assert!(!job.advance(&ParserObservation::new(&invocation)).unwrap());
        }
        assert!(job.finish().unwrap_err().message.contains("incomplete"));
    }
    assert_eq!(drive(&resolver, &receiver, &span, 1, None).0, expected);
    assert_eq!(
        resolver.ancestry_expression(&receiver, &span).unwrap(),
        expected.unwrap()
    );
}
