//! Canonical bundle compilation and shared structural lowering for inspection.
use super::{
    SchemaLoweringPolicy, Xml, attrs, lower_datatype_observed, lower_enumeration_observed,
    lower_schema_with_datatypes, required,
};
use crate::definitions::{
    bundle::{DefinitionBundle, SourceId, SourceKind},
    declarations::DeclarationInventory,
    ecore::{ErrorKind, ImportError, ObservedImportError, ParserObservation, PayloadKind},
    resources::{DeclarationId, ResourceSet},
    schema::{ClassifierDeclaration, Schema},
};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};

/// Ecore may be inspected or migrated, but cannot enter executable schema APIs.
pub(crate) fn require_canonical_sources(bundle: &DefinitionBundle) -> Result<(), ImportError> {
    let mut preparation = CanonicalSourcePreparation::default();
    while !preparation.advance(bundle)? {}
    Ok(())
}

/// Handwritten scan over exact imported source kinds. No observer or authority
/// is retained; callers own their checkpoint and accounting boundaries.
#[derive(Clone, Copy, Default)]
pub(super) struct CanonicalSourcePreparation {
    index: usize,
}
impl CanonicalSourcePreparation {
    pub(super) fn advance(&mut self, bundle: &DefinitionBundle) -> Result<bool, ImportError> {
        let Some(source) = bundle.sources.get(self.index) else {
            return Ok(true);
        };
        if source.kind == SourceKind::Ecore {
            return Err(ImportError::new(
                ErrorKind::Unsupported,
                &crate::definitions::bundle::SourceSpan {
                    source: source.id.clone(),
                    start: 0,
                    end: source.text.len(),
                },
                "Ecore sources are migration inputs; migrate the complete bundle before compiling structure",
            ));
        }
        self.index += 1;
        Ok(false)
    }
}

/// Compile canonical Rcore resources reachable from an explicitly selected package.
/// Primary package class keys remain their names; other keys are opaque and can
/// be selected using `Schema::class_key`. Provenance identifies every classifier.
pub fn compile_structure_bundle(
    bundle: &DefinitionBundle,
    entry_source: &SourceId,
    namespace: &str,
) -> Result<Schema, ImportError> {
    require_canonical_sources(bundle)?;
    lower_bundle_context(bundle, entry_source, namespace, None)
}
/// Explicit source-size policy; default entry points retain their original limits.
/// This compiles structure only and does not supply derived/domain semantics.
pub fn compile_structure_bundle_with_limits(
    bundle: &DefinitionBundle,
    entry_source: &SourceId,
    namespace: &str,
    limits: crate::definitions::ecore::ParseLimits,
) -> Result<Schema, ImportError> {
    require_canonical_sources(bundle)?;
    compile_context_with_limits(
        bundle,
        entry_source,
        namespace,
        None,
        &ParserObservation::new(&crate::definitions::conversion::InvocationContext::default()),
        limits,
    )
}
/// The explicit policy shares the observed compiler and optional conversion path.
pub fn compile_structure_bundle_with_limits_observed(
    bundle: &DefinitionBundle,
    entry_source: &SourceId,
    namespace: &str,
    conversions: Option<&crate::definitions::conversion::ConversionRegistry<'_>>,
    context: &crate::definitions::conversion::InvocationContext<'_>,
    limits: crate::definitions::ecore::ParseLimits,
) -> Result<Schema, ObservedImportError> {
    require_canonical_sources(bundle).map_err(ObservedImportError::Import)?;
    context
        .checkpoint()
        .map_err(ObservedImportError::Observation)?;
    let observation = ParserObservation::new(context);
    observation.finish(compile_context_with_limits(
        bundle,
        entry_source,
        namespace,
        conversions,
        &observation,
        limits,
    ))
}
pub fn compile_structure_bundle_with_conversions(
    bundle: &DefinitionBundle,
    entry_source: &SourceId,
    namespace: &str,
    conversions: &crate::definitions::conversion::ConversionRegistry<'_>,
) -> Result<Schema, ImportError> {
    require_canonical_sources(bundle)?;
    lower_bundle_context(bundle, entry_source, namespace, Some(conversions))
}
pub(super) fn lower_bundle_context(
    bundle: &DefinitionBundle,
    entry_source: &SourceId,
    namespace: &str,
    conversions: Option<&crate::definitions::conversion::ConversionRegistry<'_>>,
) -> Result<Schema, ImportError> {
    compile_context(
        bundle,
        entry_source,
        namespace,
        conversions,
        &ParserObservation::new(&crate::definitions::conversion::InvocationContext::default()),
    )
}
/// Observe resource parsing, indexing and package selection. Subsequent generic
/// preparation and schema lowering still have synchronous allocation boundaries.
pub fn compile_structure_bundle_with_conversions_observed(
    bundle: &DefinitionBundle,
    entry_source: &SourceId,
    namespace: &str,
    conversions: &crate::definitions::conversion::ConversionRegistry<'_>,
    context: &crate::definitions::conversion::InvocationContext<'_>,
) -> Result<Schema, ObservedImportError> {
    require_canonical_sources(bundle).map_err(ObservedImportError::Import)?;
    context
        .checkpoint()
        .map_err(ObservedImportError::Observation)?;
    let observation = ParserObservation::new(context);
    observation.finish(compile_context(
        bundle,
        entry_source,
        namespace,
        Some(conversions),
        &observation,
    ))
}
fn compile_context(
    bundle: &DefinitionBundle,
    entry_source: &SourceId,
    namespace: &str,
    conversions: Option<&crate::definitions::conversion::ConversionRegistry<'_>>,
    observation: &ParserObservation<'_, '_>,
) -> Result<Schema, ImportError> {
    compile_context_with_limits(
        bundle,
        entry_source,
        namespace,
        conversions,
        observation,
        crate::definitions::ecore::ParseLimits::default(),
    )
}
pub(super) fn compile_context_with_limits(
    bundle: &DefinitionBundle,
    entry_source: &SourceId,
    namespace: &str,
    conversions: Option<&crate::definitions::conversion::ConversionRegistry<'_>>,
    observation: &ParserObservation<'_, '_>,
    limits: crate::definitions::ecore::ParseLimits,
) -> Result<Schema, ImportError> {
    compile_context_with_package_imports(
        bundle,
        entry_source,
        namespace,
        conversions,
        observation,
        limits,
        None,
    )
}

pub(super) use crate::definitions::declarations::DeclarationSelection;

/// Inspect the exact structural dependency closure without grammar compilation.
/// This returns declaration data only, not a schema or executable language grant.
pub fn inspect_declarations_with_limits(
    bundle: &DefinitionBundle,
    entry_source: &SourceId,
    namespace: &str,
    classifiers: bool,
    context: &crate::definitions::conversion::InvocationContext<'_>,
    limits: crate::definitions::ecore::ParseLimits,
) -> Result<DeclarationInventory, ObservedImportError> {
    require_canonical_sources(bundle).map_err(ObservedImportError::Import)?;
    context
        .checkpoint()
        .map_err(ObservedImportError::Observation)?;
    let observation = ParserObservation::new(context);
    let mut inventory = DeclarationInventory::default();
    let result =
        selected_context_with_limits(bundle, entry_source, namespace, &observation, limits)
            .and_then(|(resources, _, reachable)| {
                retain_declarations(
                    &resources,
                    &reachable,
                    DeclarationSelection {
                        inventory: &mut inventory,
                        classifiers,
                    },
                    &observation,
                )
            });
    observation.finish(result)?;
    Ok(inventory)
}

/// Export exact selected package identities, including packages with no classifiers.
/// Collection policy belongs to the caller; no grammar policy enters this layer.
pub(super) fn retain_declarations(
    resources: &ResourceSet,
    reachable: &BTreeSet<SourceId>,
    selection: DeclarationSelection<'_>,
    observation: &ParserObservation<'_, '_>,
) -> Result<(), ImportError> {
    let DeclarationSelection {
        inventory: imports,
        classifiers,
    } = selection;
    for source in reachable {
        observation.work(1, source)?;
        let span = observation.span(source, 0, 0)?;
        let packages = resources
            .packages(source)
            .map_err(|message| ImportError::new(ErrorKind::InvalidDefinition, &span, message))?;
        for (namespace, declaration) in packages {
            observation.work(1, source)?;
            if resources
                .namespace_binding(namespace)
                .is_some_and(|selected| selected != declaration)
            {
                continue;
            }
            observation.serialized(&(namespace, declaration), source)?;
            observation.reserve(128, source)?;
            if imports
                .packages
                .insert(namespace.clone(), declaration.clone())
                .is_some()
            {
                return Err(ImportError::new(
                    ErrorKind::InvalidDefinition,
                    &span,
                    format!("Duplicate selected package namespace {namespace}"),
                ));
            }
            if classifiers {
                let package = resources.node_context(declaration, observation)?;
                let mut names = BTreeMap::new();
                for (index, child) in package.children.iter().enumerate() {
                    observation.work(1, source)?;
                    if child.name.local != "eClassifiers" {
                        continue;
                    }
                    observation.work(child.attributes.len(), source)?;
                    let Some(name) = child.attributes.iter().find(|attribute| {
                        attribute.name.namespace.is_none() && attribute.name.local == "name"
                    }) else {
                        continue;
                    };
                    observation.serialized(&name.value, source)?;
                    observation.serialized(declaration, source)?;
                    observation.reserve(128, source)?;
                    let mut id = declaration.clone();
                    id.path.push(index);
                    if names.insert(name.value.clone(), id).is_some() {
                        return Err(ImportError::new(
                            ErrorKind::InvalidDefinition,
                            &span,
                            format!("Duplicate imported classifier {namespace}::{}", name.value),
                        ));
                    }
                }
                observation.serialized(namespace, source)?;
                observation.reserve(128, source)?;
                imports.classifiers.insert(namespace.clone(), names);
            }
        }
    }
    Ok(())
}

pub(super) fn compile_context_with_package_imports(
    bundle: &DefinitionBundle,
    entry_source: &SourceId,
    namespace: &str,
    conversions: Option<&crate::definitions::conversion::ConversionRegistry<'_>>,
    observation: &ParserObservation<'_, '_>,
    limits: crate::definitions::ecore::ParseLimits,
    package_imports: Option<DeclarationSelection<'_>>,
) -> Result<Schema, ImportError> {
    compile_context_with_completed_inputs(
        bundle,
        entry_source,
        namespace,
        conversions,
        observation,
        limits,
        package_imports,
        false,
    )
    .map(|(schema, _)| schema)
}

#[allow(clippy::too_many_arguments)] // Existing compiler context plus private completed-input capture policy.
pub(super) fn compile_context_with_completed_inputs(
    bundle: &DefinitionBundle,
    entry_source: &SourceId,
    namespace: &str,
    conversions: Option<&crate::definitions::conversion::ConversionRegistry<'_>>,
    observation: &ParserObservation<'_, '_>,
    limits: crate::definitions::ecore::ParseLimits,
    package_imports: Option<DeclarationSelection<'_>>,
    capture_inputs: bool,
) -> Result<(Schema, Option<super::CompletedLoweredInputs>), ImportError> {
    let entry_span = observation.span(entry_source, 0, 0)?;
    let error = |message| ImportError::new(ErrorKind::InvalidDefinition, &entry_span, message);
    let (resources, entry, reachable) =
        selected_context_with_limits(bundle, entry_source, namespace, observation, limits)?;
    if let Some(imports) = package_imports {
        retain_declarations(&resources, &reachable, imports, observation)?;
    }
    let entry = &entry;
    let generic_signatures = super::generic_schema::prepare_with_limits(
        bundle,
        &resources,
        &reachable,
        observation,
        limits,
    )?;
    observation.serialized(&entry.path, entry_source)?;
    let mut root = Xml::header_from_node_at_observed(
        resources.node(entry).map_err(error)?,
        &resources.document(entry_source).map_err(error)?.source.text,
        entry.path.clone(),
        observation,
    )?;
    root.name = "ecore:EPackage".into();
    let mut declarations = BTreeMap::new();
    let mut classes: BTreeMap<DeclarationId, (String, Xml)> = BTreeMap::new();
    let mut datatype_keys = BTreeMap::new();
    let mut enumerations = BTreeMap::new();
    let mut datatypes = BTreeMap::new();
    for source in &reachable {
        let document = resources.document(source).map_err(error)?;
        if let Some(payload) = document
            .misc
            .iter()
            .find(|p| p.kind == PayloadKind::ProcessingInstruction)
        {
            return Err(ImportError::new(
                ErrorKind::Unsupported,
                &payload.span,
                "Unsupported document processing instruction",
            ));
        }
        if document.root.name.local == "XMI" {
            let mut wrapper = Xml::header_from_node_at_observed(
                &document.root,
                &document.source.text,
                Vec::new(),
                observation,
            )?;
            wrapper.name = "ecore:EPackage".into();
            attrs(&wrapper, &[])?;
        }
        for (namespace, package_id) in resources.packages(source).map_err(error)? {
            let package = resources.node(package_id).map_err(error)?;
            observation.serialized(&package_id.path, source)?;
            let mut package_xml = Xml::header_from_node_at_observed(
                package,
                &document.source.text,
                package_id.path.clone(),
                observation,
            )?;
            package_xml.name = "ecore:EPackage".into();
            package_xml.attrs.remove("xmi:id");
            if package_xml.attrs.get("xsi:type").map(String::as_str) == Some("ecore:EPackage") {
                package_xml.attrs.remove("xsi:type");
            }
            attrs(&package_xml, &["name", "nsURI", "nsPrefix"])?;
            for (index, child) in package.children.iter().enumerate() {
                observation.work(1, source)?;
                if child.name.namespace.is_none() && child.name.local == "eSubpackages" {
                    continue;
                }
                observation.serialized(&package_id.path, source)?;
                observation.reserve(32, source)?;
                let mut path = package_id.path.clone();
                path.push(index);
                let mut node = Xml::from_node_at_observed_with(
                    child,
                    &document.source.text,
                    path,
                    observation,
                    &|node| check_annotation(node, &resources, observation),
                )?;
                if node.name == "eAnnotations" {
                    continue;
                }
                if node.name != "eClassifiers"
                    || !matches!(
                        node.attrs.get("xsi:type").map(String::as_str),
                        Some("ecore:EClass" | "ecore:EEnum" | "ecore:EDataType")
                    )
                {
                    return Err(node.unsupported(
                        "Only EClass, EEnum and EDataType classifiers are supported",
                    ));
                }
                if let Some(signatures) = &generic_signatures {
                    super::generic_schema::project(&mut node, signatures, &resources, observation)?;
                }
                if let Some(name) = node.attrs.get("name").filter(|name| !name.is_empty()) {
                    observation.work(name.len().div_ceil(256).max(1), source)?;
                    observation.string(name.len(), source)?;
                }
                let name = required(&node, "name")?;
                observation.serialized(package_id, source)?;
                observation.work(1, source)?;
                observation.reserve(32, source)?;
                let mut id = package_id.clone();
                id.path.push(index);
                // Admit each owned component before constructing provenance.
                observation.serialized(source, source)?;
                observation.serialized(&id.path, source)?;
                observation.work(namespace.len().div_ceil(256).max(1), source)?;
                observation.string(namespace.len(), source)?;
                observation.work(name.len().div_ceil(256).max(1), source)?;
                observation.string(name.len(), source)?;
                observation.reserve(128, source)?;
                let declaration = ClassifierDeclaration {
                    source: source.clone(),
                    path: id.path.clone(),
                    namespace: namespace.clone(),
                    name: name.clone(),
                };
                let key = if package_id == entry && !name.contains('/') {
                    name
                } else {
                    let digest = json_digest(&declaration, source, observation)
                        .map_err(|error| node.error(error.message))?;
                    observation.work(1, source)?;
                    observation.string(71, source)?;
                    format!("__decl_{digest}")
                };
                observation.work(key.len().div_ceil(256).max(1), source)?;
                observation.string(key.len(), source)?;
                observation.reserve(128, source)?;
                if declarations.insert(key.clone(), declaration).is_some() {
                    return Err(node.error("Classifier key collision"));
                }
                if node.attrs.get("xsi:type").map(String::as_str) == Some("ecore:EEnum") {
                    let mut node = node;
                    node.attrs.remove("xmi:id");
                    let enumeration = lower_enumeration_observed(&node, Some(observation))?;
                    observation.work(key.len().div_ceil(256).max(1), source)?;
                    observation.string(key.len(), source)?;
                    observation.reserve(128, source)?;
                    enumerations.insert(key.clone(), enumeration);
                    observation.reserve(128, source)?;
                    datatype_keys.insert(id, key);
                } else if node.attrs.get("xsi:type").map(String::as_str) == Some("ecore:EDataType")
                {
                    let mut node = node;
                    node.attrs.remove("xmi:id");
                    let datatype = lower_datatype_observed(&node, Some(observation))?;
                    super::require_datatype_binding(&datatype, conversions)
                        .map_err(|message| node.unsupported(message))?;
                    observation.work(key.len().div_ceil(256).max(1), source)?;
                    observation.string(key.len(), source)?;
                    observation.reserve(128, source)?;
                    datatypes.insert(key.clone(), datatype);
                    observation.reserve(128, source)?;
                    datatype_keys.insert(id, key);
                } else {
                    observation.reserve(128, source)?;
                    classes.insert(id, (key, node));
                }
            }
        }
    }
    // Only exact declaration-to-key lookup is needed during lowering. Consume
    // the original XML trees rather than cloning a second full classifier graph.
    let mut class_keys = BTreeMap::new();
    for (id, (key, _)) in &classes {
        observation.work(1, &id.source)?;
        observation.serialized(id, &id.source)?;
        observation.work(key.len().div_ceil(256).max(1), &id.source)?;
        observation.string(key.len(), &id.source)?;
        observation.reserve(128, &id.source)?;
        class_keys.insert(id.clone(), key.clone());
    }
    for (id, (key, mut node)) in classes {
        observation.work(1, &id.source)?;
        observation.work(key.len().div_ceil(256).max(1), &id.source)?;
        observation.string(4, &id.source)?;
        observation.string(key.len(), &id.source)?;
        observation.reserve(128, &id.source)?;
        node.attrs.insert("name".into(), key.clone());
        // XMI IDs are consumed by the resource index and retained in exact source.
        node.attrs.remove("xmi:id");
        let mut parents = Vec::new();
        if let Some(value) = node.attrs.get("eSuperTypes") {
            let mut words = value.split_whitespace();
            while let Some(word) = words.next() {
                let href = if word == "ecore:EClass" {
                    words.next().ok_or_else(|| {
                        node.at(
                            "eSuperTypes",
                            ErrorKind::InvalidDefinition,
                            "Missing typed supertype reference",
                        )
                    })?
                } else {
                    word
                };
                let target = resolve_class(
                    &resources,
                    &class_keys,
                    &id,
                    href,
                    &node,
                    "eSuperTypes",
                    observation,
                )?;
                let bytes = observation.size(target.len().checked_add(3), &id.source)?;
                observation.work(bytes.div_ceil(256).max(1), &id.source)?;
                observation.string(bytes, &id.source)?;
                observation.reserve(32, &id.source)?;
                parents.push(format!("#//{target}"));
            }
        }
        let mut retained = Vec::new();
        for child in std::mem::take(&mut node.children) {
            observation.work(1, &id.source)?;
            if child.name != "eSuperTypes" {
                observation.reserve(32, &id.source)?;
                retained.push(child);
                continue;
            }
            // Resolve each child independently so an error identifies its own href.
            let mut reference = node.clone();
            reference.attrs.remove("eSuperTypes");
            reference.children = vec![child];
            reference_children(&mut reference, &["eSuperTypes"], observation)?;
            let value = required(&reference, "eSuperTypes")?;
            let href = typed_href(&value, &reference, "eSuperTypes")?;
            let target = resolve_class(
                &resources,
                &class_keys,
                &id,
                href,
                &reference,
                "eSuperTypes",
                observation,
            )?;
            let bytes = observation.size(target.len().checked_add(3), &id.source)?;
            observation.work(bytes.div_ceil(256).max(1), &id.source)?;
            observation.string(bytes, &id.source)?;
            observation.reserve(32, &id.source)?;
            parents.push(format!("#//{target}"));
        }
        node.children = retained;
        let mut parent_bytes = parents.len().saturating_sub(1);
        for parent in &parents {
            observation.work(1, &id.source)?;
            parent_bytes = observation.size(parent_bytes.checked_add(parent.len()), &id.source)?;
        }
        observation.work(parent_bytes.div_ceil(256).max(1), &id.source)?;
        observation.string(11, &id.source)?;
        observation.string(parent_bytes, &id.source)?;
        observation.reserve(128, &id.source)?;
        node.attrs.insert("eSuperTypes".into(), parents.join(" "));
        for field in &mut node.children {
            field.attrs.remove("xmi:id");
            if field.name == "eOperations" {
                operation_types(
                    field,
                    &resources,
                    &class_keys,
                    &datatype_keys,
                    &id,
                    observation,
                )?;
                continue;
            }
            reference_children(field, &["eType", "eOpposite"], observation)?;
            if let Some(ty) = field.attrs.get("eType").cloned() {
                // Preserve the scalar datatype contract; E05/E06 expand it.
                if let Some(name) = ty.strip_prefix("http://www.eclipse.org/emf/2002/Ecore#//") {
                    if crate::definitions::scalars::Builtin::named(name).is_none()
                        && !matches!(
                            name,
                            "EJavaClass"
                                | "EJavaObject"
                                | "EEnumerator"
                                | "EInvocationTargetException"
                                | "EDiagnosticChain"
                                | "EResource"
                                | "EResourceSet"
                                | "ETreeIterator"
                                | "EEList"
                                | "EMap"
                                | "EFeatureMap"
                                | "EFeatureMapEntry"
                        )
                    {
                        return Err(field.at(
                            "eType",
                            ErrorKind::Unsupported,
                            "Unsupported standard datatype",
                        ));
                    }
                    field
                        .attrs
                        .insert("eType".into(), format!("ecore:EDataType {ty}"));
                } else if !ty
                    .starts_with("ecore:EDataType http://www.eclipse.org/emf/2002/Ecore#//")
                {
                    let href = typed_href(&ty, field, "eType")?;
                    let target = if field.attrs.get("xsi:type").map(String::as_str)
                        == Some("ecore:EAttribute")
                    {
                        let resolved = resources
                            .resolve_context(&id.source, href, observation)
                            .map_err(|error| {
                                field.at("eType", ErrorKind::InvalidDefinition, error.message)
                            })?;
                        datatype_keys.get(&resolved).cloned().ok_or_else(|| {
                            field.at(
                                "eType",
                                ErrorKind::InvalidDefinition,
                                "Attribute must identify a selected datatype",
                            )
                        })?
                    } else {
                        resolve_class(
                            &resources,
                            &class_keys,
                            &id,
                            href,
                            field,
                            "eType",
                            observation,
                        )?
                    };
                    field.attrs.insert("eType".into(), format!("#//{target}"));
                }
            }
            if let Some(opposite) = field.attrs.get("eOpposite").cloned() {
                let (target, opposite_xml) =
                    resolve_opposite(&resources, &id.source, field, &opposite, observation)?;
                let mut owner = target.clone();
                owner.path.pop().ok_or_else(|| {
                    field.at(
                        "eOpposite",
                        ErrorKind::InvalidDefinition,
                        "Opposite has no declaring class",
                    )
                })?;
                let owner_key = class_keys.get(&owner).ok_or_else(|| {
                    field.at(
                        "eOpposite",
                        ErrorKind::InvalidDefinition,
                        "Opposite owner is not an EClass",
                    )
                })?;
                let name = required(&opposite_xml, "name")?;
                field
                    .attrs
                    .insert("eOpposite".into(), format!("#//{owner_key}/{name}"));
            }
        }
        observation.reserve(32, &id.source)?;
        root.children.push(node);
    }
    // A schema revision includes exact sources AND dependency/namespace choices,
    // ordered independently of the bundle's input order.
    let mut sources = Vec::new();
    for id in &reachable {
        observation.work(1, id)?;
        let mut dependencies = Vec::new();
        for dependency in &resources.document(id).map_err(error)?.source.dependencies {
            observation.work(1, id)?;
            observation.reserve(32, id)?;
            dependencies.push(dependency);
        }
        // Sorting remains atomic, with guarded temporary storage. Borrowed
        // identities serialize exactly like the former owned dependency copies.
        observation.work(dependencies.len(), id)?;
        observation.reserve(
            observation.size(dependencies.len().checked_mul(32), id)?,
            id,
        )?;
        dependencies.sort();
        observation.reserve(32, id)?;
        sources.push((id, dependencies));
    }
    let mut bindings = Vec::new();
    for binding in &bundle.namespaces {
        observation.work(1, entry_source)?;
        if reachable.contains(&binding.source) {
            observation.reserve(32, entry_source)?;
            bindings.push(binding);
        }
    }
    observation.work(bindings.len(), entry_source)?;
    observation.reserve(
        observation.size(bindings.len().checked_mul(32), entry_source)?,
        entry_source,
    )?;
    bindings.sort_by(|a, b| a.namespace.cmp(&b.namespace));
    let revision = json_digest(
        &(entry_source, namespace, sources, bindings),
        entry_source,
        observation,
    )?;
    let policy = SchemaLoweringPolicy {
        validate_projected_operations: true,
        move_feature_declarations: false,
        constructor_contracts: vec![],
        reference_top: None,
        attribute_domain_defaults: BTreeMap::new(),
    };
    if capture_inputs {
        let mut schema = super::lower_schema_output_with_datatypes(
            &mut root,
            revision,
            enumerations,
            datatypes,
            conversions,
            policy,
            observation,
        )?;
        // Only this private draft opt-in installs the FINAL resource identities
        // before the SAME single canonical validation. Premutation maps are
        // never promoted. Public import ordering stays in the branch below.
        schema.declarations = declarations;
        schema.generic_signatures = generic_signatures.map(|prepared| prepared.inventory);
        let mut validation =
            super::LoweredSchemaValidation::new_with_completed_inputs(&root, true, observation)?;
        while !validation.advance(&root, &schema, observation)? {}
        let inputs = validation.finish_completed_inputs(&root, observation)?;
        Ok((schema, Some(inputs)))
    } else {
        let mut schema = lower_schema_with_datatypes(
            &mut root,
            revision,
            enumerations,
            datatypes,
            conversions,
            policy,
            observation,
        )?;
        schema.declarations = declarations;
        schema.generic_signatures = generic_signatures.map(|prepared| prepared.inventory);
        Ok((schema, None))
    }
}

// Stream the existing serde_json byte sequence into the hash instead of retaining
// a second encoded declaration/revision buffer. Each emitted chunk is observed
// before hashing; serialization and formatting are still synchronous intervals.
#[path = "revision_preparation.rs"]
mod revision_preparation;
pub(super) use revision_preparation::MapRevisionPreparation;
#[path = "metadata_revision.rs"]
mod metadata_revision;
pub(super) use metadata_revision::MetadataRevisionPreparation;

pub(super) fn json_digest<T: ?Sized + serde::Serialize>(
    value: &T,
    source: &SourceId,
    observation: &ParserObservation<'_, '_>,
) -> Result<String, ImportError> {
    json_digest_with_buffering(value, source, observation, false)
}
/// Same byte stream and SHA-256, with bounded updates over actual 256-byte blocks.
pub(super) fn json_digest_buffered<T: ?Sized + serde::Serialize>(
    value: &T,
    source: &SourceId,
    observation: &ParserObservation<'_, '_>,
) -> Result<String, ImportError> {
    json_digest_with_buffering(value, source, observation, true)
}
struct ObservedDigestWriter<'a, 'b, 'c> {
    digest: Sha256,
    source: &'a SourceId,
    observation: &'a ParserObservation<'b, 'c>,
}
impl std::io::Write for ObservedDigestWriter<'_, '_, '_> {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.observation
            .work(bytes.len().div_ceil(256).max(1), self.source)
            .map_err(|error| std::io::Error::other(error.message))?;
        self.digest.update(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
use crate::definitions::json_digest::BufferedWrite as Blocks;
fn json_digest_with_buffering<T: ?Sized + serde::Serialize>(
    value: &T,
    source: &SourceId,
    observation: &ParserObservation<'_, '_>,
    buffered: bool,
) -> Result<String, ImportError> {
    let mut writer = ObservedDigestWriter {
        digest: Sha256::new(),
        source,
        observation,
    };
    let serialized = if buffered {
        // Charge the bounded live buffer before copying any JSON bytes.
        observation.reserve(256, source)?;
        let mut blocks = Blocks {
            writer: &mut writer,
            bytes: [0; 256],
            used: 0,
        };
        serde_json::to_writer(&mut blocks, value)
            .and_then(|()| std::io::Write::flush(&mut blocks).map_err(serde_json::Error::io))
    } else {
        serde_json::to_writer(&mut writer, value)
    };
    serialized.map_err(|error| {
        ImportError::new(
            ErrorKind::InvalidDefinition,
            &crate::definitions::bundle::SourceSpan {
                source: source.clone(),
                start: 0,
                end: 0,
            },
            error.to_string(),
        )
    })?;
    observation.work(1, source)?;
    observation.string(64, source)?;
    Ok(format!("{:x}", writer.digest.finalize()))
}

fn operation_types(
    node: &mut Xml,
    resources: &ResourceSet,
    classes: &BTreeMap<DeclarationId, String>,
    datatype_keys: &BTreeMap<DeclarationId, String>,
    owner: &DeclarationId,
    observation: &ParserObservation<'_, '_>,
) -> Result<(), ImportError> {
    if let Some(kind) = node.attrs.get("xsi:type") {
        let expected = match node.name.as_str() {
            "eOperations" => "ecore:EOperation",
            "eParameters" => "ecore:EParameter",
            _ => return Err(node.error("Invalid operation member kind")),
        };
        if kind != expected {
            return Err(node.at(
                "xsi:type",
                ErrorKind::InvalidDefinition,
                "Incorrect operation member type",
            ));
        }
        node.attrs.remove("xsi:type");
    }
    reference_children(node, &["eType"], observation)?;
    if let Some(ty) = node.attrs.get("eType").cloned() {
        if ty.split_whitespace().last() == Some("http://www.eclipse.org/emf/2002/Ecore#//EObject") {
            return Err(node.at(
                "eType",
                ErrorKind::Unsupported,
                "EObject operation types require explicit runtime type materialization",
            ));
        }
        if ty.starts_with("http://www.eclipse.org/emf/2002/Ecore#//") {
            node.attrs
                .insert("eType".into(), format!("ecore:EDataType {ty}"));
        } else if !ty.starts_with("ecore:EDataType http://www.eclipse.org/emf/2002/Ecore#//") {
            let href = typed_href(&ty, node, "eType")?;
            let resolved = resources
                .resolve_context(&owner.source, href, observation)
                .map_err(|error| node.at("eType", ErrorKind::InvalidDefinition, error.message))?;
            let target = if let Some(key) = datatype_keys.get(&resolved) {
                key.clone()
            } else {
                resolve_class(resources, classes, owner, href, node, "eType", observation)?
            };
            node.attrs.insert("eType".into(), format!("#//{target}"));
        }
    }
    for parameter in &mut node.children {
        if parameter.name == "eParameters" {
            parameter.attrs.remove("xmi:id");
            operation_types(
                parameter,
                resources,
                classes,
                datatype_keys,
                owner,
                observation,
            )?;
        }
    }
    Ok(())
}

pub(super) fn resolve_opposite(
    resources: &ResourceSet,
    source: &SourceId,
    field: &Xml,
    href: &str,
    observation: &ParserObservation<'_, '_>,
) -> Result<(DeclarationId, Xml), ImportError> {
    let fail = |e| field.at("eOpposite", ErrorKind::InvalidDefinition, e);
    let target = resources
        .resolve_context(source, href, observation)
        .map_err(|error| fail(error.message))?;
    let node = resources.node(&target).map_err(fail)?;
    let document = resources.document(&target.source).map_err(fail)?;
    observation.serialized(&target.path, &target.source)?;
    let xml = Xml::from_node_at_observed(
        node,
        &document.source.text,
        target.path.clone(),
        observation,
    )?;
    if xml.name != "eStructuralFeatures"
        || xml.attrs.get("xsi:type").map(String::as_str) != Some("ecore:EReference")
    {
        return Err(fail("Opposite must identify an EReference".into()));
    }
    Ok((target, xml))
}

fn check_annotation(
    node: &Xml,
    resources: &ResourceSet,
    observation: &ParserObservation<'_, '_>,
) -> Result<(), ImportError> {
    if node.name == "eAnnotations" {
        observation.work(1, &node.declaration.source)?;
        super::annotation_metadata(node)?;
        if let Some(references) = node.attrs.get("references") {
            for href in references.split_whitespace() {
                observation.work(1, &node.declaration.source)?;
                resources.resolve_context(&node.declaration.source, href, observation)?;
            }
        }
    }
    Ok(())
}
pub(super) fn reference_children(
    node: &mut Xml,
    names: &[&str],
    observation: &ParserObservation<'_, '_>,
) -> Result<(), ImportError> {
    let mut retained = Vec::new();
    for child in std::mem::take(&mut node.children) {
        observation.work(1, &child.declaration.source)?;
        if !names.contains(&child.name.as_str()) {
            observation.reserve(32, &child.declaration.source)?;
            retained.push(child);
            continue;
        }
        for _ in &child.attrs {
            observation.work(1, &child.declaration.source)?;
        }
        attrs(&child, &["href", "xsi:type"])?;
        if !child.children.is_empty() {
            return Err(child.unsupported("Nested reference payload is unsupported"));
        }
        if node.attrs.contains_key(&child.name) {
            return Err(child.error("Duplicate reference encoding"));
        }
        if let Some(href) = child.attrs.get("href").filter(|href| !href.is_empty()) {
            observation.work(href.len().div_ceil(256).max(1), &child.declaration.source)?;
            observation.string(href.len(), &child.declaration.source)?;
        }
        let href = required(&child, "href")?;
        let value = if let Some(kind) = child.attrs.get("xsi:type") {
            let allowed = match child.name.as_str() {
                "eType" => {
                    ["ecore:EClass", "ecore:EDataType", "ecore:EEnum"].contains(&kind.as_str())
                }
                "eSuperTypes" => kind == "ecore:EClass",
                "eOpposite" => kind == "ecore:EReference",
                _ => false,
            };
            if !allowed {
                return Err(child.at(
                    "xsi:type",
                    ErrorKind::InvalidDefinition,
                    "Incorrect reference target kind",
                ));
            }
            if child.name == "eType" {
                let bytes = observation.size(
                    kind.len()
                        .checked_add(href.len())
                        .and_then(|n| n.checked_add(1)),
                    &child.declaration.source,
                )?;
                observation.work(bytes.div_ceil(256).max(1), &child.declaration.source)?;
                observation.string(bytes, &child.declaration.source)?;
                format!("{kind} {href}")
            } else {
                href
            }
        } else {
            href
        };
        let span = child.attr_spans.get("href").unwrap_or(&child.opening);
        observation.string(child.name.len(), &child.declaration.source)?;
        observation.serialized(span, &child.declaration.source)?;
        observation.reserve(128, &child.declaration.source)?;
        node.attr_spans.insert(child.name.clone(), span.clone());
        observation.reserve(128, &child.declaration.source)?;
        node.attrs.insert(child.name, value);
    }
    node.children = retained;
    Ok(())
}

fn typed_href<'a>(value: &'a str, node: &Xml, key: &str) -> Result<&'a str, ImportError> {
    let mut words = value.split_whitespace();
    match (words.next(), words.next(), words.next()) {
        (Some(href), None, None) => Ok(href),
        (Some("ecore:EClass"), Some(href), None) => Ok(href),
        (Some("ecore:EDataType" | "ecore:EEnum"), Some(href), None) if key == "eType" => Ok(href),
        _ => Err(node.at(
            key,
            ErrorKind::Unsupported,
            "Unsupported typed resource reference",
        )),
    }
}
fn resolve_class(
    resources: &ResourceSet,
    classes: &BTreeMap<DeclarationId, String>,
    from: &DeclarationId,
    href: &str,
    node: &Xml,
    key: &str,
    observation: &ParserObservation<'_, '_>,
) -> Result<String, ImportError> {
    let id = resources
        .resolve_context(&from.source, href, observation)
        .map_err(|e| {
            if href == "http://www.eclipse.org/emf/2002/Ecore#//EObject" {
                node.at(
                    key,
                    ErrorKind::Unsupported,
                    "EObject references require explicit runtime type materialization",
                )
            } else {
                node.at(key, ErrorKind::InvalidDefinition, e.message)
            }
        })?;
    let name = classes.get(&id).ok_or_else(|| {
        node.at(
            key,
            ErrorKind::InvalidDefinition,
            "Reference does not identify a selected EClass",
        )
    })?;
    observation.work(name.len().div_ceil(256).max(1), &from.source)?;
    observation.string(name.len(), &from.source)?;
    Ok(name.clone())
}

pub(super) fn selected_context_observed(
    bundle: &DefinitionBundle,
    entry_source: &SourceId,
    namespace: &str,
    observation: &ParserObservation<'_, '_>,
) -> Result<(ResourceSet, DeclarationId, BTreeSet<SourceId>), ImportError> {
    selected_context_with_limits(
        bundle,
        entry_source,
        namespace,
        observation,
        crate::definitions::ecore::ParseLimits::default(),
    )
}
fn selected_context_with_limits(
    bundle: &DefinitionBundle,
    entry_source: &SourceId,
    namespace: &str,
    observation: &ParserObservation<'_, '_>,
    limits: crate::definitions::ecore::ParseLimits,
) -> Result<(ResourceSet, DeclarationId, BTreeSet<SourceId>), ImportError> {
    selected_context_with_cursor_policy(bundle, entry_source, namespace, observation, limits, false)
}
pub(super) fn selected_context_with_cursor_policy(
    bundle: &DefinitionBundle,
    entry_source: &SourceId,
    namespace: &str,
    observation: &ParserObservation<'_, '_>,
    limits: crate::definitions::ecore::ParseLimits,
    borrowed_cursors: bool,
) -> Result<(ResourceSet, DeclarationId, BTreeSet<SourceId>), ImportError> {
    let entry_span = observation.span(entry_source, 0, 0)?;
    let resources =
        ResourceSet::new_context_with_cursor_policy(bundle, observation, limits, borrowed_cursors)?;
    selected_context_from_resources(
        bundle,
        entry_source,
        namespace,
        resources,
        entry_span,
        observation,
    )
}
pub(super) fn selected_context_from_resources(
    bundle: &DefinitionBundle,
    entry_source: &SourceId,
    namespace: &str,
    resources: ResourceSet,
    entry_span: crate::definitions::bundle::SourceSpan,
    observation: &ParserObservation<'_, '_>,
) -> Result<(ResourceSet, DeclarationId, BTreeSet<SourceId>), ImportError> {
    let error = |message| ImportError::new(ErrorKind::InvalidDefinition, &entry_span, message);
    for binding in &bundle.namespaces {
        observation.work(1, &binding.source)?;
        if binding.namespace == namespace && &binding.source != entry_source {
            return Err(error(
                "Entry package conflicts with explicit namespace binding".into(),
            ));
        }
    }
    let entry = resources
        .packages(entry_source)
        .map_err(error)?
        .get(namespace)
        .ok_or_else(|| {
            error(format!(
                "Entry namespace {namespace} is not declared in {}",
                entry_source.uri
            ))
        })?;
    observation.serialized(entry, entry_source)?;
    let entry = entry.clone();
    let mut reachable = BTreeSet::new();
    observation.serialized(entry_source, entry_source)?;
    observation.reserve(32, entry_source)?;
    let mut pending = vec![entry_source.clone()];
    while let Some(id) = pending.pop() {
        observation.work(1, &id)?;
        observation.serialized(&id, &id)?;
        observation.reserve(128, &id)?;
        if !reachable.insert(id.clone()) {
            continue;
        }
        let mut selected = None;
        for source in &bundle.sources {
            observation.work(1, &id)?;
            if source.id == id {
                selected = Some(source);
                break;
            }
        }
        let source = selected.ok_or_else(|| error("Missing selected source".into()))?;
        if !matches!(source.kind, SourceKind::Ecore | SourceKind::Rcore) {
            return Err(error("Ecore resource dependency must select Ecore".into()));
        }
        for dependency in &source.dependencies {
            observation.work(1, dependency)?;
            observation.serialized(dependency, dependency)?;
            observation.reserve(32, dependency)?;
            if bundle.sources.iter().any(|s| {
                s.id == *dependency && matches!(s.kind, SourceKind::Ecore | SourceKind::Rcore)
            }) {
                pending.push(dependency.clone());
            } else if source.kind != SourceKind::Rcore {
                return Err(error("Ecore resource dependency must select Ecore".into()));
            }
        }
    }
    Ok((resources, entry, reachable))
}

/// Keep the exact dependency closure when a Rcore source carries structure and
/// behavior together. Semantic dependencies must not disappear in materialization.
pub(super) fn complete_dependencies(
    bundle: &DefinitionBundle,
    roots: &BTreeSet<SourceId>,
    observation: &ParserObservation<'_, '_>,
) -> Result<BTreeSet<SourceId>, ImportError> {
    let mut selected = BTreeSet::new();
    let mut pending = Vec::new();
    for root in roots {
        observation.serialized(root, root)?;
        observation.reserve(32, root)?;
        pending.push(root.clone());
    }
    while let Some(id) = pending.pop() {
        observation.work(1, &id)?;
        if selected.contains(&id) {
            continue;
        }
        let mut source = None;
        for candidate in &bundle.sources {
            observation.work(1, &id)?;
            if candidate.id == id {
                source = Some(candidate);
                break;
            }
        }
        let source = source.ok_or_else(|| {
            ImportError::new(
                ErrorKind::InvalidDefinition,
                &crate::definitions::bundle::SourceSpan {
                    source: id.clone(),
                    start: 0,
                    end: 0,
                },
                "Missing exact Rcore dependency",
            )
        })?;
        for dependency in &source.dependencies {
            observation.serialized(dependency, &id)?;
            observation.reserve(32, &id)?;
            pending.push(dependency.clone());
        }
        observation.reserve(128, &id)?;
        selected.insert(id);
    }
    Ok(selected)
}

#[cfg(test)]
mod reference_folding_controls {
    use super::*;
    use crate::definitions::{
        bundle::DefinitionSource,
        conversion::{InvocationContext, InvocationObserver, ProviderId},
        ecore::Document,
    };
    use std::{cell::Cell, rc::Rc};
    struct Probe {
        storage: Cell<usize>,
        work: Cell<usize>,
        calls: Cell<usize>,
        storage_limit: usize,
        work_limit: usize,
        cancel: Option<usize>,
    }
    impl Probe {
        fn new(storage_limit: usize, work_limit: usize, cancel: Option<usize>) -> Rc<Self> {
            Rc::new(Self {
                storage: Cell::new(0),
                work: Cell::new(0),
                calls: Cell::new(0),
                storage_limit,
                work_limit,
                cancel,
            })
        }
        fn touch(&self) -> Result<(), String> {
            let call = self.calls.get();
            self.calls.set(call + 1);
            if self.cancel == Some(call) {
                Err("reference folding cancelled".into())
            } else {
                Ok(())
            }
        }
    }
    impl InvocationObserver for Probe {
        fn before_invoke(&self, _: &ProviderId) -> Result<(), String> {
            panic!("reference folding cannot invoke a provider")
        }
        fn work(&self, units: usize) -> Result<(), String> {
            self.touch()?;
            let next = self.work.get() + units;
            if next > self.work_limit {
                return Err("reference folding work denied".into());
            }
            self.work.set(next);
            Ok(())
        }
        fn retain_storage(&self, bytes: usize) -> Result<(), String> {
            self.touch()?;
            let next = self.storage.get() + bytes;
            if next > self.storage_limit {
                return Err("reference folding storage denied".into());
            }
            self.storage.set(next);
            Ok(())
        }
    }
    fn xml(body: &str) -> Xml {
        let source = DefinitionSource::new(
            "memory:x",
            SourceKind::Ecore,
            format!(
                r#"<a xmlns:x="http://www.w3.org/2001/XMLSchema-instance" xmlns:e="http://www.eclipse.org/emf/2002/Ecore">{body}</a>"#
            ),
        );
        let document = Document::parse(source).unwrap();
        Xml::from_node(&document.root, &document.source.text).unwrap()
    }
    fn run(original: &Xml, names: &[&str], probe: Rc<Probe>) -> Result<Xml, ObservedImportError> {
        let mut node = original.clone();
        let context = InvocationContext::new(probe);
        let observation = ParserObservation::new(&context);
        observation.finish(reference_children(&mut node, names, &observation))?;
        Ok(node)
    }
    #[test]
    fn buffered_json_digest_keeps_every_byte_exact_boundaries_and_fresh_cancellation() {
        let source = DefinitionSource::new("memory:buffered-hash", SourceKind::Ecore, "<a/>");
        for value in [
            serde_json::json!(""),
            serde_json::json!("é".repeat(257)),
            serde_json::json!({"escaped": "é\n\"\\\0", "nested": [null, true, -17, "x".repeat(4096)]}),
            serde_json::json!((0..513).collect::<Vec<_>>()),
        ] {
            let encoded = serde_json::to_vec(&value).unwrap();
            let expected = format!("{:x}", Sha256::digest(&encoded));
            let needed = encoded.len().div_ceil(256) + 1;
            let run = |probe: Rc<Probe>| {
                let context = InvocationContext::new(probe);
                let observation = ParserObservation::new(&context);
                observation.finish(json_digest_buffered(&value, &source.id, &observation))
            };
            let full = Probe::new(384, needed, None);
            assert_eq!(run(full.clone()).unwrap(), expected);
            assert_eq!((full.storage.get(), full.work.get()), (384, needed));
            for storage in [0, 255, 256, 383, 384, 385] {
                let result = run(Probe::new(storage, needed, None));
                if storage < 384 {
                    assert_eq!(
                        result.unwrap_err(),
                        ObservedImportError::Observation("reference folding storage denied".into())
                    );
                } else {
                    assert_eq!(result.unwrap(), expected);
                }
            }
            for work in [0, needed - 1, needed, needed + 1] {
                let result = run(Probe::new(384, work, None));
                if work < needed {
                    assert_eq!(
                        result.unwrap_err(),
                        ObservedImportError::Observation("reference folding work denied".into())
                    );
                } else {
                    assert_eq!(result.unwrap(), expected);
                }
            }
            for call in 0..full.calls.get() {
                assert_eq!(
                    run(Probe::new(384, needed, Some(call))).unwrap_err(),
                    ObservedImportError::Observation("reference folding cancelled".into())
                );
                assert_eq!(run(Probe::new(384, needed, None)).unwrap(), expected);
            }
            assert_eq!(serde_json::to_vec(&value).unwrap(), encoded);
            let legacy = Probe::new(128, usize::MAX, None);
            let context = InvocationContext::new(legacy.clone());
            let observation = ParserObservation::new(&context);
            assert_eq!(
                observation
                    .finish(json_digest(&value, &source.id, &observation))
                    .unwrap(),
                expected
            );
            assert_eq!(legacy.storage.get(), 128);
            if value.is_array() {
                assert!(legacy.work.get() > full.work.get());
            }
        }
    }
    #[test]
    fn streamed_json_digest_preserves_literal_utf8_and_observes_each_chunk() {
        let source = DefinitionSource::new("memory:hash", SourceKind::Ecore, "<a/>");
        let value = "é".repeat(257);
        // JSON is a quote, 514 UTF-8 bytes, and a quote. Work1+3+1 precedes
        // hashing, then formatting1; only the 64-byte digest string is owned.
        // Independent Python hashlib over that literal byte sequence pins SHA.
        let expected = "92618c66cdac8c9826ac636c5aff8a2e0f4e21c886ee527176b22ebd02c42d38";
        let run = |probe: Rc<Probe>| {
            let context = InvocationContext::new(probe);
            let observation = ParserObservation::new(&context);
            observation.finish(json_digest(&value, &source.id, &observation))
        };
        let full = Probe::new(128, 6, None);
        assert_eq!(run(full.clone()).unwrap(), expected);
        assert_eq!((full.storage.get(), full.work.get()), (128, 6));
        for storage in [0, 127, 128, 129] {
            let result = run(Probe::new(storage, 6, None));
            if storage < 128 {
                assert_eq!(
                    result.unwrap_err(),
                    ObservedImportError::Observation("reference folding storage denied".into())
                );
            } else {
                assert_eq!(result.unwrap(), expected);
            }
        }
        for work in [0, 5, 6, 7] {
            let result = run(Probe::new(128, work, None));
            if work < 6 {
                assert_eq!(
                    result.unwrap_err(),
                    ObservedImportError::Observation("reference folding work denied".into())
                );
            } else {
                assert_eq!(result.unwrap(), expected);
            }
        }
        for call in 0..full.calls.get() {
            assert_eq!(
                run(Probe::new(128, 6, Some(call))).unwrap_err(),
                ObservedImportError::Observation("reference folding cancelled".into())
            );
            assert_eq!(run(Probe::new(128, 6, None)).unwrap(), expected);
        }
        assert_eq!(value.len(), 514);
        assert_eq!(source.text, "<a/>");
        let escaped = "é\n\"\\\0";
        let context = InvocationContext::new(Probe::new(128, usize::MAX, None));
        let observation = ParserObservation::new(&context);
        assert_eq!(
            observation
                .finish(json_digest(escaped, &source.id, &observation))
                .unwrap(),
            "205a2fc0832682dc899131cfe8646e4b3d84609f3f062a5f8538ca9f766614be"
        );
    }
    #[test]
    fn child_folding_preflights_literal_storage_work_and_each_interruption() {
        let original = xml("<eType href='é' x:type='e:EDataType'/><other/>");
        // href 64+2, formatted value 64+18, cloned eType key 64+5,
        // span 11 nodes *128 +167 metadata bytes, two map entries *128,
        // retained child vector slot 32: 2080 total. Work: five scan/copy
        // units, 11 metadata visits, seven span-copy units, one retained child.
        let full = Probe::new(2080, 24, None);
        let result = run(&original, &["eType"], full.clone()).unwrap();
        let assert_result = |node: &Xml| {
            assert_eq!(node.attrs["eType"], "ecore:EDataType é");
            assert_eq!(
                node.attr_spans["eType"],
                original.children[0].attr_spans["href"]
            );
            assert_eq!(node.children.len(), 1);
            assert_eq!(
                node.children[0].declaration,
                original.children[1].declaration
            );
            assert_eq!(node.declaration, original.declaration);
            assert_eq!(node.source_payload, original.source_payload);
        };
        assert_result(&result);
        assert_eq!((full.storage.get(), full.work.get()), (2080, 24));
        for prefix in [66, 148, 217, 1792, 1920, 2048, 2080] {
            assert!(
                matches!(run(&original, &["eType"], Probe::new(prefix-1,usize::MAX,None)), Err(ObservedImportError::Observation(message)) if message=="reference folding storage denied")
            );
        }
        for call in 0..full.calls.get() {
            assert!(
                matches!(run(&original, &["eType"], Probe::new(usize::MAX,usize::MAX,Some(call))), Err(ObservedImportError::Observation(message)) if message=="reference folding cancelled")
            );
        }
        for limit in [23, 24, 25] {
            let result = run(&original, &["eType"], Probe::new(2080, limit, None));
            if limit == 23 {
                assert!(
                    matches!(result, Err(ObservedImportError::Observation(message)) if message=="reference folding work denied")
                );
            } else {
                assert_result(&result.unwrap());
            }
        }
        assert_result(&run(&original, &["eType"], Probe::new(2081, 25, None)).unwrap());
        assert_eq!(original.children.len(), 2);
        assert!(!original.attrs.contains_key("eType"));
    }
    #[test]
    fn child_errors_keep_exact_diagnostics_and_typed_tokens_remain_borrowed() {
        for body in [
            "<eType/>",
            "<eType href=''/>",
            "<eType href='a' extra='b'/>",
            "<eType href='a'><unknown/></eType>",
            "<eType href='a' x:type='e:EReference'/>",
            "<eType href='a'/><eType href='b'/>",
            "<eType href='a'>payload</eType>",
        ] {
            let original = xml(body);
            let mut compatible = original.clone();
            let error = reference_children(
                &mut compatible,
                &["eType"],
                &ParserObservation::new(&InvocationContext::default()),
            )
            .unwrap_err();
            let result = run(
                &original,
                &["eType"],
                Probe::new(usize::MAX, usize::MAX, None),
            );
            assert!(matches!(result, Err(ObservedImportError::Import(actual)) if actual == error));
        }
        let node = xml("");
        for (value, key, expected) in [
            (" href ", "eType", Some("href")),
            ("ecore:EClass\t#//A", "eSuperTypes", Some("#//A")),
            ("ecore:EDataType\n#//D", "eType", Some("#//D")),
            ("ecore:EEnum #//E", "eType", Some("#//E")),
            ("ecore:EDataType #//D", "eOpposite", None),
            ("a b c d", "eType", None),
            ("", "eType", None),
        ] {
            let result = typed_href(value, &node, key);
            match expected {
                Some(expected) => assert_eq!(result.unwrap(), expected),
                None => assert_eq!(result.unwrap_err().kind, ErrorKind::Unsupported),
            }
        }
    }
}
