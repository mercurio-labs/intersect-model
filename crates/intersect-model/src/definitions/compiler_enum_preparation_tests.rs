//! Compatibility captures from 13e8144b before factory continuation changes.
use super::*;
use crate::definitions::{
    bundle::DefinitionBundle,
    conversion::{InvocationContext, InvocationObserver, ProviderId},
    ecore::ParserObservation,
    resources::{DeclarationId, ResourceSet},
};
use std::{cell::RefCell, rc::Rc};
#[derive(Default)]
struct Meter {
    events: Rc<RefCell<Vec<(char, usize)>>>,
    deny_at: Option<usize>,
}
impl InvocationObserver for Meter {
    fn before_invoke(&self, _: &ProviderId) -> Result<(), String> {
        panic!("enum import cannot invoke providers")
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
impl Meter {
    fn record(&self, kind: char, units: usize) -> Result<(), String> {
        let mut events = self.events.borrow_mut();
        events.push((kind, units));
        if self.deny_at == Some(events.len()) {
            return Err("enum factory denied".into());
        }
        Ok(())
    }
}
fn fixture(case: usize) -> DefinitionBundle {
    let text = if case == 15 {
        "rcore 1;package p version \"1\";metamodel \"urn:enum-retained\" prefix p {enum Mode {First = 0;Second = 1;}}".into()
    } else {
        let (header, body) = match case {
            0 => (
                "name='Mode'",
                "<eLiterals name='First'/><eLiterals name='Alias' value='0' literal=''/><eLiterals name='Low' value='-2147483648'/><eLiterals name='High' value='2147483647'/>",
            ),
            1 => (
                "name='Mode' serializable='false' instanceClassName='java.Mode' instanceTypeName='p.Mode' xmi:id='enum-id'",
                "<eAnnotations source='urn:opaque'><details key='k' value='v'/></eAnnotations><eLiterals name='First' value='7'><eAnnotations source='urn:literal'><details key='a' value='b'/></eAnnotations></eLiterals>",
            ),
            2 => (
                "name='Mode'",
                "<eLiterals name='Same'/><eLiterals name='Same' value='1'/>",
            ),
            3 => (
                "name='Mode'",
                "<eLiterals name='First' literal='x'/><eLiterals name='Second' literal='x'/>",
            ),
            4 => ("name='Mode'", "<eLiterals value='1'/>"),
            5 => ("name='Mode'", "<eLiterals name='Bad' value='2147483648'/>"),
            6 => (
                "name='Mode' serializable='bad'",
                "<eLiterals value='not-a-number'/>",
            ),
            7 => ("name='Mode'", "<eGenericSuperTypes/>"),
            8 => ("name='Mode' extra='retained?'", "<eLiterals name='First'/>"),
            9 => ("serializable='bad'", "<eLiterals name='First'/>"),
            10 => ("name='Mode'", "<eLiterals x:type='e:EClass' name='First'/>"),
            11 => (
                "name='Mode'",
                "<eLiterals name='First'><eTypeParameters name='T'/></eLiterals>",
            ),
            12 => ("name='Mode'", "<eLiterals name=''/>"),
            13 => ("name='' serializable='bad'", "<eLiterals name='First'/>"),
            14 => ("name='Mode'", ""),
            _ => unreachable!(),
        };
        format!(
            "<e:EPackage xmlns:e='http://www.eclipse.org/emf/2002/Ecore' xmlns:x='http://www.w3.org/2001/XMLSchema-instance' xmlns:xmi='http://www.omg.org/XMI' name='p' nsURI='urn:enum-retained'><eClassifiers x:type='e:EEnum' {header}>{body}</eClassifiers></e:EPackage>"
        )
    };
    DefinitionBundle {
        version: 1,
        sources: vec![DefinitionSource::new(
            "memory:enum-retained",
            if case == 15 {
                SourceKind::Rcore
            } else {
                SourceKind::Ecore
            },
            text,
        )],
        namespaces: vec![],
    }
}
fn snapshot(xml: &Xml) -> Value {
    serde_json::json!({"name":xml.name,"attrs":xml.attrs,"attr_spans":xml.attr_spans,"children":xml.children.iter().map(snapshot).collect::<Vec<_>>(),"opening":xml.opening,"payload":xml.payload,"declaration":xml.declaration,"source_payload":xml.source_payload})
}
#[test]
fn complete_enum_factory_outputs_visitors_and_ordered_fees_match_recursive_capture() {
    let baseline: Value =
        serde_json::from_str(include_str!("compiler_enum_preparation_baseline.json")).unwrap();
    for case in 0..16 {
        let bundle = fixture(case);
        let resources = match ResourceSet::new_diagnostic(&bundle) {
            Ok(resources) => resources,
            Err(error) => {
                let row = serde_json::json!({"case":case,"resource_error":format!("{error:?}")});
                assert_eq!(row, baseline["observations"][case]);
                continue;
            }
        };
        let declaration = DeclarationId {
            source: bundle.sources[0].id.clone(),
            path: vec![0],
        };
        let node = resources.node(&declaration).unwrap();
        let document = resources.document(&declaration.source).unwrap();
        let meter = Rc::new(Meter::default());
        let invocation = InvocationContext::new(meter.clone());
        let observation = ParserObservation::new(&invocation);
        let visited = RefCell::new(vec![]);
        let xml = Xml::from_node_at_observed_with(
            node,
            &document.source.text,
            declaration.path.clone(),
            &observation,
            &|xml| {
                visited.borrow_mut().push(xml.declaration.clone());
                Ok(())
            },
        )
        .unwrap();
        let tree = snapshot(&xml);
        let mut xml = xml;
        xml.attrs.remove("xmi:id");
        let result = observation.finish(lower_enumeration_observed(&xml, Some(&observation)));
        let output = match result {
            Ok(value) => serde_json::json!({"ok":value}),
            Err(error) => serde_json::json!({"error":format!("{error:?}")}),
        };
        let events = meter.events.borrow();
        let payload = serde_json::to_vec(&*events).unwrap();
        let row = serde_json::json!({"case":case,"output":output,"xml":tree,"postorder":*visited.borrow(),"events_sha256":format!("{:x}",Sha256::digest(payload)),"work":events.iter().filter(|e|e.0=='w').map(|e|e.1).sum::<usize>(),"storage":events.iter().filter(|e|e.0=='s').map(|e|e.1).sum::<usize>()});
        assert_eq!(row, baseline["observations"][case]);
    }
}

type Events = Rc<RefCell<Vec<(char, usize)>>>;
fn drive(
    mut step: impl FnMut(&ParserObservation<'_, '_>) -> Result<bool, ImportError>,
    events: &Events,
    quantum: usize,
    deny_at: Option<usize>,
) -> Result<usize, crate::definitions::ecore::ObservedImportError> {
    let mut advances = 0;
    loop {
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
                result = observation.finish(step(&observation));
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
            Ok(true) => return Ok(advances),
            Err(error) => return Err(error),
        }
    }
}
type FactoryOutcome = Result<Value, crate::definitions::ecore::ObservedImportError>;
fn retained(
    case: usize,
    quantum: usize,
    deny_at: Option<usize>,
) -> (FactoryOutcome, Events, usize) {
    let bundle = fixture(case);
    let events = Rc::new(RefCell::new(vec![]));
    let resources = match ResourceSet::new_diagnostic(&bundle) {
        Ok(resources) => resources,
        Err(error) => {
            return (
                Ok(serde_json::json!({"case":case,"resource_error":format!("{error:?}")})),
                events,
                0,
            );
        }
    };
    let declaration = DeclarationId {
        source: bundle.sources[0].id.clone(),
        path: vec![0],
    };
    let node = resources.node(&declaration).unwrap();
    let document = resources.document(&declaration.source).unwrap();
    let visited = RefCell::new(vec![]);
    let mut projection = Some(XmlProjectionPreparation::begin(
        node,
        &document.source.text,
        declaration.path.clone(),
    ));
    let steps = drive(
        |observation| {
            let mut state = std::hint::black_box(projection.take().unwrap());
            let lease = Rc::new(());
            let weak = Rc::downgrade(&lease);
            let visits = &visited;
            let visitor = move |xml: &Xml| {
                let _keep = &lease;
                visits.borrow_mut().push(xml.declaration.clone());
                Ok(())
            };
            let result = state.advance(observation, &visitor);
            drop(visitor);
            assert!(weak.upgrade().is_none());
            projection = Some(std::hint::black_box(state));
            result
        },
        &events,
        quantum,
        deny_at,
    );
    let steps = match steps {
        Ok(steps) => steps,
        Err(error) => {
            assert!(projection.take().unwrap().finish().is_err());
            return (Err(error), events, 0);
        }
    };
    let mut xml = projection.take().unwrap().finish().unwrap();
    let tree = snapshot(&xml);
    xml.attrs.remove("xmi:id");
    let mut lowering = Some(EnumLoweringPreparation::begin(&xml));
    let lower_steps = drive(
        |observation| {
            let mut state = std::hint::black_box(lowering.take().unwrap());
            let result = state.advance(Some(observation));
            lowering = Some(std::hint::black_box(state));
            result
        },
        &events,
        quantum,
        deny_at,
    );
    let (output, lower_steps) = match lower_steps {
        Ok(count) => (
            serde_json::json!({"ok":lowering.take().unwrap().finish().unwrap()}),
            count,
        ),
        Err(error @ crate::definitions::ecore::ObservedImportError::Observation(_)) => {
            assert!(lowering.take().unwrap().finish().is_err());
            return (Err(error), events, steps);
        }
        Err(error) => {
            assert!(lowering.take().unwrap().finish().is_err());
            (serde_json::json!({"error":format!("{error:?}")}), 0)
        }
    };
    let row = {
        let receipts = events.borrow();
        serde_json::json!({"case":case,"output":output,"xml":tree,"postorder":*visited.borrow(),"events_sha256":format!("{:x}",Sha256::digest(serde_json::to_vec(&*receipts).unwrap())),"work":receipts.iter().filter(|e|e.0=='w').map(|e|e.1).sum::<usize>(),"storage":receipts.iter().filter(|e|e.0=='s').map(|e|e.1).sum::<usize>()})
    };
    (Ok(row), events, steps + lower_steps)
}
#[test]
fn retained_enum_factory_matches_frozen_results_fees_and_visitors_with_fresh_leases() {
    let baseline: Value =
        serde_json::from_str(include_str!("compiler_enum_preparation_baseline.json")).unwrap();
    for case in 0..16 {
        let mut count = None;
        for quantum in [1, 3, 256] {
            let (row, _, advances) = retained(case, quantum, None);
            assert_eq!(row.unwrap(), baseline["observations"][case]);
            assert_eq!(*count.get_or_insert(advances), advances);
        }
    }
}
#[test]
fn retained_enum_factory_denies_every_callback_and_releases_observers_and_visitors() {
    let (expected, events, _) = retained(0, 1, None);
    assert!(expected.is_ok());
    for callback in 1..=events.borrow().len() {
        let (denied, prefix, _) = retained(0, 3, Some(callback));
        assert_eq!(
            denied,
            Err(crate::definitions::ecore::ObservedImportError::Observation(
                "enum factory denied".into()
            ))
        );
        assert_eq!(*prefix.borrow(), events.borrow()[..callback]);
    }
    assert_eq!(retained(0, 256, None).0, expected);
}
#[test]
fn complete_retained_enum_import_preserves_all_frozen_source_results_and_quanta() {
    let baseline: Value =
        serde_json::from_str(include_str!("compiler_enum_preparation_baseline.json")).unwrap();
    for case in 0..16 {
        let bundle = fixture(case);
        let declaration = DeclarationId {
            source: bundle.sources[0].id.clone(),
            path: vec![0],
        };
        let mut count = None;
        let mut receipt = None;
        for quantum in [1, 3, 256] {
            let events = Rc::new(RefCell::new(vec![]));
            let initial = Rc::new(Meter {
                events: events.clone(),
                deny_at: None,
            });
            let weak = Rc::downgrade(&initial);
            let mut preparation = {
                let context = InvocationContext::new(initial.clone());
                EnumerationPreparation::begin(&bundle, declaration.clone(), &context).unwrap()
            };
            drop(initial);
            assert!(weak.upgrade().is_none());
            let mut advances = 0;
            let result = loop {
                let moved = preparation;
                preparation = std::hint::black_box(moved);
                let lease = Rc::new(Meter {
                    events: events.clone(),
                    deny_at: None,
                });
                let weak = Rc::downgrade(&lease);
                let ready = {
                    let context = InvocationContext::new(lease.clone());
                    let mut ready = Ok(false);
                    for _ in 0..quantum {
                        advances += 1;
                        ready = preparation.advance(&context);
                        if ready != Ok(false) {
                            break;
                        }
                    }
                    ready
                };
                drop(lease);
                assert!(weak.upgrade().is_none());
                match ready {
                    Ok(false) => {}
                    Ok(true) => break preparation.finish(),
                    Err(error) => {
                        assert_eq!(
                            preparation.advance(&InvocationContext::default()),
                            Err(error.clone())
                        );
                        assert_eq!(preparation.finish(), Err(error.clone()));
                        break Err(error);
                    }
                }
            };
            if let Some(expected) = baseline["observations"][case].get("resource_error") {
                let Err(crate::definitions::ecore::ObservedImportError::Import(error)) = result
                else {
                    panic!("resource admission must fail first")
                };
                assert_eq!(format!("{error:?}"), expected.as_str().unwrap());
            } else {
                let actual = match result {
                    Ok(value) => serde_json::json!({"ok":value}),
                    Err(error) => serde_json::json!({"error":format!("{error:?}")}),
                };
                assert_eq!(actual, baseline["observations"][case]["output"]);
            }
            assert_eq!(*count.get_or_insert(advances), advances);
            assert_eq!(
                *receipt.get_or_insert_with(|| events.borrow().clone()),
                *events.borrow()
            );
        }
    }
}

#[test]
fn enum_import_drop_prefixes_ready_cancellation_and_fresh_recovery_preserve_old_values() {
    let bundle = fixture(15);
    let declaration = DeclarationId {
        source: bundle.sources[0].id.clone(),
        path: vec![0],
    };
    let prior = import_enum_bundle(&bundle, &declaration).unwrap();
    let mut job =
        EnumerationPreparation::begin(&bundle, declaration.clone(), &InvocationContext::default())
            .unwrap();
    let mut count = 0;
    loop {
        count += 1;
        if job.advance(&InvocationContext::default()).unwrap() {
            break;
        }
    }
    assert_eq!(job.finish().unwrap(), prior);
    for prefix in 0..count {
        let mut job = EnumerationPreparation::begin(
            &bundle,
            declaration.clone(),
            &InvocationContext::default(),
        )
        .unwrap();
        for _ in 0..prefix {
            assert!(!job.advance(&InvocationContext::default()).unwrap());
        }
        assert!(job.finish().is_err());
    }
    let mut ready =
        EnumerationPreparation::begin(&bundle, declaration.clone(), &InvocationContext::default())
            .unwrap();
    while !ready.advance(&InvocationContext::default()).unwrap() {}
    let events = Rc::new(RefCell::new(vec![]));
    let context = InvocationContext::new(Rc::new(Meter {
        events,
        deny_at: Some(1),
    }));
    let error = ready.advance(&context).unwrap_err();
    assert!(matches!(
        error,
        crate::definitions::ecore::ObservedImportError::Observation(_)
    ));
    assert_eq!(
        ready.advance(&InvocationContext::default()),
        Err(error.clone())
    );
    assert_eq!(ready.finish(), Err(error));
    assert_eq!(import_enum_bundle(&bundle, &declaration).unwrap(), prior);
}
