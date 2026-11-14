use intersect_model::definitions::{
    bundle::{DefinitionBundle, DefinitionSource, SourceKind},
    compiler::standard_datatype_source,
    generic_resolution::{Resolver, TargetKind},
    resources::DeclarationId,
};
fn source(uri: &str, body: &str) -> DefinitionSource {
    DefinitionSource::new(
        uri,
        SourceKind::Ecore,
        format!(
            r#"<e:EPackage xmlns:e="http://www.eclipse.org/emf/2002/Ecore" xmlns:x="http://www.w3.org/2001/XMLSchema-instance" name="g" nsURI="{uri}">{body}</e:EPackage>"#
        ),
    )
}
fn resolver(sources: Vec<DefinitionSource>) -> Resolver {
    Resolver::new(&DefinitionBundle {
        version: 1,
        sources,
        namespaces: vec![],
    })
    .unwrap()
}
fn id(source: &DefinitionSource, path: &[usize]) -> DeclarationId {
    DeclarationId {
        source: source.id.clone(),
        path: path.to_vec(),
    }
}
#[test]
fn f_bounds_class_and_operation_parameter_scopes_keep_exact_identity() {
    let s = source(
        "memory:g",
        r##"
<eClassifiers x:type="e:EClass" name="Comparable"><eTypeParameters name="X"/></eClassifiers>
<eClassifiers x:type="e:EClass" name="Box">
 <eTypeParameters name="T"><eBounds eClassifier="#//Comparable"><eTypeArguments eTypeParameter="#//Box/T"/></eBounds></eTypeParameters>
 <eStructuralFeatures x:type="e:EReference" name="value"><eGenericType eTypeParameter="#//Box/T"/></eStructuralFeatures>
 <eOperations name="convert"><eTypeParameters name="T"/><eGenericType eTypeParameter="#//Box/convert/T"/><eParameters name="input"><eGenericType eTypeParameter="#//Box/T"/></eParameters></eOperations>
</eClassifiers>"##,
    );
    let r = resolver(vec![s.clone()]);
    let parameter = r.resolve_parameter(&id(&s, &[1, 0])).unwrap();
    assert_eq!(parameter.bounds[0].bindings[0].target, id(&s, &[0]));
    assert_eq!(parameter.bounds[0].bindings[1].target, id(&s, &[1, 0]));
    assert_eq!(
        r.resolve_type(&id(&s, &[1, 1, 0])).unwrap().bindings[0].target,
        id(&s, &[1, 0])
    );
    assert_eq!(
        r.resolve_type(&id(&s, &[1, 2, 1])).unwrap().bindings[0].target,
        id(&s, &[1, 2, 0])
    );
    assert_eq!(
        r.resolve_type(&id(&s, &[1, 2, 2, 0])).unwrap().bindings[0].target,
        id(&s, &[1, 0])
    );
    assert_eq!(
        parameter.bounds[0].bindings[1].target_kind,
        TargetKind::Parameter
    );
}
#[test]
fn direct_self_forward_and_unrelated_scope_references_reject_but_prior_bounds_work() {
    for (parameters, index, valid) in [
        (
            r##"<eTypeParameters name="T"><eBounds eTypeParameter="#//C/T"/></eTypeParameters>"##,
            0,
            false,
        ),
        (
            r##"<eTypeParameters name="T"><eBounds eTypeParameter="#//C/U"/></eTypeParameters><eTypeParameters name="U"/>"##,
            0,
            false,
        ),
        (
            r##"<eTypeParameters name="T"/><eTypeParameters name="U"><eBounds eTypeParameter="#//C/T"/></eTypeParameters>"##,
            1,
            true,
        ),
        (
            r##"<eTypeParameters name="T"><eBounds eClassifier="#//C"><eTypeArguments eTypeParameter="#//C/U"/><eTypeArguments/></eBounds></eTypeParameters><eTypeParameters name="U"/>"##,
            0,
            true,
        ),
    ] {
        let s = source(
            "memory:g",
            &format!(r#"<eClassifiers x:type="e:EClass" name="C">{parameters}</eClassifiers>"#),
        );
        assert_eq!(
            resolver(vec![s.clone()])
                .resolve_parameter(&id(&s, &[0, index]))
                .is_ok(),
            valid
        );
    }
    let s = source(
        "memory:g",
        r##"<eClassifiers x:type="e:EClass" name="C"><eTypeParameters name="T"/></eClassifiers><eClassifiers x:type="e:EClass" name="Other"><eStructuralFeatures x:type="e:EReference" name="value"><eGenericType eTypeParameter="#//C/T"/></eStructuralFeatures></eClassifiers>"##,
    );
    assert!(
        resolver(vec![s.clone()])
            .resolve_type(&id(&s, &[1, 0, 0]))
            .unwrap_err()
            .message
            .contains("scope")
    );
}
fn application(application: &str, feature_kind: &str) -> (DefinitionSource, Resolver) {
    let s = source(
        "memory:g",
        &format!(
            r##"<eClassifiers x:type="e:EClass" name="Box"><eTypeParameters name="T"/></eClassifiers><eClassifiers x:type="e:EClass" name="Holder"><eStructuralFeatures x:type="e:{feature_kind}" name="value">{application}</eStructuralFeatures></eClassifiers>"##
        ),
    );
    let r = resolver(vec![s.clone()]);
    (s, r)
}
#[test]
fn raw_warning_exact_arity_kind_hints_and_primitive_positions_are_checked() {
    let (s, r) = application(r##"<eGenericType eClassifier="#//Box"/>"##, "EReference");
    let report = r.resolve_type(&id(&s, &[1, 0, 0])).unwrap();
    assert!(report.bindings[0].raw);
    assert_eq!(report.warnings[0].code, "raw_type");
    for (application, feature, valid) in [
        (
            r##"<eGenericType eClassifier="#//Box"><eTypeArguments/></eGenericType>"##,
            "EReference",
            true,
        ),
        (
            r##"<eGenericType eClassifier="#//Box"><eTypeArguments/><eTypeArguments/></eGenericType>"##,
            "EReference",
            false,
        ),
        (
            r##"<eGenericType eClassifier="e:EDataType #//Box"/>"##,
            "EReference",
            false,
        ),
        (
            r##"<eGenericType eClassifier="#//Box"/>"##,
            "EAttribute",
            false,
        ),
        (
            r##"<eGenericType eClassifier="e:EDataType http://www.eclipse.org/emf/2002/Ecore#//EInt"/>"##,
            "EAttribute",
            true,
        ),
        (
            r##"<eGenericType eClassifier="http://www.eclipse.org/emf/2002/Ecore#//EInt"/>"##,
            "EReference",
            false,
        ),
        (
            r##"<eGenericType eClassifier="#//Box"><eTypeArguments eClassifier="http://www.eclipse.org/emf/2002/Ecore#//EInt"/></eGenericType>"##,
            "EReference",
            false,
        ),
        (
            r##"<eGenericType eClassifier="#//Box"><eTypeArguments><eUpperBound eClassifier="http://www.eclipse.org/emf/2002/Ecore#//EInt"/></eTypeArguments></eGenericType>"##,
            "EReference",
            false,
        ),
        (
            r##"<eGenericType eClassifier="#//Box"><eTypeArguments eClassifier="http://www.eclipse.org/emf/2002/Ecore#//EIntegerObject"/></eGenericType>"##,
            "EReference",
            true,
        ),
        (
            r##"<eGenericType eClassifier="#//Box/T"/>"##,
            "EReference",
            false,
        ),
    ] {
        let (s, r) = crate::definitions_ecore_generic_resolution::application(application, feature);
        let result = r.resolve_type(&id(&s, &[1, 0, 0]));
        assert_eq!(result.is_ok(), valid, "{application}: {result:?}");
    }
}
#[test]
fn external_references_require_exact_dependencies_and_resolve_standard_identities() {
    let remote = source(
        "file:///types.ecore",
        r#"<eClassifiers x:type="e:EClass" name="Box"><eTypeParameters name="T"/></eClassifiers>"#,
    );
    let mut local = source(
        "file:///main.ecore",
        r##"<eClassifiers x:type="e:EClass" name="Holder"><eStructuralFeatures x:type="e:EReference" name="value"><eGenericType><eClassifier x:type="e:EClass" href="types.ecore#//Box"/><eTypeArguments eClassifier="http://www.eclipse.org/emf/2002/Ecore#//EString"/></eGenericType></eStructuralFeatures></eClassifiers>"##,
    );
    let selected = id(&local, &[0, 0, 0]);
    assert!(
        resolver(vec![local.clone(), remote.clone()])
            .resolve_type(&selected)
            .is_err()
    );
    local.dependencies.push(remote.id.clone());
    let report = resolver(vec![local.clone(), remote.clone()])
        .resolve_type(&selected)
        .unwrap();
    assert_eq!(report.bindings[0].target, id(&remote, &[0]));
    assert_eq!(
        report.bindings[1].target.source,
        standard_datatype_source().id
    );
    assert_eq!(report.bindings[1].target_kind, TargetKind::Datatype);
    let other = source(
        "file:///types.ecore",
        r#"<eClassifiers x:type="e:EClass" name="Box"><eTypeParameters name="U"/></eClassifiers>"#,
    );
    local.dependencies.push(other.id.clone());
    assert!(
        resolver(vec![local.clone(), remote, other])
            .resolve_type(&selected)
            .unwrap_err()
            .message
            .contains("Ambiguous")
    );
    local.text.push(' ');
    assert!(
        Resolver::new(&DefinitionBundle {
            version: 1,
            sources: vec![local],
            namespaces: vec![]
        })
        .is_err()
    );
}
#[test]
fn generic_owners_duplicate_formals_and_supertype_wildcards_reject() {
    for (body, path, valid) in [
        (
            r##"<eClassifiers x:type="e:EClass" name="Box"><eTypeParameters name="T"/></eClassifiers><eClassifiers x:type="e:EClass" name="C"><eGenericSuperTypes eClassifier="#//Box"><eTypeArguments/></eGenericSuperTypes></eClassifiers>"##,
            vec![1, 0],
            false,
        ),
        (
            r##"<eClassifiers x:type="e:EClass" name="C"><eGenericSuperTypes eClassifier="http://www.eclipse.org/emf/2002/Ecore#//EString"/></eClassifiers>"##,
            vec![0, 0],
            false,
        ),
        (
            r##"<eClassifiers x:type="e:EDataType" name="D"><eGenericSuperTypes eClassifier="#//C"/></eClassifiers><eClassifiers x:type="e:EClass" name="C"/>"##,
            vec![0, 0],
            false,
        ),
        (
            r##"<eClassifiers x:type="e:EClass" name="C"><eOperations name="op"><eGenericExceptions eClassifier="http://www.eclipse.org/emf/2002/Ecore#//EInvocationTargetException"/></eOperations></eClassifiers>"##,
            vec![0, 0, 0],
            true,
        ),
        (
            r##"<eClassifiers x:type="e:EClass" name="C"><eGenericType eClassifier="#//C"/></eClassifiers>"##,
            vec![0, 0],
            false,
        ),
        (
            r##"<eClassifiers x:type="e:EClass" name="C"><eStructuralFeatures x:type="e:EReference" name="f"><eGenericType eClassifier="#//C"/><eGenericType eClassifier="#//C"/></eStructuralFeatures></eClassifiers>"##,
            vec![0, 0, 0],
            false,
        ),
        (
            r##"<eClassifiers x:type="e:EClass" name="C"><eTypeParameters name="T"/><eTypeParameters name="T"/><eStructuralFeatures x:type="e:EReference" name="f"><eGenericType eClassifier="#//C"/></eStructuralFeatures></eClassifiers>"##,
            vec![0, 2, 0],
            false,
        ),
    ] {
        let s = source("memory:g", body);
        let result = resolver(vec![s.clone()]).resolve_type(&id(&s, &path));
        assert_eq!(result.is_ok(), valid, "{body}: {result:?}");
    }
}
#[test]
fn pinned_generic_signatures_resolve_with_finite_raw_projections() {
    fn visit(
        r: &Resolver,
        node: &intersect_model::definitions::ecore::Node,
        id: DeclarationId,
        count: &mut usize,
    ) {
        if node.name.namespace.is_none() && node.name.local == "eTypeParameters" {
            r.resolve_parameter(&id).unwrap();
            *count += 1;
        }
        if node.name.namespace.is_none()
            && matches!(
                node.name.local.as_str(),
                "eGenericType" | "eGenericSuperTypes" | "eGenericExceptions"
            )
        {
            r.resolve_type(&id).unwrap();
            *count += 1;
        }
        for (i, nested) in node.children.iter().enumerate() {
            let mut next = id.clone();
            next.path.push(i);
            visit(r, nested, next, count);
        }
    }
    let s = standard_datatype_source();
    let document = intersect_model::definitions::ecore::Document::parse(s.clone()).unwrap();
    let r = resolver(vec![s.clone()]);
    let mut count = 0;
    visit(&r, &document.root, id(&s, &[]), &mut count);
    assert_eq!(count, 11);
    let inventory = r.resolve_all().unwrap();
    assert_eq!(inventory.parameters.len(), 5);
    assert_eq!(inventory.types.len(), 6);
}

#[test]
fn complete_inventory_checks_unused_signatures_and_preserves_opaque_payloads() {
    let text = include_str!("../../../../fixtures/ecore-generics/signatures.ecore");
    let s = DefinitionSource::new("memory:generics", SourceKind::Ecore, text);
    let report = resolver(vec![s.clone()]).resolve_all().unwrap();
    assert_eq!(report.parameters.len(), 3);
    assert_eq!(report.types.len(), 5);
    assert_eq!(
        report.types.iter().map(|t| t.warnings.len()).sum::<usize>(),
        1
    );
    assert_eq!(report.sources, vec![s.id.clone()]);
    // A selected valid subtree cannot certify an invalid unused sibling.
    let bad = DefinitionSource::new(
        "memory:generics",
        SourceKind::Ecore,
        text.replace(
            "<eTypeParameters name=\"U\"/>",
            "<eTypeParameters name=\"U\"/><eTypeParameters name=\"U\"/>",
        ),
    );
    let r = resolver(vec![bad.clone()]);
    assert!(r.resolve_type(&id(&bad, &[1, 1, 0])).is_ok());
    assert!(r.resolve_all().unwrap_err().message.contains("Duplicate"));
    let opaque = source(
        "memory:opaque",
        r##"<eAnnotations source="extension"><contents><eTypeParameters name="invalid"><eBounds eClassifier="#//missing"/></eTypeParameters></contents></eAnnotations><ext:payload xmlns:ext="urn:extension"><eGenericType eClassifier="#//missing"/></ext:payload>"##,
    );
    let r = resolver(vec![opaque.clone()]).resolve_all().unwrap();
    assert!(r.types.is_empty());
    assert!(r.parameters.is_empty());
    assert_eq!(r.retained_payloads.len(), 2);
    let bundle = DefinitionBundle {
        version: 1,
        sources: vec![opaque],
        namespaces: vec![],
    };
    for span in r.retained_payloads {
        assert!(span.text(&bundle).unwrap().contains("#//missing"));
    }
}
#[test]
fn complete_inventory_is_source_order_independent_and_checks_nested_xmi_resources() {
    let a = source(
        "memory:a",
        r##"<eClassifiers x:type="e:EDataType" name="List"><eTypeParameters name="E"/></eClassifiers>"##,
    );
    let b = DefinitionSource::new(
        "memory:b",
        SourceKind::Ecore,
        r##"<x:XMI xmlns:x="http://www.omg.org/XMI" xmlns:e="http://www.eclipse.org/emf/2002/Ecore" xmlns:s="http://www.w3.org/2001/XMLSchema-instance"><e:EPackage name="b" nsURI="urn:b"><eSubpackages name="nested" nsURI="urn:nested"><eClassifiers s:type="e:EEnum" name="Enum"><eTypeParameters name="T"/></eClassifiers></eSubpackages></e:EPackage></x:XMI>"##,
    );
    let first = resolver(vec![a.clone(), b.clone()]).resolve_all().unwrap();
    assert_eq!(first, resolver(vec![b, a]).resolve_all().unwrap());
    assert_eq!(first.parameters.len(), 2);
    assert_eq!(
        first.parameters[1].syntax.declaration.path,
        vec![0, 0, 0, 0]
    );
    let malformed = source(
        "memory:bad",
        r##"<eClassifiers x:type="e:EClass" name="Owner"><eUpperBound eClassifier="#//Owner"/></eClassifiers>"##,
    );
    assert!(resolver(vec![malformed]).resolve_all().is_err());
}
