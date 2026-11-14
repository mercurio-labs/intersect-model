use super::*;
use crate::definitions::{
    bundle::{DefinitionSource, SourceKind},
    conversion::{InvocationObserver, ProviderId},
};
use std::{cell::RefCell, rc::Rc};

type Events = Rc<RefCell<Vec<(char, usize)>>>;
struct Meter {
    events: Events,
    deny_at: Option<usize>,
}
impl Meter {
    fn record(&self, kind: char, units: usize) -> Result<(), String> {
        let mut events = self.events.borrow_mut();
        events.push((kind, units));
        if self.deny_at == Some(events.len()) {
            return Err("binding query denied".into());
        }
        Ok(())
    }
}
impl InvocationObserver for Meter {
    fn before_invoke(&self, _: &ProviderId) -> Result<(), String> {
        panic!("declaration lookup cannot invoke providers")
    }
    fn checkpoint(&self) -> Result<(), String> {
        self.record('c', 0)
    }
    fn work(&self, units: usize) -> Result<(), String> {
        self.record('w', units)
    }
    fn retain_storage(&self, bytes: usize) -> Result<(), String> {
        self.record('s', bytes)
    }
}
fn fixture(case: usize) -> (DefinitionBundle, usize) {
    let root = "<eClassifiers x:type='e:EClass' name='Root'><eStructuralFeatures x:type='e:EAttribute' name='label' eType='e:EDataType http://www.eclipse.org/emf/2002/Ecore#//EString'/></eClassifiers>";
    let (classes, index) = match case {
        0 | 3 => (format!("{root}<eClassifiers x:type='e:EClass' name='Child' eSuperTypes='#//Root'/>"), 1),
        1 => (format!("{root}<eClassifiers x:type='e:EClass' name='Left' eSuperTypes='#//Root'/><eClassifiers x:type='e:EClass' name='Right' eSuperTypes='#//Root'/><eClassifiers x:type='e:EClass' name='Child' eSuperTypes='#//Left #//Right'/>"), 3),
        2 => (format!("{root}<eClassifiers x:type='e:EClass' name='Other'><eStructuralFeatures x:type='e:EAttribute' name='label' eType='e:EDataType http://www.eclipse.org/emf/2002/Ecore#//EString'/></eClassifiers><eClassifiers x:type='e:EClass' name='Child' eSuperTypes='#//Root #//Other'/>"), 2),
        4 => ("<eClassifiers x:type='e:EClass' name='Root' eSuperTypes='#//Child'/><eClassifiers x:type='e:EClass' name='Child' eSuperTypes='#//Root'/>".into(), 1),
        _ => unreachable!(),
    };
    let source = DefinitionSource::new(
        "memory:binding-contract-query.ecore",
        SourceKind::Ecore,
        format!(
            "<e:EPackage xmlns:e='http://www.eclipse.org/emf/2002/Ecore' xmlns:x='http://www.w3.org/2001/XMLSchema-instance' name='p' nsURI='urn:binding-query'>{classes}</e:EPackage>"
        ),
    );
    (
        DefinitionBundle {
            version: 1,
            sources: vec![source],
            namespaces: vec![],
        },
        index,
    )
}
fn request(bundle: &DefinitionBundle, index: usize, case: usize) -> ContractQuery {
    ContractQuery {
        receiver: Type::Classifier {
            declaration: DeclarationId {
                source: bundle.sources[0].id.clone(),
                path: vec![index],
            },
            arguments: vec![],
            raw: false,
        },
        name: if case == 3 { "missing" } else { "label" }.into(),
        span: SourceSpan {
            source: bundle.sources[0].id.clone(),
            start: 19,
            end: 23,
        },
    }
}
fn bridge_request(query: ContractQuery) -> MetadataQuery {
    MetadataQuery::ContractFeature {
        receiver: query.receiver,
        name: query.name,
        span: query.span,
    }
}
type FullOutcome = Result<Result<Target, String>, MetadataDriverError>;
fn full_drive(
    bundle: &DefinitionBundle,
    index: usize,
    case: usize,
    limits: ParseLimits,
    quantum: usize,
) -> (FullOutcome, Events, usize) {
    let resources = ResourceSet::new(bundle).unwrap();
    let bridge = MetadataBridge::default();
    bridge.0.lock().unwrap().request = Some(bridge_request(request(bundle, index, case)));
    let mut driver = MetadataDriver::new(bundle, resources, limits);
    let events = Rc::new(RefCell::new(vec![]));
    let mut advances = 0;
    loop {
        let moved = driver;
        driver = std::hint::black_box(moved);
        let lease = Rc::new(Meter {
            events: events.clone(),
            deny_at: None,
        });
        let weak = Rc::downgrade(&lease);
        let outcome = {
            let context = InvocationContext::new(lease.clone());
            let mut outcome = None;
            for _ in 0..quantum {
                advances += 1;
                assert!(advances < 100_000);
                if let Err(error) = driver.advance(&bridge, &context) {
                    outcome = Some(Err(error));
                    break;
                }
                if let Some(reply) = bridge.0.lock().unwrap().reply.take() {
                    outcome = Some(Ok(reply));
                    break;
                }
            }
            outcome
        };
        drop(lease);
        assert!(weak.upgrade().is_none());
        if let Some(outcome) = outcome {
            return (outcome, events, advances);
        }
    }
}
#[test]
fn contract_query_driver_matches_literal_features_errors_quanta_and_legacy_policies() {
    for case in 0..5 {
        let (bundle, index) = fixture(case);
        let expected =
            bridge_request(request(&bundle, index, case)).resolve(&bundle, ParseLimits::default());
        match case {
            0 | 1 => assert!(
                matches!(&expected, Ok(Target::Feature(id)) if id.path == [0, 0] && id.source == bundle.sources[0].id)
            ),
            2 => assert!(
                expected
                    .as_ref()
                    .unwrap_err()
                    .contains("Ambiguous inherited provider feature")
            ),
            3 => assert!(
                expected
                    .as_ref()
                    .unwrap_err()
                    .contains("Unknown provider feature")
            ),
            4 => assert!(
                expected
                    .as_ref()
                    .unwrap_err()
                    .contains("Cyclic generic inheritance")
            ),
            _ => unreachable!(),
        }
        let mut receipt = None;
        let mut count = None;
        for quantum in [1, 3, 256] {
            let (outcome, events, advances) =
                full_drive(&bundle, index, case, ParseLimits::default(), quantum);
            assert_eq!(format!("{:?}", outcome.unwrap()), format!("{expected:?}"));
            assert_eq!(
                *receipt.get_or_insert_with(|| events.borrow().clone()),
                *events.borrow()
            );
            assert_eq!(*count.get_or_insert(advances), advances);
        }
    }
    let (bundle, index) = fixture(0);
    for bytes in [
        bundle.sources[0].text.len() - 1,
        bundle.sources[0].text.len(),
        262_144,
    ] {
        let limits = ParseLimits {
            source_bytes: bytes,
        };
        let expected = bridge_request(request(&bundle, index, 0)).resolve(&bundle, limits);
        let (outcome, _, _) = full_drive(&bundle, index, 0, limits, 3);
        assert_eq!(format!("{:?}", outcome.unwrap()), format!("{expected:?}"));
        assert_eq!(expected.is_ok(), bytes >= bundle.sources[0].text.len());
    }
}
fn seeded(bundle: &DefinitionBundle, index: usize) -> ContractPreparation<'static> {
    let query = Box::new(request(bundle, index, 0));
    let span = query.span.clone();
    let owner = FeatureOwner {
        resolver: Box::new(Resolver::new(bundle).unwrap()),
        query,
    };
    ContractPreparation {
        phase: ContractPhase::Feature(
            ContractPreparation::feature_inputs(owner, &InvocationContext::default()).unwrap(),
        ),
        span,
        output: None,
        failure: None,
    }
}
type LookupOutcome = Result<DeclarationId, ObservedImportError>;
fn lookup_drive(
    mut job: ContractPreparation<'static>,
    quantum: usize,
    deny_at: Option<usize>,
) -> (LookupOutcome, Events, usize) {
    let events = Rc::new(RefCell::new(vec![]));
    let mut advances = 0;
    loop {
        let moved = job;
        job = std::hint::black_box(moved);
        let lease = Rc::new(Meter {
            events: events.clone(),
            deny_at,
        });
        let weak = Rc::downgrade(&lease);
        let result = {
            let context = InvocationContext::new(lease.clone());
            let mut result = Ok(false);
            for _ in 0..quantum {
                advances += 1;
                result = job.advance(&context);
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
            Ok(true) => return (job.finish(), events, advances),
            Err(error) => {
                let before = events.borrow().len();
                assert_eq!(
                    job.advance(&InvocationContext::default()),
                    Err(error.clone())
                );
                assert_eq!(events.borrow().len(), before);
                assert_eq!(job.finish(), Err(error.clone()));
                return (Err(error), events, advances);
            }
        }
    }
}
#[test]
fn contract_lookup_denies_every_callback_and_releases_each_observer_without_output() {
    let (bundle, index) = fixture(0);
    let (expected, events, _) = lookup_drive(seeded(&bundle, index), 1, None);
    assert_eq!(expected.as_ref().unwrap().path, [0, 0]);
    let count = events.borrow().len();
    for callback in 1..=count {
        let (result, prefix, _) = lookup_drive(seeded(&bundle, index), 3, Some(callback));
        assert_eq!(
            result,
            Err(ObservedImportError::Observation(
                "binding query denied".into()
            ))
        );
        assert_eq!(*prefix.borrow(), events.borrow()[..callback]);
    }
    assert_eq!(lookup_drive(seeded(&bundle, index), 256, None).0, expected);
}
#[test]
fn contract_lookup_drop_prefixes_ready_cancellation_and_metadata_failure_are_terminal() {
    let (bundle, index) = fixture(0);
    let (expected, _, advances) = lookup_drive(seeded(&bundle, index), 1, None);
    for prefix in 0..advances {
        let mut job = seeded(&bundle, index);
        for _ in 0..prefix {
            assert!(!job.advance(&InvocationContext::default()).unwrap());
        }
        assert!(job.finish().is_err());
    }
    let mut ready = seeded(&bundle, index);
    while !ready.advance(&InvocationContext::default()).unwrap() {}
    let events = Rc::new(RefCell::new(vec![]));
    let context = InvocationContext::new(Rc::new(Meter {
        events,
        deny_at: Some(1),
    }));
    assert!(matches!(
        ready.advance(&context),
        Err(ObservedImportError::Observation(_))
    ));
    assert!(ready.finish().is_err());
    let bridge = MetadataBridge::default();
    bridge.0.lock().unwrap().request = Some(bridge_request(request(&bundle, index, 0)));
    let mut driver = MetadataDriver::new(
        &bundle,
        ResourceSet::new(&bundle).unwrap(),
        ParseLimits::default(),
    );
    let events = Rc::new(RefCell::new(vec![]));
    let context = InvocationContext::new(Rc::new(Meter {
        events: events.clone(),
        deny_at: Some(1),
    }));
    let error = driver.advance(&bridge, &context).unwrap_err();
    assert!(matches!(error, MetadataDriverError::Observation(_)));
    assert_eq!(
        driver.advance(&bridge, &InvocationContext::default()),
        Err(error)
    );
    assert!(bridge.0.lock().unwrap().reply.is_none());
    assert_eq!(lookup_drive(seeded(&bundle, index), 1, None).0, expected);
}

#[test]
fn contract_query_rechecks_whole_bundle_before_reusing_parsed_resources() {
    let (bundle, index) = fixture(0);
    let resources = ResourceSet::new(&bundle).unwrap();
    for duplicate in [false, true] {
        let mut invalid = bundle.clone();
        if duplicate {
            invalid.sources.push(invalid.sources[0].clone());
        } else {
            invalid.sources[0].text.push(' ');
        }
        let expected =
            bridge_request(request(&bundle, index, 0)).resolve(&invalid, ParseLimits::default());
        assert!(expected.is_err());
        let bridge = MetadataBridge::default();
        bridge.0.lock().unwrap().request = Some(bridge_request(request(&bundle, index, 0)));
        let mut driver = MetadataDriver::new(&invalid, resources.clone(), ParseLimits::default());
        let mut reply = None;
        for _ in 0..100_000 {
            driver
                .advance(&bridge, &InvocationContext::default())
                .unwrap();
            reply = bridge.0.lock().unwrap().reply.take();
            if reply.is_some() {
                break;
            }
        }
        assert_eq!(format!("{:?}", reply.unwrap()), format!("{expected:?}"));
        assert_eq!(
            Resolver::new(&bundle)
                .unwrap()
                .declared_feature(
                    &request(&bundle, index, 0).receiver,
                    "label",
                    &request(&bundle, index, 0).span
                )
                .unwrap()
                .path,
            [0, 0]
        );
    }
}

#[test]
fn enum_query_driver_preserves_literal_identities_diagnostics_and_loaded_source_policy() {
    let payload = "<eClassifiers x:type='e:EEnum' name='Mode'><eAnnotations source='urn:opaque'/><eLiterals name='First' value='7'/><eLiterals name='Alias' value='7' literal=''/></eClassifiers>";
    for (name, padding) in [
        ("First", 0),
        ("Alias", 0),
        ("Missing", 0),
        ("First", 270_000),
    ] {
        let text = format!(
            "<e:EPackage xmlns:e='http://www.eclipse.org/emf/2002/Ecore' xmlns:x='http://www.w3.org/2001/XMLSchema-instance' name='p' nsURI='urn:enum-query'><!--{}-->{payload}</e:EPackage>",
            " ".repeat(padding)
        );
        let bundle = DefinitionBundle {
            version: 1,
            sources: vec![DefinitionSource::new(
                "memory:enum-query.ecore",
                SourceKind::Ecore,
                text,
            )],
            namespaces: vec![],
        };
        let limits = ParseLimits {
            source_bytes: bundle.sources[0].text.len() + 1,
        };
        let resources = ResourceSet::new_with_limits(&bundle, limits).unwrap();
        let classifier = DeclarationId {
            source: bundle.sources[0].id.clone(),
            path: vec![0],
        };
        let mut expected = MetadataQuery::EnumLiteral {
            classifier: classifier.clone(),
            name: name.into(),
        }
        .resolve(&bundle, limits);
        if padding > 0 {
            // The default import policy refuses the padded source, but the
            // driver reads the resources the compile loaded under its limits.
            assert!(expected.is_err());
            expected = Ok(Target::EnumLiteral {
                enumeration: classifier.clone(),
                literal: DeclarationId {
                    source: bundle.sources[0].id.clone(),
                    path: vec![0, 1],
                },
            });
        } else if name == "Missing" {
            assert_eq!(expected, Err("Unknown enum literal Missing".into()));
        } else {
            assert!(
                matches!(&expected, Ok(Target::EnumLiteral {enumeration, literal}) if enumeration == &classifier && literal.path == [0, if name == "First" {1} else {2}])
            );
        }
        let mut count = None;
        let mut receipt = None;
        for quantum in [1, 3, 256] {
            let bridge = MetadataBridge::default();
            bridge.0.lock().unwrap().request = Some(MetadataQuery::EnumLiteral {
                classifier: classifier.clone(),
                name: name.into(),
            });
            let mut driver = MetadataDriver::new(&bundle, resources.clone(), limits);
            let events = Rc::new(RefCell::new(vec![]));
            let mut advances = 0;
            let reply = loop {
                let moved = driver;
                driver = std::hint::black_box(moved);
                let lease = Rc::new(Meter {
                    events: events.clone(),
                    deny_at: None,
                });
                let weak = Rc::downgrade(&lease);
                let reply = {
                    let context = InvocationContext::new(lease.clone());
                    let mut reply = None;
                    for _ in 0..quantum {
                        advances += 1;
                        driver.advance(&bridge, &context).unwrap();
                        reply = bridge.0.lock().unwrap().reply.take();
                        if reply.is_some() {
                            break;
                        }
                    }
                    reply
                };
                drop(lease);
                assert!(weak.upgrade().is_none());
                if let Some(reply) = reply {
                    break reply;
                }
            };
            assert_eq!(reply, expected);
            assert_eq!(*count.get_or_insert(advances), advances);
            assert_eq!(
                *receipt.get_or_insert_with(|| events.borrow().clone()),
                *events.borrow()
            );
        }
    }
}

fn model_drive(
    bundle: &DefinitionBundle,
    expression: &Type,
    name: &str,
    resources: ResourceSet,
    limits: ParseLimits,
    quantum: usize,
) -> (FullOutcome, Events, usize) {
    let bridge = MetadataBridge::default();
    bridge.0.lock().unwrap().request = Some(MetadataQuery::ModelFeature {
        expression: expression.clone(),
        name: name.into(),
    });
    let mut driver = MetadataDriver::new(bundle, resources, limits);
    let events = Rc::new(RefCell::new(vec![]));
    let mut advances = 0;
    loop {
        driver = std::hint::black_box(driver);
        let lease = Rc::new(Meter {
            events: events.clone(),
            deny_at: None,
        });
        let weak = Rc::downgrade(&lease);
        let result = {
            let context = InvocationContext::new(lease.clone());
            let mut result = None;
            for _ in 0..quantum {
                advances += 1;
                assert!(advances < 100_000);
                if let Err(error) = driver.advance(&bridge, &context) {
                    result = Some(Err(error));
                    break;
                }
                if let Some(reply) = bridge.0.lock().unwrap().reply.take() {
                    result = Some(Ok(reply));
                    break;
                }
            }
            result
        };
        drop(lease);
        assert!(weak.upgrade().is_none());
        if let Some(result) = result {
            return (result, events, advances);
        }
    }
}
#[test]
fn model_query_driver_preserves_frozen_projection_identities_and_errors_with_fresh_leases() {
    let baseline: serde_json::Value =
        serde_json::from_str(include_str!("runtime_schema_baseline.json")).unwrap();
    for case in 0..12 {
        let (bundle, requested, name) =
            crate::definitions::runtime_schema_test_inputs::fixture(case);
        let expected = match baseline["model"][case]["cold_projection"]
            .as_object()
            .unwrap()
            .iter()
            .next()
            .unwrap()
        {
            (kind, value) if kind == "Ok" => Ok(Target::Feature(
                serde_json::from_value(value["declaration"].clone()).unwrap(),
            )),
            (_, value) => Err(value.as_str().unwrap().to_string()),
        };
        let resources = ResourceSet::new(&bundle).unwrap();
        let pointer = resources.document(&bundle.sources[0].id).unwrap() as *const _;
        let mut count = None;
        let mut receipt = None;
        for quantum in [1, 3, 256] {
            let (outcome, events, advances) = model_drive(
                &bundle,
                &requested[0],
                name,
                resources.clone(),
                ParseLimits { source_bytes: 1 },
                quantum,
            );
            assert_eq!(outcome.unwrap(), expected, "case {case}");
            assert_eq!(
                resources.document(&bundle.sources[0].id).unwrap() as *const _,
                pointer
            );
            assert_eq!(*count.get_or_insert(advances), advances);
            assert_eq!(
                *receipt.get_or_insert_with(|| events.borrow().clone()),
                *events.borrow()
            );
        }
    }
}
#[test]
fn model_query_driver_rechecks_default_sources_before_receiver_and_never_reuses_stale_inputs() {
    let (bundle, requested, name) = crate::definitions::runtime_schema_test_inputs::fixture(0);
    let resources = ResourceSet::new(&bundle).unwrap();
    for corruption in 0..3 {
        let mut invalid = bundle.clone();
        match corruption {
            0 => invalid.sources[0].text.push(' '),
            1 => invalid.sources.push(invalid.sources[0].clone()),
            _ => {
                invalid.sources[0] = DefinitionSource::new(
                    "memory:large-model-query.rcore",
                    SourceKind::Rcore,
                    format!("{}{}", " ".repeat(270_000), bundle.sources[0].text),
                );
            }
        }
        // Deliberately invalid receiver as well: original source-first precedence wins.
        let expression = Type::Classifier {
            declaration: DeclarationId {
                source: invalid.sources[0].id.clone(),
                path: vec![999],
            },
            arguments: vec![],
            raw: false,
        };
        let expected = MetadataQuery::ModelFeature {
            expression: expression.clone(),
            name: name.into(),
        }
        .resolve(
            &invalid,
            ParseLimits {
                source_bytes: 400_000,
            },
        );
        assert!(expected.is_err());
        let (observed, _, _) = model_drive(
            &invalid,
            &expression,
            name,
            resources.clone(),
            ParseLimits {
                source_bytes: 400_000,
            },
            3,
        );
        assert_eq!(observed.unwrap(), expected);
    }
    let expected = MetadataQuery::ModelFeature {
        expression: requested[0].clone(),
        name: name.into(),
    }
    .resolve(&bundle, ParseLimits::default());
    assert!(expected.is_ok());
    assert_eq!(
        model_drive(
            &bundle,
            &requested[0],
            name,
            resources,
            ParseLimits::default(),
            256
        )
        .0
        .unwrap(),
        expected
    );
}
#[test]
fn model_query_driver_cancellation_is_terminal_and_publishes_no_partial_reply() {
    let (bundle, requested, name) = crate::definitions::runtime_schema_test_inputs::fixture(0);
    let resources = ResourceSet::new(&bundle).unwrap();
    let (expected, _, count) = model_drive(
        &bundle,
        &requested[0],
        name,
        resources.clone(),
        ParseLimits::default(),
        1,
    );
    for prefix in [0, 1, 2, count / 4, count / 2, count - 2, count - 1] {
        let bridge = MetadataBridge::default();
        bridge.0.lock().unwrap().request = Some(MetadataQuery::ModelFeature {
            expression: requested[0].clone(),
            name: name.into(),
        });
        let mut driver = MetadataDriver::new(&bundle, resources.clone(), ParseLimits::default());
        for _ in 0..prefix {
            // Match the measured observed source-preparation profile. The existing
            // unobserved XML profile may complete individual stages sooner.
            let context = InvocationContext::new(Rc::new(Meter {
                events: Rc::new(RefCell::new(vec![])),
                deny_at: None,
            }));
            driver.advance(&bridge, &context).unwrap();
            assert!(bridge.0.lock().unwrap().reply.is_none());
        }
        let events = Rc::new(RefCell::new(vec![]));
        let lease = Rc::new(Meter {
            events,
            deny_at: Some(1),
        });
        let weak = Rc::downgrade(&lease);
        let error = {
            let context = InvocationContext::new(lease.clone());
            driver.advance(&bridge, &context).unwrap_err()
        };
        drop(lease);
        assert!(weak.upgrade().is_none());
        assert!(matches!(error, MetadataDriverError::Observation(_)));
        assert!(bridge.0.lock().unwrap().reply.is_none());
        assert_eq!(
            driver.advance(&bridge, &InvocationContext::default()),
            Err(error)
        );
    }
    assert_eq!(
        model_drive(
            &bundle,
            &requested[0],
            name,
            resources,
            ParseLimits::default(),
            3
        )
        .0,
        expected
    );
}
