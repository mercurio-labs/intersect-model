//! Original direct-parent outputs and ordered receipts captured before retention.
use super::*;
use crate::definitions::{
    bundle::DefinitionSource,
    conversion::{InvocationContext, InvocationObserver, ProviderId},
    ecore::ParserObservation,
};
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
            Err("direct-parent job denied".into())
        } else {
            Ok(())
        }
    }
}
impl InvocationObserver for Lease {
    fn before_invoke(&self, _: &ProviderId) -> Result<(), String> {
        panic!("direct-parent preparation cannot invoke providers")
    }
    fn work(&self, n: usize) -> Result<(), String> {
        self.record('w', n)
    }
    fn retain_storage(&self, n: usize) -> Result<(), String> {
        self.record('s', n)
    }
}
fn setup(case: usize, profile: usize) -> Result<(Resolver, Type, SourceSpan), String> {
    let child = match case {
        0 => "<eClassifiers x:type='e:EClass' name='Child'/>",
        1 => "<eClassifiers x:type='e:EClass' name='Child' eSuperTypes='#//Root #//Other'/>",
        2 => "<eClassifiers x:type='e:EClass' name='Child' eSuperTypes='#//Base'/>",
        3..=6 => {
            "<eClassifiers x:type='e:EClass' name='Child'><eTypeParameters name='U'/><eGenericSuperTypes eClassifier='#//Base'><eTypeArguments eTypeParameter='#//Child/U'/></eGenericSuperTypes></eClassifiers>"
        }
        7 => "<eClassifiers x:type='e:EClass' name='Child' eSuperTypes='#//Root #//Root'/>",
        8 => "<eClassifiers x:type='e:EClass' name='Child' eSuperTypes='#//Missing'/>",
        9 => "<eClassifiers x:type='e:EClass' name='Child' eSuperTypes='#//Scalar'/>",
        10 => {
            "<eClassifiers x:type='e:EClass' name='Child' eSuperTypes='#//Root'><eGenericSuperTypes eClassifier='#//Other'/></eClassifiers>"
        }
        11 => {
            "<eClassifiers x:type='e:EClass' name='Child'><eGenericSuperTypes eClassifier='#//Base'><eTypeArguments eClassifier='#//Root'/></eGenericSuperTypes><eGenericSuperTypes eClassifier='#//Base'><eTypeArguments eClassifier='#//Other'/></eGenericSuperTypes></eClassifiers>"
        }
        12 => {
            "<eClassifiers x:type='e:EClass' name='Child'><eTypeParameters name='U'/><eTypeParameters name='V'/><eGenericSuperTypes eClassifier='#//Pair'><eTypeArguments eTypeParameter='#//Child/V'/><eTypeArguments eTypeParameter='#//Child/U'/></eGenericSuperTypes></eClassifiers>"
        }
        13 => {
            "<eClassifiers x:type='e:EClass' name='Child'><eGenericSuperTypes eClassifier='#//Root'/><eGenericSuperTypes eClassifier='#//Other'/></eClassifiers>"
        }
        14 => "<eClassifiers x:type='e:EClass' name='Child' eSuperTypes='#//Root'/>",
        _ => unreachable!(),
    };
    let text = format!(
        "<e:EPackage xmlns:e='http://www.eclipse.org/emf/2002/Ecore' xmlns:x='http://www.w3.org/2001/XMLSchema-instance' name='p' nsURI='urn:direct-parent'><eClassifiers x:type='e:EClass' name='Root'/><eClassifiers x:type='e:EClass' name='Other'/><eClassifiers x:type='e:EClass' name='Base'><eTypeParameters name='T'/></eClassifiers><eClassifiers x:type='e:EDataType' name='Scalar'/><eClassifiers x:type='e:EClass' name='Pair'><eTypeParameters name='A'/><eTypeParameters name='B'/></eClassifiers>{child}</e:EPackage>"
    );
    let source = DefinitionSource::new("memory:direct-parent.ecore", SourceKind::Ecore, text);
    let bundle = DefinitionBundle {
        version: 1,
        sources: vec![source.clone()],
        namespaces: vec![],
    };
    let mut resolver = Resolver::new(&bundle).map_err(|e| format!("{e:?}"))?;
    let context = InvocationContext::default();
    let observation = ParserObservation::new(&context);
    if profile > 0 {
        resolver
            .resolve_materialization_inventory_observed(&observation)
            .map_err(|e| format!("{e:?}"))?;
    }
    if profile > 1 {
        resolver
            .enable_compiled_feature_signatures_observed(&observation)
            .map_err(|e| format!("{e:?}"))?;
    }
    let id = |n| DeclarationId {
        source: source.id.clone(),
        path: vec![n],
    };
    let classifier = |n| Type::Classifier {
        declaration: id(n),
        arguments: vec![],
        raw: false,
    };
    let receiver = if case == 14 {
        Type::Wildcard {
            upper: None,
            lower: None,
        }
    } else {
        Type::Classifier {
            declaration: id(5),
            arguments: match case {
                3 => vec![classifier(0)],
                5 => vec![classifier(0), classifier(1)],
                12 => vec![classifier(0), classifier(1)],
                _ => vec![],
            },
            raw: matches!(case, 4 | 6),
        }
    };
    Ok((
        resolver,
        receiver,
        SourceSpan {
            source: source.id,
            start: 17,
            end: 29,
        },
    ))
}
fn observation(case: usize, profile: usize) -> serde_json::Value {
    let (resolver, receiver, span) = match setup(case, profile) {
        Ok(inputs) => inputs,
        Err(error) => return serde_json::json!({"case":case,"profile":profile,"setup_error":error}),
    };
    let events = Rc::new(RefCell::new(vec![]));
    let context = InvocationContext::new(Rc::new(Lease {
        events: events.clone(),
        deny: None,
    }));
    let observation = ParserObservation::new(&context);
    let result =
        observation.finish(resolver.direct_parent_types_observed(&receiver, &span, &observation));
    serde_json::json!({"case":case,"profile":profile,"result":format!("{result:?}"),"events":*events.borrow()})
}
#[test]
fn frozen_direct_parent_outputs_and_ordered_receipts() {
    let expected: Vec<serde_json::Value> = serde_json::from_str(include_str!(
        "generic_direct_parent_preparation_baseline.json"
    ))
    .unwrap();
    let rows: Vec<_> = (0..15)
        .flat_map(|case| (0..3).map(move |profile| observation(case, profile)))
        .collect();
    assert_eq!(rows, expected);
    assert_eq!(rows.len(), 45);
}

type Outcome = Result<Vec<(Type, SourceSpan)>, crate::definitions::ecore::ObservedImportError>;
fn drive(
    resolver: &Resolver,
    receiver: &Type,
    span: &SourceSpan,
    quantum: usize,
    deny: Option<usize>,
) -> (Outcome, Events, usize) {
    let events = Rc::new(RefCell::new(vec![]));
    let mut job = DirectParentPreparation::begin(resolver);
    let mut advances = 0;
    loop {
        job = std::hint::black_box(job);
        let lease = Rc::new(Lease {
            events: events.clone(),
            deny,
        });
        let weak = Rc::downgrade(&lease);
        let result = {
            let context = InvocationContext::new(lease.clone());
            let observation = ParserObservation::new(&context);
            let mut result = Ok(false);
            for _ in 0..quantum {
                advances += 1;
                assert!(advances < 1000);
                result = job.advance(receiver, span, &observation);
                if result != Ok(false) {
                    break;
                }
            }
            observation.finish(result)
        };
        drop(lease);
        assert!(
            weak.upgrade().is_none(),
            "parent job retained an observer lease"
        );
        match result {
            Ok(false) => {}
            Ok(true) => {
                let count = events.borrow().len();
                let context = InvocationContext::default();
                assert!(
                    job.advance(receiver, span, &ParserObservation::new(&context))
                        .unwrap()
                );
                assert_eq!(events.borrow().len(), count);
                return (Ok(job.finish(span).unwrap()), events, advances);
            }
            Err(error) => {
                let count = events.borrow().len();
                let context = InvocationContext::default();
                assert!(
                    job.advance(receiver, span, &ParserObservation::new(&context))
                        .is_err()
                );
                assert!(job.finish(span).is_err());
                assert_eq!(events.borrow().len(), count);
                return (Err(error), events, advances);
            }
        }
    }
}
#[test]
fn retained_direct_parents_preserve_frozen_outputs_receipts_and_moved_fresh_leases() {
    let expected: Vec<serde_json::Value> = serde_json::from_str(include_str!(
        "generic_direct_parent_preparation_baseline.json"
    ))
    .unwrap();
    let mut checked = 0;
    for row in expected {
        let case = row["case"].as_u64().unwrap() as usize;
        let profile = row["profile"].as_u64().unwrap() as usize;
        let Ok((resolver, receiver, span)) = setup(case, profile) else {
            continue;
        };
        let original = resolver.resource_sets().0.document(&span.source).unwrap() as *const _;
        let mut count = None;
        for quantum in [1, 3, 256] {
            let (output, events, advances) = drive(&resolver, &receiver, &span, quantum, None);
            assert_eq!(
                format!("{output:?}"),
                row["result"],
                "case {case}, profile {profile}"
            );
            assert_eq!(
                serde_json::to_value(&*events.borrow()).unwrap(),
                row["events"],
                "case {case}, profile {profile}"
            );
            assert_eq!(*count.get_or_insert(advances), advances);
            assert_eq!(
                resolver.resource_sets().0.document(&span.source).unwrap() as *const _,
                original
            );
        }
        checked += 1;
    }
    assert_eq!(checked, 39);
}
#[test]
fn retained_direct_parents_keep_exact_literal_substitutions_and_raw_identities() {
    for profile in 0..3 {
        let (resolver, receiver, span) = setup(12, profile).unwrap();
        let (output, _, advances) = drive(&resolver, &receiver, &span, 1, None);
        let output = output.unwrap();
        assert!(
            advances >= 9,
            "two bindings and one parent must be retained"
        );
        let classifier = |index| Type::Classifier {
            declaration: DeclarationId {
                source: span.source.clone(),
                path: vec![index],
            },
            arguments: vec![],
            raw: false,
        };
        assert_eq!(output.len(), 1);
        assert_eq!(
            output[0].0,
            Type::Classifier {
                declaration: DeclarationId {
                    source: span.source.clone(),
                    path: vec![4]
                },
                arguments: vec![classifier(1), classifier(0)],
                raw: false
            }
        );
        assert_eq!(output[0].1.source, span.source);
        assert_eq!(
            output[0].1.start,
            resolver
                .resources(&DeclarationId {
                    source: span.source.clone(),
                    path: vec![5, 2]
                })
                .document(&span.source)
                .unwrap()
                .source
                .text
                .find("<eGenericSuperTypes")
                .unwrap()
        );
        let (resolver, receiver, span) = setup(4, profile).unwrap();
        let output = drive(&resolver, &receiver, &span, 3, None).0.unwrap();
        assert_eq!(
            output[0].0,
            Type::Classifier {
                declaration: DeclarationId {
                    source: span.source.clone(),
                    path: vec![2]
                },
                arguments: vec![Type::Parameter {
                    declaration: DeclarationId {
                        source: span.source.clone(),
                        path: vec![5, 0]
                    }
                }],
                raw: false
            }
        );
    }
}
#[test]
fn retained_direct_parents_deny_every_callback_terminally_and_recover() {
    for profile in [0, 2] {
        let (resolver, receiver, span) = setup(12, profile).unwrap();
        let (expected, full, _) = drive(&resolver, &receiver, &span, 1, None);
        let full = full.borrow().clone();
        for callback in 1..=full.len() {
            let (resolver, receiver, span) = setup(12, profile).unwrap();
            let (error, prefix, _) = drive(&resolver, &receiver, &span, 3, Some(callback));
            assert_eq!(
                error.unwrap_err(),
                crate::definitions::ecore::ObservedImportError::Observation(
                    "direct-parent job denied".into()
                )
            );
            assert_eq!(*prefix.borrow(), full[..callback]);
            assert_eq!(drive(&resolver, &receiver, &span, 256, None).0, expected);
        }
    }
}
#[test]
fn retained_direct_parents_drop_every_frontier_without_mutating_previous_outputs() {
    for profile in [0, 2] {
        let (resolver, receiver, span) = setup(12, profile).unwrap();
        let (expected, _, advances) = drive(&resolver, &receiver, &span, 1, None);
        let original = resolver.resource_sets().0.document(&span.source).unwrap() as *const _;
        for prefix in 0..advances {
            let context = InvocationContext::default();
            let observation = ParserObservation::new(&context);
            let prior = resolver
                .direct_parent_types_observed(&receiver, &span, &observation)
                .unwrap();
            let mut job = DirectParentPreparation::begin(&resolver);
            for _ in 0..prefix {
                assert!(!job.advance(&receiver, &span, &observation).unwrap());
            }
            assert!(job.finish(&span).is_err());
            assert_eq!(
                resolver
                    .direct_parent_types_observed(&receiver, &span, &observation)
                    .unwrap(),
                prior
            );
            assert_eq!(drive(&resolver, &receiver, &span, 256, None).0, expected);
            assert_eq!(
                resolver.resource_sets().0.document(&span.source).unwrap() as *const _,
                original
            );
        }
    }
}
