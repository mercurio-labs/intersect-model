use intersect_model::definitions::dynamic::{DynamicSchema, SchemaBuilder};

fn source(opposite: &str) -> String {
    format!(
        r##"<e:EPackage xmlns:e="http://www.eclipse.org/emf/2002/Ecore" xmlns:x="http://www.w3.org/2001/XMLSchema-instance" name="identity" nsURI="urn:identity">
<eClassifiers x:type="e:EClass" name="Node"><eStructuralFeatures x:type="e:EReference" name="peer" eType="#//Node" eOpposite="{opposite}"/></eClassifiers>
<eClassifiers x:type="e:EClass" name="Child" eSuperTypes="#//Node"/>
</e:EPackage>"##
    )
}

#[test]
fn source_and_shared_validator_reject_self_opposites_including_inherited_aliases() {
    for target in ["#//Node/peer", "#//Child/peer"] {
        let error = intersect_model_acceptance::migration_fixture::compile_migrated_schema_fixture(
            &source(target),
        )
        .unwrap_err();
        // Inherited XML fragments do not identify locally declared members.
        let expected = if target == "#//Child/peer" {
            "Missing named fragment peer"
        } else {
            "own opposite"
        };
        assert!(error.contains(expected), "{target}: {error}");
    }
    let mut builder = SchemaBuilder::new("identity", "urn:identity");
    let node = builder.class("Node").unwrap();
    let child = builder.class("Child").unwrap();
    child.extends(&node).unwrap();
    let peer = node.reference("peer", &node).optional().unwrap();
    assert!(builder.opposites(&peer, &peer).is_err());
    let valid = builder.build().unwrap();
    for target in ["Node/peer", "Child/peer"] {
        let mut damaged = valid.as_schema().clone();
        damaged
            .classes
            .get_mut("Node")
            .unwrap()
            .features
            .get_mut("peer")
            .unwrap()
            .opposite = Some(target.into());
        assert!(damaged.validate().unwrap_err().contains("own opposite"));
        assert!(DynamicSchema::from_schema(damaged).is_err());
    }
}

#[test]
fn same_name_references_on_distinct_classes_remain_valid() {
    let mut builder = SchemaBuilder::new("identity", "urn:identity");
    let a = builder.class("A").unwrap();
    let b = builder.class("B").unwrap();
    let left = a.reference("peer", &b).optional().unwrap();
    let right = b.reference("peer", &a).optional().unwrap();
    builder.opposites(&left, &right).unwrap();
    builder.build().unwrap();
}
