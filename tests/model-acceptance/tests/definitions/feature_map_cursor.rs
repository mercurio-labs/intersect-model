use intersect_model::definitions::schema::{
    FeatureMapPreparation, InstantiationValidation, MemberIdentity, PreparedFeatures,
};

#[test]
fn wide_inherited_feature_maps_yield_defaults_and_release_every_retained_entry() {
    let fields = (0..64).map(|n| format!(r#"<eStructuralFeatures x:type="e:EAttribute" name="f{n:02}" defaultValueLiteral="v{n:02}" eType="e:EDataType http://www.eclipse.org/emf/2002/Ecore#//EString"/>"#)).collect::<String>();
    let schema = intersect_model_acceptance::migration_fixture::compile_migrated_schema_fixture(&format!(r##"<e:EPackage xmlns:e="http://www.eclipse.org/emf/2002/Ecore" xmlns:x="http://www.w3.org/2001/XMLSchema-instance" name="m" nsURI="urn:feature-map"><eClassifiers x:type="e:EClass" name="Base">{fields}</eClassifiers><eClassifiers x:type="e:EClass" name="Node" eSuperTypes="#//Base"/></e:EPackage>"##)).unwrap();
    assert!(FeatureMapPreparation::new("Node").finish().is_err());
    let mut pending = FeatureMapPreparation::new("Node");
    let mut steps = 0;
    while !pending.advance(&schema).unwrap() {
        steps += 1;
        assert!(steps < 10000);
    }
    assert!(steps >= 64 * 8);
    assert!(pending.advance(&schema).unwrap());
    let mut projected = pending.finish().unwrap();
    assert_eq!(projected.receiver(), "Node");
    assert_eq!(
        projected.order(),
        &(0..64).map(|n| format!("f{n:02}")).collect::<Vec<_>>()
    );
    assert_eq!(projected.member_identities().len(), 64);
    let synchronous = schema.features("Node").unwrap();
    for n in 0..64 {
        let name = format!("f{n:02}");
        assert_eq!(projected.get(&name), synchronous.get(&name));
        assert_eq!(
            projected.get(&name).unwrap().default,
            serde_json::json!(format!("v{n:02}"))
        );
        assert_eq!(projected.owner(&name), Some("Base"));
        assert_eq!(
            projected.member_identities().get(&name),
            Some(&MemberIdentity::Source {
                declaration: intersect_model::definitions::resources::DeclarationId {
                    source: schema.declarations["Base"].source.clone(),
                    path: vec![0, n],
                },
            })
        );
    }
    for stop in [0, 1, 64, steps / 2, steps - 1] {
        let mut partial = FeatureMapPreparation::new("Node");
        for _ in 0..stop {
            assert!(!partial.advance(&schema).unwrap());
        }
        assert!(partial.finish().is_err());
    }
    let remaining = |projection: &PreparedFeatures| {
        projection.fields().len()
            + (0..64)
                .filter(|n| projection.owner(&format!("f{n:02}")).is_some())
                .count()
            + projection.member_identities().len()
            + projection.order().len()
    };
    assert_eq!(remaining(&projected), 256);
    let mut cleanup = 0;
    loop {
        let before = remaining(&projected);
        let done = projected.clear_one();
        let after = remaining(&projected);
        cleanup += 1;
        assert_eq!(before - after, 1, "one retained entry per release");
        assert_eq!(
            done,
            after == 0,
            "completion requires every inventory empty"
        );
        assert!(cleanup <= 256);
        if done {
            break;
        }
    }
    assert_eq!(cleanup, 256);
    assert!(projected.clear_one());
    assert_eq!(remaining(&projected), 0);
    assert!(projected.get("f63").is_none());
    assert!(projected.owner("f63").is_none());
    let mut missing = FeatureMapPreparation::new("Missing");
    assert_eq!(
        missing.advance(&schema).unwrap_err(),
        "Unknown class Missing"
    );
    assert_eq!(
        missing.advance(&schema).unwrap_err(),
        "Feature map preparation has failed"
    );
    assert!(missing.finish().is_err());

    fn validation_steps(schema: &intersect_model::definitions::schema::Schema) -> usize {
        let mut pending = InstantiationValidation::new();
        for step in 1..10000 {
            if pending.advance(schema).unwrap() {
                return step;
            }
        }
        panic!("runtime validation did not terminate")
    }
    let baseline = validation_steps(&schema);
    for runtime in [false, true] {
        let mut checked = schema.clone();
        // Empty retained policies still validate the owner's full inventory.
        if runtime {
            checked
                .runtime_feature_domains
                .insert("Node".into(), vec![]);
        } else {
            checked
                .raw_reference_bounds
                .insert("Node".into(), Default::default());
        }
        assert!(validation_steps(&checked) >= baseline + 64 * 8);
        checked.validate_instantiations().unwrap();
    }
}
