//! Handwritten independent expectations for grammar-neutral declaration inspection.
use intersect_model::definitions::{
    bundle::{DefinitionBundle, DefinitionSource, NamespaceBinding, SourceId, SourceKind},
    compiler::inspect_declarations_with_limits,
    conversion::{InvocationContext, InvocationObserver, ProviderId},
    declarations::DeclarationInventory,
    ecore::{ObservedImportError, ParseLimits},
};
use std::{cell::Cell, rc::Rc};

fn inputs() -> (DefinitionBundle, SourceId, SourceId) {
    let empty = DefinitionSource::new(
        "memory:empty.rcore",
        SourceKind::Rcore,
        "rcore 1; package empty version \"1\"; metamodel \"urn:inventory:empty\" prefix e {}",
    );
    let mut main = DefinitionSource::new(
        "memory:main.rcore",
        SourceKind::Rcore,
        "rcore 1; package main version \"1\"; metamodel \"urn:inventory:main\" prefix m { class Node { attr name: String [0,1]; } }",
    );
    main.dependencies.push(empty.id.clone());
    let other = DefinitionSource::new(
        "memory:unreachable.rcore",
        SourceKind::Rcore,
        "rcore 1; package other version \"1\"; metamodel \"urn:inventory:other\" prefix o {}",
    );
    let entry = main.id.clone();
    let dependency = empty.id.clone();
    (
        DefinitionBundle {
            version: 1,
            sources: vec![main, empty, other],
            namespaces: vec![],
        },
        entry,
        dependency,
    )
}

fn inspect(
    bundle: &DefinitionBundle,
    entry: &SourceId,
    classifiers: bool,
    context: &InvocationContext<'_>,
) -> Result<DeclarationInventory, ObservedImportError> {
    inspect_declarations_with_limits(
        bundle,
        entry,
        "urn:inventory:main",
        classifiers,
        context,
        ParseLimits::default(),
    )
}

#[test]
fn exact_reachable_packages_and_empty_classifier_sets_survive_inspection()
-> Result<(), Box<dyn std::error::Error>> {
    let (bundle, entry, dependency) = inputs();
    let inventory = inspect(&bundle, &entry, true, &InvocationContext::default())?;
    assert_eq!(inventory.packages().len(), 2);
    assert_eq!(inventory.packages()["urn:inventory:main"].source, entry);
    assert_eq!(
        inventory.packages()["urn:inventory:empty"].source,
        dependency
    );
    assert!(inventory.classifiers()["urn:inventory:empty"].is_empty());
    let node = &inventory.classifiers()["urn:inventory:main"]["Node"];
    assert_eq!(node.source, entry);
    assert_eq!(node.path, vec![0]);
    let packages_only = inspect(&bundle, &entry, false, &InvocationContext::default())?;
    assert_eq!(packages_only.packages(), inventory.packages());
    assert!(packages_only.classifiers().is_empty());
    Ok(())
}

#[test]
fn ambiguous_namespaces_require_an_exact_binding_and_stale_sources_reject()
-> Result<(), Box<dyn std::error::Error>> {
    let (mut bundle, entry, dependency) = inputs();
    let other = DefinitionSource::new(
        "memory:other-empty.rcore",
        SourceKind::Rcore,
        "rcore 1; package otherEmpty version \"2\"; metamodel \"urn:inventory:empty\" prefix e {}",
    );
    bundle.sources[0].dependencies.push(other.id.clone());
    bundle.sources.push(other);
    assert!(inspect(&bundle, &entry, true, &InvocationContext::default()).is_err());
    bundle.namespaces.push(NamespaceBinding {
        namespace: "urn:inventory:empty".into(),
        source: dependency.clone(),
    });
    let inventory = inspect(&bundle, &entry, true, &InvocationContext::default())?;
    assert_eq!(
        inventory.packages()["urn:inventory:empty"].source,
        dependency
    );
    let before = bundle.clone();
    bundle.sources[0].text.push(' ');
    assert!(inspect(&bundle, &entry, true, &InvocationContext::default()).is_err());
    assert_eq!(
        inspect(&before, &entry, true, &InvocationContext::default())?,
        inventory
    );
    Ok(())
}

struct Grant {
    work: Cell<usize>,
    storage: Cell<usize>,
    work_limit: usize,
    storage_limit: usize,
}
impl Grant {
    fn new(work_limit: usize, storage_limit: usize) -> Rc<Self> {
        Rc::new(Self {
            work: Cell::new(0),
            storage: Cell::new(0),
            work_limit,
            storage_limit,
        })
    }
}
impl InvocationObserver for Grant {
    fn before_invoke(&self, _: &ProviderId) -> Result<(), String> {
        Err("Declaration inspection cannot invoke a provider".into())
    }
    fn work(&self, units: usize) -> Result<(), String> {
        let total = self
            .work
            .get()
            .checked_add(units)
            .ok_or("inventory work overflow")?;
        if total > self.work_limit {
            return Err("inventory work denied".into());
        }
        self.work.set(total);
        Ok(())
    }
    fn retain_storage(&self, bytes: usize) -> Result<(), String> {
        let total = self
            .storage
            .get()
            .checked_add(bytes)
            .ok_or("inventory storage overflow")?;
        if total > self.storage_limit {
            return Err("inventory storage denied".into());
        }
        self.storage.set(total);
        Ok(())
    }
}

#[test]
fn observer_failures_keep_their_category_and_grants_are_not_reset()
-> Result<(), Box<dyn std::error::Error>> {
    let (bundle, entry, _) = inputs();
    let full = Grant::new(usize::MAX, usize::MAX);
    let inventory = inspect(&bundle, &entry, true, &InvocationContext::new(full.clone()))?;
    let (work, storage) = (full.work.get(), full.storage.get());
    assert!(work > 0 && storage > 0);
    for (work_limit, storage_limit, expected) in [
        (work - 1, usize::MAX, "inventory work denied"),
        (usize::MAX, storage - 1, "inventory storage denied"),
    ] {
        let grant = Grant::new(work_limit, storage_limit);
        let context = InvocationContext::new(grant.clone());
        assert!(
            matches!(inspect(&bundle, &entry, true, &context), Err(ObservedImportError::Observation(message)) if message == expected)
        );
        assert!(inspect(&bundle, &entry, true, &context).is_err());
    }
    let exact = Grant::new(work, storage);
    assert_eq!(
        inspect(
            &bundle,
            &entry,
            true,
            &InvocationContext::new(exact.clone())
        )?,
        inventory
    );
    assert_eq!((exact.work.get(), exact.storage.get()), (work, storage));
    assert_eq!(
        inspect(&bundle, &entry, true, &InvocationContext::default())?,
        inventory
    );
    Ok(())
}
