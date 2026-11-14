use intersect_model::definitions::ecore_document::{self, Document, Edit};
use serde_json::{Value, json};

const EMPTY: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<ecore:EPackage xmlns:ecore="http://www.eclipse.org/emf/2002/Ecore" xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance" name="library" nsURI="urn:library" nsPrefix="library"/>"#;

#[test]
fn inspection_exposes_operations_without_claiming_execution() {
    let source = EMPTY.replace("/>", r##"><eClassifiers xsi:type="ecore:EClass" name="Type"><eOperations name="allGenerals" eType="#//Type" upperBound="-1"><eParameters name="includeSelf" eType="ecore:EDataType http://www.eclipse.org/emf/2002/Ecore#//EBoolean"/><eAnnotations source="urn:retained"/></eOperations></eClassifiers></ecore:EPackage>"##);
    let document = ecore_document::inspect(&source).unwrap();
    assert_eq!(document.source, source);
    assert!(document.warnings.is_empty(), "{:?}", document.warnings);
    let operation = &document.classes[0].operations[0];
    assert_eq!(operation.name, "allGenerals");
    assert_eq!(operation.parameters[0].name, "includeSelf");
    assert_eq!(operation.result.upper, -1);
    assert!(operation.source_payload.contains("urn:retained"));

    let invalid = source.replace("#//Type", "#//Missing");
    let draft = ecore_document::inspect(&invalid).unwrap();
    assert_eq!(draft.source, invalid);
    assert!(draft.classes[0].operations.is_empty());
    assert!(
        draft
            .warnings
            .iter()
            .any(|warning| warning.contains("Type.eOperations"))
    );
}

fn change(source: &str, value: Value) -> Result<Document, String> {
    ecore_document::edit(source, serde_json::from_value::<Edit>(value).unwrap())
}

fn library() -> String {
    let doc = change(EMPTY, json!({"op":"add_class","name":"Book"})).unwrap();
    let doc = change(&doc.source, json!({"op":"add_class","name":"Shelf"})).unwrap();
    let doc = change(
        &doc.source,
        json!({"op":"add_attribute","class":"Book","name":"title","type":"EString"}),
    )
    .unwrap();
    change(
        &doc.source,
        json!({"op":"add_reference","class":"Shelf","name":"books","type":"Book"}),
    )
    .unwrap()
    .source
}

fn enumeration() -> String {
    let doc = change(&library(), json!({"op":"add_enum","name":"Status"})).unwrap();
    assert!(doc.warnings.is_empty(), "{:?}", doc.warnings);
    assert_eq!(doc.enums[0].name(), "Status");
    assert!(doc.enums[0].literals().is_empty());
    change(
        &doc.source,
        json!({"op":"add_literal","enumeration":"Status","name":"DRAFT","value":-7}),
    )
    .unwrap()
    .source
}

#[test]
fn enum_authoring_preserves_numeric_aliases_and_absent_or_empty_spellings() {
    let source = enumeration();
    let doc = change(
        &source,
        json!({"op":"add_literal","enumeration":"Status","name":"EMPTY","value":-7,"literal":""}),
    )
    .unwrap();
    assert!(doc.warnings.is_empty(), "{:?}", doc.warnings);
    let literals = doc.enums[0].literals();
    assert_eq!(literals[0].literal, None);
    assert_eq!(literals[1].literal.as_deref(), Some(""));
    assert_eq!(literals[0].value, literals[1].value);
    assert!(
        change(
            &doc.source,
            json!({"op":"add_literal","enumeration":"Status","name":"OTHER","value":3,"literal":""})
        )
        .unwrap_err()
        .contains("Duplicate enum literal spelling")
    );
    assert!(
        change(
            &doc.source,
            json!({"op":"add_literal","enumeration":"Status","name":"DRAFT","value":3})
        )
        .is_err()
    );
    assert!(
        change(
            &doc.source,
            json!({"op":"set_literal","enumeration":"Status","literal":"EMPTY","spelling":"DRAFT"})
        )
        .is_err()
    );
    assert!(
        serde_json::from_value::<Edit>(
            json!({"op":"add_literal","enumeration":"Status","name":"HUGE","value":2147483648_i64})
        )
        .is_err()
    );
    let doc = change(&doc.source, json!({"op":"set_literal","enumeration":"Status","literal":"EMPTY","name":"ACTIVE","value":2147483647,"spelling":null})).unwrap();
    assert_eq!(doc.enums[0].literals()[1].spelling(), "ACTIVE");
    assert_eq!(doc.enums[0].literals()[1].value, i32::MAX);
    assert!(!doc.source.contains("literal=\"\""));
}

#[test]
fn enum_rename_updates_types_and_literal_edits_guard_explicit_defaults() {
    let doc = change(
        &enumeration(),
        json!({"op":"add_attribute","class":"Book","name":"status","type":"Status"}),
    )
    .unwrap();
    let doc = change(
        &doc.source,
        json!({"op":"set_feature","class":"Book","feature":"status","default":"DRAFT"}),
    )
    .unwrap();
    let doc = change(
        &doc.source,
        json!({"op":"set_enum","enumeration":"Status","name":"State"}),
    )
    .unwrap();
    assert!(doc.source.contains("eType=\"#//State\""));
    assert!(!doc.source.contains("#//Status"));
    assert!(doc.warnings.is_empty(), "{:?}", doc.warnings);
    assert!(
        change(
            &doc.source,
            json!({"op":"delete_enum","enumeration":"State"})
        )
        .unwrap_err()
        .contains("still references")
    );
    for edit in [
        json!({"op":"delete_literal","enumeration":"State","literal":"DRAFT"}),
        json!({"op":"set_literal","enumeration":"State","literal":"DRAFT","name":"NEW"}),
        json!({"op":"set_literal","enumeration":"State","literal":"DRAFT","spelling":"new"}),
    ] {
        assert!(
            change(&doc.source, edit)
                .unwrap_err()
                .contains("explicit attribute default")
        );
    }
    // Renaming while preserving the effective spelling is safe for defaults.
    let doc = change(&doc.source, json!({"op":"set_literal","enumeration":"State","literal":"DRAFT","name":"NEW","spelling":"DRAFT","value":-2147483648_i64})).unwrap();
    assert!(doc.warnings.is_empty(), "{:?}", doc.warnings);
    let doc = change(
        &doc.source,
        json!({"op":"delete_feature","class":"Book","feature":"status"}),
    )
    .unwrap();
    let doc = change(
        &doc.source,
        json!({"op":"delete_literal","enumeration":"State","literal":"NEW"}),
    )
    .unwrap();
    assert!(doc.enums[0].literals().is_empty());
    let doc = change(
        &doc.source,
        json!({"op":"delete_enum","enumeration":"State"}),
    )
    .unwrap();
    assert!(doc.enums.is_empty());
}

#[test]
fn enum_edits_preserve_unknown_source_and_guard_deletion() {
    let source = enumeration()
        .replace("name=\"Status\"", "name='Status' extra='café &amp; 日本語'")
        .replace("name=\"DRAFT\"", "name='DRAFT'")
        .replace(
            "<eLiterals",
            "<!-- keep literal documentation -->\n<eLiterals",
        );
    let doc = change(
        &source,
        json!({"op":"set_literal","enumeration":"Status","literal":"DRAFT","name":"NEW"}),
    )
    .unwrap();
    assert_eq!(doc.source, source.replace("name='DRAFT'", "name='NEW'"));
    let doc = change(
        &doc.source,
        json!({"op":"set_enum","enumeration":"Status","name":"State"}),
    )
    .unwrap();
    assert_eq!(
        doc.source,
        source
            .replace("name='DRAFT'", "name='NEW'")
            .replace("name='Status'", "name='State'")
    );
    assert!(
        change(
            &doc.source,
            json!({"op":"delete_enum","enumeration":"State"})
        )
        .unwrap_err()
        .contains("unsupported Ecore content")
    );
    assert!(change(&source, json!({"op":"add_enum","name":"Book"})).is_err());
    assert!(
        change(
            &source,
            json!({"op":"set_enum","enumeration":"Status","name":"Book"})
        )
        .is_err()
    );
    let annotated = enumeration().replace("name=\"DRAFT\"", "extra=\"keep\" name=\"DRAFT\"");
    assert!(
        change(
            &annotated,
            json!({"op":"delete_literal","enumeration":"Status","literal":"DRAFT"})
        )
        .is_err()
    );
}

#[test]
fn constructs_and_compiles_the_supported_profile() {
    let source = library();
    let doc = change(&source, json!({"op":"set_feature","class":"Shelf","feature":"books","lower":0,"upper":-1,"containment":true})).unwrap();
    assert!(doc.warnings.is_empty(), "{:?}", doc.warnings);
    assert_eq!(doc.classes[0].features[0].type_name, "EString");
    assert_eq!(doc.classes[1].features[0].upper, -1);
    let schema =
        intersect_model_acceptance::migration_fixture::compile_migrated_schema_fixture(&doc.source)
            .unwrap();
    assert!(schema.classes["Shelf"].features["books"].containment);
}

#[test]
fn preserves_unknown_xml_comments_quotes_and_unicode_bytes() {
    let source = library().replace("name=\"Book\"", "name='Book' extra='preserve &amp; café'")
        .replace("<eStructuralFeatures", "<!-- 日本語: unchanged -->\n  <eStructuralFeatures")
        .replace("</ecore:EPackage>", "<eAnnotations source='urn:tool'><details key='layout' value='42'/></eAnnotations></ecore:EPackage>");
    let doc = change(
        &source,
        json!({"op":"set_class","class":"Book","name":"Bücher"}),
    )
    .unwrap();
    let expected = source
        .replace("name='Book'", "name='Bücher'")
        .replace("#//Book", "#//Bücher");
    assert_eq!(doc.source, expected);
    assert!(!doc.warnings.is_empty());
    assert_eq!(ecore_document::inspect(&source).unwrap().source, source);
}

#[test]
fn unrelated_edits_preserve_external_source_identities() {
    let source = library().replace("#//Book", "other.ecore#//Book");
    let doc = change(
        &source,
        json!({"op":"set_class","class":"Book","name":"Volume"}),
    )
    .unwrap();
    assert!(doc.source.contains("other.ecore#//Book"));
    assert_eq!(doc.classes[1].features[0].type_name, "other.ecore#//Book");
    assert!(!doc.warnings.is_empty());
}

#[test]
fn class_and_feature_renames_update_inheritance_types_and_opposites() {
    let source = library();
    let source = change(&source, json!({"op":"add_class","name":"Novel"}))
        .unwrap()
        .source;
    let source = change(
        &source,
        json!({"op":"set_class","class":"Novel","super_types":["Book"]}),
    )
    .unwrap()
    .source;
    let source = change(
        &source,
        json!({"op":"add_reference","class":"Book","name":"shelf","type":"Shelf"}),
    )
    .unwrap()
    .source;
    let source = change(
        &source,
        json!({"op":"set_feature","class":"Book","feature":"shelf","opposite":"Shelf/books"}),
    )
    .unwrap()
    .source;
    let source = change(
        &source,
        json!({"op":"set_feature","class":"Shelf","feature":"books","opposite":"Book/shelf"}),
    )
    .unwrap()
    .source;
    let source = change(
        &source,
        json!({"op":"set_class","class":"Book","name":"Volume"}),
    )
    .unwrap()
    .source;
    assert!(source.contains("eSuperTypes=\"#//Volume\""));
    assert!(source.contains("eType=\"#//Volume\""));
    assert!(source.contains("eOpposite=\"#//Volume/shelf\""));
    let doc = change(
        &source,
        json!({"op":"set_feature","class":"Volume","feature":"shelf","name":"location"}),
    )
    .unwrap();
    assert!(doc.source.contains("eOpposite=\"#//Volume/location\""));
    intersect_model_acceptance::migration_fixture::compile_migrated_schema_fixture(&doc.source)
        .unwrap();
}

#[test]
fn edits_flags_defaults_package_properties_and_clears_optional_values() {
    let source = library();
    let source = change(
        &source,
        json!({"op":"set_package","name":"books","namespace":"urn:books?x=1&y=2","prefix":"books"}),
    )
    .unwrap()
    .source;
    let source = change(
        &source,
        json!({"op":"set_class","class":"Book","abstract":true}),
    )
    .unwrap()
    .source;
    let doc = change(&source, json!({"op":"set_feature","class":"Book","feature":"title","default":"A \"title\" & <a>\nβ","ordered":false,"unique":false})).unwrap();
    assert_eq!(doc.namespace, "urn:books?x=1&y=2");
    assert!(doc.classes[0].abstract_type);
    let feature = &doc.classes[0].features[0];
    assert_eq!(feature.default.as_deref(), Some("A \"title\" & <a>\nβ"));
    assert!(!feature.ordered && !feature.unique);
    assert!(doc.source.contains("&quot;title&quot; &amp; &lt;a>&#10;β"));
    let doc = change(
        &doc.source,
        json!({"op":"set_feature","class":"Book","feature":"title","default":null}),
    )
    .unwrap();
    assert_eq!(doc.classes[0].features[0].default, None);
    assert!(!doc.source.contains("defaultValueLiteral"));
}

#[test]
fn defaults_and_opposites_are_unchanged_when_not_provided() {
    let source = library();
    let source = change(
        &source,
        json!({"op":"set_feature","class":"Book","feature":"title","default":"kept"}),
    )
    .unwrap()
    .source;
    let doc = change(
        &source,
        json!({"op":"set_feature","class":"Book","feature":"title","ordered":false}),
    )
    .unwrap();
    assert_eq!(doc.classes[0].features[0].default.as_deref(), Some("kept"));
}

#[test]
fn rejects_dangling_deletions_and_then_removes_unreferenced_elements() {
    let source = library();
    assert!(
        change(&source, json!({"op":"delete_class","class":"Book"}))
            .unwrap_err()
            .contains("references")
    );
    let source = change(
        &source,
        json!({"op":"delete_feature","class":"Shelf","feature":"books"}),
    )
    .unwrap()
    .source;
    let doc = change(&source, json!({"op":"delete_class","class":"Book"})).unwrap();
    assert_eq!(doc.classes.len(), 1);
    assert_eq!(doc.classes[0].name, "Shelf");
    intersect_model_acceptance::migration_fixture::compile_migrated_schema_fixture(&doc.source)
        .unwrap();
}

#[test]
fn prevents_loss_of_unsupported_content_during_deletion() {
    let source = library().replace("name=\"Book\"", "name=\"Book\" custom=\"keep\"");
    assert!(
        change(&source, json!({"op":"delete_class","class":"Book"}))
            .unwrap_err()
            .contains("unsupported")
    );
    let source = library().replace("<eStructuralFeatures", "<!-- keep --> <eStructuralFeatures");
    assert!(
        change(&source, json!({"op":"delete_class","class":"Shelf"}))
            .unwrap_err()
            .contains("comments")
    );
    let source = library().replace("name=\"title\"", "name=\"title\" custom=\"keep\"");
    assert!(
        change(
            &source,
            json!({"op":"delete_feature","class":"Book","feature":"title"})
        )
        .unwrap_err()
        .contains("unsupported")
    );
}

#[test]
fn refuses_unknown_local_reference_rewrites_instead_of_guessing() {
    let source = library().replace(
        "</ecore:EPackage>",
        "<eAnnotations source=\"#//Book\"/></ecore:EPackage>",
    );
    assert!(
        change(
            &source,
            json!({"op":"set_class","class":"Book","name":"Volume"})
        )
        .unwrap_err()
        .contains("unsupported")
    );
}

#[test]
fn parseable_semantic_drafts_are_inspectable_and_repairable() {
    let source = library().replace("#//Book", "#//Missing").replace(
        "name=\"title\"",
        "name=\"title\" ordered=\"wrong\" lowerBound=\"oops\"",
    );
    let doc = ecore_document::inspect(&source).unwrap();
    assert!(!doc.warnings.is_empty());
    assert_eq!(doc.classes.len(), 2);
    assert_eq!(doc.classes[1].features[0].type_name, "Missing");
    let doc = change(
        &source,
        json!({"op":"set_feature","class":"Book","feature":"title","ordered":true,"lower":0}),
    )
    .unwrap();
    assert!(!doc.source.contains("oops") && !doc.source.contains("wrong"));
    let doc = change(
        &doc.source,
        json!({"op":"set_feature","class":"Shelf","feature":"books","type":"Book"}),
    )
    .unwrap();
    assert!(doc.warnings.is_empty());
}

#[test]
fn rejects_invalid_edits_without_mutating_source() {
    let source = library();
    for edit in [
        json!({"op":"add_class","name":"Book"}),
        json!({"op":"add_class","name":"broken name"}),
        json!({"op":"add_attribute","class":"Book","name":"title","type":"EString"}),
        json!({"op":"add_attribute","class":"Book","name":"bad","type":"Shelf"}),
        json!({"op":"add_reference","class":"Book","name":"bad","type":"EString"}),
        json!({"op":"add_reference","class":"Book","name":"bad","type":"Missing"}),
        json!({"op":"set_feature","class":"Book","feature":"title","lower":-1}),
        json!({"op":"set_feature","class":"Book","feature":"title","lower":2,"upper":1}),
        json!({"op":"set_feature","class":"Book","feature":"title","upper":-2}),
        json!({"op":"set_feature","class":"Book","feature":"title","lower":2147483648_i64,"upper":-1}),
        json!({"op":"set_feature","class":"Book","feature":"title","containment":true}),
        json!({"op":"set_feature","class":"Book","feature":"title","opposite":"Shelf/books"}),
        json!({"op":"set_feature","class":"Shelf","feature":"books","opposite":"Book/title"}),
        json!({"op":"set_feature","class":"Shelf","feature":"books","default":"bad"}),
        json!({"op":"set_feature","class":"Book","feature":"title","default":"\u{0001}"}),
        json!({"op":"set_class","class":"Book","super_types":["Book"]}),
        json!({"op":"set_class","class":"Book","super_types":["Shelf","Book"]}),
    ] {
        assert!(change(&source, edit.clone()).is_err(), "accepted {edit}");
    }
    assert_eq!(ecore_document::inspect(&source).unwrap().source, source);
}

#[test]
fn rejects_wrong_json_field_types_and_unknown_fields() {
    for edit in [
        json!({"op":"set_class","class":"Book","abstract":"yes"}),
        json!({"op":"set_feature","class":"Book","feature":"title","lower":0.5}),
        json!({"op":"set_feature","class":"Book","feature":"title","default":12}),
        json!({"op":"add_class","name":"Book","typo":true}),
    ] {
        assert!(serde_json::from_value::<Edit>(edit).is_err());
    }
}

#[test]
fn rejects_malformed_xml_and_ambiguous_namespaces() {
    for source in [
        "",
        "<",
        "<broken>",
        "<a></b>",
        "<a/><b/>",
        "<!DOCTYPE x><x/>",
    ] {
        assert!(
            ecore_document::inspect(source).is_err(),
            "accepted {source}"
        );
    }
    let source = library().replace("name=\"Book\"", "name=\"Book\" name=\"Duplicate\"");
    assert!(ecore_document::inspect(&source).is_err());
    let source = library().replace("name=\"Book\"", "name=\"Book\" xmlns:ecore=\"urn:other\"");
    assert!(ecore_document::inspect(&source).unwrap().classes.is_empty());
    assert!(change(&source, json!({"op":"add_class","name":"Extra"})).is_err());
}

#[test]
fn preserves_crlf_line_endings_and_safe_empty_element_expansion() {
    let source = EMPTY.replace('\n', "\r\n");
    let source = change(&source, json!({"op":"add_class","name":"Book"}))
        .unwrap()
        .source;
    let doc = change(
        &source,
        json!({"op":"add_attribute","class":"Book","name":"title","type":"EString"}),
    )
    .unwrap();
    assert!(!doc.source.replace("\r\n", "").contains('\n'));
    assert!(doc.warnings.is_empty());
}

#[test]
fn same_value_edit_keeps_entity_spelling_and_whitespace_exactly() {
    let source = library().replace("name=\"Book\"", "name = 'B&#111;ok'");
    let doc = change(
        &source,
        json!({"op":"set_class","class":"Book","name":"Book"}),
    )
    .unwrap();
    assert_eq!(doc.source, source);
}

#[test]
fn preserves_utf8_bom_with_or_without_declaration() {
    for source in [
        EMPTY.to_owned(),
        EMPTY.lines().skip(1).collect::<Vec<_>>().join("\n"),
    ] {
        let source = format!("\u{feff}{source}");
        let doc = change(&source, json!({"op":"set_package","name":"books"})).unwrap();
        assert_eq!(
            doc.source,
            source.replace("name=\"library\"", "name=\"books\"")
        );
    }
}

#[test]
fn feature_deletion_requires_clearing_its_opposite_dependency() {
    let source = library();
    let source = change(
        &source,
        json!({"op":"add_reference","class":"Book","name":"shelf","type":"Shelf"}),
    )
    .unwrap()
    .source;
    let source = change(
        &source,
        json!({"op":"set_feature","class":"Book","feature":"shelf","opposite":"Shelf/books"}),
    )
    .unwrap()
    .source;
    assert!(
        change(
            &source,
            json!({"op":"delete_feature","class":"Shelf","feature":"books"})
        )
        .unwrap_err()
        .contains("references")
    );
    let source = change(
        &source,
        json!({"op":"set_feature","class":"Book","feature":"shelf","opposite":null}),
    )
    .unwrap()
    .source;
    let doc = change(
        &source,
        json!({"op":"delete_feature","class":"Shelf","feature":"books"}),
    )
    .unwrap();
    assert_eq!(doc.classes[0].features[1].opposite, None);
    assert!(doc.classes[1].features.is_empty());
}

#[test]
fn blocks_unresolved_inherited_opposite_aliases_instead_of_dangling_them() {
    let source = library();
    let source = change(&source, json!({"op":"add_class","name":"Novel"}))
        .unwrap()
        .source;
    let source = change(
        &source,
        json!({"op":"set_class","class":"Novel","super_types":["Book"]}),
    )
    .unwrap()
    .source;
    let source = change(
        &source,
        json!({"op":"add_reference","class":"Book","name":"shelf","type":"Shelf"}),
    )
    .unwrap()
    .source;
    let source = source.replace(
        "name=\"books\"",
        "name=\"books\" eOpposite=\"#//Novel/shelf\"",
    );
    for op in [
        json!({"op":"set_feature","class":"Book","feature":"shelf","name":"location"}),
        json!({"op":"delete_feature","class":"Book","feature":"shelf"}),
    ] {
        assert!(
            change(&source, op)
                .unwrap_err()
                .contains("Cannot safely resolve")
        );
    }
}

#[test]
fn class_rename_only_rewrites_complete_local_path_components() {
    let source = library();
    let source = change(&source, json!({"op":"add_class","name":"BookCase"}))
        .unwrap()
        .source;
    let source = change(
        &source,
        json!({"op":"add_reference","class":"Shelf","name":"case","type":"BookCase"}),
    )
    .unwrap()
    .source;
    let doc = change(
        &source,
        json!({"op":"set_class","class":"Book","name":"Volume"}),
    )
    .unwrap();
    assert!(doc.source.contains("eType=\"#//BookCase\""));
    assert!(doc.source.contains("eType=\"#//Volume\""));
}

#[test]
fn enum_inspection_and_attribute_edits_use_shared_enum_contracts() {
    let declaration = r#"<eClassifiers xsi:type="ecore:EEnum" name="Status"><eLiterals name="DRAFT" value="7" literal="draft"/><eLiterals name="ACTIVE" value="7" literal="active"/></eClassifiers>"#;
    let source = library().replace(
        "</ecore:EPackage>",
        &format!("{declaration}</ecore:EPackage>"),
    );
    let inspected = ecore_document::inspect(&source).unwrap();
    assert!(inspected.warnings.is_empty(), "{:?}", inspected.warnings);
    assert_eq!(inspected.enums[0].literals()[0].value, 7);
    assert_eq!(inspected.enums[0].literals()[1].spelling(), "active");
    let edited = change(
        &source,
        json!({"op":"add_attribute", "class":"Book", "name":"status", "type":"Status"}),
    )
    .unwrap();
    let edited = change(
        &edited.source,
        json!({"op":"set_feature", "class":"Book", "feature":"status", "default":"active"}),
    )
    .unwrap();
    assert!(
        edited.source.contains(declaration),
        "enum source must remain untouched"
    );
    let schema = intersect_model_acceptance::migration_fixture::compile_migrated_schema_fixture(
        &edited.source,
    )
    .unwrap();
    assert_eq!(
        schema.classes["Book"].features["status"].default,
        serde_json::to_value(
            schema.enumerations["Status"]
                .value_by_id(&schema.enumerations["Status"].literals()[1].declaration)
                .unwrap()
        )
        .unwrap()
    );
    assert!(
        change(
            &edited.source,
            json!({"op":"add_reference", "class":"Book", "name":"invalid", "type":"Status"})
        )
        .is_err()
    );
}
