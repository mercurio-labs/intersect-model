use intersect_model::core::interchange::{ModelFormat, Resource};
use intersect_model::definitions::{
    bundle::{DefinitionBundle, DefinitionSource, SourceKind},
    ecore_migration::migrate_ecore,
    rcore::compile_structure,
};
use intersect_model::xmi::XmiFormat;

#[test]
fn migration_keeps_namespace_and_xmi_model_interchange() -> Result<(), String> {
    let xml = r##"<e:EPackage xmlns:e="http://www.eclipse.org/emf/2002/Ecore" xmlns:x="http://www.w3.org/2001/XMLSchema-instance" name="graph" nsURI="urn:graph" nsPrefix="g"><eClassifiers x:type="e:EClass" name="Node"><eStructuralFeatures x:type="e:EAttribute" name="name" eType="e:EDataType http://www.eclipse.org/emf/2002/Ecore#//EString"/><eStructuralFeatures x:type="e:EReference" name="links" eType="#//Node" upperBound="-1"/></eClassifiers></e:EPackage>"##;
    let original = intersect_model_acceptance::migration_fixture::original_schema_fixture(xml);
    let report = migrate_ecore(
        DefinitionSource::new("memory:graph.ecore", SourceKind::Ecore, xml),
        "memory:graph.rcore",
    )
    .map_err(|e| e.to_string())?;
    let target = report.converted.ok_or("Conversion failed")?;
    let bundle = DefinitionBundle {
        version: 1,
        sources: vec![target.clone()],
        namespaces: vec![],
    };
    let converted = compile_structure(&bundle, &target.id)
        .map_err(|e| format!("{e:?}"))?
        .schema;
    let external = [Resource {uri:"objects.xmi".into(),bytes:br##"<xmi:XMI xmlns:xmi="http://www.omg.org/XMI" xmlns:g="urn:graph"><g:Node xmi:id="a" name="alpha &amp; beta" links="#b"/><g:Node xmi:id="b"/></xmi:XMI>"##.to_vec()}];
    let model = XmiFormat.decode(&original, &external)?;
    let migrated = XmiFormat.decode(&converted, &external)?;
    assert_eq!(migrated.elements.len(), model.elements.len());
    for (actual, expected) in migrated.elements.iter().zip(&model.elements) {
        assert_eq!(actual.id, expected.id);
        assert_eq!(actual.class, expected.class);
        assert_eq!(actual.resource, expected.resource);
        assert_eq!(actual.values, expected.values);
        assert!(actual.type_identity.contains(&target.id.sha256));
        assert_ne!(actual.type_identity, expected.type_identity);
    }
    let serialized = XmiFormat.encode(&converted, &migrated)?;
    let returned = XmiFormat.decode(&converted, &serialized)?;
    assert_eq!(returned.elements.len(), 2);
    assert_eq!(returned.elements[0].values, model.elements[0].values);
    // Intersect's exact revision stamp remains authoritative. Conversion is not
    // permission to rebind existing saved instances to a new definition revision.
    assert!(
        XmiFormat
            .decode(&converted, &XmiFormat.encode(&original, &model)?)
            .unwrap_err()
            .contains("revision mismatch")
    );
    Ok(())
}
