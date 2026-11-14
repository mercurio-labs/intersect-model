use intersect_model::definitions::dynamic::{DynamicSchema, ScalarType, SchemaBuilder};
use serde_json::json;

#[test]
fn handles_freeze_and_inherited_lookup_preserves_declaration_identity() {
    let mut builder = SchemaBuilder::new("example", "urn:example");
    let named = builder.class("Named").unwrap();
    named.abstract_type().unwrap();
    let name = named
        .attribute("name", ScalarType::String)
        .required()
        .unwrap();
    let book = builder.class("Book").unwrap();
    book.extends(&named).unwrap();
    let delayed = book.attribute("late", ScalarType::Boolean);
    let schema = builder.build().unwrap();
    assert_eq!(schema.class("Book").unwrap(), book);
    assert_eq!(book.feature("name").unwrap(), name);
    assert!(delayed.build().is_err());
    assert!(named.abstract_type().is_err());
    assert!(book.extends(&named).is_err());
    assert!(builder.class("Late").is_err());
    assert!(builder.build().is_err());
    let fresh = DynamicSchema::from_schema(schema.as_schema().clone()).unwrap();
    assert!(fresh.check_class(&book).is_err());
}

#[test]
fn malformed_definitions_fail_without_freezing_and_foreign_handles_fail_early() {
    let mut builder = SchemaBuilder::new("example", "urn:example");
    assert!(builder.build().is_err());
    let a = builder.class("A").unwrap();
    assert!(builder.class("A").is_err());
    let mut foreign = SchemaBuilder::new("example", "urn:example");
    let b = foreign.class("B").unwrap();
    assert!(a.extends(&b).is_err());
    assert!(a.reference("b", &b).optional().is_err());
    let x = a.attribute("x", ScalarType::String).optional().unwrap();
    assert!(a.attribute("x", ScalarType::Integer).build().is_err());
    assert!(builder.opposites(&x, &x).is_err());
    builder.build().unwrap();
}

#[test]
fn shared_validator_rejects_invalid_defaults_bounds_targets_and_inheritance() {
    let mut builder = SchemaBuilder::new("example", "urn:example");
    let a = builder.class("A").unwrap();
    a.attribute("x", ScalarType::Integer).default(1).unwrap();
    let valid = builder.build().unwrap().as_schema().clone();
    for invalid in [json!(true), json!(2147483648_i64), json!([])] {
        let mut s = valid.clone();
        s.classes
            .get_mut("A")
            .unwrap()
            .features
            .get_mut("x")
            .unwrap()
            .default = invalid;
        assert!(s.validate().is_err());
    }
    for upper in [-2, 0] {
        let mut s = valid.clone();
        s.classes
            .get_mut("A")
            .unwrap()
            .features
            .get_mut("x")
            .unwrap()
            .upper = upper;
        assert!(s.validate().is_err());
    }
    let mut s = valid.clone();
    s.classes.get_mut("A").unwrap().parents = vec!["Missing".into()];
    assert!(s.validate().is_err());
    let mut s = valid.clone();
    s.classes.get_mut("A").unwrap().parents = vec!["A".into()];
    assert!(s.validate().is_err());
    let mut s = valid.clone();
    s.classes
        .get_mut("A")
        .unwrap()
        .features
        .get_mut("x")
        .unwrap()
        .target = "EFloat".into();
    assert!(s.validate().is_err());
    let mut s = valid;
    let f = s
        .classes
        .get_mut("A")
        .unwrap()
        .features
        .get_mut("x")
        .unwrap();
    f.reference = true;
    f.target = "Missing".into();
    f.default = json!(null);
    assert!(s.validate().is_err());
}

#[test]
fn deterministic_revisions_and_ecore_structural_equivalence_keep_provenance_separate() {
    fn build(reverse: bool) -> DynamicSchema {
        let mut b = SchemaBuilder::new("example", "urn:example");
        let (a, c) = if reverse {
            let c = b.class("Child").unwrap();
            (b.class("Parent").unwrap(), c)
        } else {
            let a = b.class("Parent").unwrap();
            (a, b.class("Child").unwrap())
        };
        a.abstract_type().unwrap();
        c.extends(&a).unwrap();
        a.attribute("name", ScalarType::String).required().unwrap();
        c.attribute("edition", ScalarType::Integer)
            .default(1)
            .unwrap();
        b.build().unwrap()
    }
    let built = build(false);
    let reordered = build(true);
    assert_eq!(built.as_schema(), reordered.as_schema());
    let imported=intersect_model_acceptance::migration_fixture::compile_migrated_schema_fixture(r##"<ecore:EPackage xmlns:ecore="http://www.eclipse.org/emf/2002/Ecore" xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance" name="example" nsURI="urn:example">
<eClassifiers xsi:type="ecore:EClass" name="Parent" abstract="true"><eStructuralFeatures xsi:type="ecore:EAttribute" name="name" lowerBound="1" eType="ecore:EDataType http://www.eclipse.org/emf/2002/Ecore#//EString"/></eClassifiers>
<eClassifiers xsi:type="ecore:EClass" name="Child" eSuperTypes="#//Parent"><eStructuralFeatures xsi:type="ecore:EAttribute" name="edition" defaultValueLiteral="1" eType="ecore:EDataType http://www.eclipse.org/emf/2002/Ecore#//EInt"/></eClassifiers></ecore:EPackage>"##).unwrap();
    let mut structural = imported.classes.clone();
    for (name, class) in &mut structural {
        let dynamic = &built.as_schema().classes[name];
        assert!(dynamic.feature_declarations.is_empty());
        assert!(dynamic.feature_order.is_empty());
        assert_eq!(class.feature_order.len(), class.features.len());
        assert_eq!(class.feature_declarations.len(), class.features.len());
        for declaration in class.feature_declarations.values() {
            assert_eq!(declaration.source.uri, "memory:fixture.rcore");
            assert_eq!(declaration.path.len(), 2);
        }
        // Only source provenance/order differ from the programmatic declarations.
        class.feature_order.clear();
        class.feature_declarations.clear();
    }
    assert_eq!(built.as_schema().classes, structural);
    assert_ne!(built.as_schema().revision, imported.revision);
    let mut corrupt = built.as_schema().clone();
    corrupt.name = "changed".into();
    assert!(DynamicSchema::from_schema(corrupt).is_err());
}

#[test]
fn inherited_opposites_validate_at_the_declaring_class() {
    let mut b = SchemaBuilder::new("example", "urn:example");
    let parent = b.class("Parent").unwrap();
    let child = b.class("Child").unwrap();
    let item = b.class("Item").unwrap();
    child.extends(&parent).unwrap();
    let items = parent
        .reference("items", &item)
        .many()
        .containment()
        .unwrap();
    let owner = item.reference("owner", &parent).optional().unwrap();
    b.opposites(&items, &owner).unwrap();
    b.build().unwrap();
}

#[test]
fn builder_rejects_invalid_final_definitions() {
    for invalid in ["default", "bounds", "cycle", "override", "opposite"] {
        let mut b = SchemaBuilder::new("example", "urn:example");
        let a = b.class("A").unwrap();
        let c = b.class("Child").unwrap();
        match invalid {
            "default" => {
                a.attribute("x", ScalarType::Integer).default(true).unwrap();
            }
            "bounds" => {
                a.attribute("x", ScalarType::Integer)
                    .bounds(3, 2)
                    .build()
                    .unwrap();
            }
            "cycle" => {
                a.extends(&c).unwrap();
                c.extends(&a).unwrap();
            }
            "override" => {
                c.extends(&a).unwrap();
                a.attribute("x", ScalarType::String).optional().unwrap();
                c.attribute("x", ScalarType::String).optional().unwrap();
            }
            "opposite" => {
                let one = a.reference("one", &c).containment().unwrap();
                let two = c.reference("two", &a).containment().unwrap();
                b.opposites(&one, &two).unwrap();
            }
            _ => unreachable!(),
        }
        assert!(b.build().is_err(), "{invalid}");
    }
}
