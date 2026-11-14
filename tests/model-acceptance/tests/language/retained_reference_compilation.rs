use intersect_model::core::model::Model;
use intersect_model::definitions::{
    bundle::{DefinitionBundle, DefinitionSource, SourceKind},
    compiler::materialize_runtime_bundle,
    conversion::{
        ConversionRegistry, InvocationContext, InvocationObserver, ProviderId, ReferenceContext,
    },
    generic_types::{RuntimeType, Type},
    resources::DeclarationId,
    schema::SharedRuntimeSourceArrayTarget,
};
use intersect_model::model_reference::{ReferenceDomain, ReferenceDomainState};
use std::{cell::Cell, rc::Rc};
type SharedDomain = ReferenceDomainState<SharedRuntimeSourceArrayTarget>;
fn fixture() -> (ReferenceContext, RuntimeType) {
    let source = DefinitionSource::new(
        "memory:retained-reference.ecore",
        SourceKind::Ecore,
        "<e:EPackage xmlns:e='http://www.eclipse.org/emf/2002/Ecore' xmlns:x='http://www.w3.org/2001/XMLSchema-instance' name='m' nsURI='urn:retained-reference'><eClassifiers x:type='e:EClass' name='Root'><eTypeParameters name='T'><eBounds eClassifier='#//A'/></eTypeParameters><eStructuralFeatures x:type='e:EReference' name='r'><eGenericType eTypeParameter='#//Root/T'/></eStructuralFeatures></eClassifiers><eClassifiers x:type='e:EClass' name='A'/><eClassifiers x:type='e:EClass' name='B'/><eClassifiers x:type='e:EClass' name='Child' eSuperTypes='#//Root'/></e:EPackage>",
    );
    let (source, _) = intersect_model_acceptance::migration_fixture::migrate_source_fixture(source);
    let types = (0..4)
        .map(|i| {
            RuntimeType::new(Type::Classifier {
                declaration: DeclarationId {
                    source: source.id.clone(),
                    path: vec![i],
                },
                arguments: vec![],
                raw: i == 0,
            })
            .unwrap()
        })
        .collect::<Vec<_>>();
    let bundle = DefinitionBundle {
        version: 1,
        sources: vec![source.clone()],
        namespaces: vec![],
    };
    let schema =
        materialize_runtime_bundle(&bundle, &source.id, "urn:retained-reference", &types, None)
            .unwrap();
    let target = types[3].clone();
    let class = format!("__type_{}", target.identity().replace(':', "_"));
    let mut model = Model::default();
    model
        .create(&schema, "child", &class, "memory:instances", 0, 0)
        .unwrap();
    (
        ReferenceContext {
            bundle,
            entry: source.id,
            namespace: "urn:retained-reference".into(),
            conversions: vec![],
            materialized_types: types,
            applied_conversions: vec![],
            constructors: vec![],
            model: serde_json::to_value(model).unwrap(),
        },
        target,
    )
}
#[derive(Default)]
struct Control {
    cancelled: Cell<bool>,
    work: Cell<usize>,
    bytes: Cell<usize>,
}
impl InvocationObserver for Control {
    fn before_invoke(&self, _: &ProviderId) -> Result<(), String> {
        Err("Unexpected provider".into())
    }
    fn work(&self, n: usize) -> Result<(), String> {
        if self.cancelled.get() {
            return Err("Retained compiler cancelled".into());
        }
        self.work.set(self.work.get() + n);
        Ok(())
    }
    fn retain_storage(&self, n: usize) -> Result<(), String> {
        self.work(0)?;
        self.bytes.set(self.bytes.get() + n);
        Ok(())
    }
}
#[test]
fn retained_source_compilation_restores_exact_context_and_shared_domain_schema() {
    let (context, target) = fixture();
    let registry = ConversionRegistry::default();
    let observation = InvocationContext::default();
    let baseline = ReferenceDomain::new_runtime_observed(
        target.clone(),
        context.clone(),
        &registry,
        &observation,
    )
    .unwrap();
    let mut pending =
        SharedDomain::begin_runtime_observed(target, context.clone(), &registry, &observation)
            .unwrap();
    let mut advances = 0;
    while !pending.advance(&registry, &observation).unwrap() {
        advances += 1;
        assert!(advances < 20000);
    }
    assert!(advances > context.materialized_types.len());
    let domain = pending.finish(&observation).unwrap();
    assert_eq!(domain.context(), &context);
    assert_eq!(
        serde_json::to_value(domain.schema()).unwrap(),
        serde_json::to_value(baseline.schema()).unwrap()
    );
    assert_eq!(
        domain.bind("child").unwrap(),
        baseline.bind("child").unwrap()
    );
}
#[test]
fn retained_compilation_cancellation_is_terminal_and_fresh_admission_recovers() {
    let (context, target) = fixture();
    let registry = ConversionRegistry::default();
    for prefix in [0, 1, 2, 8, 32, 64] {
        let control = Rc::new(Control::default());
        let observation = InvocationContext::new(control.clone());
        let mut pending = SharedDomain::begin_runtime_observed(
            target.clone(),
            context.clone(),
            &registry,
            &observation,
        )
        .unwrap();
        for _ in 0..prefix {
            assert!(!pending.advance(&registry, &observation).unwrap());
        }
        control.cancelled.set(true);
        assert!(pending.advance(&registry, &observation).is_err());
        assert!(
            pending
                .advance(&registry, &InvocationContext::default())
                .is_err()
        );
        assert!(pending.finish(&InvocationContext::default()).is_err());
    }
    let observation = InvocationContext::default();
    let mut fresh =
        SharedDomain::begin_runtime_observed(target, context, &registry, &observation).unwrap();
    while !fresh.advance(&registry, &observation).unwrap() {}
    assert!(fresh.finish(&observation).unwrap().bind("child").is_ok());
}
#[test]
fn retained_compilation_cannot_publish_invalid_model_identity() {
    let (mut context, target) = fixture();
    context.model["elements"][0]["type_identity"] = serde_json::json!("forged");
    let registry = ConversionRegistry::default();
    let observation = InvocationContext::default();
    let mut pending =
        SharedDomain::begin_runtime_observed(target, context, &registry, &observation).unwrap();
    let mut advances = 0;
    loop {
        match pending.advance(&registry, &observation) {
            Err(_) => break,
            Ok(false) => {
                advances += 1;
                assert!(advances < 20000);
            }
            Ok(true) => panic!("invalid model admitted"),
        }
    }
    assert!(pending.advance(&registry, &observation).is_err());
    assert!(pending.finish(&observation).is_err());
}
