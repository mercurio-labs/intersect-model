use intersect_model::definitions::{
    bundle::{DefinitionBundle, DefinitionSource, SourceKind},
    compiler::standard_datatype_source,
    ecore::{ErrorKind, Node},
    generics::{GenericKind, GenericType, parse_parameter, parse_type},
    resources::{DeclarationId, ResourceSet},
};
fn source(body: &str) -> DefinitionSource {
    DefinitionSource::new(
        "memory:generics",
        SourceKind::Ecore,
        format!(
            r#"<e:EPackage xmlns:e="http://www.eclipse.org/emf/2002/Ecore" xmlns:x="http://www.w3.org/2001/XMLSchema-instance" name="g" nsURI="urn:g"><eClassifiers x:type="e:EClass" name="Holder">{body}</eClassifiers></e:EPackage>"#
        ),
    )
}
fn resources(source: DefinitionSource) -> ResourceSet {
    ResourceSet::new(&DefinitionBundle {
        version: 1,
        sources: vec![source],
        namespaces: vec![],
    })
    .unwrap()
}
fn generic(text: &str) -> Result<GenericType, intersect_model::definitions::ecore::ImportError> {
    let source = source(&format!(
        r#"<eStructuralFeatures x:type="e:EReference" name="value">{text}</eStructuralFeatures>"#
    ));
    let id = DeclarationId {
        source: source.id.clone(),
        path: vec![0, 0, 0],
    };
    parse_type(&resources(source), &id)
}
#[test]
fn recursive_parameter_bounds_and_wildcards_retain_order_identity_and_source() {
    let text = r##"<eTypeParameters name="T"><eAnnotations source="example"><details key="doc" value="retained"/></eAnnotations><eBounds eClassifier="#//Comparable"><eTypeArguments eTypeParameter="#//Holder/T"/></eBounds><eBounds eClassifier="#//Tagged"/></eTypeParameters>"##;
    let source = source(text);
    let id = DeclarationId {
        source: source.id.clone(),
        path: vec![0, 0],
    };
    let parameter = parse_parameter(&resources(source.clone()), &id).unwrap();
    assert_eq!(parameter.name, "T");
    assert_eq!(parameter.bounds.len(), 2);
    assert_eq!(parameter.source_payload, text);
    assert_eq!(&source.text[parameter.span.start..parameter.span.end], text);
    assert_eq!(parameter.bounds[0].declaration.path, [0, 0, 1]);
    let GenericKind::Classifier { arguments, .. } = &parameter.bounds[0].value else {
        panic!("classifier")
    };
    assert!(matches!(arguments[0].value, GenericKind::Parameter { .. }));
    let generic = generic(r##"<eGenericType eClassifier="e:EClass #//Map"><eTypeArguments/><eTypeArguments><eUpperBound eClassifier="#//Holder"/></eTypeArguments><eTypeArguments><eLowerBound eTypeParameter="#//Holder/T"/></eTypeArguments></eGenericType>"##).unwrap();
    let GenericKind::Classifier {
        reference,
        arguments,
    } = &generic.value
    else {
        panic!("classifier")
    };
    assert_eq!(reference.kind_hint.as_deref(), Some("EClass"));
    assert_eq!(reference.href, "#//Map");
    assert_eq!(arguments.len(), 3);
    assert!(matches!(
        arguments[0].value,
        GenericKind::Wildcard {
            upper: None,
            lower: None
        }
    ));
    assert!(matches!(
        arguments[1].value,
        GenericKind::Wildcard {
            upper: Some(_),
            lower: None
        }
    ));
    assert!(matches!(
        arguments[2].value,
        GenericKind::Wildcard {
            upper: None,
            lower: Some(_)
        }
    ));
    assert_eq!(
        serde_json::from_value::<GenericType>(serde_json::to_value(&generic).unwrap()).unwrap(),
        generic
    );
}
#[test]
fn reference_hints_and_href_children_are_namespace_aware() {
    let first =
        generic(r##"<eGenericType eClassifier="e:EDataType types.ecore#//List"/>"##).unwrap();
    let second = generic(r##"<eGenericType><eClassifier xmlns:q="http://www.eclipse.org/emf/2002/Ecore" x:type="q:EDataType" href="types.ecore#//List"/></eGenericType>"##).unwrap();
    let (
        GenericKind::Classifier { reference: a, .. },
        GenericKind::Classifier { reference: b, .. },
    ) = (first.value, second.value)
    else {
        panic!("classifiers")
    };
    assert_eq!(a.href, b.href);
    assert_eq!(a.kind_hint, b.kind_hint);
    assert!(generic(r##"<eGenericType eClassifier="x:EDataType #//List"/>"##).is_err());
    assert!(generic(r##"<eGenericType eTypeParameter="e:EClass #//Holder/T"/>"##).is_err());
    let parameter=generic(r##"<eGenericType><eTypeParameter href="#//Holder/T" x:type="e:ETypeParameter"/></eGenericType>"##).unwrap();
    assert!(matches!(parameter.value, GenericKind::Parameter { .. }));
}
#[test]
fn inconsistent_shapes_and_unsupported_fields_have_located_errors() {
    for text in [
        r##"<eGenericType/>"##,
        r##"<eGenericType eClassifier="#//A" eTypeParameter="#//T"/>"##,
        r##"<eGenericType eTypeParameter="#//T"><eTypeArguments/></eGenericType>"##,
        r##"<eGenericType eClassifier="#//A"><eUpperBound eClassifier="#//B"/></eGenericType>"##,
        r##"<eGenericType eClassifier="#//A"><eTypeArguments><eUpperBound eClassifier="#//B"/><eLowerBound eClassifier="#//B"/></eTypeArguments></eGenericType>"##,
        r##"<eGenericType eClassifier="#//A"><eTypeArguments><eUpperBound eClassifier="#//B"/><eUpperBound eClassifier="#//C"/></eTypeArguments></eGenericType>"##,
        r##"<eGenericType eClassifier="#//A"><eClassifier href="#//B"/></eGenericType>"##,
        r##"<eGenericType><eClassifier href=""/></eGenericType>"##,
        r##"<eGenericType><eClassifier href="#//A">text</eClassifier></eGenericType>"##,
        r##"<eGenericType eClassifier="e:EClass #//A #//B"/>"##,
        r##"<eGenericType eRawType="#//A"/>"##,
        r##"<eGenericType eClassifier="#//A"><unknown/></eGenericType>"##,
        r##"<eGenericType eClassifier="#//A"><![CDATA[ignored]]></eGenericType>"##,
    ] {
        let error = generic(text).unwrap_err();
        assert!(error.span.end > error.span.start, "{text}: {error}");
        assert!(matches!(
            error.kind,
            ErrorKind::InvalidDefinition | ErrorKind::Unsupported
        ));
    }
}
#[test]
fn parameter_shapes_reject_missing_names_unknown_fields_and_wrong_nodes() {
    for text in [
        r#"<eTypeParameters/>"#,
        r#"<eTypeParameters name=""/>"#,
        r#"<eTypeParameters name=" T"/>"#,
        r#"<eTypeParameters name="T" unknown="x"/>"#,
        r#"<eTypeParameters name="T"><unknown/></eTypeParameters>"#,
        r#"<eTypeParameters name="T" x:type="e:EClass"/>"#,
        r#"<eTypeArguments/>"#,
    ] {
        let source = source(text);
        let id = DeclarationId {
            source: source.id.clone(),
            path: vec![0, 0],
        };
        assert!(parse_parameter(&resources(source), &id).is_err(), "{text}");
    }
}
#[test]
fn all_pinned_generic_nodes_and_parameters_have_lossless_ast_consumers() {
    fn visit(
        resources: &ResourceSet,
        node: &Node,
        id: &DeclarationId,
        counts: &mut (usize, usize),
    ) {
        if node.name.namespace.is_none() && node.name.local == "eTypeParameters" {
            let parsed = parse_parameter(resources, id).unwrap();
            assert_eq!(parsed.declaration, *id);
            counts.0 += 1;
        }
        if node.name.namespace.is_none()
            && matches!(
                node.name.local.as_str(),
                "eGenericType"
                    | "eGenericSuperTypes"
                    | "eGenericExceptions"
                    | "eTypeArguments"
                    | "eBounds"
                    | "eUpperBound"
                    | "eLowerBound"
            )
        {
            let parsed = parse_type(resources, id).unwrap();
            assert_eq!(
                parsed.source_payload,
                resources.document(&id.source).unwrap().source.text[node.span.start..node.span.end]
            );
            counts.1 += 1;
        }
        for (index, nested) in node.children.iter().enumerate() {
            let mut next = id.clone();
            next.path.push(index);
            visit(resources, nested, &next, counts);
        }
    }
    let source = standard_datatype_source();
    let id = DeclarationId {
        source: source.id.clone(),
        path: vec![],
    };
    let resources = resources(source);
    let mut counts = (0, 0);
    visit(&resources, resources.node(&id).unwrap(), &id, &mut counts);
    assert_eq!(counts, (5, 12));
}

#[test]
fn parameter_names_follow_pinned_unicode_identifier_rules() {
    for (name, valid) in [
        ("T", true),
        ("_T", true),
        ("λ", true),
        ("€", true),
        ("𐐀", true),
        ("中", true),
        ("T́", true),
        ("T٠", true),
        ("T‌", true),
        ("class", true),
        ("T T", false),
        ("0T", false),
        ("T$", false),
        ("$T", false),
        ("́T", false),
        ("😀", false),
        ("Ᲊ", false),
        ("T/name", false),
    ] {
        let source = source(&format!("<eTypeParameters name=\"{name}\"/>"));
        let id = DeclarationId {
            source: source.id.clone(),
            path: vec![0, 0],
        };
        let parsed = parse_parameter(&resources(source), &id);
        assert_eq!(parsed.is_ok(), valid, "{name}: {parsed:?}");
        if valid {
            assert_eq!(parsed.unwrap().name, name);
        }
    }
}
