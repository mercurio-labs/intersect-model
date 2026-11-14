use intersect_model::definitions::{
    bundle::{DefinitionBundle, DefinitionSource, SourceKind},
    compiler::standard_datatype_source,
    generic_resolution::Resolver,
    resources::{DeclarationId, ResourceSet},
};
fn source(body: &str) -> DefinitionSource {
    DefinitionSource::new(
        "memory:agreement",
        SourceKind::Ecore,
        format!(
            r#"<e:EPackage xmlns:e="http://www.eclipse.org/emf/2002/Ecore" xmlns:x="http://www.w3.org/2001/XMLSchema-instance" name="g" nsURI="urn:g"><!-- λ 🧭 -->{body}</e:EPackage>"#
        ),
    )
}
fn resolver(s: &DefinitionSource) -> Resolver {
    Resolver::new(&DefinitionBundle {
        version: 1,
        sources: vec![s.clone()],
        namespaces: vec![],
    })
    .unwrap()
}
fn id(s: &DefinitionSource, path: &[usize]) -> DeclarationId {
    DeclarationId {
        source: s.id.clone(),
        path: path.to_vec(),
    }
}
fn standard(name: &str) -> DeclarationId {
    let s = standard_datatype_source();
    let r = ResourceSet::new(&DefinitionBundle {
        version: 1,
        sources: vec![s.clone()],
        namespaces: vec![],
    })
    .unwrap();
    r.resolve(&s.id, &format!("#//{name}")).unwrap()
}
#[test]
fn raw_parameter_derivation_selects_context_appropriate_bounds_and_defaults() {
    let s = source(
        r##"<eClassifiers x:type="e:EClass" name="Base"/>
 <eClassifiers x:type="e:EClass" name="Owner">
 <eTypeParameters name="T"><eBounds eClassifier="http://www.eclipse.org/emf/2002/Ecore#//EString"/><eBounds eClassifier="#//Base"/></eTypeParameters>
 <eTypeParameters name="U"/>
 <eStructuralFeatures x:type="e:EReference" name="ref"><eGenericType eTypeParameter="#//Owner/T"/></eStructuralFeatures>
 <eStructuralFeatures x:type="e:EAttribute" name="attr"><eGenericType eTypeParameter="#//Owner/T"/></eStructuralFeatures>
 <eOperations name="result"><eGenericType eTypeParameter="#//Owner/T"/></eOperations>
 <eStructuralFeatures x:type="e:EReference" name="unboundRef"><eGenericType eTypeParameter="#//Owner/U"/></eStructuralFeatures>
 <eStructuralFeatures x:type="e:EAttribute" name="unboundAttr"><eGenericType eTypeParameter="#//Owner/U"/></eStructuralFeatures>
 <eOperations name="unboundResult"><eGenericType eTypeParameter="#//Owner/U"/></eOperations>
 </eClassifiers>"##,
    );
    let r = resolver(&s);
    for (path, expected) in [
        (vec![1, 2, 0], id(&s, &[0])),
        (vec![1, 3, 0], standard("EString")),
        (vec![1, 4, 0], standard("EString")),
        (vec![1, 5, 0], standard("EObject")),
        (vec![1, 6, 0], standard("EJavaObject")),
        (vec![1, 7, 0], standard("EJavaObject")),
    ] {
        assert_eq!(r.resolve_type(&id(&s, &path)).unwrap().raw_type, expected);
    }
    assert!(r.resolve_all().is_ok());
}
#[test]
fn raw_derivation_keeps_f_bounds_chains_and_wildcards_distinct_from_substitution() {
    let s = source(
        r##"<eClassifiers x:type="e:EClass" name="Box"><eTypeParameters name="T"><eBounds eClassifier="#//Box"><eTypeArguments eTypeParameter="#//Box/T"/><eTypeArguments/></eBounds></eTypeParameters><eTypeParameters name="U"><eBounds eTypeParameter="#//Box/T"/></eTypeParameters><eOperations name="get"><eGenericType eTypeParameter="#//Box/U"/></eOperations><eStructuralFeatures x:type="e:EReference" name="value"><eGenericType eClassifier="#//Box"><eTypeArguments><eUpperBound eClassifier="http://www.eclipse.org/emf/2002/Ecore#//EString"/></eTypeArguments><eTypeArguments/></eGenericType></eStructuralFeatures></eClassifiers>"##,
    );
    let r = resolver(&s);
    assert_eq!(
        r.resolve_type(&id(&s, &[0, 2, 0])).unwrap().raw_type,
        id(&s, &[0])
    );
    assert_eq!(
        r.resolve_type(&id(&s, &[0, 3, 0, 0])).unwrap().raw_type,
        standard("EJavaObject")
    );
    assert!(r.resolve_all().is_ok());
}
#[test]
fn legacy_and_generic_type_agreement_preserves_hints_and_void_results() {
    let body = r##"<eClassifiers x:type="e:EClass" name="Base"/><eClassifiers x:type="e:EClass" name="Owner"><eTypeParameters name="T"><eBounds eClassifier="#//Base"/></eTypeParameters><eStructuralFeatures x:type="e:EReference" name="ref" eType="e:EClass #//Base"><eGenericType eTypeParameter="#//Owner/T"/></eStructuralFeatures><eOperations name="nothing"/><eOperations name="text"><eType x:type="e:EDataType" href="http://www.eclipse.org/emf/2002/Ecore#//EString"/><eGenericType eClassifier="e:EDataType http://www.eclipse.org/emf/2002/Ecore#//EString"/></eOperations></eClassifiers>"##;
    let s = source(body);
    let report = resolver(&s).resolve_all().unwrap();
    let member = report
        .raw_signatures
        .iter()
        .find(|r| r.owner == id(&s, &[1, 1]))
        .unwrap();
    assert_eq!(
        member.legacy.as_ref().unwrap()[0].kind_hint.as_deref(),
        Some("EClass")
    );
    assert_eq!(member.generic, vec![id(&s, &[1, 1, 0])]);
    assert_eq!(member.raw_types, vec![id(&s, &[0])]);
    let void = report
        .raw_signatures
        .iter()
        .find(|r| r.owner == id(&s, &[1, 2]) && r.feature == "eType")
        .unwrap();
    assert!(void.raw_types.is_empty());
    assert!(void.legacy.is_none());
    assert!(void.generic.is_empty());
    let bad = source(&body.replace("eType=\"e:EClass #//Base\"", "eType=\"e:EClass #//Owner\""));
    let error = resolver(&bad).resolve_all().unwrap_err();
    assert!(error.message.contains("disagrees"));
    let bundle = DefinitionBundle {
        version: 1,
        sources: vec![bad],
        namespaces: vec![],
    };
    assert_eq!(
        error.span.text(&bundle).unwrap(),
        "eType=\"e:EClass #//Owner\""
    );
}
#[test]
fn ordered_supertype_and_exception_views_reject_disagreement_and_wrong_kinds() {
    let body = r##"<eClassifiers x:type="e:EClass" name="A"/><eClassifiers x:type="e:EClass" name="B"/><eClassifiers x:type="e:EClass" name="C" eSuperTypes="e:EClass #//A"><eSuperTypes x:type="e:EClass" href="#//B"/><eGenericSuperTypes eClassifier="#//A"/><eGenericSuperTypes eClassifier="#//B"/><eOperations name="f" eExceptions="e:EClassifier #//A e:EClass #//B"><eGenericExceptions eClassifier="#//A"/><eGenericExceptions eClassifier="#//B"/></eOperations></eClassifiers>"##;
    let s = source(body);
    let report = resolver(&s).resolve_all().unwrap();
    let supers = report
        .raw_signatures
        .iter()
        .find(|r| r.owner == id(&s, &[2]))
        .unwrap();
    assert_eq!(supers.raw_types, vec![id(&s, &[0]), id(&s, &[1])]);
    for changed in [
        body.replace(
            "eSuperTypes=\"e:EClass #//A\"",
            "eSuperTypes=\"e:EClass #//B\"",
        ),
        body.replace(
            "eExceptions=\"e:EClassifier #//A e:EClass #//B\"",
            "eExceptions=\"e:EClass #//B e:EClass #//A\"",
        ),
        body.replace(
            "eSuperTypes=\"e:EClass #//A\"",
            "eSuperTypes=\"e:EDataType #//A\"",
        ),
    ] {
        let s = source(&changed);
        assert!(resolver(&s).resolve_all().is_err());
    }
}
#[test]
fn malformed_legacy_encodings_and_namespace_aliases_are_checked() {
    let base = r##"<eClassifiers x:type="e:EClass" name="A"><eStructuralFeatures x:type="e:EReference" name="r" ENCODING><eGenericType eClassifier="#//A"/>CHILD</eStructuralFeatures></eClassifiers>"##;
    for (encoding, child) in [
        ("eType=\"#//A #//A\"", ""),
        ("eType=\"#//A\"", "<eType href=\"#//A\"/>"),
        ("", "<eType href=\"#//A\">text</eType>"),
        ("", "<eType href=\"#//A\" other=\"bad\"/>"),
        ("eType=\"bad:EClass #//A\"", ""),
        ("eType=\"e:EEnum #//A\"", ""),
        ("eType=\"\"", ""),
    ] {
        let s = source(&base.replace("ENCODING", encoding).replace("CHILD", child));
        assert!(
            Resolver::new(&DefinitionBundle {
                version: 1,
                sources: vec![s],
                namespaces: vec![],
            })
            .and_then(|r| r.resolve_all())
            .is_err()
        );
    }
    let s = source(
        &base
            .replace(
                "ENCODING",
                "eType=\"other:EClass #//A\" xmlns:other=\"http://www.eclipse.org/emf/2002/Ecore\"",
            )
            .replace("CHILD", ""),
    );
    assert!(resolver(&s).resolve_all().is_ok());
}

#[test]
fn legacy_cross_resource_identity_requires_explicit_dependency() {
    let target = DefinitionSource::new(
        "https://example.test/base.ecore",
        SourceKind::Ecore,
        r#"<e:EPackage xmlns:e="http://www.eclipse.org/emf/2002/Ecore" xmlns:x="http://www.w3.org/2001/XMLSchema-instance" name="base" nsURI="urn:base"><eClassifiers x:type="e:EClass" name="Base"/></e:EPackage>"#,
    );
    let mut usage = DefinitionSource::new(
        "https://example.test/use.ecore",
        SourceKind::Ecore,
        r#"<e:EPackage xmlns:e="http://www.eclipse.org/emf/2002/Ecore" xmlns:x="http://www.w3.org/2001/XMLSchema-instance" name="use" nsURI="urn:use"><eClassifiers x:type="e:EClass" name="Owner"><eStructuralFeatures x:type="e:EReference" name="r" eType="e:EClass base.ecore#//Base"><eGenericType eClassifier="base.ecore#//Base"/></eStructuralFeatures></eClassifiers></e:EPackage>"#,
    );
    let make = |usage: DefinitionSource| DefinitionBundle {
        version: 1,
        sources: vec![target.clone(), usage],
        namespaces: vec![],
    };
    assert!(
        Resolver::new(&make(usage.clone()))
            .unwrap()
            .resolve_all()
            .is_err()
    );
    usage.dependencies.push(target.id.clone());
    let report = Resolver::new(&make(usage.clone()))
        .unwrap()
        .resolve_all()
        .unwrap();
    let member = report
        .raw_signatures
        .iter()
        .find(|r| r.owner == id(&usage, &[0, 0]))
        .unwrap();
    assert_eq!(member.raw_types, vec![id(&target, &[0])]);
}
#[test]
fn excessive_parameter_erasure_depth_is_an_explicit_resource_limit() {
    let mut body = String::from(
        r#"<eClassifiers x:type="e:EClass" name="Owner"><eTypeParameters name="P0"/>"#,
    );
    for i in 1..=129 {
        body.push_str(&format!(r##"<eTypeParameters name="P{i}"><eBounds eTypeParameter="#//Owner/P{}"/></eTypeParameters>"##,i-1));
    }
    body.push_str(r##"<eOperations name="result"><eGenericType eTypeParameter="#//Owner/P129"/></eOperations></eClassifiers>"##);
    let s = source(&body);
    let error = resolver(&s)
        .resolve_type(&id(&s, &[0, 130, 0]))
        .unwrap_err();
    assert_eq!(
        error.kind,
        intersect_model::definitions::ecore::ErrorKind::ResourceLimit
    );
    assert!(error.message.contains("128"));
}
