//! Focused controls for one exact immutable validator's canonical opposite inputs.
use super::*;
use crate::definitions::conversion::{InvocationObserver, ProviderId};
use std::{cell::Cell, rc::Rc};

fn repeated_opposites() -> Schema {
    let mut value = crate::definitions::compiler::inspect_ecore_schema(
        r##"<e:EPackage xmlns:e="http://www.eclipse.org/emf/2002/Ecore" xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance" name="t" nsURI="urn:opposite-reuse">
<eClassifiers xsi:type="e:EClass" name="A"/>
<eClassifiers xsi:type="e:EClass" name="B"/>
</e:EPackage>"##,
    ).unwrap();
    let field = crate::definitions::schema::Feature {
        name: "y".into(),
        target: "A".into(),
        datatype: None,
        reference: true,
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
    };
    let target = value.classes.get_mut("B").unwrap();
    target.features.insert("y".into(), field.clone());
    target.feature_order.push("y".into());
    for index in 0..16 {
        let name = format!("member{index:02}");
        let mut other = field.clone();
        other.name = name.clone();
        target.features.insert(name.clone(), other);
        target.feature_order.push(name);
    }
    let owner = value.classes.get_mut("A").unwrap();
    for index in 0..16 {
        let name = format!("link{index:02}");
        let mut other = field.clone();
        other.name = name.clone();
        other.target = "B".into();
        other.upper = -1;
        other.opposite = Some("B/y".into());
        owner.features.insert(name.clone(), other);
        owner.feature_order.push(name);
    }
    value
}
struct Meter {
    work: Cell<usize>,
    limit: usize,
    bytes: Cell<usize>,
    checkpoints: Cell<usize>,
    stop: usize,
}
impl InvocationObserver for Meter {
    fn before_invoke(&self, _: &ProviderId) -> Result<(), String> {
        Err("Unexpected metadata provider".into())
    }
    fn work(&self, units: usize) -> Result<(), String> {
        let next = self.work.get().checked_add(units).ok_or("Work overflow")?;
        if next > self.limit {
            return Err("opposite work denied".into());
        }
        self.work.set(next);
        Ok(())
    }
    fn retain_storage(&self, bytes: usize) -> Result<(), String> {
        self.bytes.set(
            self.bytes
                .get()
                .checked_add(bytes)
                .ok_or("Storage overflow")?,
        );
        Ok(())
    }
    fn checkpoint(&self) -> Result<(), String> {
        let count = self.checkpoints.get();
        self.checkpoints.set(count + 1);
        if count == self.stop {
            Err("opposite cancelled".into())
        } else {
            Ok(())
        }
    }
}
fn meter(limit: usize, stop: usize) -> Rc<Meter> {
    Rc::new(Meter {
        work: Cell::new(0),
        limit,
        bytes: Cell::new(0),
        checkpoints: Cell::new(0),
        stop,
    })
}
fn complete(value: &Schema, observer: Rc<Meter>) -> Result<(), PreparationError> {
    let context = InvocationContext::new(observer);
    let mut pending = ValidatedRuntimeTypes::begin(value);
    loop {
        match pending.advance_observed(&context) {
            Ok(false) => {}
            Ok(true) => {
                return pending
                    .finish()
                    .map(|proof| assert!(proof.belongs_to(value)));
            }
            Err(error) => {
                assert!(
                    pending
                        .advance_observed(&InvocationContext::default())
                        .is_err()
                );
                assert!(pending.finish().is_err());
                return Err(error);
            }
        }
    }
}
#[test]
fn opposite_inputs_reuse_preserves_same_schema_and_reduces_actual_repeated_preparation() {
    let value = repeated_opposites();
    let original = value.clone();
    let fresh = meter(usize::MAX, usize::MAX);
    let context = InvocationContext::new(fresh.clone());
    let mut cold = super::super::InstantiationValidation::new();
    while !cold.advance_observed(&value, &context).unwrap() {}
    let reused = meter(usize::MAX, usize::MAX);
    complete(&value, reused.clone()).unwrap();
    assert!(reused.work.get() < fresh.work.get());
    assert!(reused.bytes.get() < fresh.bytes.get());
    let proof =
        ValidatedRuntimeTypes::prepare_observed(&value, &InvocationContext::default()).unwrap();
    assert!(proof.belongs_to(&value));
    assert!(!proof.belongs_to(&value.clone()));
    assert_eq!(value, original);
}
#[test]
fn opposite_inputs_reuse_preserves_malformed_and_self_opposite_errors() {
    let value = repeated_opposites();
    let mut variants = Vec::new();
    let mut bad = value.clone();
    bad.classes
        .get_mut("B")
        .unwrap()
        .features
        .get_mut("y")
        .unwrap()
        .containment = true;
    variants.push(bad);
    let mut bad = value.clone();
    bad.classes
        .get_mut("A")
        .unwrap()
        .features
        .get_mut("link01")
        .unwrap()
        .opposite = Some("B/missing".into());
    variants.push(bad);
    let mut bad = value.clone();
    bad.classes
        .get_mut("A")
        .unwrap()
        .features
        .get_mut("link01")
        .unwrap()
        .unique = false;
    variants.push(bad);
    let mut bad = value.clone();
    let field = bad
        .classes
        .get_mut("A")
        .unwrap()
        .features
        .get_mut("link01")
        .unwrap();
    field.opposite = Some("A/link01".into());
    variants.push(bad);
    for invalid in variants {
        let original = invalid.clone();
        let cold = invalid.validate_instantiations().unwrap_err();
        let reused = complete(&invalid, meter(usize::MAX, usize::MAX)).unwrap_err();
        assert_eq!(reused, PreparationError::Schema(cold));
        assert_eq!(invalid, original);
    }
}
#[test]
fn opposite_inputs_reuse_pays_exact_work_and_keeps_failures_sticky() {
    let value = repeated_opposites();
    let measured = meter(usize::MAX, usize::MAX);
    complete(&value, measured.clone()).unwrap();
    let required = measured.work.get();
    assert!(required > 0);
    complete(&value, meter(required, usize::MAX)).unwrap();
    assert_eq!(
        complete(&value, meter(required - 1, usize::MAX)).unwrap_err(),
        PreparationError::Observation("opposite work denied".into())
    );
    for stop in [
        0,
        measured.checkpoints.get() / 2,
        measured.checkpoints.get() - 1,
    ] {
        assert_eq!(
            complete(&value, meter(usize::MAX, stop)).unwrap_err(),
            PreparationError::Observation("opposite cancelled".into())
        );
    }
}

#[test]
fn opposite_inputs_reuse_keeps_actual_imported_declaration_inputs_and_independent_owners() {
    use crate::definitions::bundle::{DefinitionBundle, DefinitionSource, SourceKind};
    let mut source = String::from(
        r#"<e:EPackage xmlns:e="http://www.eclipse.org/emf/2002/Ecore" xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance" name="t" nsURI="urn:source-opposite-reuse"><eClassifiers xsi:type="e:EClass" name="A">"#,
    );
    for index in 0..8 {
        source.push_str(&format!(r##"<eStructuralFeatures xsi:type="e:EReference" name="link{index}" upperBound="-1" eType="#//B" eOpposite="#//B/back{index}"/>"##));
    }
    source.push_str(r#"</eClassifiers><eClassifiers xsi:type="e:EClass" name="B">"#);
    for index in 0..8 {
        source.push_str(&format!(r##"<eStructuralFeatures xsi:type="e:EReference" name="back{index}" eType="#//A" eOpposite="#//A/link{index}"/>"##));
    }
    source.push_str("</eClassifiers></e:EPackage>");
    let entry = DefinitionSource::new("memory:source-opposite-reuse", SourceKind::Ecore, source);
    let original_bundle = DefinitionBundle {
        version: 1,
        sources: vec![entry.clone()],
        namespaces: vec![],
    };
    let migration = crate::migration::migrate_ecore_bundle(
        original_bundle.clone(),
        &[(
            entry.id.clone(),
            "memory:source-opposite-reuse.rcore".into(),
        )]
        .into(),
    )
    .unwrap();
    assert_eq!(migration.original, original_bundle);
    let report = migration
        .reports
        .iter()
        .find(|report| report.original.id == entry.id)
        .unwrap();
    let converted_entry = report.converted.as_ref().unwrap();
    let converted = migration.converted.as_ref().unwrap();
    let value = crate::definitions::compiler::compile_structure_bundle(
        converted,
        &converted_entry.id,
        "urn:source-opposite-reuse",
    )
    .unwrap();
    for owner in ["A", "B"] {
        for declaration in value.classes[owner].feature_declarations.values() {
            assert_eq!(declaration.source, converted_entry.id);
            assert!(
                report
                    .correspondence
                    .iter()
                    .any(|row| row.converted == *declaration && row.original.source == entry.id)
            );
        }
    }
    assert_eq!(value.classes["A"].feature_declarations.len(), 8);
    assert_eq!(value.classes["B"].feature_declarations.len(), 8);
    let original = value.clone();
    complete(&value, meter(usize::MAX, usize::MAX)).unwrap();
    let other_owner = value.clone();
    complete(&other_owner, meter(usize::MAX, usize::MAX)).unwrap();
    assert_eq!(value, original);
    let proof =
        ValidatedRuntimeTypes::prepare_observed(&value, &InvocationContext::default()).unwrap();
    assert!(!proof.belongs_to(&other_owner));
}
