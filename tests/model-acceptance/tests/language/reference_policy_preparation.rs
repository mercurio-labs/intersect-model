use intersect_model::core::model::Model;
use intersect_model::definitions::{
    bundle::{DefinitionBundle, DefinitionSource, SourceKind},
    compiler,
    conversion::{
        ConversionContract, ConversionProvider, ConversionRegistry, InvocationContext,
        InvocationObserver, ProviderId, ReferenceContext,
    },
    generic_types::{RuntimeType, Type},
    resources::DeclarationId,
};
use intersect_model::model_reference::ReferenceDomain;
use serde_json::{Value, json};
use std::{cell::Cell, rc::Rc};

struct Provider {
    contract: ConversionContract,
    calls: Cell<usize>,
}
impl ConversionProvider for Provider {
    fn contract(&self) -> &ConversionContract {
        &self.contract
    }
    fn parse(&self, _: &str) -> Result<Value, String> {
        self.calls.set(self.calls.get() + 1);
        Ok(json!("value"))
    }
    fn format(&self, _: &Value) -> Result<String, String> {
        self.calls.set(self.calls.get() + 1);
        Ok("value".into())
    }
    fn validate(&self, _: &Value) -> Result<(), String> {
        self.calls.set(self.calls.get() + 1);
        Ok(())
    }
    fn default_value(&self) -> Result<Option<Value>, String> {
        self.calls.set(self.calls.get() + 1);
        Ok(None)
    }
}
struct Probe {
    work: Cell<usize>,
    limit: Cell<usize>,
    failed: Cell<bool>,
}
impl InvocationObserver for Probe {
    fn before_invoke(&self, _: &ProviderId) -> Result<(), String> {
        panic!("metadata-only fixture must invoke no provider")
    }
    fn checkpoint(&self) -> Result<(), String> {
        if self.failed.get() {
            Err("policy admission cancelled".into())
        } else {
            Ok(())
        }
    }
    fn work(&self, units: usize) -> Result<(), String> {
        self.checkpoint()?;
        if self.work.get() + units > self.limit.get() {
            self.failed.set(true);
            return self.checkpoint();
        }
        self.work.set(self.work.get() + units);
        Ok(())
    }
}
fn probe() -> Rc<Probe> {
    Rc::new(Probe {
        work: Cell::new(0),
        limit: Cell::new(usize::MAX),
        failed: Cell::new(false),
    })
}

#[test]
fn source_policy_inventory_yields_all_post_compile_boundaries_and_rejects_before_target_or_model() {
    const WIDTH: usize = 16;
    let declarations = (0..WIDTH)
        .map(|n| {
            format!(
                r#"<eClassifiers x:type="e:EDataType" name="D{n}" instanceClassName="test.D{n}"/>"#
            )
        })
        .collect::<String>();
    let source = DefinitionSource::new(
        "memory:policy-admission.ecore",
        SourceKind::Ecore,
        format!(
            r#"<e:EPackage xmlns:e="http://www.eclipse.org/emf/2002/Ecore" xmlns:x="http://www.w3.org/2001/XMLSchema-instance" name="p" nsURI="urn:policy-admission">{declarations}<eClassifiers x:type="e:EClass" name="Node"/></e:EPackage>"#
        ),
    );
    let (source, declarations) =
        intersect_model_acceptance::migration_fixture::migrate_source_fixture(source);
    let id = |index| DeclarationId {
        source: source.id.clone(),
        path: vec![index],
    };
    let providers: Vec<_> = (0..WIDTH)
        .map(|n| Provider {
            contract: ConversionContract {
                datatype: declarations[&vec![n]].clone(),
                provider: ProviderId {
                    name: format!("test.D{n}"),
                    version: "1".into(),
                },
                serializable: true,
                declarations: vec![],
                members: vec![],
            },
            calls: Cell::new(0),
        })
        .collect();
    let bundle = DefinitionBundle {
        version: 1,
        sources: vec![source.clone()],
        namespaces: vec![],
    };
    let mut registry = ConversionRegistry::default();
    for provider in &providers {
        let datatype =
            compiler::import_datatype_bundle(&bundle, &provider.contract.datatype).unwrap();
        registry.register(&datatype, provider).unwrap();
    }
    let schema = compiler::compile_structure_bundle_with_conversions(
        &bundle,
        &source.id,
        "urn:policy-admission",
        &registry,
    )
    .unwrap();
    assert_eq!(schema.conversion_contracts.len(), WIDTH);
    let mut model = Model::default();
    model.create(&schema, "node", "Node", "r", 0, 0).unwrap();
    let mut context = ReferenceContext {
        bundle,
        entry: source.id.clone(),
        namespace: "urn:policy-admission".into(),
        conversions: schema.conversion_contracts.clone(),
        materialized_types: vec![],
        applied_conversions: vec![],
        constructors: vec![],
        model: serde_json::to_value(model).unwrap(),
    };
    context.conversions.reverse();
    let target = RuntimeType::new(Type::Classifier {
        declaration: declarations[&vec![WIDTH]].clone(),
        arguments: vec![],
        raw: false,
    })
    .unwrap();
    let before = context.clone();
    let semantic_calls = || {
        providers
            .iter()
            .map(|provider| provider.calls.get())
            .sum::<usize>()
    };
    let control = probe();
    let observation = InvocationContext::new(control.clone());
    let mut pending = ReferenceDomain::begin_runtime_observed(
        target.clone(),
        context.clone(),
        &registry,
        &observation,
    )
    .unwrap();
    // Compilation is still atomic. These counters cover post-compile admission.
    control.work.set(0);
    let before_calls = semantic_calls();
    let mut steps = 0;
    loop {
        let prior = control.work.get();
        let done = pending.advance(&registry, &observation).unwrap();
        if steps < WIDTH * WIDTH {
            assert!(!done);
            assert_eq!(
                control.work.get() - prior,
                1,
                "one policy comparison transition per advance"
            );
        }
        steps += 1;
        assert!(steps < 2000);
        if done {
            break;
        }
    }
    assert_eq!(semantic_calls(), before_calls);
    let total = control.work.get();
    let domain = pending.finish(&observation).unwrap();
    let expected =
        json!({"kind":"model_reference", "context": context.digest().unwrap(), "element":"node"});
    assert_eq!(domain.bind("node").unwrap(), expected);
    for stop in 0..total {
        let control = probe();
        let observation = InvocationContext::new(control.clone());
        let mut pending = ReferenceDomain::begin_runtime_observed(
            target.clone(),
            context.clone(),
            &registry,
            &observation,
        )
        .unwrap();
        control.work.set(0);
        control.limit.set(stop);
        let before_calls = semantic_calls();
        let error = loop {
            match pending.advance(&registry, &observation) {
                Ok(false) => {}
                Ok(true) => panic!("insufficient post-compile budget admitted context"),
                Err(error) => break error,
            }
        };
        assert_eq!(error, "policy admission cancelled", "budget {stop}");
        assert!(control.work.get() <= stop);
        assert_eq!(semantic_calls(), before_calls);
        assert_eq!(
            pending
                .advance(&registry, &InvocationContext::default())
                .unwrap_err(),
            "Reference admission has failed"
        );
        assert!(pending.finish(&InvocationContext::default()).is_err());
        assert_eq!(context, before);
    }
    let control = probe();
    let observation = InvocationContext::new(control.clone());
    let mut pending = ReferenceDomain::begin_runtime_observed(
        target.clone(),
        context.clone(),
        &registry,
        &observation,
    )
    .unwrap();
    control.work.set(0);
    control.limit.set(total);
    while !pending.advance(&registry, &observation).unwrap() {}
    assert!(pending.advance(&registry, &observation).unwrap());
    assert_eq!(control.work.get(), total);
    assert_eq!(
        pending.finish(&observation).unwrap().bind("node").unwrap(),
        expected
    );
    for mutation in 0..3 {
        let mut invalid = context.clone();
        match mutation {
            0 => {
                invalid.conversions.pop();
            }
            1 => invalid.conversions[WIDTH - 1] = invalid.conversions[0].clone(),
            _ => invalid.conversions[WIDTH - 1].provider.version = "foreign".into(),
        }
        invalid.model = Value::Null;
        let foreign_target = RuntimeType::new(Type::Classifier {
            declaration: id(999),
            arguments: vec![],
            raw: false,
        })
        .unwrap();
        let observation = InvocationContext::default();
        let mut pending = ReferenceDomain::begin_runtime_observed(
            foreign_target,
            invalid.clone(),
            &registry,
            &observation,
        )
        .unwrap();
        let error = loop {
            match pending.advance(&registry, &observation) {
                Ok(false) => {}
                Ok(true) => panic!("foreign policy admitted"),
                Err(error) => break error,
            }
        };
        assert_eq!(
            error,
            "Reference context converter selections differ from compiled schema"
        );
        assert!(pending.finish(&observation).is_err());
        assert_eq!(
            ReferenceDomain::new(id(999), invalid, &registry)
                .err()
                .unwrap(),
            error
        );
    }
}
