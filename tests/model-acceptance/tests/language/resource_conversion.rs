use intersect_model::definitions::{
    bundle::{DefinitionBundle, DefinitionSource, SourceKind},
    compiler::standard_datatype,
    conversion::{ConversionContract, ConversionProvider, ConversionRegistry, ProviderId},
    resources::DeclarationId,
};
use intersect_model::resource_conversion::{ResourceConverter, ResourceKind};
use serde_json::json;
fn source(uri: &str, name: &str) -> DefinitionSource {
    DefinitionSource::new(
        uri,
        SourceKind::Ecore,
        format!(
            "<e:EPackage xmlns:e='http://www.eclipse.org/emf/2002/Ecore' xmlns:xsi='http://www.w3.org/2001/XMLSchema-instance' name='{name}' nsURI='urn:{name}'><eClassifiers xsi:type='e:EClass' name='Item'/></e:EPackage>"
        ),
    )
}
fn setup() -> DefinitionBundle {
    let b = source("https://example.test/b.ecore", "b");
    let mut a = source("https://example.test/a.ecore", "a");
    a.dependencies.push(b.id.clone());
    DefinitionBundle {
        version: 1,
        sources: vec![a, b],
        namespaces: vec![],
    }
}
fn contract(bundle: &DefinitionBundle, kind: ResourceKind) -> ConversionContract {
    let name = if kind == ResourceKind::Resource {
        "EResource"
    } else {
        "EResourceSet"
    };
    let mut declarations: Vec<_> = bundle
        .sources
        .iter()
        .map(|s| DeclarationId {
            source: s.id.clone(),
            path: vec![],
        })
        .collect();
    declarations.sort();
    ConversionContract {
        members: Vec::new(),
        declarations,
        datatype: standard_datatype(name).unwrap().declaration().clone(),
        provider: ProviderId {
            name: "test.resource".into(),
            version: "1".into(),
        },
        serializable: false,
    }
}
#[test]
fn exact_resource_contents_and_cross_resource_resolution_use_explicit_members_and_dependencies() {
    let bundle = setup();
    let a = &bundle.sources[0].id;
    let b = &bundle.sources[1].id;
    let single = ResourceConverter::new(
        contract(&bundle, ResourceKind::Resource),
        ResourceKind::Resource,
        &bundle,
    )
    .unwrap();
    let handle = json!({"resource":a});
    assert_eq!(
        single.document(&handle, a).unwrap().source,
        bundle.sources[0]
    );
    assert_eq!(
        single.resolve(&handle, a, "#//Item").unwrap(),
        DeclarationId {
            source: a.clone(),
            path: vec![0]
        }
    );
    assert!(
        single
            .resolve(&handle, a, "b.ecore#//Item")
            .unwrap_err()
            .contains("not a member")
    );
    let set = ResourceConverter::new(
        contract(&bundle, ResourceKind::Set),
        ResourceKind::Set,
        &bundle,
    )
    .unwrap();
    let handle = json!({"resources":[b,a]});
    assert_eq!(set.members(&handle).unwrap(), [b.clone(), a.clone()]);
    assert_eq!(
        set.resolve(&handle, a, "b.ecore#//Item").unwrap(),
        DeclarationId {
            source: b.clone(),
            path: vec![0]
        }
    );
    assert!(set.resolve(&handle, b, "a.ecore#//Item").is_err()); // No reverse dependency.
    assert!(set.resolve(&handle, a, "#//Missing").is_err());
    assert!(
        set.resolve(&json!({"resources":[b]}), a, "#//Item")
            .is_err()
    );
    assert!(set.document(&json!({"resources":[a]}), b).is_err());
    assert!(
        set.resolve(&handle, a, "https://not-loaded.test/remote.ecore#//Item")
            .is_err()
    );
    let mut revised = bundle.clone();
    let next = source(&b.uri, "changed");
    revised.sources.push(next.clone());
    let both = ResourceConverter::new(
        contract(&revised, ResourceKind::Set),
        ResourceKind::Set,
        &revised,
    )
    .unwrap();
    let handle = json!({"resources":[a,b,next.id]});
    assert_eq!(
        both.resolve(&handle, a, "b.ecore#//Item").unwrap().source,
        *b
    );
    revised.sources[0].dependencies.push(next.id.clone());
    let ambiguous = ResourceConverter::new(
        contract(&revised, ResourceKind::Set),
        ResourceKind::Set,
        &revised,
    )
    .unwrap();
    assert!(
        ambiguous
            .resolve(&handle, a, "b.ecore#//Item")
            .unwrap_err()
            .contains("Ambiguous")
    );
}
#[test]
fn scopes_and_handles_reject_wrong_roots_kinds_revisions_duplicates_and_opaque_payloads() {
    let bundle = setup();
    let good = contract(&bundle, ResourceKind::Set);
    let adapter = ResourceConverter::new(good.clone(), ResourceKind::Set, &bundle).unwrap();
    let a = &bundle.sources[0].id;
    let mut stale = a.clone();
    stale.sha256 = "changed".into();
    for payload in [
        json!(null),
        json!({"resource":a}),
        json!({"resources":[a,a]}),
        json!({"resources":[stale]}),
        json!({"resources":[],"javaObject":1}),
        json!({"resources":[{"uri":a.uri,"sha256":a.sha256,"extra":true}]}),
    ] {
        assert!(adapter.validate(&payload).is_err(), "{payload}");
    }
    for mutation in 0..5 {
        let mut bad = good.clone();
        match mutation {
            0 => bad.declarations[0].path = vec![0],
            1 => bad.declarations[0].source.sha256 = "stale".into(),
            2 => bad.declarations.reverse(),
            3 => bad.declarations.push(bad.declarations[1].clone()),
            _ => bad.serializable = true,
        };
        assert!(ResourceConverter::new(bad, ResourceKind::Set, &bundle).is_err());
    }
    let mut non_ecore = bundle.clone();
    let text = DefinitionSource::new("grammar.xtext", SourceKind::Xtext, "grammar example.X");
    non_ecore.sources.push(text.clone());
    let mut bad = good.clone();
    bad.declarations = vec![DeclarationId {
        source: text.id,
        path: vec![],
    }];
    assert!(ResourceConverter::new(bad, ResourceKind::Set, &non_ecore).is_err());
    for kind in [ResourceKind::Resource, ResourceKind::Set] {
        let mut empty = contract(&bundle, kind);
        empty.declarations.clear();
        let adapter = ResourceConverter::new(empty, kind, &bundle).unwrap();
        assert_eq!(
            adapter.validate(&json!({"resources":[]})).is_ok(),
            kind == ResourceKind::Set
        );
        assert!(adapter.validate(&json!({"resource":a})).is_err());
    }
}
#[test]
fn serializability_and_exact_provider_context_are_required_for_both_domains() {
    let bundle = setup();
    for kind in [ResourceKind::Resource, ResourceKind::Set] {
        let contract = contract(&bundle, kind);
        let datatype = standard_datatype(if kind == ResourceKind::Resource {
            "EResource"
        } else {
            "EResourceSet"
        })
        .unwrap();
        assert!(!datatype.serializable());
        let adapter = ResourceConverter::new(contract, kind, &bundle).unwrap();
        let mut registry = ConversionRegistry::default();
        registry.register(&datatype, &adapter).unwrap();
        let payload = if kind == ResourceKind::Resource {
            json!({"resource":bundle.sources[0].id})
        } else {
            json!({"resources":[]})
        };
        let value = registry
            .bind(datatype.declaration(), payload.clone())
            .unwrap();
        registry.validate(&value).unwrap();
        assert!(registry.format(&value).is_err());
        assert!(registry.parse(datatype.declaration(), "anything").is_err());
        assert!(adapter.parse("anything").is_err());
        assert!(adapter.format(&payload).is_err());
        assert_eq!(adapter.default_value().unwrap(), None);
        assert!(ConversionRegistry::default().validate(&value).is_err());
    }
}
