use super::*;
use crate::definitions::conversion::{InvocationObserver, ProviderId};
use std::{cell::Cell, rc::Rc};

fn schema() -> Schema {
    crate::definitions::compiler::inspect_ecore_schema(r##"<e:EPackage xmlns:e="http://www.eclipse.org/emf/2002/Ecore" xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance" name="t" nsURI="urn:observed">
<eClassifiers xsi:type="e:EClass" name="A"/>
<eClassifiers xsi:type="e:EClass" name="B" eSuperTypes="#//A"/>
<eClassifiers xsi:type="e:EClass" name="C" eSuperTypes="#//A"/>
<eClassifiers xsi:type="e:EClass" name="D" eSuperTypes="#//B #//C"/>
</e:EPackage>"##).unwrap()
}

fn feature() -> super::super::Feature {
    super::super::Feature {
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
        default: serde_json::Value::Null,
    }
}
fn legacy_feature_schema() -> Schema {
    let mut value = schema();
    let class = value.classes.get_mut("A").unwrap();
    class.features.insert("x".into(), feature());
    class.feature_order.push("x".into());
    assert!(class.feature_declarations.is_empty());
    assert_eq!(value.uri, "urn:observed");
    assert_eq!(value.revision.len(), 64);
    value
}

fn reference_fixture() -> (Schema, Feature) {
    let mut value = schema();
    let mut opposite = feature();
    opposite.name = "y".into();
    opposite.target = "A".into();
    opposite.reference = true;
    // Use independent owners; B must not inherit A for these literal totals.
    let class = value.classes.get_mut("B").unwrap();
    class.parents.clear();
    class.features.insert("y".into(), opposite);
    class.feature_order.push("y".into());
    let mut field = feature();
    field.target = "A".into();
    field.reference = true;
    field.upper = -1;
    field.opposite = Some("B/y".into());
    (value, field)
}

fn raw_validation_schema() -> Schema {
    use crate::definitions::{
        bundle::{DefinitionBundle, DefinitionSource, SourceKind},
        generic_types::{RuntimeType, Type},
    };
    let source = DefinitionSource::new(
        "memory:observed-validation",
        SourceKind::Ecore,
        include_str!("../../assets/fixtures/ecore-generics/raw-feature-domains.ecore"),
    );
    let (source, ids) = crate::definitions::migration_test_fixture::source_with_ids(source);
    let raw = RuntimeType::new(Type::Classifier {
        declaration: ids[&vec![8]].clone(),
        arguments: vec![],
        raw: true,
    })
    .unwrap();
    crate::definitions::compiler::materialize_runtime_types(
        &DefinitionBundle {
            version: 1,
            sources: vec![source],
            namespaces: vec![],
        },
        &[raw],
        None,
    )
    .unwrap()
}

fn run_reference_observed(
    schema: &Schema,
    receiver: &str,
    name: &str,
    actual: &str,
    observation: &InvocationContext<'_>,
) -> Result<bool, PreparationError> {
    let mut pending =
        ReferenceConformancePreparation::new_observed(receiver, name, actual, observation)?;
    loop {
        match pending.advance_observed(schema, observation) {
            Ok(false) => {}
            Ok(true) => return pending.finish().map_err(PreparationError::Schema),
            Err(error) => {
                assert!(pending.advance(schema).is_err());
                assert!(pending.finish().is_err());
                return Err(error);
            }
        }
    }
}

fn class_key_fixture() -> (Schema, crate::definitions::generic_types::RuntimeType) {
    use crate::definitions::{
        bundle::{DefinitionBundle, DefinitionSource, SourceKind},
        generic_types::{RuntimeType, Type},
        resources::DeclarationId,
    };
    let source = DefinitionSource::new(
        "memory:observed-keys",
        SourceKind::Rcore,
        r##"rcore 1; package k version "1"; metamodel "urn:observed-keys" prefix k { class A {} class B {} }"##,
    );
    assert_eq!(source.id.uri.len(), 20);
    let target = RuntimeType::new(Type::Classifier {
        declaration: DeclarationId {
            source: source.id.clone(),
            path: vec![0],
        },
        arguments: vec![],
        raw: false,
    })
    .unwrap();
    let schema = crate::definitions::compiler::compile_structure_bundle(
        &DefinitionBundle {
            version: 1,
            sources: vec![source.clone()],
            namespaces: vec![],
        },
        &source.id,
        "urn:observed-keys",
    )
    .unwrap();
    (schema, target)
}

#[test]
fn observed_class_key_inputs_and_selection_have_independent_literal_policies() {
    let (schema, target) = class_key_fixture();
    let crate::definitions::generic_types::Type::Classifier { declaration, .. } =
        target.expression()
    else {
        panic!("classifier fixture")
    };
    // Declaration: 10 nodes, 145 metadata bytes = 1425. Type classifier:
    // 18 nodes, 220 metadata bytes = 2524. Exact selection adds validation's
    // state128/B cursor65, then A cursor/selection/B cursor195 -> total2912.
    // Legacy selection skips validation: 1425+195 = 1620.
    for (legacy, needed) in [(false, 2912usize), (true, 1620)] {
        for limit in [0, needed - 1, needed, needed + 1] {
            let budget = probe(limit, usize::MAX);
            let observation = InvocationContext::new(budget.clone());
            let constructor = if legacy {
                ClassKeyPreparation::reference_declaration_observed(declaration, &observation)
            } else {
                ClassKeyPreparation::new_observed(&target, &observation)
            };
            let mut pending = match constructor {
                Ok(value) => value,
                Err(error) => {
                    assert!(limit < needed);
                    assert_eq!(
                        error,
                        PreparationError::Observation("schema storage denied".into())
                    );
                    assert_eq!(budget.used.get(), 0);
                    continue;
                }
            };
            loop {
                match pending.advance_observed(&schema, &observation) {
                    Ok(false) => {}
                    Ok(true) => {
                        assert!(limit >= needed);
                        assert_eq!(pending.finish().unwrap(), "A");
                        assert_eq!(budget.used.get(), needed);
                        break;
                    }
                    Err(error) => {
                        assert!(limit < needed);
                        assert_eq!(
                            error,
                            PreparationError::Observation("schema storage denied".into())
                        );
                        assert!(pending.advance(&schema).is_err());
                        assert!(pending.finish().is_err());
                        break;
                    }
                }
            }
        }
    }
}

#[test]
fn observed_class_key_cancelled_closed_and_raw_selection_recovers_exact_identity() {
    let (closed_schema, closed) = class_key_fixture();
    let raw_schema = raw_validation_schema();
    let (raw_key, raw) = raw_schema.instantiations.iter().next().unwrap();
    for (schema, target, expected) in [
        (&closed_schema, &closed, "A"),
        (&raw_schema, raw, raw_key.as_str()),
    ] {
        let budget = probe(usize::MAX, usize::MAX);
        let observation = InvocationContext::new(budget.clone());
        let mut baseline = ClassKeyPreparation::new_observed(target, &observation).unwrap();
        while !baseline.advance_observed(schema, &observation).unwrap() {}
        assert_eq!(baseline.finish().unwrap(), expected);
        for stop in 0..budget.checkpoints.get() {
            let stopped = InvocationContext::new(probe(usize::MAX, stop));
            let mut pending = match ClassKeyPreparation::new_observed(target, &stopped) {
                Ok(value) => value,
                Err(error) => {
                    assert_eq!(
                        error,
                        PreparationError::Observation("schema cancelled".into())
                    );
                    continue;
                }
            };
            loop {
                match pending.advance_observed(schema, &stopped) {
                    Ok(false) => {}
                    Ok(true) => panic!("missed class-key cancellation"),
                    Err(error) => {
                        assert_eq!(
                            error,
                            PreparationError::Observation("schema cancelled".into())
                        );
                        assert!(pending.advance(schema).is_err());
                        assert!(pending.finish().is_err());
                        break;
                    }
                }
            }
        }
        let mut recovery = ClassKeyPreparation::new(target);
        while !recovery.advance(schema).unwrap() {}
        assert_eq!(recovery.finish().unwrap(), expected);
    }
    let observation = InvocationContext::new(probe(usize::MAX, usize::MAX));
    let mut raw_only = ClassKeyPreparation::instantiated_observed(raw, &observation).unwrap();
    while !raw_only
        .advance_observed(&raw_schema, &observation)
        .unwrap()
    {}
    assert_eq!(raw_only.finish().unwrap(), *raw_key);
    let mut ambiguous = closed_schema.clone();
    let mut class = ambiguous.classes["A"].clone();
    class.name = "ZZ".into();
    ambiguous.classes.insert("ZZ".into(), class);
    ambiguous
        .declarations
        .insert("ZZ".into(), ambiguous.declarations["A"].clone());
    let mut pending = ClassKeyPreparation::new_observed(&closed, &observation).unwrap();
    loop {
        match pending.advance_observed(&ambiguous, &observation) {
            Ok(false) => {}
            Ok(true) => panic!("ambiguous declaration accepted"),
            Err(error) => {
                assert_eq!(
                    error,
                    PreparationError::Schema("Ambiguous closed class declaration".into())
                );
                assert!(pending.finish().is_err());
                break;
            }
        }
    }
}

#[test]
fn observed_reference_conformance_has_literal_limits_and_never_turns_storage_denial_into_false() {
    let (mut schema, _) = reference_fixture();
    let original = schema.clone();
    // Inputs B/y/A 195 + effective B.y 1273 + target A 65 +
    // class conformance A/A (130 keys+452 ancestry) = 2115.
    // Two additional raw A bounds each add 582: 3279.
    for (bounds, needed) in [(vec![], 2115usize), (vec!["A".into(), "A".into()], 3279)] {
        schema
            .raw_reference_bounds
            .insert("B".into(), [("y".into(), bounds)].into());
        for limit in [
            0,
            194,
            195,
            196,
            356,
            357,
            358,
            1467,
            1468,
            1469,
            1532,
            1533,
            1534,
            1662,
            1663,
            1664,
            needed - 1,
            needed,
            needed + 1,
        ] {
            let budget = probe(limit, usize::MAX);
            let observation = InvocationContext::new(budget.clone());
            let result = run_reference_observed(&schema, "B", "y", "A", &observation);
            if limit < needed {
                assert_eq!(
                    result.unwrap_err(),
                    PreparationError::Observation("schema storage denied".into())
                );
            } else {
                assert!(result.unwrap());
                assert_eq!(budget.used.get(), needed);
            }
            assert!(budget.used.get() <= limit);
        }
    }
    schema.raw_reference_bounds.clear();
    assert_eq!(schema, original);
    let budget = probe(usize::MAX, usize::MAX);
    let observation = InvocationContext::new(budget.clone());
    assert!(!run_reference_observed(&schema, "B", "y", "B", &observation).unwrap());
    assert_eq!(budget.used.get(), 2115); // Same-width unrelated class, same scratch.
    schema.classes.get_mut("C").unwrap().parents = vec!["Missing".into()];
    // Invalid actual ancestry historically means predicate false, while resource
    // errors above must propagate. The receiver/member ancestry is valid.
    assert!(!run_reference_observed(&schema, "B", "y", "C", &observation).unwrap());
    assert!(!run_reference_observed(&schema, "B", "y", "Missing", &observation).unwrap());
}

#[test]
fn observed_inherited_reference_bounds_cancel_every_checkpoint_without_false_publication() {
    let (mut schema, _) = reference_fixture();
    schema.raw_reference_bounds.insert(
        "B".into(),
        [("y".into(), vec!["B".into(), "C".into()])].into(),
    );
    let original = schema.clone();
    let budget = probe(usize::MAX, usize::MAX);
    let observation = InvocationContext::new(budget.clone());
    assert!(run_reference_observed(&schema, "D", "y", "D", &observation).unwrap());
    for stop in 0..budget.checkpoints.get() {
        let stopped = InvocationContext::new(probe(usize::MAX, stop));
        assert_eq!(
            run_reference_observed(&schema, "D", "y", "D", &stopped).unwrap_err(),
            PreparationError::Observation("schema cancelled".into())
        );
    }
    let mut recovery = ReferenceConformancePreparation::new("D", "y", "D");
    while !recovery.advance(&schema).unwrap() {}
    assert!(recovery.finish().unwrap());
    assert_eq!(schema, original);
    schema
        .raw_reference_bounds
        .get_mut("B")
        .unwrap()
        .get_mut("y")
        .unwrap()[1] = "Missing".into();
    assert!(!run_reference_observed(&schema, "D", "y", "D", &observation).unwrap());
}

#[test]
fn observed_runtime_reference_domains_preserve_exact_alternatives_unresolved_errors_and_cancellation()
 {
    use crate::definitions::{
        bundle::{DefinitionBundle, DefinitionSource, SourceKind},
        generic_types::{RuntimeType, Type},
    };
    let source = DefinitionSource::new(
        "memory:observed-reference",
        SourceKind::Ecore,
        include_str!("../../assets/fixtures/ecore-generics/raw-feature-domains.ecore"),
    );
    let (source, ids) = crate::definitions::migration_test_fixture::source_with_ids(source);
    let ty = |n, arguments, raw| {
        RuntimeType::new(Type::Classifier {
            declaration: ids[&vec![n]].clone(),
            arguments,
            raw,
        })
        .unwrap()
    };
    let a = ty(1, vec![], false);
    let b = ty(2, vec![], false);
    let other = ty(3, vec![], false);
    let pair = ty(
        4,
        vec![a.expression().clone(), b.expression().clone()],
        false,
    );
    let raw = ty(5, vec![], true);
    let mut schema = crate::definitions::compiler::materialize_runtime_types(
        &DefinitionBundle {
            version: 1,
            sources: vec![source.clone()],
            namespaces: vec![],
        },
        &[a, b, other.clone(), pair.clone(), raw.clone()],
        None,
    )
    .unwrap();
    let receiver = schema.class_key_for_type(&raw).unwrap().to_owned();
    let target = schema.class_key_for_type(&pair).unwrap().to_owned();
    let foreign = schema.class_key_for_type(&other).unwrap().to_owned();
    assert_eq!(
        schema.runtime_feature_domains[&receiver][0]
            .source
            .projection
            .source
            .name,
        "pair"
    );
    let observation = InvocationContext::new(probe(usize::MAX, usize::MAX));
    assert!(run_reference_observed(&schema, &receiver, "pair", &target, &observation).unwrap());
    assert!(!run_reference_observed(&schema, &receiver, "pair", &foreign, &observation).unwrap());
    // Repeated synthetic alternatives isolate scheduling; this mutation is not
    // a claim that definition validation admits repeated source alternatives.
    let domain = &mut schema.runtime_feature_domains.get_mut(&receiver).unwrap()[0];
    domain.alternatives = vec![foreign.clone(); 64];
    domain.alternatives.push(target.clone());
    let budget = probe(usize::MAX, usize::MAX);
    let baseline = InvocationContext::new(budget.clone());
    assert!(run_reference_observed(&schema, &receiver, "pair", &target, &baseline).unwrap());
    let original = schema.clone();
    for stop in 0..budget.checkpoints.get() {
        let stopped = InvocationContext::new(probe(usize::MAX, stop));
        assert_eq!(
            run_reference_observed(&schema, &receiver, "pair", &target, &stopped).unwrap_err(),
            PreparationError::Observation("schema cancelled".into())
        );
    }
    assert_eq!(schema, original);
    let domain = &mut schema.runtime_feature_domains.get_mut(&receiver).unwrap()[0];
    domain.alternatives.pop();
    domain
        .unresolved
        .push(super::super::UnresolvedRuntimeFeatureCandidate {
            candidate: pair,
            target: target.clone(),
            reason: "pinned uncertainty".into(),
        });
    assert_eq!(
        run_reference_observed(&schema, &receiver, "pair", &target, &observation).unwrap_err(),
        PreparationError::Schema(
            "Unresolved runtime feature membership: pinned uncertainty".into()
        )
    );
    schema.runtime_feature_domains.get_mut(&receiver).unwrap()[0]
        .alternatives
        .push(target.clone());
    assert!(run_reference_observed(&schema, &receiver, "pair", &target, &observation).unwrap());
}

#[test]
fn observed_schema_validation_has_literal_class_feature_and_raw_bound_policies() {
    use super::super::InstantiationValidation;
    let (mut raw, _) = reference_fixture();
    raw.raw_reference_bounds
        .insert("B".into(), [("y".into(), vec!["A".into()])].into());
    // Empty A/B/C/D: three next-class cursor copies, 3*65=195.
    // Legacy A.x: receiver/Feature/field cursor 65+3265+65=3395,
    // plus the same three class cursors: 3590.
    // Raw B.y: feature shorter by six target bytes -> 3389, class cursors
    // 195 -> 3584; B map 5019, receiver/field cursors 130 -> total 8733.
    for (schema, needed) in [
        (schema(), 195usize),
        (legacy_feature_schema(), 3590),
        (raw.clone(), 8733),
    ] {
        let original = schema.clone();
        for limit in [0, needed - 1, needed, needed + 1] {
            let budget = probe(limit, usize::MAX);
            let observation = InvocationContext::new(budget.clone());
            let mut pending = InstantiationValidation::new();
            loop {
                match pending.advance_observed(&schema, &observation) {
                    Ok(false) => {}
                    Ok(true) => {
                        assert!(limit >= needed);
                        assert_eq!(budget.used.get(), needed);
                        assert!(pending.advance(&schema).unwrap());
                        break;
                    }
                    Err(error) => {
                        assert!(limit < needed);
                        assert_eq!(
                            error,
                            PreparationError::Observation("schema storage denied".into())
                        );
                        assert!(pending.advance(&schema).is_err());
                        break;
                    }
                }
            }
            assert!(budget.used.get() <= limit);
            assert_eq!(schema, original);
        }
    }
    raw.raw_reference_bounds
        .get_mut("B")
        .unwrap()
        .get_mut("y")
        .unwrap()[0] = "Missing".into();
    let observation = InvocationContext::new(probe(usize::MAX, usize::MAX));
    let mut pending = InstantiationValidation::new();
    loop {
        match pending.advance_observed(&raw, &observation) {
            Ok(false) => {}
            Ok(true) => panic!("invalid raw bound accepted"),
            Err(error) => {
                assert_eq!(
                    error,
                    PreparationError::Schema("Invalid raw reference bound projection".into())
                );
                assert!(pending.advance(&raw).is_err());
                break;
            }
        }
    }
}

#[test]
fn observed_materialized_validation_cancels_every_checkpoint_and_recovers() {
    use super::super::InstantiationValidation;
    let schema = raw_validation_schema();
    let original = schema.clone();
    assert!(!schema.instantiations.is_empty());
    assert!(
        schema
            .runtime_feature_domains
            .values()
            .any(|v| !v.is_empty())
    );
    let budget = probe(usize::MAX, usize::MAX);
    let observation = InvocationContext::new(budget.clone());
    let mut baseline = InstantiationValidation::new();
    while !baseline.advance_observed(&schema, &observation).unwrap() {}
    let needed = budget.used.get();
    assert!(needed > 8733);
    for stop in 0..budget.checkpoints.get() {
        let stopped = InvocationContext::new(probe(usize::MAX, stop));
        let mut pending = InstantiationValidation::new();
        loop {
            match pending.advance_observed(&schema, &stopped) {
                Ok(false) => {}
                Ok(true) => panic!("missed materialized schema cancellation"),
                Err(error) => {
                    assert_eq!(
                        error,
                        PreparationError::Observation("schema cancelled".into())
                    );
                    assert!(pending.advance(&schema).is_err());
                    break;
                }
            }
        }
    }
    for limit in [0, needed - 1, needed, needed + 1] {
        let budget = probe(limit, usize::MAX);
        let observation = InvocationContext::new(budget.clone());
        let mut pending = InstantiationValidation::new();
        loop {
            match pending.advance_observed(&schema, &observation) {
                Ok(false) => {}
                Ok(true) => {
                    assert!(limit >= needed);
                    assert_eq!(budget.used.get(), needed);
                    break;
                }
                Err(error) => {
                    assert!(limit < needed);
                    assert_eq!(
                        error,
                        PreparationError::Observation("schema storage denied".into())
                    );
                    assert!(pending.advance(&schema).is_err());
                    break;
                }
            }
        }
    }
    schema.validate_instantiations().unwrap();
    assert_eq!(schema, original);
}

#[test]
fn observed_domain_duplicate_comparisons_yield_for_long_utf8_prefixes_and_cancel_exactly() {
    use super::super::{InstantiationValidation, InstantiationValidationStage};
    let mut schema = raw_validation_schema();
    let valid = schema.clone();
    let receiver = schema
        .runtime_feature_domains
        .iter()
        .find(|(_, v)| !v.is_empty())
        .unwrap()
        .0
        .clone();
    let prefix = "é😀".repeat(1400);
    let high = format!("{prefix}z");
    let low = format!("{prefix}a");
    schema.runtime_feature_domains.get_mut(&receiver).unwrap()[0].alternatives =
        vec![high.clone(), low, high];
    let original = schema.clone();
    let start = || {
        let mut pending = InstantiationValidation::new();
        pending.stage = InstantiationValidationStage::DomainDuplicates;
        pending.outer = Some(receiver.clone());
        pending
    };
    let budget = probe(usize::MAX, usize::MAX);
    let observation = InvocationContext::new(budget.clone());
    let mut pending = start();
    let mut advances = 0;
    loop {
        advances += 1;
        match pending.advance_observed(&schema, &observation) {
            Ok(false) => {}
            Ok(true) => panic!("duplicate long-prefix domain was accepted"),
            Err(error) => {
                assert_eq!(
                    error,
                    PreparationError::Schema("Duplicate runtime domain alternative".into())
                );
                break;
            }
        }
    }
    // The actual schema continuation, not just the index helper, yields across
    // both byte comparisons. No copied source strings enter its storage ledger.
    assert!(advances > prefix.len() / 256 * 2);
    assert_eq!(budget.used.get(), 64);
    for stop in 0..budget.checkpoints.get() {
        let observation = InvocationContext::new(probe(usize::MAX, stop));
        let mut cancelled = start();
        loop {
            match cancelled.advance_observed(&schema, &observation) {
                Ok(false) => {}
                Ok(true) => panic!("missed long-prefix duplicate cancellation"),
                Err(error) => {
                    assert_eq!(
                        error,
                        PreparationError::Observation("schema cancelled".into())
                    );
                    assert!(cancelled.advance(&schema).is_err());
                    break;
                }
            }
        }
    }
    assert_eq!(schema, original);
    valid.validate_instantiations().unwrap();
}

#[test]
fn observed_validation_duplicate_sets_precharge_once_and_preserve_schema_precedence() {
    use super::super::{InstantiationValidation, InstantiationValidationStage};
    let mut schema = raw_validation_schema();
    let receiver = schema
        .runtime_feature_domains
        .iter()
        .find(|(_, v)| !v.is_empty())
        .unwrap()
        .0
        .clone();
    schema.runtime_feature_domains.get_mut(&receiver).unwrap()[0].alternatives =
        vec!["é".into(), "é".into()];
    // Seed the private stage to isolate its retention delta from earlier caches.
    // The UTF-8 alternative stays borrowed; one primitive index reserves 32.
    // Search and insertion are separate bounded advances. A duplicate retains
    // no second slot and still precedes unknown-target validation.
    for limit in [0, 31, 32, 33] {
        let budget = probe(limit, usize::MAX);
        let observation = InvocationContext::new(budget.clone());
        let mut pending = InstantiationValidation::new();
        pending.stage = InstantiationValidationStage::DomainDuplicates;
        pending.outer = Some(receiver.clone());
        let first = loop {
            match pending.advance_observed(&schema, &observation) {
                Ok(false) if budget.used.get() == 0 => {}
                result => break result,
            }
        };
        if limit < 32 {
            assert_eq!(
                first.unwrap_err(),
                PreparationError::Observation("schema storage denied".into())
            );
            assert_eq!(budget.used.get(), 0);
            assert!(pending.alternatives.is_empty());
        } else {
            assert!(!first.unwrap());
            assert_eq!(budget.used.get(), 32);
            let duplicate = loop {
                match pending.advance_observed(&schema, &observation) {
                    Ok(false) => {}
                    result => break result,
                }
            };
            assert_eq!(
                duplicate.unwrap_err(),
                PreparationError::Schema("Duplicate runtime domain alternative".into())
            );
            assert_eq!(budget.used.get(), 32);
        }
        assert!(pending.advance(&schema).is_err());
    }
    let (key, ty) = schema.instantiations.iter().next().unwrap();
    assert_eq!(ty.identity().len(), 84); // 20-byte prefix + 64 hex digits.
    for limit in [0, 147, 148, 149] {
        let budget = probe(limit, usize::MAX);
        let observation = InvocationContext::new(budget.clone());
        let mut pending = InstantiationValidation::new();
        pending.stage = InstantiationValidationStage::InstantiationIdentity;
        pending.outer = Some(key.clone());
        if limit < 148 {
            assert_eq!(
                pending.advance_observed(&schema, &observation).unwrap_err(),
                PreparationError::Observation("schema storage denied".into())
            );
            assert!(pending.identities.is_empty());
            assert_eq!(budget.used.get(), 0);
        } else {
            assert!(!pending.advance_observed(&schema, &observation).unwrap());
            assert_eq!(budget.used.get(), 148);
            pending.stage = InstantiationValidationStage::InstantiationIdentity;
            assert_eq!(
                pending.advance_observed(&schema, &observation).unwrap_err(),
                PreparationError::Schema("Duplicate instantiated class identity".into())
            );
            assert_eq!(budget.used.get(), 148);
        }
        assert!(pending.advance(&schema).is_err());
    }
}

#[test]
fn observed_reference_structure_precharges_literal_inputs_and_nested_map() {
    let (schema, field) = reference_fixture();
    let original = schema.clone();
    // Baseline Feature 3265; shorter target removes 6 bytes. Some("B/y")
    // replaces a 4-byte None with 1+5 bytes and one extra 128-byte node.
    // Receiver 65 + Feature 3389 = 3454; split owner/name adds 130.
    // Independent B.y map is the existing 5025-byte legacy policy minus
    // six target bytes: 5019. Total 3584+5019 = 8603.
    for limit in [
        0, 64, 65, 66, 3453, 3454, 3455, 3583, 3584, 3585, 8602, 8603, 8604,
    ] {
        let budget = probe(limit, usize::MAX);
        let observation = InvocationContext::new(budget.clone());
        let mut pending =
            match ReferenceStructurePreparation::new_observed("A", &field, &observation) {
                Ok(value) => value,
                Err(error) => {
                    assert!(limit < 3454);
                    assert_eq!(
                        error,
                        PreparationError::Observation("schema storage denied".into())
                    );
                    assert_eq!(budget.used.get(), if limit < 65 { 0 } else { 65 });
                    continue;
                }
            };
        loop {
            match pending.advance_observed(&schema, &observation) {
                Ok(false) => {}
                Ok(true) => {
                    assert!(limit >= 8603);
                    assert_eq!(budget.used.get(), 8603);
                    pending.finish().unwrap();
                    break;
                }
                Err(error) => {
                    assert!(limit < 8603);
                    assert_eq!(
                        error,
                        PreparationError::Observation("schema storage denied".into())
                    );
                    assert!(budget.used.get() <= limit);
                    assert!(pending.advance(&schema).is_err());
                    assert!(pending.finish().is_err());
                    break;
                }
            }
        }
    }
    assert_eq!(schema, original);
}

#[test]
fn observed_reference_structure_cancellation_and_schema_errors_preserve_precedence() {
    let (mut schema, field) = reference_fixture();
    let budget = probe(usize::MAX, usize::MAX);
    let observation = InvocationContext::new(budget.clone());
    let mut baseline =
        ReferenceStructurePreparation::new_observed("A", &field, &observation).unwrap();
    while !baseline.advance_observed(&schema, &observation).unwrap() {}
    baseline.finish().unwrap();
    for stop in 0..budget.checkpoints.get() {
        let observation = InvocationContext::new(probe(usize::MAX, stop));
        let mut pending =
            match ReferenceStructurePreparation::new_observed("A", &field, &observation) {
                Err(error) => {
                    assert_eq!(
                        error,
                        PreparationError::Observation("schema cancelled".into())
                    );
                    continue;
                }
                Ok(value) => value,
            };
        loop {
            match pending.advance_observed(&schema, &observation) {
                Ok(false) => {}
                Ok(true) => panic!("missed reference cancellation"),
                Err(error) => {
                    assert_eq!(
                        error,
                        PreparationError::Observation("schema cancelled".into())
                    );
                    assert!(pending.advance(&schema).is_err());
                    assert!(pending.finish().is_err());
                    break;
                }
            }
        }
    }
    let mut repaired = ReferenceStructurePreparation::new("A", &field);
    while !repaired.advance(&schema).unwrap() {}
    repaired.finish().unwrap();
    schema
        .classes
        .get_mut("B")
        .unwrap()
        .features
        .get_mut("y")
        .unwrap()
        .containment = true;
    let observation = InvocationContext::new(probe(usize::MAX, usize::MAX));
    let mut invalid =
        ReferenceStructurePreparation::new_observed("A", &field, &observation).unwrap();
    loop {
        match invalid.advance_observed(&schema, &observation) {
            Ok(false) => {}
            Ok(true) => panic!("container Many must fail"),
            Err(error) => {
                assert_eq!(
                    error,
                    PreparationError::Schema(
                        "Container reference must have upper bound 1: A.x".into()
                    )
                );
                assert!(invalid.advance(&schema).is_err());
                assert!(invalid.finish().is_err());
                break;
            }
        }
    }
    for (opposite, unique, message) in [
        ("malformed", true, "Invalid opposite identity"),
        (
            "B/y",
            false,
            "Containment or bidirectional reference must be unique when upper bound differs from 1: A.x",
        ),
    ] {
        let mut bad = field.clone();
        bad.opposite = Some(opposite.into());
        bad.unique = unique;
        let mut pending =
            ReferenceStructurePreparation::new_observed("A", &bad, &observation).unwrap();
        assert_eq!(
            pending.advance_observed(&schema, &observation).unwrap_err(),
            PreparationError::Schema(message.into())
        );
        assert!(pending.finish().is_err());
    }
}

#[test]
fn observed_reference_owner_selection_rejects_inherited_self_opposite_and_recovers() {
    let mut schema = legacy_feature_schema();
    let mut field = feature();
    field.target = "A".into();
    field.reference = true;
    field.opposite = Some("A/x".into());
    schema
        .classes
        .get_mut("A")
        .unwrap()
        .features
        .insert("x".into(), field.clone());
    // Inputs+opposite 3584, each declaring-owner preparation 130+1273+65=1468.
    // Both owners resolve to A: total 6520 before the semantic rejection.
    for limit in [0, 3453, 3583, 3584, 5051, 5052, 5053, 6519, 6520, 6521] {
        let budget = probe(limit, usize::MAX);
        let observation = InvocationContext::new(budget.clone());
        let mut pending =
            match ReferenceStructurePreparation::new_observed("A", &field, &observation) {
                Ok(value) => value,
                Err(error) => {
                    assert!(limit < 3454);
                    assert!(matches!(error, PreparationError::Observation(_)));
                    continue;
                }
            };
        loop {
            match pending.advance_observed(&schema, &observation) {
                Ok(false) => {}
                Ok(true) => panic!("self opposite must fail"),
                Err(error) => {
                    if limit < 6520 {
                        assert_eq!(
                            error,
                            PreparationError::Observation("schema storage denied".into())
                        );
                    } else {
                        assert_eq!(budget.used.get(), 6520);
                        assert_eq!(
                            error,
                            PreparationError::Schema(
                                "A.x: A reference cannot be its own opposite".into()
                            )
                        );
                    }
                    assert!(pending.advance(&schema).is_err());
                    assert!(pending.finish().is_err());
                    break;
                }
            }
        }
    }
    let inherited_budget = probe(usize::MAX, usize::MAX);
    let observation = InvocationContext::new(inherited_budget.clone());
    let mut inherited =
        ReferenceStructurePreparation::new_observed("D", &field, &observation).unwrap();
    loop {
        match inherited.advance_observed(&schema, &observation) {
            Ok(false) => {}
            Ok(true) => panic!("inherited owner must still be A"),
            Err(error) => {
                assert_eq!(
                    error,
                    PreparationError::Schema("D.x: A reference cannot be its own opposite".into())
                );
                break;
            }
        }
    }
    for stop in 0..inherited_budget.checkpoints.get() {
        let stopped = InvocationContext::new(probe(usize::MAX, stop));
        let mut pending = match ReferenceStructurePreparation::new_observed("D", &field, &stopped) {
            Ok(value) => value,
            Err(error) => {
                assert_eq!(
                    error,
                    PreparationError::Observation("schema cancelled".into())
                );
                continue;
            }
        };
        loop {
            match pending.advance_observed(&schema, &stopped) {
                Ok(false) => {}
                Ok(true) => panic!("missed inherited owner cancellation"),
                Err(error) => {
                    assert_eq!(
                        error,
                        PreparationError::Observation("schema cancelled".into())
                    );
                    assert!(pending.advance(&schema).is_err());
                    assert!(pending.finish().is_err());
                    break;
                }
            }
        }
    }
    field.opposite = None;
    let mut recovered =
        ReferenceStructurePreparation::new_observed("D", &field, &observation).unwrap();
    assert!(recovered.advance_observed(&schema, &observation).unwrap());
    recovered.finish().unwrap();
}

#[test]
fn observed_default_and_legacy_feature_map_have_literal_storage_boundaries() {
    let schema = legacy_feature_schema();
    let original = schema.clone();
    // Feature: struct + 11 key/value pairs = 23 nodes; 321 metadata
    // bytes (75 key bytes, 244 value bytes, 2 container bytes).
    // 23*128+321 = 3265; receiver A is 65: default preparation 3330.
    for limit in [0, 64, 65, 3329, 3330, 3331] {
        let budget = probe(limit, usize::MAX);
        let observation = InvocationContext::new(budget.clone());
        match FeatureDefaultPreparation::new_observed("A", &feature(), &observation) {
            Err(error) => {
                assert!(limit < 3330);
                assert_eq!(
                    error,
                    PreparationError::Observation("schema storage denied".into())
                );
            }
            Ok(mut pending) => {
                assert!(limit >= 3330);
                assert!(pending.advance_observed(&schema, &observation).unwrap());
                assert_eq!(pending.finish().unwrap(), feature());
                assert_eq!(budget.used.get(), 3330);
            }
        }
    }
    // Map receiver 65 + effective kind 65 + ancestry 452 + cursor/order/
    // cursor 195 + legacy identity 334 + selected-member retention 227
    // + owner 65 + default 3330 + publication 292 = 5025.
    let prefixes = [
        0, 65, 130, 227, 324, 356, 485, 582, 647, 712, 777, 1111, 1338, 1403, 1468, 4733, 5025,
    ];
    for limit in [
        0, 64, 65, 129, 130, 226, 227, 323, 324, 355, 356, 484, 485, 581, 582, 646, 647, 711, 712,
        776, 777, 1110, 1111, 1337, 1338, 1402, 1403, 1467, 1468, 4732, 4733, 5024, 5025, 5026,
    ] {
        let budget = probe(limit, usize::MAX);
        let observation = InvocationContext::new(budget.clone());
        match FeatureMapPreparation::new_observed("A", &observation) {
            Err(error) => {
                assert!(limit < 227);
                assert_eq!(
                    error,
                    PreparationError::Observation("schema storage denied".into())
                );
            }
            Ok(mut pending) => loop {
                match pending.advance_observed(&schema, &observation) {
                    Ok(false) => {}
                    Ok(true) => {
                        assert!(limit >= 5025);
                        let fields = pending.finish().unwrap();
                        assert_eq!(fields.fields(), &[("x".into(), feature())].into());
                        assert_eq!(fields.order(), ["x"]);
                        assert_eq!(
                            fields.member_identities()["x"],
                            MemberIdentity::Programmatic {
                                revision: schema.revision.clone(),
                                namespace: "urn:observed".into(),
                                owner: "A".into(),
                                name: "x".into(),
                            }
                        );
                        break;
                    }
                    Err(error) => {
                        assert!(limit < 5025);
                        assert_eq!(
                            error,
                            PreparationError::Observation("schema storage denied".into())
                        );
                        assert!(pending.advance(&schema).is_err());
                        assert!(pending.finish().is_err());
                        break;
                    }
                }
            },
        }
        assert_eq!(
            budget.used.get(),
            prefixes.into_iter().filter(|n| *n <= limit).max().unwrap()
        );
    }
    assert_eq!(schema, original);
}

#[test]
fn observed_feature_maps_cancel_every_checkpoint_and_recover_inherited_order() {
    let schema = legacy_feature_schema();
    let baseline = probe(usize::MAX, usize::MAX);
    let observation = InvocationContext::new(baseline.clone());
    let mut pending = FeatureMapPreparation::new_observed("D", &observation).unwrap();
    while !pending.advance_observed(&schema, &observation).unwrap() {}
    let expected = pending.finish().unwrap();
    assert_eq!(expected.order(), ["x"]);
    assert_eq!(expected.fields()["x"], feature());
    for stop in 0..baseline.checkpoints.get() {
        let observation = InvocationContext::new(probe(usize::MAX, stop));
        match FeatureMapPreparation::new_observed("D", &observation) {
            Err(error) => assert_eq!(
                error,
                PreparationError::Observation("schema cancelled".into())
            ),
            Ok(mut pending) => loop {
                match pending.advance_observed(&schema, &observation) {
                    Ok(false) => {}
                    Ok(true) => panic!("missed cancellation"),
                    Err(error) => {
                        assert_eq!(
                            error,
                            PreparationError::Observation("schema cancelled".into())
                        );
                        assert!(pending.advance(&schema).is_err());
                        assert!(pending.finish().is_err());
                        break;
                    }
                }
            },
        }
    }
    let mut recovered = FeatureMapPreparation::new("D");
    while !recovered.advance(&schema).unwrap() {}
    let recovered = recovered.finish().unwrap();
    assert_eq!(recovered.fields(), expected.fields());
    assert_eq!(recovered.member_identities(), expected.member_identities());
    assert_eq!(recovered.order(), expected.order());
}

#[test]
fn unobserved_default_projection_does_not_acquire_an_observed_metadata_ceiling() {
    let schema = schema();
    let mut wide = feature();
    wide.default = serde_json::Value::Array(vec![serde_json::Value::Null; 65536]);
    let mut legacy = FeatureDefaultPreparation::new("A", &wide);
    assert!(legacy.advance(&schema).unwrap());
    assert_eq!(legacy.finish().unwrap(), wide);
    let observation = InvocationContext::new(probe(usize::MAX, usize::MAX));
    let error = FeatureDefaultPreparation::new_observed("A", &wide, &observation)
        .err()
        .unwrap();
    assert!(matches!(error, PreparationError::Observation(_)));
    assert_eq!(wide.default.as_array().unwrap().len(), 65536);
}

#[test]
fn observed_source_member_identity_copies_have_literal_reservation_boundaries() {
    use crate::definitions::bundle::{DefinitionBundle, DefinitionSource, SourceKind};
    let source = DefinitionSource::new(
        "memory:observed-source",
        SourceKind::Rcore,
        r##"rcore 1; package t version "1"; metamodel "urn:observed" prefix t { class A { attr x: String [0,1]; } }"##,
    );
    assert_eq!(source.id.uri.len(), 22);
    let schema = crate::definitions::compiler::compile_structure_bundle(
        &DefinitionBundle {
            version: 1,
            sources: vec![source.clone()],
            namespaces: vec![],
        },
        &source.id,
        "urn:observed",
    )
    .unwrap();
    // Each declaration has 11 metadata nodes and 179 metadata bytes:
    // 11*128+179 = 1587. Three copies replace the legacy identity's 334;
    // its declaration cursor adds 65: 1273 - 334 + 3*1587 + 65 = 5765.
    let prefixes = [
        0, 65, 162, 259, 291, 420, 517, 582, 647, 712, 777, 2364, 3951, 5538, 5765,
    ];
    for limit in [
        0, 64, 65, 161, 162, 776, 777, 2363, 2364, 3950, 3951, 5537, 5538, 5764, 5765, 5766,
    ] {
        let budget = probe(limit, usize::MAX);
        let observation = InvocationContext::new(budget.clone());
        match EffectiveFeaturePreparation::new_observed("A", &observation) {
            Err(error) => {
                assert!(limit < 162);
                assert_eq!(
                    error,
                    PreparationError::Observation("schema storage denied".into())
                );
            }
            Ok(mut pending) => loop {
                match pending.advance_observed(&schema, &observation) {
                    Ok(false) => {}
                    Ok(true) => {
                        assert!(limit >= 5765);
                        let members = pending.finish().unwrap();
                        assert_eq!(members.len(), 1);
                        assert_eq!(
                            (
                                members[0].owner.as_str(),
                                members[0].name.as_str(),
                                members[0].position
                            ),
                            ("A", "x", 0)
                        );
                        assert_eq!(
                            members[0].identity,
                            MemberIdentity::Source {
                                declaration: crate::definitions::resources::DeclarationId {
                                    source: source.id.clone(),
                                    path: vec![0, 0]
                                },
                            }
                        );
                        break;
                    }
                    Err(error) => {
                        assert!(limit < 5765);
                        assert_eq!(
                            error,
                            PreparationError::Observation("schema storage denied".into())
                        );
                        assert!(pending.finish().is_err());
                        break;
                    }
                }
            },
        }
        assert_eq!(
            budget.used.get(),
            prefixes.into_iter().filter(|n| *n <= limit).max().unwrap()
        );
    }
}

#[test]
fn observed_runtime_defaults_charge_only_first_copy_and_preserve_conflicts() {
    use crate::definitions::{
        bundle::{DefinitionBundle, DefinitionSource, SourceKind},
        generic_types::{RuntimeType, Type},
    };
    let source = DefinitionSource::new(
        "memory:observed-defaults",
        SourceKind::Ecore,
        include_str!("../../assets/fixtures/ecore-generics/raw-feature-domains.ecore"),
    );
    let (source, ids) = crate::definitions::migration_test_fixture::source_with_ids(source);
    let raw = RuntimeType::new(Type::Classifier {
        declaration: ids[&vec![8]].clone(),
        arguments: vec![],
        raw: true,
    })
    .unwrap();
    let mut schema = crate::definitions::compiler::materialize_runtime_types(
        &DefinitionBundle {
            version: 1,
            sources: vec![source],
            namespaces: vec![],
        },
        &[raw],
        None,
    )
    .unwrap();
    let (receiver, template) = schema
        .runtime_feature_domains
        .iter()
        .find_map(|(receiver, domains)| {
            domains
                .first()
                .map(|domain| (receiver.clone(), domain.clone()))
        })
        .unwrap();
    assert_eq!(template.source.projection.source.name, "value");
    let mut field = feature();
    field.name = "value".into();
    // Feature payload has four more name bytes than x: 3269. Receiver
    // retention adds 64+len. Values: null 132; [] 130; é 132;
    // [é,null] 394; {items:[é,null]} 659 under the shared node policy.
    for (value, payload) in [
        (serde_json::json!(null), 132),
        (serde_json::json!([]), 130),
        (serde_json::json!("é"), 132),
        (serde_json::json!(["é", null]), 394),
        (serde_json::json!({"items":["é",null]}), 659),
    ] {
        let mut domain = template.clone();
        domain.default = value.clone();
        schema
            .runtime_feature_domains
            .insert(receiver.clone(), vec![domain; 3]);
        let initial = 3333 + receiver.len();
        for limit in [
            initial - 1,
            initial,
            initial + payload - 1,
            initial + payload,
            initial + payload + 1,
        ] {
            let budget = probe(limit, usize::MAX);
            let observation = InvocationContext::new(budget.clone());
            match FeatureDefaultPreparation::new_observed(&receiver, &field, &observation) {
                Err(error) => {
                    assert!(limit < initial);
                    assert_eq!(
                        error,
                        PreparationError::Observation("schema storage denied".into())
                    );
                }
                Ok(mut pending) => match pending.advance_observed(&schema, &observation) {
                    Err(error) => {
                        assert!(limit < initial + payload);
                        assert_eq!(
                            error,
                            PreparationError::Observation("schema storage denied".into())
                        );
                        assert!(pending.finish().is_err());
                    }
                    Ok(done) => {
                        assert!(!done);
                        assert!(limit >= initial + payload);
                        let used = budget.used.get();
                        assert_eq!(used, initial + payload);
                        assert!(!pending.advance_observed(&schema, &observation).unwrap());
                        assert!(!pending.advance_observed(&schema, &observation).unwrap());
                        assert!(pending.advance_observed(&schema, &observation).unwrap());
                        assert_eq!(budget.used.get(), used);
                        assert_eq!(pending.finish().unwrap().default, value);
                    }
                },
            }
        }
        schema.runtime_feature_domains.get_mut(&receiver).unwrap()[2].default =
            serde_json::json!("conflict");
        let budget = probe(usize::MAX, usize::MAX);
        let observation = InvocationContext::new(budget.clone());
        let mut pending =
            FeatureDefaultPreparation::new_observed(&receiver, &field, &observation).unwrap();
        assert!(!pending.advance_observed(&schema, &observation).unwrap());
        assert!(!pending.advance_observed(&schema, &observation).unwrap());
        let before = budget.used.get();
        assert_eq!(
            pending.advance_observed(&schema, &observation).unwrap_err(),
            PreparationError::Schema("Conflicting receiver-specific source defaults".into())
        );
        assert_eq!(budget.used.get(), before);
        assert!(pending.advance(&schema).is_err());
        assert!(pending.finish().is_err());
    }
}
struct Probe {
    used: Cell<usize>,
    limit: usize,
    checkpoints: Cell<usize>,
    stop: usize,
}

#[test]
fn runtime_attribute_codec_has_literal_selection_and_prepared_storage_policies() {
    use super::super::RuntimeAttributeConversionPreparation;
    let schema = legacy_feature_schema();
    let field = feature();
    let mut map = FeatureMapPreparation::new("A");
    while !map.advance(&schema).unwrap() {}
    let fields = map.finish().unwrap();
    for prepared in [false, true] {
        // Receiver 65 + Feature 3265 + codec's separate Feature 3265.
        // Fresh selection also retains the shared 5025-byte member map.
        let required = if prepared { 6595 } else { 11620 };
        for limit in [0, 64, 65, 3329, 3330, required - 1, required, required + 1] {
            let budget = probe(limit, usize::MAX);
            let observation = InvocationContext::new(budget.clone());
            let result = if prepared {
                RuntimeAttributeConversionPreparation::from_prepared_observed(
                    "A",
                    &field,
                    &fields,
                    &observation,
                )
            } else {
                RuntimeAttributeConversionPreparation::new_observed("A", &field, &observation)
            };
            match result {
                Err(error) => {
                    assert!(limit < required);
                    assert_eq!(
                        error,
                        PreparationError::Observation("schema storage denied".into())
                    );
                }
                Ok(mut pending) => loop {
                    match pending.advance_observed(&schema, None, &observation) {
                        Ok(false) => {}
                        Ok(true) => {
                            assert!(limit >= required);
                            assert_eq!(budget.used.get(), required);
                            pending.finish().unwrap();
                            break;
                        }
                        Err(error) => {
                            assert!(limit < required);
                            assert_eq!(
                                error,
                                PreparationError::Observation("schema storage denied".into())
                            );
                            assert!(pending.advance(&schema, None).is_err());
                            assert!(pending.finish().is_err());
                            break;
                        }
                    }
                },
            }
        }
    }
    let mut changed = field.clone();
    changed.default = serde_json::json!("other");
    let budget = probe(0, 0);
    let observation = InvocationContext::new(budget.clone());
    let error = RuntimeAttributeConversionPreparation::from_prepared_observed(
        "A",
        &changed,
        &fields,
        &observation,
    )
    .err()
    .unwrap();
    assert_eq!(
        error,
        PreparationError::Schema(
            "Attribute does not belong to the receiver's effective features".into()
        )
    );
    assert_eq!(budget.used.get(), 0);
    assert_eq!(budget.checkpoints.get(), 0);
}

#[test]
fn runtime_attribute_alternative_precharges_literal_copies_and_rejects_unknown_before_copy() {
    let mut schema = schema();
    let field = feature();
    // Existing Feature 3265, then target EInt costs 64 + 4 bytes.
    for limit in [0, 3264, 3265, 3266, 3332, 3333, 3334] {
        let budget = probe(limit, usize::MAX);
        let observation = InvocationContext::new(budget.clone());
        match schema.domain_alternative_observed(&field, "EInt", &observation) {
            Err(error) => {
                assert!(limit < 3333);
                assert_eq!(
                    error,
                    PreparationError::Observation("schema storage denied".into())
                );
                assert_eq!(budget.used.get(), if limit < 3265 { 0 } else { 3265 });
            }
            Ok(alternative) => {
                assert!(limit >= 3333);
                assert_eq!(budget.used.get(), 3333);
                assert_eq!(alternative.target, "EInt");
                assert_eq!(alternative.datatype, None);
                assert_eq!(alternative.name, "x");
                assert_eq!(alternative.default, serde_json::Value::Null);
            }
        }
    }
    let budget = probe(0, 0);
    let observation = InvocationContext::new(budget.clone());
    assert_eq!(
        schema
            .domain_alternative_observed(&field, "unknown", &observation)
            .unwrap_err(),
        PreparationError::Schema("Unknown runtime attribute domain codec".into())
    );
    assert_eq!(budget.used.get(), 0);
    assert_eq!(budget.checkpoints.get(), 0);
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
    // Library policy fixture: the 20-byte URI/[0] declaration costs 1425
    // (10 nodes + 145 metadata bytes). This is not source import qualification.
    // Original Feature 3265 + replacement target N 65 + declaration 1425 = 4755.
    for limit in [0, 3264, 3265, 3329, 3330, 4754, 4755, 4756] {
        let budget = probe(limit, usize::MAX);
        let observation = InvocationContext::new(budget.clone());
        match schema.domain_alternative_observed(&field, "N", &observation) {
            Err(error) => {
                assert!(limit < 4755);
                assert_eq!(
                    error,
                    PreparationError::Observation("schema storage denied".into())
                );
                assert_eq!(
                    budget.used.get(),
                    if limit < 3265 {
                        0
                    } else if limit < 3330 {
                        3265
                    } else {
                        3330
                    }
                );
            }
            Ok(alternative) => {
                assert!(limit >= 4755);
                assert_eq!(budget.used.get(), 4755);
                assert_eq!(alternative.target, "N");
                assert_eq!(alternative.datatype, Some(declaration.clone()));
                assert_eq!(alternative.name, "x");
            }
        }
    }
}

#[test]
fn attribute_codec_precharges_literal_feature_and_poisoned_observation() {
    use super::super::AttributeConversionPreparation;
    let schema = schema();
    let field = feature();
    // The same fixed Feature has 23 visited nodes and 321 metadata bytes:
    // 23 * 128 + 321 = 3265, without a receiver copy in this preparation.
    for limit in [0, 3264, 3265, 3266] {
        let budget = probe(limit, usize::MAX);
        let observation = InvocationContext::new(budget.clone());
        match AttributeConversionPreparation::new_observed(&field, &observation) {
            Err(error) => {
                assert!(limit < 3265);
                assert_eq!(
                    error,
                    PreparationError::Observation("schema storage denied".into())
                );
                assert_eq!(budget.used.get(), 0);
            }
            Ok(mut pending) => {
                assert!(limit >= 3265);
                assert!(
                    pending
                        .advance_observed(&schema, None, &observation)
                        .unwrap()
                );
                pending.finish().unwrap();
                assert_eq!(budget.used.get(), 3265);
            }
        }
    }
    let baseline = probe(usize::MAX, usize::MAX);
    let observation = InvocationContext::new(baseline.clone());
    let mut pending = AttributeConversionPreparation::new_observed(&field, &observation).unwrap();
    assert!(
        pending
            .advance_observed(&schema, None, &observation)
            .unwrap()
    );
    pending.finish().unwrap();
    for stop in 0..baseline.checkpoints.get() {
        let budget = probe(usize::MAX, stop);
        let observation = InvocationContext::new(budget);
        match AttributeConversionPreparation::new_observed(&field, &observation) {
            Err(error) => assert_eq!(
                error,
                PreparationError::Observation("schema cancelled".into())
            ),
            Ok(mut pending) => {
                assert_eq!(
                    pending
                        .advance_observed(&schema, None, &observation)
                        .unwrap_err(),
                    PreparationError::Observation("schema cancelled".into())
                );
                assert!(pending.advance(&schema, None).is_err());
                assert!(pending.finish().is_err());
            }
        }
        let mut fresh = AttributeConversionPreparation::new(&field);
        assert!(fresh.advance(&schema, None).unwrap());
        fresh.finish().unwrap();
    }
}
impl InvocationObserver for Probe {
    fn before_invoke(&self, _: &ProviderId) -> Result<(), String> {
        panic!("schema preparation invokes no provider")
    }
    fn checkpoint(&self) -> Result<(), String> {
        if self.checkpoints.get() == self.stop {
            return Err("schema cancelled".into());
        }
        self.checkpoints.set(self.checkpoints.get() + 1);
        Ok(())
    }
    fn retain_storage(&self, bytes: usize) -> Result<(), String> {
        let next = self
            .used
            .get()
            .checked_add(bytes)
            .filter(|n| *n <= self.limit)
            .ok_or("schema storage denied")?;
        self.used.set(next);
        Ok(())
    }
}
fn probe(limit: usize, stop: usize) -> Rc<Probe> {
    Rc::new(Probe {
        used: Cell::new(0),
        limit,
        checkpoints: Cell::new(0),
        stop,
    })
}

#[test]
fn ancestry_reserves_literal_chain_and_diamond_storage_before_retention() {
    let schema = schema();
    // Chain B->A: 97/97/97/32/129/129/97/32/129/97/97.
    // Each new task slot costs 32, each copied string/tree slot 64+len.
    let prefixes = [0, 97, 194, 291, 323, 452, 581, 678, 710, 839, 936, 1033];
    for limit in [
        0, 96, 97, 193, 194, 290, 291, 322, 323, 451, 452, 580, 581, 677, 678, 709, 710, 838, 839,
        935, 936, 1032, 1033, 1034,
    ] {
        let probe = probe(limit, usize::MAX);
        let observation = InvocationContext::new(probe.clone());
        match AncestryPreparation::new_observed("B", &observation) {
            Err(error) => assert_eq!(
                error,
                PreparationError::Observation("schema storage denied".into())
            ),
            Ok(mut pending) => loop {
                match pending.advance_observed(&schema, &observation) {
                    Ok(false) => {}
                    Ok(true) => {
                        assert!(limit >= 1033);
                        assert_eq!(pending.finish().unwrap(), ["A", "B"]);
                        break;
                    }
                    Err(error) => {
                        assert!(limit < 1033);
                        assert_eq!(
                            error,
                            PreparationError::Observation("schema storage denied".into())
                        );
                        assert!(pending.advance(&schema).is_err());
                        assert!(pending.finish().is_err());
                        break;
                    }
                }
            },
        }
        assert_eq!(
            probe.used.get(),
            prefixes
                .into_iter()
                .filter(|bytes| *bytes <= limit)
                .max()
                .unwrap()
        );
    }
    let probe = probe(2453, usize::MAX);
    let observation = InvocationContext::new(probe.clone());
    let mut diamond = AncestryPreparation::new_observed("D", &observation).unwrap();
    while !diamond.advance_observed(&schema, &observation).unwrap() {}
    assert_eq!(diamond.finish().unwrap(), ["A", "B", "C", "D"]);
    assert_eq!(probe.used.get(), 2453);
}

#[test]
fn conformance_observation_failures_are_distinct_and_every_checkpoint_is_terminal() {
    let schema = schema();
    // Two retained class keys 130 + B->A ancestry 1033 = 1163.
    for limit in [0, 129, 130, 1162, 1163, 1164] {
        let probe = probe(limit, usize::MAX);
        let observation = InvocationContext::new(probe.clone());
        if let Ok(mut pending) = ConformancePreparation::new_observed("B", "A", &observation) {
            loop {
                match pending.advance_observed(&schema, &observation) {
                    Ok(false) => {}
                    Ok(true) => {
                        assert!(limit >= 1163);
                        assert!(pending.finish().unwrap());
                        break;
                    }
                    Err(error) => {
                        assert!(limit < 1163);
                        assert_eq!(
                            error,
                            PreparationError::Observation("schema storage denied".into())
                        );
                        assert!(pending.finish().is_err());
                        break;
                    }
                }
            }
        } else {
            assert!(limit < 130);
        }
        assert!(probe.used.get() <= limit);
    }
    let baseline = probe(usize::MAX, usize::MAX);
    let observation = InvocationContext::new(baseline.clone());
    let mut pending = ConformancePreparation::new_observed("D", "A", &observation).unwrap();
    while !pending.advance_observed(&schema, &observation).unwrap() {}
    assert!(pending.finish().unwrap());
    for stop in 0..baseline.checkpoints.get() {
        let probe = probe(usize::MAX, stop);
        let observation = InvocationContext::new(probe);
        match ConformancePreparation::new_observed("D", "A", &observation) {
            Err(error) => assert_eq!(
                error,
                PreparationError::Observation("schema cancelled".into())
            ),
            Ok(mut pending) => loop {
                match pending.advance_observed(&schema, &observation) {
                    Ok(false) => {}
                    Ok(true) => panic!("missed cancellation"),
                    Err(error) => {
                        assert_eq!(
                            error,
                            PreparationError::Observation("schema cancelled".into())
                        );
                        assert!(pending.advance(&schema).is_err());
                        assert!(pending.finish().is_err());
                        break;
                    }
                }
            },
        }
    }
    for (actual, expected, answer) in [("B", "A", true), ("A", "B", false)] {
        let mut fresh = ConformancePreparation::new(actual, expected);
        while !fresh.advance(&schema).unwrap() {}
        assert_eq!(fresh.finish().unwrap(), answer);
    }
}

#[test]
fn ancestry_schema_errors_preserve_their_category() {
    let mut schema = schema();
    schema.classes.get_mut("A").unwrap().parents = vec!["B".into()];
    let mut cycle = ConformancePreparation::new("B", "A");
    loop {
        match cycle.advance_observed(&schema, &InvocationContext::default()) {
            Ok(false) => {}
            Ok(true) => panic!("cycle accepted"),
            Err(error) => {
                assert_eq!(
                    error,
                    PreparationError::Schema("Cyclic inheritance at B".into())
                );
                break;
            }
        }
    }
    assert!(cycle.finish().is_err());
    let mut duplicate_schema = schema.clone();
    duplicate_schema
        .classes
        .get_mut("B")
        .unwrap()
        .parents
        .push("A".into());
    let budget = probe(usize::MAX, usize::MAX);
    let observation = InvocationContext::new(budget.clone());
    let mut duplicate = AncestryPreparation::new_observed("B", &observation).unwrap();
    loop {
        match duplicate.advance_observed(&duplicate_schema, &observation) {
            Ok(false) => {}
            Ok(true) => panic!("duplicate parent accepted"),
            Err(error) => {
                assert_eq!(
                    error,
                    PreparationError::Schema("Duplicate supertype A of B".into())
                );
                assert_eq!(budget.used.get(), 291);
                assert!(duplicate.finish().is_err());
                break;
            }
        }
    }
    let mut unknown = ConformancePreparation::new("Missing", "A");
    loop {
        match unknown.advance_observed(&schema, &InvocationContext::default()) {
            Ok(false) => {}
            Ok(true) => panic!("unknown accepted"),
            Err(error) => {
                assert_eq!(
                    error,
                    PreparationError::Schema("Unknown class Missing".into())
                );
                break;
            }
        }
    }
}

#[test]
fn validated_class_key_lookup_borrows_keys_retains_query_and_cancels_every_checkpoint() {
    let (closed_schema, closed) = class_key_fixture();
    let raw_schema = raw_validation_schema();
    let (raw_key, raw) = raw_schema.instantiations.iter().next().unwrap();
    for (schema, target, expected, needed) in [
        (&closed_schema, &closed, "A", 2524usize),
        (&raw_schema, raw, raw_key.as_str(), 2536usize),
    ] {
        let before = serde_json::to_value(schema).unwrap();
        let prepared =
            ValidatedRuntimeTypes::prepare_observed(schema, &InvocationContext::default()).unwrap();
        let budget = probe(needed, usize::MAX);
        let context = InvocationContext::new(budget.clone());
        let selected = prepared
            .class_key_for_type_observed(target, &context)
            .unwrap();
        assert_eq!(selected, expected);
        assert_eq!(selected, schema.class_key_for_type(target).unwrap());
        let stored = schema.classes.get_key_value(selected).unwrap().0.as_str();
        assert!(std::ptr::eq(selected, stored));
        // Eighteen nodes and 220 metadata bytes need 2524 for the closed
        // input; the migrated raw fixture URI adds twelve UTF-8 bytes. Boolean scalar
        // policy is fixed-width. Cursor/result keys add no retained bytes.
        assert_eq!(budget.used.get(), needed);
        assert_eq!(
            prepared
                .class_key_for_type_observed(
                    target,
                    &InvocationContext::new(probe(needed - 1, usize::MAX))
                )
                .unwrap_err(),
            PreparationError::Observation("schema storage denied".into())
        );
        assert!(budget.checkpoints.get() > 0);
        for stop in 0..budget.checkpoints.get() {
            let stopped = InvocationContext::new(probe(needed, stop));
            assert_eq!(
                prepared
                    .class_key_for_type_observed(target, &stopped)
                    .unwrap_err(),
                PreparationError::Observation("schema cancelled".into())
            );
            assert_eq!(
                prepared
                    .class_key_for_type_observed(
                        target,
                        &InvocationContext::new(probe(needed, usize::MAX))
                    )
                    .unwrap(),
                expected
            );
        }
        assert_eq!(serde_json::to_value(schema).unwrap(), before);
    }
}

#[test]
fn validated_borrowed_class_keys_keep_identity_ambiguity_and_fresh_mutation_errors() {
    use crate::definitions::generic_types::Type;
    let (schema, target) = class_key_fixture();
    let prepared =
        ValidatedRuntimeTypes::prepare_observed(&schema, &InvocationContext::default()).unwrap();
    let mut foreign = target.expression().clone();
    let Type::Classifier { declaration, .. } = &mut foreign else {
        panic!("classifier");
    };
    declaration.source.sha256 = "0".repeat(64);
    struct Input(Type);
    impl AsRef<Type> for Input {
        fn as_ref(&self) -> &Type {
            &self.0
        }
    }
    let parameter = Type::Parameter {
        declaration: declaration.clone(),
    };
    for expression in [foreign, parameter] {
        let expression = Input(expression);
        let borrowed = prepared.class_key_for_type_observed(
            &expression,
            &InvocationContext::new(probe(usize::MAX, usize::MAX)),
        );
        let fresh = schema.class_key_for_type_observed(&expression, &InvocationContext::default());
        assert_eq!(borrowed, fresh);
        assert!(borrowed.is_err());
    }
    let mut ambiguous = schema.clone();
    let mut class = ambiguous.classes["A"].clone();
    class.name = "ZZ".into();
    ambiguous.classes.insert("ZZ".into(), class);
    ambiguous
        .declarations
        .insert("ZZ".into(), ambiguous.declarations["A"].clone());
    let prepared =
        ValidatedRuntimeTypes::prepare_observed(&ambiguous, &InvocationContext::default()).unwrap();
    assert_eq!(
        prepared
            .class_key_for_type_observed(
                &target,
                &InvocationContext::new(probe(usize::MAX, usize::MAX))
            )
            .unwrap_err(),
        PreparationError::Schema("Ambiguous closed class declaration".into())
    );
    let mut altered = raw_validation_schema();
    let raw = altered.instantiations.values().next().unwrap().clone();
    altered.runtime_feature_domains.values_mut().next().unwrap()[0]
        .alternatives
        .push("foreign-class".into());
    let fresh_error = altered
        .class_key_for_type_observed(&raw, &InvocationContext::default())
        .unwrap_err();
    match ValidatedRuntimeTypes::prepare_observed(&altered, &InvocationContext::default()) {
        Ok(_) => panic!("caller-mutated schema granted validation authority"),
        Err(error) => assert_eq!(error, fresh_error),
    }
}

#[test]
fn declared_reference_views_read_complete_values_and_preserve_public_ownership() {
    let mut schema = legacy_feature_schema();
    schema
        .classes
        .get_mut("A")
        .unwrap()
        .features
        .get_mut("x")
        .unwrap()
        .default = serde_json::json!({"méta": ["café", {"nested": true}]});
    let field = &schema.classes["A"].features["x"];
    let observation = InvocationContext::default();
    let borrowed =
        ReferenceStructurePreparation::declared_observed("A", "x", &observation).unwrap();
    assert!(matches!(
        borrowed.feature,
        ReferenceFeatureInput::Declared(_)
    ));
    let resolved = borrowed.feature.resolve(&schema, "A", None).unwrap();
    assert!(std::ptr::eq(resolved, field));
    assert_eq!(
        serde_json::to_value(resolved).unwrap(),
        serde_json::to_value(field).unwrap()
    );
    let owned = ReferenceStructurePreparation::new("A", field);
    assert!(matches!(owned.feature, ReferenceFeatureInput::Owned(_)));
    assert!(!std::ptr::eq(
        owned.feature.resolve(&schema, "A", None).unwrap(),
        field
    ));
    assert_eq!(owned.feature.resolve(&schema, "A", None).unwrap(), field);
    let saved = serde_json::to_value(&schema).unwrap();
    for mut preparation in [borrowed, owned] {
        while !preparation.advance(&schema).unwrap() {}
        preparation.finish().unwrap();
    }
    assert_eq!(serde_json::to_value(&schema).unwrap(), saved);
}

#[test]
fn declared_reference_views_precharge_keys_cancel_and_match_fresh_errors() {
    let (mut schema, field) = reference_fixture();
    let class = schema.classes.get_mut("A").unwrap();
    class.features.insert(field.name.clone(), field.clone());
    class.feature_order.push(field.name.clone());
    let run = |schema: &Schema, budget: Rc<Probe>, borrowed: bool| {
        let observation = InvocationContext::new(budget);
        let mut preparation = if borrowed {
            ReferenceStructurePreparation::declared_observed("A", "x", &observation)?
        } else {
            ReferenceStructurePreparation::new_observed(
                "A",
                &schema.classes["A"].features["x"],
                &observation,
            )?
        };
        while !preparation.advance_observed(schema, &observation)? {}
        preparation.finish().map_err(PreparationError::Schema)
    };
    let borrowed = probe(usize::MAX, usize::MAX);
    let owned = probe(usize::MAX, usize::MAX);
    assert_eq!(
        run(&schema, borrowed.clone(), true),
        run(&schema, owned.clone(), false)
    );
    assert!(borrowed.used.get() < owned.used.get());
    let needed = borrowed.used.get();
    assert_eq!(run(&schema, probe(needed, usize::MAX), true), Ok(()));
    assert_eq!(
        run(&schema, probe(needed - 1, usize::MAX), true),
        Err(PreparationError::Observation(
            "schema storage denied".into()
        ))
    );
    // Two one-character keys each admit 64 + 1, before retention.
    for limit in [0, 129] {
        let budget = probe(limit, usize::MAX);
        let observation = InvocationContext::new(budget);
        assert!(matches!(
            ReferenceStructurePreparation::declared_observed("A", "x", &observation),
            Err(PreparationError::Observation(_))
        ));
    }
    for stop in 0..borrowed.checkpoints.get() {
        assert_eq!(
            run(&schema, probe(usize::MAX, stop), true),
            Err(PreparationError::Observation("schema cancelled".into()))
        );
        assert_eq!(run(&schema, probe(usize::MAX, usize::MAX), true), Ok(()));
    }
    let mut changed = schema.clone();
    changed
        .classes
        .get_mut("A")
        .unwrap()
        .features
        .get_mut("x")
        .unwrap()
        .unique = false;
    let fresh = run(&changed, probe(usize::MAX, usize::MAX), false);
    assert!(matches!(fresh, Err(PreparationError::Schema(_))));
    assert_eq!(run(&changed, probe(usize::MAX, usize::MAX), true), fresh);
    changed.classes.get_mut("A").unwrap().features.remove("x");
    let mut missing =
        ReferenceStructurePreparation::declared_observed("A", "x", &InvocationContext::default())
            .unwrap();
    assert_eq!(
        missing.advance_observed(&changed, &InvocationContext::default()),
        Err(PreparationError::Schema(
            "Missing declared reference feature".into()
        ))
    );
    assert!(missing.advance(&schema).is_err());
    assert!(missing.finish().is_err());
}

fn shared_map_schema() -> Schema {
    use crate::definitions::{
        bundle::{DefinitionBundle, DefinitionSource, SourceKind},
        generic_types::{RuntimeType, Type},
    };
    let source = DefinitionSource::new(
        "memory:feature-map-sharing.ecore",
        SourceKind::Ecore,
        r##"<e:EPackage xmlns:e="http://www.eclipse.org/emf/2002/Ecore" xmlns:x="http://www.w3.org/2001/XMLSchema-instance" name="m" nsURI="urn:shared-maps"><eClassifiers x:type="e:EClass" name="Root"><eTypeParameters name="T"><eBounds eClassifier="#//Root"/></eTypeParameters><eStructuralFeatures x:type="e:EReference" name="a"><eGenericType eTypeParameter="#//Root/T"/></eStructuralFeatures><eStructuralFeatures x:type="e:EReference" name="b"><eGenericType eTypeParameter="#//Root/T"/></eStructuralFeatures><eStructuralFeatures x:type="e:EAttribute" name="label" eType="e:EDataType http://www.eclipse.org/emf/2002/Ecore#//EString" defaultValueLiteral="café"/></eClassifiers><eClassifiers x:type="e:EClass" name="Child" eSuperTypes="#//Root"/><eClassifiers x:type="e:EClass" name="Other" eSuperTypes="#//Root"/></e:EPackage>"##,
    );
    let (source, ids) = crate::definitions::migration_test_fixture::source_with_ids(source);
    let requested = (0..3)
        .map(|index| {
            RuntimeType::new(Type::Classifier {
                declaration: ids[&vec![index]].clone(),
                arguments: vec![],
                raw: index == 0,
            })
            .unwrap()
        })
        .collect::<Vec<_>>();
    crate::definitions::compiler::materialize_runtime_types(
        &DefinitionBundle {
            version: 1,
            sources: vec![source],
            namespaces: vec![],
        },
        &requested,
        None,
    )
    .unwrap()
}

fn run_shared_map_validation(
    schema: &Schema,
    observation: &InvocationContext<'_>,
    shared: bool,
) -> Result<(), PreparationError> {
    let mut state = if shared {
        super::super::InstantiationValidation::with_reference_views()
    } else {
        super::super::InstantiationValidation::new()
    };
    while !state.advance_observed(schema, observation)? {}
    assert!(state.shared_fields.is_empty());
    Ok(())
}

#[test]
fn immutable_validation_shares_complete_feature_maps_and_rechecks_domains() {
    use super::super::{InstantiationValidation, InstantiationValidationStage, ValidationFeatures};
    let schema = shared_map_schema();
    let saved = serde_json::to_value(&schema).unwrap();
    let mut state = InstantiationValidation::with_reference_views();
    let observation = InvocationContext::default();
    let mut reused = false;
    let mut published = false;
    loop {
        if let Some(ValidationFeatures::Shared(fields)) = &state.fields {
            let receiver = state.outer.as_ref().unwrap();
            assert!(std::sync::Arc::ptr_eq(
                fields,
                &state.shared_fields[receiver]
            ));
            let mut fresh = FeatureMapPreparation::new(receiver);
            while !fresh.advance(&schema).unwrap() {}
            let fresh = fresh.finish().unwrap();
            assert_eq!(fields.receiver(), fresh.receiver());
            assert_eq!(fields.fields_for_test(), fresh.fields);
            assert_eq!(&fields.owners_for_test(), &fresh.owners);
            assert_eq!(&fields.member_identities(), &fresh.identities);
            assert_eq!(fields.order(), fresh.order);
            assert_eq!(
                serde_json::to_value(fields.fields_for_test()).unwrap(),
                serde_json::to_value(fresh.fields()).unwrap()
            );
            published = true;
            reused |= matches!(state.stage, InstantiationValidationStage::RawNext);
        }
        let cleanup = matches!(state.stage, InstantiationValidationStage::IdentityClear);
        let remaining = |state: &InstantiationValidation| {
            state
                .shared_fields
                .values()
                .map(|fields| fields.bookkeeping_len())
                .sum::<usize>()
        };
        let before = remaining(&state);
        let done = state.advance_observed(&schema, &observation).unwrap();
        if cleanup {
            assert!(before - remaining(&state) <= 1);
        }
        if done {
            break;
        }
    }
    assert!(published && reused);
    assert!(state.shared_fields.is_empty());
    assert_eq!(serde_json::to_value(&schema).unwrap(), saved);
    assert_eq!(
        run_shared_map_validation(&schema, &observation, false),
        Ok(())
    );
    let mut changed = schema.clone();
    let domain = changed
        .runtime_feature_domains
        .values_mut()
        .flat_map(|rows| rows.iter_mut())
        .find(|row| row.source.reference)
        .unwrap();
    domain
        .alternatives
        .push(domain.alternatives.first().unwrap().clone());
    let fresh = run_shared_map_validation(&changed, &observation, false);
    assert!(matches!(fresh, Err(PreparationError::Schema(_))));
    assert_eq!(
        run_shared_map_validation(&changed, &observation, true),
        fresh
    );
}

#[test]
fn immutable_feature_map_validation_observes_limits_cancellation_and_recovery() {
    let schema = shared_map_schema();
    let shared = probe(usize::MAX, usize::MAX);
    let owned = probe(usize::MAX, usize::MAX);
    assert_eq!(
        run_shared_map_validation(&schema, &InvocationContext::new(shared.clone()), true),
        Ok(())
    );
    assert_eq!(
        run_shared_map_validation(&schema, &InvocationContext::new(owned.clone()), false),
        Ok(())
    );
    assert!(shared.used.get() < owned.used.get());
    assert_eq!(
        run_shared_map_validation(
            &schema,
            &InvocationContext::new(probe(shared.used.get(), usize::MAX)),
            true
        ),
        Ok(())
    );
    assert_eq!(
        run_shared_map_validation(
            &schema,
            &InvocationContext::new(probe(shared.used.get() - 1, usize::MAX)),
            true
        ),
        Err(PreparationError::Observation(
            "schema storage denied".into()
        ))
    );
    for stop in 0..shared.checkpoints.get() {
        assert_eq!(
            run_shared_map_validation(
                &schema,
                &InvocationContext::new(probe(usize::MAX, stop)),
                true
            ),
            Err(PreparationError::Observation("schema cancelled".into()))
        );
    }
    assert_eq!(
        run_shared_map_validation(
            &schema,
            &InvocationContext::new(probe(usize::MAX, usize::MAX)),
            true
        ),
        Ok(())
    );
}

#[test]
fn immutable_feature_map_growth_uses_complete_owned_fallback() {
    use super::super::{InstantiationValidation, ValidationFeatures};
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<InstantiationValidation>();
    let schema = shared_map_schema();
    let mut map = FeatureMapPreparation::new(schema.classes.keys().next().unwrap());
    while !map.advance(&schema).unwrap() {}
    let metadata = std::sync::Arc::new(PreparedFeatureInputs::Owned(map.finish().unwrap()));
    let mut state = InstantiationValidation::with_reference_views();
    // Policy-boundary fixture, not qualification of 4096 modeled receivers.
    for index in 0..4096 {
        state
            .shared_fields
            .insert(format!("test-only-unused-key-{index}"), metadata.clone());
    }
    let mut owned = false;
    loop {
        if matches!(
            state.stage,
            super::super::InstantiationValidationStage::RuntimeNext
                | super::super::InstantiationValidationStage::RawNext
        ) && let Some(ValidationFeatures::Owned(fields)) = &state.fields
        {
            assert_eq!(
                fields.fields_for_test(),
                schema.features(fields.receiver()).unwrap()
            );
            owned = true;
        }
        if state.advance(&schema).unwrap() {
            break;
        }
    }
    assert!(owned);
    assert!(state.shared_fields.is_empty());
    assert_eq!(
        run_shared_map_validation(&schema, &InvocationContext::default(), false),
        Ok(())
    );
}

#[test]
fn private_feature_identity_indices_preserve_complete_selection_and_fresh_errors() {
    let schema = shared_map_schema();
    let saved = serde_json::to_value(&schema).unwrap();
    let run = |schema: &Schema, receiver: &str, view: bool, meter: Rc<Probe>| {
        let observation = InvocationContext::new(meter);
        let mut pending = EffectiveFeaturePreparation::new_observed(receiver, &observation)?;
        pending.identity_views = view;
        while !pending.advance_observed(schema, &observation)? {}
        Ok::<_, PreparationError>(
            pending
                .finish()?
                .into_iter()
                .map(|field| (field.owner, field.name, field.identity, field.position))
                .collect::<Vec<_>>(),
        )
    };
    for receiver in schema.classes.keys() {
        let meter = probe(usize::MAX, usize::MAX);
        let view = run(&schema, receiver, true, meter.clone()).unwrap();
        let owned_meter = probe(usize::MAX, usize::MAX);
        let owned = run(&schema, receiver, false, owned_meter.clone()).unwrap();
        assert_eq!(view, owned);
        assert_eq!(
            serde_json::to_value(&view).unwrap(),
            serde_json::to_value(&owned).unwrap()
        );
        assert!(meter.used.get() < owned_meter.used.get());
        let bytes = meter.used.get();
        assert_eq!(
            run(&schema, receiver, true, probe(bytes, usize::MAX)).unwrap(),
            owned
        );
        assert_eq!(
            run(&schema, receiver, true, probe(bytes - 1, usize::MAX)),
            Err(PreparationError::Observation(
                "schema storage denied".into()
            ))
        );
        for stop in 0..meter.checkpoints.get() {
            assert_eq!(
                run(&schema, receiver, true, probe(usize::MAX, stop)),
                Err(PreparationError::Observation("schema cancelled".into()))
            );
        }
        assert_eq!(
            run(&schema, receiver, true, probe(bytes, usize::MAX)).unwrap(),
            owned
        );
    }
    let receiver = schema
        .classes
        .iter()
        .find(|(_, class)| class.feature_order.len() >= 2)
        .unwrap()
        .0
        .clone();
    let mut repeated = schema.clone();
    let class = repeated.classes.get_mut(&receiver).unwrap();
    let names = class.feature_order.clone();
    assert!(names.len() >= 2);
    let declaration = class.feature_declarations[&names[0]].clone();
    class
        .feature_declarations
        .insert(names[1].clone(), declaration);
    let owned = run(&repeated, &receiver, false, probe(usize::MAX, usize::MAX));
    assert!(matches!(owned, Err(PreparationError::Schema(_))));
    assert_eq!(
        run(&repeated, &receiver, true, probe(usize::MAX, usize::MAX)),
        owned
    );
    let mut reordered = schema.clone();
    reordered
        .classes
        .get_mut(&receiver)
        .unwrap()
        .feature_order
        .reverse();
    let owned = run(&reordered, &receiver, false, probe(usize::MAX, usize::MAX));
    assert!(matches!(owned, Err(PreparationError::Schema(_))));
    assert_eq!(
        run(&reordered, &receiver, true, probe(usize::MAX, usize::MAX)),
        owned
    );
    assert_eq!(serde_json::to_value(&schema).unwrap(), saved);
}

#[test]
fn private_domain_keys_resolve_complete_features_through_unchanged_domain_checks() {
    use super::super::{InstantiationValidation, ValidationDomainFeature};
    let schema = shared_map_schema();
    let saved = serde_json::to_value(&schema).unwrap();
    let observation = InvocationContext::default();
    let mut state = InstantiationValidation::with_reference_views();
    let mut checked = 0usize;
    loop {
        if let Some(ValidationDomainFeature::Prepared(name)) = &state.domain_field {
            let receiver = state.outer.as_ref().unwrap();
            let resolved = state
                .domain_field
                .as_ref()
                .unwrap()
                .resolve(state.fields.as_ref())
                .unwrap();
            let fresh = schema.features(receiver).unwrap();
            assert_eq!(resolved, &fresh[name]);
            assert_eq!(
                serde_json::to_value(resolved).unwrap(),
                serde_json::to_value(&fresh[name]).unwrap()
            );
            checked += 1;
        }
        if state.advance_observed(&schema, &observation).unwrap() {
            break;
        }
    }
    assert!(checked > 0);
    let mut changed = schema.clone();
    let receiver = changed
        .runtime_feature_domains
        .keys()
        .next()
        .unwrap()
        .clone();
    let name = changed.runtime_feature_domains[&receiver][0]
        .source
        .projection
        .source
        .name
        .clone();
    let declaring_owner = changed.feature_owner(&receiver, &name).unwrap().to_owned();
    changed
        .classes
        .get_mut(&declaring_owner)
        .unwrap()
        .features
        .get_mut(&name)
        .unwrap()
        .reference = false;
    let owned = run_shared_map_validation(&changed, &observation, false);
    assert!(matches!(owned, Err(PreparationError::Schema(_))));
    assert_eq!(
        run_shared_map_validation(&changed, &observation, true),
        owned
    );
    assert_eq!(serde_json::to_value(&schema).unwrap(), saved);
}

#[test]
fn prepared_reference_map_inputs_preserve_all_fields_limits_and_recovery() {
    let schema = shared_map_schema();
    let saved = serde_json::to_value(&schema).unwrap();
    let receiver = schema
        .classes
        .keys()
        .find(|receiver| schema.features(receiver).unwrap().len() >= 2)
        .unwrap();
    let mut map = FeatureMapPreparation::with_identity_views_observed(
        receiver,
        &InvocationContext::default(),
    )
    .unwrap();
    while !map.advance(&schema).unwrap() {}
    let fields = map.finish().unwrap();
    for (name, feature) in fields.fields() {
        let run = |view: bool, meter: Rc<Probe>| {
            let context = InvocationContext::new(meter);
            let mut state = if view {
                ReferenceStructurePreparation::prepared_observed(receiver, name, &context)?
            } else {
                ReferenceStructurePreparation::new_observed(receiver, feature, &context)?
            };
            loop {
                let done = if view {
                    state.advance_prepared_observed(&schema, &fields, &context)?
                } else {
                    state.advance_observed(&schema, &context)?
                };
                if done {
                    return state.finish().map_err(PreparationError::Schema);
                }
            }
        };
        let meter = probe(usize::MAX, usize::MAX);
        assert_eq!(
            run(true, meter.clone()),
            run(false, probe(usize::MAX, usize::MAX))
        );
        let bytes = meter.used.get();
        assert_eq!(run(true, probe(bytes, usize::MAX)), Ok(()));
        assert!(run(true, probe(bytes - 1, usize::MAX)).is_err());
        for stop in 0..meter.checkpoints.get() {
            assert_eq!(
                run(true, probe(usize::MAX, stop)),
                Err(PreparationError::Observation("schema cancelled".into()))
            );
        }
        assert_eq!(run(true, probe(bytes, usize::MAX)), Ok(()));
        let mut state = ReferenceStructurePreparation::prepared_observed(
            receiver,
            name,
            &InvocationContext::default(),
        )
        .unwrap();
        assert_eq!(
            state
                .feature
                .resolve(&schema, receiver, Some(&fields))
                .unwrap(),
            feature
        );
        assert_eq!(
            serde_json::to_value(
                state
                    .feature
                    .resolve(&schema, receiver, Some(&fields))
                    .unwrap()
            )
            .unwrap(),
            serde_json::to_value(feature).unwrap()
        );
        assert!(
            state
                .advance_observed(&schema, &InvocationContext::default())
                .is_err()
        );
        assert!(state.finish().is_err());
    }
    assert_eq!(serde_json::to_value(&schema).unwrap(), saved);
}

fn prepare_source_input_map(
    schema: &Schema,
    receiver: &str,
    cache: SharedFeatureInputCache,
    observation: &InvocationContext<'_>,
) -> Result<PreparedFeatureInputs, PreparationError> {
    let mut pending =
        FeatureMapPreparation::with_source_inputs_observed(receiver, cache, observation)?;
    while !pending.advance_observed(schema, observation)? {}
    pending.finish_inputs().map_err(PreparationError::Schema)
}
#[test]
fn declared_feature_inputs_share_complete_metadata_with_fresh_default_selection() {
    let schema = shared_map_schema();
    let before = serde_json::to_value(&schema).unwrap();
    let cache = std::sync::Arc::new(std::sync::Mutex::new(FeatureInputCache::default()));
    let observation = InvocationContext::default();
    let mut values = Vec::new();
    let mut reused = false;
    for receiver in schema.classes.keys() {
        let input =
            prepare_source_input_map(&schema, receiver, cache.clone(), &observation).unwrap();
        let mut fresh = FeatureMapPreparation::new(receiver);
        while !fresh.advance(&schema).unwrap() {}
        let fresh = fresh.finish().unwrap();
        assert_eq!(input.fields_for_test(), fresh.fields);
        assert_eq!(&input.owners_for_test(), &fresh.owners);
        assert_eq!(&input.member_identities(), fresh.member_identities());
        assert_eq!(input.order(), fresh.order());
        assert_eq!(
            serde_json::to_value(input.fields_for_test()).unwrap(),
            serde_json::to_value(fresh.fields()).unwrap()
        );
        let PreparedFeatureInputs::Shared(input) = input else {
            panic!("source input map was owned");
        };
        for (name, input) in input.fields {
            let FeatureInput::Shared(input) = input.field else {
                panic!("unchanged source feature was copied");
            };
            let identity = &input.name;
            assert_eq!(identity.as_str(), name.as_ref());
            for (other_name, other) in &values {
                if other_name == &name && std::sync::Arc::ptr_eq(&input, other) {
                    reused = true;
                }
            }
            values.push((name, input));
        }
    }
    assert!(reused);
    assert_eq!(serde_json::to_value(&schema).unwrap(), before);
}
#[test]
fn source_input_cache_cold_admission_hits_limits_cancellation_and_owned_fallback() {
    struct Meter {
        bytes: Cell<usize>,
        calls: Cell<usize>,
        limit: usize,
        stop: usize,
    }
    impl Meter {
        fn step(&self) -> Result<(), String> {
            let n = self.calls.get();
            self.calls.set(n + 1);
            if n == self.stop {
                Err("source input cancelled".into())
            } else {
                Ok(())
            }
        }
    }
    impl InvocationObserver for Meter {
        fn before_invoke(&self, _: &ProviderId) -> Result<(), String> {
            Err("no providers".into())
        }
        fn work(&self, _: usize) -> Result<(), String> {
            self.step()
        }
        fn retain_storage(&self, bytes: usize) -> Result<(), String> {
            self.step()?;
            let n = self
                .bytes
                .get()
                .checked_add(bytes)
                .filter(|n| *n <= self.limit)
                .ok_or("input storage denied")?;
            self.bytes.set(n);
            Ok(())
        }
    }
    let schema = shared_map_schema();
    let receiver = schema.classes.keys().next().unwrap();
    let selected = schema.effective_features(receiver).unwrap().remove(0);
    let run = |limit, stop| {
        let cache = std::sync::Arc::new(std::sync::Mutex::new(FeatureInputCache::default()));
        let meter = Rc::new(Meter {
            bytes: Cell::new(0),
            calls: Cell::new(0),
            limit,
            stop,
        });
        let context = InvocationContext::new(meter.clone());
        let result = FeatureInputCache::retain_observed(
            &cache,
            &FeatureIdentityInput::Owned(selected.identity.clone()),
            selected.feature,
            &context,
        );
        let entries = cache.lock().unwrap().entries;
        if let Ok(Some(value)) = &result {
            assert_eq!(value.as_ref(), selected.feature);
            assert_eq!(
                serde_json::to_value(value.as_ref()).unwrap(),
                serde_json::to_value(selected.feature).unwrap()
            );
        }
        (result, meter.bytes.get(), meter.calls.get(), entries)
    };
    let (complete, bytes, calls, entries) = run(usize::MAX, usize::MAX);
    assert!(complete.unwrap().is_some());
    assert_eq!(entries, 1);
    assert!(run(bytes, usize::MAX).0.unwrap().is_some());
    assert!(run(bytes - 1, usize::MAX).0.is_err());
    for stop in 0..calls {
        let denied = run(usize::MAX, stop);
        assert!(denied.0.is_err());
        assert_eq!(denied.3, 0);
        assert!(run(bytes, usize::MAX).0.unwrap().is_some());
    }
    let cache = std::sync::Arc::new(std::sync::Mutex::new(FeatureInputCache::default()));
    let observation = InvocationContext::default();
    let first = FeatureInputCache::retain_observed(
        &cache,
        &FeatureIdentityInput::Owned(selected.identity.clone()),
        selected.feature,
        &observation,
    )
    .unwrap()
    .unwrap();
    let second = FeatureInputCache::retain_observed(
        &cache,
        &FeatureIdentityInput::Owned(selected.identity.clone()),
        selected.feature,
        &observation,
    )
    .unwrap()
    .unwrap();
    assert!(std::sync::Arc::ptr_eq(&first, &second));
    {
        let guard = cache.lock().unwrap();
        assert!(
            FeatureInputCache::retain_observed(
                &cache,
                &FeatureIdentityInput::Owned(selected.identity.clone()),
                selected.feature,
                &observation
            )
            .is_err()
        );
        drop(guard);
    }
    let fallback = std::sync::Arc::new(std::sync::Mutex::new(FeatureInputCache {
        entries: 4096,
        ..Default::default()
    }));
    let input = prepare_source_input_map(&schema, receiver, fallback, &observation).unwrap();
    let PreparedFeatureInputs::Shared(input) = input else {
        panic!("input map missing");
    };
    assert!(
        input
            .fields
            .values()
            .all(|input| matches!(input.field, FeatureInput::Owned(_)))
    );
    assert_eq!(
        input
            .fields
            .into_iter()
            .map(|(name, input)| (name.to_string(), (*input.field).clone()))
            .collect::<BTreeMap<_, _>>(),
        schema.features(receiver).unwrap()
    );
}
#[test]
fn source_feature_default_projection_keeps_complete_values_and_conflicts_fresh() {
    let mut schema = shared_map_schema();
    let receiver = schema
        .runtime_feature_domains
        .keys()
        .next()
        .unwrap()
        .clone();
    let domains = schema.runtime_feature_domains.get_mut(&receiver).unwrap();
    let name = domains[0].source.projection.source.name.clone();
    domains[0].default = serde_json::json!("modified");
    let owner = schema.feature_owner(&receiver, &name).unwrap();
    let input = schema.classes[owner].features[&name].clone();
    let observation = InvocationContext::default();
    let mut view = FeatureDefaultPreparation::from_input_observed(
        &receiver,
        std::sync::Arc::new(input.clone()),
        &observation,
    )
    .unwrap();
    let mut owned = FeatureDefaultPreparation::new(&receiver, &input);
    while !view.advance(&schema).unwrap() {}
    while !owned.advance(&schema).unwrap() {}
    let FeatureInput::Owned(actual) = view.finish_input_observed(&observation).unwrap() else {
        panic!("changed receiver default remained a shared source input");
    };
    let expected = owned.finish().unwrap();
    assert_eq!(actual, expected);
    assert_eq!(
        serde_json::to_value(actual).unwrap(),
        serde_json::to_value(expected).unwrap()
    );
    let mut conflict = schema.clone();
    let mut extra = conflict.runtime_feature_domains[&receiver][0].clone();
    extra.default = serde_json::json!("conflict");
    conflict
        .runtime_feature_domains
        .get_mut(&receiver)
        .unwrap()
        .push(extra);
    let mut view = FeatureDefaultPreparation::from_input_observed(
        &receiver,
        std::sync::Arc::new(input.clone()),
        &observation,
    )
    .unwrap();
    let mut owned = FeatureDefaultPreparation::new(&receiver, &input);
    let run = |state: &mut FeatureDefaultPreparation| -> Result<(), String> {
        while !state.advance(&conflict)? {}
        Ok(())
    };
    assert_eq!(run(&mut view).unwrap_err(), run(&mut owned).unwrap_err());
    assert!(view.advance(&conflict).is_err());
    assert!(view.finish_input_observed(&observation).is_err());
}

#[test]
fn complete_source_input_map_owned_inspection_precharges_all_fields_and_recovers() {
    struct Meter {
        bytes: Cell<usize>,
        calls: Cell<usize>,
        limit: usize,
        stop: usize,
    }
    impl Meter {
        fn step(&self) -> Result<(), String> {
            let n = self.calls.get();
            self.calls.set(n + 1);
            if n == self.stop {
                Err("input inspection cancelled".into())
            } else {
                Ok(())
            }
        }
    }
    impl InvocationObserver for Meter {
        fn before_invoke(&self, _: &ProviderId) -> Result<(), String> {
            Err("no providers".into())
        }
        fn work(&self, _: usize) -> Result<(), String> {
            self.step()
        }
        fn retain_storage(&self, bytes: usize) -> Result<(), String> {
            self.step()?;
            let n = self
                .bytes
                .get()
                .checked_add(bytes)
                .filter(|n| *n <= self.limit)
                .ok_or("inspection storage denied")?;
            self.bytes.set(n);
            Ok(())
        }
    }
    let schema = shared_map_schema();
    let before = serde_json::to_value(&schema).unwrap();
    let receiver = schema.classes.keys().next().unwrap();
    let mut fresh = FeatureMapPreparation::new(receiver);
    while !fresh.advance(&schema).unwrap() {}
    let expected = fresh.finish().unwrap();
    let run = |limit, stop| {
        let cache = std::sync::Arc::new(std::sync::Mutex::new(FeatureInputCache::default()));
        let input = prepare_source_input_map(
            &schema,
            receiver,
            cache.clone(),
            &InvocationContext::default(),
        )
        .unwrap();
        let meter = Rc::new(Meter {
            bytes: Cell::new(0),
            calls: Cell::new(0),
            limit,
            stop,
        });
        let context = InvocationContext::new(meter.clone());
        let result = input.into_owned_observed(&context);
        if let Ok(value) = &result {
            assert_eq!(value.receiver(), expected.receiver());
            assert_eq!(value.fields, expected.fields);
            assert_eq!(value.owners, expected.owners);
            assert_eq!(value.identities, expected.identities);
            assert_eq!(value.order, expected.order);
            assert_eq!(
                serde_json::to_value(value.fields()).unwrap(),
                serde_json::to_value(expected.fields()).unwrap()
            );
        }
        (result, meter.bytes.get(), meter.calls.get())
    };
    let (complete, bytes, calls) = run(usize::MAX, usize::MAX);
    complete.unwrap();
    assert!(bytes > 0);
    assert!(run(bytes, usize::MAX).0.is_ok());
    assert!(run(bytes - 1, usize::MAX).0.is_err());
    for stop in 0..calls {
        assert!(run(usize::MAX, stop).0.is_err());
        assert!(run(bytes, usize::MAX).0.is_ok());
    }
    assert_eq!(serde_json::to_value(schema).unwrap(), before);
}

#[test]
fn selected_source_identity_inputs_share_all_values_and_keep_validation_fresh() {
    let schema = shared_map_schema();
    let before = serde_json::to_value(&schema).unwrap();
    let cache = std::sync::Arc::new(std::sync::Mutex::new(FeatureInputCache::default()));
    let context = InvocationContext::default();
    let mut retained = BTreeMap::<MemberIdentity, std::sync::Arc<MemberIdentity>>::new();
    let mut reused = false;
    for receiver in schema.classes.keys() {
        let input = prepare_source_input_map(&schema, receiver, cache.clone(), &context).unwrap();
        let mut owned = FeatureMapPreparation::new(receiver);
        while !owned.advance(&schema).unwrap() {}
        let owned = owned.finish().unwrap();
        assert_eq!(&input.member_identities(), owned.member_identities());
        assert_eq!(input.fields_for_test(), owned.fields);
        assert_eq!(&input.owners_for_test(), &owned.owners);
        assert_eq!(input.order(), owned.order());
        let PreparedFeatureInputs::Shared(input) = input else {
            panic!("missing complete input map")
        };
        for (name, row) in input.fields {
            let identity = row.identity;
            let FeatureIdentityInput::Shared(identity) = identity else {
                panic!("source identity was copied")
            };
            assert_eq!(
                serde_json::to_value(identity.as_ref()).unwrap(),
                serde_json::to_value(&owned.member_identities()[name.as_ref()]).unwrap()
            );
            if let Some(previous) = retained.get(identity.as_ref()) {
                assert!(std::sync::Arc::ptr_eq(previous, &identity));
                reused = true;
            }
            retained.insert(identity.as_ref().clone(), identity);
        }
    }
    assert!(reused);
    assert_eq!(serde_json::to_value(&schema).unwrap(), before);
    let mut invalid = schema.clone();
    let (owner, class) = invalid
        .classes
        .iter_mut()
        .find(|(_, class)| !class.feature_declarations.is_empty())
        .unwrap();
    let owner = owner.clone();
    class
        .feature_declarations
        .values_mut()
        .next()
        .unwrap()
        .path
        .clear();
    let private_error = prepare_source_input_map(&invalid, &owner, cache, &context)
        .err()
        .unwrap()
        .to_string();
    let mut public = FeatureMapPreparation::new(&owner);
    let public_error = loop {
        match public.advance(&invalid) {
            Ok(false) => {}
            Ok(true) => panic!("invalid declaration accepted"),
            Err(error) => break error,
        }
    };
    assert_eq!(private_error, public_error);
}

#[test]
fn complete_source_identity_inputs_precharge_cold_values_cancel_recover_and_fallback() {
    struct Meter {
        bytes: Cell<usize>,
        calls: Cell<usize>,
        limit: usize,
        stop: usize,
    }
    impl Meter {
        fn step(&self) -> Result<(), String> {
            let n = self.calls.get();
            self.calls.set(n + 1);
            if n == self.stop {
                Err("identity input cancelled".into())
            } else {
                Ok(())
            }
        }
    }
    impl InvocationObserver for Meter {
        fn before_invoke(&self, _: &ProviderId) -> Result<(), String> {
            Err("no providers".into())
        }
        fn work(&self, _: usize) -> Result<(), String> {
            self.step()
        }
        fn retain_storage(&self, bytes: usize) -> Result<(), String> {
            self.step()?;
            let n = self
                .bytes
                .get()
                .checked_add(bytes)
                .filter(|n| *n <= self.limit)
                .ok_or("identity input storage denied")?;
            self.bytes.set(n);
            Ok(())
        }
    }
    let schema = shared_map_schema();
    let declaration = schema
        .classes
        .values()
        .find_map(|class| class.feature_declarations.values().next())
        .unwrap();
    let expected = MemberIdentity::Source {
        declaration: declaration.clone(),
    };
    let run = |limit, stop| {
        let cache = std::sync::Arc::new(std::sync::Mutex::new(FeatureInputCache::default()));
        let meter = Rc::new(Meter {
            bytes: Cell::new(0),
            calls: Cell::new(0),
            limit,
            stop,
        });
        let context = InvocationContext::new(meter.clone());
        let result =
            FeatureInputCache::retain_source_identity_observed(&cache, declaration, &context);
        if let Ok(value) = &result {
            assert_eq!(&**value, &expected);
            assert_eq!(
                serde_json::to_value(&**value).unwrap(),
                serde_json::to_value(&expected).unwrap()
            );
        }
        let count = cache.lock().unwrap().source_ids.len();
        (result, meter.bytes.get(), meter.calls.get(), count)
    };
    let (result, bytes, calls, count) = run(usize::MAX, usize::MAX);
    assert!(result.is_ok());
    assert_eq!(count, 1);
    assert!(run(bytes, usize::MAX).0.is_ok());
    assert!(run(bytes - 1, usize::MAX).0.is_err());
    for stop in 0..calls {
        let denied = run(usize::MAX, stop);
        assert!(denied.0.is_err());
        assert_eq!(denied.3, 0);
        assert!(run(bytes, usize::MAX).0.is_ok());
    }
    let cache = std::sync::Arc::new(std::sync::Mutex::new(FeatureInputCache::default()));
    let context = InvocationContext::default();
    let FeatureIdentityInput::Shared(first) =
        FeatureInputCache::retain_source_identity_observed(&cache, declaration, &context).unwrap()
    else {
        panic!("cold value copied")
    };
    let meter = Rc::new(Meter {
        bytes: Cell::new(0),
        calls: Cell::new(0),
        limit: 0,
        stop: usize::MAX,
    });
    let FeatureIdentityInput::Shared(second) = FeatureInputCache::retain_source_identity_observed(
        &cache,
        declaration,
        &InvocationContext::new(meter.clone()),
    )
    .unwrap() else {
        panic!("hot value copied")
    };
    assert!(std::sync::Arc::ptr_eq(&first, &second));
    assert_eq!(meter.bytes.get(), 0);
    {
        let guard = cache.lock().unwrap();
        assert!(
            FeatureInputCache::retain_source_identity_observed(&cache, declaration, &context)
                .is_err()
        );
        drop(guard);
    }
    {
        let mut guard = cache.lock().unwrap();
        for n in 0..4096 {
            let mut key = declaration.clone();
            key.path = vec![usize::MAX, n];
            guard
                .source_ids
                .insert(key, std::sync::Arc::new(expected.clone()));
        }
    }
    let mut extra = declaration.clone();
    extra.path.push(usize::MAX);
    let fallback =
        FeatureInputCache::retain_source_identity_observed(&cache, &extra, &context).unwrap();
    assert!(matches!(fallback, FeatureIdentityInput::Owned(_)));
    assert_eq!(*fallback, MemberIdentity::Source { declaration: extra });
}

#[test]
fn source_input_table_release_is_staged_preserves_aliases_and_cancels_terminally() {
    use super::super::{InstantiationValidation, InstantiationValidationStage};
    struct Stop;
    impl InvocationObserver for Stop {
        fn before_invoke(&self, _: &ProviderId) -> Result<(), String> {
            Err("no providers".into())
        }
        fn work(&self, _: usize) -> Result<(), String> {
            Ok(())
        }
        fn checkpoint(&self) -> Result<(), String> {
            Err("input release cancelled".into())
        }
    }
    let schema = shared_map_schema();
    let before = serde_json::to_value(&schema).unwrap();
    let context = InvocationContext::default();
    let cache = std::sync::Arc::new(std::sync::Mutex::new(FeatureInputCache::default()));
    let receiver = schema.classes.keys().next().unwrap();
    let retained = prepare_source_input_map(&schema, receiver, cache.clone(), &context).unwrap();
    let saved_fields = retained.fields_for_test();
    let saved_ids = retained.member_identities();
    let remaining =
        |cache: &FeatureInputCache| cache.entries + cache.inputs.len() + cache.source_ids.len();
    {
        let mut guard = cache.lock().unwrap();
        assert!(remaining(&guard) > 0);
        loop {
            let n = remaining(&guard);
            let done = guard.clear_one().unwrap();
            assert!(n - remaining(&guard) <= 1);
            if done {
                break;
            }
        }
        assert_eq!(guard.entries, 0);
    }
    assert_eq!(retained.fields_for_test(), saved_fields);
    assert_eq!(retained.member_identities(), saved_ids);
    let rebuilt = prepare_source_input_map(&schema, receiver, cache.clone(), &context).unwrap();
    assert_eq!(rebuilt.fields_for_test(), saved_fields);
    assert_eq!(rebuilt.member_identities(), saved_ids);
    let mut state = InstantiationValidation::with_reference_views();
    state.stage = InstantiationValidationStage::IdentityClear;
    state.input_cache = Some(cache.clone());
    assert!(state.advance_observed(&schema, &context).unwrap());
    assert!(state.input_cache.is_none());
    assert!(cache.lock().unwrap().entries > 0); // Aliased inputs are still available.
    assert!(state.advance_observed(&schema, &context).unwrap());
    assert!(state.input_cache.is_none());
    drop(cache);
    let cache = std::sync::Arc::new(std::sync::Mutex::new(FeatureInputCache::default()));
    let _retained = prepare_source_input_map(&schema, receiver, cache.clone(), &context).unwrap();
    let mut state = InstantiationValidation::with_reference_views();
    state.stage = InstantiationValidationStage::IdentityClear;
    state.input_cache = Some(cache);
    let count = remaining(&state.input_cache.as_ref().unwrap().lock().unwrap());
    assert!(
        state
            .advance_observed(&schema, &InvocationContext::new(Rc::new(Stop)))
            .is_err()
    );
    assert_eq!(
        remaining(&state.input_cache.as_ref().unwrap().lock().unwrap()),
        count
    );
    assert!(state.advance_observed(&schema, &context).is_err());
    let mut recovery = InstantiationValidation::with_reference_views();
    while !recovery.advance_observed(&schema, &context).unwrap() {}
    assert!(recovery.input_cache.is_none());
    assert!(recovery.shared_fields.is_empty());
    assert_eq!(serde_json::to_value(&schema).unwrap(), before);
}

#[test]
fn staged_runtime_type_proof_requires_completion_and_exact_immutable_schema() {
    let value = schema();
    assert!(ValidatedRuntimeTypes::begin(&value).finish().is_err());
    let mut preparation = ValidatedRuntimeTypes::begin(&value);
    while !preparation
        .advance_observed(&InvocationContext::default())
        .unwrap()
    {}
    let proof = preparation.finish().unwrap();
    assert!(proof.belongs_to(&value));
    let clone = value.clone();
    assert!(!proof.belongs_to(&clone));
    assert!(
        ValidatedRuntimeTypes::prepare_observed(&clone, &InvocationContext::default())
            .unwrap()
            .belongs_to(&clone)
    );
}

#[test]
fn staged_runtime_type_proof_cancels_every_advance_and_rejects_recovery_on_failed_progress() {
    struct Cancel {
        stopped: Cell<bool>,
    }
    impl InvocationObserver for Cancel {
        fn before_invoke(&self, _: &ProviderId) -> Result<(), String> {
            Err("Unexpected type-proof provider call".into())
        }
        fn work(&self, _: usize) -> Result<(), String> {
            if self.stopped.get() {
                Err("Type proof cancelled".into())
            } else {
                Ok(())
            }
        }
        fn checkpoint(&self) -> Result<(), String> {
            if self.stopped.get() {
                Err("Type proof cancelled".into())
            } else {
                Ok(())
            }
        }
    }
    let value = schema();
    let mut baseline = ValidatedRuntimeTypes::begin(&value);
    let mut advances = 0;
    loop {
        advances += 1;
        if baseline
            .advance_observed(&InvocationContext::default())
            .unwrap()
        {
            break;
        }
    }
    for prefix in 0..advances {
        let mut preparation = ValidatedRuntimeTypes::begin(&value);
        for _ in 0..prefix {
            assert!(
                !preparation
                    .advance_observed(&InvocationContext::default())
                    .unwrap()
            );
        }
        let meter = Rc::new(Cancel {
            stopped: Cell::new(true),
        });
        assert!(
            preparation
                .advance_observed(&InvocationContext::new(meter.clone()))
                .is_err()
        );
        meter.stopped.set(false);
        assert!(
            preparation
                .advance_observed(&InvocationContext::new(meter))
                .is_err()
        );
        assert!(preparation.finish().is_err());
        let proof =
            ValidatedRuntimeTypes::prepare_observed(&value, &InvocationContext::default()).unwrap();
        assert!(proof.belongs_to(&value));
    }
}

#[test]
fn prepared_member_positions_preserve_inherited_derived_declaration_order() {
    let mut schema = legacy_feature_schema();
    for (owner, name, derived) in [("A", "zeta", true), ("D", "alpha", false)] {
        let mut field = feature();
        field.name = name.into();
        field.derived = derived;
        let class = schema.classes.get_mut(owner).unwrap();
        class.features.insert(name.into(), field);
        class.feature_order.push(name.into());
    }
    let observation = InvocationContext::default();
    let cache = std::sync::Arc::new(std::sync::Mutex::new(FeatureInputCache::default()));
    let shared = prepare_source_input_map(&schema, "D", cache, &observation).unwrap();
    let mut fresh = FeatureMapPreparation::new("D");
    while !fresh.advance(&schema).unwrap() {}
    let owned = fresh.finish_inputs().unwrap();
    let expected = schema.effective_features("D").unwrap();
    assert_eq!(
        expected
            .iter()
            .map(|field| field.feature.name.as_str())
            .collect::<Vec<_>>(),
        ["x", "zeta", "alpha"]
    );
    for fields in [&shared, &owned] {
        for field in &expected {
            let position = fields
                .named_position_observed(&field.feature.name, &observation)
                .unwrap();
            assert_eq!(position, Some(field.position));
            assert_eq!(
                fields.ordered_name(position.unwrap()),
                Some(field.feature.name.as_str())
            );
            assert_eq!(
                fields.member_identity(&field.feature.name),
                Some(&field.identity)
            );
        }
        assert_eq!(
            fields
                .named_position_observed("missing", &observation)
                .unwrap(),
            None
        );
        assert_eq!(
            fields.named_position_observed("", &observation).unwrap(),
            None
        );
    }
}

#[test]
fn prepared_member_position_lookup_is_paid_and_cancellable_before_lookup() {
    struct Meter {
        work: Cell<usize>,
        limit: usize,
        cancelled: bool,
    }
    impl InvocationObserver for Meter {
        fn before_invoke(&self, _: &ProviderId) -> Result<(), String> {
            Err("Unexpected provider".into())
        }
        fn checkpoint(&self) -> Result<(), String> {
            if self.cancelled {
                Err("member lookup cancelled".into())
            } else {
                Ok(())
            }
        }
        fn work(&self, units: usize) -> Result<(), String> {
            let next = self.work.get().checked_add(units).ok_or("work overflow")?;
            if next > self.limit {
                return Err("member lookup exhausted".into());
            }
            self.work.set(next);
            Ok(())
        }
    }
    let mut schema = legacy_feature_schema();
    for index in 0..128 {
        let name = format!("field{index:03}");
        let mut field = feature();
        field.name = name.clone();
        let class = schema.classes.get_mut("A").unwrap();
        class.features.insert(name.clone(), field);
        class.feature_order.push(name);
    }
    let cache = std::sync::Arc::new(std::sync::Mutex::new(FeatureInputCache::default()));
    let shared =
        prepare_source_input_map(&schema, "D", cache, &InvocationContext::default()).unwrap();
    let mut fresh = FeatureMapPreparation::new("D");
    while !fresh.advance(&schema).unwrap() {}
    let owned = fresh.finish_inputs().unwrap();
    let measure = |fields: &PreparedFeatureInputs| {
        let meter = Rc::new(Meter {
            work: Cell::new(0),
            limit: usize::MAX,
            cancelled: false,
        });
        let result = fields
            .named_position_observed("field127", &InvocationContext::new(meter.clone()))
            .unwrap();
        (result, meter.work.get())
    };
    let (result, indexed) = measure(&shared);
    let (literal_result, scanned) = measure(&owned);
    assert_eq!(result, literal_result);
    assert!(indexed > 0 && indexed < scanned);
    for (limit, cancelled, expected) in [
        (indexed - 1, false, "member lookup exhausted"),
        (usize::MAX, true, "member lookup cancelled"),
    ] {
        let meter = Rc::new(Meter {
            work: Cell::new(0),
            limit,
            cancelled,
        });
        assert_eq!(
            shared
                .named_position_observed("field127", &InvocationContext::new(meter))
                .unwrap_err(),
            expected
        );
    }
}
