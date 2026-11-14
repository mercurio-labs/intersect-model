use intersect_model::{
    api::ModelEdit,
    compile_rcore_at,
    definitions::bundle::{DefinitionSource, SourceKind},
    migration::migrate_ecore,
    rcore::{DataWire, ExecutionLimits, RcoreError, RcorePackage},
};

#[test]
fn migrated_model_edits_behavior_and_reload_share_the_rcore_lifecycle()
-> Result<(), Box<dyn std::error::Error>> {
    let original = DefinitionSource::new(
        "memory:graph.ecore",
        SourceKind::Ecore,
        r##"<e:EPackage xmlns:e="http://www.eclipse.org/emf/2002/Ecore" xmlns:x="http://www.w3.org/2001/XMLSchema-instance" name="graph" nsURI="urn:graph" nsPrefix="g">
        <eClassifiers x:type="e:EClass" name="Node">
          <eStructuralFeatures x:type="e:EAttribute" name="name" eType="e:EDataType http://www.eclipse.org/emf/2002/Ecore#//EString" defaultValueLiteral="unnamed"/>
          <eStructuralFeatures x:type="e:EReference" name="links" eType="#//Node" upperBound="-1"/>
          <eOperations name="reachable" eType="#//Node" upperBound="-1"/>
        </eClassifiers></e:EPackage>"##,
    );
    let report = migrate_ecore(original.clone(), "memory:graph.rcore")?;
    let mut converted = report.converted.ok_or("Expected complete conversion")?;
    let unbound = compile_rcore_at(&converted.id.uri, &converted.text)?;
    let mut model = unbound.new_model()?;
    model.create("a", "Node", "memory:objects")?;
    assert!(matches!(
        unbound.invoke(&model, "a", "reachable", &[], &ExecutionLimits::default()),
        Err(RcoreError::MissingImplementation { .. })
    ));
    // The host supplies this implementation explicitly; migration never invents it.
    converted.text.push_str(r#"behavior { import model "urn:graph" as m; fn reachable(node:m.Node)->OrderedSet<m.Node> { closure([node], |n| n.links) } }"#);
    let mut package = compile_rcore_at(&converted.id.uri, &converted.text)?;
    package.bind_operation("Node", "reachable", "reachable")?;
    let mut model = package.new_model()?;
    for id in ["a", "b", "c"] {
        model.create(id, "Node", "memory:objects")?;
    }
    model.set("a", "links", serde_json::json!(["b"]))?;
    model.set("b", "links", serde_json::json!(["c"]))?;
    model.validate()?;
    let before = package.save(&model)?;
    assert!(
        model
            .set("b", "links", serde_json::json!(["missing"]))
            .is_err()
    );
    assert_eq!(before, package.save(&model)?);
    let result = package.invoke(&model, "a", "reachable", &[], &ExecutionLimits::default())?;
    let DataWire::Collection(values) = &result.value.data else {
        panic!("Expected collection")
    };
    assert_eq!(values.len(), 3);
    let (restored, model) = RcorePackage::reload(&before)?;
    assert_eq!(
        restored
            .invoke(&model, "a", "reachable", &[], &ExecutionLimits::default())?
            .value,
        result.value
    );
    assert_eq!(report.original, original);
    Ok(())
}

#[test]
fn same_source_type_references_migrate_as_names_and_features_stay_positional()
-> Result<(), Box<dyn std::error::Error>> {
    // The enum precedes the classes, so Rcore's class-first numbering differs
    // from source order. `@eClassifiers.2` is an original positional href.
    let original = DefinitionSource::new(
        "memory:org.ecore",
        SourceKind::Ecore,
        r##"<e:EPackage xmlns:e="http://www.eclipse.org/emf/2002/Ecore" xmlns:x="http://www.w3.org/2001/XMLSchema-instance" name="org" nsURI="urn:org" nsPrefix="o">
        <eClassifiers x:type="e:EEnum" name="Kind"><eLiterals name="a"/></eClassifiers>
        <eClassifiers x:type="e:EClass" name="Named"/>
        <eClassifiers x:type="e:EClass" name="Team" eSuperTypes="#//Named">
          <eStructuralFeatures x:type="e:EAttribute" name="kind" eType="#//Kind"/>
          <eStructuralFeatures x:type="e:EReference" name="members" eType="#//Person" upperBound="-1" eOpposite="#//Person/team"/>
          <eOperations name="lead" eType="#//@eClassifiers.3"/>
        </eClassifiers>
        <eClassifiers x:type="e:EClass" name="Person" eSuperTypes="#//@eClassifiers.1">
          <eStructuralFeatures x:type="e:EReference" name="team" eType="#//@eClassifiers.2" eOpposite="#//Team/members"/>
        </eClassifiers></e:EPackage>"##,
    );
    let report = migrate_ecore(original, "memory:org.rcore")?;
    let converted = report.converted.ok_or("Expected complete conversion")?;
    let text = &converted.text;
    for named in [
        "class Team extends Named",
        "class Person extends Named",
        "attr kind: Kind",
        "ref members: Person",
        "ref team: Team",
        ") -> Person",
    ] {
        assert!(text.contains(named), "missing {named} in {text}");
    }
    for positional_type in [": external", "-> external", "extends external"] {
        assert!(!text.contains(positional_type), "{text}");
    }
    // Feature references are not type names; they keep the exact fragment.
    assert!(
        text.contains("opposite external \"#//@eClassifiers."),
        "{text}"
    );
    compile_rcore_at(&converted.id.uri, text)?;
    Ok(())
}

#[test]
fn migrated_names_are_quoted_only_where_a_bare_word_would_be_read_differently()
-> Result<(), Box<dyn std::error::Error>> {
    let original = DefinitionSource::new(
        "memory:words.ecore",
        SourceKind::Ecore,
        r##"<e:EPackage xmlns:e="http://www.eclipse.org/emf/2002/Ecore" xmlns:x="http://www.w3.org/2001/XMLSchema-instance" name="words" nsURI="urn:words" nsPrefix="w">
        <eClassifiers x:type="e:EClass" name="String">
          <eStructuralFeatures x:type="e:EAttribute" name="in" eType="e:EDataType http://www.eclipse.org/emf/2002/Ecore#//EString"/>
        </eClassifiers>
        <eClassifiers x:type="e:EClass" name="Holder">
          <eStructuralFeatures x:type="e:EReference" name="opposite" eType="#//String"/>
          <eStructuralFeatures x:type="e:EAttribute" name="type" eType="e:EDataType http://www.eclipse.org/emf/2002/Ecore#//EBoolean"/>
        </eClassifiers></e:EPackage>"##,
    );
    let report = migrate_ecore(original, "memory:words.rcore")?;
    let converted = report.converted.ok_or("Expected complete conversion")?;
    let text = &converted.text;
    for spelled in [
        "package words version",
        "prefix w {",
        "class `String` {",
        "attr `in`: EString",
        "class Holder {",
        "ref `opposite`: `String`",
        "attr type: EBoolean",
    ] {
        assert!(text.contains(spelled), "missing {spelled} in {text}");
    }
    // The quoted class stays a model class; it is not the String scalar.
    let package = compile_rcore_at(&converted.id.uri, text)?;
    let mut model = package.new_model()?;
    model.create("s", "String", "memory:objects")?;
    model.create("h", "Holder", "memory:objects")?;
    model.set("h", "opposite", serde_json::json!("s"))?;
    model.set("s", "in", serde_json::json!("text"))?;
    model.validate()?;
    Ok(())
}
