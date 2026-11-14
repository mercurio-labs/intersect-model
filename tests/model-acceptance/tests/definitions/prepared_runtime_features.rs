use intersect_model::definitions::{
    bundle::{DefinitionBundle, DefinitionSource, SourceKind},
    compiler,
    conversion::{
        ConversionContract, ConversionProvider, ConversionRegistry, InvocationContext,
        InvocationObserver, ProviderId,
    },
    dynamic::{ScalarType, SchemaBuilder},
    generic_types::{RuntimeType, Type},
    schema::UnresolvedRuntimeFeatureCandidate,
};
use serde_json::{Value, json};
use std::{
    cell::{Cell, RefCell},
    rc::Rc,
};

#[test]
fn inherited_fields_defaults_and_boundary_values_match_checked_entry_points() {
    let mut builder = SchemaBuilder::new("prepared", "urn:prepared");
    let base = builder.class("Base").unwrap();
    base.attribute("name", ScalarType::String)
        .default("default")
        .unwrap();
    base.attribute("count", ScalarType::Integer)
        .default(3)
        .unwrap();
    let child = builder.class("Child").unwrap();
    child.extends(&base).unwrap();
    child.reference("parent", &base).optional().unwrap();
    let dynamic = builder.build().unwrap();
    let schema = dynamic.as_schema();
    let prepared = schema.prepare_runtime_features("Child").unwrap();
    assert_eq!(prepared.fields(), &schema.features("Child").unwrap());
    for (name, field) in prepared.fields() {
        assert_eq!(
            prepared.require_attribute_conversion(name, None),
            schema.require_runtime_attribute_conversion("Child", field, None)
        );
        for value in [
            &field.default,
            &Value::Null,
            &json!("text"),
            &json!(true),
            &json!(-2147483648_i64),
            &json!(2147483647_i64),
            &json!(2147483648_i64),
            &json!([]),
            &json!({}),
        ] {
            assert_eq!(
                prepared.validate_attribute(name, value, None),
                schema.validate_runtime_attribute("Child", field, value, None),
                "{name}: {value}"
            );
        }
    }
    assert!(
        prepared
            .validate_attribute("name", &json!("valid"), None)
            .is_ok()
    );
    assert!(
        prepared
            .validate_attribute("count", &json!(2147483648_i64), None)
            .is_err()
    );
    assert_eq!(
        prepared.validate_attribute("parent", &json!("id"), None),
        Err("Reference is not an attribute".into())
    );
}

#[test]
fn prepared_names_are_canonical_and_legacy_fields_still_reject_forgery_and_staleness() {
    let mut builder = SchemaBuilder::new("prepared", "urn:prepared");
    builder
        .class("C")
        .unwrap()
        .attribute("value", ScalarType::String)
        .default("a")
        .unwrap();
    let mut schema = builder.build().unwrap().as_schema().clone();
    let old = schema.features("C").unwrap()["value"].clone();
    assert!(schema.prepare_runtime_features("unknown").is_err());
    {
        let prepared = schema.prepare_runtime_features("C").unwrap();
        assert!(
            prepared
                .validate_attribute("unknown", &json!(true), None)
                .is_err()
        );
        assert!(
            prepared
                .require_attribute_conversion("unknown", None)
                .is_err()
        );
        let mut forged = prepared.fields()["value"].clone();
        forged.target = "EBoolean".into();
        assert!(
            schema
                .validate_runtime_attribute("C", &forged, &json!(true), None)
                .is_err()
        );
        assert!(
            schema
                .require_runtime_attribute_conversion("C", &forged, None)
                .is_err()
        );
        // Editing a clone of the public view never changes the prepared predicate.
        assert!(
            prepared
                .validate_attribute("value", &json!(true), None)
                .is_err()
        );
    }
    schema
        .classes
        .get_mut("C")
        .unwrap()
        .features
        .get_mut("value")
        .unwrap()
        .default = json!("b");
    assert!(
        schema
            .validate_runtime_attribute("C", &old, &json!("value"), None)
            .is_err()
    );
    let prepared = schema.prepare_runtime_features("C").unwrap();
    assert_eq!(prepared.fields()["value"].default, json!("b"));
    assert!(
        prepared
            .validate_attribute("value", &json!("value"), None)
            .is_ok()
    );
    let mut changed = schema.clone();
    changed
        .classes
        .get_mut("C")
        .unwrap()
        .features
        .get_mut("value")
        .unwrap()
        .target = "EBoolean".into();
    assert!(
        changed
            .prepare_runtime_features("C")
            .unwrap()
            .validate_attribute("value", &json!(true), None)
            .is_ok()
    );
    assert!(
        prepared
            .validate_attribute("value", &json!(true), None)
            .is_err()
    );
}

struct Provider {
    contract: ConversionContract,
    trace: Rc<RefCell<Vec<String>>>,
    reject: Cell<bool>,
    swallow: Cell<bool>,
}
impl ConversionProvider for Provider {
    fn contract(&self) -> &ConversionContract {
        self.trace.borrow_mut().push("contract".into());
        &self.contract
    }
    fn parse(&self, text: &str) -> Result<Value, String> {
        Ok(json!(text))
    }
    fn format(&self, value: &Value) -> Result<String, String> {
        Ok(value.to_string())
    }
    fn default_value(&self) -> Result<Option<Value>, String> {
        Ok(None)
    }
    fn validate(&self, _: &Value) -> Result<(), String> {
        self.trace.borrow_mut().push("validate".into());
        if self.reject.get() {
            Err("provider rejected value".into())
        } else {
            Ok(())
        }
    }
    fn validate_observed(
        &self,
        value: &Value,
        observation: &InvocationContext<'_>,
    ) -> Result<(), String> {
        self.trace.borrow_mut().push("validate_observed".into());
        let result = observation.work(1);
        if !self.swallow.get() {
            result?;
        }
        self.validate(value)
    }
}
struct Probe {
    trace: Rc<RefCell<Vec<String>>>,
    count: Cell<usize>,
    fail_at: usize,
    failed: Cell<bool>,
}
impl InvocationObserver for Probe {
    fn before_invoke(&self, provider: &ProviderId) -> Result<(), String> {
        self.trace
            .borrow_mut()
            .push(format!("invoke:{}", provider.name));
        self.checkpoint()
    }
    fn checkpoint(&self) -> Result<(), String> {
        self.trace.borrow_mut().push("checkpoint".into());
        if self.failed.get() {
            Err("terminal observation".into())
        } else {
            Ok(())
        }
    }
    fn work(&self, units: usize) -> Result<(), String> {
        self.trace.borrow_mut().push(format!("work:{units}"));
        self.checkpoint()?;
        self.count.set(self.count.get() + units);
        if self.count.get() >= self.fail_at {
            self.failed.set(true);
        }
        self.checkpoint()
    }
}

#[test]
fn custom_and_generic_checks_preserve_provider_traces_and_every_observer_budget() {
    let source = DefinitionSource::new(
        "memory:prepared-observation",
        SourceKind::Ecore,
        r##"<e:EPackage xmlns:e="http://www.eclipse.org/emf/2002/Ecore" xmlns:x="http://www.w3.org/2001/XMLSchema-instance" name="p" nsURI="urn:p">
<eClassifiers x:type="e:EDataType" name="D"/>
<eClassifiers x:type="e:EClass" name="C"><eStructuralFeatures x:type="e:EAttribute" name="value" eType="#//D"/><eStructuralFeatures x:type="e:EAttribute" name="derived" derived="true" eType="#//D"/></eClassifiers>
<eClassifiers x:type="e:EClass" name="Box"><eTypeParameters name="T"><eBounds eClassifier="#//D"/></eTypeParameters><eStructuralFeatures x:type="e:EAttribute" name="value"><eGenericType eTypeParameter="#//Box/T"/></eStructuralFeatures></eClassifiers>
</e:EPackage>"##,
    );
    let (source, migration) =
        intersect_model_acceptance::migration_fixture::migrate_source_fixture(source);
    let id = |index| migration[&vec![index]].clone();
    let trace = Rc::new(RefCell::new(Vec::new()));
    let provider = Provider {
        contract: ConversionContract {
            datatype: id(0),
            provider: ProviderId {
                name: "test.prepared".into(),
                version: "1".into(),
            },
            serializable: true,
            declarations: vec![],
            members: vec![],
        },
        trace: trace.clone(),
        reject: Cell::new(false),
        swallow: Cell::new(false),
    };
    let bundle = DefinitionBundle {
        version: 1,
        sources: vec![source.clone()],
        namespaces: vec![],
    };
    let datatype = compiler::import_datatype_bundle(&bundle, &id(0)).unwrap();
    let mut registry = ConversionRegistry::default();
    registry.register(&datatype, &provider).unwrap();
    let receiver_type = |index, raw| {
        RuntimeType::new(Type::Classifier {
            declaration: id(index),
            arguments: vec![],
            raw,
        })
        .unwrap()
    };
    let concrete = receiver_type(1, false);
    let raw = receiver_type(2, true);
    let schema = compiler::materialize_runtime_bundle(
        &bundle,
        &source.id,
        "urn:p",
        &[concrete.clone(), raw.clone()],
        Some(&registry),
    )
    .unwrap();
    let value = serde_json::to_value(registry.parse(&id(0), "valid").unwrap()).unwrap();
    let concrete_key = schema.class_key_for_type(&concrete).unwrap().to_owned();
    let raw_key = schema.class_key_for_type(&raw).unwrap().to_owned();

    for receiver in [&concrete_key, &raw_key] {
        let prepared = schema.prepare_runtime_features(receiver).unwrap();
        for (name, field) in prepared.fields() {
            // Missing providers fail even for absent or derived attributes, and results aren't cached.
            assert_eq!(
                prepared.require_attribute_conversion(name, None),
                schema.require_runtime_attribute_conversion(receiver, field, None)
            );
            assert!(prepared.require_attribute_conversion(name, None).is_err());
            for conversions in [Some(&registry), None] {
                trace.borrow_mut().clear();
                let expected =
                    schema.require_runtime_attribute_conversion(receiver, field, conversions);
                let expected_trace = trace.borrow().clone();
                trace.borrow_mut().clear();
                assert_eq!(
                    prepared.require_attribute_conversion(name, conversions),
                    expected
                );
                assert_eq!(*trace.borrow(), expected_trace);
            }
            for reject in [false, true] {
                provider.reject.set(reject);
                for tested_value in [&value, &Value::Null, &json!("not a converted value")] {
                    for conversions in [Some(&registry), None] {
                        trace.borrow_mut().clear();
                        let expected = schema.validate_runtime_attribute(
                            receiver,
                            field,
                            tested_value,
                            conversions,
                        );
                        let expected_trace = trace.borrow().clone();
                        trace.borrow_mut().clear();
                        assert_eq!(
                            prepared.validate_attribute(name, tested_value, conversions),
                            expected
                        );
                        assert_eq!(*trace.borrow(), expected_trace);
                    }
                }
            }
            provider.reject.set(false);
            for swallow in [false, true] {
                provider.swallow.set(swallow);
                let run = |use_prepared: bool, fail_at| {
                    trace.borrow_mut().clear();
                    let probe = Rc::new(Probe {
                        trace: trace.clone(),
                        count: Cell::new(0),
                        fail_at,
                        failed: Cell::new(false),
                    });
                    let observation = InvocationContext::new(probe.clone());
                    let result = if use_prepared {
                        prepared.validate_attribute_observed(
                            name,
                            &value,
                            Some(&registry),
                            &observation,
                        )
                    } else {
                        schema.validate_runtime_attribute_observed(
                            receiver,
                            field,
                            &value,
                            Some(&registry),
                            &observation,
                        )
                    };
                    if probe.failed.get() {
                        assert_eq!(result, Err("terminal observation".into()));
                    }
                    // Preparation has its own work/checkpoints; provider invocation
                    // order and semantic outcomes remain shared.
                    let provider_trace: Vec<_> = trace
                        .borrow()
                        .iter()
                        .filter(|entry| {
                            entry.as_str() != "checkpoint" && !entry.starts_with("work:")
                        })
                        .cloned()
                        .collect();
                    (result, probe.count.get(), provider_trace)
                };
                let ordinary = run(false, usize::MAX);
                let cached = run(true, usize::MAX);
                assert_eq!(ordinary.0, cached.0, "{receiver}.{name}, swallow {swallow}");
                assert_eq!(ordinary.2, cached.2);
                assert!(cached.1 <= ordinary.1);
                for (use_prepared, baseline) in [(false, ordinary), (true, cached)] {
                    for fail_at in 1..=baseline.1 {
                        assert_eq!(
                            run(use_prepared, fail_at).0,
                            Err("terminal observation".into())
                        );
                    }
                    let recovered = run(use_prepared, usize::MAX);
                    assert_eq!(recovered, baseline);
                }
            }
            // Registry observation remains active when no per-operation observer is passed.
            let mut runs = Vec::new();
            for use_prepared in [false, true] {
                let probe = Rc::new(Probe {
                    trace: trace.clone(),
                    count: Cell::new(0),
                    fail_at: usize::MAX,
                    failed: Cell::new(false),
                });
                let observed = registry
                    .clone()
                    .with_observation(InvocationContext::new(probe));
                trace.borrow_mut().clear();
                let result = if use_prepared {
                    prepared.validate_attribute(name, &value, Some(&observed))
                } else {
                    schema.validate_runtime_attribute(receiver, field, &value, Some(&observed))
                };
                runs.push((result, trace.borrow().clone()));
            }
            assert_eq!(runs[0], runs[1]);
        }
    }

    // A prepared context is not a source-validation certificate: even retained
    // unresolved alternatives and unsupported codecs keep the original runtime errors.
    let mut unresolved = schema.clone();
    let domain = &mut unresolved
        .runtime_feature_domains
        .get_mut(&raw_key)
        .unwrap()[0];
    let target = domain.alternatives.remove(0);
    domain.unresolved.push(UnresolvedRuntimeFeatureCandidate {
        candidate: receiver_type(0, false),
        target,
        reason: "unproven candidate".into(),
    });
    let mut unknown_codec = schema.clone();
    unknown_codec
        .runtime_feature_domains
        .get_mut(&raw_key)
        .unwrap()[0]
        .alternatives
        .insert(0, "unknown-codec".into());
    for altered in [&unresolved, &unknown_codec] {
        let prepared = altered.prepare_runtime_features(&raw_key).unwrap();
        let expected = altered.validate_runtime_attribute(
            &raw_key,
            &prepared.fields()["value"],
            &value,
            Some(&registry),
        );
        assert!(expected.is_err());
        assert_eq!(
            prepared.validate_attribute("value", &value, Some(&registry)),
            expected
        );
    }
}
