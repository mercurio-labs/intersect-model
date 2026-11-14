use intersect_model::definitions::schema::{
    InstantiationValidation, ReferenceStructurePreparation, Schema,
};

fn wide_schema() -> Schema {
    let fields = (0..64).map(|n| format!(r##"<eStructuralFeatures x:type="e:EAttribute" name="f{n:02}" eType="e:EDataType http://www.eclipse.org/emf/2002/Ecore#//EString"/>"##)).collect::<String>();
    intersect_model_acceptance::migration_fixture::compile_migrated_schema_fixture(&format!(r##"<e:EPackage xmlns:e="http://www.eclipse.org/emf/2002/Ecore" xmlns:x="http://www.w3.org/2001/XMLSchema-instance" name="m" nsURI="urn:reference-structure">
      <eClassifiers x:type="e:EClass" name="Base">{fields}</eClassifiers>
      <eClassifiers x:type="e:EClass" name="Node" eSuperTypes="#//Base">
        <eStructuralFeatures x:type="e:EReference" name="next" eType="#//Node" upperBound="-1"/>
      </eClassifiers>
      <eClassifiers x:type="e:EClass" name="Alias" eSuperTypes="#//Node"/>
      <eClassifiers x:type="e:EClass" name="Other">{fields}
        <eStructuralFeatures x:type="e:EReference" name="next" eType="#//Node"/>
      </eClassifiers>
    </e:EPackage>"##)).unwrap()
}

fn drain(
    schema: &Schema,
    cursor: &mut ReferenceStructurePreparation,
) -> (usize, Result<(), String>) {
    for step in 1..10_000 {
        match cursor.advance(schema) {
            Ok(false) => {}
            Ok(true) => return (step, Ok(())),
            Err(error) => return (step, Err(error)),
        }
    }
    panic!("reference structure failed to terminate")
}

#[test]
fn wide_opposite_members_and_alias_owners_yield_without_publishing_partial_success() {
    let mut schema = wide_schema();
    schema
        .classes
        .get_mut("Node")
        .unwrap()
        .features
        .get_mut("next")
        .unwrap()
        .opposite = Some("Other/next".into());
    let feature = &schema.classes["Node"].features["next"];
    assert!(
        ReferenceStructurePreparation::new("Alias", feature)
            .finish()
            .is_err()
    );
    let mut cursor = ReferenceStructurePreparation::new("Alias", feature);
    let (steps, result) = drain(&schema, &mut cursor);
    assert_eq!(result, Ok(()));
    assert!(steps >= 64 * 8, "opposite inventories must yield: {steps}");
    assert!(cursor.advance(&schema).unwrap());
    cursor.finish().unwrap();
    schema.validate_instantiations().unwrap();
    let mut validation = InstantiationValidation::new();
    let mut validation_steps = 0;
    while !validation.advance(&schema).unwrap() {
        validation_steps += 1;
        assert!(validation_steps < 20_000);
    }
    assert!(validation_steps >= steps);
    for budget in [0, 1, 64, steps / 2, steps - 1] {
        let mut cancelled = ReferenceStructurePreparation::new("Alias", feature);
        for _ in 0..budget {
            assert!(!cancelled.advance(&schema).unwrap());
        }
        assert!(cancelled.finish().is_err());
    }

    // An inherited receiver spelling cannot hide an identical declaring owner.
    let mut alias = feature.clone();
    alias.opposite = Some("Node/next".into());
    let mut cursor = ReferenceStructurePreparation::new("Alias", &alias);
    let (steps, result) = drain(&schema, &mut cursor);
    assert!(steps >= 64 * 8);
    assert_eq!(
        result.unwrap_err(),
        "Alias.next: A reference cannot be its own opposite"
    );
    assert_eq!(
        cursor.advance(&schema).unwrap_err(),
        "Reference structure preparation has failed"
    );
    assert!(cursor.finish().is_err());
}

#[test]
fn structural_errors_preserve_literal_precedence_and_poison_the_cursor() {
    let mut schema = wide_schema();
    let mut feature = schema.classes["Node"].features["next"].clone();
    feature.unique = false;
    feature.opposite = Some("invalid".into());
    let mut cursor = ReferenceStructurePreparation::new("Node", &feature);
    assert_eq!(
        cursor.advance(&schema).unwrap_err(),
        "Containment or bidirectional reference must be unique when upper bound differs from 1: Node.next"
    );
    assert!(cursor.advance(&schema).is_err());
    assert!(cursor.finish().is_err());
    feature.unique = true;
    let mut cursor = ReferenceStructurePreparation::new("Node", &feature);
    assert_eq!(
        cursor.advance(&schema).unwrap_err(),
        "Invalid opposite identity"
    );
    feature.opposite = Some("Other/missing".into());
    assert_eq!(
        drain(
            &schema,
            &mut ReferenceStructurePreparation::new("Node", &feature)
        )
        .1
        .unwrap_err(),
        "Unknown opposite feature"
    );
    feature.opposite = Some("Other/next".into());
    schema
        .classes
        .get_mut("Other")
        .unwrap()
        .features
        .get_mut("next")
        .unwrap()
        .containment = true;
    let mut cursor = ReferenceStructurePreparation::new("Node", &feature);
    let (steps, result) = drain(&schema, &mut cursor);
    assert!(steps >= 64 * 4);
    assert_eq!(
        result.unwrap_err(),
        "Container reference must have upper bound 1: Node.next"
    );
    assert!(cursor.advance(&schema).is_err());
    assert!(cursor.finish().is_err());
    // Singular references retain the previous absence of a container-map lookup.
    feature.upper = 1;
    feature.opposite = Some("Missing/different".into());
    let mut cursor = ReferenceStructurePreparation::new("Node", &feature);
    assert_eq!(drain(&schema, &mut cursor).1, Ok(()));
    cursor.finish().unwrap();
}
