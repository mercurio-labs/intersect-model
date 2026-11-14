use intersect_model::definitions::{
    bundle::{DefinitionBundle, DefinitionSource, SourceKind},
    compiler,
    conversion::{
        AppliedConversionContract, ConversionContract, ConversionProvider, ConversionRegistry,
        InvocationContext, InvocationObserver, ProviderId,
    },
    generic_resolution::Resolver,
    generic_types::{ClosedType, Type},
    resources::DeclarationId,
    schema::{
        AppliedDatatypeContractPreparation, AttributeConversionPreparation, Feature,
        FeatureMapPreparation, PreparationError, RuntimeAttributeConversionPreparation,
        RuntimeAttributeValidationPreparation, Schema, UnresolvedRuntimeFeatureCandidate,
    },
};
use serde_json::{Value, json};
use std::{cell::Cell, rc::Rc};

struct MetadataProbe {
    storage: Cell<usize>,
    limit: usize,
    checkpoints: Cell<usize>,
    stop: usize,
}
impl InvocationObserver for MetadataProbe {
    fn before_invoke(&self, _: &ProviderId) -> Result<(), String> {
        panic!("codec qualification must not invoke provider semantics")
    }
    fn checkpoint(&self) -> Result<(), String> {
        let next = self.checkpoints.get();
        if next == self.stop {
            return Err("metadata cancelled".into());
        }
        self.checkpoints.set(next + 1);
        Ok(())
    }
    fn retain_storage(&self, bytes: usize) -> Result<(), String> {
        let next = self
            .storage
            .get()
            .checked_add(bytes)
            .filter(|next| *next <= self.limit)
            .ok_or("metadata storage denied")?;
        self.storage.set(next);
        Ok(())
    }
}
fn metadata_probe(limit: usize, stop: usize) -> Rc<MetadataProbe> {
    Rc::new(MetadataProbe {
        storage: Cell::new(0),
        limit,
        checkpoints: Cell::new(0),
        stop,
    })
}

#[test]
fn observed_ordinary_codec_copies_contract_once_and_cancellation_is_terminal() {
    let (bundle, provider) = fixture();
    let datatype = compiler::import_datatype_bundle(&bundle, &provider.contract.datatype).unwrap();
    let mut registry = ConversionRegistry::default();
    registry.register(&datatype, &provider).unwrap();
    let mut schema = compiler::compile_structure_bundle_with_conversions(
        &bundle,
        &bundle.sources[0].id,
        "urn:attribute-cursor",
        &registry,
    )
    .unwrap();
    let field = schema.features("Child").unwrap()["f63"].clone();
    let unrelated = (0..64)
        .map(|n| {
            let mut contract = provider.contract.clone();
            contract.datatype.path = vec![n + 10];
            contract
        })
        .collect::<Vec<_>>();
    schema.conversion_contracts = unrelated;
    schema
        .conversion_contracts
        .insert(32, provider.contract.clone());
    let provider_calls_before_qualification = provider.calls.get();
    let baseline = metadata_probe(usize::MAX, usize::MAX);
    let observation = InvocationContext::new(baseline.clone());
    let mut pending = AttributeConversionPreparation::new_observed(&field, &observation).unwrap();
    let field_bytes = baseline.storage.get();
    assert!(
        !pending
            .advance_observed(&schema, Some(&registry), &observation)
            .unwrap()
    );
    let required = baseline.storage.get();
    assert!(required > field_bytes);
    while !pending
        .advance_observed(&schema, Some(&registry), &observation)
        .unwrap()
    {}
    pending.finish().unwrap();
    assert_eq!(
        baseline.storage.get(),
        required,
        "contract table scans do not recopy metadata"
    );
    for limit in [
        0,
        field_bytes - 1,
        field_bytes,
        field_bytes + 1,
        required - 1,
        required,
        required + 1,
    ] {
        let budget = metadata_probe(limit, usize::MAX);
        let observation = InvocationContext::new(budget.clone());
        let result = match AttributeConversionPreparation::new_observed(&field, &observation) {
            Err(error) => Err(error),
            Ok(mut pending) => loop {
                match pending.advance_observed(&schema, Some(&registry), &observation) {
                    Ok(false) => {}
                    Ok(true) => break pending.finish().map_err(PreparationError::Schema),
                    Err(error) => {
                        assert!(pending.advance(&schema, Some(&registry)).is_err());
                        assert!(pending.finish().is_err());
                        break Err(error);
                    }
                }
            },
        };
        if limit < required {
            assert_eq!(
                result.unwrap_err(),
                PreparationError::Observation("metadata storage denied".into())
            );
            assert_eq!(
                budget.storage.get(),
                if limit < field_bytes { 0 } else { field_bytes }
            );
        } else {
            result.unwrap();
            assert_eq!(budget.storage.get(), required);
        }
    }
    for stop in 0..baseline.checkpoints.get() {
        let observation = InvocationContext::new(metadata_probe(usize::MAX, stop));
        match AttributeConversionPreparation::new_observed(&field, &observation) {
            Err(error) => assert_eq!(
                error,
                PreparationError::Observation("metadata cancelled".into())
            ),
            Ok(mut pending) => loop {
                match pending.advance_observed(&schema, Some(&registry), &observation) {
                    Ok(false) => {}
                    Ok(true) => panic!("cancelled metadata published"),
                    Err(error) => {
                        assert_eq!(
                            error,
                            PreparationError::Observation("metadata cancelled".into())
                        );
                        assert!(pending.advance(&schema, Some(&registry)).is_err());
                        assert!(pending.finish().is_err());
                        break;
                    }
                }
            },
        }
        schema
            .require_attribute_conversion(&field, Some(&registry))
            .unwrap();
    }
    schema.conversion_contracts[32].provider.version = "changed".into();
    let observation = InvocationContext::new(metadata_probe(usize::MAX, usize::MAX));
    let mut pending = AttributeConversionPreparation::new_observed(&field, &observation).unwrap();
    loop {
        match pending.advance_observed(&schema, Some(&registry), &observation) {
            Ok(false) => {}
            Ok(true) => panic!("mismatched provider qualified"),
            Err(error) => {
                assert_eq!(
                    error,
                    PreparationError::Schema(
                        "Runtime conversion provider differs from compiled schema contract".into()
                    )
                );
                assert!(pending.finish().is_err());
                break;
            }
        }
    }
    assert_eq!(provider.calls.get(), provider_calls_before_qualification);
}

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
fn fixture() -> (DefinitionBundle, Provider) {
    let fields = (0..64)
        .map(|n| {
            format!(
                r##"<eStructuralFeatures x:type="e:EAttribute" name="f{n:02}" eType="#//Value"/>"##
            )
        })
        .collect::<String>();
    let source = DefinitionSource::new(
        "memory:attribute-cursor.ecore",
        SourceKind::Ecore,
        format!(
            r##"<e:EPackage xmlns:e="http://www.eclipse.org/emf/2002/Ecore" xmlns:x="http://www.w3.org/2001/XMLSchema-instance" name="m" nsURI="urn:attribute-cursor"><eClassifiers x:type="e:EDataType" name="Value" instanceClassName="test.Value"/><eClassifiers x:type="e:EClass" name="Base">{fields}</eClassifiers><eClassifiers x:type="e:EClass" name="Child" eSuperTypes="#//Base"/></e:EPackage>"##
        ),
    );
    let (source, declarations) =
        intersect_model_acceptance::migration_fixture::migrate_source_fixture(source);
    let provider = Provider {
        contract: ConversionContract {
            datatype: declarations[&vec![0]].clone(),
            provider: ProviderId {
                name: "test.value".into(),
                version: "1".into(),
            },
            serializable: true,
            declarations: vec![],
            members: vec![],
        },
        calls: Cell::new(0),
    };
    (
        DefinitionBundle {
            version: 1,
            sources: vec![source],
            namespaces: vec![],
        },
        provider,
    )
}

struct ValueProbe {
    work: Cell<usize>,
    limit: usize,
    failed: Cell<bool>,
    invocations: Cell<usize>,
}
impl InvocationObserver for ValueProbe {
    fn before_invoke(&self, _: &ProviderId) -> Result<(), String> {
        self.checkpoint()?;
        self.invocations.set(self.invocations.get() + 1);
        Ok(())
    }
    fn checkpoint(&self) -> Result<(), String> {
        if self.failed.get() {
            Err("value qualification cancelled".into())
        } else {
            Ok(())
        }
    }
    fn work(&self, units: usize) -> Result<(), String> {
        assert_eq!(
            units, 1,
            "metadata and transitions precharge one visit at a time"
        );
        self.checkpoint()?;
        if self.work.get() + units > self.limit {
            self.failed.set(true);
            return self.checkpoint();
        }
        self.work.set(self.work.get() + units);
        Ok(())
    }
}
fn value_probe(limit: usize) -> Rc<ValueProbe> {
    Rc::new(ValueProbe {
        work: Cell::new(0),
        limit,
        failed: Cell::new(false),
        invocations: Cell::new(0),
    })
}
fn value_run(
    schema: &Schema,
    pending: &mut RuntimeAttributeValidationPreparation,
    value: &Value,
    registry: Option<&ConversionRegistry<'_>>,
    observation: &InvocationContext<'_>,
    probe: &ValueProbe,
) -> Result<usize, String> {
    for step in 1..10000 {
        let before = probe.work.get();
        let result = pending.advance(schema, value, registry, Some(observation));
        // Policy/alternative slicing is checked by the callers' step counts.
        // Metadata copies now inspect charged visits within an advance; every
        // work-unit interruption is still checked independently below.
        if matches!(result, Ok(false)) {
            assert!(
                probe.work.get() > before,
                "pending validation must charge progress"
            );
        }
        if result? {
            return Ok(step);
        }
    }
    panic!("value qualification did not terminate")
}

#[test]
fn present_custom_values_yield_contract_rows_and_cancel_at_every_pending_boundary() {
    let (bundle, provider) = fixture();
    let datatype = compiler::import_datatype_bundle(&bundle, &provider.contract.datatype).unwrap();
    let mut registry = ConversionRegistry::default();
    registry.register(&datatype, &provider).unwrap();
    let mut schema = compiler::compile_structure_bundle_with_conversions(
        &bundle,
        &bundle.sources[0].id,
        "urn:attribute-cursor",
        &registry,
    )
    .unwrap();
    // Unrelated scheduling rows preserve the exact selected contract.
    let mut unrelated = provider.contract.clone();
    unrelated.datatype.path = vec![999];
    schema.conversion_contracts = vec![unrelated; 96];
    schema
        .conversion_contracts
        .insert(48, provider.contract.clone());
    let mut map = FeatureMapPreparation::new("Child");
    while !map.advance(&schema).unwrap() {}
    let fields = map.finish().unwrap();
    let field = fields.get("f63").unwrap();
    let value = serde_json::to_value(
        registry
            .parse(&provider.contract.datatype, "value")
            .unwrap(),
    )
    .unwrap();
    let probe = value_probe(usize::MAX);
    let observation = InvocationContext::new(probe.clone());
    let mut pending =
        RuntimeAttributeValidationPreparation::from_prepared("Child", field, &fields).unwrap();
    assert!(
        RuntimeAttributeValidationPreparation::new("Child", field)
            .finish()
            .is_err()
    );
    let steps = value_run(
        &schema,
        &mut pending,
        &value,
        Some(&registry),
        &observation,
        &probe,
    )
    .unwrap();
    assert!(steps > 97);
    assert_eq!(probe.invocations.get(), 1);
    let total = probe.work.get();
    assert!(
        pending
            .advance(&schema, &value, Some(&registry), Some(&observation))
            .unwrap()
    );
    assert_eq!(probe.work.get(), total, "terminal advance is idempotent");
    pending.finish().unwrap();
    for limit in 0..total {
        let probe = value_probe(limit);
        let observation = InvocationContext::new(probe.clone());
        let mut pending =
            RuntimeAttributeValidationPreparation::from_prepared("Child", field, &fields).unwrap();
        let before_calls = provider.calls.get();
        assert_eq!(
            value_run(
                &schema,
                &mut pending,
                &value,
                Some(&registry),
                &observation,
                &probe
            )
            .unwrap_err(),
            "value qualification cancelled"
        );
        assert_eq!(probe.work.get(), limit);
        assert_eq!(probe.invocations.get(), 0);
        assert_eq!(provider.calls.get(), before_calls);
        assert_eq!(
            pending
                .advance(&schema, &value, Some(&registry), None)
                .unwrap_err(),
            "Runtime attribute validation preparation has failed"
        );
        assert!(pending.finish().is_err());
    }
    let probe = value_probe(total);
    let observation = InvocationContext::new(probe.clone());
    let mut pending =
        RuntimeAttributeValidationPreparation::from_prepared("Child", field, &fields).unwrap();
    assert_eq!(
        value_run(
            &schema,
            &mut pending,
            &value,
            Some(&registry),
            &observation,
            &probe
        )
        .unwrap(),
        steps
    );
    pending.finish().unwrap();
    schema
        .validate_runtime_attribute("Child", field, &value, Some(&registry))
        .unwrap();
    assert_eq!(
        schema
            .validate_runtime_attribute("Child", field, &Value::Null, None)
            .unwrap_err(),
        "Explicit conversion registry required for custom datatype"
    );
    assert_eq!(
        schema
            .validate_runtime_attribute("Child", field, &Value::Null, Some(&registry))
            .unwrap_err(),
        "Null is absence, not a present attribute value"
    );
}

fn raw_value_fixture() -> (Schema, String, Feature, DeclarationId, DeclarationId) {
    use intersect_model::definitions::generic_types::RuntimeType;
    let source = DefinitionSource::new(
        "memory:value-domain.ecore",
        SourceKind::Ecore,
        include_str!("../../../../fixtures/ecore-generics/raw-feature-domains.ecore"),
    );
    let (source, declarations) =
        intersect_model_acceptance::migration_fixture::migrate_source_fixture(source);
    let id = |index| declarations[&vec![index]].clone();
    let raw = RuntimeType::new(Type::Classifier {
        declaration: id(8),
        arguments: vec![],
        raw: true,
    })
    .unwrap();
    let bundle = DefinitionBundle {
        version: 1,
        sources: vec![source.clone()],
        namespaces: vec![],
    };
    let schema =
        compiler::materialize_runtime_types(&bundle, std::slice::from_ref(&raw), None).unwrap();
    let receiver = schema.instantiated_class_key(&raw).unwrap().to_owned();
    let field = schema.features(&receiver).unwrap()["value"].clone();
    (schema, receiver, field, id(6), id(7))
}

#[test]
fn observed_runtime_codec_domains_cancel_every_copy_and_recover_exact_projection() {
    let (mut schema, receiver, field, _, _) = raw_value_fixture();
    let mut policy = schema.runtime_feature_domains[&receiver][0].clone();
    policy.alternatives = vec!["EInt".into(), "EString".into()];
    // Repeated policies exercise scheduling, not source-support qualification.
    schema
        .runtime_feature_domains
        .insert(receiver.clone(), vec![policy; 2]);
    let original = serde_json::to_value(&schema).unwrap();
    let mut map = FeatureMapPreparation::new(&receiver);
    while !map.advance(&schema).unwrap() {}
    let fields = map.finish().unwrap();
    let index_budget = metadata_probe(usize::MAX, usize::MAX);
    let observation = InvocationContext::new(index_budget.clone());
    let mut indices = RuntimeAttributeConversionPreparation::from_prepared_observed(
        &receiver,
        &field,
        &fields,
        &observation,
    )
    .unwrap();
    let input_bytes = index_budget.storage.get();
    assert!(
        !indices
            .advance_observed(&schema, None, &observation)
            .unwrap()
    );
    assert_eq!(index_budget.storage.get(), input_bytes + 32);
    assert!(
        !indices
            .advance_observed(&schema, None, &observation)
            .unwrap()
    );
    assert_eq!(index_budget.storage.get(), input_bytes + 64);
    assert!(indices.finish().is_err());
    for prepared in [false, true] {
        let run = |observation: &InvocationContext<'_>| {
            let mut pending = if prepared {
                RuntimeAttributeConversionPreparation::from_prepared_observed(
                    &receiver,
                    &field,
                    &fields,
                    observation,
                )?
            } else {
                RuntimeAttributeConversionPreparation::new_observed(&receiver, &field, observation)?
            };
            loop {
                match pending.advance_observed(&schema, None, observation) {
                    Ok(false) => {}
                    Ok(true) => return pending.finish().map_err(PreparationError::Schema),
                    Err(error) => {
                        assert!(pending.advance(&schema, None).is_err());
                        assert!(pending.finish().is_err());
                        return Err(error);
                    }
                }
            }
        };
        let baseline = metadata_probe(usize::MAX, usize::MAX);
        run(&InvocationContext::new(baseline.clone())).unwrap();
        let required = baseline.storage.get();
        for limit in [0, required - 1, required, required + 1] {
            let budget = metadata_probe(limit, usize::MAX);
            let result = run(&InvocationContext::new(budget.clone()));
            if limit < required {
                assert_eq!(
                    result.unwrap_err(),
                    PreparationError::Observation("metadata storage denied".into())
                );
                assert!(budget.storage.get() < required);
            } else {
                result.unwrap();
                assert_eq!(budget.storage.get(), required);
            }
        }
        for stop in 0..baseline.checkpoints.get() {
            assert_eq!(
                run(&InvocationContext::new(metadata_probe(usize::MAX, stop))).unwrap_err(),
                PreparationError::Observation("metadata cancelled".into())
            );
            schema
                .require_runtime_attribute_conversion(&receiver, &field, None)
                .unwrap();
        }
    }
    assert_eq!(serde_json::to_value(&schema).unwrap(), original);
    schema.runtime_feature_domains.get_mut(&receiver).unwrap()[1].alternatives =
        vec!["unknown".into()];
    let observation = InvocationContext::new(metadata_probe(usize::MAX, usize::MAX));
    let mut pending = RuntimeAttributeConversionPreparation::from_prepared_observed(
        &receiver,
        &field,
        &fields,
        &observation,
    )
    .unwrap();
    loop {
        match pending.advance_observed(&schema, None, &observation) {
            Ok(false) => {}
            Ok(true) => panic!("later invalid domain qualified"),
            Err(error) => {
                assert_eq!(
                    error,
                    PreparationError::Schema("Unknown runtime attribute domain codec".into())
                );
                assert!(pending.advance(&schema, None).is_err());
                assert!(pending.finish().is_err());
                break;
            }
        }
    }
}

#[test]
fn every_matching_value_domain_yields_rejected_alternatives_and_preserves_null_precedence() {
    let (mut schema, receiver, field, _, _) = raw_value_fixture();
    let mut policy = schema.runtime_feature_domains[&receiver][0].clone();
    policy.alternatives = vec!["EInt".into(); 32];
    policy.alternatives.push("EString".into());
    // Repeated metadata is a scheduling control, not a source-support claim.
    schema
        .runtime_feature_domains
        .insert(receiver.clone(), vec![policy; 12]);
    let value = json!("literal text");
    let probe = value_probe(usize::MAX);
    let observation = InvocationContext::new(probe.clone());
    let mut pending = RuntimeAttributeValidationPreparation::new(&receiver, &field);
    let steps = value_run(&schema, &mut pending, &value, None, &observation, &probe).unwrap();
    assert!(steps > 12 * 32 * 4);
    assert_eq!(probe.invocations.get(), 0);
    pending.finish().unwrap();
    schema
        .validate_runtime_attribute(&receiver, &field, &value, None)
        .unwrap();
    assert_eq!(
        schema
            .validate_runtime_attribute(&receiver, &field, &Value::Null, None)
            .unwrap_err(),
        "Null is absence, not a present attribute value"
    );
    // A later policy must still admit the value; earlier success is insufficient.
    schema.runtime_feature_domains.get_mut(&receiver).unwrap()[11].alternatives =
        vec!["EInt".into()];
    assert_eq!(
        schema
            .validate_runtime_attribute(&receiver, &field, &value, None)
            .unwrap_err(),
        "Value does not satisfy the receiver-specific feature domain"
    );
    // Required codecs are all qualified before testing any value alternative.
    schema.runtime_feature_domains.get_mut(&receiver).unwrap()[11].alternatives =
        vec!["UnknownCodec".into()];
    assert_eq!(
        schema
            .validate_runtime_attribute(&receiver, &field, &value, None)
            .unwrap_err(),
        "Unknown runtime attribute domain codec"
    );
    assert_eq!(
        schema
            .validate_runtime_attribute(&receiver, &field, &Value::Null, None)
            .unwrap_err(),
        "Null is absence, not a present attribute value"
    );
}

#[test]
fn unresolved_value_candidates_yield_and_reject_with_the_retained_reason() {
    use intersect_model::definitions::generic_types::RuntimeType;
    let (mut schema, receiver, field, text, number) = raw_value_fixture();
    let source = DefinitionSource::new(
        "memory:value-domain.ecore",
        SourceKind::Ecore,
        include_str!("../../../../fixtures/ecore-generics/raw-feature-domains.ecore"),
    );
    let (source, _) = intersect_model_acceptance::migration_fixture::migrate_source_fixture(source);
    let bundle = DefinitionBundle {
        version: 1,
        sources: vec![source],
        namespaces: vec![],
    };
    schema.datatypes.insert(
        "Text".into(),
        compiler::import_datatype_bundle(&bundle, &text).unwrap(),
    );
    let make_type = |id| {
        RuntimeType::new(Type::Classifier {
            declaration: id,
            arguments: vec![],
            raw: false,
        })
        .unwrap()
    };
    let policy = &mut schema.runtime_feature_domains.get_mut(&receiver).unwrap()[0];
    policy.alternatives = vec!["EInt".into()];
    policy.unresolved = vec![
        UnresolvedRuntimeFeatureCandidate {
            candidate: make_type(number),
            target: "EInt".into(),
            reason: "unrelated scheduling candidate".into(),
        };
        64
    ];
    policy.unresolved.push(UnresolvedRuntimeFeatureCandidate {
        candidate: make_type(text.clone()),
        target: "Text".into(),
        reason: "pinned value uncertainty".into(),
    });
    let value = serde_json::to_value(
        intersect_model::definitions::datatypes::DatatypeValue::Datatype {
            datatype: text,
            value: json!("present"),
        },
    )
    .unwrap();
    let probe = value_probe(usize::MAX);
    let observation = InvocationContext::new(probe.clone());
    let mut pending = RuntimeAttributeValidationPreparation::new(&receiver, &field);
    let error = value_run(&schema, &mut pending, &value, None, &observation, &probe).unwrap_err();
    assert_eq!(
        error,
        "Unresolved runtime feature membership: pinned value uncertainty"
    );
    assert!(probe.work.get() > 64);
    assert_eq!(
        pending.advance(&schema, &value, None, None).unwrap_err(),
        "Runtime attribute validation preparation has failed"
    );
    assert!(pending.finish().is_err());
    assert_eq!(
        schema
            .validate_runtime_attribute(&receiver, &field, &value, None)
            .unwrap_err(),
        error
    );
}
fn run(
    schema: &Schema,
    field: &Feature,
    registry: &ConversionRegistry<'_>,
) -> (usize, Result<(), String>) {
    let mut pending = AttributeConversionPreparation::new(field);
    let mut steps = 0;
    loop {
        steps += 1;
        match pending.advance(schema, Some(registry)) {
            Ok(false) => {}
            Ok(true) => {
                assert!(pending.advance(schema, Some(registry)).unwrap());
                return (steps, pending.finish());
            }
            Err(error) => {
                assert_eq!(
                    pending.advance(schema, Some(registry)).unwrap_err(),
                    "Attribute conversion preparation has failed"
                );
                assert!(pending.finish().is_err());
                return (steps, Err(error));
            }
        }
        assert!(steps < 1000);
    }
}
#[test]
fn ordinary_contract_rows_yield_and_preserve_exact_metadata_and_error_order() {
    let (bundle, provider) = fixture();
    let datatype = compiler::import_datatype_bundle(&bundle, &provider.contract.datatype).unwrap();
    let mut registry = ConversionRegistry::default();
    registry.register(&datatype, &provider).unwrap();
    let mut schema = compiler::compile_structure_bundle_with_conversions(
        &bundle,
        &bundle.sources[0].id,
        "urn:attribute-cursor",
        &registry,
    )
    .unwrap();
    let field = schema.features("Child").unwrap()["f63"].clone();
    let unrelated = (0..96)
        .map(|n| {
            let mut contract = provider.contract.clone();
            contract.datatype.path = vec![n + 10];
            contract
        })
        .collect::<Vec<_>>();
    // These unrelated retained rows are table-scheduling fixtures, not claims
    // that arbitrary forged contracts passed source/schema qualification.
    schema.conversion_contracts = unrelated;
    schema
        .conversion_contracts
        .insert(48, provider.contract.clone());
    let before = provider.calls.get();
    let (required, result) = run(&schema, &field, &registry);
    assert_eq!(result, Ok(()));
    assert_eq!(required, 99);
    schema
        .require_attribute_conversion(&field, Some(&registry))
        .unwrap();
    assert_eq!(
        provider.calls.get(),
        before,
        "metadata qualification invokes no semantics"
    );
    for boundary in [0, 1, 48, required - 1] {
        let mut pending = AttributeConversionPreparation::new(&field);
        for _ in 0..boundary {
            assert!(!pending.advance(&schema, Some(&registry)).unwrap());
        }
        assert!(pending.finish().is_err());
    }
    let mut duplicate = schema.clone();
    duplicate
        .conversion_contracts
        .push(provider.contract.clone());
    assert_eq!(
        run(&duplicate, &field, &registry).1.unwrap_err(),
        "Runtime conversion provider differs from compiled schema contract"
    );
    let mut missing = schema.clone();
    missing.conversion_contracts.remove(48);
    assert_eq!(
        run(&missing, &field, &registry).1.unwrap_err(),
        "Runtime conversion provider differs from compiled schema contract"
    );
    let mut foreign = schema.clone();
    foreign.conversion_contracts[48].provider.version = "foreign".into();
    assert_eq!(
        run(&foreign, &field, &registry).1.unwrap_err(),
        "Runtime conversion provider differs from compiled schema contract"
    );
    let mut wrong = field.clone();
    wrong.datatype.as_mut().unwrap().path = vec![999];
    assert_eq!(
        schema
            .require_attribute_conversion(&wrong, None)
            .unwrap_err(),
        "Attribute datatype identity mismatch"
    );
    assert_eq!(
        schema
            .require_attribute_conversion(&field, None)
            .unwrap_err(),
        "Explicit conversion registry required for custom datatype"
    );
    wrong.reference = true;
    schema.require_attribute_conversion(&wrong, None).unwrap();
}
#[test]
fn runtime_field_inventory_is_shared_and_prepared_reuse_preserves_full_feature_equality() {
    let (bundle, provider) = fixture();
    let datatype = compiler::import_datatype_bundle(&bundle, &provider.contract.datatype).unwrap();
    let mut registry = ConversionRegistry::default();
    registry.register(&datatype, &provider).unwrap();
    let schema = compiler::compile_structure_bundle_with_conversions(
        &bundle,
        &bundle.sources[0].id,
        "urn:attribute-cursor",
        &registry,
    )
    .unwrap();
    let field = schema.features("Child").unwrap()["f63"].clone();
    let mut pending = RuntimeAttributeConversionPreparation::new("Child", &field);
    let mut steps = 0;
    while !pending.advance(&schema, Some(&registry)).unwrap() {
        steps += 1;
        assert!(steps < 2000);
    }
    pending.finish().unwrap();
    assert!(steps > 64 * 4);
    let mut map = FeatureMapPreparation::new("Child");
    while !map.advance(&schema).unwrap() {}
    let fields = map.finish().unwrap();
    assert_eq!(fields.receiver(), "Child");
    assert_eq!(fields.order().first().unwrap(), "f00");
    assert_eq!(fields.order().last().unwrap(), "f63");
    assert_eq!(fields.member_identities().len(), 64);
    let mut fast =
        RuntimeAttributeConversionPreparation::from_prepared("Child", &field, &fields).unwrap();
    let mut fast_steps = 0;
    while !fast.advance(&schema, Some(&registry)).unwrap() {
        fast_steps += 1;
    }
    fast.finish().unwrap();
    assert!(fast_steps < 10);
    assert!(RuntimeAttributeConversionPreparation::from_prepared("Base", &field, &fields).is_err());
    for changed in 0..3 {
        let mut wrong = field.clone();
        match changed {
            0 => wrong.default = json!("forged"),
            1 => wrong.unique = !wrong.unique,
            _ => wrong.derived = true,
        };
        assert!(
            RuntimeAttributeConversionPreparation::from_prepared("Child", &wrong, &fields).is_err()
        );
        let mut pending = RuntimeAttributeConversionPreparation::new("Child", &wrong);
        let error = loop {
            match pending.advance(&schema, None) {
                Ok(false) => {}
                Ok(true) => panic!("forged field accepted"),
                Err(error) => break error,
            }
        };
        assert_eq!(
            error,
            "Attribute does not belong to the receiver's effective features"
        );
        assert_eq!(
            pending.advance(&schema, Some(&registry)).unwrap_err(),
            "Runtime attribute conversion preparation has failed"
        );
        assert!(pending.finish().is_err());
    }
    assert_eq!(
        schema
            .require_runtime_attribute_conversion("Child", &field, None)
            .unwrap_err(),
        "Explicit conversion registry required for custom datatype"
    );
}
#[test]
fn applied_contract_selection_yields_rows_and_preserves_missing_duplicate_and_invalid_errors() {
    let source = DefinitionSource::new(
        "memory:applied-cursor.ecore",
        SourceKind::Ecore,
        include_str!("../../../../fixtures/ecore-generics/applied-collections.ecore"),
    );
    let (source, declarations) =
        intersect_model_acceptance::migration_fixture::migrate_source_fixture(source);
    let bundle = DefinitionBundle {
        version: 1,
        sources: vec![source.clone()],
        namespaces: vec![],
    };
    let id = |index| declarations[&vec![index]].clone();
    let ty = |index, args| Type::Classifier {
        declaration: id(index),
        arguments: args,
        raw: false,
    };
    let application = ClosedType::new(ty(2, vec![ty(0, vec![])])).unwrap();
    let checked = Resolver::new(&bundle)
        .unwrap()
        .instantiate_datatype(&application)
        .unwrap();
    let datatype = compiler::import_datatype_bundle(&bundle, &id(2))
        .unwrap()
        .specialize(&checked)
        .unwrap();
    let contract = AppliedConversionContract {
        application,
        conversion: ConversionContract {
            datatype: id(2),
            provider: ProviderId {
                name: "test.list".into(),
                version: "1".into(),
            },
            serializable: false,
            declarations: vec![],
            members: vec![],
        },
    };
    let mut other = contract.clone();
    other.application = ClosedType::new(ty(2, vec![ty(1, vec![])])).unwrap();
    let mut schema=intersect_model_acceptance::migration_fixture::compile_migrated_schema_fixture(r#"<e:EPackage xmlns:e="http://www.eclipse.org/emf/2002/Ecore" xmlns:x="http://www.w3.org/2001/XMLSchema-instance" name="empty" nsURI="urn:empty"><eClassifiers x:type="e:EClass" name="A"/></e:EPackage>"#).unwrap();
    schema.applied_conversion_contracts = vec![other; 96];
    schema
        .applied_conversion_contracts
        .insert(48, contract.clone());
    let mut pending = AppliedDatatypeContractPreparation::new(&datatype);
    let mut steps = 0;
    while !pending.advance(&schema).unwrap() {
        steps += 1;
    }
    assert_eq!(steps + 1, 100);
    let baseline = metadata_probe(usize::MAX, usize::MAX);
    let observation = InvocationContext::new(baseline.clone());
    let mut observed =
        AppliedDatatypeContractPreparation::new_observed(&datatype, &observation).unwrap();
    let required = baseline.storage.get();
    while !observed.advance_observed(&schema, &observation).unwrap() {}
    assert_eq!(observed.finish(&schema).unwrap(), &contract);
    assert_eq!(
        baseline.storage.get(),
        required,
        "borrowed contract scans retain no copies"
    );
    for limit in [0, required - 1, required, required + 1] {
        let budget = metadata_probe(limit, usize::MAX);
        let observation = InvocationContext::new(budget.clone());
        match AppliedDatatypeContractPreparation::new_observed(&datatype, &observation) {
            Err(error) => {
                assert!(limit < required);
                assert_eq!(
                    error,
                    PreparationError::Observation("metadata storage denied".into())
                );
                assert_eq!(budget.storage.get(), 0);
            }
            Ok(mut observed) => {
                assert!(limit >= required);
                while !observed.advance_observed(&schema, &observation).unwrap() {}
                assert_eq!(observed.finish(&schema).unwrap(), &contract);
                assert_eq!(budget.storage.get(), required);
            }
        }
    }
    for stop in 0..baseline.checkpoints.get() {
        let observation = InvocationContext::new(metadata_probe(usize::MAX, stop));
        match AppliedDatatypeContractPreparation::new_observed(&datatype, &observation) {
            Err(error) => assert_eq!(
                error,
                PreparationError::Observation("metadata cancelled".into())
            ),
            Ok(mut observed) => loop {
                match observed.advance_observed(&schema, &observation) {
                    Ok(false) => {}
                    Ok(true) => panic!("cancelled applied metadata published"),
                    Err(error) => {
                        assert_eq!(
                            error,
                            PreparationError::Observation("metadata cancelled".into())
                        );
                        assert!(observed.advance(&schema).is_err());
                        assert!(observed.finish(&schema).is_err());
                        break;
                    }
                }
            },
        }
        assert_eq!(
            schema.applied_datatype_contract(&datatype).unwrap(),
            &contract
        );
    }
    assert!(pending.advance(&schema).unwrap());
    assert_eq!(pending.finish(&schema).unwrap(), &contract);
    assert_eq!(
        schema.applied_datatype_contract(&datatype).unwrap(),
        &contract
    );
    for boundary in [0, 1, 48, 99] {
        let mut pending = AppliedDatatypeContractPreparation::new(&datatype);
        for _ in 0..boundary {
            assert!(!pending.advance(&schema).unwrap());
        }
        assert!(pending.finish(&schema).is_err());
    }
    for mutation in 0..3 {
        let mut invalid = schema.clone();
        let expected = match mutation {
            0 => {
                invalid.applied_conversion_contracts.remove(48);
                "Missing compiled applied conversion contract"
            }
            1 => {
                invalid.applied_conversion_contracts.push(contract.clone());
                "Invalid compiled applied conversion contract"
            }
            _ => {
                invalid.applied_conversion_contracts[48]
                    .conversion
                    .provider
                    .version = " ".into();
                "Invalid compiled applied conversion contract"
            }
        };
        assert_eq!(
            invalid.applied_datatype_contract(&datatype).unwrap_err(),
            expected
        );
        let mut pending = AppliedDatatypeContractPreparation::new(&datatype);
        loop {
            match pending.advance(&invalid) {
                Ok(false) => {}
                Ok(true) => panic!("invalid contract accepted"),
                Err(error) => {
                    assert_eq!(error, expected);
                    break;
                }
            }
        }
        assert!(pending.advance(&schema).is_err());
        assert!(pending.finish(&schema).is_err());
    }
    let ordinary = compiler::import_datatype_bundle(&bundle, &id(0)).unwrap();
    assert_eq!(
        schema.applied_datatype_contract(&ordinary).unwrap_err(),
        "Datatype has no application"
    );
}

#[test]
fn matching_runtime_policy_rows_and_every_codec_alternative_yield_in_order() {
    use intersect_model::definitions::generic_types::RuntimeType;
    let source = DefinitionSource::new(
        "memory:runtime-attribute.ecore",
        SourceKind::Ecore,
        include_str!("../../../../fixtures/ecore-generics/raw-feature-domains.ecore"),
    );
    let (source, declarations) =
        intersect_model_acceptance::migration_fixture::migrate_source_fixture(source);
    let ty = |index, raw| {
        RuntimeType::new(Type::Classifier {
            declaration: declarations[&vec![index]].clone(),
            arguments: vec![],
            raw,
        })
        .unwrap()
    };
    let raw = ty(8, true);
    let bundle = DefinitionBundle {
        version: 1,
        sources: vec![source.clone()],
        namespaces: vec![],
    };
    let mut schema =
        compiler::materialize_runtime_types(&bundle, std::slice::from_ref(&raw), None).unwrap();
    let receiver = schema.instantiated_class_key(&raw).unwrap().to_owned();
    let field = schema.features(&receiver).unwrap()["value"].clone();
    let mut policy = schema.runtime_feature_domains[&receiver][0].clone();
    let target = policy.alternatives[0].clone();
    policy.alternatives = vec![target; 16];
    schema
        .runtime_feature_domains
        .insert(receiver.clone(), vec![policy; 24]);
    let mut map = FeatureMapPreparation::new(&receiver);
    while !map.advance(&schema).unwrap() {}
    let fields = map.finish().unwrap();
    let mut pending =
        RuntimeAttributeConversionPreparation::from_prepared(&receiver, &field, &fields).unwrap();
    let mut steps = 0;
    while !pending.advance(&schema, None).unwrap() {
        steps += 1;
        assert!(steps < 2000);
    }
    assert!(steps > 24 * 16 * 2);
    pending.finish().unwrap();
    schema
        .require_runtime_attribute_conversion(&receiver, &field, None)
        .unwrap();
    schema.runtime_feature_domains.get_mut(&receiver).unwrap()[23].alternatives[15] =
        "unknown-codec".into();
    let mut pending =
        RuntimeAttributeConversionPreparation::from_prepared(&receiver, &field, &fields).unwrap();
    let mut failed_steps = 0;
    let error = loop {
        failed_steps += 1;
        match pending.advance(&schema, None) {
            Ok(false) => {}
            Ok(true) => panic!("unknown alternative accepted"),
            Err(error) => break error,
        }
    };
    assert!(failed_steps > 24 * 15 * 2);
    assert_eq!(error, "Unknown runtime attribute domain codec");
    assert!(pending.advance(&schema, None).is_err());
    assert!(pending.finish().is_err());
}

#[test]
fn a_provider_changed_between_contract_rows_cannot_publish_qualification() {
    struct Mutating {
        original: Provider,
        changed: ConversionContract,
        mutate: Cell<bool>,
    }
    impl ConversionProvider for Mutating {
        fn contract(&self) -> &ConversionContract {
            if self.mutate.get() {
                &self.changed
            } else {
                &self.original.contract
            }
        }
        fn parse(&self, text: &str) -> Result<Value, String> {
            self.original.parse(text)
        }
        fn format(&self, value: &Value) -> Result<String, String> {
            self.original.format(value)
        }
        fn validate(&self, value: &Value) -> Result<(), String> {
            self.original.validate(value)
        }
        fn default_value(&self) -> Result<Option<Value>, String> {
            self.original.default_value()
        }
    }
    let (bundle, original) = fixture();
    let mut changed = original.contract.clone();
    changed.provider.version = "changed".into();
    let provider = Mutating {
        original,
        changed,
        mutate: Cell::new(false),
    };
    let datatype =
        compiler::import_datatype_bundle(&bundle, &provider.original.contract.datatype).unwrap();
    let mut registry = ConversionRegistry::default();
    registry.register(&datatype, &provider).unwrap();
    let mut schema = compiler::compile_structure_bundle_with_conversions(
        &bundle,
        &bundle.sources[0].id,
        "urn:attribute-cursor",
        &registry,
    )
    .unwrap();
    let field = schema.features("Child").unwrap()["f63"].clone();
    for n in 0..64 {
        let mut other = provider.original.contract.clone();
        other.datatype.path = vec![n + 10];
        schema.conversion_contracts.push(other);
    }
    let before = provider.original.calls.get();
    let mut pending = AttributeConversionPreparation::new(&field);
    for _ in 0..16 {
        assert!(!pending.advance(&schema, Some(&registry)).unwrap());
    }
    provider.mutate.set(true);
    let error = loop {
        match pending.advance(&schema, Some(&registry)) {
            Ok(false) => {}
            Ok(true) => panic!("changed provider accepted"),
            Err(error) => break error,
        }
    };
    assert_eq!(error, "Conversion provider changed its registered contract");
    assert!(pending.finish().is_err());
    assert_eq!(provider.original.calls.get(), before);
    provider.mutate.set(false);
    assert_eq!(run(&schema, &field, &registry).1, Ok(()));
}
