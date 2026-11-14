use intersect_model::definitions::{
    bundle::{DefinitionBundle, DefinitionSource, SourceKind},
    compiler,
    conversion::{
        ConversionContract, ConversionProvider, ConversionRegistry, InvocationContext,
        InvocationObserver, ProviderId,
    },
    generic_types::{RuntimeType, Type},
};
use serde_json::{Value, json};
use std::{cell::Cell, rc::Rc};

struct Provider {
    contract: ConversionContract,
    swallow: Cell<bool>,
    calls: Cell<usize>,
}
impl ConversionProvider for Provider {
    fn contract(&self) -> &ConversionContract {
        &self.contract
    }
    fn parse(&self, text: &str) -> Result<Value, String> {
        Ok(json!(text))
    }
    fn format(&self, value: &Value) -> Result<String, String> {
        Ok(value.to_string())
    }
    fn validate(&self, _: &Value) -> Result<(), String> {
        Ok(())
    }
    fn default_value(&self) -> Result<Option<Value>, String> {
        Ok(None)
    }
    fn validate_observed(
        &self,
        _: &Value,
        observation: &InvocationContext<'_>,
    ) -> Result<(), String> {
        self.calls.set(self.calls.get() + 1);
        let result = observation.work(1);
        if self.swallow.get() { Ok(()) } else { result }
    }
}
struct Probe<'a> {
    count: &'a Cell<usize>,
    fail_at: usize,
    failed: Cell<bool>,
    invocations: Cell<usize>,
}
impl InvocationObserver for Probe<'_> {
    fn before_invoke(&self, _: &ProviderId) -> Result<(), String> {
        self.checkpoint()?;
        self.invocations.set(self.invocations.get() + 1);
        Ok(())
    }
    fn checkpoint(&self) -> Result<(), String> {
        if self.failed.get() {
            Err("terminal observation".into())
        } else {
            Ok(())
        }
    }
    fn work(&self, units: usize) -> Result<(), String> {
        self.checkpoint()?;
        self.count.set(self.count.get() + units);
        if self.count.get() >= self.fail_at {
            self.failed.set(true);
        }
        self.checkpoint()
    }
}

#[test]
fn scoped_schema_validation_preserves_terminal_failures_and_registry_observers() {
    let source = DefinitionSource::new(
        "memory:schema-observation",
        SourceKind::Ecore,
        r##"<e:EPackage xmlns:e="http://www.eclipse.org/emf/2002/Ecore" xmlns:x="http://www.w3.org/2001/XMLSchema-instance" name="p" nsURI="urn:p">
<eClassifiers x:type="e:EDataType" name="D"/>
<eClassifiers x:type="e:EClass" name="C"><eStructuralFeatures x:type="e:EAttribute" name="value" eType="#//D"/></eClassifiers>
<eClassifiers x:type="e:EClass" name="Box"><eTypeParameters name="T"><eBounds eClassifier="#//D"/></eTypeParameters><eStructuralFeatures x:type="e:EAttribute" name="value"><eGenericType eTypeParameter="#//Box/T"/></eStructuralFeatures></eClassifiers>
</e:EPackage>"##,
    );
    let (source, migration) =
        intersect_model_acceptance::migration_fixture::migrate_source_fixture(source);
    let id = |index| migration[&vec![index]].clone();
    let provider = Provider {
        contract: ConversionContract {
            datatype: id(0),
            provider: ProviderId {
                name: "test.schema".into(),
                version: "1".into(),
            },
            serializable: true,
            declarations: vec![],
            members: vec![],
        },
        swallow: Cell::new(false),
        calls: Cell::new(0),
    };
    let raw = RuntimeType::new(Type::Classifier {
        declaration: id(2),
        arguments: vec![],
        raw: true,
    })
    .unwrap();
    let concrete = RuntimeType::new(Type::Classifier {
        declaration: id(1),
        arguments: vec![],
        raw: false,
    })
    .unwrap();
    let bundle = DefinitionBundle {
        version: 1,
        sources: vec![source.clone()],
        namespaces: vec![],
    };
    let datatype = compiler::import_datatype_bundle(&bundle, &id(0)).unwrap();
    let mut registry = ConversionRegistry::default();
    registry.register(&datatype, &provider).unwrap();
    let schema = compiler::materialize_runtime_bundle(
        &bundle,
        &source.id,
        "urn:p",
        &[concrete.clone(), raw.clone()],
        Some(&registry),
    )
    .unwrap();
    let value = serde_json::to_value(registry.parse(&id(0), "valid").unwrap()).unwrap();
    for receiver in [
        schema.class_key_for_type(&concrete).unwrap(),
        schema.class_key_for_type(&raw).unwrap(),
    ] {
        let field = schema.features(receiver).unwrap().remove("value").unwrap();
        schema
            .validate_runtime_attribute(receiver, &field, &value, Some(&registry))
            .unwrap();
        let count = Cell::new(0);
        let probe = Rc::new(Probe {
            count: &count,
            fail_at: usize::MAX,
            failed: Cell::new(false),
            invocations: Cell::new(0),
        });
        let observation = InvocationContext::new(probe.clone());
        schema
            .validate_runtime_attribute_observed(
                receiver,
                &field,
                &value,
                Some(&registry),
                &observation,
            )
            .unwrap();
        assert_eq!(probe.invocations.get(), 1);
        let total = count.get();
        assert!(total >= 3);
        for swallow in [false, true] {
            provider.swallow.set(swallow);
            for fail_at in 1..=total {
                let count = Cell::new(0);
                let observation = InvocationContext::new(Rc::new(Probe {
                    count: &count,
                    fail_at,
                    failed: Cell::new(false),
                    invocations: Cell::new(0),
                }));
                let error = schema
                    .validate_runtime_attribute_observed(
                        receiver,
                        &field,
                        &value,
                        Some(&registry),
                        &observation,
                    )
                    .unwrap_err();
                assert_eq!(
                    error, "terminal observation",
                    "{receiver} at {fail_at}, swallow={swallow}"
                );
                assert_eq!(count.get(), fail_at);
            }
        }
        // An operation observer never replaces or persists in the registry.
        schema
            .validate_runtime_attribute(receiver, &field, &value, Some(&registry))
            .unwrap();
        let count = Cell::new(0);
        let probe = Rc::new(Probe {
            count: &count,
            fail_at: usize::MAX,
            failed: Cell::new(false),
            invocations: Cell::new(0),
        });
        let stored = registry
            .clone()
            .with_observation(InvocationContext::new(probe.clone()));
        schema
            .validate_runtime_attribute(receiver, &field, &value, Some(&stored))
            .unwrap();
        assert_eq!(probe.invocations.get(), 1);
        assert_eq!(count.get(), 1);
    }
}
