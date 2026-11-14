use intersect_model::definitions::{
    bundle::{DefinitionBundle, DefinitionSource, NamespaceBinding, SourceKind},
    resources::ResourceSet,
};

fn source(uri: &str, body: &str) -> DefinitionSource {
    DefinitionSource::new(uri, SourceKind::Ecore, body)
}
fn package(name: &str, uri: &str, body: &str) -> String {
    format!(
        "<e:EPackage xmlns:e='http://www.eclipse.org/emf/2002/Ecore' xmlns:xsi='http://www.w3.org/2001/XMLSchema-instance' name='{name}' nsURI='{uri}'>{body}</e:EPackage>"
    )
}
fn class(name: &str) -> String {
    format!("<eClassifiers xsi:type='e:EClass' name='{name}'/>")
}
fn bundle(sources: Vec<DefinitionSource>) -> DefinitionBundle {
    DefinitionBundle {
        version: 1,
        sources,
        namespaces: vec![],
    }
}

#[test]
fn named_structural_and_namespace_paths_identify_the_same_nested_declaration() {
    let a = source(
        "file:///models/a.ecore",
        &package(
            "a",
            "urn:a",
            &format!(
                "<eSubpackages name='nested' nsURI='urn:nested'>{}</eSubpackages>",
                class("C")
            ),
        ),
    );
    let mut b = bundle(vec![a.clone()]);
    b.namespaces.push(NamespaceBinding {
        namespace: "urn:nested".into(),
        source: a.id.clone(),
    });
    let set = ResourceSet::new(&b).unwrap();
    let named = set.resolve(&a.id, "#//nested/C").unwrap();
    assert_eq!(
        named,
        set.resolve(&a.id, "#//@eSubpackages.0/@eClassifiers.0")
            .unwrap()
    );
    assert_eq!(named, set.resolve(&a.id, "urn:nested#//C").unwrap());
    assert_eq!(set.node(&named).unwrap().lexical_name, "eClassifiers");
    assert_eq!(named.source, a.id);
}

#[test]
fn cyclic_resources_are_indexed_before_resolution_and_order_is_irrelevant() {
    let mut a = source(
        "platform:/resource/project/a.ecore",
        &package("a", "urn:a", &class("C")),
    );
    let mut b = source(
        "platform:/resource/project/sub/b.ecore",
        &package("b", "urn:b", &class("C")),
    );
    a.dependencies.push(b.id.clone());
    b.dependencies.push(a.id.clone());
    let set = ResourceSet::new(&bundle(vec![a.clone(), b.clone()])).unwrap();
    let reversed = ResourceSet::new(&bundle(vec![b.clone(), a.clone()])).unwrap();
    let target = set.resolve(&a.id, "sub/../sub/b.ecore#//C").unwrap();
    assert_eq!(target.source, b.id);
    assert_eq!(target, reversed.resolve(&a.id, "sub/b.ecore#//C").unwrap());
    assert_eq!(set.resolve(&b.id, "../a.ecore#//C").unwrap().source, a.id);
    assert_ne!(set.resolve(&a.id, "#//C").unwrap(), target);
}

#[test]
fn revisions_require_explicit_dependency_selection_and_namespace_bindings() {
    let old = source("file:///b.ecore", &package("b", "urn:b", &class("Old")));
    let new = source("file:///b.ecore", &package("b", "urn:b", &class("New")));
    let mut a = source("file:///a.ecore", &package("a", "urn:a", &class("A")));
    a.dependencies.push(new.id.clone());
    let mut b = bundle(vec![a.clone(), old.clone(), new.clone()]);
    b.namespaces.push(NamespaceBinding {
        namespace: "urn:b".into(),
        source: new.id.clone(),
    });
    let set = ResourceSet::new(&b).unwrap();
    assert_eq!(set.resolve(&a.id, "b.ecore#//New").unwrap().source, new.id);
    assert_eq!(set.resolve(&a.id, "urn:b#//New").unwrap().source, new.id);
    assert!(set.resolve(&a.id, "b.ecore#//Old").is_err());
    b.sources[0].dependencies.push(old.id.clone());
    assert!(
        ResourceSet::new(&b)
            .unwrap()
            .resolve(&a.id, "b.ecore#//New")
            .unwrap_err()
            .contains("Ambiguous")
    );
    b.sources[0].dependencies.clear();
    assert!(
        ResourceSet::new(&b)
            .unwrap()
            .resolve(&a.id, "urn:b#//New")
            .unwrap_err()
            .contains("dependency")
    );
}

#[test]
fn multiple_roots_ids_and_encoded_names_are_source_distinct() {
    let text = format!(
        "<xmi:XMI xmlns:xmi='http://www.omg.org/XMI'>{}{}</xmi:XMI>",
        package(
            "a",
            "urn:a",
            "<eClassifiers xsi:type='e:EClass' name='café' xmi:id='class-id'/>"
        ),
        package("b", "urn:b", &class("C"))
    );
    let a = source("file:///multi.ecore", &text);
    let set = ResourceSet::new(&bundle(vec![a.clone()])).unwrap();
    let id = set.resolve(&a.id, "#class-id").unwrap();
    assert_eq!(id, set.resolve(&a.id, "#/0/caf%C3%A9").unwrap());
    assert_eq!(id, set.resolve(&a.id, "#/0/@eClassifiers.0").unwrap());
    assert_ne!(id, set.resolve(&a.id, "#/1/C").unwrap());
    assert!(set.resolve(&a.id, "#//C").is_err());
}

#[test]
fn duplicate_and_missing_identities_are_errors() {
    for body in [
        format!("{}{}", class("C"), class("C")),
        "<eSubpackages name='x' nsURI='urn:x'/><eSubpackages name='x' nsURI='urn:y'/>".into(),
        "<eSubpackages name='x' nsURI='urn:a'/>".into(),
    ] {
        let a = source("file:///a.ecore", &package("a", "urn:a", &body));
        assert!(ResourceSet::new(&bundle(vec![a])).is_err());
    }
    let a = source("file:///a.ecore", &package("a", "urn:a", &class("C")));
    let mut b = bundle(vec![a.clone()]);
    let set = ResourceSet::new(&b).unwrap();
    for href in ["missing.ecore#//C", "#//Missing", "#%FF", "#%xy"] {
        assert!(set.resolve(&a.id, href).is_err(), "{href}");
    }
    b.namespaces.push(NamespaceBinding {
        namespace: "urn:wrong".into(),
        source: a.id,
    });
    assert!(ResourceSet::new(&b).is_err());
}

#[test]
fn uri_queries_and_normalization_do_not_change_exact_source_identity() {
    let mut a = source(
        "https://EXAMPLE.org/models/a.ecore?rev=one",
        &package("a", "urn:a", &class("A")),
    );
    let b = source(
        "https://example.org/models/b.ecore?rev=two",
        &package("b", "urn:b", &class("B")),
    );
    a.dependencies.push(b.id.clone());
    let set = ResourceSet::new(&bundle(vec![a.clone(), b.clone()])).unwrap();
    assert_eq!(
        set.resolve(&a.id, "./b.ecore?rev=two#//B").unwrap().source,
        b.id
    );
    assert_eq!(
        set.resolve(&a.id, "https://EXAMPLE.org/models/b.ecore?rev=two#//B")
            .unwrap()
            .source,
        b.id
    );
    assert!(set.resolve(&a.id, "b.ecore?rev=one#//B").is_err());
    assert_eq!(set.resolve(&a.id, "#//A").unwrap().source, a.id);
}

#[test]
fn duplicate_ids_and_ambiguous_named_segments_never_choose_a_first_match() {
    let a = source(
        "file:///a.ecore",
        &package(
            "a",
            "urn:a",
            "<eClassifiers xmlns:xmi='http://www.omg.org/XMI' xsi:type='e:EClass' name='C' xmi:id='dup'/><eClassifiers xmlns:xmi='http://www.omg.org/XMI' xsi:type='e:EClass' name='D' xmi:id='dup'/>",
        ),
    );
    assert!(
        ResourceSet::new(&bundle(vec![a]))
            .unwrap_err()
            .contains("Duplicate XMI ID")
    );
    let a = source(
        "file:///a.ecore",
        &package(
            "a",
            "urn:a",
            "<eClassifiers xsi:type='e:EClass' name='C'/><eSubpackages name='C' nsURI='urn:child'/>",
        ),
    );
    let set = ResourceSet::new(&bundle(vec![a.clone()])).unwrap();
    assert!(
        set.resolve(&a.id, "#//C")
            .unwrap_err()
            .contains("Ambiguous")
    );
    assert_ne!(
        set.resolve(&a.id, "#//@eClassifiers.0").unwrap(),
        set.resolve(&a.id, "#//@eSubpackages.0").unwrap()
    );
}

#[test]
fn pinned_upstream_package_indexes_retain_their_exact_documents() {
    for text in [
        include_str!(
            "../../../../fixtures/definition-conformance/upstream/ecore/model/Ecore.ecore"
        ),
        include_str!(
            "../../../../fixtures/definition-conformance/upstream/xtext/org/eclipse/xtext/Xtext.ecore"
        ),
    ] {
        let a = source("file:///upstream.ecore", text);
        let set = ResourceSet::new(&bundle(vec![a.clone()])).unwrap();
        assert_eq!(set.document(&a.id).unwrap().source, a);
        assert!(!set.packages(&a.id).unwrap().is_empty());
    }
}

#[test]
fn index_diagnostics_point_to_the_external_source_and_duplicate_attribute() {
    let a = source("file:///a.ecore", &package("a", "urn:a", &class("A")));
    let b = source(
        "file:///b.ecore",
        &package("b", "urn:b", "<eSubpackages name='sub' nsURI='urn:b'/>"),
    );
    let error = ResourceSet::new_diagnostic(&bundle(vec![a.clone(), b.clone()])).unwrap_err();
    assert_eq!(error.span.source, b.id);
    assert_eq!(&b.text[error.span.start..error.span.end], "nsURI='urn:b'");
    let malformed = source("file:///broken.ecore", "<a><b></a>");
    let error = ResourceSet::new_diagnostic(&bundle(vec![a, malformed.clone()])).unwrap_err();
    assert_eq!(error.span.source, malformed.id);
    assert_eq!(&malformed.text[error.span.start..error.span.end], "</a>");
}
