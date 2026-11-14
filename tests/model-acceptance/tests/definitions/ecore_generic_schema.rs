use intersect_model::definitions::{
    bundle::{DefinitionBundle, DefinitionSource, SourceKind},
    compiler::compile_structure_bundle,
    ecore::ErrorKind,
    ecore_document::validate_structure,
    operations::TypeTarget,
};
fn wrap(body: &str) -> String {
    format!(
        r#"<e:EPackage xmlns:e="http://www.eclipse.org/emf/2002/Ecore" xmlns:x="http://www.w3.org/2001/XMLSchema-instance" name="g" nsURI="urn:g">{body}</e:EPackage>"#
    )
}
#[test]
fn concrete_generic_operations_defaults_and_supertypes_use_existing_schema_services() {
    let text = wrap(
        r##"<eClassifiers x:type="e:EClass" name="Base"><eStructuralFeatures x:type="e:EAttribute" name="name" defaultValueLiteral="default"><eGenericType eClassifier="e:EDataType http://www.eclipse.org/emf/2002/Ecore#//EString"/></eStructuralFeatures></eClassifiers><eClassifiers x:type="e:EClass" name="Child"><eGenericSuperTypes eClassifier="#//Base"/><eOperations name="lookup"><eGenericType eClassifier="#//Base"/><eParameters name="input"><eGenericType eClassifier="http://www.eclipse.org/emf/2002/Ecore#//EString"/></eParameters><eGenericExceptions eClassifier="http://www.eclipse.org/emf/2002/Ecore#//EInvocationTargetException"/></eOperations><eOperations name="nothing"/></eClassifiers>"##,
    );
    let schema =
        intersect_model_acceptance::migration_fixture::compile_migrated_schema_fixture(&text)
            .unwrap();
    assert!(schema.conforms("Child", "Base"));
    assert_eq!(schema.features("Child").unwrap()["name"].default, "default");
    let ops = schema.effective_operations("Child").unwrap();
    assert_eq!(ops.len(), 2);
    let lookup = &schema.classes["Child"].operations[0];
    assert_eq!(lookup.result.target, Some(TypeTarget::Class("Base".into())));
    assert!(lookup.source_payload.contains("throws"));
    assert!(
        schema.classes["Child"].operations[1]
            .result
            .target
            .is_none()
    );
    assert_eq!(schema.generic_signatures.as_ref().unwrap().types.len(), 5);
}
#[test]
fn applied_and_formal_generics_are_checked_before_explicit_e08_runtime_rejection() {
    let formal = wrap(
        r##"<eClassifiers x:type="e:EClass" name="Box"><eTypeParameters name="T"/></eClassifiers>"##,
    );
    let error = validate_structure(&formal).unwrap_err();
    assert_eq!(error.kind, ErrorKind::Unsupported);
    assert!(error.message.contains("E08"));
    let applied = wrap(
        r##"<eClassifiers x:type="e:EClass" name="Box"><eStructuralFeatures x:type="e:EAttribute" name="value"><eGenericType eClassifier="http://www.eclipse.org/emf/2002/Ecore#//EJavaClass"><eTypeArguments/></eGenericType></eStructuralFeatures></eClassifiers>"##,
    );
    let error = validate_structure(&applied).unwrap_err();
    assert_eq!(error.kind, ErrorKind::Unsupported);
    assert!(error.message.contains("E08"));
    let malformed = applied.replace("<eTypeArguments/>", "<eTypeArguments/><eTypeArguments/>");
    let error = validate_structure(&malformed).unwrap_err();
    assert_eq!(error.kind, ErrorKind::InvalidDefinition);
    assert!(error.message.contains("arity"));
}
#[test]
fn generic_projection_preserves_cross_resource_xmi_and_nested_identities() {
    let base = DefinitionSource::new(
        "https://example.test/base.ecore",
        SourceKind::Ecore,
        r#"<m:XMI xmlns:m="http://www.omg.org/XMI" xmlns:e="http://www.eclipse.org/emf/2002/Ecore" xmlns:x="http://www.w3.org/2001/XMLSchema-instance"><e:EPackage name="unused" nsURI="urn:empty"/><e:EPackage name="base" nsURI="urn:base"><eSubpackages name="nested" nsURI="urn:nested"><eClassifiers x:type="e:EClass" name="Base"/></eSubpackages></e:EPackage></m:XMI>"#,
    );
    let mut usage = DefinitionSource::new(
        "https://example.test/use.ecore",
        SourceKind::Ecore,
        wrap(
            r##"<eClassifiers x:type="e:EClass" name="Child"><eGenericSuperTypes eClassifier="base.ecore#/1/nested/Base"/><eStructuralFeatures x:type="e:EReference" name="ref"><eGenericType eClassifier="base.ecore#/1/nested/Base"/></eStructuralFeatures></eClassifiers>"##,
        ),
    );
    usage.dependencies.push(base.id.clone());
    let unrelated=DefinitionSource::new("memory:unrelated",SourceKind::Ecore,wrap(r##"<eClassifiers x:type="e:EClass" name="Bad"><eTypeParameters name="T"/><eTypeParameters name="T"/></eClassifiers>"##).replace("urn:g","urn:unrelated"));
    let bundle = DefinitionBundle {
        version: 1,
        sources: vec![unrelated, usage.clone(), base.clone()],
        namespaces: vec![],
    };
    let targets = bundle
        .sources
        .iter()
        .map(|s| (s.id.clone(), format!("{}.rcore", s.id.uri)))
        .collect();
    assert!(intersect_model::migration::migrate_ecore_bundle(bundle.clone(), &targets).is_err());
    // The complete migration refuses the malformed unused resource. Select the
    // intended input bundle explicitly before converting its dependency closure.
    let mut bundle = bundle;
    bundle.sources.remove(0);
    let report = intersect_model_acceptance::migration_fixture::migrate_resources_fixture(bundle);
    let base =
        intersect_model_acceptance::migration_fixture::converted_source_fixture(&report, &base.id);
    let usage =
        intersect_model_acceptance::migration_fixture::converted_source_fixture(&report, &usage.id);
    let bundle = report.converted.unwrap();
    let schema = compile_structure_bundle(&bundle, &usage.id, "urn:g").unwrap();
    let key = schema.class_key(&base.id, "urn:nested", "Base").unwrap();
    assert!(schema.conforms("Child", key));
    assert_eq!(schema.classes["Child"].features["ref"].target, key);
    let inventory = schema.generic_signatures.unwrap();
    assert_eq!(inventory.sources.len(), 2);
    assert_eq!(inventory.types[0].raw_type.source, base.id);
    assert_eq!(inventory.types[0].raw_type.path, vec![1, 0, 0]);
}
#[test]
fn legacy_schemas_omit_generic_metadata_and_conflicting_inputs_reject() {
    let legacy = include_str!("../../../../fixtures/library/library.ecore");
    let schema =
        intersect_model_acceptance::migration_fixture::compile_migrated_schema_fixture(legacy)
            .unwrap();
    assert!(schema.generic_signatures.is_none());
    assert!(
        serde_json::to_value(schema)
            .unwrap()
            .get("generic_signatures")
            .is_none()
    );
    let text = wrap(
        r##"<eClassifiers x:type="e:EClass" name="A"/><eClassifiers x:type="e:EClass" name="B"><eStructuralFeatures x:type="e:EReference" name="r" eType="#//B"><eGenericType eClassifier="#//A"/></eStructuralFeatures></eClassifiers>"##,
    );
    let error = validate_structure(&text).unwrap_err();
    assert_eq!(error.kind, ErrorKind::InvalidDefinition);
    assert!(error.message.contains("disagrees"));
}

#[test]
fn concrete_generic_declared_datatypes_and_enums_preserve_nominal_defaults() {
    let text = wrap(
        r##"<eClassifiers x:type="e:EDataType" name="Count" instanceClassName="int"/><eClassifiers x:type="e:EEnum" name="Mode"><eLiterals name="FIRST" value="7"/><eLiterals name="ALIAS" literal="alias" value="7"/></eClassifiers><eClassifiers x:type="e:EClass" name="Holder"><eStructuralFeatures x:type="e:EAttribute" name="count" defaultValueLiteral="3"><eGenericType eClassifier="#//Count"/></eStructuralFeatures><eStructuralFeatures x:type="e:EAttribute" name="mode" defaultValueLiteral="alias"><eGenericType eClassifier="#//Mode"/></eStructuralFeatures></eClassifiers>"##,
    );
    let schema =
        intersect_model_acceptance::migration_fixture::compile_migrated_schema_fixture(&text)
            .unwrap();
    let fields = &schema.classes["Holder"].features;
    for name in ["count", "mode"] {
        assert!(fields[name].datatype.is_some());
        schema
            .validate_attribute(&fields[name], &fields[name].default)
            .unwrap();
    }
    assert!(
        schema
            .validate_attribute(&fields["count"], &fields["mode"].default)
            .is_err()
    );
    let retained = &schema.generic_signatures.as_ref().unwrap().types;
    for name in ["count", "mode"] {
        assert!(
            retained
                .iter()
                .any(|signature| Some(&signature.raw_type) == fields[name].datatype.as_ref())
        );
    }
}
