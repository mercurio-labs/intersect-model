use super::*;
use crate::definitions::{
    bundle::{DefinitionSource, SourceKind},
    conversion::{InvocationContext, InvocationObserver, ProviderId},
    ecore::ParserObservation,
    runtime_schema_test_inputs,
};
use sha2::{Digest, Sha256};
use std::{cell::RefCell, rc::Rc};
struct Meter(RefCell<Vec<(char, usize)>>);
impl InvocationObserver for Meter {
    fn before_invoke(&self, _: &ProviderId) -> Result<(), String> {
        panic!("report preparation cannot invoke a provider")
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
pub(super) fn inputs(case: usize) -> (DefinitionBundle, RuntimeType) {
    if case < 12 {
        let (bundle, types, _) = runtime_schema_test_inputs::fixture(case);
        return (bundle, RuntimeType::new(types[0].clone()).unwrap());
    }
    let source = DefinitionSource::new(
        "memory:member-report-domains.ecore",
        SourceKind::Ecore,
        include_str!("../../assets/fixtures/ecore-generics/raw-feature-domains.ecore"),
    );
    let plain = |index, arguments, raw| Type::Classifier {
        declaration: DeclarationId {
            source: source.id.clone(),
            path: vec![index],
        },
        arguments,
        raw,
    };
    let expression = match case {
        12..=29 => plain(case - 12, vec![], true),
        30 => plain(5, vec![plain(1, vec![], false)], false),
        31 => plain(5, vec![plain(3, vec![], false)], false),
        32 => plain(12, vec![plain(11, vec![], false)], false),
        33 => plain(12, vec![plain(1, vec![], false)], false),
        34 => plain(
            17,
            vec![plain(0, vec![], false), plain(1, vec![], false)],
            false,
        ),
        _ => {
            let source = DefinitionSource::new(
                "memory:member-report-method.ecore",
                SourceKind::Ecore,
                r##"<e:EPackage xmlns:e="http://www.eclipse.org/emf/2002/Ecore" xmlns:x="http://www.w3.org/2001/XMLSchema-instance" name="p" nsURI="urn:report-method"><eClassifiers x:type="e:EClass" name="Base"/><eClassifiers x:type="e:EClass" name="Box"><eTypeParameters name="T"><eBounds eClassifier="#//Base"/></eTypeParameters><eStructuralFeatures x:type="e:EReference" name="value"><eGenericType eTypeParameter="#//Box/T"/></eStructuralFeatures><eOperations name="pick"><eTypeParameters name="U"><eBounds eClassifier="#//Base"/></eTypeParameters><eGenericType eTypeParameter="#//Box/T"/><eParameters name="input"><eGenericType eTypeParameter="#//Box/pick/U"/></eParameters></eOperations></eClassifiers></e:EPackage>"##,
            );
            let expression = Type::Classifier {
                declaration: DeclarationId {
                    source: source.id.clone(),
                    path: vec![1],
                },
                arguments: vec![],
                raw: true,
            };
            return (
                DefinitionBundle {
                    version: 1,
                    sources: vec![source],
                    namespaces: vec![],
                },
                RuntimeType::new(expression).unwrap(),
            );
        }
    };
    (
        DefinitionBundle {
            version: 1,
            sources: vec![source],
            namespaces: vec![],
        },
        RuntimeType::new(expression).unwrap(),
    )
}
fn row(case: usize, profile: usize) -> serde_json::Value {
    let (bundle, receiver) = inputs(case);
    let mut resolver = Resolver::new(&bundle).unwrap();
    if profile >= 2
        && let Err(error) = resolver.resolve_materialization_inventory_observed(
            &ParserObservation::new(&InvocationContext::default()),
        )
    {
        return serde_json::json!({"case":case,"profile":profile,"setup_error":format!("{error:?}")});
    }
    if profile >= 3 {
        let context = InvocationContext::default();
        let observation = ParserObservation::new(&context);
        drop(resolver.runtime_members_for_materialization_observed(&receiver, &observation));
    }
    let meter = Rc::new(Meter(RefCell::new(vec![])));
    let context = InvocationContext::new(meter.clone());
    let observation = ParserObservation::new(&context);
    let result = if profile == 0 || profile == 4 {
        observation
            .finish(resolver.runtime_members_observed(&receiver, &observation))
            .map(|v| serde_json::to_value(v).unwrap())
    } else {
        observation
            .finish(resolver.runtime_members_for_materialization_observed(&receiver, &observation))
            .map(|v| serde_json::to_value(v).unwrap())
    };
    let output = match result {
        Ok(v) => serde_json::json!({"ok":v}),
        Err(e) => serde_json::json!({"error":format!("{e:?}")}),
    };
    let events = meter.0.borrow();
    serde_json::json!({"case":case,"profile":profile,"output":output,"events_sha256":format!("{:x}",Sha256::digest(serde_json::to_vec(&*events).unwrap())),"work":events.iter().filter(|e|e.0=='w').map(|e|e.1).sum::<usize>(),"storage":events.iter().filter(|e|e.0=='s').map(|e|e.1).sum::<usize>(),"cache":format!("{:?}",resolver.prepared_erased_signatures.get().map(|v|v.borrow()))})
}
#[test]
fn runtime_members_capture_complete_reports_errors_fees_and_caches() {
    let rows = (0..36)
        .flat_map(|case| (0..5).map(move |profile| row(case, profile)))
        .collect::<Vec<_>>();
    let expected: serde_json::Value =
        serde_json::from_str(include_str!("generic_runtime_members_baseline.json")).unwrap();
    assert_eq!(serde_json::to_value(rows).unwrap(), expected);
}
