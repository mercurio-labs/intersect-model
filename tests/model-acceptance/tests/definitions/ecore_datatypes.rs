use intersect_model::definitions::{
    bundle::{DefinitionBundle, DefinitionSource, SourceKind},
    compiler::import_datatype_bundle,
    datatypes::{Binding, Datatype, DatatypeValue},
    resources::DeclarationId,
    scalars::Builtin,
};
use serde_json::{Value, json};

#[test]
fn rcore_behavior_imported_datatype_parameters_cannot_cross_same_uri_revisions() {
    use intersect_model::definitions::rcore_behavior::checking::check;
    let xml = "<e:EPackage xmlns:e='http://www.eclipse.org/emf/2002/Ecore' xmlns:xsi='http://www.w3.org/2001/XMLSchema-instance' name='types' nsURI='urn:types'><eClassifiers xsi:type='e:EDataType' name='Counter' instanceClassName='long'/></e:EPackage>";
    let first = DefinitionSource::new("memory:types.ecore", SourceKind::Ecore, xml);
    let second = DefinitionSource::new("memory:types.ecore", SourceKind::Ecore, format!("{xml}\n"));
    let mut helper = DefinitionSource::new(
        "memory:helper-behavior.rcore",
        SourceKind::RcoreBehavior,
        "rcore 1; package helper version \"1\"; import model \"urn:types\" as m; pub fn identity(value: m.Counter) -> m.Counter = value;",
    );
    helper.dependencies.push(first.id.clone());
    let mut entry = DefinitionSource::new(
        "memory:entry-behavior.rcore",
        SourceKind::RcoreBehavior,
        "rcore 1; package entry version \"1\"; import model \"urn:types\" as m; import package helper version \"1\" as h; fn f(value: m.Counter) -> m.Counter = h.identity(value);",
    );
    entry.dependencies = vec![second.id.clone(), helper.id.clone()];
    let bundle = DefinitionBundle {
        version: 1,
        sources: vec![first.clone(), second, helper.clone(), entry.clone()],
        namespaces: vec![],
    };
    let errors = check(&bundle, &entry.id).unwrap_err();
    assert!(
        errors
            .iter()
            .any(|error| error.message.contains("Type mismatch")),
        "{errors:?}"
    );
    entry.dependencies = vec![first.id.clone(), helper.id.clone()];
    let mut bundle = DefinitionBundle {
        version: 1,
        sources: vec![first, helper, entry.clone()],
        namespaces: vec![],
    };
    check(&bundle, &entry.id).unwrap();
    bundle.sources.reverse();
    check(&bundle, &entry.id).unwrap();
}

#[test]
fn rcore_behavior_datatype_annotations_features_and_functions_are_nominal() {
    use intersect_model::definitions::rcore_behavior::checking::check;
    let model = DefinitionSource::new(
        "memory:types.ecore",
        SourceKind::Ecore,
        "<e:EPackage xmlns:e='http://www.eclipse.org/emf/2002/Ecore' xmlns:xsi='http://www.w3.org/2001/XMLSchema-instance' name='test' nsURI='urn:types'><eClassifiers xsi:type='e:EDataType' name='Counter' instanceClassName='long'/><eClassifiers xsi:type='e:EDataType' name='Other' instanceClassName='long'/><eClassifiers xsi:type='e:EClass' name='Item'><eStructuralFeatures xsi:type='e:EAttribute' name='count' lowerBound='1' eType='#//Counter'/><eStructuralFeatures xsi:type='e:EAttribute' name='optional' eType='#//Counter'/><eStructuralFeatures xsi:type='e:EAttribute' name='derived' lowerBound='1' derived='true' eType='#//Counter'/></eClassifiers></e:EPackage>",
    );
    let (model, _) = intersect_model_acceptance::migration_fixture::migrate_source_fixture(model);
    for (body, valid) in [
        ("fn f(x: m.Counter) -> m.Counter = x;", true),
        ("fn f(item: m.Item) -> m.Counter = item.count;", true),
        ("fn f(item: m.Item?) -> m.Counter? = item?.count;", true),
        (
            "fn f(item: m.Item) -> m.Counter = item.optional ?? item.count;",
            true,
        ),
        ("fn f(a: m.Counter, b: m.Counter) -> Bool = a == b;", true),
        (
            "fn f(x: m.Counter) -> m.Counter = { let get: () -> m.Counter = || x; get() };",
            true,
        ),
        (
            "fn f(x: Result<m.Counter?, String>, fallback: m.Counter) -> m.Counter = match x { Ok(Some(value)) => value, Ok(none) => fallback, Err(_) => fallback };",
            true,
        ),
        (
            "derive m.Item.derived(self) -> m.Counter = self.count;",
            true,
        ),
        ("fn f(x: m.Counter) -> Int64 = x;", false),
        ("fn f(x: Int64) -> m.Counter = x;", false),
        ("fn f(x: m.Counter) -> m.Other = x;", false),
        ("fn f(a: m.Counter, b: m.Other) -> Bool = a == b;", false),
        ("fn f(x: m.Counter) -> m.Counter = x + x;", false),
        ("fn f() -> m.Counter = 1;", false),
        ("fn f(item: m.Item) -> m.Counter = item.optional;", false),
    ] {
        let mut source = DefinitionSource::new(
            "memory:types-behavior.rcore",
            SourceKind::RcoreBehavior,
            format!(
                "rcore 1; package types version \"1\"; import model \"urn:types\" as m; {body}"
            ),
        );
        source.dependencies.push(model.id.clone());
        let result = check(
            &DefinitionBundle {
                version: 1,
                sources: vec![model.clone(), source.clone()],
                namespaces: vec![],
            },
            &source.id,
        );
        assert_eq!(result.is_ok(), valid, "{body}: {result:?}");
    }
}

#[test]
fn schema_defaults_and_operation_overloads_keep_nominal_scalar_identity() {
    use intersect_model::definitions::operations::{ParameterType, TypeTarget};
    let text = "<e:EPackage xmlns:e='http://www.eclipse.org/emf/2002/Ecore' xmlns:xsi='http://www.w3.org/2001/XMLSchema-instance' name='test' nsURI='urn:test'><eClassifiers xsi:type='e:EDataType' name='EInt' instanceClassName='long'/><eClassifiers xsi:type='e:EClass' name='Base'><eStructuralFeatures xsi:type='e:EAttribute' name='count' eType='#//EInt' defaultValueLiteral='9007199254740993'/><eOperations name='convert' eType='#//EInt'><eParameters name='value' eType='#//EInt'/></eOperations><eOperations name='convert'><eParameters name='value' eType='e:EDataType http://www.eclipse.org/emf/2002/Ecore#//EInt'/></eOperations></eClassifiers><eClassifiers xsi:type='e:EClass' name='Child' eSuperTypes='#//Base'><eOperations name='convert' eType='#//EInt'><eParameters name='value' eType='#//EInt'/></eOperations></eClassifiers></e:EPackage>";
    let schema =
        intersect_model_acceptance::migration_fixture::compile_migrated_schema_fixture(text)
            .unwrap();
    let ty = &schema.datatypes["EInt"];
    let feature = &schema.classes["Base"].features["count"];
    assert_eq!(feature.datatype.as_ref(), Some(ty.declaration()));
    assert_eq!(
        feature.default,
        serde_json::to_value(ty.parse("9007199254740993").unwrap()).unwrap()
    );
    schema
        .validate_attribute(feature, &feature.default)
        .unwrap();
    assert!(
        schema
            .validate_attribute(feature, &Builtin::ELong.parse("9007199254740993").unwrap())
            .is_err()
    );
    let operations = schema.resolved_operations("Child").unwrap();
    assert_eq!(operations.len(), 2);
    let selected = operations
        .iter()
        .find(|entry| entry.selected.owner == "Child")
        .unwrap();
    assert_eq!(selected.overridden.len(), 1);
    assert_eq!(
        selected.selected.operation.result.target,
        Some(TypeTarget::Datatype("EInt".into()))
    );
    assert_eq!(
        selected.signature.parameters,
        [ParameterType::Classifier {
            declaration: ty.declaration().clone()
        }]
    );
    assert!(
        schema
            .operation_parameter_type(&TypeTarget::Class("EInt".into()))
            .is_err()
    );
    assert!(
        schema
            .operation_parameter_type(&TypeTarget::Enumeration("EInt".into()))
            .is_err()
    );
    let restored: intersect_model::definitions::schema::Schema =
        serde_json::from_value(serde_json::to_value(&schema).unwrap()).unwrap();
    assert_eq!(restored, schema);
    for invalid in [
        text.replace("instanceClassName='long'", "instanceClassName='unknown'"),
        text.replace("instanceClassName='long'", "instanceClassName='byte'"),
        text.replace(
            "instanceClassName='long'",
            "instanceClassName='long' serializable='false'",
        ),
        text.replace("xsi:type='e:EAttribute'", "xsi:type='e:EReference'"),
        text.replace(
            "name='Child' eSuperTypes='#//Base'",
            "name='Child' eSuperTypes='#//EInt'",
        ),
    ] {
        assert!(
            intersect_model_acceptance::migration_fixture::compile_migrated_schema_fixture(
                &invalid
            )
            .is_err()
        );
    }
}

#[test]
fn external_datatype_resolution_preserves_exact_ids_under_source_permutation() {
    use intersect_model::definitions::compiler::compile_structure_bundle;
    let (mut bundle, id) = input("instanceClassName='long'", "");
    let mut entry = DefinitionSource::new(
        "file:///entry.ecore",
        SourceKind::Ecore,
        "<e:EPackage xmlns:e='http://www.eclipse.org/emf/2002/Ecore' xmlns:xsi='http://www.w3.org/2001/XMLSchema-instance' name='entry' nsURI='urn:entry'><eClassifiers xsi:type='e:EClass' name='Item'><eStructuralFeatures xsi:type='e:EAttribute' name='value' eType='datatypes.ecore#//nested/EInt'/><eOperations name='value' eType='datatypes.ecore#//nested/EInt'><eParameters name='v' eType='datatypes.ecore#//nested/EInt'/></eOperations></eClassifiers></e:EPackage>",
    );
    entry.dependencies.push(id.source.clone());
    bundle.sources.push(entry.clone());
    let report = intersect_model_acceptance::migration_fixture::migrate_resources_fixture(bundle);
    let id = report
        .reports
        .iter()
        .flat_map(|row| &row.correspondence)
        .find(|row| row.original == id)
        .unwrap()
        .converted
        .clone();
    let entry =
        intersect_model_acceptance::migration_fixture::converted_source_fixture(&report, &entry.id);
    let mut bundle = report.converted.unwrap();
    let first = compile_structure_bundle(&bundle, &entry.id, "urn:entry").unwrap();
    bundle.sources.reverse();
    let second = compile_structure_bundle(&bundle, &entry.id, "urn:entry").unwrap();
    assert_eq!(first, second);
    let field = &first.classes["Item"].features["value"];
    assert_eq!(field.datatype.as_ref(), Some(&id));
    assert_eq!(first.datatypes[&field.target].declaration(), &id);
    first.validate_attribute(field, &field.default).unwrap();
    assert_eq!(
        first.resolved_operations("Item").unwrap()[0]
            .signature
            .parameters,
        [intersect_model::definitions::operations::ParameterType::Classifier { declaration: id }]
    );
}

#[test]
fn pinned_ecore_scalar_declarations_select_the_expected_native_domains() {
    let source = DefinitionSource::new(
        "file:///upstream/Ecore.ecore",
        SourceKind::Ecore,
        include_str!(
            "../../../../fixtures/definition-conformance/upstream/ecore/model/Ecore.ecore"
        ),
    );
    let bundle = DefinitionBundle {
        version: 1,
        sources: vec![source.clone()],
        namespaces: vec![],
    };
    let resources = intersect_model::definitions::resources::ResourceSet::new(&bundle).unwrap();
    for name in [
        "EString",
        "EBoolean",
        "EByte",
        "EShort",
        "EInt",
        "ELong",
        "EFloat",
        "EDouble",
        "EBooleanObject",
        "EByteObject",
        "EShortObject",
        "EIntegerObject",
        "ELongObject",
        "EFloatObject",
        "EDoubleObject",
    ] {
        let id = resources
            .resolve(&source.id, &format!("#//{name}"))
            .unwrap();
        let ty = import_datatype_bundle(&bundle, &id).unwrap();
        assert_eq!(ty.codec().unwrap(), Builtin::named(name).unwrap());
        assert_eq!(ty.name(), name);
        assert!(ty.source_payload().unwrap().contains("eAnnotations"));
    }
    let id = resources.resolve(&source.id, "#//EJavaObject").unwrap();
    assert!(matches!(
        import_datatype_bundle(&bundle, &id).unwrap().binding(),
        Binding::Unsupported { .. }
    ));
}

fn input(attributes: &str, body: &str) -> (DefinitionBundle, DeclarationId) {
    let source = DefinitionSource::new(
        "file:///datatypes.ecore",
        SourceKind::Ecore,
        format!(
            "<e:EPackage xmlns:e='http://www.eclipse.org/emf/2002/Ecore' xmlns:xsi='http://www.w3.org/2001/XMLSchema-instance' name='root' nsURI='urn:root'><eSubpackages name='nested' nsURI='urn:nested'><eClassifiers xsi:type='e:EDataType' name='EInt' {attributes}>{body}</eClassifiers></eSubpackages></e:EPackage>"
        ),
    );
    let declaration = DeclarationId {
        source: source.id.clone(),
        path: vec![0, 0],
    };
    (
        DefinitionBundle {
            version: 1,
            sources: vec![source],
            namespaces: vec![],
        },
        declaration,
    )
}

#[test]
fn every_exact_native_mapping_shares_codec_defaults_and_roundtrips() {
    for (name, codec, text) in [
        ("java.lang.String", Builtin::EString, "λ & 🙂"),
        ("boolean", Builtin::EBoolean, "falſe"),
        ("byte", Builtin::EByte, "-128"),
        ("short", Builtin::EShort, "32767"),
        ("int", Builtin::EInt, "-2147483648"),
        ("long", Builtin::ELong, "9223372036854775807"),
        ("float", Builtin::EFloat, "-0x1p-150"),
        ("double", Builtin::EDouble, "NaN"),
        ("java.lang.Boolean", Builtin::EBooleanObject, "true"),
        ("java.lang.Byte", Builtin::EByteObject, "127"),
        ("java.lang.Short", Builtin::EShortObject, "-32768"),
        ("java.lang.Integer", Builtin::EIntegerObject, "2147483647"),
        ("java.lang.Long", Builtin::ELongObject, "9007199254740993"),
        ("java.lang.Float", Builtin::EFloatObject, "Infinity"),
        ("java.lang.Double", Builtin::EDoubleObject, "0x1p-1074"),
    ] {
        for attributes in [
            format!("instanceClassName='{name}'"),
            format!("instanceTypeName='{name}'"),
            format!("instanceClassName='{name}' instanceTypeName='{name}'"),
        ] {
            let (bundle, id) = input(&attributes, "");
            let ty = import_datatype_bundle(&bundle, &id).unwrap();
            assert_eq!(ty.name(), "EInt"); // The classifier's name never chooses the codec.
            assert_eq!(ty.binding(), &Binding::Scalar { codec });
            let value = ty.parse(text).unwrap();
            assert_eq!(
                ty.validate_value(&value).unwrap(),
                &codec.parse(text).unwrap()
            );
            assert_eq!(ty.parse(&ty.format(&value).unwrap()).unwrap(), value);
            let restored: Datatype =
                serde_json::from_value(serde_json::to_value(&ty).unwrap()).unwrap();
            assert_eq!(restored, ty);
            let restored_value: DatatypeValue =
                serde_json::from_value(serde_json::to_value(&value).unwrap()).unwrap();
            assert_eq!(
                restored.format(&restored_value).unwrap(),
                ty.format(&value).unwrap()
            );
            let default = ty.default_value().unwrap();
            if codec.nullable() {
                assert_eq!(default, Value::Null);
            } else {
                let default: DatatypeValue = serde_json::from_value(default).unwrap();
                assert_eq!(ty.validate_value(&default).unwrap(), &codec.default_value());
            }
        }
    }
}

#[test]
fn unknown_and_conflicting_bindings_are_retained_without_execution() {
    for attributes in [
        "",
        "instanceClassName='Long'",
        "instanceClassName='long '",
        "instanceClassName='example.Counter'",
        "instanceClassName=''",
        "instanceClassName='long' instanceTypeName='java.lang.Long'",
        "instanceTypeName='java.util.List&lt;java.lang.String&gt;'",
    ] {
        let (bundle, id) = input(
            attributes,
            "<eAnnotations source='retained'><details key='key' value='value'/></eAnnotations>",
        );
        let ty = import_datatype_bundle(&bundle, &id).unwrap();
        assert!(matches!(ty.binding(), Binding::Unsupported { .. }));
        assert!(ty.source_payload().unwrap().contains(attributes));
        assert!(ty.parse("1").is_err());
        assert!(ty.default_value().is_err());
        assert!(ty.bind_value(json!(1)).is_err());
        assert!(
            ty.validate_value(&DatatypeValue::Datatype {
                datatype: id,
                value: json!(1)
            })
            .is_err()
        );
        let restored: Datatype =
            serde_json::from_value(serde_json::to_value(&ty).unwrap()).unwrap();
        assert_eq!(restored, ty);
    }
}

#[test]
fn identities_serializability_and_wire_validation_are_independent() {
    let (bundle, id) = input("instanceClassName='long' serializable='false'", "");
    let ty = import_datatype_bundle(&bundle, &id).unwrap();
    assert!(!ty.serializable());
    assert!(ty.parse("1").is_err());
    let value = ty
        .bind_value(Builtin::ELong.parse("9007199254740993").unwrap())
        .unwrap();
    assert!(ty.format(&value).is_err());
    assert!(ty.validate_value(&value).is_ok());
    assert!(ty.default_value().is_ok());
    for invalid in [
        json!(1),
        Value::Null,
        json!({"kind":"int64", "value":"01"}),
        json!({"kind":"int64", "value":"9223372036854775808"}),
    ] {
        assert!(ty.bind_value(invalid.clone()).is_err());
        assert!(
            ty.validate_value(&DatatypeValue::Datatype {
                datatype: id.clone(),
                value: invalid
            })
            .is_err()
        );
    }
    let mut foreign = id.clone();
    foreign.path.push(0);
    assert!(
        ty.validate_value(&DatatypeValue::Datatype {
            datatype: foreign,
            value: Builtin::ELong.default_value()
        })
        .is_err()
    );
    let other = DefinitionSource::new(
        id.source.uri.clone(),
        SourceKind::Ecore,
        format!("{}\n", bundle.sources[0].text),
    );
    let revised = Datatype::new(
        DeclarationId {
            source: other.id,
            path: id.path.clone(),
        },
        "EInt".into(),
        true,
        Some("long".into()),
        None,
    )
    .unwrap();
    assert!(revised.validate_value(&value).is_err());
    let mut definition = serde_json::to_value(&ty).unwrap();
    definition["binding"]["codec"] = json!("EString");
    assert!(serde_json::from_value::<Datatype>(definition).is_err());
    let mut wire = serde_json::to_value(&value).unwrap();
    wire["extra"] = json!(true);
    assert!(serde_json::from_value::<DatatypeValue>(wire).is_err());
}

#[test]
fn import_preserves_metadata_and_rejects_wrong_declarations_or_unsupported_children() {
    let annotation = "<eAnnotations source='example'><details key='x' value='y'/></eAnnotations>";
    let (bundle, id) = input(
        "instanceClassName='java.lang.String' instanceTypeName='java.lang.String'",
        annotation,
    );
    let ty = import_datatype_bundle(&bundle, &id).unwrap();
    assert_eq!(ty.declaration(), &id);
    assert_eq!(ty.instance_class_name(), Some("java.lang.String"));
    assert_eq!(ty.instance_type_name(), Some("java.lang.String"));
    assert!(ty.source_payload().unwrap().contains(annotation));
    for path in [vec![], vec![0], vec![0, 0, 0], vec![99]] {
        assert!(
            import_datatype_bundle(
                &bundle,
                &DeclarationId {
                    source: id.source.clone(),
                    path
                }
            )
            .is_err()
        );
    }
    for (attributes, body) in [
        ("serializable='maybe'", ""),
        ("unsupported='x'", ""),
        ("", "<unsupported/>"),
    ] {
        let (bundle, id) = input(attributes, body);
        assert!(import_datatype_bundle(&bundle, &id).is_err());
    }
}

#[test]
fn imported_generic_datatype_retains_formals_without_erasing_to_a_scalar() {
    let (bundle, id) = input(
        "instanceClassName='java.lang.String'",
        "<eTypeParameters name='T'/>",
    );
    let datatype = import_datatype_bundle(&bundle, &id).unwrap();
    assert_eq!(datatype.parameters().len(), 1);
    assert_eq!(datatype.parameters()[0].name, "T");
    assert!(
        datatype
            .codec()
            .unwrap_err()
            .contains("checked application")
    );
}
