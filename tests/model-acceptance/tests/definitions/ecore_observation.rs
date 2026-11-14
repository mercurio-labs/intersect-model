use intersect_model::definitions::{
    bundle::{DefinitionBundle, DefinitionSource, NamespaceBinding, SourceKind},
    conversion::{InvocationContext, InvocationObserver, ProviderId},
    ecore::{Document, ObservedImportError},
};
use std::{cell::Cell, rc::Rc};

struct Probe {
    storage: Cell<usize>,
    calls: Cell<usize>,
    work: Cell<usize>,
    max_work: usize,
    max_storage: usize,
    cancel_at: Option<usize>,
}
impl Probe {
    fn new(max_storage: usize, cancel_at: Option<usize>) -> Rc<Self> {
        Self::with_work(max_storage, usize::MAX, cancel_at)
    }
    fn with_work(max_storage: usize, max_work: usize, cancel_at: Option<usize>) -> Rc<Self> {
        Rc::new(Self {
            storage: Cell::new(0),
            calls: Cell::new(0),
            work: Cell::new(0),
            max_work,
            max_storage,
            cancel_at,
        })
    }
    fn touch(&self) -> Result<(), String> {
        let call = self.calls.get();
        self.calls.set(call + 1);
        if self.cancel_at == Some(call) {
            Err("cancelled by XML control".into())
        } else {
            Ok(())
        }
    }
}
impl InvocationObserver for Probe {
    fn before_invoke(&self, _: &ProviderId) -> Result<(), String> {
        panic!("XML parsing must not invoke a provider")
    }
    fn checkpoint(&self) -> Result<(), String> {
        self.touch()
    }
    fn work(&self, units: usize) -> Result<(), String> {
        self.touch()?;
        let total = self.work.get() + units;
        if total > self.max_work {
            return Err("XML work denied".into());
        }
        self.work.set(total);
        Ok(())
    }
    fn retain_storage(&self, bytes: usize) -> Result<(), String> {
        self.touch()?;
        let total = self.storage.get() + bytes;
        if total > self.max_storage {
            return Err("XML storage denied".into());
        }
        self.storage.set(total);
        Ok(())
    }
}
fn observed(source: &DefinitionSource, probe: &Rc<Probe>) -> Result<Document, ObservedImportError> {
    Document::parse_observed(source, &InvocationContext::new(probe.clone()))
}

#[test]
fn materialized_raw_operations_preserve_templates_and_recover_from_late_interruptions() {
    use intersect_model::definitions::{
        compiler,
        generic_types::{RuntimeType, Type},
        resources::DeclarationId,
    };
    let source = DefinitionSource::new(
        "memory:ops",
        SourceKind::Ecore,
        "<e:EPackage xmlns:e='http://www.eclipse.org/emf/2002/Ecore' xmlns:x='http://www.w3.org/2001/XMLSchema-instance' name='ops' nsURI='urn:ops'><eClassifiers x:type='e:EClass' name='Box'><eTypeParameters name='T'/><eOperations name='convert'><eTypeParameters name='U'/><eGenericType eTypeParameter='#//Box/convert/U'/><eParameters name='input'><eGenericType eTypeParameter='#//Box/T'/></eParameters><eParameters name='mapped'><eGenericType eTypeParameter='#//Box/convert/U'/></eParameters></eOperations></eClassifiers></e:EPackage>",
    );
    let (source, _) = intersect_model_acceptance::migration_fixture::migrate_source_fixture(source);
    let bundle = DefinitionBundle {
        version: 1,
        sources: vec![source.clone()],
        namespaces: vec![],
    };
    let receiver = RuntimeType::new(Type::Classifier {
        declaration: DeclarationId {
            source: source.id.clone(),
            path: vec![0],
        },
        arguments: vec![],
        raw: true,
    })
    .unwrap();
    let run = |probe: Rc<Probe>| {
        compiler::materialize_runtime_bundle_observed(
            &bundle,
            &source.id,
            "urn:ops",
            std::slice::from_ref(&receiver),
            None,
            &InvocationContext::new(probe),
        )
    };
    let full = Probe::new(usize::MAX, None);
    let original = run(full.clone()).unwrap();
    assert_eq!(
        original,
        compiler::materialize_runtime_bundle(
            &bundle,
            &source.id,
            "urn:ops",
            std::slice::from_ref(&receiver),
            None
        )
        .unwrap()
    );
    let key = original.class_key_for_type(&receiver).unwrap();
    let group = &original.declared_operation_selections.as_ref().unwrap()[key].groups[0];
    assert_eq!(group.signature.name, "convert");
    assert_eq!(group.signature.parameters.len(), 2);
    assert_eq!(
        group.selected,
        intersect_model_acceptance::migration_fixture::named_declaration_fixture(
            &source,
            "Box/convert"
        )
    );
    assert_eq!(group.operations.len(), 1);
    let operation = &group.operations[0];
    assert_eq!(
        operation
            .operation
            .parameters
            .iter()
            .map(|parameter| parameter.name.as_str())
            .collect::<Vec<_>>(),
        vec!["input", "mapped"]
    );
    assert!(operation.template.is_some());
    assert!(operation.projection.is_some());
    assert!(operation.operation.generic);
    assert!(
        operation
            .raw_parameters
            .iter()
            .all(|parameter| parameter.feature == "eType")
    );
    for offset in [1, 2, 30, 150] {
        assert_eq!(
            run(Probe::new(usize::MAX, Some(full.calls.get() - offset))).unwrap_err(),
            ObservedImportError::Observation("cancelled by XML control".into())
        );
        assert_eq!(run(Probe::new(usize::MAX, None)).unwrap(), original);
    }
    assert_eq!(
        run(Probe::new(full.storage.get() - 1, None)).unwrap_err(),
        ObservedImportError::Observation("XML storage denied".into())
    );
    assert_eq!(
        run(Probe::with_work(
            full.storage.get(),
            full.work.get() - 1,
            None
        ))
        .unwrap_err(),
        ObservedImportError::Observation("XML work denied".into())
    );
    assert_eq!(
        run(Probe::with_work(full.storage.get(), full.work.get(), None)).unwrap(),
        original
    );
}
fn source(text: &str) -> DefinitionSource {
    DefinitionSource::new("memory:x", SourceKind::Ecore, text)
}

#[test]
fn resolution_retains_exact_literal_identity_fragment_and_work_fees() {
    use intersect_model::definitions::resources::ResourceSet;
    let mut bundle = DefinitionBundle {
        version: 1,
        sources: vec![source(
            "<e:EPackage xmlns:e='http://www.eclipse.org/emf/2002/Ecore' xmlns:x='http://www.w3.org/2001/XMLSchema-instance' xmlns:m='http://www.omg.org/XMI' name='a' nsURI='urn:a'><eClassifiers x:type='e:EClass' name='C' m:id='é'/></e:EPackage>",
        )],
        namespaces: vec![],
    };
    bundle.namespaces.push(NamespaceBinding {
        namespace: "urn:a".into(),
        source: bundle.sources[0].id.clone(),
    });
    let set = ResourceSet::new_diagnostic(&bundle).unwrap();
    let id = &bundle.sources[0].id;
    // SourceId 727; empty root DeclarationId 1253; decoded string 64+UTF-8
    // bytes; appended path slot 32. XMI lookup copies a [0] DeclarationId
    // (1413). Namespace lookup borrows the source instead of its 727-byte copy.
    for (href, bytes, work) in [
        ("#//C", 2077, 30),
        ("#//@eClassifiers.0", 2091, 28),
        ("#%C3%A9", 2206, 28),
        ("urn:a#//C", 1350, 23),
    ] {
        let expected = set.resolve(id, href).unwrap();
        assert_eq!(expected.path, vec![0]);
        let full = Probe::with_work(bytes, work, None);
        assert_eq!(
            set.resolve_observed(id, href, &InvocationContext::new(full.clone()))
                .unwrap(),
            expected
        );
        assert_eq!((full.storage.get(), full.work.get()), (bytes, work));
        for call in 0..full.calls.get() {
            assert_eq!(
                set.resolve_observed(
                    id,
                    href,
                    &InvocationContext::new(Probe::new(usize::MAX, Some(call)))
                )
                .unwrap_err(),
                ObservedImportError::Observation("cancelled by XML control".into())
            );
        }
        for limit in [bytes - 1, bytes, bytes + 1] {
            let result =
                set.resolve_observed(id, href, &InvocationContext::new(Probe::new(limit, None)));
            if limit < bytes {
                assert_eq!(
                    result.unwrap_err(),
                    ObservedImportError::Observation("XML storage denied".into())
                );
            } else {
                assert_eq!(result.unwrap(), expected);
            }
        }
        for limit in [work - 1, work, work + 1] {
            let result = set.resolve_observed(
                id,
                href,
                &InvocationContext::new(Probe::with_work(bytes, limit, None)),
            );
            if limit < work {
                assert_eq!(
                    result.unwrap_err(),
                    ObservedImportError::Observation("XML work denied".into())
                );
            } else {
                assert_eq!(result.unwrap(), expected);
            }
        }
    }
    for prefix in [727, 1980, 2045, 2077] {
        assert_eq!(
            set.resolve_observed(
                id,
                "#//C",
                &InvocationContext::new(Probe::new(prefix - 1, None))
            )
            .unwrap_err(),
            ObservedImportError::Observation("XML storage denied".into())
        );
    }
}

#[test]
fn relative_resolution_guards_uri_formatting_and_dependency_scans() {
    use intersect_model::definitions::resources::ResourceSet;
    let base = DefinitionSource::new(
        "file:///base.ecore",
        SourceKind::Ecore,
        "<e:EPackage xmlns:e='http://www.eclipse.org/emf/2002/Ecore' xmlns:x='http://www.w3.org/2001/XMLSchema-instance' name='b' nsURI='urn:base'><eClassifiers x:type='e:EClass' name='C'/></e:EPackage>",
    );
    let mut usage = DefinitionSource::new(
        "file:///use.ecore",
        SourceKind::Ecore,
        "<e:EPackage xmlns:e='http://www.eclipse.org/emf/2002/Ecore' name='u' nsURI='urn:use'/>",
    );
    usage.dependencies.push(base.id.clone());
    let bundle = DefinitionBundle {
        version: 1,
        sources: vec![usage.clone(), base.clone()],
        namespaces: vec![],
    };
    let set = ResourceSet::new_diagnostic(&bundle).unwrap();
    // Relative address formatting/normalization: two strings 82 each.
    // Two candidate URI strings: 82+81. Target source:737, root:1263,
    // decoded C:65, path slot:32. Total 2424; 48 explicit work units.
    let full = Probe::with_work(2424, 48, None);
    let expected = set
        .resolve_observed(
            &usage.id,
            "base.ecore#//C",
            &InvocationContext::new(full.clone()),
        )
        .unwrap();
    assert_eq!(expected.source, base.id);
    assert_eq!(expected.path, vec![0]);
    assert_eq!((full.storage.get(), full.work.get()), (2424, 48));
    // Independent retention prefixes: address, normalized address, base/use
    // candidate URIs, selected source/root, decoded C, appended path slot.
    for prefix in [82, 164, 246, 327, 1064, 2327, 2392, 2424] {
        assert_eq!(
            set.resolve_observed(
                &usage.id,
                "base.ecore#//C",
                &InvocationContext::new(Probe::new(prefix - 1, None))
            )
            .unwrap_err(),
            ObservedImportError::Observation("XML storage denied".into())
        );
    }
    for call in 0..full.calls.get() {
        assert_eq!(
            set.resolve_observed(
                &usage.id,
                "base.ecore#//C",
                &InvocationContext::new(Probe::new(usize::MAX, Some(call)))
            )
            .unwrap_err(),
            ObservedImportError::Observation("cancelled by XML control".into())
        );
    }
    for limit in [2423, 2424, 2425] {
        let result = set.resolve_observed(
            &usage.id,
            "base.ecore#//C",
            &InvocationContext::new(Probe::new(limit, None)),
        );
        if limit == 2423 {
            assert_eq!(
                result.unwrap_err(),
                ObservedImportError::Observation("XML storage denied".into())
            );
        } else {
            assert_eq!(result.unwrap(), expected);
        }
    }
    for limit in [47, 48, 49] {
        let result = set.resolve_observed(
            &usage.id,
            "base.ecore#//C",
            &InvocationContext::new(Probe::with_work(2424, limit, None)),
        );
        if limit == 47 {
            assert_eq!(
                result.unwrap_err(),
                ObservedImportError::Observation("XML work denied".into())
            );
        } else {
            assert_eq!(result.unwrap(), expected);
        }
    }
}

#[test]
fn observed_resolution_preserves_normal_messages_without_reclassifying_observer_failures() {
    use intersect_model::definitions::resources::ResourceSet;
    let bundle = resource_bundle();
    let set = ResourceSet::new_diagnostic(&bundle).unwrap();
    let id = &bundle.sources[0].id;
    for href in [
        "#//missing",
        "#missing",
        "#/%FF",
        "#//@eClassifiers.1",
        "#/bad",
        "other.ecore#//C",
        "http://example.test/absent#//C",
        "#/%",
        "#/%xx",
    ] {
        let expected = set.resolve(id, href).unwrap_err();
        let error = set
            .resolve_observed(
                id,
                href,
                &InvocationContext::new(Probe::new(usize::MAX, None)),
            )
            .unwrap_err();
        let ObservedImportError::Import(error) = error else {
            panic!("ordinary URI failure became an observer failure")
        };
        assert_eq!(error.message, expected);
        assert_eq!(error.span.source, *id);
        assert_eq!((error.span.start, error.span.end), (0, 0));
    }
}

fn resource_bundle() -> DefinitionBundle {
    DefinitionBundle {
        version: 1,
        sources: vec![source(
            "<e:EPackage xmlns:e='http://www.eclipse.org/emf/2002/Ecore' name='a' nsURI='urn:a'/>",
        )],
        namespaces: vec![],
    }
}

#[test]
fn resource_ownership_has_independent_literal_prefixes_and_work() {
    use intersect_model::definitions::resources::ResourceSet;
    let bundle = resource_bundle();
    let parser = Probe::new(usize::MAX, None);
    observed(&bundle.sources[0], &parser).unwrap();
    let parsed_bytes = parser.storage.get();
    // Resource-only policy: two 328-byte spans, four 128-byte records;
    // root declaration 1253 + slot 32, resource 128; package key 69,
    // declaration 1253 + map slot 128; ID traversal declaration 1253;
    // resource key 727 + map slot 128. Total: 6139.
    let needed = parsed_bytes + 6139;
    let full = Probe::new(needed, None);
    let set = ResourceSet::new_observed(&bundle, &InvocationContext::new(full.clone())).unwrap();
    assert_eq!(full.storage.get(), needed);
    // Resource traversals/validation: 18; metadata visits: 32;
    // copying declaration/source bounds: 3*5 + 3 = 18.
    assert_eq!(full.work.get(), parser.work.get() + 68);
    assert_eq!(
        set.packages(&bundle.sources[0].id).unwrap()["urn:a"].path,
        Vec::<usize>::new()
    );
    let prefixes = [
        328,
        456,
        584,
        712,
        840,
        1168,
        parsed_bytes + 2421,
        parsed_bytes + 2453,
        parsed_bytes + 2581,
        parsed_bytes + 2650,
        parsed_bytes + 3903,
        parsed_bytes + 4031,
        parsed_bytes + 5284,
        parsed_bytes + 6011,
        needed,
    ];
    for prefix in prefixes {
        let denied = Probe::new(prefix - 1, None);
        assert_eq!(
            ResourceSet::new_observed(&bundle, &InvocationContext::new(denied)).unwrap_err(),
            ObservedImportError::Observation("XML storage denied".into())
        );
    }
    for call in 0..full.calls.get() {
        assert_eq!(
            ResourceSet::new_observed(
                &bundle,
                &InvocationContext::new(Probe::new(usize::MAX, Some(call)))
            )
            .unwrap_err(),
            ObservedImportError::Observation("cancelled by XML control".into())
        );
    }
    for limit in [needed, needed + 1] {
        assert_eq!(
            ResourceSet::new_observed(&bundle, &InvocationContext::new(Probe::new(limit, None)))
                .unwrap()
                .document(&bundle.sources[0].id)
                .unwrap()
                .source,
            bundle.sources[0]
        );
    }
    let empty = DefinitionBundle {
        version: 1,
        sources: vec![],
        namespaces: vec![],
    };
    for limit in [460, 461, 462] {
        let result =
            ResourceSet::new_observed(&empty, &InvocationContext::new(Probe::new(limit, None)));
        assert_eq!(result.is_ok(), limit >= 461);
    }
    let mut invalid_empty = empty;
    invalid_empty.version = 2;
    let error = ResourceSet::new_diagnostic(&invalid_empty).unwrap_err();
    assert_eq!(
        error.span.source,
        DefinitionSource::new("memory:bundle", SourceKind::Ecore, "").id
    );
}

#[test]
fn observed_resource_graph_preserves_nested_xmi_ids_bindings_and_cycles() {
    use intersect_model::definitions::resources::ResourceSet;
    let mut base = DefinitionSource::new(
        "file:///base.ecore",
        SourceKind::Ecore,
        "<m:XMI xmlns:m='http://www.omg.org/XMI' xmlns:e='http://www.eclipse.org/emf/2002/Ecore' xmlns:x='http://www.w3.org/2001/XMLSchema-instance'><e:EPackage name='unused' nsURI='urn:empty'/><e:EPackage name='base' nsURI='urn:base'><eSubpackages name='nested' nsURI='urn:n'><eClassifiers x:type='e:EClass' name='C' m:id='é'/></eSubpackages></e:EPackage></m:XMI>",
    );
    let mut usage = DefinitionSource::new(
        "file:///use.ecore",
        SourceKind::Ecore,
        "<e:EPackage xmlns:e='http://www.eclipse.org/emf/2002/Ecore' name='use' nsURI='urn:use'/>",
    );
    base.dependencies.push(usage.id.clone());
    usage.dependencies.push(base.id.clone());
    let bundle = DefinitionBundle {
        version: 1,
        sources: vec![usage.clone(), base.clone()],
        namespaces: vec![NamespaceBinding {
            namespace: "urn:n".into(),
            source: base.id.clone(),
        }],
    };
    let reference = ResourceSet::new_diagnostic(&bundle).unwrap();
    let full = Probe::new(usize::MAX, None);
    let set = ResourceSet::new_observed(&bundle, &InvocationContext::new(full.clone())).unwrap();
    for address in ["base.ecore#%C3%A9", "base.ecore#/1/nested/C", "urn:n#//C"] {
        let id = set.resolve(&usage.id, address).unwrap();
        assert_eq!(id, reference.resolve(&usage.id, address).unwrap());
        assert_eq!(id.source, base.id);
        assert_eq!(id.path, vec![1, 0, 0]);
    }
    assert_eq!(set.document(&base.id).unwrap().source, base);
    for call in 0..full.calls.get() {
        assert_eq!(
            ResourceSet::new_observed(
                &bundle,
                &InvocationContext::new(Probe::new(usize::MAX, Some(call)))
            )
            .unwrap_err(),
            ObservedImportError::Observation("cancelled by XML control".into())
        );
    }
    assert_eq!(
        ResourceSet::new_observed(
            &bundle,
            &InvocationContext::new(Probe::new(full.storage.get(), None))
        )
        .unwrap()
        .document(&usage.id)
        .unwrap()
        .source,
        usage
    );
}

#[test]
fn resource_failures_keep_exact_import_diagnostics_and_recover() {
    use intersect_model::definitions::resources::ResourceSet;
    let valid = resource_bundle();
    let mut duplicate = valid.clone();
    duplicate.sources.push(duplicate.sources[0].clone());
    let mut missing = valid.clone();
    missing.sources[0].dependencies.push(source("<a/>").id);
    let mut binding = valid.clone();
    binding.namespaces.push(NamespaceBinding {
        namespace: "urn:missing".into(),
        source: binding.sources[0].id.clone(),
    });
    let mut inputs = vec![duplicate, missing, binding];
    for body in [
        "<a/>",
        "<e:EPackage xmlns:e='http://www.eclipse.org/emf/2002/Ecore' name='a'/>",
        "<e:EPackage xmlns:e='http://www.eclipse.org/emf/2002/Ecore' xmlns:m='http://www.omg.org/XMI' name='a' nsURI='urn:a'><eClassifiers name='A' m:id='x'/><eClassifiers name='B' m:id='x'/></e:EPackage>",
        "<e:EPackage xmlns:e='http://www.eclipse.org/emf/2002/Ecore' name='a' nsURI='urn:a'><eSubpackages name='b' nsURI='urn:a'/></e:EPackage>",
    ] {
        inputs.push(DefinitionBundle {
            version: 1,
            sources: vec![source(body)],
            namespaces: vec![],
        });
    }
    for input in inputs {
        let original = ResourceSet::new_diagnostic(&input).unwrap_err();
        assert_eq!(
            ResourceSet::new_observed(
                &input,
                &InvocationContext::new(Probe::new(usize::MAX, None))
            )
            .unwrap_err(),
            ObservedImportError::Import(original)
        );
        assert!(matches!(
            ResourceSet::new_observed(&input, &InvocationContext::new(Probe::new(0, None))),
            Err(ObservedImportError::Observation(_))
        ));
    }
    assert!(
        ResourceSet::new_observed(
            &valid,
            &InvocationContext::new(Probe::new(usize::MAX, None))
        )
        .is_ok()
    );
}

#[test]
fn compiler_resource_phase_keeps_exact_selection_receipts_and_schema() {
    use intersect_model::definitions::{compiler, conversion::ConversionRegistry};
    use sha2::{Digest, Sha256};
    let source = DefinitionSource::new(
        "memory:observed.rcore",
        SourceKind::Rcore,
        "rcore 1; package a version \"1\"; metamodel \"urn:a\" prefix a { class C {} }",
    );
    let bundle = DefinitionBundle {
        version: 1,
        sources: vec![source.clone()],
        namespaces: vec![],
    };
    let registry = ConversionRegistry::default();
    let run = |probe| {
        compiler::compile_structure_bundle_with_conversions_observed(
            &bundle,
            &source.id,
            "urn:a",
            &registry,
            &InvocationContext::new(probe),
        )
    };
    let full = Probe::new(usize::MAX, None);
    let actual = run(full.clone()).unwrap();
    let expected = compiler::compile_structure_bundle_with_conversions(
        &bundle, &source.id, "urn:a", &registry,
    )
    .unwrap();
    assert_eq!(actual, expected);
    // Independently assemble the documented canonical revision payload.
    let bytes = serde_json::to_vec(&(
        &source.id,
        "urn:a",
        vec![(
            &source.id,
            Vec::<intersect_model::definitions::bundle::SourceId>::new(),
        )],
        Vec::<NamespaceBinding>::new(),
    ))
    .unwrap();
    assert_eq!(actual.revision, format!("{:x}", Sha256::digest(bytes)));
    // Aggregate Rcore boundaries complement the independent XML parser receipts
    // above; these measurements do not claim an independent allocation derivation.
    for limit in [0, full.storage.get() / 2, full.storage.get() - 1] {
        assert_eq!(
            run(Probe::new(limit, None)).unwrap_err(),
            ObservedImportError::Observation("XML storage denied".into())
        );
    }
    for limit in [0, full.work.get() / 2, full.work.get() - 1] {
        assert_eq!(
            run(Probe::with_work(usize::MAX, limit, None)).unwrap_err(),
            ObservedImportError::Observation("XML work denied".into())
        );
    }
    assert_eq!(
        run(Probe::with_work(full.storage.get(), full.work.get(), None)).unwrap(),
        expected
    );
    for call in 0..full.calls.get() {
        assert_eq!(
            run(Probe::new(usize::MAX, Some(call))).unwrap_err(),
            ObservedImportError::Observation("cancelled by XML control".into())
        );
    }
    let original_error = compiler::compile_structure_bundle_with_conversions(
        &bundle,
        &source.id,
        "urn:missing",
        &registry,
    )
    .unwrap_err();
    assert_eq!(
        compiler::compile_structure_bundle_with_conversions_observed(
            &bundle,
            &source.id,
            "urn:missing",
            &registry,
            &InvocationContext::new(Probe::new(usize::MAX, None))
        )
        .unwrap_err(),
        ObservedImportError::Import(original_error)
    );
}

#[test]
fn observed_compiler_keeps_opaque_keys_and_sorted_reachable_revision_identity() {
    use intersect_model::definitions::{
        compiler, conversion::ConversionRegistry, schema::ClassifierDeclaration,
    };
    use sha2::{Digest, Sha256};
    let package = |label: &str| {
        DefinitionSource::new(
            format!("file:///{label}.ecore"),
            SourceKind::Ecore,
            format!(
                "<e:EPackage xmlns:e='http://www.eclipse.org/emf/2002/Ecore' xmlns:x='http://www.w3.org/2001/XMLSchema-instance' name='{label}' nsURI='urn:{label}'><eClassifiers x:type='e:EClass' name='C'/></e:EPackage>"
            ),
        )
    };
    let mut a = package("a");
    let b = package("b");
    let c = package("c");
    let unused = package("unused");
    a.dependencies = vec![c.id.clone(), b.id.clone()];
    let bundle = DefinitionBundle {
        version: 1,
        sources: vec![c.clone(), unused.clone(), a.clone(), b.clone()],
        namespaces: vec![
            NamespaceBinding {
                namespace: "urn:unused".into(),
                source: unused.id.clone(),
            },
            NamespaceBinding {
                namespace: "urn:c".into(),
                source: c.id.clone(),
            },
            NamespaceBinding {
                namespace: "urn:b".into(),
                source: b.id.clone(),
            },
        ],
    };
    let report = intersect_model_acceptance::migration_fixture::migrate_resources_fixture(bundle);
    let a = intersect_model_acceptance::migration_fixture::converted_source_fixture(&report, &a.id);
    let b = intersect_model_acceptance::migration_fixture::converted_source_fixture(&report, &b.id);
    let c = intersect_model_acceptance::migration_fixture::converted_source_fixture(&report, &c.id);
    let unused = intersect_model_acceptance::migration_fixture::converted_source_fixture(
        &report, &unused.id,
    );
    let bundle = report.converted.unwrap();
    let registry = ConversionRegistry::default();
    let run = |bundle: &DefinitionBundle, probe: Rc<Probe>| {
        compiler::compile_structure_bundle_with_conversions_observed(
            bundle,
            &a.id,
            "urn:a",
            &registry,
            &InvocationContext::new(probe),
        )
    };
    let full = Probe::new(usize::MAX, None);
    let schema = run(&bundle, full.clone()).unwrap();
    // Independent assembly of the pre-existing revision contract; unreachable
    // sources/bindings are excluded, dependency/source/binding order is sorted.
    let revision_json = serde_json::to_vec(&(
        &a.id,
        "urn:a",
        vec![
            (&a.id, vec![&b.id, &c.id]),
            (&b.id, vec![]),
            (&c.id, vec![]),
        ],
        vec![&bundle.namespaces[2], &bundle.namespaces[1]],
    ))
    .unwrap();
    assert_eq!(
        schema.revision,
        format!("{:x}", Sha256::digest(revision_json))
    );
    assert_eq!(schema.classes.len(), 3);
    assert_eq!(schema.declarations["C"].source, a.id);
    for (source, namespace) in [(&b, "urn:b"), (&c, "urn:c")] {
        let expected = ClassifierDeclaration {
            source: source.id.clone(),
            path: vec![0],
            namespace: namespace.into(),
            name: "C".into(),
        };
        let key = format!(
            "__decl_{:x}",
            Sha256::digest(serde_json::to_vec(&expected).unwrap())
        );
        assert_eq!(schema.declarations[&key], expected);
        assert_eq!(schema.classes[&key].name, key);
    }
    assert!(
        !schema
            .declarations
            .values()
            .any(|decl| decl.source == unused.id)
    );
    let expected = serde_json::to_value(&schema).unwrap();
    let mut reordered = bundle.clone();
    reordered.sources.reverse();
    reordered.namespaces.reverse();
    reordered
        .sources
        .iter_mut()
        .find(|source| source.id == a.id)
        .unwrap()
        .dependencies
        .reverse();
    assert_eq!(
        serde_json::to_value(run(&reordered, Probe::new(usize::MAX, None)).unwrap()).unwrap(),
        expected
    );
    // Measured aggregate boundaries complement the literal component receipts;
    // these are not an independent allocation-size derivation for this fixture.
    for (storage, work, message) in [
        (full.storage.get() - 1, usize::MAX, "XML storage denied"),
        (usize::MAX, full.work.get() - 1, "XML work denied"),
    ] {
        assert_eq!(
            run(&bundle, Probe::with_work(storage, work, None)).unwrap_err(),
            ObservedImportError::Observation(message.into())
        );
        assert_eq!(
            serde_json::to_value(
                run(
                    &bundle,
                    Probe::with_work(full.storage.get(), full.work.get(), None)
                )
                .unwrap()
            )
            .unwrap(),
            expected
        );
    }
    assert_eq!(bundle.sources[2].dependencies, vec![c.id, b.id]);
}

#[test]
fn literal_parser_storage_precedes_every_owned_component_and_recovers() {
    let input = source("<a/>");
    // Source envelope: 13 shared retained nodes (1664) + 124 metadata bytes.
    // Static struct field names and the unit enum use their shared unquoted policy.
    // Parser: 392 scratch/record, three 328-byte spans, a 128-byte node,
    // and two 65-byte strings. EOF has its own diagnostic span.
    let prefixes = [0, 1788, 2180, 2508, 2636, 2964, 3029, 3094, 3422];
    let reference = Document::parse(input.clone()).unwrap();
    let full = Probe::new(3422, None);
    let doc = observed(&input, &full).unwrap();
    assert_eq!(full.storage.get(), 3422);
    // Thirteen metadata visits, seven source-copy bound units, three source
    // passes, two events and one QName.
    assert_eq!(full.work.get(), 26);
    assert_eq!(format!("{doc:?}"), format!("{reference:?}"));
    assert_eq!(doc.root.name.local, "a");
    assert_eq!(doc.root.opening.source, input.id);
    assert_eq!((doc.root.span.start, doc.root.span.end), (0, 4));
    for prefix in prefixes.into_iter().skip(1) {
        let denied = Probe::new(prefix - 1, None);
        assert_eq!(
            observed(&input, &denied).unwrap_err(),
            ObservedImportError::Observation("XML storage denied".into())
        );
        assert!(prefixes.contains(&denied.storage.get()));
    }
    for limit in [3422, 3423] {
        assert_eq!(
            observed(&input, &Probe::new(limit, None)).unwrap().source,
            input
        );
    }
    for call in 0..full.calls.get() {
        let cancelled = Probe::new(usize::MAX, Some(call));
        assert_eq!(
            observed(&input, &cancelled).unwrap_err(),
            ObservedImportError::Observation("cancelled by XML control".into())
        );
        assert_eq!(cancelled.calls.get(), call + 1);
        assert_eq!(
            observed(&input, &Probe::new(3422, None)).unwrap().source,
            input
        );
    }
    assert_eq!(input.text, "<a/>");
    for limit in [25, 26, 27] {
        let probe = Rc::new(Probe {
            storage: Cell::new(0),
            calls: Cell::new(0),
            work: Cell::new(0),
            max_work: limit,
            max_storage: 3422,
            cancel_at: None,
        });
        let result = observed(&input, &probe);
        if limit < 26 {
            assert_eq!(
                result.unwrap_err(),
                ObservedImportError::Observation("XML work denied".into())
            );
        } else {
            assert_eq!(result.unwrap().source, input);
        }
    }
}

#[test]
fn observed_source_copy_uses_shared_shape_limits_without_changing_compatibility() {
    let mut input = source("<a/>");
    input.dependencies = (0..13_108).map(|_| input.id.clone()).collect();
    assert_eq!(Document::parse(input.clone()).unwrap().source, input);
    assert_eq!(
        Document::parse_observed(&input, &InvocationContext::default())
            .unwrap()
            .source,
        input
    );
    assert!(matches!(
        observed(&input, &Probe::new(usize::MAX, None)),
        Err(ObservedImportError::Observation(_))
    ));
    assert_eq!(
        observed(&source("<a/>"), &Probe::new(3422, None))
            .unwrap()
            .root
            .name
            .local,
        "a"
    );
}

#[test]
fn namespace_attributes_entities_and_payloads_preserve_source_under_interruption() {
    let input = source(
        "<?xml version='1.0'?><!--é--><e:a xmlns:e='urn:é' xmlns:x='http://www.w3.org/2001/XMLSchema-instance' x:type='e:T' label='é&#10;&quot;'><child/>text&amp;<![CDATA[x]]><?do x?></e:a>",
    );
    let reference = Document::parse(input.clone()).unwrap();
    let full = Probe::new(usize::MAX, None);
    let doc = observed(&input, &full).unwrap();
    assert_eq!(format!("{doc:?}"), format!("{reference:?}"));
    assert_eq!(doc.root.name.namespace.as_deref(), Some("urn:é"));
    assert_eq!(
        doc.root.attributes[2].type_name.as_ref().unwrap().local,
        "T"
    );
    assert_eq!(doc.root.attributes[3].value, "é\n\"");
    assert_eq!(doc.root.children[0].opening.source, input.id);
    assert_eq!(doc.root.payloads.len(), 4);
    assert_eq!(doc.misc.len(), 2);
    for call in 0..full.calls.get() {
        assert_eq!(
            observed(&input, &Probe::new(usize::MAX, Some(call))).unwrap_err(),
            ObservedImportError::Observation("cancelled by XML control".into())
        );
    }
    let denied = Probe::new(full.storage.get() - 1, None);
    assert_eq!(
        observed(&input, &denied).unwrap_err(),
        ObservedImportError::Observation("XML storage denied".into())
    );
    assert_eq!(
        format!(
            "{:?}",
            observed(&input, &Probe::new(full.storage.get(), None)).unwrap()
        ),
        format!("{reference:?}")
    );
}

#[test]
fn import_failures_keep_exact_diagnostics_and_never_become_observer_failures() {
    let mut inputs = vec![
        source("<a>"),
        source("<!DOCTYPE a><a/>"),
        source("<a x='1' x='2'/>"),
        source("<a xmlns:p='u' xmlns:q='u' p:x='1' q:x='2'/>"),
        source("<p:a/>"),
        source("<a>&unknown;</a>"),
        source(&"<a/>".repeat(64_001)),
        source(&format!("{}{}", "<a>".repeat(64), "</a>".repeat(64))),
    ];
    let mut forged = source("<a/>");
    forged.id.sha256 = "forged".into();
    inputs.push(forged);
    for input in inputs {
        let original = Document::parse(input.clone()).unwrap_err();
        assert_eq!(
            observed(&input, &Probe::new(usize::MAX, None)).unwrap_err(),
            ObservedImportError::Import(original)
        );
        assert_eq!(
            observed(&input, &Probe::new(0, None)).unwrap_err(),
            ObservedImportError::Observation("XML storage denied".into())
        );
        assert_eq!(
            Document::parse_observed(&input, &InvocationContext::default()).unwrap_err(),
            ObservedImportError::Import(Document::parse(input).unwrap_err())
        );
    }
}

#[test]
fn explicit_source_policy_preserves_observer_receipts_and_cancellation() {
    use intersect_model::definitions::ecore::ParseLimits;
    let source = DefinitionSource::new(
        "memory:policy",
        SourceKind::Ecore,
        "<e:EPackage xmlns:e='http://www.eclipse.org/emf/2002/Ecore' name='m' nsURI='urn:m'/>",
    );
    let baseline = Probe::new(usize::MAX, None);
    let expanded = Probe::new(usize::MAX, None);
    let original = observed(&source, &baseline).unwrap();
    let selected = Document::parse_observed_with_limits(
        &source,
        &InvocationContext::new(expanded.clone()),
        ParseLimits {
            source_bytes: 700_000,
        },
    )
    .unwrap();
    assert_eq!(original.source, selected.source);
    assert_eq!(
        (
            baseline.storage.get(),
            baseline.work.get(),
            baseline.calls.get()
        ),
        (
            expanded.storage.get(),
            expanded.work.get(),
            expanded.calls.get()
        )
    );
    let large = DefinitionSource::new(
        "memory:large-policy",
        SourceKind::Ecore,
        format!(
            "<e:EPackage xmlns:e='http://www.eclipse.org/emf/2002/Ecore'><!--{}--></e:EPackage>",
            "x".repeat(629_610)
        ),
    );
    let limits = ParseLimits {
        source_bytes: large.text.len(),
    };
    let accepted = Probe::new(usize::MAX, None);
    assert_eq!(
        Document::parse_observed_with_limits(
            &large,
            &InvocationContext::new(accepted.clone()),
            limits
        )
        .unwrap()
        .source,
        large
    );
    for probe in [
        Probe::new(0, None),
        Probe::with_work(usize::MAX, 0, None),
        Probe::new(usize::MAX, Some(0)),
        Probe::new(usize::MAX, Some(accepted.calls.get() - 1)),
    ] {
        assert!(matches!(
            Document::parse_observed_with_limits(&large, &InvocationContext::new(probe), limits),
            Err(ObservedImportError::Observation(_))
        ));
    }
    assert!(
        matches!(Document::parse_observed(&large,&InvocationContext::default()),Err(ObservedImportError::Import(error)) if error.kind==intersect_model::definitions::ecore::ErrorKind::ResourceLimit)
    );
}

#[test]
fn explicit_resource_policy_keeps_default_receipts_and_observed_resolver_guards() {
    use intersect_model::definitions::{
        ecore::ParseLimits, generic_resolution::Resolver, resources::ResourceSet,
    };
    let source = DefinitionSource::new(
        "memory:resource-policy",
        SourceKind::Ecore,
        "<e:EPackage xmlns:e='http://www.eclipse.org/emf/2002/Ecore' name='m' nsURI='urn:m'/>",
    );
    let mut bundle = DefinitionBundle {
        version: 1,
        sources: vec![source],
        namespaces: vec![],
    };
    let baseline = Probe::new(usize::MAX, None);
    let explicit = Probe::new(usize::MAX, None);
    ResourceSet::new_observed(&bundle, &InvocationContext::new(baseline.clone())).unwrap();
    ResourceSet::new_observed_with_limits(
        &bundle,
        &InvocationContext::new(explicit.clone()),
        ParseLimits {
            source_bytes: 700_000,
        },
    )
    .unwrap();
    assert_eq!(
        (
            baseline.storage.get(),
            baseline.work.get(),
            baseline.calls.get()
        ),
        (
            explicit.storage.get(),
            explicit.work.get(),
            explicit.calls.get()
        )
    );
    bundle.sources.push(DefinitionSource::new("memory:large-resource",SourceKind::Ecore,format!("<e:EPackage xmlns:e='http://www.eclipse.org/emf/2002/Ecore' name='large' nsURI='urn:large'><!--{}--></e:EPackage>","x".repeat(300_000))));
    let limits = ParseLimits {
        source_bytes: 700_000,
    };
    let accepted = Probe::new(usize::MAX, None);
    Resolver::new_observed_with_limits(&bundle, &InvocationContext::new(accepted.clone()), limits)
        .unwrap();
    for probe in [
        Probe::new(0, None),
        Probe::with_work(usize::MAX, 0, None),
        Probe::new(usize::MAX, Some(accepted.calls.get() - 1)),
    ] {
        assert!(matches!(
            Resolver::new_observed_with_limits(&bundle, &InvocationContext::new(probe), limits),
            Err(ObservedImportError::Observation(_))
        ));
    }
    assert!(ResourceSet::new_diagnostic(&bundle).is_err());
    assert!(Resolver::new(&bundle).is_err());
}

#[test]
fn explicit_compiler_policy_preserves_receipts_and_exact_observation_guards() {
    use intersect_model::definitions::{
        compiler, conversion::ConversionRegistry, ecore::ParseLimits,
    };
    let source = DefinitionSource::new(
        "memory:compile-policy",
        SourceKind::Ecore,
        "<e:EPackage xmlns:e='http://www.eclipse.org/emf/2002/Ecore' xmlns:x='http://www.w3.org/2001/XMLSchema-instance' name='m' nsURI='urn:m'><eClassifiers x:type='e:EClass' name='Item'/></e:EPackage>",
    );
    let (source, _) = intersect_model_acceptance::migration_fixture::migrate_source_fixture(source);
    let bundle = DefinitionBundle {
        version: 1,
        sources: vec![source.clone()],
        namespaces: vec![],
    };
    let registry = ConversionRegistry::default();
    let baseline = Probe::new(usize::MAX, None);
    let expected = compiler::compile_structure_bundle_with_conversions_observed(
        &bundle,
        &source.id,
        "urn:m",
        &registry,
        &InvocationContext::new(baseline.clone()),
    )
    .unwrap();
    let run = |probe| {
        compiler::compile_structure_bundle_with_limits_observed(
            &bundle,
            &source.id,
            "urn:m",
            Some(&registry),
            &InvocationContext::new(probe),
            ParseLimits {
                source_bytes: 700_000,
            },
        )
    };
    let full = Probe::new(usize::MAX, None);
    assert_eq!(expected, run(full.clone()).unwrap());
    assert_eq!(
        (
            baseline.storage.get(),
            baseline.work.get(),
            baseline.calls.get()
        ),
        (full.storage.get(), full.work.get(), full.calls.get())
    );
    assert_eq!(
        expected,
        run(Probe::with_work(full.storage.get(), full.work.get(), None)).unwrap()
    );
    for probe in [
        Probe::new(full.storage.get() - 1, None),
        Probe::with_work(usize::MAX, full.work.get() - 1, None),
        Probe::new(usize::MAX, Some(full.calls.get() - 1)),
    ] {
        assert!(matches!(
            run(probe),
            Err(ObservedImportError::Observation(_))
        ));
    }
    assert_eq!(expected, run(Probe::new(usize::MAX, None)).unwrap());
}
