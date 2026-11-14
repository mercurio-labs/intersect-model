//! Handwritten, deterministic resolution over retained Ecore resources.
//! Indexing and resolving declarations does not make their semantics executable.
use crate::definitions::{
    bundle::{DefinitionBundle, SourceId, SourceKind, SourceSpan, ValidationStep},
    conversion::InvocationContext,
    ecore::{
        Document, ECORE, ErrorKind, ImportError, Node, ObservedImportError, ParserObservation, XMI,
    },
};
use iri_string::types::{IriAbsoluteStr, IriReferenceStr};
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
};
#[path = "resource_index_preparation.rs"]
mod index_preparation;
use index_preparation::OwnedResourceIndexPreparation;
#[path = "resource_set_preparation.rs"]
mod preparation;
pub use preparation::ResourceSetPreparation;
#[path = "resource_bundle_validation.rs"]
mod bundle_validation;
pub(crate) use bundle_validation::ResourceBundleValidation;

/// A declaration in one exact revision, independent of its short name or URI alias.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, serde::Serialize, serde::Deserialize)]
pub struct DeclarationId {
    pub source: SourceId,
    /// Child ordinals in the retained XML tree; the empty path identifies its root.
    pub path: Vec<usize>,
}

#[derive(Clone, Debug)]
struct Resource {
    document: Document,
    roots: Vec<DeclarationId>,
    packages: BTreeMap<String, DeclarationId>,
    ids: BTreeMap<String, DeclarationId>,
}

/// Resource documents and their indexes are immutable after admission. Clones
/// share that immutable storage, while resource/namespace selection maps remain
/// owned by each set. Debug formatting intentionally remains the old logical
/// resource representation used by overload snapshot accounting.
#[derive(Clone, Debug)]
pub struct ResourceSet {
    resources: BTreeMap<SourceId, Arc<ParsedResource>>,
    namespaces: BTreeMap<String, DeclarationId>,
}
// Admission provenance stays with the immutable allocation, keeping ResourceSet
// handles and the established owned-context receipt the same size.
struct ParsedResource {
    resource: Resource,
    limits: crate::definitions::ecore::ParseLimits,
}
impl std::ops::Deref for ParsedResource {
    type Target = Resource;
    fn deref(&self) -> &Resource {
        &self.resource
    }
}
// Preserve the old logical snapshot representation and its exact fees.
impl std::fmt::Debug for ParsedResource {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        std::fmt::Debug::fmt(&self.resource, formatter)
    }
}

fn attr_observed<'a>(
    node: &'a Node,
    namespace: Option<&str>,
    name: &str,
    observation: &ParserObservation<'_, '_>,
) -> Result<Option<&'a str>, ImportError> {
    for attribute in &node.attributes {
        observation.work(1, &node.opening.source)?;
        if attribute.name.namespace.as_deref() == namespace && attribute.name.local == name {
            return Ok(Some(attribute.value.as_str()));
        }
    }
    Ok(None)
}
#[derive(serde::Serialize)]
struct BorrowedDeclaration<'a> {
    source: &'a SourceId,
    path: &'a [usize],
}
fn reserve_declaration(
    source: &SourceId,
    path: &[usize],
    observation: &ParserObservation<'_, '_>,
) -> Result<(), ImportError> {
    observation.serialized(&BorrowedDeclaration { source, path }, source)
}

impl ResourceSet {
    // Conservative provenance: a stricter requested policy must take the normal
    // parser path, preserving all old diagnostics and their precedence.
    pub(crate) fn admitted_for(&self, limits: crate::definitions::ecore::ParseLimits) -> bool {
        self.resources
            .values()
            .all(|resource| resource.limits.source_bytes <= limits.source_bytes)
    }

    pub fn new(bundle: &DefinitionBundle) -> Result<Self, String> {
        Self::new_diagnostic(bundle).map_err(|e| e.to_string())
    }

    pub fn new_diagnostic(bundle: &DefinitionBundle) -> Result<Self, ImportError> {
        Self::new_context(
            bundle,
            &ParserObservation::new(&InvocationContext::default()),
        )
    }
    /// Index the same retained resources with an explicit per-Ecore-source limit.
    pub fn new_with_limits(
        bundle: &DefinitionBundle,
        limits: crate::definitions::ecore::ParseLimits,
    ) -> Result<Self, ImportError> {
        Self::new_context_with_limits(
            bundle,
            &ParserObservation::new(&InvocationContext::default()),
            limits,
        )
    }
    /// Build the same resource indexes with guarded source parsing and ownership.
    /// Resolution calls and native parsing/comparisons remain synchronous.
    pub fn new_observed(
        bundle: &DefinitionBundle,
        context: &InvocationContext<'_>,
    ) -> Result<Self, ObservedImportError> {
        Self::new_observed_with_limits(
            bundle,
            context,
            crate::definitions::ecore::ParseLimits::default(),
        )
    }
    pub fn new_observed_with_limits(
        bundle: &DefinitionBundle,
        context: &InvocationContext<'_>,
        limits: crate::definitions::ecore::ParseLimits,
    ) -> Result<Self, ObservedImportError> {
        context
            .checkpoint()
            .map_err(ObservedImportError::Observation)?;
        let observation = ParserObservation::new(context);
        observation.finish(Self::new_context_with_limits(bundle, &observation, limits))
    }
    pub(crate) fn new_context(
        bundle: &DefinitionBundle,
        observation: &ParserObservation<'_, '_>,
    ) -> Result<Self, ImportError> {
        Self::new_context_with_limits(
            bundle,
            observation,
            crate::definitions::ecore::ParseLimits::default(),
        )
    }
    pub(crate) fn new_context_with_limits(
        bundle: &DefinitionBundle,
        observation: &ParserObservation<'_, '_>,
        limits: crate::definitions::ecore::ParseLimits,
    ) -> Result<Self, ImportError> {
        Self::new_context_with_cursor_policy(bundle, observation, limits, false)
    }
    pub(crate) fn new_context_with_cursor_policy(
        bundle: &DefinitionBundle,
        observation: &ParserObservation<'_, '_>,
        limits: crate::definitions::ecore::ParseLimits,
        borrowed_cursors: bool,
    ) -> Result<Self, ImportError> {
        let bundle_span = validate_bundle_context(bundle, observation)?;
        let mut resources = BTreeMap::new();
        // Every resource is indexed before following any reference. Cyclic source
        // dependencies therefore cannot cause recursive loading or partial indexes.
        for source in bundle
            .sources
            .iter()
            .filter(|s| matches!(s.kind, SourceKind::Ecore | SourceKind::Rcore))
        {
            validate_resource_uri(source, observation)?;
            let document = observation.document_with_limits(source, limits)?;
            let resource = if borrowed_cursors {
                let mut preparation = OwnedResourceIndexPreparation::new(document, observation)?;
                while !preparation.advance(observation)? {}
                preparation.finish()?
            } else {
                let root = &document.root;
                let roots = match resource_root_mode(root)? {
                    RootMode::Single => vec![retain_resource_root(&source.id, &[], observation)?],
                    RootMode::Xmi => {
                        let mut roots = Vec::new();
                        for (index, node) in root.children.iter().enumerate() {
                            observation.work(1, &source.id)?;
                            check_xmi_package_root(node)?;
                            roots.push(retain_resource_root(&source.id, &[index], observation)?);
                        }
                        roots
                    }
                };
                observation.reserve(128, &source.id)?;
                let mut resource = Resource {
                    document,
                    roots,
                    packages: BTreeMap::new(),
                    ids: BTreeMap::new(),
                };
                for root in &resource.roots {
                    observation.work(root.path.len().max(1), &source.id)?;
                    index_package(
                        node_at(&resource.document.root, &root.path)
                            .map_err(|e| invalid(&resource.document.root, None, "", e))?,
                        root,
                        &mut resource.packages,
                        observation,
                    )?;
                }
                reserve_declaration(&source.id, &[], observation)?;
                index_ids(
                    &resource.document.root,
                    &DeclarationId {
                        source: source.id.clone(),
                        path: vec![],
                    },
                    &mut resource.ids,
                    observation,
                )?;
                resource
            };
            observation.serialized(&source.id, &source.id)?;
            observation.reserve(128, &source.id)?;
            resources.insert(
                source.id.clone(),
                Arc::new(ParsedResource { resource, limits }),
            );
        }
        let mut namespaces = BTreeMap::new();
        for binding in &bundle.namespaces {
            insert_namespace_binding(
                &resources,
                binding,
                &bundle_span,
                &mut namespaces,
                observation,
            )?;
        }
        Ok(Self {
            resources,
            namespaces,
        })
    }

    // Complete immutable parsed inputs only. This grants no source semantics,
    // generic bounds, provider or membership authority to a later consumer.
    pub(crate) fn matches_bundle_observed(
        &self,
        bundle: &DefinitionBundle,
        observation: &ParserObservation<'_, '_>,
    ) -> Result<bool, ImportError> {
        if bundle.version != crate::definitions::bundle::BUNDLE_VERSION
            || self.namespaces.len() != bundle.namespaces.len()
        {
            return Ok(false);
        }
        let mut ecore_sources = 0;
        for source in &bundle.sources {
            observation.work(1, &source.id)?;
            if !matches!(source.kind, SourceKind::Ecore | SourceKind::Rcore) {
                // Semantic inputs are admitted independently by Resolver; this
                // resource universe contains only exact Ecore documents.
                continue;
            }
            ecore_sources += 1;
            observation.work(source.text.len().div_ceil(256), &source.id)?;
            let Some(resource) = self.resources.get(&source.id) else {
                return Ok(false);
            };
            let retained = &resource.document.source;
            if retained.id != source.id
                || retained.kind != source.kind
                || retained.text != source.text
                || retained.dependencies != source.dependencies
            {
                return Ok(false);
            }
        }
        if self.resources.len() != ecore_sources {
            return Ok(false);
        }
        for binding in &bundle.namespaces {
            observation.work(1 + binding.namespace.len().div_ceil(256), &binding.source)?;
            let Some(retained) = self.namespaces.get(&binding.namespace) else {
                return Ok(false);
            };
            let Some(resource) = self.resources.get(&binding.source) else {
                return Ok(false);
            };
            if retained.source != binding.source
                || resource.packages.get(&binding.namespace) != Some(retained)
            {
                return Ok(false);
            }
        }
        Ok(true)
    }

    // Move only selected immutable resources; never reconstruct or reparse XML.
    // Selection and last-owner destruction remain synchronous; no latency bound is claimed.
    pub(crate) fn retain_selected_observed(
        &mut self,
        selected: &BTreeSet<SourceId>,
        observation: &ParserObservation<'_, '_>,
    ) -> Result<(), ImportError> {
        for source in self.resources.keys() {
            observation.work(1, source)?;
        }
        for declaration in self.namespaces.values() {
            observation.work(1, &declaration.source)?;
        }
        self.resources.retain(|source, _| selected.contains(source));
        self.namespaces
            .retain(|_, declaration| selected.contains(&declaration.source));
        Ok(())
    }

    pub fn document(&self, source: &SourceId) -> Result<&Document, String> {
        self.resources
            .get(source)
            .map(|r| &r.document)
            .ok_or_else(|| {
                format!(
                    "Missing exact Ecore source {}@{}",
                    source.uri, source.sha256
                )
            })
    }

    pub fn node(&self, id: &DeclarationId) -> Result<&Node, String> {
        self.node_with_work(id, &|_| Ok(()))
    }
    fn node_with_work(
        &self,
        id: &DeclarationId,
        work: &dyn Fn(usize) -> Result<(), String>,
    ) -> Result<&Node, String> {
        self.node_path_with_work(&id.source, &id.path, work)
    }
    fn node_path_with_work(
        &self,
        source: &SourceId,
        path: &[usize],
        work: &dyn Fn(usize) -> Result<(), String>,
    ) -> Result<&Node, String> {
        work(1)?;
        node_at_with_work(&self.document(source)?.root, path, work)
    }
    pub(crate) fn node_context(
        &self,
        id: &DeclarationId,
        observation: &ParserObservation<'_, '_>,
    ) -> Result<&Node, ImportError> {
        self.node_path_context(&id.source, &id.path, observation)
    }
    pub(crate) fn node_path_context(
        &self,
        source: &SourceId,
        path: &[usize],
        observation: &ParserObservation<'_, '_>,
    ) -> Result<&Node, ImportError> {
        self.node_path_with_work(source, path, &|units| {
            observation
                .work(units, source)
                .map_err(|error| error.message)
        })
        .map_err(|message| resolution_error(source, message))
    }

    pub fn packages(&self, source: &SourceId) -> Result<&BTreeMap<String, DeclarationId>, String> {
        self.resources
            .get(source)
            .map(|r| &r.packages)
            .ok_or_else(|| format!("Missing exact Ecore source {}", source.uri))
    }

    /// Exact namespace selection already validated while constructing this set.
    pub fn namespace_binding(&self, namespace: &str) -> Option<&DeclarationId> {
        self.namespaces.get(namespace)
    }

    /// Inspect selected package identities and the supplied schema's declarations.
    /// This copies metadata; it does not verify the schema or grant compilation,
    /// execution, provider or source-origin authority.
    pub fn inspect_schema_declarations<'a>(
        &self,
        sources: impl IntoIterator<Item = &'a SourceId>,
        schema: &crate::definitions::schema::Schema,
    ) -> Result<crate::definitions::declarations::DeclarationInventory, ImportError> {
        let mut inventory = crate::definitions::declarations::DeclarationInventory::default();
        for source in sources {
            for (namespace, declaration) in self
                .packages(source)
                .map_err(|message| resolution_error(source, message))?
            {
                if self.namespace_binding(namespace) == Some(declaration) {
                    inventory
                        .packages
                        .insert(namespace.clone(), declaration.clone());
                }
            }
        }
        for declaration in schema.declarations.values() {
            inventory
                .classifiers
                .entry(declaration.namespace.clone())
                .or_default()
                .insert(
                    declaration.name.clone(),
                    DeclarationId {
                        source: declaration.source.clone(),
                        path: declaration.path.clone(),
                    },
                );
        }
        Ok(inventory)
    }

    /// Resolve against the referring resource's explicit exact dependencies.
    /// Namespace selection never authorizes an undeclared source dependency.
    pub fn resolve(&self, from: &SourceId, href: &str) -> Result<DeclarationId, String> {
        self.resolve_context(
            from,
            href,
            &ParserObservation::new(&InvocationContext::default()),
        )
        .map_err(|error| error.message)
    }
    /// Guard the shared resolver's owned URI/fragment/identity results. Native
    /// URI formatting, validation and comparisons remain synchronous boundaries.
    pub fn resolve_observed(
        &self,
        from: &SourceId,
        href: &str,
        context: &InvocationContext<'_>,
    ) -> Result<DeclarationId, ObservedImportError> {
        context
            .checkpoint()
            .map_err(ObservedImportError::Observation)?;
        let observation = ParserObservation::new(context);
        observation.finish(self.resolve_context(from, href, &observation))
    }
    pub(crate) fn resolve_context(
        &self,
        from: &SourceId,
        href: &str,
        observation: &ParserObservation<'_, '_>,
    ) -> Result<DeclarationId, ImportError> {
        let fail = |message| resolution_error(from, message);
        let source = &self.document(from).map_err(fail)?.source;
        observation.work(href.len().div_ceil(256).max(1), from)?;
        IriReferenceStr::new(href)
            .map_err(|e| fail(format!("Invalid Ecore URI reference {href}: {e}")))?;
        let (address, fragment) = href.split_once('#').unwrap_or((href, ""));
        let allowed = |id: &SourceId| -> Result<bool, ImportError> {
            observation.work(1, from)?;
            if id == from {
                return Ok(true);
            }
            for dependency in &source.dependencies {
                observation.work(1, from)?;
                if dependency == id {
                    return Ok(true);
                }
            }
            Ok(false)
        };
        if let Some(package) = self.namespaces.get(address) {
            if !allowed(&package.source)? {
                return Err(fail(format!(
                    "Undeclared exact source dependency for namespace {address}"
                )));
            }
            return self.fragment(
                &package.source,
                std::slice::from_ref(package),
                fragment,
                observation,
            );
        }
        // Internal lowering can emit an exact self URI even when a caller uses
        // a relative resource identity. No external dependency is authorized here.
        let target = if address.is_empty() || address == from.uri {
            observation.serialized(from, from)?;
            from.clone()
        } else {
            let address = absolute_reference(&from.uri, address, from, observation)?;
            let mut selected = None;
            let mut ambiguous = false;
            for id in self.resources.keys() {
                observation.work(1, from)?;
                if !allowed(id)? {
                    continue;
                }
                let matches = match canonical_uri(&id.uri, from, observation) {
                    Ok(uri) => uri == address,
                    Err(error) if error.kind == ErrorKind::ResourceLimit => return Err(error),
                    Err(_) => false,
                };
                if matches {
                    ambiguous |= selected.is_some();
                    selected = Some(id);
                }
            }
            if ambiguous {
                return Err(fail(format!("Ambiguous resource revision: {address}")));
            }
            match selected {
                Some(id) => {
                    observation.serialized(id, from)?;
                    id.clone()
                }
                None => {
                    return Err(fail(format!(
                        "Missing resource or undeclared exact dependency: {address}"
                    )));
                }
            }
        };
        let resource = self
            .resources
            .get(&target)
            .ok_or_else(|| fail("Missing selected Ecore resource".into()))?;
        self.fragment(&target, &resource.roots, fragment, observation)
    }

    fn fragment(
        &self,
        source: &SourceId,
        roots: &[DeclarationId],
        fragment: &str,
        observation: &ParserObservation<'_, '_>,
    ) -> Result<DeclarationId, ImportError> {
        let fail = |message| resolution_error(source, message);
        if !fragment.is_empty() && !fragment.starts_with('/') {
            let id = decode_segment(fragment, source, observation)?;
            let declaration = self
                .resources
                .get(source)
                .and_then(|r| r.ids.get(&id))
                .ok_or_else(|| fail(format!("Missing XMI ID {id} in {}", source.uri)))?;
            reserve_declaration(&declaration.source, &declaration.path, observation)?;
            return Ok(declaration.clone());
        }
        let (mut current, segments) = if let Some(path) = fragment.strip_prefix("//") {
            if roots.len() != 1 {
                return Err(fail(
                    "Ambiguous default root in multi-package resource; specify its root index"
                        .into(),
                ));
            }
            reserve_declaration(&roots[0].source, &roots[0].path, observation)?;
            (roots[0].clone(), path)
        } else if fragment.is_empty() || fragment == "/" {
            if roots.len() != 1 {
                return Err(fail("Ambiguous resource root".into()));
            }
            reserve_declaration(&roots[0].source, &roots[0].path, observation)?;
            return Ok(roots[0].clone());
        } else {
            let path = fragment
                .strip_prefix('/')
                .ok_or_else(|| fail("Invalid resource fragment".into()))?;
            let (index, tail) = path.split_once('/').unwrap_or((path, ""));
            let index = index
                .parse::<usize>()
                .map_err(|_| fail("Invalid resource root index".into()))?;
            let root = roots
                .get(index)
                .ok_or_else(|| fail("Resource root index out of bounds".into()))?;
            reserve_declaration(&root.source, &root.path, observation)?;
            (root.clone(), tail)
        };
        if segments.is_empty() {
            return Ok(current);
        }
        for segment in segments.split('/') {
            observation.work(1, source)?;
            let segment = decode_segment(segment, source, observation)?;
            for _ in &current.path {
                observation.work(1, source)?;
            }
            let node = self.node(&current).map_err(fail)?;
            let index = if let Some(feature) = segment.strip_prefix('@') {
                let (name, ordinal) = feature.rsplit_once('.').unwrap_or((feature, "0"));
                let ordinal = ordinal
                    .parse::<usize>()
                    .map_err(|_| fail("Invalid containment ordinal".into()))?;
                let mut found = None;
                let mut remaining = ordinal;
                for (index, child) in node.children.iter().enumerate() {
                    observation.work(1, source)?;
                    if child.name.namespace.is_none() && child.name.local == name {
                        if remaining == 0 {
                            found = Some(index);
                            break;
                        }
                        remaining -= 1;
                    }
                }
                found.ok_or_else(|| fail(format!("Missing structural fragment {segment}")))?
            } else {
                let mut selected = None;
                let mut ambiguous = false;
                for (index, child) in node.children.iter().enumerate() {
                    observation.work(1, source)?;
                    if child.name.namespace.is_none()
                        && attr_observed(child, None, "name", observation)?
                            == Some(segment.as_str())
                    {
                        ambiguous |= selected.is_some();
                        selected = Some(index);
                    }
                }
                if ambiguous {
                    return Err(fail(format!(
                        "Ambiguous named fragment {segment} in {}",
                        source.uri
                    )));
                }
                match selected {
                    Some(index) => index,
                    None => {
                        return Err(fail(format!(
                            "Missing named fragment {segment} in {}",
                            source.uri
                        )));
                    }
                }
            };
            observation.reserve(32, source)?;
            current.path.push(index);
        }
        Ok(current)
    }
}

fn node_at<'a>(root: &'a Node, path: &[usize]) -> Result<&'a Node, String> {
    node_at_with_work(root, path, &|_| Ok(()))
}
fn node_at_with_work<'a>(
    root: &'a Node,
    path: &[usize],
    work: &dyn Fn(usize) -> Result<(), String>,
) -> Result<&'a Node, String> {
    let mut node = root;
    for index in path {
        work(1)?;
        node = node
            .children
            .get(*index)
            .ok_or("Declaration path out of bounds")?;
    }
    Ok(node)
}

fn invalid(
    node: &Node,
    namespace: Option<&str>,
    name: &str,
    message: impl Into<String>,
) -> ImportError {
    let span = node
        .attributes
        .iter()
        .find(|a| a.name.namespace.as_deref() == namespace && a.name.local == name)
        .map(|a| &a.span)
        .unwrap_or(&node.opening);
    ImportError::new(ErrorKind::InvalidDefinition, span, message)
}
fn validate_bundle_context(
    bundle: &DefinitionBundle,
    observation: &ParserObservation<'_, '_>,
) -> Result<SourceSpan, ImportError> {
    let (bundle_span, mut validation) = ResourceBundleValidation::begin(bundle, observation)?;
    while !validation.advance(&bundle_span, observation)? {}
    Ok(bundle_span)
}
fn validate_resource_uri(
    source: &crate::definitions::bundle::DefinitionSource,
    observation: &ParserObservation<'_, '_>,
) -> Result<(), ImportError> {
    observation.work(1 + source.id.uri.len().div_ceil(256), &source.id)?;
    let source_span = observation.span(&source.id, 0, 0)?;
    IriReferenceStr::new(&source.id.uri).map_err(|e| {
        ImportError::new(
            ErrorKind::InvalidDefinition,
            &source_span,
            format!("Invalid resource URI {}: {e}", source.id.uri),
        )
    })?;
    if source.id.uri.contains('#') {
        return Err(ImportError::new(
            ErrorKind::InvalidDefinition,
            &source_span,
            "Resource URI must not contain a fragment",
        ));
    }
    Ok(())
}
fn insert_namespace_binding(
    resources: &BTreeMap<SourceId, Arc<ParsedResource>>,
    binding: &crate::definitions::bundle::NamespaceBinding,
    bundle_span: &SourceSpan,
    namespaces: &mut BTreeMap<String, DeclarationId>,
    observation: &ParserObservation<'_, '_>,
) -> Result<(), ImportError> {
    observation.work(1, &binding.source)?;
    let resource = resources.get(&binding.source).ok_or_else(|| {
        ImportError::new(
            ErrorKind::InvalidDefinition,
            bundle_span,
            "Namespace binding must select an Ecore resource",
        )
    })?;
    let package = resource.packages.get(&binding.namespace).ok_or_else(|| {
        invalid(
            &resource.document.root,
            None,
            "nsURI",
            format!(
                "Namespace {} is not declared in {}",
                binding.namespace, binding.source.uri
            ),
        )
    })?;
    observation.string(binding.namespace.len(), &binding.source)?;
    reserve_declaration(&package.source, &package.path, observation)?;
    observation.reserve(128, &binding.source)?;
    namespaces.insert(binding.namespace.clone(), package.clone());
    Ok(())
}

fn index_package(
    node: &Node,
    id: &DeclarationId,
    packages: &mut BTreeMap<String, DeclarationId>,
    observation: &ParserObservation<'_, '_>,
) -> Result<(), ImportError> {
    let namespace = index_package_header(node, &id.source, &id.path, packages, observation)?;
    let mut names = BTreeSet::new();
    for (i, child) in node.children.iter().enumerate() {
        if check_package_member(child, namespace, &mut names, &id.source, observation)? {
            reserve_declaration(&id.source, &id.path, observation)?;
            observation.reserve(32, &id.source)?;
            let mut nested = id.clone();
            nested.path.push(i);
            index_package(child, &nested, packages, observation)?;
        }
    }
    Ok(())
}
fn index_package_header<'node>(
    node: &'node Node,
    source: &SourceId,
    path: &[usize],
    packages: &mut BTreeMap<String, DeclarationId>,
    observation: &ParserObservation<'_, '_>,
) -> Result<&'node str, ImportError> {
    observation.work(1, source)?;
    let namespace = attr_observed(node, None, "nsURI", observation)?
        .filter(|s| !s.is_empty())
        .ok_or_else(|| invalid(node, None, "nsURI", "EPackage requires nsURI"))?;
    observation.work(namespace.len().div_ceil(256).max(1), source)?;
    IriReferenceStr::new(namespace).map_err(|e| {
        invalid(
            node,
            None,
            "nsURI",
            format!("Invalid package namespace URI: {e}"),
        )
    })?;
    attr_observed(node, None, "name", observation)?
        .filter(|s| !s.is_empty())
        .ok_or_else(|| invalid(node, None, "name", "EPackage requires a name"))?;
    observation.string(namespace.len(), source)?;
    reserve_declaration(source, path, observation)?;
    observation.reserve(128, source)?;
    if packages
        .insert(
            namespace.into(),
            DeclarationId {
                source: source.clone(),
                path: path.to_vec(),
            },
        )
        .is_some()
    {
        return Err(invalid(
            node,
            None,
            "nsURI",
            format!("Duplicate package namespace {namespace} in {}", source.uri),
        ));
    }
    Ok(namespace)
}
fn check_package_member<'node>(
    child: &'node Node,
    namespace: &str,
    names: &mut BTreeSet<(&'node str, &'node str)>,
    source: &SourceId,
    observation: &ParserObservation<'_, '_>,
) -> Result<bool, ImportError> {
    observation.work(1, source)?;
    if child.name.namespace.is_some()
        || !["eSubpackages", "eClassifiers"].contains(&child.name.local.as_str())
    {
        return Ok(false);
    }
    let name = attr_observed(child, None, "name", observation)?
        .filter(|s| !s.is_empty())
        .ok_or_else(|| invalid(child, None, "name", "Package member requires a name"))?;
    observation.reserve(128, source)?;
    if !names.insert((child.name.local.as_str(), name)) {
        return Err(invalid(
            child,
            None,
            "name",
            format!("Duplicate package member {name} in {namespace}"),
        ));
    }
    Ok(child.name.local == "eSubpackages")
}
#[derive(Clone, Copy)]
enum RootMode {
    Single,
    Xmi,
}
fn resource_root_mode(root: &Node) -> Result<RootMode, ImportError> {
    if root.name.namespace.as_deref() == Some(ECORE) && root.name.local == "EPackage" {
        Ok(RootMode::Single)
    } else if root.name.namespace.as_deref() == Some(XMI) && root.name.local == "XMI" {
        if root.children.is_empty() {
            return Err(invalid(root, None, "", "Empty XMI resource"));
        }
        Ok(RootMode::Xmi)
    } else {
        Err(ImportError::new(
            ErrorKind::Unsupported,
            &root.opening,
            "Expected EPackage or xmi:XMI resource",
        ))
    }
}
fn check_xmi_package_root(node: &Node) -> Result<(), ImportError> {
    if node.name.namespace.as_deref() != Some(ECORE) || node.name.local != "EPackage" {
        return Err(ImportError::new(
            ErrorKind::Unsupported,
            &node.opening,
            format!("Unsupported Ecore resource root {}", node.lexical_name),
        ));
    }
    Ok(())
}
fn retain_resource_root(
    source: &SourceId,
    path: &[usize],
    observation: &ParserObservation<'_, '_>,
) -> Result<DeclarationId, ImportError> {
    reserve_declaration(source, path, observation)?;
    observation.reserve(32, source)?;
    Ok(DeclarationId {
        source: source.clone(),
        path: path.to_vec(),
    })
}

fn index_ids(
    node: &Node,
    id: &DeclarationId,
    ids: &mut BTreeMap<String, DeclarationId>,
    observation: &ParserObservation<'_, '_>,
) -> Result<(), ImportError> {
    index_id_at_node(node, &id.source, &id.path, ids, observation)?;
    for (index, child) in node.children.iter().enumerate() {
        observation.work(1, &id.source)?;
        reserve_declaration(&id.source, &id.path, observation)?;
        observation.reserve(32, &id.source)?;
        let mut nested = id.clone();
        nested.path.push(index);
        index_ids(child, &nested, ids, observation)?;
    }
    Ok(())
}

fn index_id_at_node(
    node: &Node,
    source: &SourceId,
    path: &[usize],
    ids: &mut BTreeMap<String, DeclarationId>,
    observation: &ParserObservation<'_, '_>,
) -> Result<(), ImportError> {
    observation.work(1, source)?;
    if let Some(name) = attr_observed(node, Some(XMI), "id", observation)? {
        observation.work(name.len().div_ceil(256).max(1), source)?;
        if name.is_empty() || name.chars().any(char::is_whitespace) {
            return Err(invalid(
                node,
                Some(XMI),
                "id",
                "Invalid empty or whitespace XMI ID",
            ));
        }
        observation.string(name.len(), source)?;
        reserve_declaration(source, path, observation)?;
        observation.reserve(128, source)?;
        if ids
            .insert(
                name.into(),
                DeclarationId {
                    source: source.clone(),
                    path: path.to_vec(),
                },
            )
            .is_some()
        {
            return Err(invalid(
                node,
                Some(XMI),
                "id",
                format!("Duplicate XMI ID {name}"),
            ));
        }
    }
    Ok(())
}

fn resolution_error(source: &SourceId, message: String) -> ImportError {
    ImportError::new(
        ErrorKind::InvalidDefinition,
        &SourceSpan {
            source: source.clone(),
            start: 0,
            end: 0,
        },
        message,
    )
}
fn canonical_uri(
    uri: &str,
    source: &SourceId,
    observation: &ParserObservation<'_, '_>,
) -> Result<String, ImportError> {
    observation.work(uri.len().div_ceil(256).max(1), source)?;
    match IriAbsoluteStr::new(uri) {
        Ok(uri) => observation.formatted(&uri.normalize(), source),
        Err(_) => {
            let uri =
                IriReferenceStr::new(uri).map_err(|e| resolution_error(source, e.to_string()))?;
            observation.formatted(&uri, source)
        }
    }
}
fn absolute_reference(
    base: &str,
    address: &str,
    source: &SourceId,
    observation: &ParserObservation<'_, '_>,
) -> Result<String, ImportError> {
    observation.work(address.len().div_ceil(256).max(1), source)?;
    if let Ok(absolute) = IriAbsoluteStr::new(address) {
        return observation.formatted(&absolute.normalize(), source);
    }
    observation.work(base.len().div_ceil(256).max(1), source)?;
    let base = IriAbsoluteStr::new(base).map_err(|_| {
        resolution_error(
            source,
            "Relative references require an absolute resource base URI".into(),
        )
    })?;
    let reference =
        IriReferenceStr::new(address).map_err(|e| resolution_error(source, e.to_string()))?;
    let resolved = observation.formatted(&reference.resolve_against(base), source)?;
    canonical_uri(&resolved, source, observation)
}
fn decode_segment(
    segment: &str,
    source: &SourceId,
    observation: &ParserObservation<'_, '_>,
) -> Result<String, ImportError> {
    let work = observation.size(segment.len().div_ceil(256).max(1).checked_mul(3), source)?;
    observation.work(work, source)?;
    let mut length = 0;
    visit_segment(segment, &mut |_| {
        length += 1;
    })
    .map_err(|message| resolution_error(source, message))?;
    observation.string(length, source)?;
    let mut bytes = Vec::with_capacity(length);
    visit_segment(segment, &mut |byte| bytes.push(byte))
        .map_err(|message| resolution_error(source, message))?;
    String::from_utf8(bytes)
        .map_err(|_| resolution_error(source, "URI fragment is not valid UTF-8".into()))
}
fn visit_segment(segment: &str, visit: &mut impl FnMut(u8)) -> Result<(), String> {
    let mut input = segment.as_bytes().iter().copied();
    while let Some(byte) = input.next() {
        if byte == b'%' {
            let hex = |b: u8| {
                (b as char)
                    .to_digit(16)
                    .map(|n| n as u8)
                    .ok_or("Invalid percent escape")
            };
            let high = hex(input.next().ok_or("Incomplete percent escape")?)?;
            let low = hex(input.next().ok_or("Incomplete percent escape")?)?;
            visit(high * 16 + low);
        } else {
            visit(byte);
        }
    }
    Ok(())
}

#[cfg(test)]
mod parsed_input_matching_controls {
    use super::*;
    use crate::definitions::bundle::{DefinitionSource, NamespaceBinding};
    use crate::definitions::conversion::{InvocationObserver, ProviderId};
    use std::{cell::Cell, rc::Rc};
    fn fixture() -> DefinitionBundle {
        let source = DefinitionSource::new(
            "memory:parsed-input.ecore",
            SourceKind::Ecore,
            "<e:EPackage xmlns:e='http://www.eclipse.org/emf/2002/Ecore' name='p' nsURI='urn:parsed'><eClassifiers name='Node' instanceClassName='opaque.ÃƒÆ’Ã…Â½Ãƒâ€šÃ‚Â±'/></e:EPackage>",
        );
        DefinitionBundle {
            version: 1,
            namespaces: vec![NamespaceBinding {
                namespace: "urn:parsed".into(),
                source: source.id.clone(),
            }],
            sources: vec![source],
        }
    }
    #[test]
    fn complete_parsed_resource_matching_rejects_changed_payload_dependencies_and_namespace_inputs()
    {
        let bundle = fixture();
        let resources = ResourceSet::new(&bundle).unwrap();
        let context = InvocationContext::default();
        let observation = ParserObservation::new(&context);
        assert!(
            resources
                .matches_bundle_observed(&bundle, &observation)
                .unwrap()
        );
        let original = resources
            .document(&bundle.sources[0].id)
            .unwrap()
            .source
            .clone();
        let mut variants = Vec::new();
        let mut changed = bundle.clone();
        changed.version = 2;
        variants.push(changed);
        let mut changed = bundle.clone();
        changed.sources[0].text.push(' ');
        variants.push(changed);
        let mut changed = bundle.clone();
        changed.sources[0].kind = SourceKind::RcoreBehavior;
        variants.push(changed);
        let mut changed = bundle.clone();
        let dependency = changed.sources[0].id.clone();
        changed.sources[0].dependencies.push(dependency);
        variants.push(changed);
        let mut changed = bundle.clone();
        changed.sources[0].id.sha256 = "0".repeat(64);
        variants.push(changed);
        let mut changed = bundle.clone();
        changed.namespaces[0].namespace = "urn:foreign".into();
        variants.push(changed);
        let mut changed = bundle.clone();
        changed.namespaces.clear();
        variants.push(changed);
        for changed in variants {
            assert!(
                !resources
                    .matches_bundle_observed(&changed, &observation)
                    .unwrap()
            );
        }
        let retained = &resources.document(&bundle.sources[0].id).unwrap().source;
        assert_eq!(retained.id, original.id);
        assert_eq!(retained.kind, original.kind);
        assert_eq!(retained.text, original.text);
        assert_eq!(retained.dependencies, original.dependencies);
        assert!(
            resources
                .matches_bundle_observed(&bundle, &observation)
                .unwrap()
        );
    }
    #[test]
    fn exact_ecore_resources_are_reused_with_independently_admitted_semantic_inputs() {
        let mut bundle = fixture();
        bundle.sources.push(DefinitionSource::new(
            "memory:extra.rcore",
            SourceKind::RcoreBehavior,
            "semantic inputs are validated by their own consumer",
        ));
        let resources = ResourceSet::new(&bundle).unwrap();
        let pointer = resources
            .document(&bundle.sources[0].id)
            .unwrap()
            .source
            .text
            .as_ptr();
        let operation = InvocationContext::default();
        let observation = ParserObservation::new(&operation);
        assert!(
            resources
                .matches_bundle_observed(&bundle, &observation)
                .unwrap()
        );
        let standard =
            crate::definitions::compiler::standard_datatype_source_observed(&observation).unwrap();
        let resolver =
            crate::definitions::generic_resolution::Resolver::with_standard_source_and_borrowed_cursors(
                &bundle,
                standard,
                Some(resources),
                &observation,
            )
            .unwrap();
        assert_eq!(
            resolver
                .resource_sets()
                .0
                .document(&bundle.sources[0].id)
                .unwrap()
                .source
                .text
                .as_ptr(),
            pointer
        );
    }
    #[test]
    fn parsed_input_matching_borrows_full_data_cancels_each_step_and_recovers() {
        struct Meter {
            work: Cell<usize>,
            calls: Cell<usize>,
            stop: usize,
        }
        impl InvocationObserver for Meter {
            fn before_invoke(&self, _: &ProviderId) -> Result<(), String> {
                Err("no providers".into())
            }
            fn work(&self, n: usize) -> Result<(), String> {
                let call = self.calls.get();
                self.calls.set(call + 1);
                if call == self.stop {
                    return Err("parsed input cancelled".into());
                }
                self.work.set(self.work.get() + n);
                Ok(())
            }
            fn retain_storage(&self, n: usize) -> Result<(), String> {
                assert_eq!(n, 0);
                Ok(())
            }
        }
        let bundle = fixture();
        let resources = ResourceSet::new(&bundle).unwrap();
        let run = |stop| {
            let meter = Rc::new(Meter {
                work: Cell::new(0),
                calls: Cell::new(0),
                stop,
            });
            let context = InvocationContext::new(meter.clone());
            let observation = ParserObservation::new(&context);
            let result =
                observation.finish(resources.matches_bundle_observed(&bundle, &observation));
            (result, meter.work.get(), meter.calls.get())
        };
        let (result, work, calls) = run(usize::MAX);
        assert!(result.unwrap());
        assert!(work > 0 && calls > 0);
        for stop in 0..calls {
            assert!(
                matches!(run(stop).0,Err(ObservedImportError::Observation(message)) if message=="parsed input cancelled")
            );
            assert!(run(usize::MAX).0.unwrap());
        }
    }
}

#[cfg(test)]
#[path = "resources_cursor_tests.rs"]
mod cursor_input_controls;

#[cfg(test)]
#[path = "resource_set_preparation_tests.rs"]
mod retained_preparation_controls;

#[cfg(test)]
mod shared_resource_tests {
    use super::*;
    use crate::definitions::{
        bundle::{DefinitionSource, NamespaceBinding},
        conversion::{InvocationObserver, ProviderId},
        rcore_behavior::checking::snapshot_charge,
    };
    use std::{cell::Cell, rc::Rc};

    fn fixture() -> DefinitionBundle {
        let a = DefinitionSource::new(
            "memory:shared-a.ecore",
            SourceKind::Ecore,
            "<e:EPackage xmlns:e='http://www.eclipse.org/emf/2002/Ecore' xmlns:xsi='http://www.w3.org/2001/XMLSchema-instance' name='p' nsURI='urn:shared:a'><eClassifiers xsi:type='e:EClass' name='Café'/></e:EPackage>",
        );
        let b = DefinitionSource::new(
            "memory:shared-b.rcore",
            SourceKind::Rcore,
            "rcore 1;package p version \"1\";metamodel \"urn:shared:b\" prefix b {class Node {attr label:String [0,1];}}",
        );
        DefinitionBundle {
            version: 1,
            namespaces: vec![
                NamespaceBinding {
                    namespace: "urn:shared:a".into(),
                    source: a.id.clone(),
                },
                NamespaceBinding {
                    namespace: "urn:shared:b".into(),
                    source: b.id.clone(),
                },
            ],
            sources: vec![a, b],
        }
    }

    struct Meter {
        calls: Cell<usize>,
        stop: usize,
    }
    impl InvocationObserver for Meter {
        fn before_invoke(&self, _: &ProviderId) -> Result<(), String> {
            Err("No provider belongs to resource selection".into())
        }
        fn work(&self, n: usize) -> Result<(), String> {
            assert_eq!(n, 1);
            let call = self.calls.get();
            self.calls.set(call + 1);
            if call == self.stop {
                Err("shared selection cancelled".into())
            } else {
                Ok(())
            }
        }
        fn retain_storage(&self, _: usize) -> Result<(), String> {
            Err("Selection must not charge new storage".into())
        }
    }

    fn check(resources: ResourceSet, bundle: &DefinitionBundle) -> Result<(), String> {
        let deep: BTreeMap<_, Resource> = resources
            .resources
            .iter()
            .map(|(id, resource)| (id.clone(), resource.as_ref().resource.clone()))
            .collect();
        // This reconstructs the pre-sharing field layout using actual deep values,
        // rather than comparing a clone's Debug output with itself.
        let legacy = format!(
            "ResourceSet {{ resources: {deep:?}, namespaces: {:?} }}",
            resources.namespaces
        );
        assert_eq!(format!("{resources:?}"), legacy);
        let mut exact = legacy.len();
        snapshot_charge(&mut exact, &resources)?;
        assert_eq!(exact, 0);
        let mut short = legacy.len() - 1;
        assert_eq!(
            snapshot_charge(&mut short, &resources),
            Err("Overload snapshot byte limit exceeded".into())
        );
        assert_eq!(short, 0);
        let clone = resources.clone();
        for source in &bundle.sources {
            let left = resources
                .resources
                .get(&source.id)
                .ok_or("Missing original resource")?;
            let right = clone
                .resources
                .get(&source.id)
                .ok_or("Missing cloned resource")?;
            assert!(Arc::ptr_eq(left, right));
            let a = resources.document(&source.id)?;
            let b = clone.document(&source.id)?;
            assert!(std::ptr::eq(a, b));
            assert_eq!(a.source.text.as_ptr(), b.source.text.as_ptr());
            assert_eq!(a.root.children.as_ptr(), b.root.children.as_ptr());
        }
        let selected = BTreeSet::from([bundle.sources[0].id.clone()]);
        let calls = resources.resources.len() + resources.namespaces.len();
        assert_eq!(calls, 4);
        for stop in 0..=calls {
            let mut candidate = resources.clone();
            let meter = Rc::new(Meter {
                calls: Cell::new(0),
                stop,
            });
            let context = InvocationContext::new(meter.clone());
            let observation = ParserObservation::new(&context);
            let result =
                observation.finish(candidate.retain_selected_observed(&selected, &observation));
            if stop < calls {
                assert!(
                    matches!(result,Err(crate::definitions::ecore::ObservedImportError::Observation(ref m)) if m=="shared selection cancelled")
                );
                assert_eq!(format!("{candidate:?}"), legacy);
                assert_eq!(meter.calls.get(), stop + 1);
            } else {
                result.map_err(|error| error.to_string())?;
                assert_eq!(meter.calls.get(), calls);
                assert_eq!(candidate.resources.len(), 1);
                assert_eq!(candidate.namespaces.len(), 1);
                assert!(candidate.document(&bundle.sources[1].id).is_err());
                assert!(candidate.namespace_binding("urn:shared:b").is_none());
                assert!(std::ptr::eq(
                    candidate.document(&bundle.sources[0].id)?,
                    resources.document(&bundle.sources[0].id)?
                ));
            }
            assert_eq!(format!("{resources:?}"), legacy);
            assert_eq!(format!("{clone:?}"), legacy);
        }
        drop(resources);
        for source in &bundle.sources {
            assert_eq!(clone.document(&source.id)?.source.id, source.id);
        }
        Ok(())
    }

    #[test]
    fn resource_sharing_keeps_debug_fees_selection_isolation_and_lifetimes() -> Result<(), String> {
        let bundle = fixture();
        let resources = ResourceSet::new(&bundle)?;
        let expected = format!("{resources:?}");
        check(resources, &bundle)?;
        for quantum in [1, 3, 256] {
            let context = InvocationContext::default();
            let mut preparation = ResourceSetPreparation::begin_with_retained_bundle(
                &bundle,
                Default::default(),
                &context,
            )
            .map_err(|e| e.to_string())?;
            let mut complete = false;
            let mut steps = 0;
            while !complete {
                for _ in 0..quantum {
                    complete = preparation.advance(&context).map_err(|e| e.to_string())?;
                    steps += 1;
                    if steps > 100_000 {
                        return Err("Resource preparation did not finish".into());
                    }
                    if complete {
                        break;
                    }
                }
            }
            let resources = preparation.finish().map_err(|e| e.to_string())?;
            assert_eq!(format!("{resources:?}"), expected);
            check(resources, &bundle)?;
        }
        Ok(())
    }
}
