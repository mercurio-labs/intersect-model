use super::*;
use crate::definitions::conversion::{InvocationObserver, ProviderId};
use std::{cell::Cell, rc::Rc};

struct Probe {
    bytes: Cell<usize>,
    limit: usize,
    calls: Cell<usize>,
    stop: usize,
    work: Cell<usize>,
    fail_work_once: Cell<bool>,
}
impl InvocationObserver for Probe {
    fn before_invoke(&self, _: &ProviderId) -> Result<(), String> {
        panic!("scalar fixtures have no provider");
    }
    fn checkpoint(&self) -> Result<(), String> {
        if self.calls.get() == self.stop {
            return Err("value cancelled".into());
        }
        self.calls.set(self.calls.get() + 1);
        Ok(())
    }
    fn work(&self, units: usize) -> Result<(), String> {
        assert_eq!(units, 1);
        if self.fail_work_once.replace(false) {
            return Err("one-shot work denied".into());
        }
        self.checkpoint()?;
        self.work.set(self.work.get() + units);
        Ok(())
    }
    fn retain_storage(&self, bytes: usize) -> Result<(), String> {
        let next = self
            .bytes
            .get()
            .checked_add(bytes)
            .filter(|n| *n <= self.limit)
            .ok_or("value storage denied")?;
        self.bytes.set(next);
        Ok(())
    }
}
fn probe(limit: usize, stop: usize) -> Rc<Probe> {
    Rc::new(Probe {
        bytes: Cell::new(0),
        limit,
        calls: Cell::new(0),
        stop,
        work: Cell::new(0),
        fail_work_once: Cell::new(false),
    })
}
fn fixture() -> (Schema, Feature) {
    let mut schema = crate::definitions::compiler::inspect_ecore_schema(r#"<e:EPackage xmlns:e="http://www.eclipse.org/emf/2002/Ecore" xmlns:x="http://www.w3.org/2001/XMLSchema-instance" name="t" nsURI="urn:observed"><eClassifiers x:type="e:EClass" name="A"/></e:EPackage>"#).unwrap();
    let field = Feature {
        name: "x".into(),
        target: "EString".into(),
        datatype: None,
        reference: false,
        containment: false,
        opposite: None,
        lower: 0,
        upper: 1,
        unique: true,
        ordered: true,
        derived: false,
        changeable: true,
        transient: false,
        volatile: false,
        unsettable: false,
        default: Value::Null,
    };
    let class = schema.classes.get_mut("A").unwrap();
    class.features.insert("x".into(), field.clone());
    class.feature_order.push("x".into());
    (schema, field)
}

#[test]
fn observed_scalar_value_has_literal_two_feature_copies_and_work_policy() {
    let (schema, field) = fixture();
    let value = serde_json::json!("present");
    // Each Feature is 23 nodes/321 metadata bytes: 3265 bytes. Validation
    // and codec each own one copy. Scalar payload validation borrows the value.
    for limit in [0, 3264, 3265, 3266, 6529, 6530, 6531] {
        let budget = probe(limit, usize::MAX);
        let observation = InvocationContext::new(budget.clone());
        match AttributeValidationPreparation::new_observed(&field, &observation) {
            Err(error) => {
                assert!(limit < 6530);
                assert_eq!(
                    error,
                    PreparationError::Observation("value storage denied".into())
                );
                assert_eq!(budget.bytes.get(), if limit < 3265 { 0 } else { 3265 });
            }
            Ok(mut pending) => {
                assert!(limit >= 6530);
                assert_eq!(budget.work.get(), 46);
                assert!(
                    !pending
                        .advance_observed(&schema, &value, None, &observation)
                        .unwrap()
                );
                assert!(
                    pending
                        .advance_observed(&schema, &value, None, &observation)
                        .unwrap()
                );
                assert_eq!(budget.work.get(), 48);
                assert!(
                    pending
                        .advance_observed(&schema, &value, None, &observation)
                        .unwrap()
                );
                assert_eq!(budget.work.get(), 48);
                assert_eq!(budget.bytes.get(), 6530);
                pending.finish().unwrap();
            }
        }
    }
}

#[test]
fn observed_runtime_scalar_value_has_literal_prepared_and_fresh_policies_and_cancellation() {
    let (schema, field) = fixture();
    let value = serde_json::json!("present");
    let mut map = FeatureMapPreparation::new("A");
    while !map.advance(&schema).unwrap() {}
    let fields = map.finish().unwrap();
    for prepared in [false, true] {
        // Two receiver+Feature records are 6660. Nested scalar validation
        // owns two more Features (6530). Fresh selection also owns map 5025.
        let required = if prepared { 13190 } else { 18215 };
        let run = |observation: &InvocationContext<'_>| {
            let mut pending = if prepared {
                RuntimeAttributeValidationPreparation::from_prepared_observed(
                    "A",
                    &field,
                    &fields,
                    observation,
                )?
            } else {
                RuntimeAttributeValidationPreparation::new_observed("A", &field, observation)?
            };
            loop {
                match pending.advance_observed(&schema, &value, None, observation) {
                    Ok(false) => {}
                    Ok(true) => return pending.finish().map_err(PreparationError::Schema),
                    Err(error) => {
                        assert!(pending.advance(&schema, &value, None, None).is_err());
                        assert!(pending.finish().is_err());
                        return Err(error);
                    }
                }
            }
        };
        let baseline = probe(usize::MAX, usize::MAX);
        run(&InvocationContext::new(baseline.clone())).unwrap();
        assert_eq!(baseline.bytes.get(), required);
        for limit in [0, 64, 65, 3329, 3330, required - 1, required, required + 1] {
            let budget = probe(limit, usize::MAX);
            let result = run(&InvocationContext::new(budget.clone()));
            if limit < required {
                assert_eq!(
                    result.unwrap_err(),
                    PreparationError::Observation("value storage denied".into())
                );
                assert!(budget.bytes.get() < required);
            } else {
                result.unwrap();
                assert_eq!(budget.bytes.get(), required);
            }
        }
        for stop in 0..baseline.calls.get() {
            assert_eq!(
                run(&InvocationContext::new(probe(usize::MAX, stop))).unwrap_err(),
                PreparationError::Observation("value cancelled".into())
            );
            schema
                .validate_runtime_attribute("A", &field, &value, None)
                .unwrap();
        }
    }
}

#[test]
fn one_shot_nested_observation_is_terminal_while_invalid_value_tries_another_alternative() {
    let (schema, mut field) = fixture();
    field.target = "EInt".into();
    let value = serde_json::json!("invalid integer");
    // Seed only the existing nested Validate boundary to isolate its error
    // classification. Full raw/source-domain admission has separate controls.
    let make = || {
        let mut pending = RuntimeAttributeValidationPreparation::new("A", &field);
        pending.qualification = None;
        pending.validation = Some(Box::new(AttributeValidationPreparation::new(&field)));
        pending.stage = RuntimeValueStage::Validate;
        pending
    };
    let budget = probe(usize::MAX, usize::MAX);
    budget.fail_work_once.set(true);
    let observation = InvocationContext::new(budget);
    let mut pending = make();
    assert_eq!(
        pending
            .advance_observed(&schema, &value, None, &observation)
            .unwrap_err(),
        PreparationError::Observation("one-shot work denied".into())
    );
    assert_eq!(pending.alternative, 0);
    assert!(pending.advance(&schema, &value, None, None).is_err());
    assert!(pending.finish().is_err());
    assert!(
        observation.checkpoint().is_ok(),
        "observer itself is not sticky"
    );
    let observation = InvocationContext::new(probe(usize::MAX, usize::MAX));
    let mut pending = make();
    assert!(
        !pending
            .advance_observed(&schema, &value, None, &observation)
            .unwrap()
    );
    assert!(
        !pending
            .advance_observed(&schema, &value, None, &observation)
            .unwrap()
    );
    assert_eq!(pending.alternative, 1);
    assert!(matches!(pending.stage, RuntimeValueStage::Alternative));
    assert!(!pending.failed);
    assert!(pending.validation.is_none());
    assert!(pending.finish().is_err());
}

#[test]
fn declared_value_decode_precharges_literal_json_copy_and_preserves_identity() {
    let (mut schema, mut field) = fixture();
    let source = crate::definitions::bundle::DefinitionSource::new(
        "memory:observed-keys",
        crate::definitions::bundle::SourceKind::Ecore,
        "x",
    );
    let declaration = crate::definitions::resources::DeclarationId {
        source: source.id,
        path: vec![0],
    };
    schema.datatypes.insert(
        "N".into(),
        crate::definitions::datatypes::Datatype::new(
            declaration.clone(),
            "N".into(),
            true,
            Some("java.lang.String".into()),
            None,
        )
        .unwrap(),
    );
    field.target = "N".into();
    field.datatype = Some(declaration.clone());
    let value = serde_json::to_value(crate::definitions::datatypes::DatatypeValue::Datatype {
        datatype: declaration,
        value: serde_json::json!("é"),
    })
    .unwrap();
    // Library policy/identity fixture, not source import qualification.
    // Feature: 35 nodes/469 metadata bytes = 4949; two copies = 9898.
    // Struct keys use bare UTF-8 lengths. JSON keys add two bytes each;
    // its declaration is 153 bytes, and every container adds two bytes.
    // Borrowed JSON payload: 16 nodes/192 metadata bytes = 2240, decoded once.
    for limit in [0, 4948, 4949, 4950, 9897, 9898, 9899, 12137, 12138, 12139] {
        let budget = probe(limit, usize::MAX);
        let observation = InvocationContext::new(budget.clone());
        match AttributeValidationPreparation::new_observed(&field, &observation) {
            Err(error) => {
                assert!(limit < 9898);
                assert_eq!(
                    error,
                    PreparationError::Observation("value storage denied".into())
                );
                assert_eq!(budget.bytes.get(), if limit < 4949 { 0 } else { 4949 });
            }
            Ok(mut pending) => {
                assert!(
                    !pending
                        .advance_observed(&schema, &value, None, &observation)
                        .unwrap()
                );
                match pending.advance_observed(&schema, &value, None, &observation) {
                    Err(error) => {
                        assert!(limit < 12138);
                        assert_eq!(
                            error,
                            PreparationError::Observation("value storage denied".into())
                        );
                        assert_eq!(budget.bytes.get(), 9898);
                        assert!(pending.advance(&schema, &value, None, None).is_err());
                        assert!(pending.finish().is_err());
                    }
                    Ok(done) => {
                        assert!(done && limit >= 12138);
                        assert_eq!(budget.bytes.get(), 12138);
                        assert_eq!(budget.work.get(), 88);
                        pending.finish().unwrap();
                    }
                }
            }
        }
    }
    let baseline = probe(usize::MAX, usize::MAX);
    let observation = InvocationContext::new(baseline.clone());
    let mut pending = AttributeValidationPreparation::new_observed(&field, &observation).unwrap();
    while !pending
        .advance_observed(&schema, &value, None, &observation)
        .unwrap()
    {}
    pending.finish().unwrap();
    for stop in 0..baseline.calls.get() {
        let observation = InvocationContext::new(probe(usize::MAX, stop));
        match AttributeValidationPreparation::new_observed(&field, &observation) {
            Err(error) => assert_eq!(
                error,
                PreparationError::Observation("value cancelled".into())
            ),
            Ok(mut pending) => loop {
                match pending.advance_observed(&schema, &value, None, &observation) {
                    Ok(false) => {}
                    Ok(true) => panic!("cancelled payload accepted"),
                    Err(error) => {
                        assert_eq!(
                            error,
                            PreparationError::Observation("value cancelled".into())
                        );
                        assert!(pending.advance(&schema, &value, None, None).is_err());
                        assert!(pending.finish().is_err());
                        break;
                    }
                }
            },
        }
        let mut fresh = AttributeValidationPreparation::new(&field);
        while !fresh.advance(&schema, &value, None, None).unwrap() {}
        fresh.finish().unwrap();
    }
}
