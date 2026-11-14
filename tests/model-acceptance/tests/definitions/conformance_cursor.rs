use intersect_model::definitions::schema::ConformancePreparation;

#[test]
fn deep_conformance_yields_and_publishes_only_after_complete_validation() {
    let mut body = String::new();
    for n in 0..64 {
        let parents = if n == 0 {
            String::new()
        } else {
            format!(r##" eSuperTypes="#//C{:02}""##, n - 1)
        };
        body.push_str(&format!(
            r#"<eClassifiers x:type="e:EClass" name="C{n:02}"{parents}/>"#
        ));
    }
    body.push_str(r#"<eClassifiers x:type="e:EClass" name="Other"/>"#);
    let schema = intersect_model_acceptance::migration_fixture::compile_migrated_schema_fixture(&format!(r#"<e:EPackage xmlns:e="http://www.eclipse.org/emf/2002/Ecore" xmlns:x="http://www.w3.org/2001/XMLSchema-instance" name="m" nsURI="urn:conformance-cursor">{body}</e:EPackage>"#)).unwrap();
    assert!(ConformancePreparation::new("C63", "C00").finish().is_err());
    for (expected, accepted) in [
        ("C00", true),
        ("C32", true),
        ("C63", true),
        ("Other", false),
        ("Missing", false),
    ] {
        let mut pending = ConformancePreparation::new("C63", expected);
        let mut slices = 0;
        while !pending.advance(&schema).unwrap() {
            slices += 1;
            assert!(slices < 1000);
        }
        assert!(slices >= 64 * 4);
        assert!(pending.advance(&schema).unwrap());
        assert_eq!(pending.finish().unwrap(), accepted);
        assert_eq!(schema.conforms("C63", expected), accepted);
    }
    let mut invalid = schema;
    invalid.classes.get_mut("C00").unwrap().parents = vec!["C63".into()];
    // Even an exact expected class cannot bypass a cycle elsewhere in ancestry.
    assert!(!invalid.conforms("C63", "C63"));
    let mut pending = ConformancePreparation::new("C63", "C63");
    loop {
        match pending.advance(&invalid) {
            Ok(false) => {}
            Ok(true) => panic!("cycle accepted"),
            Err(error) => {
                assert_eq!(error, "Cyclic inheritance at C63");
                assert_eq!(
                    pending.advance(&invalid).unwrap_err(),
                    "Conformance preparation has failed"
                );
                break;
            }
        }
    }
    assert!(pending.finish().is_err());
}
