use intersect_model::definitions::{
    bundle::{DefinitionBundle, DefinitionSource, SourceKind},
    compiler::import_datatype_bundle,
    conversion::{
        ClassifierRelation as R, ConstructorContract, ConstructorProvider, ConstructorRegistry,
        ProviderId,
    },
    resources::DeclarationId,
};
use std::cell::{Cell, RefCell};

fn bundle() -> DefinitionBundle {
    DefinitionBundle {
        version: 1,
        sources: vec![DefinitionSource::new(
            "memory:constructors",
            SourceKind::Ecore,
            include_str!("../../../../fixtures/ecore-generics/applied-collections.ecore"),
        )],
        namespaces: vec![],
    }
}
struct Provider {
    contract: ConstructorContract,
    changed: ConstructorContract,
    mutate: Cell<bool>,
    mutated: Cell<bool>,
    answer: Result<Option<R>, String>,
    seen: RefCell<Vec<ConstructorContract>>,
}
impl Provider {
    fn new(bundle: &DefinitionBundle, index: usize, answer: Result<Option<R>, String>) -> Self {
        let id = DeclarationId {
            source: bundle.sources[0].id.clone(),
            path: vec![index],
        };
        let contract = ConstructorContract {
            datatype: import_datatype_bundle(bundle, &id).unwrap(),
            provider: ProviderId {
                name: "test.constructor".into(),
                version: "1".into(),
            },
        };
        let mut changed = contract.clone();
        changed.provider.version = "2".into();
        Self {
            contract,
            changed,
            answer,
            mutate: Cell::new(false),
            mutated: Cell::new(false),
            seen: RefCell::new(vec![]),
        }
    }
    fn id(&self) -> &DeclarationId {
        self.contract.datatype.declaration()
    }
}
impl ConstructorProvider for Provider {
    fn contract(&self) -> &ConstructorContract {
        if self.mutated.get() {
            &self.changed
        } else {
            &self.contract
        }
    }
    fn constructor_relation(&self, target: &ConstructorContract) -> Result<Option<R>, String> {
        self.seen.borrow_mut().push(target.clone());
        if self.mutate.get() {
            self.mutated.set(true);
        }
        self.answer.clone()
    }
}

#[test]
fn source_compilation_validates_unconsumed_constructor_registrations_and_recovers() {
    use intersect_model::definitions::{
        compiler::{
            compile_structure_bundle_with_conversions,
            compile_structure_bundle_with_conversions_observed,
        },
        conversion::{ConversionRegistry, InvocationContext},
        ecore::{ErrorKind, ObservedImportError},
    };
    let providers = bundle();
    let provider = Provider::new(&providers, 2, Ok(None));
    let mut registry = ConversionRegistry::default();
    registry
        .register_constructor(&providers, &provider)
        .unwrap();
    let source = DefinitionSource::new(
        "memory:entry",
        SourceKind::Ecore,
        "<e:EPackage xmlns:e='http://www.eclipse.org/emf/2002/Ecore' xmlns:x='http://www.w3.org/2001/XMLSchema-instance' name='m' nsURI='urn:m'><eClassifiers x:type='e:EClass' name='C'/></e:EPackage>",
    );
    let (source, _) = intersect_model_acceptance::migration_fixture::migrate_source_fixture(source);
    let input = DefinitionBundle {
        version: 1,
        sources: vec![source.clone()],
        namespaces: vec![],
    };
    let original = input.clone();
    let run = || compile_structure_bundle_with_conversions(&input, &source.id, "urn:m", &registry);
    let baseline = run().unwrap();
    assert!(baseline.constructor_contracts.is_empty());
    provider.mutated.set(true);
    let failure = run().unwrap_err();
    assert_eq!(failure.kind, ErrorKind::InvalidDefinition);
    assert_eq!(
        failure.message,
        "Constructor provider changed its registered contract"
    );
    assert_eq!(failure.span.source, source.id);
    let resources =
        intersect_model::definitions::resources::ResourceSet::new_diagnostic(&input).unwrap();
    assert_eq!(
        failure.span,
        resources.document(&source.id).unwrap().root.opening
    );
    assert_eq!(run().unwrap_err(), failure);
    assert_eq!(
        compile_structure_bundle_with_conversions_observed(
            &input,
            &source.id,
            "urn:m",
            &registry,
            &InvocationContext::default()
        )
        .unwrap_err(),
        ObservedImportError::Import(failure),
    );
    assert_eq!(
        registry.constructors().registered_contracts().unwrap_err(),
        "Constructor provider changed its registered contract"
    );
    assert!(provider.seen.borrow().is_empty());
    assert_eq!(input, original);
    provider.mutated.set(false);
    assert_eq!(
        serde_json::to_value(run().unwrap()).unwrap(),
        serde_json::to_value(baseline).unwrap()
    );
    assert_eq!(
        registry.constructors().registered_contracts().unwrap(),
        vec![provider.contract.clone()]
    );
    assert!(provider.seen.borrow().is_empty());
}

#[test]
fn constructor_callbacks_are_reciprocal_and_receive_full_source_contracts() {
    let bundle = bundle();
    for (forward, reverse) in [
        (R::Equivalent, R::Equivalent),
        (R::Subtype, R::Supertype),
        (R::Supertype, R::Subtype),
        (R::Unrelated, R::Unrelated),
    ] {
        let a = Provider::new(&bundle, 2, Ok(Some(forward)));
        let b = Provider::new(&bundle, 3, Ok(Some(reverse)));
        let mut registry = ConstructorRegistry::default();
        registry.register(&bundle, &a).unwrap();
        registry.register(&bundle, &b).unwrap();
        assert_eq!(
            registry.constructor_relation(a.id(), b.id()).unwrap(),
            Some(forward)
        );
        assert_eq!(
            registry.constructor_relation(b.id(), a.id()).unwrap(),
            Some(reverse)
        );
        assert!(a.seen.borrow().iter().all(|c| c == &b.contract));
        assert!(b.seen.borrow().iter().all(|c| c == &a.contract));
        let contracts = registry.registered_contracts().unwrap();
        assert_eq!(
            serde_json::from_str::<Vec<ConstructorContract>>(
                &serde_json::to_string(&contracts).unwrap()
            )
            .unwrap(),
            contracts
        );
        // This deliberately allows constructor assertions of differing arity:
        // the application consumer must check arity and ordered arguments itself.
        assert_ne!(
            a.contract.datatype.parameters().len(),
            b.contract.datatype.parameters().len()
        );
    }
}

#[test]
fn unknown_disagreement_and_callback_errors_are_not_compatibility() {
    let bundle = bundle();
    for (answer, expected) in [
        (Ok(None), None),
        (Ok(Some(R::Equivalent)), Some("disagree")),
        (Err("policy failed".into()), Some("policy failed")),
    ] {
        let a = Provider::new(&bundle, 2, Ok(Some(R::Subtype)));
        let b = Provider::new(&bundle, 3, answer);
        let mut registry = ConstructorRegistry::default();
        assert!(registry.constructor_relation(a.id(), a.id()).is_err());
        registry.register(&bundle, &a).unwrap();
        registry.register(&bundle, &b).unwrap();
        let result = registry.constructor_relation(a.id(), b.id());
        match expected {
            Some(message) => assert!(result.unwrap_err().contains(message)),
            None => assert_eq!(result.unwrap(), None),
        }
        assert_eq!(
            registry.constructor_relation(a.id(), a.id()).unwrap(),
            Some(R::Equivalent)
        );
    }
}

#[test]
fn mutation_during_either_callback_invalidates_relations_and_snapshots() {
    let bundle = bundle();
    for left in [true, false] {
        let a = Provider::new(&bundle, 2, Ok(Some(R::Equivalent)));
        let b = Provider::new(&bundle, 3, Ok(Some(R::Equivalent)));
        a.mutate.set(left);
        b.mutate.set(!left);
        let mut registry = ConstructorRegistry::default();
        registry.register(&bundle, &a).unwrap();
        registry.register(&bundle, &b).unwrap();
        assert!(
            registry
                .constructor_relation(a.id(), b.id())
                .unwrap_err()
                .contains("changed its registered contract")
        );
        assert!(registry.registered_contracts().is_err());
    }
}

#[test]
fn registration_rechecks_source_and_rejects_nongeneric_duplicate_and_empty_identity() {
    let bundle = bundle();
    let scalar = Provider::new(&bundle, 0, Ok(None));
    let a = Provider::new(&bundle, 2, Ok(None));
    let mut empty = Provider::new(&bundle, 3, Ok(None));
    empty.contract.provider.version.clear();
    let mut forged = Provider::new(&bundle, 3, Ok(None));
    let mut wire = serde_json::to_value(&forged.contract.datatype).unwrap();
    wire["serializable"] = true.into();
    forged.contract.datatype = serde_json::from_value(wire).unwrap();
    let mut registry = ConstructorRegistry::default();
    assert!(
        registry
            .register(&bundle, &scalar)
            .unwrap_err()
            .contains("uninstantiated generic")
    );
    registry.register(&bundle, &a).unwrap();
    assert!(
        registry
            .register(&bundle, &a)
            .unwrap_err()
            .contains("already registered")
    );
    assert!(
        registry
            .register(&bundle, &empty)
            .unwrap_err()
            .contains("nonempty")
    );
    assert!(
        registry
            .register(&bundle, &forged)
            .unwrap_err()
            .contains("differs from imported")
    );
    let mut stale = bundle.clone();
    stale.sources.clear();
    assert!(ConstructorRegistry::default().register(&stale, &a).is_err());
}

fn generic_bundle(body: &str) -> DefinitionBundle {
    DefinitionBundle {
        version: 1,
        sources: vec![DefinitionSource::new(
            "memory:constructor-consumers",
            SourceKind::Ecore,
            format!(
                r#"<e:EPackage xmlns:e="http://www.eclipse.org/emf/2002/Ecore" xmlns:x="http://www.w3.org/2001/XMLSchema-instance" name="g" nsURI="urn:g">
<eClassifiers x:type="e:EClass" name="Text"/>
<eClassifiers x:type="e:EClass" name="Number"/>
<eClassifiers x:type="e:EDataType" name="D" instanceClassName="example.Domain"><eTypeParameters name="T"/></eClassifiers>
<eClassifiers x:type="e:EDataType" name="A" instanceClassName="example.Alias"><eTypeParameters name="T"/></eClassifiers>
{body}</e:EPackage>"#
            ),
        )],
        namespaces: vec![],
    }
}
fn argument(name: &str) -> String {
    format!(r##"<eTypeArguments eClassifier="#//{name}"/>"##)
}
fn app(tag: &str, name: &str, args: &str) -> String {
    format!(r##"<{tag} eClassifier="#//{name}">{args}</{tag}>"##)
}
fn bounded_bundle(actual: &str, bound: &str) -> DefinitionBundle {
    generic_bundle(&format!(
        r##"
<eClassifiers x:type="e:EClass" name="Box"><eTypeParameters name="T">{}</eTypeParameters></eClassifiers>
<eClassifiers x:type="e:EClass" name="Use"><eStructuralFeatures x:type="e:EReference" name="value">{}</eStructuralFeatures></eClassifiers>"##,
        app("eBounds", "D", bound),
        app("eGenericType", "Box", &app("eTypeArguments", "A", actual))
    ))
}
fn selected(bundle: &DefinitionBundle, path: &[usize]) -> DeclarationId {
    DeclarationId {
        source: bundle.sources[0].id.clone(),
        path: path.to_vec(),
    }
}
fn resolver(
    bundle: &DefinitionBundle,
    relation: R,
) -> intersect_model::definitions::generic_resolution::Resolver {
    use intersect_model::definitions::generic_resolution::Resolver;
    let reverse = match relation {
        R::Equivalent => R::Equivalent,
        R::Subtype => R::Supertype,
        R::Supertype => R::Subtype,
        R::Unrelated => R::Unrelated,
    };
    let a = Provider::new(bundle, 3, Ok(Some(relation)));
    let b = Provider::new(bundle, 2, Ok(Some(reverse)));
    let mut registry = ConstructorRegistry::default();
    registry.register(bundle, &a).unwrap();
    registry.register(bundle, &b).unwrap();
    Resolver::new(bundle)
        .unwrap()
        .with_constructors(&registry)
        .unwrap()
}

#[test]
fn constructor_bounds_check_direction_and_ordered_arguments_and_retain_separate_dependencies() {
    for (relation, expected) in [
        (R::Equivalent, true),
        (R::Subtype, true),
        (R::Supertype, false),
        (R::Unrelated, false),
    ] {
        for actual in ["Text", "Number"] {
            let bundle = bounded_bundle(&argument(actual), &argument("Text"));
            let resolver = resolver(&bundle, relation);
            assert!(resolver.consumed_constructor_contracts().is_empty());
            let report = resolver
                .application_bounds(&selected(&bundle, &[5, 0, 0]))
                .unwrap();
            assert_eq!(report.valid, expected && actual == "Text");
            assert!(resolver.consumed_relation_datatypes().is_empty());
            let consumed = resolver.consumed_constructor_contracts();
            assert_eq!(consumed.len(), 2);
            assert_eq!(consumed[0].datatype.declaration(), &selected(&bundle, &[2]));
            assert_eq!(consumed[1].datatype.declaration(), &selected(&bundle, &[3]));
        }
    }
}

#[test]
fn constructor_alias_wildcards_and_nested_arguments_use_shared_matching() {
    let text = argument("Text");
    let any = "<eTypeArguments/>".to_string();
    let upper = format!(
        "<eTypeArguments>{}</eTypeArguments>",
        app("eUpperBound", "Text", "")
    );
    let lower = format!(
        "<eTypeArguments>{}</eTypeArguments>",
        app("eLowerBound", "Text", "")
    );
    for bound in [&any, &upper, &lower] {
        for actual in ["Text", "Number"] {
            let bundle = bounded_bundle(&argument(actual), bound);
            assert_eq!(
                resolver(&bundle, R::Equivalent)
                    .application_bounds(&selected(&bundle, &[5, 0, 0]))
                    .unwrap()
                    .valid,
                bound == &any || actual == "Text"
            );
        }
    }
    // Equivalent nested constructors still compare invariant inner arguments.
    let bound = app("eTypeArguments", "D", &text);
    for (relation, actual, expected) in [
        (R::Equivalent, "Text", true),
        (R::Equivalent, "Number", false),
        (R::Subtype, "Text", false),
    ] {
        let bundle = bounded_bundle(&app("eTypeArguments", "A", &argument(actual)), &bound);
        assert_eq!(
            resolver(&bundle, relation)
                .application_bounds(&selected(&bundle, &[5, 0, 0]))
                .unwrap()
                .valid,
            expected
        );
    }
}

#[test]
fn missing_or_stale_constructor_context_does_not_infer_aliases_from_metadata() {
    use intersect_model::definitions::generic_resolution::Resolver;
    let bundle = bounded_bundle(&argument("Text"), &argument("Text"));
    let usage = selected(&bundle, &[5, 0, 0]);
    assert!(
        Resolver::new(&bundle)
            .unwrap()
            .application_bounds(&usage)
            .unwrap_err()
            .to_string()
            .contains("assignability contract")
    );
    let a = Provider::new(&bundle, 3, Ok(Some(R::Equivalent)));
    let b = Provider::new(&bundle, 2, Ok(None));
    let mut registry = ConstructorRegistry::default();
    registry.register(&bundle, &a).unwrap();
    registry.register(&bundle, &b).unwrap();
    let unresolved = Resolver::new(&bundle)
        .unwrap()
        .with_constructors(&registry)
        .unwrap();
    assert!(unresolved.application_bounds(&usage).is_err());
    assert!(unresolved.consumed_constructor_contracts().is_empty());
    let stale = bounded_bundle(&argument("Number"), &argument("Text"));
    assert!(
        Resolver::new(&stale)
            .unwrap()
            .with_constructors(&registry)
            .is_err()
    );
    a.mutate.set(true);
    assert!(
        Resolver::new(&bundle)
            .unwrap()
            .with_constructors(&registry)
            .is_err()
    );
}

#[test]
fn inherited_constructor_aliases_must_agree_on_nested_arguments() {
    for actual in ["Text", "Number"] {
        let body = format!(
            r##"
<eClassifiers x:type="e:EClass" name="Box"><eTypeParameters name="T"/></eClassifiers>
<eClassifiers x:type="e:EClass" name="Left">{}</eClassifiers>
<eClassifiers x:type="e:EClass" name="Right">{}</eClassifiers>
<eClassifiers x:type="e:EClass" name="Root" eSuperTypes="#//Left #//Right"/>
<eClassifiers x:type="e:EClass" name="Use"><eStructuralFeatures x:type="e:EReference" name="value" >{}</eStructuralFeatures></eClassifiers>"##,
            app(
                "eGenericSuperTypes",
                "Box",
                &app("eTypeArguments", "D", &argument("Text"))
            ),
            app(
                "eGenericSuperTypes",
                "Box",
                &app("eTypeArguments", "A", &argument(actual))
            ),
            app("eGenericType", "Root", "")
        );
        let bundle = generic_bundle(&body);
        let result =
            resolver(&bundle, R::Equivalent).inherited_types(&selected(&bundle, &[8, 0, 0]));
        if actual == "Text" {
            assert!(result.is_ok());
        } else {
            assert!(
                result
                    .unwrap_err()
                    .to_string()
                    .contains("Incompatible generic diamond")
            );
        }
        assert!(
            resolver(&bundle, R::Subtype)
                .inherited_types(&selected(&bundle, &[8, 0, 0]))
                .is_err()
        );
    }
}

#[test]
fn operation_erasure_uses_constructor_equivalence_without_comparing_projected_arguments() {
    let bundle = generic_bundle(&format!(
        r##"
<eClassifiers x:type="e:EClass" name="Base"><eOperations name="f"><eParameters name="p">{}</eParameters></eOperations></eClassifiers>
<eClassifiers x:type="e:EClass" name="Child" eSuperTypes="#//Base"><eOperations name="f"><eParameters name="p">{}</eParameters></eOperations></eClassifiers>"##,
        app("eGenericType", "D", &argument("Text")),
        app("eGenericType", "A", &argument("Number"))
    ));
    for (relation, expected) in [
        (R::Equivalent, true),
        (R::Subtype, false),
        (R::Supertype, false),
    ] {
        let resolver = resolver(&bundle, relation);
        let result = resolver
            .operation_override(&selected(&bundle, &[5, 0]), &selected(&bundle, &[4, 0]))
            .unwrap();
        assert_eq!(result.overrides, expected);
        assert_eq!(resolver.consumed_constructor_contracts().len(), 2);
    }
}

#[test]
fn constructor_compatibility_cannot_bypass_arity_or_positional_matching() {
    for (both_two, swapped, expected) in [
        (false, false, false),
        (true, false, true),
        (true, true, false),
    ] {
        let actual = if swapped {
            format!("{}{}", argument("Number"), argument("Text"))
        } else {
            format!("{}{}", argument("Text"), argument("Number"))
        };
        let bound = if both_two {
            format!("{}{}", argument("Text"), argument("Number"))
        } else {
            argument("Text")
        };
        let original = bounded_bundle(&actual, &bound);
        let source = &original.sources[0];
        let second = "<eTypeParameters name=\"T\"/><eTypeParameters name=\"U\"/>";
        let text = if both_two {
            source.text.replace("<eTypeParameters name=\"T\"/>", second)
        } else {
            source.text.replace(
                "instanceClassName=\"example.Alias\"><eTypeParameters name=\"T\"/>",
                &format!("instanceClassName=\"example.Alias\">{second}"),
            )
        };
        let bundle = DefinitionBundle {
            version: 1,
            sources: vec![DefinitionSource::new(
                source.id.uri.clone(),
                SourceKind::Ecore,
                text,
            )],
            namespaces: vec![],
        };
        assert_eq!(
            resolver(&bundle, R::Equivalent)
                .application_bounds(&selected(&bundle, &[5, 0, 0]))
                .unwrap()
                .valid,
            expected
        );
    }
}

#[test]
fn shared_constructor_registration_retains_authority_until_last_registry_drops() {
    let bundle = bundle();
    let provider = std::rc::Rc::new(Provider::new(&bundle, 2, Ok(None)));
    let weak = std::rc::Rc::downgrade(&provider);
    let contract = provider.contract.clone();
    let mut registry = ConstructorRegistry::default();
    registry.register_shared(&bundle, provider.clone()).unwrap();
    let retained = registry.clone();
    drop(provider);
    drop(registry);
    assert!(weak.upgrade().is_some());
    assert_eq!(retained.registered_contracts().unwrap(), vec![contract]);
    drop(retained);
    assert!(weak.upgrade().is_none());
}

#[test]
fn exact_constructor_lookup_checks_snapshot_without_semantic_invocation() {
    let bundle = bundle();
    let provider = Provider::new(&bundle, 2, Ok(None));
    let mut registry = ConstructorRegistry::default();
    assert!(!registry.contains_contract(&provider.contract).unwrap());
    registry.register(&bundle, &provider).unwrap();
    assert!(registry.contains_contract(&provider.contract).unwrap());
    assert!(!registry.contains_contract(&provider.changed).unwrap());
    assert!(provider.seen.borrow().is_empty());
    provider.mutated.set(true);
    assert!(
        registry
            .contains_contract(&provider.contract)
            .unwrap_err()
            .contains("changed")
    );
}
