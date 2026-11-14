use super::*;
use crate::definitions::{
    bundle::DefinitionSource,
    conversion::{InvocationContext, InvocationObserver, ProviderId},
    ecore::ParserObservation,
};
use std::{cell::Cell, rc::Rc};

fn fixture() -> (DefinitionBundle, DeclarationId) {
    let source = DefinitionSource::new(
        "memory:operation-inputs.ecore",
        SourceKind::Ecore,
        "<e:EPackage xmlns:e='http://www.eclipse.org/emf/2002/Ecore' xmlns:x='http://www.w3.org/2001/XMLSchema-instance' name='m' nsURI='urn:operation-inputs'><eClassifiers x:type='e:EClass' name='Base'/><eClassifiers x:type='e:EClass' name='Other'/><eClassifiers x:type='e:EClass' name='Holder'><eTypeParameters name='T'><eBounds eClassifier='#//Base'/></eTypeParameters></eClassifiers></e:EPackage>",
    );
    let id = DeclarationId {
        source: source.id.clone(),
        path: vec![2, 0],
    };
    (
        DefinitionBundle {
            version: 1,
            sources: vec![source],
            namespaces: vec![],
        },
        id,
    )
}
fn state(resolver: &Resolver, cached: bool, remaining: usize) -> Checks<'_> {
    Checks {
        classifiers: None,
        parameter_inputs: cached.then(|| OperationParameterInputs {
            resolver,
            rows: std::collections::BTreeMap::new(),
        }),
        seen: BTreeSet::new(),
        shared_seen: None,
        bounds: vec![],
        warnings: CheckWarnings::Owned(vec![]),
        remaining,
    }
}
fn ty(id: &DeclarationId, index: usize, arguments: Vec<Type>, raw: bool) -> Type {
    Type::Classifier {
        declaration: DeclarationId {
            source: id.source.clone(),
            path: vec![index],
        },
        arguments,
        raw,
    }
}
#[test]
fn operation_inputs_preserve_fresh_bound_reports_errors_and_aggregate_budget() {
    let (bundle, id) = fixture();
    let resolver = Resolver::new(&bundle).unwrap();
    let span = fallback(&id);
    let base = ty(&id, 0, vec![], false);
    let good = ty(&id, 2, vec![base], false);
    let bad = ty(&id, 2, vec![ty(&id, 1, vec![], false)], false);
    let raw = ty(&id, 2, vec![], true);
    let wrong_arity = ty(&id, 2, vec![], false);
    let unknown = ty(&id, 2, vec![ty(&id, 99, vec![], false)], false);
    for remaining in [1, 3, 16384] {
        for tail in [&good, &bad, &raw, &wrong_arity, &unknown] {
            let run = |cached| {
                let mut checks = state(&resolver, cached, remaining);
                let mut results = vec![];
                for actual in [&good, tail, &good] {
                    checks.seen.clear();
                    results.push(resolver.check_application(actual, &span, 0, &mut checks));
                }
                (
                    results,
                    checks.bounds,
                    checks
                        .warnings
                        .into_owned_observed(
                            &span,
                            &ParserObservation::new(&InvocationContext::default()),
                        )
                        .unwrap(),
                    checks.remaining,
                )
            };
            assert_eq!(run(true), run(false));
        }
    }
}
#[test]
fn operation_inputs_reject_foreign_resolver_and_changed_source_after_warming() {
    let (bundle, id) = fixture();
    let mut resolver = Resolver::new(&bundle).unwrap();
    let foreign = Resolver::new(&bundle).unwrap();
    let context = InvocationContext::default();
    let observation = ParserObservation::new(&context);
    let mut checks = state(&resolver, true, 16384);
    let resolved = resolver
        .operation_parameter_input(&id, &mut checks, &observation)
        .unwrap();
    assert_eq!(*resolved, resolver.resolve_parameter(&id).unwrap());
    assert!(
        foreign
            .operation_parameter_input(&id, &mut checks, &observation)
            .err()
            .unwrap()
            .message
            .contains("another source resolver")
    );
    // Keep the borrowed owner stable while replacing only its resources, as in
    // the existing source-revision invalidation controls.
    let inputs = checks.parameter_inputs.take().unwrap().rows;
    drop(checks);
    let changed = DefinitionSource::new(
        id.source.uri.clone(),
        SourceKind::Ecore,
        bundle.sources[0]
            .text
            .replace("name='Base'", "name='Changed'"),
    );
    resolver.resources = ResourceSet::new(&DefinitionBundle {
        version: 1,
        sources: vec![changed],
        namespaces: vec![],
    })
    .unwrap();
    let mut checks = state(&resolver, true, 16384);
    checks.parameter_inputs.as_mut().unwrap().rows = inputs;
    assert!(
        resolver
            .operation_parameter_input(&id, &mut checks, &observation)
            .is_err()
    );
}
struct Meter {
    cancel: Cell<bool>,
    deny: Cell<bool>,
    work: Cell<usize>,
    storage: Cell<usize>,
}
impl InvocationObserver for Meter {
    fn before_invoke(&self, _: &ProviderId) -> Result<(), String> {
        Err("No provider expected".into())
    }
    fn work(&self, n: usize) -> Result<(), String> {
        if self.cancel.get() {
            return Err("Cancelled".into());
        }
        self.work.set(self.work.get() + n);
        Ok(())
    }
    fn retain_storage(&self, n: usize) -> Result<(), String> {
        if self.deny.get() {
            return Err("Storage denied".into());
        }
        self.storage.set(self.storage.get() + n);
        Ok(())
    }
}
#[test]
fn denied_operation_input_is_not_published_and_hot_lookup_remains_observed() {
    let (bundle, id) = fixture();
    let resolver = Resolver::new(&bundle).unwrap();
    let meter = Rc::new(Meter {
        cancel: Cell::new(false),
        deny: Cell::new(true),
        work: Cell::new(0),
        storage: Cell::new(0),
    });
    let context = InvocationContext::new(meter.clone());
    let observation = ParserObservation::new(&context);
    let mut checks = state(&resolver, true, 16384);
    assert!(
        resolver
            .operation_parameter_input(&id, &mut checks, &observation)
            .is_err()
    );
    assert!(checks.parameter_inputs.as_ref().unwrap().rows.is_empty());
    meter.deny.set(false);
    resolver
        .operation_parameter_input(&id, &mut checks, &observation)
        .unwrap();
    let (work, storage) = (meter.work.get(), meter.storage.get());
    resolver
        .operation_parameter_input(&id, &mut checks, &observation)
        .unwrap();
    assert!(meter.work.get() > work);
    assert!(meter.storage.get() > storage);
    meter.cancel.set(true);
    assert!(
        resolver
            .operation_parameter_input(&id, &mut checks, &observation)
            .is_err()
    );
    meter.cancel.set(false);
    meter.deny.set(true);
    assert!(
        resolver
            .operation_parameter_input(&id, &mut checks, &observation)
            .is_err()
    );
}
