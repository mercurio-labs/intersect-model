//! Fresh import rows retain canonical defaults without sharing mutable state.
use super::*;
use crate::definitions::compiler::inspect_ecore_schema;
use serde_json::json;

fn schema() -> Schema {
    inspect_ecore_schema(r##"<e:EPackage xmlns:e="http://www.eclipse.org/emf/2002/Ecore" xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance" name="test" nsURI="urn:xmi-fresh">
      <eClassifiers xsi:type="e:EClass" name="Base" abstract="true">
        <eStructuralFeatures xsi:type="e:EAttribute" name="label" unsettable="true" defaultValueLiteral="fresh" eType="e:EDataType http://www.eclipse.org/emf/2002/Ecore#//EString"/>
        <eStructuralFeatures xsi:type="e:EAttribute" name="tags" upperBound="-1" eType="e:EDataType http://www.eclipse.org/emf/2002/Ecore#//EString"/>
      </eClassifiers>
      <eClassifiers xsi:type="e:EClass" name="Item" eSuperTypes="#//Base"/>
    </e:EPackage>"##).unwrap()
}

#[test]
fn reused_rows_equal_canonical_construction_and_keep_mutations_local() {
    let schema = schema();
    let mut fresh = FreshElements::new(&schema);
    let mut first = fresh.create("first", "Item", "memory:first", 1, 2).unwrap();
    assert_eq!(
        first,
        Element::new(&schema, "first", "Item", "memory:first", 1, 2).unwrap()
    );
    first.values.insert("label".into(), json!("changed"));
    first
        .values
        .get_mut("tags")
        .unwrap()
        .as_array_mut()
        .unwrap()
        .push(json!("changed"));
    first.assignment_state = Some(crate::core::model::AssignmentState::new(["label"]).unwrap());
    first.implicit = true;
    let second = fresh
        .create("second", "Item", "memory:second", 3, 9)
        .unwrap();
    assert_eq!(
        second,
        Element::new(&schema, "second", "Item", "memory:second", 3, 9).unwrap()
    );
    assert_eq!(second.values["label"], json!("fresh"));
    assert_eq!(second.values["tags"], json!([]));
    assert!(!second.implicit);
    assert!(second.initialization.is_none());
    assert!(fresh.templates["Item"].id.is_empty());
    assert!(fresh.templates["Item"].resource.is_empty());
}

#[test]
fn failed_constructions_do_not_seed_templates_and_schema_changes_get_fresh_defaults() {
    let mut schema = schema();
    {
        let mut fresh = FreshElements::new(&schema);
        for (id, class) in [("", "Item"), ("a", "Base"), ("a", "Missing")] {
            assert_eq!(
                fresh.create(id, class, "memory:x", 0, 1),
                Element::new(&schema, id, class, "memory:x", 0, 1)
            );
        }
        assert!(fresh.templates.is_empty());
        fresh.create("a", "Item", "memory:x", 0, 1).unwrap();
        assert_eq!(
            fresh.create("", "Item", "memory:x", 0, 1),
            Element::new(&schema, "", "Item", "memory:x", 0, 1)
        );
    }
    schema
        .classes
        .get_mut("Base")
        .unwrap()
        .features
        .get_mut("label")
        .unwrap()
        .default = json!("new schema");
    let mut fresh = FreshElements::new(&schema);
    assert_eq!(
        fresh.create("b", "Item", "memory:x", 0, 1).unwrap().values["label"],
        json!("new schema")
    );
}

#[test]
fn template_capacity_falls_back_to_the_canonical_factory() {
    let mut xml = String::from(
        r#"<e:EPackage xmlns:e="http://www.eclipse.org/emf/2002/Ecore" xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance" name="test" nsURI="urn:xmi-many">"#,
    );
    for i in 0..FreshElements::CAPACITY + 2 {
        xml.push_str(&format!(
            r#"<eClassifiers xsi:type="e:EClass" name="C{i}"/>"#
        ));
    }
    xml.push_str("</e:EPackage>");
    let schema = inspect_ecore_schema(&xml).unwrap();
    let mut fresh = FreshElements::new(&schema);
    for i in 0..FreshElements::CAPACITY + 2 {
        let class = format!("C{i}");
        assert_eq!(
            fresh.create("a", &class, "memory:x", i, i + 1),
            Element::new(&schema, "a", &class, "memory:x", i, i + 1)
        );
    }
    assert_eq!(fresh.templates.len(), FreshElements::CAPACITY);
    let class = format!("C{}", FreshElements::CAPACITY);
    assert!(!fresh.templates.contains_key(&class));
    assert_eq!(
        fresh.create("b", &class, "memory:y", 7, 8),
        Element::new(&schema, "b", &class, "memory:y", 7, 8)
    );
}
