use intersect_model::definitions::{
    bundle::{DefinitionSource, SourceKind},
    ecore::{Document, ErrorKind},
    ecore_document,
};

const LIBRARY: &str = include_str!("../../../../fixtures/library/library.ecore");
fn source(text: &str) -> DefinitionSource {
    DefinitionSource::new("file:///test.ecore", SourceKind::Ecore, text)
}

#[test]
fn prefixes_are_aliases_in_names_and_typed_attribute_values() {
    let changed = LIBRARY
        .replace("ecore:", "model:")
        .replace("xmlns:ecore", "xmlns:model")
        .replace("xsi:", "instance:")
        .replace("xmlns:xsi", "xmlns:instance");
    let original =
        intersect_model_acceptance::migration_fixture::compile_migrated_schema_fixture(LIBRARY)
            .unwrap();
    let renamed =
        intersect_model_acceptance::migration_fixture::compile_migrated_schema_fixture(&changed)
            .unwrap();
    for (name, class) in &original.classes {
        let other = &renamed.classes[name];
        assert_eq!(class.features, other.features);
        assert_eq!(class.feature_order, other.feature_order);
        assert_eq!(
            class.super_types().collect::<Vec<_>>(),
            other.super_types().collect::<Vec<_>>()
        );
        for (feature, id) in &class.feature_declarations {
            let changed_id = &other.feature_declarations[feature];
            assert_eq!(id.path, changed_id.path);
            assert_eq!(id.source.sha256, changed_id.source.sha256);
        }
    }
    assert_eq!(original.uri, renamed.uri);
    // XML prefixes do not change canonical structure. Original input identities
    // remain distinct in the migration record and raw document inspection.
    assert_eq!(original.revision, renamed.revision);
    assert_ne!(source(LIBRARY).id, source(&changed).id);
    let doc = Document::parse(source(&changed)).unwrap();
    assert_eq!(doc.source.text, changed);
    assert_eq!(
        doc.root.name.namespace.as_deref(),
        Some("http://www.eclipse.org/emf/2002/Ecore")
    );
}

#[test]
fn unsupported_payloads_and_nested_nodes_are_retained_with_exact_spans() {
    let text = LIBRARY.replace("</ecore:EPackage>", "<eAnnotations source=\"vendor\"><details key=\"a&amp;b\" value=\"😀\"/><v:data xmlns:v=\"urn:vendor\"><![CDATA[<opaque/>]]><?keep x?></v:data></eAnnotations></ecore:EPackage>");
    let doc = Document::parse(source(&text)).unwrap();
    let annotation = doc.root.children.last().unwrap();
    assert_eq!(
        &text[annotation.span.start..annotation.span.end],
        "<eAnnotations source=\"vendor\"><details key=\"a&amp;b\" value=\"😀\"/><v:data xmlns:v=\"urn:vendor\"><![CDATA[<opaque/>]]><?keep x?></v:data></eAnnotations>"
    );
    assert_eq!(annotation.children[0].attributes[0].value, "a&b");
    assert_eq!(annotation.children[1].payloads.len(), 2);
    let error = ecore_document::validate_structure(&text).unwrap_err();
    assert_eq!(error.kind, ErrorKind::Unsupported);
    assert_eq!(error.span.start, annotation.opening.start);
    assert_eq!(doc.source.text, text);
}

#[test]
fn namespace_errors_and_malformed_xml_have_real_unicode_byte_locations() {
    for replacement in [
        "<lost:eClassifiers",
        "<eClassifiers xsi:type=\"bad:EClass\"",
    ] {
        let text = format!(
            "<!--😀 café-->\n{}",
            LIBRARY
                .replace("<?xml version=\"1.0\" encoding=\"UTF-8\"?>", "")
                .replacen("<eClassifiers", replacement, 1)
        );
        let error = ecore_document::validate_structure(&text).unwrap_err();
        assert_eq!(error.kind, ErrorKind::MalformedXml);
        assert!(error.span.start > "<!--😀 café-->".len());
        assert!(text.is_char_boundary(error.span.start));
        assert!(text.is_char_boundary(error.span.end));
    }
    let text = LIBRARY.replace("</ecore:EPackage>", "</wrong>");
    let error = ecore_document::validate_structure(&text).unwrap_err();
    assert_eq!(error.kind, ErrorKind::MalformedXml);
    assert_eq!(&text[error.span.start..error.span.end], "</wrong>");
}

#[test]
fn namespace_spoofing_and_duplicate_expanded_attributes_fail() {
    let spoof = LIBRARY.replace("name=\"Loan\"", "xmlns:ecore=\"urn:fake\" name=\"Loan\"");
    assert_eq!(
        ecore_document::validate_structure(&spoof).unwrap_err().kind,
        ErrorKind::Unsupported
    );
    let duplicate = LIBRARY.replace("name=\"Loan\"", "xmlns:i=\"http://www.w3.org/2001/XMLSchema-instance\" i:type=\"ecore:EClass\" name=\"Loan\"");
    assert_eq!(
        Document::parse(source(&duplicate)).unwrap_err().kind,
        ErrorKind::MalformedXml
    );
}

#[test]
fn upstream_trees_are_imported_without_claiming_execution_support() {
    for text in [
        include_str!(
            "../../../../fixtures/definition-conformance/upstream/ecore/model/Ecore.ecore"
        ),
        include_str!(
            "../../../../fixtures/definition-conformance/upstream/xtext/org/eclipse/xtext/Xtext.ecore"
        ),
    ] {
        let doc = Document::parse(source(text)).unwrap();
        assert_eq!(doc.source.text, text);
        assert!(doc.root.children.len() > 10);
        assert_eq!(
            ecore_document::validate_structure(text).unwrap_err().kind,
            ErrorKind::Unsupported
        );
    }
}

#[test]
fn local_aliases_default_namespace_and_xml_escaping_are_resolved() {
    let text = LIBRARY
        .replace(
            "<ecore:EPackage",
            "<EPackage xmlns=\"http://www.eclipse.org/emf/2002/Ecore\"",
        )
        .replace("</ecore:EPackage>", "</EPackage>")
        .replace("<eClassifiers", "<eClassifiers xmlns=\"\"")
        .replace(
            "name=\"Loan\"",
            "xmlns:m=\"http://www.eclipse.org/emf/2002/Ecore\" name=\"Loan\"",
        )
        .replace(
            "name=\"title\"",
            "name=\"title\" defaultValueLiteral=\"café &#x1F600; &amp; &quot;quoted&quot;\"",
        );
    let schema =
        intersect_model_acceptance::migration_fixture::compile_migrated_schema_fixture(&text)
            .unwrap();
    assert_eq!(
        schema.classes["BookCopy"].features["title"].default,
        "café 😀 & \"quoted\""
    );
    let alias = LIBRARY.replace("<eClassifiers xsi:type=\"ecore:EClass\" name=\"Loan\"", "<eClassifiers xmlns:local=\"http://www.eclipse.org/emf/2002/Ecore\" xsi:type=\"local:EClass\" name=\"Loan\"");
    assert!(
        intersect_model_acceptance::migration_fixture::compile_migrated_schema_fixture(&alias)
            .is_ok()
    );
}

#[test]
fn malformed_xml_is_not_misreported_as_an_unsupported_feature() {
    for text in [
        "<a/><b/>",
        "<a>",
        "<a></b>",
        "<a x='&missing;'/>",
        "<a x='&#0;'/>",
        "<a>&missing;</a>",
        "<a>&#0;</a>",
        "<a x='<bad'/>",
        "<a><!--bad--comment--></a>",
        "<a>]]></a>",
        "<a><1bad/></a>",
        "<a><b:c:d/></a>",
        "<a/>text",
        "<a/><?xml version='1.0'?>",
        "<a xmlns:xml='urn:bad'/>",
        "<a xmlns:xmlns='urn:bad'/>",
    ] {
        assert_eq!(
            Document::parse(source(text)).unwrap_err().kind,
            ErrorKind::MalformedXml,
            "{text}"
        );
    }
    assert_eq!(
        Document::parse(source(
            "<!DOCTYPE a SYSTEM 'https://example.invalid/external'><a/>"
        ))
        .unwrap_err()
        .kind,
        ErrorKind::Unsupported
    );
    assert_eq!(
        Document::parse(source(&format!(
            "{}{}",
            "<a>".repeat(64),
            "</a>".repeat(64)
        )))
        .unwrap_err()
        .kind,
        ErrorKind::ResourceLimit
    );
    assert_eq!(
        Document::parse(source(&" ".repeat(256_001)))
            .unwrap_err()
            .kind,
        ErrorKind::ResourceLimit
    );
}

#[test]
fn diagnostic_attribute_ranges_preserve_unicode_and_source_identity() {
    let text = LIBRARY.replace("name=\"Loan\"", "name=\"Loan😀\" volatile = 'true'");
    let error = ecore_document::validate_structure(&text).unwrap_err();
    assert_eq!(&text[error.span.start..error.span.end], "volatile = 'true'");
    assert_eq!(error.span.source.sha256, source(&text).id.sha256);
    let report =
        intersect_model::migration::migrate_ecore(source(&text), "memory:invalid.rcore").unwrap();
    assert!(
        report
            .issues
            .iter()
            .any(|issue| issue.span.start == error.span.start && issue.span.end == error.span.end)
    );
}

#[test]
fn bom_crlf_and_encoded_namespace_bindings_keep_exact_locations() {
    let text = "\u{feff}<e:EPackage xmlns:e='http://www.eclipse.org/emf/2002/Ecor&#101;'\r\n name='😀' nsURI='urn:test' volatile='true'/>";
    let doc = Document::parse(source(text)).unwrap();
    assert_eq!(doc.root.opening.start, 3);
    assert_eq!(
        doc.root.name.namespace.as_deref(),
        Some("http://www.eclipse.org/emf/2002/Ecore")
    );
    for attr in &doc.root.attributes {
        assert!(text[attr.span.start..attr.span.end].starts_with(&attr.lexical_name));
    }
    let error = ecore_document::validate_structure(text).unwrap_err();
    assert_eq!(&text[error.span.start..error.span.end], "volatile='true'");
    for malformed in [
        "<a xmlns:p='http://www.w3.org/XML/1998/namespac&#101;'/>",
        "<a xmlns:p=''/>",
        "<a/>\u{feff}",
    ] {
        assert_eq!(
            Document::parse(source(malformed)).unwrap_err().kind,
            ErrorKind::MalformedXml
        );
    }
}

#[test]
fn declaration_validation_and_extension_values_are_not_silently_ignored() {
    for text in [
        "<?xml version='1.0' unknown='x'?><a/>",
        "<?xml version='1.0' standalone='maybe'?><a/>",
        "<?xml version='1.0' standalone='yes' encoding='UTF-8'?><a/>",
        "<?xml version='1.0' version='1.0'?><a/>",
    ] {
        assert_eq!(
            Document::parse(source(text)).unwrap_err().kind,
            ErrorKind::MalformedXml
        );
    }
    assert!(
        Document::parse(source(
            "<?xml version='1.0' encoding='UTF-8' standalone='yes'?><a/>"
        ))
        .is_ok()
    );
    let doc = Document::parse(source(
        "<vendor xmlns='urn:extension' eType='arbitrary:opaque data'/>",
    ))
    .unwrap();
    assert_eq!(doc.root.attributes[1].value, "arbitrary:opaque data");
    let text = LIBRARY.replace("</ecore:EPackage>", "\u{a0}</ecore:EPackage>");
    let error = ecore_document::validate_structure(&text).unwrap_err();
    assert_eq!(error.kind, ErrorKind::Unsupported);
    assert!(text[error.span.start..error.span.end].contains('\u{a0}'));
}
