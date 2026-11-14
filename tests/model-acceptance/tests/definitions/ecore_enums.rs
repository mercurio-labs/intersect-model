use intersect_model::definitions::{
    bundle::{DefinitionBundle, DefinitionSource, SourceKind},
    compiler::import_enum_bundle,
    enums::{EnumValue, Enumeration},
    resources::DeclarationId,
};

#[test]
fn enum_numeric_values_use_shared_unicode_eint_lexical_rules() {
    let (bundle, id) = input(
        "",
        "<eLiterals name='Minimum' value='-２１４７４８３６４８'/><eLiterals name='Mixed' value='+٠٠7'/>",
    );
    let enumeration = import_enum_bundle(&bundle, &id).unwrap();
    assert_eq!(enumeration.literals()[0].value, i32::MIN);
    assert_eq!(enumeration.literals()[1].value, 7);
    for invalid in ["２１４７４８３６４８", "𝟙", " 1", "1.0"] {
        let (bundle, id) = input("", &format!("<eLiterals name='Bad' value='{invalid}'/>"));
        assert!(import_enum_bundle(&bundle, &id).is_err());
    }
}

fn input(attributes: &str, body: &str) -> (DefinitionBundle, DeclarationId) {
    let source = DefinitionSource::new(
        "file:///enums.ecore",
        SourceKind::Ecore,
        format!(
            "<e:EPackage xmlns:e='http://www.eclipse.org/emf/2002/Ecore' xmlns:xsi='http://www.w3.org/2001/XMLSchema-instance' name='root' nsURI='urn:root'><eSubpackages name='nested' nsURI='urn:nested'><eClassifiers xsi:type='e:EEnum' name='Mode' {attributes}>{body}</eClassifiers></eSubpackages></e:EPackage>"
        ),
    );
    let id = DeclarationId {
        source: source.id.clone(),
        path: vec![0, 0],
    };
    (
        DefinitionBundle {
            version: 1,
            sources: vec![source],
            namespaces: vec![],
        },
        id,
    )
}

#[test]
fn aliases_default_and_three_lookup_domains_remain_distinct() {
    let (bundle, id) = input(
        "",
        "<eLiterals name='First' value='7' literal='first'/><eLiterals name='Alias' value='7' literal='alias'/><eLiterals name='Zero'/>",
    );
    let enumeration = import_enum_bundle(&bundle, &id).unwrap();
    assert_eq!(enumeration.by_value(7).unwrap().name, "First");
    assert_eq!(enumeration.by_name("Alias").unwrap().value, 7);
    assert_eq!(enumeration.by_literal("alias").unwrap().name, "Alias");
    assert_eq!(
        enumeration.default_value(),
        Some(enumeration.parse("first").unwrap())
    );
    assert_eq!(enumeration.by_value(0).unwrap().name, "Zero");
    for invalid in ["First", "Alias", "7", " first", "FIRST"] {
        assert!(enumeration.parse(invalid).is_err(), "{invalid}");
    }
    let alias = enumeration.parse("alias").unwrap();
    let wire = serde_json::to_string(&alias).unwrap();
    let restored: EnumValue = serde_json::from_str(&wire).unwrap();
    assert_eq!(enumeration.format(&restored).unwrap(), "alias");
    assert_ne!(restored, enumeration.default_value().unwrap());
    let restored_definition: Enumeration =
        serde_json::from_str(&serde_json::to_string(&enumeration).unwrap()).unwrap();
    assert_eq!(restored_definition, enumeration);
    assert_eq!(restored_definition.by_value(7).unwrap().name, "First");
}

#[test]
fn exact_xml_ordinals_metadata_and_lexical_spelling_survive_import() {
    let body = "<eAnnotations source='unexecuted'><details key='x' value='y'/></eAnnotations><eLiterals name='Inherited'/><eLiterals name='Empty' literal=''/><eLiterals name='Unicode' literal='λ &amp; 🙂'/><eLiterals name='Numeric' literal='123'/><eLiterals name='Space' literal=' '/>";
    let (bundle, id) = input("instanceClassName='example.Mode'", body);
    let enumeration = import_enum_bundle(&bundle, &id).unwrap();
    assert_eq!(enumeration.literals()[0].declaration.path, [0, 0, 1]);
    assert_eq!(enumeration.literals()[0].literal, None);
    assert_eq!(enumeration.literals()[1].literal.as_deref(), Some(""));
    for spelling in ["Inherited", "", "λ & 🙂", "123", " "] {
        assert_eq!(
            enumeration
                .format(&enumeration.parse(spelling).unwrap())
                .unwrap(),
            spelling
        );
    }
    let payload = enumeration.source_payload().unwrap();
    assert!(payload.contains(body));
    assert!(payload.contains("instanceClassName='example.Mode'"));
    assert!(payload.starts_with("<eClassifiers"));
    assert!(payload.ends_with("</eClassifiers>"));
}

#[test]
fn empty_and_nonserializable_enums_keep_default_and_lookup_contracts() {
    let (bundle, id) = input("", "");
    let empty = import_enum_bundle(&bundle, &id).unwrap();
    assert_eq!(empty.default_value(), None);
    assert!(empty.parse("").is_err());
    let (bundle, id) = input("serializable='false'", "<eLiterals name='One'/>");
    let enumeration = import_enum_bundle(&bundle, &id).unwrap();
    let value = enumeration.default_value().unwrap();
    assert_eq!(enumeration.validate_value(&value).unwrap().name, "One");
    assert!(enumeration.parse("One").is_err());
    assert!(enumeration.format(&value).is_err());
    assert_eq!(
        enumeration
            .value_by_id(&enumeration.literals()[0].declaration)
            .unwrap(),
        value
    );
}

#[test]
fn duplicates_and_integer_overflow_reject_but_numeric_aliases_and_case_do_not() {
    for body in [
        "<eLiterals name='X'/><eLiterals name='X' literal='other'/>",
        "<eLiterals name='X'/><eLiterals name='Y' literal='X'/>",
        "<eLiterals name='X' value='2147483648'/>",
        "<eLiterals name='X' value='-2147483649'/>",
        "<eLiterals name='X' value='1.0'/>",
        "<eLiterals name='X' value=' 1'/>",
        "<eLiterals name='X' value=''/>",
    ] {
        let (bundle, id) = input("", body);
        assert!(import_enum_bundle(&bundle, &id).is_err(), "{body}");
    }
    let (bundle, id) = input(
        "",
        "<eLiterals name='X' value='-2147483648'/><eLiterals name='x' value='2147483647'/><eLiterals name='X_' value='2147483647'/>",
    );
    let enumeration = import_enum_bundle(&bundle, &id).unwrap();
    assert_eq!(enumeration.by_value(i32::MIN).unwrap().name, "X");
    assert_eq!(enumeration.by_value(i32::MAX).unwrap().name, "x");
}

#[test]
fn enum_and_literal_identity_reject_wrong_owner_and_edited_revision() {
    let (bundle, id) = input("", "<eLiterals name='X'/><eLiterals name='Y'/>");
    let enumeration = import_enum_bundle(&bundle, &id).unwrap();
    let (edited, edited_id) = input(" ", "<eLiterals name='X'/><eLiterals name='Y'/>");
    let changed = import_enum_bundle(&edited, &edited_id).unwrap();
    assert!(
        changed
            .validate_value(&enumeration.default_value().unwrap())
            .is_err()
    );
    let mut forged_literal = enumeration.literals()[0].declaration.clone();
    forged_literal.path = vec![0, 1, 0];
    assert!(
        enumeration
            .validate_value(&EnumValue::Enum {
                enumeration: id.clone(),
                literal: forged_literal.clone()
            })
            .is_err()
    );
    let mut literals = enumeration.literals().to_vec();
    literals[0].declaration = forged_literal;
    assert!(Enumeration::new(id.clone(), "Mode".into(), true, literals).is_err());
    let mut reversed = enumeration.literals().to_vec();
    reversed.reverse();
    assert!(Enumeration::new(id, "Mode".into(), true, reversed).is_err());
    assert!(import_enum_bundle(&bundle, &edited_id).is_err());
}

#[test]
fn deserialization_revalidates_inventory_and_does_not_accept_cached_indexes() {
    let (bundle, id) = input("", "<eLiterals name='X'/><eLiterals name='Y'/>");
    let enumeration = import_enum_bundle(&bundle, &id).unwrap();
    let original = serde_json::to_value(&enumeration).unwrap();
    let mut duplicate = original.clone();
    duplicate["literals"][1]["name"] = serde_json::json!("X");
    assert!(serde_json::from_value::<Enumeration>(duplicate).is_err());
    let mut reversed = original.clone();
    reversed["literals"].as_array_mut().unwrap().reverse();
    assert!(serde_json::from_value::<Enumeration>(reversed).is_err());
    let mut injected = original;
    injected["by_value"] = serde_json::json!({"0": 1});
    assert!(serde_json::from_value::<Enumeration>(injected).is_err());
}

#[test]
fn resource_order_and_namespace_prefixes_do_not_change_selected_enum() {
    let (mut bundle, id) = input("", "<eLiterals name='X'/>");
    let expected = import_enum_bundle(&bundle, &id).unwrap();
    let other = DefinitionSource::new(
        "file:///other.ecore",
        SourceKind::Ecore,
        bundle.sources[0]
            .text
            .replace("xmlns:e=", "xmlns:alias=")
            .replace("e:", "alias:"),
    );
    let other_id = DeclarationId {
        source: other.id.clone(),
        path: id.path.clone(),
    };
    bundle.sources.push(other);
    bundle.sources.reverse();
    assert_eq!(import_enum_bundle(&bundle, &id).unwrap(), expected);
    let renamed = import_enum_bundle(&bundle, &other_id).unwrap();
    assert_eq!(renamed.by_name("X").unwrap().value, 0);
    assert_ne!(renamed.default_value(), expected.default_value());
    assert!(renamed.source_payload().unwrap().contains("alias:EEnum"));
}

#[test]
fn unsupported_enum_extensions_are_explicit_and_not_silently_lowered() {
    for (attributes, body) in [
        ("unknown='true'", ""),
        ("", "<eTypeParameters name='T'/>"),
        ("", "<eLiterals name='X' xsi:type='e:EClass'/>"),
        ("", "<eLiterals name='X'><unexpected/></eLiterals>"),
    ] {
        let (bundle, id) = input(attributes, body);
        assert!(
            import_enum_bundle(&bundle, &id).is_err(),
            "{attributes} {body}"
        );
    }
}

#[test]
fn executable_schema_resolves_nested_external_enum_defaults_by_exact_identity() {
    use intersect_model::definitions::compiler::compile_structure_bundle;
    let (mut bundle, enum_id) = input(
        "",
        "<eLiterals name='X' value='9'/><eLiterals name='Y' value='9'/>",
    );
    let mut entry = DefinitionSource::new(
        "file:///model.ecore",
        SourceKind::Ecore,
        "<e:EPackage xmlns:e='http://www.eclipse.org/emf/2002/Ecore' xmlns:xsi='http://www.w3.org/2001/XMLSchema-instance' name='model' nsURI='urn:model'><eClassifiers xsi:type='e:EClass' name='Item'><eStructuralFeatures xsi:type='e:EAttribute' name='mode' defaultValueLiteral='Y'><eType xsi:type='e:EEnum' href='enums.ecore#//nested/Mode'/></eStructuralFeatures></eClassifiers></e:EPackage>",
    );
    entry.dependencies.push(enum_id.source.clone());
    let entry_id = entry.id.clone();
    bundle.sources.push(entry);
    let report = intersect_model_acceptance::migration_fixture::migrate_resources_fixture(bundle);
    let enum_id = report
        .reports
        .iter()
        .flat_map(|row| &row.correspondence)
        .find(|row| row.original == enum_id)
        .unwrap()
        .converted
        .clone();
    let entry_id =
        intersect_model_acceptance::migration_fixture::converted_source_fixture(&report, &entry_id)
            .id;
    let mut bundle = report.converted.unwrap();
    let schema = compile_structure_bundle(&bundle, &entry_id, "urn:model").unwrap();
    let field = &schema.classes["Item"].features["mode"];
    assert_eq!(field.datatype.as_ref(), Some(&enum_id));
    let enumeration = &schema.enumerations[&field.target];
    assert_eq!(enumeration.name(), "Mode");
    let value: EnumValue = serde_json::from_value(field.default.clone()).unwrap();
    assert_eq!(enumeration.format(&value).unwrap(), "Y");
    schema.validate_attribute(field, &field.default).unwrap();
    bundle.sources.reverse();
    assert_eq!(
        compile_structure_bundle(&bundle, &entry_id, "urn:model").unwrap(),
        schema
    );
    let only_enums = compile_structure_bundle(&bundle, &enum_id.source, "urn:nested").unwrap();
    assert!(only_enums.classes.is_empty());
    assert_eq!(only_enums.enumerations["Mode"].declaration(), &enum_id);
}
