use intersect_model::definitions::{
    bundle::{DefinitionSource, SourceKind},
    ecore::{Document, ErrorKind, ParseLimits},
};
fn source(size: usize) -> DefinitionSource {
    let prefix = "<e:EPackage xmlns:e='http://www.eclipse.org/emf/2002/Ecore' name='m' nsURI='urn:limits'><!--";
    let suffix = "--></e:EPackage>";
    DefinitionSource::new(
        "memory:large.ecore",
        SourceKind::Ecore,
        format!(
            "{prefix}{}{suffix}",
            "x".repeat(size - prefix.len() - suffix.len())
        ),
    )
}
#[test]
fn explicit_source_policy_preserves_bytes_identity_and_default_boundary() {
    let source = source(629_610);
    let limits = ParseLimits {
        source_bytes: source.text.len(),
    };
    let original = source.clone();
    let document = Document::parse_with_limits(source.clone(), limits).unwrap();
    assert_eq!(document.source, original);
    assert_eq!(document.root.span.source, original.id);
    assert_eq!(document.root.span.end, original.text.len());
    let rejected = Document::parse(source.clone()).unwrap_err();
    assert_eq!(rejected.kind, ErrorKind::ResourceLimit);
    assert_eq!(rejected.message, "Definition size budget exceeded");
    let rejected = Document::parse_with_limits(
        source,
        ParseLimits {
            source_bytes: limits.source_bytes - 1,
        },
    )
    .unwrap_err();
    assert_eq!(rejected.kind, ErrorKind::ResourceLimit);
    assert!(Document::parse(self::source(256_000)).is_ok());
    assert!(Document::parse(self::source(256_001)).is_err());
}
#[test]
fn larger_policy_does_not_accept_digest_mismatch_or_relax_xml_nesting() {
    let mut wrong = source(629_610);
    wrong.text.push(' ');
    assert_eq!(
        Document::parse_with_limits(
            wrong,
            ParseLimits {
                source_bytes: 700_000
            }
        )
        .unwrap_err()
        .kind,
        ErrorKind::InvalidDefinition
    );
    let deep = DefinitionSource::new(
        "memory:depth",
        SourceKind::Ecore,
        format!("{}{}", "<node>".repeat(65), "</node>".repeat(65)),
    );
    let default = Document::parse(deep.clone()).unwrap_err();
    let explicit = Document::parse_with_limits(
        deep,
        ParseLimits {
            source_bytes: 700_000,
        },
    )
    .unwrap_err();
    assert_eq!(default, explicit);
    assert_eq!(explicit.message, "XML nesting budget exceeded");
}

#[test]
fn policy_reaches_dependency_resources_and_preserves_resolved_type_identity() {
    use intersect_model::definitions::{
        bundle::DefinitionBundle,
        generic_resolution::Resolver,
        resources::{DeclarationId, ResourceSet},
    };
    let large = DefinitionSource::new(
        "memory:b",
        SourceKind::Ecore,
        format!(
            "<e:EPackage xmlns:e='http://www.eclipse.org/emf/2002/Ecore' xmlns:x='http://www.w3.org/2001/XMLSchema-instance' name='b' nsURI='urn:b'><eClassifiers x:type='e:EClass' name='B'/><!--{}--></e:EPackage>",
            "x".repeat(300_000)
        ),
    );
    let mut entry = DefinitionSource::new(
        "memory:a",
        SourceKind::Ecore,
        "<e:EPackage xmlns:e='http://www.eclipse.org/emf/2002/Ecore' xmlns:x='http://www.w3.org/2001/XMLSchema-instance' name='a' nsURI='urn:a'><eClassifiers x:type='e:EClass' name='A'><eStructuralFeatures x:type='e:EReference' name='value'><eGenericType eClassifier='memory:b#//B'/></eStructuralFeatures></eClassifiers></e:EPackage>",
    );
    entry.dependencies.push(large.id.clone());
    let mut bundle = DefinitionBundle {
        version: 1,
        sources: vec![entry.clone(), large.clone()],
        namespaces: vec![],
    };
    let limits = ParseLimits {
        source_bytes: large.text.len(),
    };
    for _ in 0..2 {
        let resources = ResourceSet::new_with_limits(&bundle, limits).unwrap();
        assert_eq!(resources.document(&large.id).unwrap().source, large);
        let target = resources.resolve(&entry.id, "memory:b#//B").unwrap();
        assert_eq!(
            target,
            DeclarationId {
                source: large.id.clone(),
                path: vec![0]
            }
        );
        let resolver = Resolver::new_with_limits(&bundle, limits).unwrap();
        let resolved = resolver
            .resolve_type(&DeclarationId {
                source: entry.id.clone(),
                path: vec![0, 0, 0],
            })
            .unwrap();
        assert_eq!(resolved.bindings[0].target, target);
        for error in [
            ResourceSet::new_diagnostic(&bundle).unwrap_err(),
            Resolver::new(&bundle).unwrap_err(),
            ResourceSet::new_with_limits(
                &bundle,
                ParseLimits {
                    source_bytes: limits.source_bytes - 1,
                },
            )
            .unwrap_err(),
            Resolver::new_with_limits(
                &bundle,
                ParseLimits {
                    source_bytes: limits.source_bytes - 1,
                },
            )
            .unwrap_err(),
        ] {
            assert_eq!(error.kind, ErrorKind::ResourceLimit);
            assert_eq!(error.span.source, large.id);
            assert_eq!(error.message, "Definition size budget exceeded");
        }
        // Runtime policy uses authored Rcore; the XML checks above remain
        // independent import/inspection coverage.
        {
            let large = DefinitionSource::new(
                "memory:b.rcore",
                SourceKind::Rcore,
                format!(
                    "rcore 1; package b version \"1\"; metamodel \"urn:b\" prefix b {{ class B {{}} }} //{}",
                    "x".repeat(300_000)
                ),
            );
            let mut entry = DefinitionSource::new(
                "memory:a.rcore",
                SourceKind::Rcore,
                "rcore 1; package a version \"1\"; metamodel \"urn:a\" prefix a { class A { ref value: external \"memory:b.rcore#//B\" [0,1]; } }",
            );
            entry.dependencies.push(large.id.clone());
            let limits = ParseLimits {
                source_bytes: large.text.len(),
            };
            let mut bundle = DefinitionBundle {
                version: 1,
                sources: vec![entry.clone(), large.clone()],
                namespaces: vec![],
            };
            let compiled =
                intersect_model::definitions::compiler::compile_structure_bundle_with_limits(
                    &bundle, &entry.id, "urn:a", limits,
                )
                .unwrap();
            assert_eq!(compiled.classes.len(), 2);
            assert!(compiled.classes.contains_key("A"));
            assert!(
                intersect_model::definitions::compiler::compile_structure_bundle(
                    &bundle, &entry.id, "urn:a"
                )
                .is_err()
            );
            let denied =
                intersect_model::definitions::compiler::compile_structure_bundle_with_limits(
                    &bundle,
                    &entry.id,
                    "urn:a",
                    ParseLimits {
                        source_bytes: limits.source_bytes - 1,
                    },
                )
                .unwrap_err();
            assert_eq!(denied.kind, ErrorKind::ResourceLimit);
            assert_eq!(denied.span.source, large.id);
            bundle.sources.reverse();
            assert_eq!(
                compiled,
                intersect_model::definitions::compiler::compile_structure_bundle_with_limits(
                    &bundle, &entry.id, "urn:a", limits,
                )
                .unwrap()
            );
        }
        bundle.sources.reverse();
    }
    bundle
        .sources
        .iter_mut()
        .find(|source| source.id == large.id)
        .unwrap()
        .text
        .push(' ');
    assert!(
        ResourceSet::new_with_limits(
            &bundle,
            ParseLimits {
                source_bytes: limits.source_bytes + 1
            }
        )
        .is_err()
    );
}
