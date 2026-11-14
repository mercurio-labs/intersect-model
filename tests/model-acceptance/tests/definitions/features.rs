use intersect_model::definitions::{
    bundle::{DefinitionBundle, DefinitionSource, SourceKind},
    features::{FeatureResolver, FeatureTarget},
    resources::DeclarationId,
    scalars::Builtin,
};
const MAIN: &str = r##"<e:EPackage xmlns:e="http://www.eclipse.org/emf/2002/Ecore" xmlns:q="http://www.w3.org/2001/XMLSchema-instance" name="main" nsURI="urn:main">
<eClassifiers q:type="e:EClass" name="Owner">
<eStructuralFeatures q:type="e:EAttribute" name="title" eType="e:EDataType http://www.eclipse.org/emf/2002/Ecore#//EString"/>
<eStructuralFeatures q:type="e:EAttribute" name="count" eType="e:EDataType types.ecore#//Count"/>
<eStructuralFeatures q:type="e:EAttribute" name="mode" eType="e:EDataType types.ecore#//Mode"/>
<eStructuralFeatures q:type="e:EReference" name="target" eType="e:EClass types.ecore#//Target"/>
<eStructuralFeatures q:type="e:EAttribute" name="entries" eType="http://www.eclipse.org/emf/2002/Ecore#//EFeatureMapEntry"/>
<eStructuralFeatures q:type="e:EReference" name="object" eType="http://www.eclipse.org/emf/2002/Ecore#//EObject"/>
</eClassifiers></e:EPackage>"##;
const TYPES: &str = r##"<e:EPackage xmlns:e="http://www.eclipse.org/emf/2002/Ecore" xmlns:q="http://www.w3.org/2001/XMLSchema-instance" name="types" nsURI="urn:types">
<eClassifiers q:type="e:EDataType" name="Count" instanceClassName="int"/>
<eClassifiers q:type="e:EEnum" name="Mode"><eLiterals name="A"/></eClassifiers>
<eClassifiers q:type="e:EClass" name="Target"/>
</e:EPackage>"##;
fn bundle(text: &str) -> DefinitionBundle {
    let types = DefinitionSource::new("file:///model/types.ecore", SourceKind::Ecore, TYPES);
    let mut main = DefinitionSource::new("file:///model/main.ecore", SourceKind::Ecore, text);
    main.dependencies.push(types.id.clone());
    DefinitionBundle {
        version: 1,
        sources: vec![main, types],
        namespaces: vec![],
    }
}
fn id(bundle: &DefinitionBundle, source: usize, path: &[usize]) -> DeclarationId {
    DeclarationId {
        source: bundle.sources[source].id.clone(),
        path: path.to_vec(),
    }
}
#[test]
fn feature_resolution_preserves_exact_owner_kind_and_cross_source_types() {
    let bundle = bundle(MAIN);
    let resolver = FeatureResolver::new(&bundle).unwrap();
    for (index, expected, reference) in [
        (0, FeatureTarget::Builtin(Builtin::EString), false),
        (1, FeatureTarget::Datatype(id(&bundle, 1, &[0])), false),
        (2, FeatureTarget::Enumeration(id(&bundle, 1, &[1])), false),
        (3, FeatureTarget::Class(id(&bundle, 1, &[2])), true),
    ] {
        let declaration = id(&bundle, 0, &[0, index]);
        let resolved = resolver.resolve(&declaration).unwrap();
        assert_eq!(resolved.declaration, declaration);
        assert_eq!(resolved.owner, id(&bundle, 0, &[0]));
        assert_eq!(resolved.target, expected);
        assert_eq!(resolved.reference, reference);
    }
    assert!(matches!(
        resolver.resolve(&id(&bundle, 0, &[0, 4])).unwrap().target,
        FeatureTarget::Datatype(_)
    ));
    assert!(matches!(
        resolver.resolve(&id(&bundle, 0, &[0, 5])).unwrap().target,
        FeatureTarget::Class(_)
    ));
    // Definitions requiring a future provider can be resolved without pretending to execute them.
    assert_eq!(bundle.sources[0].text, MAIN);
}
#[test]
fn feature_resolution_rejects_missing_dependency_stale_identity_and_wrong_nodes() {
    let mut bundle = bundle(MAIN);
    bundle.sources[0].dependencies.clear();
    let resolver = FeatureResolver::new(&bundle).unwrap();
    assert!(
        resolver
            .resolve(&id(&bundle, 0, &[0, 1]))
            .unwrap_err()
            .contains("dependency")
    );
    for path in [vec![], vec![0], vec![99], vec![0, 99]] {
        assert!(resolver.resolve(&id(&bundle, 0, &path)).is_err());
    }
    let mut stale = id(&bundle, 0, &[0, 0]);
    stale.source.sha256 = "0".repeat(64);
    assert!(resolver.resolve(&stale).is_err());
}
#[test]
fn feature_resolution_rejects_kind_qname_and_generic_mismatches() {
    for (text, index) in [
        (MAIN.replace("e:EDataType types.ecore#//Count","e:EClass types.ecore#//Count"),1),
        (MAIN.replace("q:type=\"e:EReference\" name=\"target\"","q:type=\"e:EAttribute\" name=\"target\""),3),
        (MAIN.replace("q:type=\"e:EAttribute\" name=\"title\"","q:type=\"e:EReference\" name=\"title\""),0),
        (MAIN.replace("q:type=\"e:EClass\" name=\"Owner\"","q:type=\"e:EEnum\" name=\"Owner\""),0),
        (MAIN.replace("eType=\"e:EDataType types.ecore#//Count\"/>","eType=\"e:EDataType types.ecore#//Count\"><eGenericType eClassifier=\"types.ecore#//Mode\"/></eStructuralFeatures>"),1),
        (MAIN.replace("eType=\"e:EDataType types.ecore#//Count\"",""),1),
        (MAIN.replace("e:EDataType types.ecore#//Count","e:EOperation types.ecore#//Count"),1),
    ] {
        let bundle = bundle(&text); let resolver = FeatureResolver::new(&bundle).unwrap();
        assert!(resolver.resolve(&id(&bundle,0,&[0,index])).is_err(),"{text}");
    }
}

#[test]
fn concrete_generic_feature_adapters_share_type_checks_and_identity() {
    let text=MAIN.replace("eType=\"e:EDataType types.ecore#//Count\"/>","eType=\"e:EDataType types.ecore#//Count\"><eGenericType eClassifier=\"types.ecore#//Count\"/></eStructuralFeatures>");
    let baseline = bundle(MAIN);
    let input = bundle(&text);
    let expected = FeatureResolver::new(&baseline)
        .unwrap()
        .resolve(&id(&baseline, 0, &[0, 1]))
        .unwrap();
    let actual = FeatureResolver::new(&input)
        .unwrap()
        .resolve(&id(&input, 0, &[0, 1]))
        .unwrap();
    assert_eq!(actual.target, expected.target);
    assert_eq!(actual.declaration, id(&input, 0, &[0, 1]));
    let text = text.replace(" eType=\"e:EDataType types.ecore#//Count\"", "");
    let input = bundle(&text);
    assert_eq!(
        FeatureResolver::new(&input)
            .unwrap()
            .resolve(&id(&input, 0, &[0, 1]))
            .unwrap()
            .target,
        expected.target
    );
}
