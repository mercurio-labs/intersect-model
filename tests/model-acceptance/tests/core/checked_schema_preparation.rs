use intersect_model::core::{
    model::Model,
    storage::{ModelView, StructuralPreparation},
};
use intersect_model::definitions::{
    conversion::{InvocationContext, InvocationObserver, ProviderId},
    schema::{CheckedRuntimeSchema, Schema},
};
use serde_json::json;
use std::{cell::Cell, rc::Rc};
fn fixture() -> (Schema, Model) {
    let schema = intersect_model_acceptance::migration_fixture::compile_migrated_schema_fixture("<e:EPackage xmlns:e='http://www.eclipse.org/emf/2002/Ecore' xmlns:x='http://www.w3.org/2001/XMLSchema-instance' name='m' nsURI='urn:checked-model'><eClassifiers x:type='e:EClass' name='Root'><eStructuralFeatures x:type='e:EAttribute' name='label' lowerBound='1' eType='e:EDataType http://www.eclipse.org/emf/2002/Ecore#//EString'/><eStructuralFeatures x:type='e:EReference' name='children' upperBound='-1' containment='true' eType='#//Child'/><eStructuralFeatures x:type='e:EReference' name='favorite' lowerBound='1' eType='#//Child'/></eClassifiers><eClassifiers x:type='e:EClass' name='Child'><eStructuralFeatures x:type='e:EAttribute' name='count' eType='e:EDataType http://www.eclipse.org/emf/2002/Ecore#//EInt'/><eStructuralFeatures x:type='e:EReference' name='children' upperBound='-1' containment='true' eType='#//Root'/></eClassifiers></e:EPackage>").unwrap();
    let mut model = Model::default();
    model
        .create(&schema, "root", "Root", "memory:checked-model", 0, 0)
        .unwrap();
    model
        .create(&schema, "child", "Child", "memory:checked-model", 0, 0)
        .unwrap();
    model.elements[0]
        .values
        .insert("label".into(), json!("root"));
    model.elements[0]
        .values
        .insert("children".into(), json!(["child"]));
    model.elements[0]
        .values
        .insert("favorite".into(), json!("child"));
    model.elements[1].values.insert("count".into(), json!(2));
    model.resources =
        intersect_model::core::resources::records_for_elements(&schema, &model.elements).unwrap();
    (schema, model)
}
fn finish(
    schema: &CheckedRuntimeSchema,
    model: &Model,
    observation: &InvocationContext<'_>,
) -> Result<intersect_model::core::storage::OwnershipIndex, String> {
    let view = ModelView::new(model)?;
    let mut pending = StructuralPreparation::ownership();
    let mut steps = 0;
    loop {
        match pending.advance_checked_schema(&view, schema, None, Some(observation)) {
            Ok(true) => return pending.finish(),
            Ok(false) => {
                steps += 1;
                assert!(steps < 2000);
            }
            Err(error) => {
                assert!(
                    pending
                        .advance_checked_schema(&view, schema, None, None)
                        .is_err()
                );
                assert!(pending.finish().is_err());
                return Err(error);
            }
        }
    }
}
#[test]
fn checked_types_keep_exact_ownership_and_all_model_failures() {
    let (raw, model) = fixture();
    let operation = InvocationContext::default();
    let checked = CheckedRuntimeSchema::prepare_observed(raw.clone(), &operation).unwrap();
    let owners = finish(&checked, &model, &operation).unwrap();
    assert_eq!(owners.parent("child"), Some("root"));
    assert_eq!(owners.children("root"), &["child"]);
    let mut cases = Vec::new();
    let mut bad = model.clone();
    bad.schema_revision = "foreign".into();
    cases.push(bad);
    let mut bad = model.clone();
    bad.elements[0].type_identity = "foreign".into();
    cases.push(bad);
    let mut bad = model.clone();
    bad.elements[0].values.remove("label");
    cases.push(bad);
    let mut bad = model.clone();
    bad.elements[0].values.remove("favorite");
    cases.push(bad);
    let mut bad = model.clone();
    bad.elements[1]
        .values
        .insert("count".into(), json!("invalid"));
    cases.push(bad);
    let mut bad = model.clone();
    bad.elements[0]
        .values
        .insert("children".into(), json!(["absent"]));
    cases.push(bad);
    let mut bad = model.clone();
    bad.elements[1]
        .values
        .insert("children".into(), json!(["root"]));
    cases.push(bad);
    for bad in cases {
        let error = finish(&checked, &bad, &operation).err().unwrap();
        let view = ModelView::new(&bad).unwrap();
        let mut ordinary = StructuralPreparation::ownership();
        let baseline = loop {
            match ordinary.advance(&view, &raw, None, None) {
                Ok(false) => {}
                Ok(true) => panic!("malformed model admitted"),
                Err(error) => break error,
            }
        };
        assert_eq!(error, baseline);
    }
}
#[test]
fn checked_schema_identity_cannot_switch_or_fall_back_to_wire_schema() {
    let (raw, model) = fixture();
    let operation = InvocationContext::default();
    let checked = CheckedRuntimeSchema::prepare_observed(raw.clone(), &operation).unwrap();
    let foreign = CheckedRuntimeSchema::prepare_observed(raw.clone(), &operation).unwrap();
    let view = ModelView::new(&model).unwrap();
    let mut pending = StructuralPreparation::ownership();
    assert!(
        !pending
            .advance_checked_schema(&view, &checked, None, None)
            .unwrap()
    );
    assert!(
        pending
            .advance_checked_schema(&view, &foreign, None, None)
            .unwrap_err()
            .contains("schema changed")
    );
    assert!(
        pending
            .advance_checked_schema(&view, &checked, None, None)
            .is_err()
    );
    assert!(pending.finish().is_err());
    let mut pending = StructuralPreparation::ownership();
    assert!(
        !pending
            .advance_checked_schema(&view, &checked, None, None)
            .unwrap()
    );
    assert!(pending.advance(&view, &raw, None, None).is_err());
    assert!(pending.finish().is_err());
    let mut pending = StructuralPreparation::ownership();
    assert!(!pending.advance(&view, &raw, None, None).unwrap());
    assert!(
        pending
            .advance_checked_schema(&view, &checked, None, None)
            .is_err()
    );
    assert!(pending.finish().is_err());
}
#[derive(Default)]
struct Control {
    cancelled: Cell<bool>,
}
impl InvocationObserver for Control {
    fn before_invoke(&self, _: &ProviderId) -> Result<(), String> {
        Err("Unexpected provider".into())
    }
    fn checkpoint(&self) -> Result<(), String> {
        if self.cancelled.get() {
            Err("checked model cancelled".into())
        } else {
            Ok(())
        }
    }
    fn work(&self, _: usize) -> Result<(), String> {
        self.checkpoint()
    }
    fn retain_storage(&self, _: usize) -> Result<(), String> {
        self.checkpoint()
    }
}
#[test]
fn checked_model_cancellation_poisoning_keeps_fresh_recovery() {
    let (raw, model) = fixture();
    let operation = InvocationContext::default();
    let checked = CheckedRuntimeSchema::prepare_observed(raw, &operation).unwrap();
    let view = ModelView::new(&model).unwrap();
    for prefix in [0, 1, 2, 5, 12, 32] {
        let control = Rc::new(Control::default());
        let operation = InvocationContext::new(control.clone());
        let mut pending = StructuralPreparation::ownership();
        for _ in 0..prefix {
            assert!(
                !pending
                    .advance_checked_schema(&view, &checked, None, Some(&operation))
                    .unwrap()
            );
        }
        control.cancelled.set(true);
        assert!(
            pending
                .advance_checked_schema(&view, &checked, None, Some(&operation))
                .unwrap_err()
                .contains("cancelled")
        );
        control.cancelled.set(false);
        assert!(
            pending
                .advance_checked_schema(&view, &checked, None, None)
                .is_err()
        );
        assert!(pending.finish().is_err());
    }
    assert_eq!(
        finish(&checked, &model, &operation)
            .unwrap()
            .parent("child"),
        Some("root")
    );
}

#[test]
fn prepared_order_preserves_inherited_containment_order_and_first_invalid_field() {
    let raw=intersect_model_acceptance::migration_fixture::compile_migrated_schema_fixture(r##"<e:EPackage xmlns:e="http://www.eclipse.org/emf/2002/Ecore" xmlns:x="http://www.w3.org/2001/XMLSchema-instance" name="order" nsURI="urn:prepared-order">
      <eClassifiers x:type="e:EClass" name="Base">
        <eStructuralFeatures x:type="e:EAttribute" name="zRequired" lowerBound="1" eType="e:EDataType http://www.eclipse.org/emf/2002/Ecore#//EString"/>
        <eStructuralFeatures x:type="e:EAttribute" name="aRequired" lowerBound="1" eType="e:EDataType http://www.eclipse.org/emf/2002/Ecore#//EString"/>
        <eStructuralFeatures x:type="e:EReference" name="zChildren" upperBound="-1" containment="true" eType="#//Child"/>
        <eStructuralFeatures x:type="e:EReference" name="aChildren" upperBound="-1" containment="true" eType="#//Child"/>
      </eClassifiers>
      <eClassifiers x:type="e:EClass" name="Root" eSuperTypes="#//Base"/>
      <eClassifiers x:type="e:EClass" name="Child"/>
    </e:EPackage>"##).unwrap();
    let operation = InvocationContext::default();
    let checked = CheckedRuntimeSchema::prepare_observed(raw.clone(), &operation).unwrap();
    let mut model = Model::default();
    for (id, class) in [("root", "Root"), ("a", "Child"), ("z", "Child")] {
        model
            .create(&raw, id, class, "memory:prepared-order", 0, 0)
            .unwrap();
    }
    for (name, value) in [
        ("zRequired", json!("kept")),
        ("aRequired", json!("kept")),
        ("zChildren", json!(["z"])),
        ("aChildren", json!(["a"])),
    ] {
        model.elements[0].values.insert(name.into(), value);
    }
    model.resources =
        intersect_model::core::resources::records_for_elements(&raw, &model.elements).unwrap();
    let before = model.clone();
    let owners = finish(&checked, &model, &operation).unwrap();
    assert_eq!(owners.children("root"), &["z", "a"]);
    assert_eq!(owners.parent("z"), Some("root"));
    assert_eq!(owners.parent("a"), Some("root"));
    assert_eq!(model, before);
    model.elements[0]
        .values
        .insert("zRequired".into(), json!(42));
    model.elements[0]
        .values
        .insert("aRequired".into(), json!(false));
    let before = model.clone();
    let actual = finish(&checked, &model, &operation).err().unwrap();
    assert!(actual.contains("Root.zRequired"), "{actual}");
    assert!(!actual.contains("Root.aRequired"), "{actual}");
    let view = ModelView::new(&model).unwrap();
    let mut ordinary = StructuralPreparation::ownership();
    let expected = loop {
        match ordinary.advance(&view, &raw, None, None) {
            Ok(false) => {}
            Ok(true) => panic!("invalid fields admitted"),
            Err(error) => break error,
        }
    };
    assert_eq!(actual, expected);
    assert_eq!(model, before);
}
