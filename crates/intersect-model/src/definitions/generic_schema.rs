//! Checked concrete generic projections into the existing structural compiler.
use super::Xml;
use crate::definitions::{
    bundle::{DefinitionBundle, SourceId},
    ecore::{ECORE, ErrorKind, ImportError, Node, XMI},
    generic_resolution::{Resolver, SignatureInventory},
    generics::GenericKind,
    resources::{DeclarationId, ResourceSet},
};
use std::collections::BTreeSet;
use std::fmt::Write;

pub(super) struct Prepared {
    pub(super) inventory: SignatureInventory,
    resolver: Resolver,
}

pub(crate) fn contains_generic(node: &Node) -> bool {
    match scan_generic(node, &|_| Ok::<(), std::convert::Infallible>(()), false) {
        Ok(found) => found,
        Err(never) => match never {},
    }
}
pub(crate) fn requires_inventory(node: &Node) -> bool {
    match scan_generic(node, &|_| Ok::<(), std::convert::Infallible>(()), true) {
        Ok(found) => found,
        Err(never) => match never {},
    }
}
fn scan_generic<E>(
    node: &Node,
    before: &impl Fn(&Node) -> Result<(), E>,
    annotations: bool,
) -> Result<bool, E> {
    before(node)?;
    if node.name.namespace.is_none() && node.name.local == "eAnnotations" {
        return Ok(annotations);
    }
    if node.name.namespace.is_some()
        && !((node.name.namespace.as_deref() == Some(ECORE) && node.name.local == "EPackage")
            || (node.name.namespace.as_deref() == Some(XMI) && node.name.local == "XMI"))
    {
        return Ok(false);
    }
    if matches!(
        node.name.local.as_str(),
        "eTypeParameters"
            | "eGenericType"
            | "eGenericSuperTypes"
            | "eGenericExceptions"
            | "eTypeArguments"
            | "eBounds"
            | "eUpperBound"
            | "eLowerBound"
    ) {
        return Ok(true);
    }
    for child in &node.children {
        if scan_generic(child, before, annotations)? {
            return Ok(true);
        }
    }
    Ok(false)
}
pub(super) fn prepare_with_limits(
    bundle: &DefinitionBundle,
    resources: &ResourceSet,
    reachable: &BTreeSet<SourceId>,
    observation: &crate::definitions::ecore::ParserObservation<'_, '_>,
    limits: crate::definitions::ecore::ParseLimits,
) -> Result<Option<Prepared>, ImportError> {
    let mut contains = false;
    for id in reachable {
        observation.work(1, id)?;
        let document = resources.document(id).map_err(|message| {
            ImportError::new(
                ErrorKind::InvalidDefinition,
                &crate::definitions::bundle::SourceSpan {
                    source: id.clone(),
                    start: 0,
                    end: 0,
                },
                message,
            )
        })?;
        if scan_generic(
            &document.root,
            &|node| observation.work(1, &node.opening.source),
            true,
        )? {
            contains = true;
            break;
        }
    }
    if !contains {
        return Ok(None);
    }
    let reachable = if bundle.sources.iter().any(|s| {
        s.kind == crate::definitions::bundle::SourceKind::Rcore && reachable.contains(&s.id)
    }) {
        std::borrow::Cow::Owned(super::resource_compiler::complete_dependencies(
            bundle,
            reachable,
            observation,
        )?)
    } else {
        std::borrow::Cow::Borrowed(reachable)
    };
    let mut sources = Vec::new();
    for source in &bundle.sources {
        observation.work(1, &source.id)?;
        if reachable.contains(&source.id) {
            observation.serialized(source, &source.id)?;
            observation.reserve(32, &source.id)?;
            sources.push(source.clone());
        }
    }
    let mut namespaces = Vec::new();
    for binding in &bundle.namespaces {
        observation.work(1, &binding.source)?;
        if reachable.contains(&binding.source) {
            observation.serialized(binding, &binding.source)?;
            observation.reserve(32, &binding.source)?;
            namespaces.push(binding.clone());
        }
    }
    if let Some(id) = reachable.first() {
        observation.reserve(128, id)?;
    }
    let selected = DefinitionBundle {
        version: bundle.version,
        sources,
        namespaces,
    };
    let resolver = Resolver::new_context_with_limits(&selected, observation, limits)?;
    let signatures = resolver.resolve_all()?;
    if let Some(parameter) = signatures.parameters.first() {
        return Err(ImportError::new(
            ErrorKind::Unsupported,
            &parameter.syntax.span,
            "Checked generic formal parameters require E08 substitution/conformance before runtime schema compilation",
        ));
    }
    for ty in &signatures.types {
        if !matches!(&ty.syntax.value,GenericKind::Classifier{arguments,..} if arguments.is_empty())
        {
            return Err(ImportError::new(
                ErrorKind::Unsupported,
                &ty.syntax.span,
                "Checked generic arguments, wildcards and parameter references require E08 substitution/conformance before runtime schema compilation",
            ));
        }
    }
    if let Some(id) = reachable.first() {
        observation.reserve(128, id)?;
    }
    Ok(Some(Prepared {
        inventory: signatures,
        resolver,
    }))
}
fn reference(
    resources: &ResourceSet,
    standard: &ResourceSet,
    standard_id: &SourceId,
    id: &DeclarationId,
    at: &Xml,
    observation: &crate::definitions::ecore::ParserObservation<'_, '_>,
) -> Result<String, ImportError> {
    observation.work(1, &id.source)?;
    if &id.source == standard_id {
        let node = standard.node(id).map_err(|e| at.error(e))?;
        let mut name = None;
        for attribute in &node.attributes {
            observation.work(1, &id.source)?;
            if attribute.name.namespace.is_none() && attribute.name.local == "name" {
                name = Some(attribute.value.as_str());
                break;
            }
        }
        let name = name.ok_or_else(|| at.error("Missing standard classifier name"))?;
        const PREFIX: &str = "http://www.eclipse.org/emf/2002/Ecore#//";
        let bytes = observation.size(PREFIX.len().checked_add(name.len()), &id.source)?;
        observation.work(bytes.div_ceil(256).max(1), &id.source)?;
        observation.string(bytes, &id.source)?;
        return Ok(format!("{PREFIX}{name}"));
    }
    let root = &resources
        .document(&id.source)
        .map_err(|e| at.error(e))?
        .root;
    let (node, path, root_index) =
        if root.name.namespace.as_deref() == Some(XMI) && root.name.local == "XMI" {
            let (index, tail) = id
                .path
                .split_first()
                .ok_or_else(|| at.error("Missing XMI package identity"))?;
            (
                root.children
                    .get(*index)
                    .ok_or_else(|| at.error("Missing XMI root"))?,
                tail,
                *index,
            )
        } else {
            (root, id.path.as_slice(), 0)
        };
    let mut bytes = observation.size(
        id.source.uri.len().checked_add(2 + digits(root_index)),
        &id.source,
    )?;
    segments(node, path, at, observation, &mut |local, ordinal| {
        bytes = observation.size(
            bytes
                .checked_add(local.len())
                .and_then(|n| n.checked_add(3 + digits(ordinal))),
            &id.source,
        )?;
        Ok(())
    })?;
    observation.work(bytes.div_ceil(256).max(1), &id.source)?;
    observation.string(bytes, &id.source)?;
    let mut href = String::with_capacity(bytes);
    write!(&mut href, "{}#/{root_index}", id.source.uri)
        .map_err(|_| at.error("Cannot format generic classifier reference"))?;
    segments(node, path, at, observation, &mut |local, ordinal| {
        write!(&mut href, "/@{local}.{ordinal}")
            .map_err(|_| at.error("Cannot format generic classifier reference"))
    })?;
    Ok(href)
}
fn digits(value: usize) -> usize {
    if value == 0 {
        1
    } else {
        value.ilog10() as usize + 1
    }
}
fn segments(
    mut node: &Node,
    path: &[usize],
    at: &Xml,
    observation: &crate::definitions::ecore::ParserObservation<'_, '_>,
    visit: &mut impl FnMut(&str, usize) -> Result<(), ImportError>,
) -> Result<(), ImportError> {
    for index in path {
        observation.work(1, &at.declaration.source)?;
        let child = node
            .children
            .get(*index)
            .ok_or_else(|| at.error("Missing generic classifier path"))?;
        if child.name.namespace.is_some() {
            return Err(at.error("Unsupported classifier containment namespace"));
        }
        let mut ordinal = 0;
        for sibling in &node.children[..*index] {
            observation.work(1, &at.declaration.source)?;
            if sibling.name == child.name {
                ordinal += 1;
            }
        }
        visit(&child.name.local, ordinal)?;
        node = child;
    }
    Ok(())
}
pub(super) fn project(
    node: &mut Xml,
    prepared: &Prepared,
    resources: &ResourceSet,
    observation: &crate::definitions::ecore::ParserObservation<'_, '_>,
) -> Result<(), ImportError> {
    let inventory = &prepared.inventory;
    let (_, standard, standard_id) = prepared.resolver.resource_sets();
    for signature in &inventory.raw_signatures {
        observation.work(1, &node.declaration.source)?;
        if signature.owner != node.declaration || signature.generic.is_empty() {
            continue;
        }
        let mut references = Vec::new();
        let mut bytes = 0usize;
        for id in &signature.raw_types {
            observation.work(1, &node.declaration.source)?;
            let reference = reference(resources, standard, standard_id, id, node, observation)?;
            bytes =
                observation.size(bytes.checked_add(reference.len()), &node.declaration.source)?;
            observation.reserve(32, &node.declaration.source)?;
            references.push(reference);
        }
        bytes = observation.size(
            bytes.checked_add(references.len().saturating_sub(1)),
            &node.declaration.source,
        )?;
        observation.work(bytes.div_ceil(256).max(1), &node.declaration.source)?;
        observation.string(bytes, &node.declaration.source)?;
        observation.string(signature.feature.len(), &node.declaration.source)?;
        observation.reserve(128, &node.declaration.source)?;
        node.attrs
            .insert(signature.feature.clone(), references.join(" "));
        let mut span = &node.opening;
        for ty in &inventory.types {
            observation.work(1, &node.declaration.source)?;
            if contains(&signature.generic, &ty.syntax.declaration, observation)? {
                span = &ty.syntax.span;
                break;
            }
        }
        observation.string(signature.feature.len(), &node.declaration.source)?;
        observation.serialized(span, &node.declaration.source)?;
        observation.reserve(128, &node.declaration.source)?;
        node.attr_spans
            .insert(signature.feature.clone(), span.clone());
        let mut failure = None;
        node.children.retain(|child| {
            if failure.is_some() {
                return true;
            }
            let keep = observation
                .work(1, &child.declaration.source)
                .and_then(|()| contains(&signature.generic, &child.declaration, observation))
                .map(|generic| !generic && child.name != signature.feature);
            match keep {
                Ok(keep) => keep,
                Err(error) => {
                    failure = Some(error);
                    true
                }
            }
        });
        if let Some(error) = failure {
            return Err(error);
        }
    }
    for child in &mut node.children {
        observation.work(1, &child.declaration.source)?;
        project(child, prepared, resources, observation)?;
    }
    Ok(())
}
fn contains(
    declarations: &[DeclarationId],
    target: &DeclarationId,
    observation: &crate::definitions::ecore::ParserObservation<'_, '_>,
) -> Result<bool, ImportError> {
    for declaration in declarations {
        observation.work(1, &target.source)?;
        if declaration == target {
            return Ok(true);
        }
    }
    Ok(false)
}

#[cfg(test)]
mod projection_controls {
    use super::*;
    use crate::definitions::{
        bundle::DefinitionSource,
        conversion::{InvocationContext, InvocationObserver, ProviderId},
        ecore::{ObservedImportError, ParserObservation},
    };
    use std::{cell::Cell, rc::Rc};

    struct Probe {
        storage: Cell<usize>,
        work: Cell<usize>,
        work_limit: Cell<usize>,
        calls: Cell<usize>,
        limit: usize,
        cancel: Option<usize>,
    }
    impl Probe {
        fn new(limit: usize, cancel: Option<usize>) -> Rc<Self> {
            Rc::new(Self {
                storage: Cell::new(0),
                work: Cell::new(0),
                work_limit: Cell::new(usize::MAX),
                calls: Cell::new(0),
                limit,
                cancel,
            })
        }
        fn touch(&self) -> Result<(), String> {
            let call = self.calls.get();
            self.calls.set(call + 1);
            if self.cancel == Some(call) {
                Err("projection cancelled".into())
            } else {
                Ok(())
            }
        }
    }
    impl InvocationObserver for Probe {
        fn before_invoke(&self, _: &ProviderId) -> Result<(), String> {
            panic!("projection cannot invoke a provider")
        }
        fn work(&self, units: usize) -> Result<(), String> {
            self.touch()?;
            let next = self.work.get() + units;
            if next > self.work_limit.get() {
                return Err("projection work denied".into());
            }
            self.work.set(next);
            Ok(())
        }
        fn retain_storage(&self, bytes: usize) -> Result<(), String> {
            self.touch()?;
            let next = self.storage.get() + bytes;
            if next > self.limit {
                return Err("projection storage denied".into());
            }
            self.storage.set(next);
            Ok(())
        }
    }
    fn bundle(text: &str) -> DefinitionBundle {
        DefinitionBundle {
            version: 1,
            sources: vec![DefinitionSource::new(
                "memory:x",
                crate::definitions::bundle::SourceKind::Ecore,
                text,
            )],
            namespaces: vec![],
        }
    }
    #[test]
    fn reference_strings_preflight_literal_utf8_xmi_and_standard_limits() {
        let bundle = bundle(
            r#"<m:XMI xmlns:m="http://www.omg.org/XMI" xmlns:e="http://www.eclipse.org/emf/2002/Ecore" xmlns:x="http://www.w3.org/2001/XMLSchema-instance"><e:EPackage name="unused" nsURI="urn:unused"/><e:EPackage name="p" nsURI="urn:p"><eSubpackages name="nested" nsURI="urn:n"><eClassifiers x:type="e:EClass" name="First"/><eClassifiers x:type="e:EClass" name="名前"/></eSubpackages></e:EPackage></m:XMI>"#,
        );
        let resolver = Resolver::new(&bundle).unwrap();
        let (resources, standard, standard_id) = resolver.resource_sets();
        let doc = resources.document(&bundle.sources[0].id).unwrap();
        let at = Xml::from_node(&doc.root, &doc.source.text).unwrap();
        let nested = DeclarationId {
            source: bundle.sources[0].id.clone(),
            path: vec![1, 0, 1],
        };
        let string = standard.resolve(standard_id, "#//EString").unwrap();
        // 64-byte owned string slot plus literal 43/47 UTF-8 bytes.
        for (id, expected, bytes, work) in [
            (
                &nested,
                "memory:x#/1/@eSubpackages.0/@eClassifiers.1",
                107,
                8,
            ),
            (
                &string,
                "http://www.eclipse.org/emf/2002/Ecore#//EString",
                111,
                4,
            ),
        ] {
            let run = |probe: Rc<Probe>| {
                let context = InvocationContext::new(probe);
                let observation = ParserObservation::new(&context);
                observation.finish(reference(
                    resources,
                    standard,
                    standard_id,
                    id,
                    &at,
                    &observation,
                ))
            };
            let full = Probe::new(bytes, None);
            assert_eq!(run(full.clone()).unwrap(), expected);
            assert_eq!((full.storage.get(), full.work.get()), (bytes, work));
            assert_eq!(
                run(Probe::new(bytes - 1, None)),
                Err(ObservedImportError::Observation(
                    "projection storage denied".into()
                ))
            );
            for call in 0..full.calls.get() {
                assert_eq!(
                    run(Probe::new(usize::MAX, Some(call))),
                    Err(ObservedImportError::Observation(
                        "projection cancelled".into()
                    ))
                );
            }
            assert_eq!(run(Probe::new(bytes + 1, None)).unwrap(), expected);
            for limit in [work - 1, work, work + 1] {
                let probe = Probe::new(bytes, None);
                probe.work_limit.set(limit);
                let result = run(probe);
                if limit < work {
                    assert_eq!(
                        result,
                        Err(ObservedImportError::Observation(
                            "projection work denied".into()
                        ))
                    );
                } else {
                    assert_eq!(result.unwrap(), expected);
                }
            }
        }
        // A UTF-8 URI is charged by bytes, not character count. No resource I/O occurs.
        let unicode = bundle.sources[0].text.replace("名前", "Other");
        let unicode_bundle = DefinitionBundle {
            version: 1,
            sources: vec![DefinitionSource::new(
                "memory:é",
                crate::definitions::bundle::SourceKind::Ecore,
                unicode,
            )],
            namespaces: vec![],
        };
        let unicode_resolver = Resolver::new(&unicode_bundle).unwrap();
        let (resources, standard, standard_id) = unicode_resolver.resource_sets();
        let id = DeclarationId {
            source: unicode_bundle.sources[0].id.clone(),
            path: vec![1, 0, 1],
        };
        let probe = Probe::new(108, None);
        let context = InvocationContext::new(probe.clone());
        let observation = ParserObservation::new(&context);
        assert_eq!(
            reference(resources, standard, standard_id, &id, &at, &observation).unwrap(),
            "memory:é#/1/@eSubpackages.0/@eClassifiers.1"
        );
        assert_eq!(probe.storage.get(), 108);
    }
    #[test]
    fn projection_preserves_certificates_and_recovers_at_every_checkpoint() {
        let bundle = bundle(
            r##"<e:EPackage xmlns:e="http://www.eclipse.org/emf/2002/Ecore" xmlns:x="http://www.w3.org/2001/XMLSchema-instance" name="p" nsURI="urn:p"><eClassifiers x:type="e:EClass" name="A"><eStructuralFeatures x:type="e:EAttribute" name="label"><eGenericType eClassifier="http://www.eclipse.org/emf/2002/Ecore#//EString"/></eStructuralFeatures><eStructuralFeatures x:type="e:EReference" name="next"><eGenericType eClassifier="#//A"/></eStructuralFeatures></eClassifiers></e:EPackage>"##,
        );
        let resolver = Resolver::new(&bundle).unwrap();
        let inventory = resolver.resolve_all().unwrap();
        let prepared = Prepared {
            inventory,
            resolver,
        };
        let (resources, _, _) = prepared.resolver.resource_sets();
        let doc = resources.document(&bundle.sources[0].id).unwrap();
        let original = Xml::from_node_at(&doc.root.children[0], &doc.source.text, vec![0]).unwrap();
        let run = |probe: Rc<Probe>| {
            let mut node = original.clone();
            let context = InvocationContext::new(probe);
            let observation = ParserObservation::new(&context);
            observation.finish(project(&mut node, &prepared, resources, &observation))?;
            Ok::<_, ObservedImportError>(node)
        };
        let full = Probe::new(usize::MAX, None);
        let projected = run(full.clone()).unwrap();
        let assert_output = |node: &Xml| {
            assert_eq!(node.declaration, original.declaration);
            assert_eq!(node.source_payload, original.source_payload);
            assert_eq!(node.children.len(), 2);
            assert_eq!(
                node.children[0].attrs["eType"],
                "http://www.eclipse.org/emf/2002/Ecore#//EString"
            );
            assert_eq!(
                node.children[1].attrs["eType"],
                "memory:x#/0/@eClassifiers.0"
            );
            for (index, child) in node.children.iter().enumerate() {
                assert!(child.children.is_empty());
                assert_eq!(
                    child.attr_spans["eType"],
                    prepared.inventory.types[index].syntax.span
                );
                assert_eq!(
                    child.source_payload,
                    original.children[index].source_payload
                );
            }
        };
        assert_output(&projected);
        assert!(
            matches!(run(Probe::new(full.storage.get() - 1, None)), Err(ObservedImportError::Observation(message)) if message == "projection storage denied")
        );
        for call in 0..full.calls.get() {
            assert!(
                matches!(run(Probe::new(usize::MAX, Some(call))), Err(ObservedImportError::Observation(message)) if message == "projection cancelled")
            );
        }
        for limit in [full.storage.get(), full.storage.get() + 1] {
            assert_output(&run(Probe::new(limit, None)).unwrap());
        }
        for limit in [full.work.get() - 1, full.work.get(), full.work.get() + 1] {
            let probe = Probe::new(full.storage.get(), None);
            probe.work_limit.set(limit);
            let result = run(probe);
            if limit < full.work.get() {
                assert!(
                    matches!(result, Err(ObservedImportError::Observation(message)) if message == "projection work denied")
                );
            } else {
                assert_output(&result.unwrap());
            }
        }
        assert_eq!(original.children[0].children.len(), 1);
        assert_eq!(original.children[1].children.len(), 1);
    }
}
