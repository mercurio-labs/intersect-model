//! Native definition compiler. Imported source is retained; unsupported XML is rejected.
use crate::definitions::schema::*;
use crate::definitions::schema::{
    FeatureInputCache, PreparedFeatureInputs, SharedFeatureInputCache,
};
use crate::definitions::{
    bundle::{DefinitionSource, SourceKind},
    ecore::{Document, ErrorKind, ImportError},
};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;

#[path = "materialization.rs"]
mod materialization;
pub(crate) use materialization::OwnedSchemaPreparation;
pub use materialization::{
    OwnedCheckedRuntimeSourceArrayPreparationResult, OwnedRuntimeSourceArrayMaterialization,
    OwnedRuntimeSourceArrayPreparation, OwnedRuntimeSourceArrayPreparationResult,
    materialize_closed_types, materialize_closed_types_with_conversions,
    materialize_owned_runtime_source_array_inputs_observed,
    materialize_runtime_array_inputs_observed, materialize_runtime_array_view_inputs_observed,
    materialize_runtime_bundle, materialize_runtime_bundle_inputs_observed,
    materialize_runtime_bundle_observed, materialize_runtime_bundle_with_limits,
    materialize_runtime_domain_inputs_observed, materialize_runtime_source_array_inputs_observed,
    materialize_runtime_types,
};
#[path = "generic_schema.rs"]
pub(crate) mod generic_schema;
#[path = "resource_compiler.rs"]
mod resource_compiler;
#[path = "standard_datatypes.rs"]
mod standard_datatypes;
pub(crate) use resource_compiler::require_canonical_sources;
pub use resource_compiler::{
    compile_structure_bundle, compile_structure_bundle_with_conversions,
    compile_structure_bundle_with_conversions_observed, compile_structure_bundle_with_limits,
    compile_structure_bundle_with_limits_observed, inspect_declarations_with_limits,
};
pub(crate) use standard_datatypes::standard_datatype_source_observed;
pub use standard_datatypes::{
    standard_datatype, standard_datatype_by_id, standard_datatype_source,
    standard_datatype_source_id,
};

/// The one structural lowering path used by checked model source owners.
pub(crate) fn compile_structure_selection(
    selection: crate::definitions::model_definition::StructureSelection<'_>,
    conversions: Option<&crate::definitions::conversion::ConversionRegistry<'_>>,
    declarations: Option<crate::definitions::declarations::DeclarationSelection<'_>>,
    capture_inputs: bool,
    observation: &crate::definitions::ecore::ParserObservation<'_, '_>,
    limits: crate::definitions::ecore::ParseLimits,
) -> Result<(Schema, Option<CompletedLoweredInputs>), ImportError> {
    if selection.materialized_types.is_empty() {
        resource_compiler::compile_context_with_completed_inputs(
            selection.bundle,
            selection.entry,
            selection.namespace,
            conversions,
            observation,
            limits,
            declarations,
            capture_inputs,
        )
    } else {
        materialization::materialize_structure_selection_with_limits(
            selection,
            conversions,
            declarations,
            observation,
            limits,
        )
        .map(|schema| (schema, None))
    }
}

#[path = "compiler_xml_projection.rs"]
mod xml_projection;
use xml_projection::XmlProjectionPreparation;
#[path = "compiler_enum_lowering.rs"]
mod enum_lowering;
use enum_lowering::EnumLoweringPreparation;
#[path = "compiler_enum_preparation.rs"]
mod enum_preparation;
pub(crate) use enum_preparation::EnumerationPreparation;

#[derive(Clone)]
pub(crate) struct Xml {
    name: String,
    attrs: BTreeMap<String, String>,
    attr_spans: BTreeMap<String, crate::definitions::bundle::SourceSpan>,
    children: Vec<Xml>,
    opening: crate::definitions::bundle::SourceSpan,
    payload: Option<crate::definitions::bundle::SourceSpan>,
    declaration: crate::definitions::resources::DeclarationId,
    source_payload: String,
}
impl Xml {
    fn from_node(
        node: &crate::definitions::ecore::Node,
        source: &str,
    ) -> Result<Self, ImportError> {
        Self::from_node_at(node, source, Vec::new())
    }
    fn from_node_at(
        node: &crate::definitions::ecore::Node,
        source: &str,
        path: Vec<usize>,
    ) -> Result<Self, ImportError> {
        Self::from_node_at_observed(
            node,
            source,
            path,
            &crate::definitions::ecore::ParserObservation::new(
                &crate::definitions::conversion::InvocationContext::default(),
            ),
        )
    }
    fn from_node_at_observed(
        node: &crate::definitions::ecore::Node,
        source: &str,
        path: Vec<usize>,
        observation: &crate::definitions::ecore::ParserObservation<'_, '_>,
    ) -> Result<Self, ImportError> {
        Self::from_node_at_observed_with(node, source, path, observation, &|_| Ok(()))
    }
    // Consumers can validate metadata during the existing observed traversal.
    // Unannotated documents retain their original work/storage receipts.
    fn from_node_at_observed_with(
        node: &crate::definitions::ecore::Node,
        source: &str,
        path: Vec<usize>,
        observation: &crate::definitions::ecore::ParserObservation<'_, '_>,
        visit: &impl Fn(&Self) -> Result<(), ImportError>,
    ) -> Result<Self, ImportError> {
        let mut preparation = XmlProjectionPreparation::begin(node, source, path);
        while !preparation.advance(observation, visit)? {}
        preparation.finish()
    }
    // Package metadata consumers must not copy a descendant graph merely to
    // discard it. Classifier consumers still use the same recursive lowerer.
    fn header_from_node_at_observed(
        node: &crate::definitions::ecore::Node,
        source: &str,
        path: Vec<usize>,
        observation: &crate::definitions::ecore::ParserObservation<'_, '_>,
    ) -> Result<Self, ImportError> {
        let id = &node.opening.source;
        observation.work(1, id)?;
        observation.reserve(
            crate::definitions::retained_storage::RETAINED_NODE_BYTES,
            id,
        )?;
        let mut attrs = BTreeMap::new();
        let mut attr_spans = BTreeMap::new();
        for attr in &node.attributes {
            observation.work(1, id)?;
            if attr.namespace_declaration() {
                continue;
            }
            let name = observation.profile_name(&attr.name, id)?;
            observation.work(attr.value.len().div_ceil(256).max(1), id)?;
            let value = if let Some(qname) = &attr.type_name {
                if name == "xsi:type" {
                    observation.profile_name(qname, id)?
                } else if let Some((_, suffix)) = attr.value.split_once(char::is_whitespace) {
                    let profile = observation.profile_name(qname, id)?;
                    let suffix = suffix.trim_start();
                    let bytes = observation.size(
                        profile
                            .len()
                            .checked_add(1)
                            .and_then(|n| n.checked_add(suffix.len())),
                        id,
                    )?;
                    observation.string(bytes, id)?;
                    format!("{profile} {suffix}")
                } else {
                    observation.string(attr.value.len(), id)?;
                    attr.value.clone()
                }
            } else {
                observation.string(attr.value.len(), id)?;
                attr.value.clone()
            };
            observation.string(name.len(), id)?;
            observation.reserve(128, id)?;
            attrs.insert(name.clone(), value);
            observation.reserve(128, id)?;
            attr_spans.insert(
                name,
                observation.span(&attr.span.source, attr.span.start, attr.span.end)?,
            );
        }
        let retained_payload = if node.name.namespace.is_none()
            && (node.name.local == "eOperations"
                || (node.name.local == "eClassifiers"
                    && attrs.get("xsi:type").is_some_and(|kind| {
                        matches!(kind.as_str(), "ecore:EEnum" | "ecore:EDataType")
                    }))) {
            &source[node.span.start..node.span.end]
        } else {
            ""
        };
        observation.work(retained_payload.len().div_ceil(256).max(1), id)?;
        observation.string(retained_payload.len(), id)?;
        let source_payload = retained_payload.to_owned();
        let name = observation.profile_name(&node.name, id)?;
        observation.serialized(id, id)?;
        observation.reserve(128, id)?;
        let opening = observation.span(id, node.opening.start, node.opening.end)?;
        let mut payload = None;
        for candidate in &node.payloads {
            observation.work(1, id)?;
            let retained = match candidate.kind {
                crate::definitions::ecore::PayloadKind::Comment => false,
                crate::definitions::ecore::PayloadKind::Text => {
                    let text = &source[candidate.span.start..candidate.span.end];
                    observation.work(text.len().div_ceil(256).max(1), id)?;
                    !text.trim_matches([' ', '\t', '\r', '\n']).is_empty()
                }
                _ => true,
            };
            if retained {
                payload = Some(observation.span(
                    &candidate.span.source,
                    candidate.span.start,
                    candidate.span.end,
                )?);
                break;
            }
        }
        Ok(Self {
            source_payload,
            name,
            attrs,
            attr_spans,
            children: Vec::new(),
            declaration: crate::definitions::resources::DeclarationId {
                source: node.opening.source.clone(),
                path,
            },
            opening,
            payload,
        })
    }
    fn error(&self, message: impl Into<String>) -> ImportError {
        ImportError::new(ErrorKind::InvalidDefinition, &self.opening, message)
    }
    fn at(&self, key: &str, kind: ErrorKind, message: impl Into<String>) -> ImportError {
        ImportError::new(
            kind,
            self.attr_spans.get(key).unwrap_or(&self.opening),
            message,
        )
    }
    fn unsupported(&self, message: impl Into<String>) -> ImportError {
        ImportError::new(ErrorKind::Unsupported, &self.opening, message)
    }
}

// Annotation content is retained by the shared signature inventory, not executed.
fn annotation_metadata(node: &Xml) -> Result<(), ImportError> {
    if node.name != "eAnnotations"
        || node.children.iter().any(|child| {
            child.name != "details" || !child.children.is_empty() || child.payload.is_some()
        })
    {
        return Err(
            node.unsupported("Unsupported annotation payload (retained in imported document)")
        );
    }
    attrs(node, &["source", "references"])?;
    for child in &node.children {
        attrs(child, &["key", "value"])?;
    }
    Ok(())
}
fn attrs(node: &Xml, allowed: &[&str]) -> Result<(), ImportError> {
    if let Some(span) = &node.payload {
        return Err(ImportError::new(
            ErrorKind::Unsupported,
            span,
            "Unsupported Ecore XML payload (retained in imported document)",
        ));
    }
    for (name, value) in &node.attrs {
        if name == "xmi:version" && node.name == "ecore:EPackage" && value == "2.0" {
            continue;
        }
        if !allowed.contains(&name.as_str()) {
            let span = node.attr_spans.get(name).unwrap_or(&node.opening);
            return Err(ImportError::new(
                ErrorKind::Unsupported,
                span,
                format!("Unsupported Ecore attribute {}.{name}", node.name),
            ));
        }
    }
    Ok(())
}
fn required(node: &Xml, name: &str) -> Result<String, ImportError> {
    required_observed(node, name, None)
}
fn required_ref<'a>(node: &'a Xml, name: &str) -> Result<&'a str, ImportError> {
    node.attrs
        .get(name)
        .filter(|v| !v.is_empty())
        .map(String::as_str)
        .ok_or_else(|| node.error(format!("Missing {}.{name}", node.name)))
}
fn retained_string(
    value: &str,
    source: &crate::definitions::bundle::SourceId,
    observation: Option<&crate::definitions::ecore::ParserObservation<'_, '_>>,
) -> Result<String, ImportError> {
    if let Some(observation) = observation {
        observation.work(value.len().div_ceil(256).max(1), source)?;
        observation.string(value.len(), source)?;
    }
    Ok(value.to_owned())
}
fn required_observed(
    node: &Xml,
    name: &str,
    observation: Option<&crate::definitions::ecore::ParserObservation<'_, '_>>,
) -> Result<String, ImportError> {
    retained_string(required_ref(node, name)?, &node.opening.source, observation)
}
fn boolean(node: &Xml, name: &str, default: bool) -> Result<bool, ImportError> {
    match node.attrs.get(name).map(String::as_str) {
        None => Ok(default),
        Some("true") => Ok(true),
        Some("false") => Ok(false),
        _ => Err(node.at(
            name,
            ErrorKind::InvalidDefinition,
            format!("Invalid Boolean {name}"),
        )),
    }
}
fn local_type(value: &str) -> Result<String, String> {
    local_type_ref(value).map(String::from)
}
fn local_type_ref(value: &str) -> Result<&str, String> {
    if let Some(local) = value.strip_prefix("#//")
        && !local.is_empty()
        && !local.contains('/')
    {
        return Ok(local);
    }
    let prefix = "ecore:EDataType http://www.eclipse.org/emf/2002/Ecore#//";
    if let Some(name) = value.strip_prefix(prefix)
        && crate::definitions::scalars::Builtin::named(name).is_some()
    {
        return Ok(name);
    }
    Err(format!("Unsupported or external Ecore type {value}"))
}
/// Import one exact enum declaration through the shared resource index. This
/// factory also supplies the values used by schema defaults and model validation.
pub fn import_enum_bundle(
    bundle: &crate::definitions::bundle::DefinitionBundle,
    declaration: &crate::definitions::resources::DeclarationId,
) -> Result<crate::definitions::enums::Enumeration, ImportError> {
    import_enum_bundle_with_limits(bundle, declaration, Default::default())
}
pub(crate) fn import_enum_bundle_with_limits(
    bundle: &crate::definitions::bundle::DefinitionBundle,
    declaration: &crate::definitions::resources::DeclarationId,
    limits: crate::definitions::ecore::ParseLimits,
) -> Result<crate::definitions::enums::Enumeration, ImportError> {
    let resources = crate::definitions::resources::ResourceSet::new_with_limits(bundle, limits)?;
    let span = crate::definitions::bundle::SourceSpan {
        source: declaration.source.clone(),
        start: 0,
        end: 0,
    };
    let error = |message| ImportError::new(ErrorKind::InvalidDefinition, &span, message);
    let node = resources.node(declaration).map_err(error)?;
    let document = resources.document(&declaration.source).map_err(error)?;
    let mut xml = Xml::from_node_at(node, &document.source.text, declaration.path.clone())?;
    xml.attrs.remove("xmi:id");
    lower_enumeration(&xml)
}

/// Import one exact EDataType, preserving an explicit unsupported binding outcome.
/// This does not by itself make the declaration executable in a language package.
pub fn import_datatype_bundle(
    bundle: &crate::definitions::bundle::DefinitionBundle,
    declaration: &crate::definitions::resources::DeclarationId,
) -> Result<crate::definitions::datatypes::Datatype, ImportError> {
    import_datatype_bundle_with_limits(bundle, declaration, Default::default())
}
pub(crate) fn import_datatype_bundle_with_limits(
    bundle: &crate::definitions::bundle::DefinitionBundle,
    declaration: &crate::definitions::resources::DeclarationId,
    limits: crate::definitions::ecore::ParseLimits,
) -> Result<crate::definitions::datatypes::Datatype, ImportError> {
    let resources = crate::definitions::resources::ResourceSet::new_with_limits(bundle, limits)?;
    let span = crate::definitions::bundle::SourceSpan {
        source: declaration.source.clone(),
        start: 0,
        end: 0,
    };
    let error = |message| ImportError::new(ErrorKind::InvalidDefinition, &span, message);
    let node = resources.node(declaration).map_err(error)?;
    let document = resources.document(&declaration.source).map_err(error)?;
    let mut xml = Xml::from_node_at(node, &document.source.text, declaration.path.clone())?;
    xml.attrs.remove("xmi:id");
    let mut parameters = Vec::new();
    if xml
        .children
        .iter()
        .any(|child| child.name == "eTypeParameters")
    {
        let resolver =
            crate::definitions::generic_resolution::Resolver::new_with_limits(bundle, limits)?;
        for child in &xml.children {
            if child.name == "eTypeParameters" {
                parameters.push(resolver.resolve_parameter(&child.declaration)?.syntax);
            }
        }
        xml.children.retain(|child| child.name != "eTypeParameters");
    }
    let mut datatype = lower_datatype(&xml)?;
    datatype.retain_parameters(parameters).map_err(error)?;
    Ok(datatype)
}

fn lower_datatype(node: &Xml) -> Result<crate::definitions::datatypes::Datatype, ImportError> {
    lower_datatype_observed(node, None)
}
fn lower_datatype_observed(
    node: &Xml,
    observation: Option<&crate::definitions::ecore::ParserObservation<'_, '_>>,
) -> Result<crate::definitions::datatypes::Datatype, ImportError> {
    if node.name != "eClassifiers"
        || node.attrs.get("xsi:type").map(String::as_str) != Some("ecore:EDataType")
    {
        return Err(node.error("Expected an EDataType classifier declaration"));
    }
    attrs(
        node,
        &[
            "xsi:type",
            "name",
            "serializable",
            "instanceClassName",
            "instanceTypeName",
        ],
    )?;
    if node
        .children
        .iter()
        .any(|child| child.name != "eAnnotations")
    {
        return Err(
            node.unsupported("Extended datatype signature requires later generic/metadata support")
        );
    }
    if let Some(observation) = observation {
        observation.serialized(&node.declaration, &node.opening.source)?;
    }
    let optional = |name: &str| {
        node.attrs
            .get(name)
            .map(|value| retained_string(value, &node.opening.source, observation))
            .transpose()
    };
    let mut datatype = crate::definitions::datatypes::Datatype::new_with_formatter(
        node.declaration.clone(),
        required_observed(node, "name", observation)?,
        boolean(node, "serializable", true)?,
        optional("instanceClassName")?,
        optional("instanceTypeName")?,
        |arguments| match observation {
            Some(observation) => observation
                .formatted(&arguments, &node.opening.source)
                .map_err(|error| error.to_string()),
            None => Ok(arguments.to_string()),
        },
    )
    .map_err(|message| node.error(message))?;
    datatype.retain_source(retained_string(
        &node.source_payload,
        &node.opening.source,
        observation,
    )?);
    Ok(datatype)
}

fn lower_enumeration(node: &Xml) -> Result<crate::definitions::enums::Enumeration, ImportError> {
    lower_enumeration_observed(node, None)
}

fn lower_enumeration_observed(
    node: &Xml,
    observation: Option<&crate::definitions::ecore::ParserObservation<'_, '_>>,
) -> Result<crate::definitions::enums::Enumeration, ImportError> {
    let mut preparation = EnumLoweringPreparation::begin(node);
    while !preparation.advance(observation)? {}
    preparation.finish()
}
/// Inspect original structural validity for migration and source editing only.
pub(crate) fn inspect_ecore_bundle(
    bundle: &crate::definitions::bundle::DefinitionBundle,
    source: &crate::definitions::bundle::SourceId,
    namespace: &str,
) -> Result<Schema, ImportError> {
    resource_compiler::lower_bundle_context(bundle, source, namespace, None)
}

pub(crate) fn inspect_ecore_bundle_with_limits(
    bundle: &crate::definitions::bundle::DefinitionBundle,
    source: &crate::definitions::bundle::SourceId,
    namespace: &str,
    limits: crate::definitions::ecore::ParseLimits,
) -> Result<Schema, ImportError> {
    resource_compiler::compile_context_with_limits(
        bundle,
        source,
        namespace,
        None,
        &crate::definitions::ecore::ParserObservation::new(
            &crate::definitions::conversion::InvocationContext::default(),
        ),
        limits,
    )
}

/// Compile the supported Ecore profile with UTF-8 byte locations and explicit error kinds.
/// Use `ecore::Document::parse` to retain valid constructs beyond that profile.
pub(crate) fn inspect_ecore_schema(source: &str) -> Result<Schema, ImportError> {
    import_ecore_context(source, None)
}
fn import_ecore_context(
    source: &str,
    conversions: Option<&crate::definitions::conversion::ConversionRegistry<'_>>,
) -> Result<Schema, ImportError> {
    let document = Document::parse(DefinitionSource::new(
        "memory:ecore",
        SourceKind::Ecore,
        source,
    ))?;
    let mut root = Xml::from_node(&document.root, source)?;
    if root.name != "ecore:EPackage" {
        return Err(root.unsupported("Expected ecore:EPackage with the Ecore namespace"));
    }
    if generic_schema::requires_inventory(&document.root)
        || root.children.iter().any(|child| {
            child.name == "eSubpackages"
                || child
                    .children
                    .iter()
                    .any(|field| matches!(field.name.as_str(), "eSuperTypes" | "eOperations"))
        })
    {
        let mut bundle = crate::definitions::bundle::DefinitionBundle {
            version: 1,
            sources: vec![document.source.clone()],
            namespaces: vec![],
        };
        let resources = crate::definitions::resources::ResourceSet::new_diagnostic(&bundle)?;
        bundle.namespaces = resources
            .packages(&document.source.id)
            .map_err(|e| root.error(e))?
            .keys()
            .map(|namespace| crate::definitions::bundle::NamespaceBinding {
                namespace: namespace.clone(),
                source: document.source.id.clone(),
            })
            .collect();
        return resource_compiler::lower_bundle_context(
            &bundle,
            &document.source.id,
            &required(&root, "nsURI")?,
            conversions,
        );
    }
    if let Some(payload) = document
        .misc
        .iter()
        .find(|p| p.kind == crate::definitions::ecore::PayloadKind::ProcessingInstruction)
    {
        return Err(ImportError::new(
            ErrorKind::Unsupported,
            &payload.span,
            "Unsupported document processing instruction",
        ));
    }
    attrs(&root, &["name", "nsURI", "nsPrefix"])?;
    lower_schema(
        &mut root,
        format!("{:x}", Sha256::digest(source.as_bytes())),
        conversions,
    )
}

fn lower_schema(
    root: &mut Xml,
    revision: String,
    conversions: Option<&crate::definitions::conversion::ConversionRegistry<'_>>,
) -> Result<Schema, ImportError> {
    let mut enumerations = BTreeMap::new();
    let mut datatypes = BTreeMap::new();
    for node in &root.children {
        if node.name == "eClassifiers"
            && node.attrs.get("xsi:type").map(String::as_str) == Some("ecore:EEnum")
        {
            let enumeration = lower_enumeration(node)?;
            if enumerations
                .insert(enumeration.name().to_owned(), enumeration)
                .is_some()
            {
                return Err(node.error("Duplicate enum classifier"));
            }
        }
    }
    for node in &root.children {
        if node.name == "eClassifiers"
            && node.attrs.get("xsi:type").map(String::as_str) == Some("ecore:EDataType")
        {
            let datatype = lower_datatype(node)?;
            require_datatype_binding(&datatype, conversions)
                .map_err(|message| node.unsupported(message))?;
            if enumerations.contains_key(datatype.name())
                || datatypes
                    .insert(datatype.name().to_owned(), datatype)
                    .is_some()
            {
                return Err(node.error("Duplicate datatype classifier"));
            }
        }
    }
    let context = crate::definitions::conversion::InvocationContext::default();
    let observation = crate::definitions::ecore::ParserObservation::new(&context);
    lower_schema_with_datatypes(
        root,
        revision,
        enumerations,
        datatypes,
        conversions,
        SchemaLoweringPolicy {
            validate_projected_operations: true,
            move_feature_declarations: false,
            constructor_contracts: vec![],
            reference_top: None,
            attribute_domain_defaults: BTreeMap::new(),
        },
        &observation,
    )
}

// One stack-local field at a time. Boxing would introduce a new per-field
// allocation solely to shrink the borrowed variant; bound the inline record.
#[allow(clippy::large_enum_variant)]
enum LoweringField<'a> {
    Borrowed(&'a Xml),
    Owned(Xml),
}
const _: () = assert!(std::mem::size_of::<LoweringField<'static>>() <= 512);
impl AsRef<Xml> for LoweringField<'_> {
    fn as_ref(&self) -> &Xml {
        match self {
            Self::Borrowed(value) => value,
            Self::Owned(value) => value,
        }
    }
}
impl LoweringField<'_> {
    fn into_declaration(self) -> crate::definitions::resources::DeclarationId {
        match self {
            Self::Borrowed(value) => value.declaration.clone(),
            Self::Owned(value) => value.declaration,
        }
    }
}
struct SchemaLoweringPolicy {
    validate_projected_operations: bool,
    move_feature_declarations: bool,
    constructor_contracts: Vec<crate::definitions::conversion::ConstructorContract>,
    reference_top: Option<crate::definitions::resources::DeclarationId>,
    attribute_domain_defaults:
        BTreeMap<(String, crate::definitions::resources::DeclarationId), Value>,
}

fn lower_schema_with_datatypes(
    root: &mut Xml,
    revision: String,
    enumerations: BTreeMap<String, crate::definitions::enums::Enumeration>,
    datatypes: BTreeMap<String, crate::definitions::datatypes::Datatype>,
    conversions: Option<&crate::definitions::conversion::ConversionRegistry<'_>>,
    policy: SchemaLoweringPolicy,
    observation: &crate::definitions::ecore::ParserObservation<'_, '_>,
) -> Result<Schema, ImportError> {
    let validate_projected_operations = policy.validate_projected_operations;
    let schema = lower_schema_output_with_datatypes(
        root,
        revision,
        enumerations,
        datatypes,
        conversions,
        policy,
        observation,
    )?;
    validate_lowered_schema(root, &schema, validate_projected_operations, observation)?;
    Ok(schema)
}

// Owning output and final structural validation are phases of the same compiler.
// Separating them lets each phase retain independent admission evidence.
fn lower_schema_output_with_datatypes(
    root: &mut Xml,
    revision: String,
    enumerations: BTreeMap<String, crate::definitions::enums::Enumeration>,
    datatypes: BTreeMap<String, crate::definitions::datatypes::Datatype>,
    conversions: Option<&crate::definitions::conversion::ConversionRegistry<'_>>,
    policy: SchemaLoweringPolicy,
    observation: &crate::definitions::ecore::ParserObservation<'_, '_>,
) -> Result<Schema, ImportError> {
    let mut pending = SchemaOutputPreparation::begin(
        root,
        revision,
        enumerations,
        datatypes,
        conversions,
        policy,
        observation,
    )?;
    while !pending.advance(root, conversions, observation)? {}
    pending.finish(root)
}

// No source tree or schema is cloned. A transition lowers one class header,
// one field, or commits that class. Provider inventory and final reference-top
// checks remain explicit synchronous boundaries, not bounded-work claims.
struct SchemaOutputPreparation {
    schema: Schema,
    move_feature_declarations: bool,
    attribute_domain_defaults:
        BTreeMap<(String, crate::definitions::resources::DeclarationId), Value>,
    node_index: usize,
    field_index: usize,
    current_class: Option<(String, Class)>,
    owned_fields: Option<std::vec::IntoIter<Xml>>,
    complete: bool,
    failed: bool,
}
impl SchemaOutputPreparation {
    fn begin(
        root: &mut Xml,
        revision: String,
        enumerations: BTreeMap<String, crate::definitions::enums::Enumeration>,
        mut datatypes: BTreeMap<String, crate::definitions::datatypes::Datatype>,
        conversions: Option<&crate::definitions::conversion::ConversionRegistry<'_>>,
        policy: SchemaLoweringPolicy,
        observation: &crate::definitions::ecore::ParserObservation<'_, '_>,
    ) -> Result<Self, ImportError> {
        let SchemaLoweringPolicy {
            validate_projected_operations: _,
            move_feature_declarations,
            mut constructor_contracts,
            reference_top,
            attribute_domain_defaults,
        } = policy;
        // All callers own this private lowering tree. Route it in place rather than
        // cloning the complete descendant graph; retained imported sources remain
        // separate. Later materialization diagnostics use the same opening span.
        standard_datatypes::route(root, &mut datatypes)?;
        let mut conversion_contracts = Vec::new();
        let mut applied_conversion_contracts = Vec::new();
        for datatype in datatypes.values() {
            observation.work(1, &root.opening.source)?;
            if let Some(application) = datatype.application() {
                let registry = conversions.ok_or_else(|| {
                    root.unsupported("Explicit applied conversion registry required")
                })?;
                registry
                    .require_datatype(datatype)
                    .map_err(|e| root.unsupported(e))?;
                let contract = registry
                    .applied()
                    .require(application)
                    .map_err(|e| root.unsupported(e))?;
                retain_compilation_contract(
                    contract,
                    &mut applied_conversion_contracts,
                    &root.opening.source,
                    observation,
                )?;
                continue;
            }
            if matches!(
                datatype.binding(),
                crate::definitions::datatypes::Binding::Unsupported { .. }
            ) {
                let contract = conversions
                    .ok_or_else(|| root.unsupported("Explicit conversion registry required"))?
                    .require(datatype)
                    .map_err(|message| root.unsupported(message))?;
                retain_compilation_contract(
                    contract,
                    &mut conversion_contracts,
                    &root.opening.source,
                    observation,
                )?;
            }
        }
        if let Some(registry) = conversions {
            for contract in registry
                .compilation_dependencies()
                .map_err(|e| root.error(e))?
            {
                observation.work(1, &root.opening.source)?;
                let contract = contract.map_err(|e| root.error(e))?;
                if !conversion_contracts.contains(contract) {
                    retain_compilation_contract(
                        contract,
                        &mut conversion_contracts,
                        &root.opening.source,
                        observation,
                    )?;
                }
            }
        }
        observe_contract_sort(
            conversion_contracts.len(),
            &root.opening.source,
            observation,
        )?;
        conversion_contracts.sort_by(|a, b| a.datatype.cmp(&b.datatype));
        let revision = if conversion_contracts.is_empty() {
            revision
        } else {
            resource_compiler::json_digest(
                &(revision, &conversion_contracts),
                &root.opening.source,
                observation,
            )
            .map_err(|error| root.error(error.message))?
        };
        observe_contract_sort(
            applied_conversion_contracts.len(),
            &root.opening.source,
            observation,
        )?;
        applied_conversion_contracts
            .sort_by(|a, b| a.application.identity().cmp(b.application.identity()));
        let revision = if applied_conversion_contracts.is_empty() {
            revision
        } else {
            resource_compiler::json_digest(
                &(revision, &applied_conversion_contracts),
                &root.opening.source,
                observation,
            )
            .map_err(|error| root.error(error.message))?
        };
        if let Some(registry) = conversions {
            for contract in registry
                .constructor_dependencies()
                .map_err(|e| root.error(e))?
            {
                observation.work(1, &root.opening.source)?;
                if !constructor_contracts.contains(contract) {
                    retain_compilation_contract(
                        contract,
                        &mut constructor_contracts,
                        &root.opening.source,
                        observation,
                    )?;
                }
            }
        }
        observe_contract_sort(
            constructor_contracts.len(),
            &root.opening.source,
            observation,
        )?;
        constructor_contracts
            .sort_by(|a, b| a.datatype.declaration().cmp(b.datatype.declaration()));
        if constructor_contracts
            .windows(2)
            .any(|pair| pair[0].datatype.declaration() == pair[1].datatype.declaration())
        {
            return Err(root.error("Conflicting consumed constructor policies"));
        }
        let revision = if constructor_contracts.is_empty() {
            revision
        } else {
            resource_compiler::json_digest(
                &(revision, &constructor_contracts),
                &root.opening.source,
                observation,
            )
            .map_err(|error| root.error(error.message))?
        };
        let schema = Schema {
            runtime_feature_domains: BTreeMap::new(),
            reference_top,
            raw_reference_bounds: BTreeMap::new(),
            runtime_warnings: vec![],
            constructor_contracts,
            applied_conversion_contracts,
            declared_operation_selections: None,
            instantiations: BTreeMap::new(),
            generic_signatures: None,
            conversion_contracts,
            revision,
            name: required_observed(root, "name", Some(observation))?,
            uri: required_observed(root, "nsURI", Some(observation))?,
            classes: BTreeMap::new(),
            enumerations,
            datatypes,
            declarations: BTreeMap::new(),
        };
        Ok(Self {
            schema,
            move_feature_declarations,
            attribute_domain_defaults,
            node_index: 0,
            field_index: 0,
            current_class: None,
            owned_fields: None,
            complete: false,
            failed: false,
        })
    }
    fn advance(
        &mut self,
        root: &mut Xml,
        conversions: Option<&crate::definitions::conversion::ConversionRegistry<'_>>,
        observation: &crate::definitions::ecore::ParserObservation<'_, '_>,
    ) -> Result<bool, ImportError> {
        if self.failed {
            return Err(root.error("Schema output lowering has failed"));
        }
        let result = self.advance_inner(root, conversions, observation);
        if result.is_err() {
            self.failed = true;
        }
        result
    }
    fn advance_inner(
        &mut self,
        root: &mut Xml,
        conversions: Option<&crate::definitions::conversion::ConversionRegistry<'_>>,
        observation: &crate::definitions::ecore::ParserObservation<'_, '_>,
    ) -> Result<bool, ImportError> {
        observation.work(0, &root.opening.source)?;
        if self.complete {
            return Ok(true);
        }
        let schema = &mut self.schema;
        if self.current_class.is_some() {
            let node = root
                .children
                .get(self.node_index)
                .ok_or_else(|| root.error("Missing lowering class input"))?;
            let input = match &mut self.owned_fields {
                Some(fields) => fields.next().map(LoweringField::Owned),
                None => node
                    .children
                    .get(self.field_index)
                    .map(LoweringField::Borrowed),
            };
            if let Some(input) = input {
                let (name, class) = self
                    .current_class
                    .as_mut()
                    .ok_or_else(|| root.error("Missing lowering class state"))?;
                lower_class_field(
                    input,
                    name,
                    class,
                    schema,
                    conversions,
                    self.move_feature_declarations,
                    &self.attribute_domain_defaults,
                    observation,
                )?;
                self.field_index += 1;
                return Ok(false);
            }
            let (name, class) = self
                .current_class
                .take()
                .ok_or_else(|| root.error("Missing complete lowering class"))?;
            observation.reserve(128, &node.opening.source)?;
            if schema.classes.insert(name, class).is_some() {
                return Err(node.error("Duplicate class"));
            }
            self.owned_fields = None;
            self.node_index += 1;
            return Ok(false);
        }
        if let Some(node) = root.children.get_mut(self.node_index) {
            observation.work(1, &root.opening.source)?;
            if node.name == "eClassifiers"
                && matches!(
                    node.attrs.get("xsi:type").map(String::as_str),
                    Some("ecore:EEnum" | "ecore:EDataType")
                )
            {
                self.node_index += 1;
                return Ok(false);
            }
            if node.name != "eClassifiers"
                || node.attrs.get("xsi:type").map(String::as_str) != Some("ecore:EClass")
            {
                return Err(node.unsupported("Only EClass classifiers are supported"));
            }
            attrs(
                node,
                &["xsi:type", "name", "abstract", "interface", "eSuperTypes"],
            )?;
            let name = required_observed(node, "name", Some(observation))?;
            if schema.enumerations.contains_key(&name) || schema.datatypes.contains_key(&name) {
                return Err(node.error("Class and datatype classifier names collide"));
            }
            let parent_refs = node
                .attrs
                .get("eSuperTypes")
                .into_iter()
                .flat_map(|p| p.split_whitespace());
            let parent_name = |value: &str| {
                let name = local_type_ref(value)
                    .map_err(|e| node.at("eSuperTypes", ErrorKind::Unsupported, e))?;
                retained_string(name, &node.opening.source, Some(observation))
            };
            let mut parents = Vec::new();
            for value in parent_refs {
                let name = parent_name(value)?;
                observation.reserve(32, &node.opening.source)?;
                parents.push(name);
            }
            let class = Class {
                name: retained_string(&name, &node.opening.source, Some(observation))?,
                abstract_type: boolean(node, "abstract", false)?,
                interface_type: boolean(node, "interface", false)?,
                parents,
                features: BTreeMap::new(),
                feature_order: Vec::new(),
                feature_declarations: BTreeMap::new(),
                operations: Vec::new(),
            };
            if self.move_feature_declarations {
                self.owned_fields = Some(std::mem::take(&mut node.children).into_iter());
            }
            self.current_class = Some((name, class));
            self.field_index = 0;
            return Ok(false);
        }
        if schema.classes.is_empty()
            && schema.enumerations.is_empty()
            && schema.datatypes.is_empty()
        {
            return Err(
                root.unsupported("At least one classifier is required for an executable schema")
            );
        }
        schema.validate_reference_top().map_err(|e| root.error(e))?;
        if schema.reference_top.is_some() {
            schema.revision = resource_compiler::json_digest(
                &(&schema.revision, &schema.reference_top),
                &root.opening.source,
                observation,
            )
            .map_err(|error| root.error(error.message))?;
        }
        self.complete = true;
        Ok(true)
    }

    fn finish(self, root: &Xml) -> Result<Schema, ImportError> {
        if self.failed || !self.complete {
            return Err(root.error("Schema output lowering is incomplete"));
        }
        Ok(self.schema)
    }
}

#[allow(clippy::too_many_arguments)]
fn lower_class_field(
    input: LoweringField<'_>,
    name: &str,
    class: &mut Class,
    schema: &Schema,
    conversions: Option<&crate::definitions::conversion::ConversionRegistry<'_>>,
    move_feature_declarations: bool,
    attribute_domain_defaults: &BTreeMap<
        (String, crate::definitions::resources::DeclarationId),
        Value,
    >,
    observation: &crate::definitions::ecore::ParserObservation<'_, '_>,
) -> Result<(), ImportError> {
    let field = input.as_ref();
    observation.work(1, &field.opening.source)?;
    if field.name == "eAnnotations" {
        annotation_metadata(field)?;
        return Ok(());
    }
    if field.name == "eOperations" {
        let operation = lower_operation(
            field,
            &schema.enumerations,
            &schema.datatypes,
            Some(observation),
        )?;
        observation.reserve(32, &field.opening.source)?;
        class.operations.push(operation);
        return Ok(());
    }
    let default_key = if attribute_domain_defaults.is_empty() {
        None
    } else {
        observation.serialized(&field.declaration, &field.opening.source)?;
        Some((
            retained_string(name, &field.opening.source, Some(observation))?,
            field.declaration.clone(),
        ))
    };
    let feature = lower_feature(
        field,
        &schema.enumerations,
        &schema.datatypes,
        conversions,
        default_key
            .as_ref()
            .and_then(|key| attribute_domain_defaults.get(key)),
        Some(observation),
    )?;
    let order_name = retained_string(&feature.name, &field.opening.source, Some(observation))?;
    observation.reserve(32, &field.opening.source)?;
    class.feature_order.push(order_name);
    let declaration_name =
        retained_string(&feature.name, &field.opening.source, Some(observation))?;
    if !move_feature_declarations {
        observation.serialized(&field.declaration, &field.opening.source)?;
    }
    observation.reserve(128, &field.opening.source)?;
    let feature_name = retained_string(&feature.name, &field.opening.source, Some(observation))?;
    observation.reserve(128, &field.opening.source)?;
    if class.features.contains_key(&feature_name) {
        return Err(field.error("Duplicate feature"));
    }
    let declaration = input.into_declaration();
    class
        .feature_declarations
        .insert(declaration_name, declaration);
    class.features.insert(feature_name, feature);
    Ok(())
}

#[path = "compiler_validation.rs"]
mod validation;
pub(crate) use validation::CompletedLoweredInputs;
use validation::{FrozenLoweredSchema, LoweredSchemaValidation};

fn validate_lowered_schema(
    root: &Xml,
    schema: &Schema,
    validate_projected_operations: bool,
    observation: &crate::definitions::ecore::ParserObservation<'_, '_>,
) -> Result<(), ImportError> {
    let mut pending =
        LoweredSchemaValidation::new(root, validate_projected_operations, observation)?;
    while !pending.advance(root, schema, observation)? {}
    Ok(())
}

#[cfg(test)]
fn compiler_features(
    schema: &Schema,
    receiver: &str,
    source: &crate::definitions::bundle::SourceId,
    observation: &crate::definitions::ecore::ParserObservation<'_, '_>,
) -> Result<PreparedFeatures, ImportError> {
    let input = compiler_features_policy(schema, receiver, source, observation, false, None)?;
    observation.preparation(source, |context| input.into_owned_observed(context))
}
#[cfg(test)]
fn compiler_features_policy(
    schema: &Schema,
    receiver: &str,
    source: &crate::definitions::bundle::SourceId,
    observation: &crate::definitions::ecore::ParserObservation<'_, '_>,
    identity_views: bool,
    input_cache: Option<&SharedFeatureInputCache>,
) -> Result<PreparedFeatureInputs, ImportError> {
    observation.preparation(source, |context| {
        let mut pending = if identity_views {
            FeatureMapPreparation::with_source_inputs_observed(
                receiver,
                input_cache
                    .ok_or("Missing compiler feature input cache")?
                    .clone(),
                context,
            )?
        } else {
            FeatureMapPreparation::new_observed(receiver, context)?
        };
        loop {
            context.work(1).map_err(PreparationError::Observation)?;
            if pending.advance_observed(schema, context)? {
                return pending.finish_inputs().map_err(PreparationError::Schema);
            }
        }
    })
}

#[cfg(test)]
fn compiler_conforms(
    schema: &Schema,
    actual: &str,
    expected: &str,
    source: &crate::definitions::bundle::SourceId,
    observation: &crate::definitions::ecore::ParserObservation<'_, '_>,
) -> Result<bool, ImportError> {
    observation.preparation(source, |context| {
        schema.conforms_observed(actual, expected, context)
    })
}

fn retain_compilation_contract<T: Clone + serde::Serialize>(
    contract: &T,
    retained: &mut Vec<T>,
    source: &crate::definitions::bundle::SourceId,
    observation: &crate::definitions::ecore::ParserObservation<'_, '_>,
) -> Result<(), ImportError> {
    observation.serialized(contract, source)?;
    observation.reserve(32, source)?;
    retained.push(contract.clone());
    Ok(())
}

// Stable sorting and comparisons remain atomic. Guard their temporary ownership
// and observe collection work without claiming host-quantum coverage.
fn observe_contract_sort(
    len: usize,
    source: &crate::definitions::bundle::SourceId,
    observation: &crate::definitions::ecore::ParserObservation<'_, '_>,
) -> Result<(), ImportError> {
    observation.work(len, source)?;
    observation.reserve(observation.size(len.checked_mul(32), source)?, source)
}
/// Canonical source metadata plus exact checked expressions, shared by schema bridges.
pub(crate) fn canonical_operation_from_source(
    node: &crate::definitions::ecore::Node,
    source: &str,
    member: &crate::definitions::generic_resolution::SpecializedMember,
) -> Result<crate::definitions::operations::Operation, ImportError> {
    let mut xml = Xml::from_node_at(node, source, member.declaration.path.clone())?;
    if xml.name != "eOperations" || xml.declaration != member.declaration {
        return Err(xml.error("Expected exact source operation declaration"));
    }
    materialization::prepare_template(&mut xml)?;
    let mut operation = lower_operation(&xml, &BTreeMap::new(), &BTreeMap::new(), None)?;
    crate::definitions::operation_templates::project(&mut operation, member)
        .map_err(|e| xml.error(e))?;
    Ok(operation)
}

fn lower_operation(
    node: &Xml,
    enumerations: &BTreeMap<String, crate::definitions::enums::Enumeration>,
    datatypes: &BTreeMap<String, crate::definitions::datatypes::Datatype>,
    observation: Option<&crate::definitions::ecore::ParserObservation<'_, '_>>,
) -> Result<crate::definitions::operations::Operation, ImportError> {
    use crate::definitions::operations::{Operation, Parameter};
    attrs(
        node,
        &[
            "name",
            "eType",
            "lowerBound",
            "upperBound",
            "ordered",
            "unique",
            "eExceptions",
        ],
    )?;
    let mut parameters = Vec::new();
    for child in &node.children {
        if let Some(observation) = observation {
            observation.work(1, &child.opening.source)?;
        }
        match child.name.as_str() {
            "eParameters" => {
                attrs(
                    child,
                    &[
                        "name",
                        "eType",
                        "lowerBound",
                        "upperBound",
                        "ordered",
                        "unique",
                    ],
                )?;
                if child.children.iter().any(|n| n.name != "eAnnotations") {
                    return Err(child.unsupported(
                        "Generic or extended parameter signature requires E07/E08/E12",
                    ));
                }
                let name = required_observed(child, "name", observation)?;
                if let Some(observation) = observation {
                    observation.serialized(&child.declaration, &child.opening.source)?;
                    observation.reserve(32, &child.opening.source)?;
                }
                parameters.push(Parameter {
                    name,
                    declaration: child.declaration.clone(),
                    value: lower_operation_value(child, enumerations, datatypes, observation)?,
                });
            }
            // Retained source metadata; delegate/exception semantics are E12/S11.
            "eAnnotations" | "eExceptions" => {}
            _ => {
                return Err(child
                    .unsupported("Generic or extended operation signature requires E07/E08/E12"));
            }
        }
    }
    let name = required_observed(node, "name", observation)?;
    if let Some(observation) = observation {
        observation.serialized(&node.declaration, &node.opening.source)?;
    }
    Ok(Operation {
        name,
        declaration: node.declaration.clone(),
        parameters,
        result: lower_operation_value(node, enumerations, datatypes, observation)?,
        source_payload: retained_string(&node.source_payload, &node.opening.source, observation)?,
        generic: false,
    })
}

fn lower_operation_value(
    node: &Xml,
    enumerations: &BTreeMap<String, crate::definitions::enums::Enumeration>,
    datatypes: &BTreeMap<String, crate::definitions::datatypes::Datatype>,
    observation: Option<&crate::definitions::ecore::ParserObservation<'_, '_>>,
) -> Result<crate::definitions::operations::TypedElement, ImportError> {
    use crate::definitions::operations::{TypeTarget, TypedElement};
    let target = node
        .attrs
        .get("eType")
        .map(|ty| {
            let name =
                local_type_ref(ty).map_err(|e| node.at("eType", ErrorKind::Unsupported, e))?;
            let name = retained_string(name, &node.opening.source, observation)?;
            Ok(if ty.starts_with("#//") {
                if enumerations.contains_key(&name) {
                    TypeTarget::Enumeration(name)
                } else if datatypes.contains_key(&name) {
                    TypeTarget::Datatype(name)
                } else {
                    TypeTarget::Class(name)
                }
            } else {
                TypeTarget::Builtin(name)
            })
        })
        .transpose()?;
    let lower = node
        .attrs
        .get("lowerBound")
        .map_or(Ok(0), |value| value.parse::<usize>())
        .map_err(|_| {
            node.at(
                "lowerBound",
                ErrorKind::InvalidDefinition,
                "Invalid operation lowerBound",
            )
        })?;
    let upper = node
        .attrs
        .get("upperBound")
        .map_or(Ok(1), |value| value.parse::<i32>())
        .map_err(|_| {
            node.at(
                "upperBound",
                ErrorKind::InvalidDefinition,
                "Invalid operation upperBound",
            )
        })?;
    if upper < -1 || (upper >= 0 && lower > upper as usize) {
        return Err(node.at(
            "upperBound",
            ErrorKind::InvalidDefinition,
            "Invalid operation multiplicity",
        ));
    }
    Ok(TypedElement {
        target,
        lower,
        upper,
        ordered: boolean(node, "ordered", true)?,
        unique: boolean(node, "unique", true)?,
    })
}
fn feature_multiplicity(field: &Xml) -> Result<(usize, i32), ImportError> {
    let lower = field
        .attrs
        .get("lowerBound")
        .map_or(Ok(0), |s| s.parse::<usize>())
        .map_err(|_| {
            field.at(
                "lowerBound",
                ErrorKind::InvalidDefinition,
                "Invalid lowerBound",
            )
        })?;
    let upper = field
        .attrs
        .get("upperBound")
        .map_or(Ok(1), |s| s.parse::<i32>())
        .map_err(|_| {
            field.at(
                "upperBound",
                ErrorKind::InvalidDefinition,
                "Invalid upperBound",
            )
        })?;
    if upper < -1 || (upper >= 0 && lower > upper as usize) {
        return Err(field.at(
            "upperBound",
            ErrorKind::InvalidDefinition,
            "Invalid multiplicity",
        ));
    }
    if upper == 0 {
        return Err(field.at(
            "upperBound",
            ErrorKind::Unsupported,
            "Zero upperBound is unsupported",
        ));
    }
    Ok((lower, upper))
}

/// Checked declaration shape shared with symbolic reads.
pub(crate) struct SymbolicFeatureShape {
    pub lower: usize,
    pub upper: i32,
    pub ordered: bool,
    pub unique: bool,
    pub derived: bool,
    pub changeable: bool,
    pub transient: bool,
    pub volatile: bool,
    pub unsettable: bool,
}

/// Symbolic reads use the same multiplicity and flag rules as materialized features.
pub(crate) fn symbolic_feature_shape(
    node: &crate::definitions::ecore::Node,
    source: &str,
    path: Vec<usize>,
) -> Result<SymbolicFeatureShape, ImportError> {
    let field = Xml::from_node_at(node, source, path)?;
    let (lower, upper) = feature_multiplicity(&field)?;
    Ok(SymbolicFeatureShape {
        lower,
        upper,
        ordered: boolean(&field, "ordered", true)?,
        unique: boolean(&field, "unique", true)?,
        derived: boolean(&field, "derived", false)?,
        changeable: boolean(&field, "changeable", true)?,
        transient: boolean(&field, "transient", false)?,
        volatile: boolean(&field, "volatile", false)?,
        unsettable: boolean(&field, "unsettable", false)?,
    })
}

fn lower_feature(
    field: &Xml,
    enumerations: &BTreeMap<String, crate::definitions::enums::Enumeration>,
    datatypes: &BTreeMap<String, crate::definitions::datatypes::Datatype>,
    conversions: Option<&crate::definitions::conversion::ConversionRegistry<'_>>,
    domain_default: Option<&Value>,
    observation: Option<&crate::definitions::ecore::ParserObservation<'_, '_>>,
) -> Result<Feature, ImportError> {
    if field.name != "eStructuralFeatures"
        || field
            .children
            .iter()
            .any(|child| child.name != "eAnnotations")
    {
        let location = field.children.first().unwrap_or(field);
        return Err(location.unsupported(
            "Operations, annotations and nested feature definitions are unsupported",
        ));
    }
    for annotation in &field.children {
        annotation_metadata(annotation)?;
    }
    attrs(
        field,
        &[
            "xsi:type",
            "name",
            "eType",
            "lowerBound",
            "upperBound",
            "containment",
            "eOpposite",
            "ordered",
            "unique",
            "derived",
            "unsettable",
            "changeable",
            "transient",
            "volatile",
            "defaultValueLiteral",
        ],
    )?;
    let reference = match required_ref(field, "xsi:type")? {
        "ecore:EAttribute" => false,
        "ecore:EReference" => true,
        _ => {
            return Err(field.at(
                "xsi:type",
                ErrorKind::Unsupported,
                "Unsupported structural feature kind",
            ));
        }
    };
    let name = required_observed(field, "name", observation)?;
    let type_spelling = required_ref(field, "eType")?;
    let target =
        local_type_ref(type_spelling).map_err(|e| field.at("eType", ErrorKind::Unsupported, e))?;
    let target = retained_string(target, &field.opening.source, observation)?;
    let declared = !reference && type_spelling.starts_with("#//");
    let enumeration = declared.then(|| enumerations.get(&target)).flatten();
    let datatype = declared.then(|| datatypes.get(&target)).flatten();
    if declared && enumeration.is_none() && datatype.is_none() && domain_default.is_none() {
        return Err(field.at(
            "eType",
            ErrorKind::InvalidDefinition,
            "Attribute must identify a supported datatype",
        ));
    }
    let (lower, upper) = feature_multiplicity(field)?;
    let literal = field.attrs.get("defaultValueLiteral");
    let default = if upper != 1 {
        if literal.is_some() {
            return Err(field.at(
                "defaultValueLiteral",
                ErrorKind::Unsupported,
                "List defaults unsupported",
            ));
        }
        Value::Array(vec![])
    } else if reference {
        if literal.is_some() {
            return Err(field.at(
                "defaultValueLiteral",
                ErrorKind::Unsupported,
                "Reference defaults unsupported",
            ));
        }
        Value::Null
    } else if let Some(default) = domain_default {
        if let Some(observation) = observation {
            observation.serialized(default, &field.opening.source)?;
        }
        default.clone()
    } else if let Some(enumeration) = enumeration {
        let value = match literal {
            Some(spelling) => Some(enumeration.parse(spelling).map_err(|message| {
                field.at("defaultValueLiteral", ErrorKind::InvalidDefinition, message)
            })?),
            None => enumeration.default_value(),
        };
        serde_json::to_value(value).map_err(|error| field.error(error.to_string()))?
    } else if let Some(datatype) = datatype {
        if matches!(
            datatype.binding(),
            crate::definitions::datatypes::Binding::Unsupported { .. }
        ) {
            let registry = conversions
                .ok_or_else(|| field.unsupported("Explicit conversion registry required"))?;
            match literal {
                Some(text) => registry.parse_datatype(datatype, text).map_err(|message| {
                    field.at("defaultValueLiteral", ErrorKind::InvalidDefinition, message)
                })?,
                None => registry
                    .datatype_default(datatype)
                    .map_err(|message| field.error(message))?,
            }
        } else {
            match literal {
                Some(text) => serde_json::to_value(datatype.parse(text).map_err(|message| {
                    field.at("defaultValueLiteral", ErrorKind::InvalidDefinition, message)
                })?)
                .map_err(|error| field.error(error.to_string()))?,
                None => datatype
                    .default_value()
                    .map_err(|message| field.error(message))?,
            }
        }
    } else {
        let codec = crate::definitions::scalars::Builtin::named(&target).ok_or_else(|| {
            field.at(
                "eType",
                ErrorKind::Unsupported,
                "Unsupported attribute type",
            )
        })?;
        match literal {
            None => codec.default_value(),
            Some(literal) => codec.parse(literal).map_err(|error| {
                field.at(
                    "defaultValueLiteral",
                    ErrorKind::InvalidDefinition,
                    format!("Invalid {target} default: {error}"),
                )
            })?,
        }
    };
    let opposite = field
        .attrs
        .get("eOpposite")
        .map(|v| {
            let opposite = v.strip_prefix("#//").ok_or_else(|| {
                field.at(
                    "eOpposite",
                    ErrorKind::Unsupported,
                    "External opposite unsupported",
                )
            })?;
            retained_string(opposite, &field.opening.source, observation)
        })
        .transpose()?;
    let declaration = enumeration
        .map(|enumeration| enumeration.declaration())
        .or_else(|| datatype.map(|datatype| datatype.declaration()));
    if let (Some(observation), Some(declaration)) = (observation, declaration) {
        observation.serialized(declaration, &field.opening.source)?;
    }
    let feature = Feature {
        name,
        target,
        datatype: declaration.cloned(),
        reference,
        containment: boolean(field, "containment", false)?,
        opposite,
        lower,
        upper,
        unique: boolean(field, "unique", true)?,
        ordered: boolean(field, "ordered", true)?,
        derived: boolean(field, "derived", false)?,
        unsettable: boolean(field, "unsettable", false)?,
        changeable: boolean(field, "changeable", true)?,
        transient: boolean(field, "transient", false)?,
        volatile: boolean(field, "volatile", false)?,
        default,
    };
    if !reference && (feature.containment || feature.opposite.is_some()) {
        return Err(field.error("Containment/opposites require EReference"));
    }
    Ok(feature)
}
pub(crate) fn check_semantics(semantics: &Semantics, schema: &Schema) -> Result<(), String> {
    check_semantics_for_finish(semantics, schema, SemanticFinish::Strict, &[])
}
use crate::definitions::model_definition::ModelPreparationMode as SemanticFinish;
fn check_semantics_for_finish(
    semantics: &Semantics,
    schema: &Schema,
    finish: SemanticFinish,
    settings: &[crate::definitions::setting::ComputedSettingContract],
) -> Result<(), String> {
    if semantics.version != 1 || semantics.scope != "unique_names" {
        return Err("rcore_behavior requires version 1 and scope unique_names".into());
    }
    let mut initialized = std::collections::BTreeSet::new();
    for rule in &semantics.initializers {
        let class = schema
            .classes
            .get(&rule.class)
            .ok_or("Unknown initializer class")?;
        if class.abstract_type || class.interface_type {
            return Err("Initializer declaration requires a concrete class".into());
        }
        if !initialized.insert(&rule.class) {
            return Err("Duplicate class initializer".into());
        }
        if rule.provider.name.trim().is_empty() || rule.provider.version.trim().is_empty() {
            return Err("Initializer requires an exact provider name and version".into());
        }
        let fields = schema.features(&rule.class)?;
        if rule.provider.name == "intersect.constant-attributes" {
            if rule.provider.version != "1" {
                return Err("Unsupported constant initializer provider version".into());
            }
            let values = rule
                .values
                .as_ref()
                .ok_or("Constant initializer requires values")?;
            let names: std::collections::BTreeSet<_> = rule.members.iter().collect();
            if values.keys().collect::<std::collections::BTreeSet<_>>() != names {
                return Err("Constant initializer values must match declared members".into());
            }
            if rule
                .members
                .iter()
                .any(|name| fields.get(name).is_some_and(|f| f.reference))
            {
                return Err("Constant initializer supports attributes only".into());
            }
        } else if rule.values.is_some() {
            return Err("Initializer values require the constant-attribute provider".into());
        }
        let mut members = std::collections::BTreeSet::new();
        for name in &rule.members {
            let field = fields.get(name).ok_or("Unknown initializer member")?;
            if field.derived || !members.insert(name) {
                return Err("Initializer members must be unique stored features".into());
            }
        }
    }
    let mut defined = std::collections::BTreeSet::new();
    for rule in &semantics.derived {
        if !defined.insert((rule.class.clone(), rule.feature.clone())) {
            return Err("Duplicate derivation".into());
        }
        let fields = schema.features(&rule.class)?;
        let output = fields.get(&rule.feature).ok_or("Unknown derived feature")?;
        if !output.derived
            || (!matches!(
                rule.expression,
                Expression::Stored { .. } | Expression::Source { .. }
            ) && (output.reference || output.upper != 1))
        {
            return Err("Derivation requires a singular derived attribute".into());
        }
        match &rule.expression {
            Expression::Source { .. } => {
                super::rcore_getter::check_storage_feature(schema, output)?;
            }
            Expression::External { arguments, .. } => {
                if output.lower != 1
                    || output.datatype.is_some()
                    || !matches!(output.target.as_str(), "ELong" | "EString" | "EBoolean")
                {
                    return Err(
                        "External derivations require a required Int64, String or Bool output"
                            .into(),
                    );
                }
                for argument in arguments {
                    if let crate::definitions::schema::DerivedArgument::Stored { feature } =
                        argument
                    {
                        let input = fields
                            .get(feature)
                            .ok_or("Unknown external argument feature")?;
                        if input.derived
                            || input.reference
                            || input.lower != 1
                            || input.upper != 1
                            || input.datatype.is_some()
                            || !matches!(input.target.as_str(), "ELong" | "EString" | "EBoolean")
                        {
                            return Err(
                                "External derivations require required stored scalar arguments"
                                    .into(),
                            );
                        }
                    }
                }
            }
            Expression::Stored { feature, writable } => {
                let input = fields
                    .get(feature)
                    .ok_or("Unknown stored projection dependency")?;
                let domains = |name: &str| {
                    schema
                        .feature_domains(&rule.class, name)
                        .into_iter()
                        .map(|domain| {
                            (
                                &domain.source.expression,
                                &domain.alternatives,
                                &domain.unresolved,
                                &domain.lexical_target,
                            )
                        })
                        .collect::<Vec<_>>()
                };
                let raw = schema.raw_reference_bounds.get(&rule.class);
                if input.derived
                    || (output.unsettable
                        && (!input.unsettable || schema.is_container_feature(input)?))
                    || output.containment
                    || output.opposite.is_some()
                    || input.reference != output.reference
                    || input.target != output.target
                    || input.datatype != output.datatype
                    || input.lower != output.lower
                    || input.upper != output.upper
                    || input.ordered != output.ordered
                    || input.unique != output.unique
                    || input.default != output.default
                    || domains(feature) != domains(&rule.feature)
                    || raw.and_then(|bounds| bounds.get(feature))
                        != raw.and_then(|bounds| bounds.get(&rule.feature))
                {
                    return Err("Stored projection requires a stored dependency with the same value contract and a derived output without containment/opposites; unsettable outputs require assignment-history backing rather than container-presence state".into());
                }
                if *writable && (!input.changeable || !output.changeable) {
                    return Err(
                        "Writable stored projection requires changeable input and output".into(),
                    );
                }
            }
            Expression::Count { feature } => {
                let input = fields.get(feature).ok_or("Unknown count dependency")?;
                if input.upper == 1
                    || input.derived
                    || output.datatype.is_some()
                    || !matches!(output.target.as_str(), "EInt" | "ELong")
                {
                    return Err("count requires a stored list and EInt or ELong output".into());
                }
            }
            Expression::NoIncoming { class, feature } => {
                let inputs = schema.features(class)?;
                let input = inputs.get(feature).ok_or("Unknown incoming dependency")?;
                if !input.reference
                    || input.derived
                    || !schema.conforms(&rule.class, &input.target)
                    || output.target != "EBoolean"
                    || output.datatype.is_some()
                {
                    return Err("no_incoming requires a stored reference to the output class and EBoolean output".into());
                }
            }
        }
    }
    for class in schema.classes.keys() {
        for field in schema.features(class)?.values().filter(|f| f.derived) {
            let providers = semantics
                .derived
                .iter()
                .filter(|d| schema.conforms(class, &d.class) && d.feature == field.name)
                .count();
            // A validated source-bound setting contract requires an installed
            // native getter as well as its inverse edits. It may own the read
            // without a second source-authored derivation of the same feature.
            let native = settings.iter().any(|contract| {
                contract.feature == field.name
                    && schema.conforms(class, &contract.class)
                    && schema
                        .effective_feature(class, &field.name)
                        .is_ok_and(|effective| effective.identity == contract.member)
            });
            if providers != 1 && !(providers == 0 && (finish.allows_missing() || native)) {
                return Err(format!(
                    "Expected exactly one semantic provider for {class}.{}",
                    field.name
                ));
            }
        }
    }
    for rule in &semantics.validations {
        let fields = schema.features(&rule.class)?;
        let input = fields
            .get(&rule.feature)
            .ok_or("Unknown validation dependency")?;
        if !input.reference || input.derived {
            return Err("max_incoming validates a stored reference".into());
        }
    }
    Ok(())
}
pub(crate) fn compile_semantics_source(
    schema: &mut Schema,
    source: &DefinitionSource,
    limits: crate::definitions::ecore::ParseLimits,
    imported_models: bool,
) -> Result<Semantics, Vec<Diagnostic>> {
    compile_semantics_source_for_finish(
        schema,
        source,
        limits,
        imported_models,
        SemanticFinish::Strict,
        &[],
    )
}
pub(crate) fn compile_semantics_source_for_finish(
    schema: &mut Schema,
    source: &DefinitionSource,
    limits: crate::definitions::ecore::ParseLimits,
    imported_models: bool,
    finish: SemanticFinish,
    settings: &[crate::definitions::setting::ComputedSettingContract],
) -> Result<Semantics, Vec<Diagnostic>> {
    let parsed = crate::definitions::rcore_behavior::parse_with_limits(source.clone(), limits);
    let errors: Vec<_> = parsed
        .diagnostics
        .iter()
        .map(|d| {
            Diagnostic::new(
                "rcore_behavior",
                d.span.start,
                d.span.end,
                d.message.clone(),
            )
        })
        .collect();
    if !errors.is_empty() {
        return Err(errors);
    }
    let semantics = if imported_models {
        crate::definitions::rcore_behavior_rules::lower_with_imported_models(&parsed, schema)?
    } else {
        crate::definitions::rcore_behavior_rules::lower(&parsed, schema)?
    };
    check_semantics_for_finish(&semantics, schema, finish, settings)
        .map_err(|e| vec![Diagnostic::new("rcore_behavior", 0, 1, e)])?;
    Ok(semantics)
}

fn require_datatype_binding(
    datatype: &crate::definitions::datatypes::Datatype,
    conversions: Option<&crate::definitions::conversion::ConversionRegistry<'_>>,
) -> Result<(), String> {
    match datatype.codec() {
        Ok(_) => Ok(()),
        Err(error) => match conversions {
            Some(registry) => registry.require_datatype(datatype),
            None => Err(error),
        },
    }
}

#[cfg(test)]
pub(crate) mod xml_projection_controls {
    // Literal totals include observed source-node lookup: one document selection
    // plus one unit per child ordinal, with no retention. Copy/validation
    // component derivations below exclude that work unless explicitly included.
    use super::*;
    use crate::definitions::{
        conversion::{InvocationContext, InvocationObserver, ProviderId},
        ecore::{ObservedImportError, ParserObservation},
    };
    use std::{cell::Cell, rc::Rc};
    pub(crate) struct Probe {
        storage: Cell<usize>,
        work: Cell<usize>,
        calls: Cell<usize>,
        cap: usize,
        max_work: usize,
        cancel: Option<usize>,
    }
    impl Probe {
        fn new(cap: usize, cancel: Option<usize>) -> Rc<Self> {
            Self::with_work(cap, usize::MAX, cancel)
        }
        fn with_work(cap: usize, max_work: usize, cancel: Option<usize>) -> Rc<Self> {
            Rc::new(Self {
                storage: Cell::new(0),
                work: Cell::new(0),
                calls: Cell::new(0),
                cap,
                max_work,
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
            panic!("projection cannot invoke providers")
        }
        fn work(&self, n: usize) -> Result<(), String> {
            self.touch()?;
            let next = self.work.get() + n;
            if next > self.max_work {
                return Err("projection work denied".into());
            }
            self.work.set(next);
            Ok(())
        }
        fn retain_storage(&self, n: usize) -> Result<(), String> {
            self.touch()?;
            let next = self.storage.get() + n;
            if next > self.cap {
                return Err("projection storage denied".into());
            }
            self.storage.set(next);
            Ok(())
        }
    }
    #[test]
    fn compiler_conformance_keeps_schema_predicates_distinct_from_interruptions() {
        let schema = inspect_ecore_schema("<ecore:EPackage xmlns:ecore='http://www.eclipse.org/emf/2002/Ecore' xmlns:xsi='http://www.w3.org/2001/XMLSchema-instance' name='p' nsURI='urn:p'><eClassifiers xsi:type='ecore:EClass' name='C'/></ecore:EPackage>").unwrap();
        let original = schema.clone();
        let source = DefinitionSource::new("memory:x", SourceKind::Ecore, "<a/>").id;
        // C/C keys130, ancestry initial97/Enter97/Check32/Clear129/Exit97:
        // 582 bytes. Header1/ancestry6/Exact1/Clear1:9 work.
        assert_lowering_receipt(&true, 582, 9, &[130, 227, 324, 356, 485, 582], |probe| {
            let context = InvocationContext::new(probe.clone());
            let observation = ParserObservation::new(&context);
            observation.finish(compiler_conforms(&schema, "C", "C", &source, &observation))
        });
        // Missing/C keys136 + Missing ancestry task103:239; Header/Enter:2.
        assert_lowering_receipt(&false, 239, 2, &[136, 239], |probe| {
            let context = InvocationContext::new(probe.clone());
            let observation = ParserObservation::new(&context);
            observation.finish(compiler_conforms(
                &schema,
                "Missing",
                "C",
                &source,
                &observation,
            ))
        });
        let mut cyclic = schema.clone();
        cyclic.classes.get_mut("C").unwrap().parents = vec!["C".into()];
        assert!(!cyclic.conforms("C", "C"));
        // C/C130, initial97, Enter97, Check-parent97, Check-end32,
        // Clear129, Parents129:711. Header plus six ancestry steps:7 work.
        assert_lowering_receipt(
            &false,
            711,
            7,
            &[130, 227, 324, 421, 453, 582, 711],
            |probe| {
                let context = InvocationContext::new(probe.clone());
                let observation = ParserObservation::new(&context);
                observation.finish(compiler_conforms(&cyclic, "C", "C", &source, &observation))
            },
        );
        let context = InvocationContext::new(Probe::new(usize::MAX, None));
        let observation = ParserObservation::new(&context);
        assert_eq!(
            observation
                .finish(compiler_features(&cyclic, "C", &source, &observation))
                .map(|_| ())
                .unwrap_err(),
            ObservedImportError::Import(ImportError {
                kind: ErrorKind::InvalidDefinition,
                span: crate::definitions::bundle::SourceSpan {
                    source,
                    start: 0,
                    end: 0
                },
                message: "Cyclic inheritance at C".into(),
            }),
        );
        assert_eq!(schema, original);
    }

    #[test]
    fn compiler_final_validation_admits_empty_feature_maps_before_retention() {
        let source = DefinitionSource::new(
            "memory:x",
            SourceKind::Ecore,
            "<ecore:EPackage xmlns:ecore='http://www.eclipse.org/emf/2002/Ecore' xmlns:xsi='http://www.w3.org/2001/XMLSchema-instance' name='p' nsURI='urn:p'><eClassifiers xsi:type='ecore:EClass' name='C'/></ecore:EPackage>",
        );
        let document = Document::parse(source).unwrap();
        let root = Xml::from_node(&document.root, &document.source.text).unwrap();
        let context = InvocationContext::default();
        let observation = ParserObservation::new(&context);
        let schema = lower_schema_output_with_datatypes(
            &mut root.clone(),
            "r".into(),
            BTreeMap::new(),
            BTreeMap::new(),
            None,
            SchemaLoweringPolicy {
                validate_projected_operations: true,
                move_feature_declarations: false,
                constructor_contracts: vec![],
                reference_top: None,
                attribute_domain_defaults: BTreeMap::new(),
            },
            &observation,
        )
        .unwrap();
        let original = schema.clone();
        // Ordinary operation inventory: ancestry452 and borrowed owner slot32,
        // eight work units. Feature maps add582 bytes and seventeen units.
        assert_lowering_receipt(
            &(),
            1066,
            25,
            &[
                97, 194, 226, 355, 452, 484, 549, 614, 711, 808, 840, 969, 1066,
            ],
            |probe| {
                let context = InvocationContext::new(probe.clone());
                let observation = ParserObservation::new(&context);
                observation.finish(validate_lowered_schema(&root, &schema, true, &observation))
            },
        );
        assert_eq!(schema, original);
    }

    #[test]
    fn ordinary_operation_resolution_guards_inventory_signature_and_selection() {
        let schema = inspect_ecore_schema("<ecore:EPackage xmlns:ecore='http://www.eclipse.org/emf/2002/Ecore' xmlns:xsi='http://www.w3.org/2001/XMLSchema-instance' name='p' nsURI='urn:p'><eClassifiers xsi:type='ecore:EClass' name='C'><eOperations name='Op'/></eClassifiers></ecore:EPackage>").unwrap();
        let original = schema.resolved_operations("C").unwrap();
        assert_eq!(original.len(), 1);
        assert_eq!(original[0].signature.name, "Op");
        assert!(original[0].signature.parameters.is_empty());
        assert_eq!(original[0].selected.owner, "C");
        assert_eq!(original[0].selected.operation.declaration.path, vec![0, 0]);
        assert!(original[0].overridden.is_empty());
        let source = DefinitionSource::new("memory:x", SourceKind::Ecore, "<a/>").id;
        // Ancestry484, inventory160, name66, signature metadata662 (struct
        // keys charge raw lengths), index128, group/candidate slots64,
        // conformance582, final result slot32:2178.
        assert_lowering_receipt(
            &original,
            2178,
            27,
            &[
                97, 194, 226, 355, 452, 484, 612, 644, 710, 1372, 1500, 1532, 1564, 1694, 1791,
                1888, 1920, 2049, 2146, 2178,
            ],
            |probe| {
                let context = InvocationContext::new(probe.clone());
                let observation = ParserObservation::new(&context);
                observation.finish(observation.preparation(&source, |context| {
                    schema.resolve_native_operation_groups("C", context)
                }))
            },
        );
    }

    #[test]
    fn ordinary_operation_parameter_identity_guards_validation_and_signature_copies() {
        let schema = inspect_ecore_schema("<ecore:EPackage xmlns:ecore='http://www.eclipse.org/emf/2002/Ecore' xmlns:xsi='http://www.w3.org/2001/XMLSchema-instance' name='p' nsURI='urn:p'><eClassifiers xsi:type='ecore:EClass' name='C'><eOperations name='Op'><eParameters name='p' eType='ecore:EDataType http://www.eclipse.org/emf/2002/Ecore#//EString'/></eOperations></eClassifiers></ecore:EPackage>").unwrap();
        let original = schema.resolved_operations("C").unwrap();
        assert_eq!(
            original[0].signature.parameters,
            vec![crate::definitions::operations::ParameterType::Builtin {
                name: "EString".into()
            }]
        );
        let source = DefinitionSource::new("memory:x", SourceKind::Ecore, "<a/>").id;
        // Relative to the void control: two parameter-set nodes256, two EString
        // copies142 and one parameter slot32 add430. Five builtin metadata
        // nodes add668 to the signature copy. Two visits plus five nodes add7 work.
        assert_lowering_receipt(
            &original,
            3276,
            34,
            &[
                97, 194, 226, 355, 452, 484, 612, 644, 772, 900, 971, 1042, 1074, 1140, 2470, 2598,
                2630, 2662, 2792, 2889, 2986, 3018, 3147, 3244, 3276,
            ],
            |probe| {
                let context = InvocationContext::new(probe.clone());
                let observation = ParserObservation::new(&context);
                observation.finish(observation.preparation(&source, |context| {
                    schema.resolve_native_operation_groups("C", context)
                }))
            },
        );
    }

    #[test]
    fn shared_lowerer_guards_parent_feature_and_operation_publication() {
        const OPERATION: &str = "<eOperations name='Op' eType='#//A'><eParameters name='n' eType='ecore:EDataType http://www.eclipse.org/emf/2002/Ecore#//EString'/></eOperations>";
        let source = DefinitionSource::new(
            "memory:x",
            SourceKind::Ecore,
            format!(
                "<ecore:EPackage xmlns:ecore='http://www.eclipse.org/emf/2002/Ecore' xmlns:xsi='http://www.w3.org/2001/XMLSchema-instance' name='p' nsURI='urn:p'><eClassifiers xsi:type='ecore:EClass' name='A'/><eClassifiers xsi:type='ecore:EClass' name='B'/><eClassifiers xsi:type='ecore:EClass' name='C' eSuperTypes='#//A #//B'><eStructuralFeatures xsi:type='ecore:EReference' name='link' eType='#//A'/>{OPERATION}</eClassifiers></ecore:EPackage>"
            ),
        );
        let document = Document::parse(source).unwrap();
        let root = Xml::from_node(&document.root, &document.source.text).unwrap();
        let policy = || SchemaLoweringPolicy {
            validate_projected_operations: true,
            move_feature_declarations: false,
            constructor_contracts: vec![],
            reference_top: None,
            attribute_domain_defaults: BTreeMap::new(),
        };
        let run = |probe: &Rc<Probe>| {
            let context = InvocationContext::new(probe.clone());
            let observation = ParserObservation::new(&context);
            observation.finish(lower_schema_output_with_datatypes(
                &mut root.clone(),
                "r".into(),
                BTreeMap::new(),
                BTreeMap::new(),
                None,
                policy(),
                &observation,
            ))
        };
        let context = InvocationContext::default();
        let observation = ParserObservation::new(&context);
        let expected = lower_schema_output_with_datatypes(
            &mut root.clone(),
            "r".into(),
            BTreeMap::new(),
            BTreeMap::new(),
            None,
            policy(),
            &observation,
        )
        .unwrap();
        let class = &expected.classes["C"];
        assert_eq!(class.parents, ["A", "B"]);
        assert_eq!(class.feature_order, ["link"]);
        assert_eq!(class.feature_declarations["link"].path, [2, 0]);
        assert_eq!(class.features["link"].target, "A");
        assert_eq!(class.operations[0].declaration.path, [2, 1]);
        assert_eq!(
            class.operations[0].parameters[0].declaration.path,
            [2, 1, 0]
        );
        assert_eq!(class.operations[0].source_payload, OPERATION);
        assert_eq!(expected.revision, "r");
        // Root134, two empty classes258 each, C key/name/parents/slots452,
        // feature strings/order/declaration/maps2198, operation3669+payload,
        // outer operation slot32:7001+payload. Work13+24+43+payload quantum.
        let bytes = 7001 + OPERATION.len();
        let work = 80 + OPERATION.len().div_ceil(256);
        assert_lowering_receipt(
            &expected,
            bytes,
            work,
            &[
                65,
                134,
                199,
                264,
                392,
                457,
                522,
                650,
                715,
                780,
                845,
                877,
                942,
                974,
                1042,
                1107,
                1175,
                1207,
                1275,
                2848,
                2976,
                3044,
                3172,
                3237,
                4970,
                5002,
                5073,
                5139,
                6712,
                6777,
                6841 + OPERATION.len(),
                6873 + OPERATION.len(),
                bytes,
            ],
            run,
        );
    }

    #[test]
    fn feature_lowering_guards_reference_strings_and_owned_domain_defaults() {
        let field = |payload: &str| {
            let source = DefinitionSource::new(
                "memory:x",
                SourceKind::Ecore,
                format!(
                    "<ecore:EPackage xmlns:ecore='http://www.eclipse.org/emf/2002/Ecore' xmlns:xsi='http://www.w3.org/2001/XMLSchema-instance' name='p' nsURI='urn:p'><eClassifiers xsi:type='ecore:EClass' name='C'>{payload}</eClassifiers></ecore:EPackage>"
                ),
            );
            let document = Document::parse(source).unwrap();
            Xml::from_node(&document.root, &document.source.text)
                .unwrap()
                .children
                .remove(0)
                .children
                .remove(0)
        };
        let reference = field(
            "<eStructuralFeatures xsi:type='ecore:EReference' name='next' eType='#//C' eOpposite='#//C/back' lowerBound='0' upperBound='1'/>",
        );
        let empty = BTreeMap::new();
        let datatypes = BTreeMap::new();
        let expected = lower_feature(&reference, &empty, &datatypes, None, None, None).unwrap();
        assert_eq!(expected.name, "next");
        assert_eq!(expected.target, "C");
        assert_eq!(expected.opposite.as_deref(), Some("C/back"));
        assert!(expected.reference);
        assert_eq!(expected.default, Value::Null);
        // next68 + C65 + C/back70:203 bytes and three string work units.
        assert_lowering_receipt(&expected, 203, 3, &[68, 133, 203], |probe| {
            let context = InvocationContext::new(probe.clone());
            let observation = ParserObservation::new(&context);
            observation.finish(lower_feature(
                &reference,
                &empty,
                &datatypes,
                None,
                None,
                Some(&observation),
            ))
        });

        let attribute = field(
            "<eStructuralFeatures xsi:type='ecore:EAttribute' name='value' eType='ecore:EDataType http://www.eclipse.org/emf/2002/Ecore#//EString'/>",
        );
        let default = Value::String("λ".repeat(257));
        let expected =
            lower_feature(&attribute, &empty, &datatypes, None, Some(&default), None).unwrap();
        assert_eq!(expected.name, "value");
        assert_eq!(expected.target, "EString");
        assert_eq!(expected.default, default);
        assert!(!expected.reference);
        // value69 + EString71 + default128 node/2 metadata/514 UTF8:784.
        // Two string copies plus one metadata visit/three copy units:6 work.
        assert_lowering_receipt(&expected, 784, 6, &[69, 140, 784], |probe| {
            let context = InvocationContext::new(probe.clone());
            let observation = ParserObservation::new(&context);
            observation.finish(lower_feature(
                &attribute,
                &empty,
                &datatypes,
                None,
                Some(&default),
                Some(&observation),
            ))
        });
        assert_eq!(default, Value::String("λ".repeat(257)));
    }

    #[test]
    fn operation_lowering_guards_parameter_and_result_ownership() {
        const PAYLOAD: &str = "<eOperations name='Op' eType='#//C'><eParameters name='n' eType='ecore:EDataType http://www.eclipse.org/emf/2002/Ecore#//EString'/></eOperations>";
        let source = DefinitionSource::new(
            "memory:x",
            SourceKind::Ecore,
            format!(
                "<ecore:EPackage xmlns:ecore='http://www.eclipse.org/emf/2002/Ecore' xmlns:xsi='http://www.w3.org/2001/XMLSchema-instance' name='p' nsURI='urn:p'><eClassifiers xsi:type='ecore:EClass' name='C'>{PAYLOAD}</eClassifiers></ecore:EPackage>"
            ),
        );
        let document = Document::parse(source).unwrap();
        let root = Xml::from_node(&document.root, &document.source.text).unwrap();
        let node = &root.children[0].children[0];
        let enums = BTreeMap::new();
        let datatypes = BTreeMap::new();
        let expected = lower_operation(node, &enums, &datatypes, None).unwrap();
        assert_eq!(expected.name, "Op");
        assert_eq!(expected.declaration.path, [0, 0]);
        assert_eq!(expected.parameters[0].name, "n");
        assert_eq!(expected.parameters[0].declaration.path, [0, 0, 0]);
        assert_eq!(
            expected.parameters[0].value.target,
            Some(crate::definitions::operations::TypeTarget::Builtin(
                "EString".into()
            ))
        );
        assert_eq!(
            expected.result.target,
            Some(crate::definitions::operations::TypeTarget::Class(
                "C".into()
            ))
        );
        assert_eq!(expected.source_payload, PAYLOAD);
        // Parameter name65, ID1733 (12 nodes/197 metadata), vector32,
        // target71; operation name66, ID1573, result65, source64+UTF8.
        // Child1/name1/ID19/target1/name1/ID18/result1=42 before payload copy.
        let bytes = 3669 + PAYLOAD.len();
        let work = 42 + PAYLOAD.len().div_ceil(256);
        assert_lowering_receipt(
            &expected,
            bytes,
            work,
            &[65, 1798, 1830, 1901, 1967, 3540, 3605, bytes],
            |probe| {
                let context = InvocationContext::new(probe.clone());
                let observation = ParserObservation::new(&context);
                observation.finish(lower_operation(
                    node,
                    &enums,
                    &datatypes,
                    Some(&observation),
                ))
            },
        );
    }

    #[test]
    fn declared_erasure_admits_exact_raw_identity_and_cumulative_parameter_slots() {
        let schema = inspect_ecore_schema("<ecore:EPackage xmlns:ecore='http://www.eclipse.org/emf/2002/Ecore' xmlns:xsi='http://www.w3.org/2001/XMLSchema-instance' name='p' nsURI='urn:p'><eClassifiers xsi:type='ecore:EClass' name='C'><eOperations name='Op'><eParameters name='p' eType='#//C'/></eOperations></eClassifiers></ecore:EPackage>").unwrap();
        let operation = &schema.classes["C"].operations[0];
        let source = DefinitionSource::new("memory:x", SourceKind::Ecore, "<a/>").id;
        let declaration = crate::definitions::resources::DeclarationId {
            source: source.clone(),
            path: vec![0],
        };
        let raw = crate::definitions::generic_resolution::RawSignature {
            owner: operation.parameters[0].declaration.clone(),
            feature: "eType".into(),
            legacy: None,
            generic: vec![],
            raw_types: vec![declaration.clone()],
        };
        let expected = crate::definitions::operations::Signature {
            name: "Op".into(),
            parameters: vec![crate::definitions::operations::ParameterType::Classifier {
                declaration: declaration.clone(),
            }],
        };
        // Declaration metadata1413/10 nodes, slot32, name64+2:1511/11.
        // Preparation observes metadata visits, without parser copy units.
        assert_lowering_receipt(&expected, 1511, 11, &[1413, 1445, 1511], |probe| {
            let context = InvocationContext::new(probe.clone());
            let observation = ParserObservation::new(&context);
            observation.finish(observation.preparation(&source, |context| {
                crate::definitions::declared_operations::erased_signature_observed(
                    operation,
                    std::slice::from_ref(&raw),
                    context,
                )
            }))
        });
        let mut two = operation.clone();
        let mut second = two.parameters[0].clone();
        second.name = "q".into();
        second.declaration.path = vec![0, 0, 1];
        let mut second_raw = raw.clone();
        second_raw.owner = second.declaration.clone();
        two.parameters.push(second);
        let parameters = [raw.clone(), second_raw];
        let mut two_expected = expected.clone();
        two_expected.parameters.push(expected.parameters[0].clone());
        assert_lowering_receipt(
            &two_expected,
            2956,
            22,
            &[1413, 1445, 2858, 2890, 2956],
            |probe| {
                let context = InvocationContext::new(probe.clone());
                let observation = ParserObservation::new(&context);
                observation.finish(observation.preparation(&source, |context| {
                    crate::definitions::declared_operations::erased_signature_observed(
                        &two,
                        &parameters,
                        context,
                    )
                }))
            },
        );
        assert_eq!(
            crate::definitions::declared_operations::erased_signature(operation, &[]),
            Err("Declared operation parameter count differs from projection".into())
        );
        for invalid in [
            crate::definitions::generic_resolution::RawSignature {
                feature: "eExceptions".into(),
                ..raw.clone()
            },
            crate::definitions::generic_resolution::RawSignature {
                raw_types: vec![],
                ..raw.clone()
            },
            crate::definitions::generic_resolution::RawSignature {
                owner: declaration,
                ..raw
            },
        ] {
            assert_eq!(
                crate::definitions::declared_operations::erased_signature_observed(
                    operation,
                    &[invalid],
                    &InvocationContext::default()
                ),
                Err(PreparationError::Schema(
                    "Invalid declared operation parameter provenance".into()
                ))
            );
        }
        assert_eq!(
            crate::definitions::declared_operations::erased_signature(operation, &parameters[..1]),
            Ok(expected)
        );
    }

    #[test]
    fn operation_template_projection_admits_each_exact_expression_copy() {
        let schema = inspect_ecore_schema("<ecore:EPackage xmlns:ecore='http://www.eclipse.org/emf/2002/Ecore' xmlns:xsi='http://www.w3.org/2001/XMLSchema-instance' name='p' nsURI='urn:p'><eClassifiers xsi:type='ecore:EClass' name='C'><eOperations name='Op'><eParameters name='p' eType='#//C'/></eOperations></eClassifiers></ecore:EPackage>").unwrap();
        let operation = &schema.classes["C"].operations[0];
        let source = DefinitionSource::new("memory:x", SourceKind::Ecore, "<a/>").id;
        let declaration = crate::definitions::resources::DeclarationId {
            source: source.clone(),
            path: vec![0],
        };
        let expression = crate::definitions::generic_types::Type::Parameter { declaration };
        let signature = |owner| crate::definitions::generic_resolution::SpecializedSignature {
            owner,
            feature: "eType".into(),
            declared: vec![],
            specialized: vec![expression.clone()],
        };
        let member = crate::definitions::generic_resolution::SpecializedMember {
            declaration: operation.declaration.clone(),
            owner: expression.clone(),
            name: "Op".into(),
            span: crate::definitions::bundle::SourceSpan {
                source: source.clone(),
                start: 0,
                end: 4,
            },
            source_payload: String::new(),
            formals: vec![],
            signatures: vec![
                signature(operation.declaration.clone()),
                signature(operation.parameters[0].declaration.clone()),
            ],
        };
        let mut expected = operation.clone();
        expected.result.target = Some(crate::definitions::operations::TypeTarget::Expression(
            expression.clone(),
        ));
        expected.parameters[0].value.target = Some(
            crate::definitions::operations::TypeTarget::Expression(expression),
        );
        expected.generic = true;
        // Each Parameter expression: four outer nodes540 + declaration1413:
        // 1953 bytes/14 visits. Two scans of two signatures + parameter visit:5.
        assert_lowering_receipt(&expected, 3906, 33, &[1953, 3906], |probe| {
            let mut result = operation.clone();
            let context = InvocationContext::new(probe.clone());
            let observation = ParserObservation::new(&context);
            observation.finish(observation.preparation(&source, |context| {
                crate::definitions::operation_templates::project_observed(
                    &mut result,
                    &member,
                    context,
                )
            }))?;
            Ok(result)
        });
        let mut compatibility = operation.clone();
        crate::definitions::operation_templates::project(&mut compatibility, &member).unwrap();
        assert_eq!(compatibility, expected);
        for (signatures, message) in [
            (vec![], "Missing operation template type signature"),
            (
                vec![member.signatures[0].clone(), member.signatures[0].clone()],
                "Ambiguous operation template type signature",
            ),
            (
                vec![
                    crate::definitions::generic_resolution::SpecializedSignature {
                        specialized: vec![member.owner.clone(), member.owner.clone()],
                        ..member.signatures[0].clone()
                    },
                ],
                "Ambiguous operation template type signature",
            ),
        ] {
            let invalid = crate::definitions::generic_resolution::SpecializedMember {
                signatures,
                ..member.clone()
            };
            assert_eq!(
                crate::definitions::operation_templates::project_observed(
                    &mut operation.clone(),
                    &invalid,
                    &InvocationContext::default()
                ),
                Err(PreparationError::Schema(message.into()))
            );
        }
        assert!(!operation.generic);
        assert!(operation.result.target.is_none());
    }

    #[test]
    fn declared_operation_group_validation_guards_maps_selection_and_result_copies() {
        use crate::definitions::operations::{
            DeclaredOperation, DeclaredOperationGroup, DeclaredOperationSet, Operation, Signature,
            TypedElement,
        };
        let mut schema = inspect_ecore_schema("<ecore:EPackage xmlns:ecore='http://www.eclipse.org/emf/2002/Ecore' xmlns:xsi='http://www.w3.org/2001/XMLSchema-instance' name='p' nsURI='urn:p'><eClassifiers xsi:type='ecore:EClass' name='C'/></ecore:EPackage>").unwrap();
        let source = DefinitionSource::new("memory:x", SourceKind::Ecore, "<a/>").id;
        schema.declarations.insert(
            "C".into(),
            crate::definitions::schema::ClassifierDeclaration {
                source: source.clone(),
                path: vec![0],
                namespace: "urn:p".into(),
                name: "C".into(),
            },
        );
        let receiver = crate::definitions::generic_types::RuntimeType::new(
            crate::definitions::generic_types::Type::Classifier {
                declaration: crate::definitions::resources::DeclarationId {
                    source: source.clone(),
                    path: vec![0],
                },
                arguments: vec![],
                raw: false,
            },
        )
        .unwrap();
        schema.generic_signatures =
            Some(crate::definitions::generic_resolution::SignatureInventory {
                sources: vec![],
                parameters: vec![],
                types: vec![],
                raw_signatures: vec![],
                retained_payloads: vec![],
            });
        // Classifier metadata2512/18 nodes, validation state128,
        // two C key copies130, nine advances.
        assert_lowering_receipt(&"C", 2770, 27, &[2512, 2640, 2705, 2770], |probe| {
            let context = InvocationContext::new(probe.clone());
            let observation = ParserObservation::new(&context);
            observation.finish(observation.preparation(&source, |context| {
                schema.class_key_for_type_observed(&receiver, context)
            }))
        });
        let mut selections = BTreeMap::from([(
            "C".into(),
            DeclaredOperationSet {
                receiver,
                groups: vec![],
            },
        )]);
        let run_empty = |probe: &Rc<Probe>| {
            let context = InvocationContext::new(probe.clone());
            let observation = ParserObservation::new(&context);
            observation.finish(observation.preparation(&source, |context| {
                schema.checked_declared_operations_observed("C", &selections, false, context)
            }))
        };
        // Receiver2770/27, ancestry484/8, one inventory-key visit:3254/36.
        assert_lowering_receipt(
            &vec![],
            3254,
            36,
            &[2512, 2640, 2705, 2770, 2867, 2964, 2996, 3125, 3222, 3254],
            run_empty,
        );
        let declaration = crate::definitions::resources::DeclarationId {
            source: source.clone(),
            path: vec![0, 0],
        };
        let operation = Operation {
            name: "Op".into(),
            declaration: declaration.clone(),
            parameters: vec![],
            result: TypedElement {
                target: None,
                lower: 0,
                upper: 1,
                ordered: true,
                unique: true,
            },
            source_payload: String::new(),
            generic: false,
        };
        schema
            .classes
            .get_mut("C")
            .unwrap()
            .operations
            .push(operation.clone());
        let signature = Signature {
            name: "Op".into(),
            parameters: vec![],
        };
        selections
            .get_mut("C")
            .unwrap()
            .groups
            .push(DeclaredOperationGroup {
                signature,
                selected: declaration.clone(),
                selected_owners: vec![],
                operations: vec![DeclaredOperation {
                    projection: None,
                    template: None,
                    owner: "C".into(),
                    operation,
                    raw_parameters: vec![],
                }],
            });
        schema.declared_operation_selections = Some(selections.clone());
        let original = schema.resolved_operations("C").unwrap();
        assert_eq!(original[0].selected.operation.declaration, declaration);
        assert_eq!(original[0].signature.name, "Op");
        assert!(original[0].overridden.is_empty());
        // Receiver2770, inventory644, four map/set nodes512, operation4646/33
        // nodes (422 metadata bytes), three name copies198, candidate/winner/
        // result slots96, ancestry484/7 (no inventory owner visit),
        // signature662/5 nodes:10012/91.
        assert_lowering_receipt(
            &original,
            10012,
            91,
            &[
                2512, 2640, 2705, 2770, 2867, 2964, 2996, 3125, 3222, 3254, 3382, 3414, 3542, 3670,
                3798, 3926, 8572, 8638, 8704, 8736, 8833, 8930, 8962, 9091, 9188, 9220, 9252, 9318,
                9980, 10012,
            ],
            |probe| {
                let context = InvocationContext::new(probe.clone());
                let observation = ParserObservation::new(&context);
                observation.finish(observation.preparation(&source, |context| {
                    schema.checked_declared_operations_observed("C", &selections, true, context)
                }))
            },
        );
        let mut missing = selections.clone();
        missing.clear();
        assert_eq!(
            schema.checked_declared_operations_observed(
                "C",
                &missing,
                false,
                &InvocationContext::default()
            ),
            Err(PreparationError::Schema(
                "Declared operation receiver inventory differs from schema classes".into()
            ))
        );
        let mut duplicate = selections.clone();
        let group = duplicate["C"].groups[0].clone();
        duplicate.get_mut("C").unwrap().groups.push(group);
        assert_eq!(
            schema.checked_declared_operations_observed(
                "C",
                &duplicate,
                false,
                &InvocationContext::default()
            ),
            Err(PreparationError::Schema(
                "Duplicate declared operation group signature".into()
            ))
        );
        let mut forged = selections.clone();
        forged.get_mut("C").unwrap().groups[0].selected.path = vec![0, 9];
        assert_eq!(
            schema.checked_declared_operations_observed(
                "C",
                &forged,
                false,
                &InvocationContext::default()
            ),
            Err(PreparationError::Schema(
                "Declared operation winner does not dominate its group".into()
            ))
        );
        assert_eq!(schema.resolved_operations("C").unwrap(), original);
    }

    #[test]
    fn borrowed_runtime_template_classification_observes_shapes_without_identity_copies() {
        use crate::definitions::{
            generic_resolution::{SpecializedMember, SpecializedSignature},
            generic_types::{RuntimeType, Type},
            resources::DeclarationId,
        };
        let source = DefinitionSource::new("memory:x", SourceKind::Ecore, "<a/>").id;
        let declaration = DeclarationId {
            source: source.clone(),
            path: vec![0],
        };
        let leaf = Type::Classifier {
            declaration: declaration.clone(),
            arguments: vec![],
            raw: false,
        };
        let make_member = |expression: Type| SpecializedMember {
            declaration: declaration.clone(),
            owner: leaf.clone(),
            name: "Op".into(),
            span: crate::definitions::bundle::SourceSpan {
                source: source.clone(),
                start: 0,
                end: 4,
            },
            source_payload: String::new(),
            formals: vec![],
            signatures: vec![SpecializedSignature {
                owner: declaration.clone(),
                feature: "eType".into(),
                declared: vec![],
                specialized: vec![expression],
            }],
        };
        let nested = Type::Classifier {
            declaration: declaration.clone(),
            arguments: vec![leaf.clone()],
            raw: false,
        };
        let mut wide_id = declaration.clone();
        wide_id.source.uri = "é".repeat(257);
        let mut invalid_id = declaration.clone();
        invalid_id.source.sha256 = "A".repeat(64);
        for (expression, expected, bytes, work, prefixes) in [
            // One shape frame32 + canonical frame32; two visits, trim1,
            // URI8 characters and SHA64 bytes:75. Classification adds3 visits.
            (leaf.clone(), false, 64, 78, vec![32, 64]),
            // Two shape/canonical frames each, two enqueue visits and two
            // canonical declarations:152, plus classification3.
            (nested, false, 128, 155, vec![32, 64, 96, 128]),
            (
                Type::Parameter {
                    declaration: declaration.clone(),
                },
                true,
                64,
                5,
                vec![32, 64],
            ),
            (
                Type::Classifier {
                    declaration: declaration.clone(),
                    raw: true,
                    arguments: vec![leaf.clone()],
                },
                true,
                32,
                4,
                vec![32],
            ),
            (
                Type::Classifier {
                    declaration: wide_id,
                    arguments: vec![],
                    raw: false,
                },
                false,
                64,
                329,
                vec![32, 64],
            ),
            (
                Type::Classifier {
                    declaration: invalid_id,
                    arguments: vec![],
                    raw: false,
                },
                true,
                64,
                15,
                vec![32, 64],
            ),
        ] {
            let member = make_member(expression.clone());
            let original = member.clone();
            assert_lowering_receipt(&expected, bytes, work, &prefixes, |probe| {
                let context = InvocationContext::new(probe.clone());
                let observation = ParserObservation::new(&context);
                observation.finish(observation.preparation(&source, |context| {
                    member.requires_runtime_template_observed(context)
                }))
            });
            assert_eq!(member.requires_runtime_template(), expected);
            assert_eq!(RuntimeType::new(expression).is_err(), expected);
            assert_eq!(member, original);
        }
        // Python hashlib independently pins the unchanged 179-byte compact JSON
        // expression and the existing intersect.generic.type.v1\0 hash prefix.
        assert_eq!(
            RuntimeType::new(leaf.clone()).unwrap().identity(),
            "intersect:generic:1:17073fefa1ff4e7fdc834e8ebf13ee9f95f7147b1c1669fc4475a3a84f307d3d"
        );
        let mut parameter = make_member(leaf.clone());
        parameter
            .formals
            .push(crate::definitions::generic_resolution::SpecializedFormal {
                declaration: crate::definitions::generics::TypeParameter {
                    declaration: declaration.clone(),
                    span: parameter.span.clone(),
                    source_payload: String::new(),
                    name: "U".into(),
                    bounds: vec![],
                },
                bounds: vec![],
            });
        assert_lowering_receipt(&true, 0, 1, &[], |probe| {
            let context = InvocationContext::new(probe.clone());
            let observation = ParserObservation::new(&context);
            observation.finish(observation.preparation(&source, |context| {
                parameter.requires_runtime_template_observed(context)
            }))
        });
        let mut plain = parameter.clone();
        plain.formals.clear();
        plain.signatures.clear();
        let top = crate::definitions::schema::eobject_reference_top_declaration().unwrap();
        let mut reflected = plain.clone();
        reflected.declaration = top;
        reflected.declaration.path.push(0);
        for (member, expected) in [(plain, false), (reflected, true)] {
            assert_lowering_receipt(&expected, 0, 2, &[], |probe| {
                let context = InvocationContext::new(probe.clone());
                let observation = ParserObservation::new(&context);
                observation.finish(observation.preparation(&source, |context| {
                    member.requires_schema_template_observed(context)
                }))
            });
            assert_eq!(member.requires_schema_template().unwrap(), expected);
        }
        assert_eq!(
            RuntimeType::new(Type::Parameter { declaration }).unwrap_err(),
            "Runtime classifier identity cannot contain parameter or wildcard types"
        );
    }

    #[test]
    fn streamed_runtime_identity_preserves_exact_bytes_and_typed_interruptions() {
        use crate::definitions::{
            generic_types::{RuntimeType, Type},
            resources::DeclarationId,
        };
        let source = DefinitionSource::new("memory:x", SourceKind::Ecore, "<a/>").id;
        let expression = Type::Classifier {
            declaration: DeclarationId {
                source: source.clone(),
                path: vec![0],
            },
            arguments: vec![],
            raw: false,
        };
        let original = expression.clone();
        let expected = RuntimeType::new(expression.clone()).unwrap();
        // Independent literal wire: 179 bytes, previously pinned with Python
        // hashlib. Serde emits 58 fragments: 11 keys/string values use three
        // fragments each; punctuation/containers/scalar atoms contribute25.
        // Validation75 + prefix1 + fragments58 + finalization1 =135 work.
        // Shape/canonical frames64 + hash state128 + identity string148 =340.
        assert_lowering_receipt(&expected, 340, 135, &[32, 64, 192, 340], |probe| {
            let context = InvocationContext::new(probe.clone());
            let observation = ParserObservation::new(&context);
            observation.finish(observation.preparation(&source, |context| {
                RuntimeType::new_observed(expression.clone(), context)
            }))
        });
        assert_eq!(expression, original);
        assert_eq!(
            expected.identity(),
            "intersect:generic:1:17073fefa1ff4e7fdc834e8ebf13ee9f95f7147b1c1669fc4475a3a84f307d3d"
        );
        let Type::Classifier { declaration, .. } = &expression else {
            unreachable!()
        };
        let nested = Type::Classifier {
            declaration: declaration.clone(),
            arguments: vec![expression.clone()],
            raw: false,
        };
        let raw = Type::Classifier {
            declaration: declaration.clone(),
            arguments: vec![],
            raw: true,
        };
        let mut wide_id = declaration.clone();
        wide_id.source.uri = "é".repeat(257);
        let wide = Type::Classifier {
            declaration: wide_id,
            arguments: vec![],
            raw: false,
        };
        // Nested JSON358 emits116 fragments; wide JSON685 emits58 fragments
        // with one514-byte fragment charged3. Independent Python digest pins
        // also retain raw178-byte and UTF-8 wire compatibility.
        for (value, bytes, work, prefixes, digest) in [
            (
                nested,
                404,
                270,
                vec![32, 64, 96, 128, 256, 404],
                "1b0a3f7515abf5939d7eb076df59bbdae1d4652d91f19a1d454dae55077b194d",
            ),
            (
                raw,
                340,
                135,
                vec![32, 64, 192, 340],
                "c823f3c9ada4c177ff3380e78339714f5900e36d1663b0283a3c471f40c4d3cb",
            ),
            (
                wide,
                340,
                388,
                vec![32, 64, 192, 340],
                "9b766f0aedc811112d0cbadd3fe17ebeae9f4fd09cfa187c19254f59d5010987",
            ),
        ] {
            let expected = RuntimeType::new(value.clone()).unwrap();
            assert_eq!(expected.identity(), format!("intersect:generic:1:{digest}"));
            assert_lowering_receipt(&expected, bytes, work, &prefixes, |probe| {
                let context = InvocationContext::new(probe.clone());
                let observation = ParserObservation::new(&context);
                observation.finish(observation.preparation(&source, |context| {
                    RuntimeType::new_observed(value.clone(), context)
                }))
            });
        }
        let parameter = Type::Parameter {
            declaration: DeclarationId {
                source: source.clone(),
                path: vec![0],
            },
        };
        let probe = Probe::with_work(64, 2, None);
        let context = InvocationContext::new(probe.clone());
        assert_eq!(
            RuntimeType::new_observed(parameter, &context).unwrap_err(),
            crate::definitions::schema::PreparationError::Schema(
                "Runtime classifier identity cannot contain parameter or wildcard types".into()
            )
        );
        assert_eq!((probe.storage.get(), probe.work.get()), (64, 2));
    }

    #[test]
    fn raw_runtime_traversal_and_classifier_copies_have_independent_receipts() {
        use crate::definitions::{
            generic_types::{RuntimeType, Type},
            resources::DeclarationId,
        };
        let source = DefinitionSource::new("memory:x", SourceKind::Ecore, "<a/>").id;
        let declaration = DeclarationId {
            source: source.clone(),
            path: vec![0],
        };
        let leaf = Type::Classifier {
            declaration: declaration.clone(),
            arguments: vec![],
            raw: false,
        };
        let raw = Type::Classifier {
            declaration: declaration.clone(),
            arguments: vec![],
            raw: true,
        };
        for (expression, expected, bytes, work, prefixes) in [
            (leaf.clone(), false, 32, 1, vec![32]),
            (raw.clone(), true, 32, 1, vec![32]),
            (
                Type::Classifier {
                    declaration: declaration.clone(),
                    arguments: vec![leaf.clone()],
                    raw: false,
                },
                false,
                64,
                3,
                vec![32, 64],
            ),
            (
                Type::Classifier {
                    declaration: declaration.clone(),
                    arguments: vec![raw.clone()],
                    raw: false,
                },
                true,
                64,
                3,
                vec![32, 64],
            ),
            // Existing DFS order visits the last argument before the raw first
            // argument: three frame admissions, two enqueues, three visits.
            (
                Type::Classifier {
                    declaration: declaration.clone(),
                    arguments: vec![raw, leaf.clone()],
                    raw: false,
                },
                true,
                96,
                5,
                vec![32, 64, 96],
            ),
        ] {
            let runtime = RuntimeType::new(expression).unwrap();
            let original = runtime.clone();
            assert_lowering_receipt(&expected, bytes, work, &prefixes, |probe| {
                let context = InvocationContext::new(probe.clone());
                let observation = ParserObservation::new(&context);
                observation.finish(
                    observation.preparation(&source, |context| runtime.has_raw_observed(context)),
                )
            });
            assert_eq!(runtime.has_raw(), expected);
            assert_eq!(runtime, original);
        }
        let node = Xml {
            name: "eClassifiers".into(),
            attrs: BTreeMap::new(),
            attr_spans: BTreeMap::new(),
            children: vec![],
            opening: crate::definitions::bundle::SourceSpan {
                source: source.clone(),
                start: 0,
                end: 4,
            },
            payload: None,
            declaration: declaration.clone(),
            source_payload: String::new(),
        };
        let expected = RuntimeType::new(leaf).unwrap();
        // Declaration copy:10 metadata visits + six256-byte copy units,
        // 1413 bytes; identity constructor340/135 adds the remaining prefixes.
        assert_lowering_receipt(
            &expected,
            1753,
            151,
            &[1413, 1445, 1477, 1605, 1753],
            |probe| {
                let context = InvocationContext::new(probe.clone());
                let observation = ParserObservation::new(&context);
                observation.finish(materialization::classifier_type(
                    &declaration,
                    false,
                    &node,
                    &observation,
                ))
            },
        );
        assert_eq!(node.declaration, declaration);
    }

    #[test]
    fn attribute_candidate_publication_preserves_sorted_unique_required_entries() {
        use crate::definitions::{
            generic_types::{RuntimeType, Type},
            resources::DeclarationId,
        };
        let source = DefinitionSource::new("memory:x", SourceKind::Ecore, "<a/>").id;
        let declaration = DeclarationId {
            source: source.clone(),
            path: vec![0],
        };
        let leaf = RuntimeType::new(Type::Classifier {
            declaration: declaration.clone(),
            arguments: vec![],
            raw: false,
        })
        .unwrap();
        let raw = RuntimeType::new(Type::Classifier {
            declaration,
            arguments: vec![],
            raw: true,
        })
        .unwrap();
        // Literal v1 hashes17073... and c823... order the closed entry first.
        let expected = vec![(leaf.clone(), true), (raw.clone(), false)];
        // Two map nodes128 and identity strings(64+84), then two slots32:
        // 2*(128+148)+64 =616; three candidates + two output moves =5 work.
        assert_lowering_receipt(
            &expected,
            616,
            5,
            &[128, 276, 404, 552, 584, 616],
            |probe| {
                let context = InvocationContext::new(probe.clone());
                let observation = ParserObservation::new(&context);
                let run = || {
                    let mut candidates = BTreeMap::new();
                    materialization::retain_attribute_candidate(
                        &mut candidates,
                        raw.clone(),
                        false,
                        &source,
                        &observation,
                    )?;
                    materialization::retain_attribute_candidate(
                        &mut candidates,
                        leaf.clone(),
                        false,
                        &source,
                        &observation,
                    )?;
                    materialization::retain_attribute_candidate(
                        &mut candidates,
                        leaf.clone(),
                        true,
                        &source,
                        &observation,
                    )?;
                    materialization::publish_attribute_candidates(candidates, &source, &observation)
                };
                observation.finish(run())
            },
        );
    }

    #[test]
    fn substitution_expansion_observes_owned_copies_without_capturing_actuals() {
        use crate::definitions::{
            generic_types::{ParameterBinding, Substitution, Type},
            resources::DeclarationId,
        };
        let source = DefinitionSource::new("memory:x", SourceKind::Ecore, "<a/>").id;
        let id = |index| DeclarationId {
            source: source.clone(),
            path: vec![index],
        };
        let p = Type::Parameter { declaration: id(0) };
        let q = Type::Parameter { declaration: id(1) };
        let leaf = Type::Classifier {
            declaration: id(2),
            arguments: vec![],
            raw: false,
        };
        let empty = Substitution::new(vec![]).unwrap();
        let nested = Type::Classifier {
            declaration: id(2),
            arguments: vec![leaf.clone()],
            raw: false,
        };
        let wildcard = Type::Wildcard {
            upper: Some(Box::new(leaf.clone())),
            lower: None,
        };
        let chain = Substitution::new(vec![
            ParameterBinding {
                parameter: id(0),
                argument: q.clone(),
            },
            ParameterBinding {
                parameter: id(1),
                argument: leaf.clone(),
            },
        ])
        .unwrap();
        let identity = Substitution::new(vec![ParameterBinding {
            parameter: id(0),
            argument: p.clone(),
        }])
        .unwrap();
        // Declaration metadata1413/10, copied Parameter metadata1953/14.
        // Each classifier header128, argument slot32, active-set node128 and
        // wildcard box128 is separately precharged. Shape frames cost32.
        for (environment, expression, simultaneous, expected, bytes, work, prefixes) in [
            (
                &empty,
                leaf.clone(),
                false,
                leaf.clone(),
                1573,
                12,
                vec![32, 160, 1573],
            ),
            (
                &empty,
                nested.clone(),
                false,
                nested,
                3178,
                26,
                vec![32, 64, 192, 1605, 1637, 1765, 3178],
            ),
            (
                &empty,
                p.clone(),
                false,
                p.clone(),
                1985,
                16,
                vec![32, 1985],
            ),
            (
                &empty,
                Type::Wildcard {
                    upper: None,
                    lower: None,
                },
                false,
                Type::Wildcard {
                    upper: None,
                    lower: None,
                },
                160,
                2,
                vec![32, 160],
            ),
            (
                &empty,
                wildcard.clone(),
                false,
                wildcard,
                1861,
                16,
                vec![32, 64, 192, 320, 448, 1861],
            ),
            (
                &chain,
                p.clone(),
                false,
                leaf,
                4655,
                34,
                vec![32, 160, 1573, 1701, 3114, 3242, 4655],
            ),
            // Actual Q belongs to the caller; one edge must never capture it.
            (&chain, p.clone(), true, q, 1985, 17, vec![32, 1985]),
            (&identity, p.clone(), false, p, 1985, 16, vec![32, 1985]),
        ] {
            let original = expression.clone();
            let bindings = environment.entries();
            assert_lowering_receipt(&expected, bytes, work, &prefixes, |probe| {
                let context = InvocationContext::new(probe.clone());
                let observation = ParserObservation::new(&context);
                observation.finish(observation.preparation(&source, |context| {
                    if simultaneous {
                        environment.apply_simultaneous_observed(&expression, context)
                    } else {
                        environment.apply_observed(&expression, context)
                    }
                }))
            });
            let compatible = if simultaneous {
                environment.apply_simultaneous(&expression)
            } else {
                environment.apply(&expression)
            };
            assert_eq!(compatible.unwrap(), expected);
            assert_eq!(expression, original);
            assert_eq!(environment.entries(), bindings);
        }
    }

    #[test]
    fn substitution_map_admission_and_cyclic_errors_preserve_typed_boundaries() {
        use crate::definitions::{
            generic_types::{ParameterBinding, Substitution, Type},
            resources::DeclarationId,
            schema::PreparationError,
        };
        let source = DefinitionSource::new("memory:x", SourceKind::Ecore, "<a/>").id;
        let id = |index| DeclarationId {
            source: source.clone(),
            path: vec![index],
        };
        let p = Type::Parameter { declaration: id(0) };
        let q = Type::Parameter { declaration: id(1) };
        let leaf = Type::Classifier {
            declaration: id(2),
            arguments: vec![],
            raw: false,
        };
        for (entries, bytes, work, prefixes) in [
            (vec![], 0, 1, vec![]),
            (
                vec![ParameterBinding {
                    parameter: id(0),
                    argument: leaf,
                }],
                160,
                3,
                vec![32, 160],
            ),
        ] {
            let expected = Substitution::new(entries.clone()).unwrap();
            assert_lowering_receipt(&expected, bytes, work, &prefixes, |probe| {
                let context = InvocationContext::new(probe.clone());
                let observation = ParserObservation::new(&context);
                observation.finish(observation.preparation(&source, |context| {
                    Substitution::new_observed(entries.clone(), context)
                }))
            });
        }
        let duplicate = vec![
            ParameterBinding {
                parameter: id(0),
                argument: p.clone(),
            },
            ParameterBinding {
                parameter: id(0),
                argument: p.clone(),
            },
        ];
        let duplicate_message = "Duplicate generic substitution parameter";
        assert_eq!(
            Substitution::new_observed(duplicate.clone(), &InvocationContext::default())
                .unwrap_err(),
            PreparationError::Schema(duplicate_message.into())
        );
        // First shape32/map128, second shape32; no duplicate map node.
        assert_lowering_receipt(
            &Err::<Substitution, _>(duplicate_message.to_owned()),
            192,
            5,
            &[32, 160, 192],
            |probe| {
                let context = InvocationContext::new(probe.clone());
                let observation = ParserObservation::new(&context);
                observation.finish(observation.preparation(&source, |context| {
                    match Substitution::new_observed(duplicate.clone(), context) {
                        Ok(value) => Ok(Ok(value)),
                        Err(PreparationError::Schema(message)) => Ok(Err(message)),
                        Err(error @ PreparationError::Observation(_)) => Err(error),
                    }
                }))
            },
        );
        let cycle = Substitution::new(vec![
            ParameterBinding {
                parameter: id(0),
                argument: q,
            },
            ParameterBinding {
                parameter: id(1),
                argument: p.clone(),
            },
        ])
        .unwrap();
        let message = "Cyclic generic substitution at memory:x [0]";
        assert_eq!(
            cycle
                .apply_observed(&p, &InvocationContext::default())
                .unwrap_err(),
            PreparationError::Schema(message.into())
        );
        // Two active nodes/declaration copies; the third parameter visit rejects
        // before a second P copy. Shape1 + visits3 + metadata20 =24 work.
        assert_lowering_receipt(
            &Err::<Type, _>(message.to_owned()),
            3114,
            24,
            &[32, 160, 1573, 1701, 3114],
            |probe| {
                let context = InvocationContext::new(probe.clone());
                let observation = ParserObservation::new(&context);
                observation.finish(observation.preparation(&source, |context| {
                    match cycle.apply_observed(&p, context) {
                        Ok(value) => Ok(Ok(value)),
                        Err(PreparationError::Schema(message)) => Ok(Err(message)),
                        Err(error @ PreparationError::Observation(_)) => Err(error),
                    }
                }))
            },
        );
    }

    #[test]
    fn resolver_parent_substitution_has_literal_copy_receipts_and_spans() {
        use crate::definitions::{
            bundle::DefinitionBundle, generic_resolution::Resolver, generic_types::Type,
            resources::DeclarationId,
        };
        for (body, generic, parent_raw, bytes, work, prefixes) in [
            (
                "<eClassifiers x:type='e:EClass' name='Base'/><eClassifiers x:type='e:EClass' name='Derived' eSuperTypes='#//Base'/>",
                false,
                false,
                3148,
                45,
                vec![1575, 1607, 3020, 3148],
            ),
            (
                "<eClassifiers x:type='e:EClass' name='Base'><eTypeParameters name='T'/></eClassifiers><eClassifiers x:type='e:EClass' name='Derived' eSuperTypes='#//Base'/>",
                false,
                true,
                11236,
                156,
                vec![
                    1413, 1445, 3018, 4431, 4522, 6095, 7670, 7735, 7863, 7928, 8056, 8088, 9663,
                    9695, 11108, 11236,
                ],
            ),
            (
                "<eClassifiers x:type='e:EClass' name='Base'><eTypeParameters name='T'/></eClassifiers><eClassifiers x:type='e:EClass' name='Derived'><eTypeParameters name='U'/><eGenericSuperTypes eClassifier='#//Base'><eTypeArguments eTypeParameter='#//Derived/U'/></eGenericSuperTypes></eClassifiers><eClassifiers x:type='e:EClass' name='Actual'/>",
                true,
                false,
                15543,
                193,
                vec![
                    1413, 1445, 3018, 4431, 4522, 6095, 7670, 7735, 7863, 7928, 8056, 8088, 10600,
                    10632, 10664, 10792, 10824, 10856, 10984, 12397, 12429, 12557, 13970, 14002,
                    15415, 15543,
                ],
            ),
        ] {
            let source = DefinitionSource::new(
                "memory:x",
                SourceKind::Ecore,
                format!(
                    "<e:EPackage xmlns:e='http://www.eclipse.org/emf/2002/Ecore' xmlns:x='http://www.w3.org/2001/XMLSchema-instance' name='p' nsURI='urn:p'>{body}</e:EPackage>"
                ),
            );
            let document = Document::parse(source.clone()).unwrap();
            let bundle = DefinitionBundle {
                version: 1,
                sources: vec![source.clone()],
                namespaces: vec![],
            };
            let resolver = Resolver::new(&bundle).unwrap();
            let id = |index| DeclarationId {
                source: source.id.clone(),
                path: vec![index],
            };
            let actual = Type::Classifier {
                declaration: id(2),
                arguments: vec![],
                raw: false,
            };
            let arguments = if generic {
                vec![actual.clone()]
            } else {
                vec![]
            };
            let receiver = Type::Classifier {
                declaration: id(1),
                arguments: arguments.clone(),
                raw: false,
            };
            let span = &document.root.children[1].span;
            let location = if generic {
                document.root.children[1].children[1].span.clone()
            } else {
                document.root.children[1].opening.clone()
            };
            let expected = vec![(
                Type::Classifier {
                    declaration: id(0),
                    arguments,
                    raw: parent_raw,
                },
                location,
            )];
            // Raw: opening span1575/18 (11 nodes including struct keys) +
            // result slot32 + declaration1413/16 + set128; five other
            // visits =3148/39. A raw generic parent adds its independent
            // source formal list8088/100 before the opening span:11236/139.
            // Generic: argument2512/28 + slot32 + map160/3 + substitution
            // 3178/27 + result32 + declaration1413/16 + set128, five other
            // visits =7455/79. Source formals add8088/100 plus the nonformal
            // superclass-child visit:15543/180. Legacy resolution remains excluded.
            assert_lowering_receipt(&expected, bytes, work, &prefixes, |probe| {
                let context = InvocationContext::new(probe.clone());
                let observation = ParserObservation::new(&context);
                observation.finish(resolver.direct_parent_types_observed(
                    &receiver,
                    span,
                    &observation,
                ))
            });
            if generic {
                let malformed = Type::Classifier {
                    declaration: id(2),
                    arguments: vec![actual],
                    raw: true,
                };
                let rejected = Type::Classifier {
                    declaration: id(1),
                    arguments: vec![malformed],
                    raw: false,
                };
                let context = InvocationContext::default();
                let observation = ParserObservation::new(&context);
                assert_eq!(
                    observation
                        .finish(resolver.direct_parent_types_observed(
                            &rejected,
                            span,
                            &observation,
                        ))
                        .unwrap_err(),
                    ObservedImportError::Import(ImportError {
                        kind: ErrorKind::InvalidDefinition,
                        span: span.clone(),
                        message: "Raw classifier cannot carry actual arguments".into(),
                    })
                );
            }
        }
    }

    #[test]
    fn observed_generic_ancestry_preserves_literal_paths_cache_and_cycle_errors() {
        use crate::definitions::{
            bundle::DefinitionBundle,
            generic_resolution::{InheritedType, Resolver},
            generic_types::Type,
            resources::DeclarationId,
        };
        for (body, selected, paths, bytes, work, prefixes) in [
            (
                "<eClassifiers x:type='e:EClass' name='A'/>",
                0,
                vec![],
                5758,
                77,
                vec![1413, 2826, 2858, 2988, 5500, 5630, 5758],
            ),
            (
                "<eClassifiers x:type='e:EClass' name='A'/><eClassifiers x:type='e:EClass' name='B' eSuperTypes='#//A'/>",
                1,
                vec![vec![1, 0]],
                25476,
                314,
                vec![
                    1413, 2826, 2858, 4433, 4465, 5878, 6006, 7419, 8832, 8864, 11376, 14332,
                    14492, 14622, 17134, 17264, 17392, 17424, 20066, 20194, 22706, 25348, 25476,
                ],
            ),
            (
                "<eClassifiers x:type='e:EClass' name='A'/><eClassifiers x:type='e:EClass' name='B' eSuperTypes='#//A'/><eClassifiers x:type='e:EClass' name='C' eSuperTypes='#//A'/><eClassifiers x:type='e:EClass' name='D' eSuperTypes='#//B #//C'/>",
                3,
                vec![vec![3, 1], vec![3, 1, 0], vec![3, 2]],
                120980,
                1546,
                vec![
                    1413, 2826, 2858, 4433, 4465, 6040, 6072, 7485, 7613, 9026, 9154, 10567, 11980,
                    12012, 14524, 17480, 17640, 19215, 19247, 20660, 20788, 22201, 23614, 23646,
                    26158, 30527, 30687, 30817, 33329, 33459, 33587, 33619, 36261, 36389, 38901,
                    41543, 41671, 41703, 41735, 43148, 44561, 44593, 47105, 50061, 50221, 51796,
                    51828, 53241, 53369, 54782, 54912, 54944, 57586, 57714, 60226, 62868, 62996,
                    63028, 63060, 73238, 75750, 78262, 79675, 81088, 82501, 85013, 87525, 88938,
                    90351, 91764, 94276, 96788, 98201, 99614, 101027, 103539, 106051, 107464,
                    108877, 110290, 110418, 110546, 110674, 113186, 120852, 120980,
                ],
            ),
        ] {
            let source = DefinitionSource::new(
                "memory:x",
                SourceKind::Ecore,
                format!(
                    "<e:EPackage xmlns:e='http://www.eclipse.org/emf/2002/Ecore' xmlns:x='http://www.w3.org/2001/XMLSchema-instance' name='p' nsURI='urn:p'>{body}</e:EPackage>"
                ),
            );
            let document = Document::parse(source.clone()).unwrap();
            let bundle = DefinitionBundle {
                version: 1,
                sources: vec![source.clone()],
                namespaces: vec![],
            };
            let resolver = Resolver::new(&bundle).unwrap();
            let id = |index| DeclarationId {
                source: source.id.clone(),
                path: vec![index],
            };
            let ty = |index| Type::Classifier {
                declaration: id(index),
                arguments: vec![],
                raw: false,
            };
            let expected = paths
                .into_iter()
                .map(|path| InheritedType {
                    expression: ty(*path.last().unwrap()),
                    path: path.into_iter().map(id).collect(),
                })
                .collect::<Vec<_>>();
            let receiver = ty(selected);
            let span = &document.root.children[selected].span;
            // Independent components: declaration1413/16, type2512/28,
            // empty vector130/2, one-type vector2642/30, two-declaration
            // path2956/33 and three-declaration path4369/49. Map nodes128,
            // result records/slots160, moves32. Each source classifier check
            // adds parent ID1413/16 plus four visits:1413/20. Diamond reaches cached A once,
            // compares four unequal pairs (two copied types per pair), removes
            // a later equal A and retains literal first-encounter report paths.
            // Classifier checks: root1, chain2, diamond5 visits +4 unequal
            // comparisons, cycle3 (including the repeated owner before failure).
            // Each unequal comparison also checks two native classifier domains:
            // relation2826/44, adding11304/176 across the diamond.
            assert_lowering_receipt(&expected, bytes, work, &prefixes, |probe| {
                let context = InvocationContext::new(probe.clone());
                let observation = ParserObservation::new(&context);
                observation.finish(resolver.ancestry_expression_observed(
                    &receiver,
                    span,
                    &observation,
                ))
            });
        }
        let source = DefinitionSource::new(
            "memory:x",
            SourceKind::Ecore,
            "<e:EPackage xmlns:e='http://www.eclipse.org/emf/2002/Ecore' xmlns:x='http://www.w3.org/2001/XMLSchema-instance' name='p' nsURI='urn:p'><eClassifiers x:type='e:EClass' name='A' eSuperTypes='#//B'/><eClassifiers x:type='e:EClass' name='B' eSuperTypes='#//A'/></e:EPackage>",
        );
        let document = Document::parse(source.clone()).unwrap();
        let bundle = DefinitionBundle {
            version: 1,
            sources: vec![source.clone()],
            namespaces: vec![],
        };
        let resolver = Resolver::new(&bundle).unwrap();
        let receiver = Type::Classifier {
            declaration: DeclarationId {
                source: source.id.clone(),
                path: vec![1],
            },
            arguments: vec![],
            raw: false,
        };
        let span = &document.root.children[1].span;
        let context = InvocationContext::default();
        let observation = ParserObservation::new(&context);
        assert_eq!(
            observation
                .finish(resolver.ancestry_expression_observed(&receiver, span, &observation))
                .unwrap_err(),
            ObservedImportError::Import(ImportError {
                kind: ErrorKind::InvalidDefinition,
                span: document.root.children[0].opening.clone(),
                message: "Cyclic generic inheritance".into()
            })
        );
        // Repeated B rejects after its budget visit and first path comparison,
        // before a third path/report copy. Local partial results never publish.
        assert_lowering_receipt(
            &Err::<Vec<InheritedType>, _>("Cyclic generic inheritance".to_owned()),
            19053,
            259,
            &[
                1413, 2826, 2858, 4433, 4465, 5878, 6006, 7419, 8832, 8864, 11376, 14332, 14492,
                16067, 16099, 17512, 17640, 19053,
            ],
            |probe| {
                let context = InvocationContext::new(probe.clone());
                let observation = ParserObservation::new(&context);
                match observation.finish(resolver.ancestry_expression_observed(
                    &receiver,
                    span,
                    &observation,
                )) {
                    Ok(value) => Ok(Ok(value)),
                    Err(ObservedImportError::Import(error)) => Ok(Err(error.message)),
                    Err(error) => Err(error),
                }
            },
        );
    }

    #[test]
    fn observed_runtime_owner_projection_preserves_concrete_and_raw_erasure() {
        use crate::definitions::{
            bundle::DefinitionBundle,
            generic_resolution::Resolver,
            generic_types::{RuntimeType, Type},
            resources::DeclarationId,
        };
        let source = DefinitionSource::new(
            "memory:x",
            SourceKind::Ecore,
            "<e:EPackage xmlns:e='http://www.eclipse.org/emf/2002/Ecore' xmlns:x='http://www.w3.org/2001/XMLSchema-instance' name='p' nsURI='urn:p'><eClassifiers x:type='e:EClass' name='Owner'><eTypeParameters name='T'/></eClassifiers></e:EPackage>",
        );
        let document = Document::parse(source.clone()).unwrap();
        let bundle = DefinitionBundle {
            version: 1,
            sources: vec![source.clone()],
            namespaces: vec![],
        };
        let resolver = Resolver::new(&bundle).unwrap();
        let declaration = DeclarationId {
            source: source.id.clone(),
            path: vec![0],
        };
        let formal = DeclarationId {
            source: source.id.clone(),
            path: vec![0, 0],
        };
        let span = &document.root.children[0].span;
        let classifier = |arguments, raw| Type::Classifier {
            declaration: declaration.clone(),
            arguments,
            raw,
        };
        let parameter = Type::Parameter {
            declaration: formal,
        };
        for (expression, expected, bytes, work, prefixes) in [
            // Borrowed validation64/75, followed by the independent leaf
            // metadata copy2512/28. Runtime construction does not check arity.
            (
                classifier(vec![], false),
                classifier(vec![], false),
                2576,
                103,
                vec![32, 64, 2576],
            ),
            (
                classifier(vec![], true),
                classifier(vec![], true),
                2576,
                103,
                vec![32, 64, 2576],
            ),
            // Shape + canonical traversal encounter the nested parameter at
            // 128/79; erasure copies declaration1413/16 and classifier node128.
            // The shared source formal list then contributes8088/100: owner/path
            // copy1445/16, parameter3432/41, owner checks2986/41 and225 bookkeeping.
            (
                classifier(vec![parameter.clone()], false),
                classifier(vec![], true),
                9757,
                208,
                vec![
                    32, 64, 96, 128, 1541, 1669, 3082, 3114, 4687, 6100, 6191, 7764, 9339, 9404,
                    9532, 9597, 9725, 9757,
                ],
            ),
            // Invalid raw actuals stop shape validation at32/1, then follow
            // the existing source-formal erasure policy.
            (
                classifier(vec![classifier(vec![], false)], true),
                classifier(vec![], true),
                9661,
                130,
                vec![
                    32, 1445, 1573, 2986, 3018, 4591, 6004, 6095, 7668, 9243, 9308, 9436, 9501,
                    9629, 9661,
                ],
            ),
        ] {
            let original = expression.clone();
            assert_lowering_receipt(&expected, bytes, work, &prefixes, |probe| {
                let context = InvocationContext::new(probe.clone());
                let observation = ParserObservation::new(&context);
                observation.finish(resolver.runtime_classifier_projection_observed(
                    &expression,
                    span,
                    &observation,
                ))
            });
            assert_eq!(expression, original);
        }
        assert_lowering_receipt(&false, 64, 2, &[32, 64], |probe| {
            let context = InvocationContext::new(probe.clone());
            let observation = ParserObservation::new(&context);
            observation.finish(observation.preparation_at(span, |context| {
                RuntimeType::expression_is_concrete_observed(&parameter, context)
            }))
        });
        let context = InvocationContext::default();
        let observation = ParserObservation::new(&context);
        assert_eq!(
            observation
                .finish(resolver.runtime_classifier_projection_observed(
                    &parameter,
                    span,
                    &observation
                ))
                .unwrap_err(),
            ObservedImportError::Import(ImportError {
                kind: ErrorKind::InvalidDefinition,
                span: span.clone(),
                message: "Runtime owner requires a classifier".into()
            })
        );
        assert_lowering_receipt(
            &Err::<Type, _>("Runtime owner requires a classifier".to_owned()),
            64,
            2,
            &[32, 64],
            |probe| {
                let context = InvocationContext::new(probe.clone());
                let observation = ParserObservation::new(&context);
                match observation.finish(resolver.runtime_classifier_projection_observed(
                    &parameter,
                    span,
                    &observation,
                )) {
                    Ok(value) => Ok(Ok(value)),
                    Err(ObservedImportError::Import(error)) => Ok(Err(error.message)),
                    Err(error) => Err(error),
                }
            },
        );
    }

    #[test]
    fn observed_feature_erasure_preserves_source_authority_and_typed_failures() {
        use crate::definitions::{
            bundle::DefinitionBundle,
            generic_resolution::Resolver,
            generic_types::{RuntimeType, Type},
            resources::DeclarationId,
        };
        let source = DefinitionSource::new(
            "memory:x",
            SourceKind::Ecore,
            "<e:EPackage xmlns:e='http://www.eclipse.org/emf/2002/Ecore' xmlns:x='http://www.w3.org/2001/XMLSchema-instance' name='p' nsURI='urn:p'><eClassifiers x:type='e:EClass' name='Base'/><eClassifiers x:type='e:EClass' name='Owner'><eTypeParameters name='T'><eBounds eClassifier='#//Base'/></eTypeParameters><eStructuralFeatures x:type='e:EReference' name='value' eType='#//Base'><eGenericType eTypeParameter='#//Owner/T'/></eStructuralFeatures></eClassifiers></e:EPackage>",
        );
        let document = Document::parse(source.clone()).unwrap();
        let bundle = DefinitionBundle {
            version: 1,
            sources: vec![source.clone()],
            namespaces: vec![],
        };
        let resolver = Resolver::new(&bundle).unwrap();
        let id = |path| DeclarationId {
            source: source.id.clone(),
            path,
        };
        let concrete = Type::Classifier {
            declaration: id(vec![0]),
            arguments: vec![],
            raw: false,
        };
        let parameter = Type::Parameter {
            declaration: id(vec![1, 0]),
        };
        let feature = id(vec![1, 1]);
        let span = &document.root.children[1].children[1].span;
        let expected = RuntimeType::new(concrete.clone()).unwrap();
        for (expression, bytes, work, prefixes) in [
            // Canonical predicate64/75 + leaf copy2512/28 + existing
            // v1 constructor340/135. The identity is retained, not discarded.
            (
                concrete.clone(),
                2916,
                238,
                vec![32, 64, 2576, 2608, 2640, 2768, 2916],
            ),
            // Parameter predicate64/2 + one legacy selector visit + borrowed
            // declaration copy1413/16 + classifier128 + constructor340/135.
            (
                parameter.clone(),
                1945,
                156,
                vec![32, 64, 1477, 1605, 1637, 1669, 1797, 1945],
            ),
        ] {
            let original = expression.clone();
            assert_lowering_receipt(&expected, bytes, work, &prefixes, |probe| {
                let context = InvocationContext::new(probe.clone());
                let observation = ParserObservation::new(&context);
                observation.finish(resolver.runtime_feature_erasure_observed(
                    &expression,
                    &feature,
                    span,
                    &observation,
                ))
            });
            assert_eq!(expression, original);
        }
        // A class has only its empty superclass signature, so no feature
        // erasure can be borrowed. The original diagnostic/span is preserved.
        let missing = id(vec![0]);
        let context = InvocationContext::default();
        let observation = ParserObservation::new(&context);
        assert_eq!(
            observation
                .finish(resolver.runtime_feature_erasure_observed(
                    &parameter,
                    &missing,
                    span,
                    &observation
                ))
                .unwrap_err(),
            ObservedImportError::Import(ImportError {
                kind: ErrorKind::InvalidDefinition,
                span: span.clone(),
                message: "Missing structural feature erasure".into()
            })
        );
        assert_lowering_receipt(
            &Err::<RuntimeType, _>("Missing structural feature erasure".to_owned()),
            64,
            3,
            &[32, 64],
            |probe| {
                let context = InvocationContext::new(probe.clone());
                let observation = ParserObservation::new(&context);
                match observation.finish(resolver.runtime_feature_erasure_observed(
                    &parameter,
                    &missing,
                    span,
                    &observation,
                )) {
                    Ok(value) => Ok(Ok(value)),
                    Err(ObservedImportError::Import(error)) => Ok(Err(error.message)),
                    Err(error) => Err(error),
                }
            },
        );
    }

    #[test]
    fn observed_projection_bindings_deduplicate_before_copy_and_preserve_conflicts() {
        use crate::definitions::{
            bundle::DefinitionBundle,
            generic_resolution::Resolver,
            generic_types::{ParameterBinding, Type},
            resources::DeclarationId,
        };
        let source = DefinitionSource::new(
            "memory:x",
            SourceKind::Ecore,
            "<e:EPackage xmlns:e='http://www.eclipse.org/emf/2002/Ecore' xmlns:x='http://www.w3.org/2001/XMLSchema-instance' name='p' nsURI='urn:p'><eClassifiers x:type='e:EClass' name='Base'/><eClassifiers x:type='e:EClass' name='Owner'><eTypeParameters name='T'/></eClassifiers><eClassifiers x:type='e:EClass' name='Other'/></e:EPackage>",
        );
        let document = Document::parse(source.clone()).unwrap();
        let bundle = DefinitionBundle {
            version: 1,
            sources: vec![source.clone()],
            namespaces: vec![],
        };
        let resolver = Resolver::new(&bundle).unwrap();
        let id = |path| DeclarationId {
            source: source.id.clone(),
            path,
        };
        let ty = |index, arguments| Type::Classifier {
            declaration: id(vec![index]),
            arguments,
            raw: false,
        };
        let base = ty(0, vec![]);
        let receiver = ty(1, vec![base.clone()]);
        let span = &document.root.children[1].span;
        let expected = vec![ParameterBinding {
            parameter: id(vec![1, 0]),
            argument: base,
        }];
        // Formal declaration1573/18 (the second path atom adds32 payload
        // and128 node bytes), argument2512/28, map node128,
        // output record/slot160. Two owner visits, two binding visits, one move.
        // Each observed formal list adds8088/100: owner copy1445/16,
        // parameter3432/41, owner validation2986/41, two visits and225 retention.
        assert_lowering_receipt(
            &expected,
            20549,
            277,
            &[
                1413, 1445, 3018, 4431, 4522, 6095, 7670, 7735, 7863, 7928, 8056, 8088, 9661,
                12173, 12301, 13714, 13746, 15319, 16732, 16823, 18396, 19971, 20036, 20164, 20229,
                20357, 20389, 20549,
            ],
            |probe| {
                let context = InvocationContext::new(probe.clone());
                let observation = ParserObservation::new(&context);
                observation.finish(resolver.runtime_projection_bindings_observed(
                    &receiver,
                    &receiver,
                    span,
                    &observation,
                ))
            },
        );
        let conflicting = ty(1, vec![ty(2, vec![])]);
        let context = InvocationContext::default();
        let observation = ParserObservation::new(&context);
        assert_eq!(
            observation
                .finish(resolver.runtime_projection_bindings_observed(
                    &receiver,
                    &conflicting,
                    span,
                    &observation
                ))
                .unwrap_err(),
            ObservedImportError::Import(ImportError {
                kind: ErrorKind::InvalidDefinition,
                span: span.clone(),
                message: "Conflicting receiver/owner bindings".into()
            })
        );
        // Conflict is observed before any second key/argument copy or output.
        assert_lowering_receipt(
            &Err::<Vec<ParameterBinding>, _>("Conflicting receiver/owner bindings".to_owned()),
            20389,
            276,
            &[
                1413, 1445, 3018, 4431, 4522, 6095, 7670, 7735, 7863, 7928, 8056, 8088, 9661,
                12173, 12301, 13714, 13746, 15319, 16732, 16823, 18396, 19971, 20036, 20164, 20229,
                20357, 20389,
            ],
            |probe| {
                let context = InvocationContext::new(probe.clone());
                let observation = ParserObservation::new(&context);
                match observation.finish(resolver.runtime_projection_bindings_observed(
                    &receiver,
                    &conflicting,
                    span,
                    &observation,
                )) {
                    Ok(value) => Ok(Ok(value)),
                    Err(ObservedImportError::Import(error)) => Ok(Err(error.message)),
                    Err(error) => Err(error),
                }
            },
        );
        let empty = ty(1, vec![]);
        assert_lowering_receipt(
            &vec![],
            16176,
            228,
            &[
                1413, 1445, 3018, 4431, 4522, 6095, 7670, 7735, 7863, 7928, 8056, 8088, 9501, 9533,
                11106, 12519, 12610, 14183, 15758, 15823, 15951, 16016, 16144, 16176,
            ],
            |probe| {
                let context = InvocationContext::new(probe.clone());
                let observation = ParserObservation::new(&context);
                observation.finish(resolver.runtime_projection_bindings_observed(
                    &empty,
                    &empty,
                    span,
                    &observation,
                ))
            },
        );
    }

    #[test]
    fn observed_application_checks_preserve_bounds_raw_warnings_and_errors() {
        use crate::definitions::{
            bundle::DefinitionBundle,
            generic_resolution::{BoundCheck, Resolver, Warning},
            generic_types::{Type, TypeUse},
            resources::DeclarationId,
        };
        let source = DefinitionSource::new(
            "memory:x",
            SourceKind::Ecore,
            "<e:EPackage xmlns:e='http://www.eclipse.org/emf/2002/Ecore' xmlns:x='http://www.w3.org/2001/XMLSchema-instance' name='p' nsURI='urn:p'><eClassifiers x:type='e:EClass' name='Base'/><eClassifiers x:type='e:EClass' name='Owner'><eTypeParameters name='T'><eBounds eClassifier='#//Base'/></eTypeParameters></eClassifiers></e:EPackage>",
        );
        let document = Document::parse(source.clone()).unwrap();
        let bundle = DefinitionBundle {
            version: 1,
            sources: vec![source.clone()],
            namespaces: vec![],
        };
        let resolver = Resolver::new(&bundle).unwrap();
        let id = |path| DeclarationId {
            source: source.id.clone(),
            path,
        };
        let ty = |index, arguments, raw| Type::Classifier {
            declaration: id(vec![index]),
            arguments,
            raw,
        };
        let base = ty(0, vec![], false);
        let applied = ty(1, vec![base.clone()], false);
        let raw = ty(1, vec![], true);
        let span = &document.root.children[1].span;
        let expected_bound = BoundCheck {
            application: applied.clone(),
            parameter: id(vec![1, 0]),
            argument: base.clone(),
            declared_bound: TypeUse {
                declaration: id(vec![1, 0, 0]),
                span: document.root.children[1].children[0].children[0]
                    .span
                    .clone(),
                expression: base.clone(),
            },
            substituted_bound: base.clone(),
            satisfied: true,
        };
        let expected_warning = Warning {
            code: "raw_type".into(),
            message: "Raw application of memory:x [1] omits 1 type arguments".into(),
            span: span.clone(),
        };
        for (expression, expected, bytes, work, prefixes) in [
            // Leaf seen copy2512/28 + set128 + visit1 + empty substitution1
            // + shared leaf ancestry4345/52. Application/ancestry classifier
            // checks each add1413/20, giving9811/122.
            (
                base.clone(),
                (vec![], vec![]),
                9811,
                132,
                vec![2512, 2640, 4053, 5466, 6879, 6911, 7041, 9553, 9683, 9811],
            ),
            // Raw adds code72, literal message118/two work, source span1575/18
            // and warning record/slot160 before the same leaf ancestry.
            // Owner<T bound Base> formal parsing is10552/121: payload lengths76
            // and32, href7, parameter ID1573/18 and bound ID1733/19.
            // Owner/path copy1445/16 + list225 + two visits gives12222/139; owner validation2986/41 gives15208/180.
            // Application and parent lookup each construct that source list.
            // Two classifier checks add2826/40, giving42152/502.
            (
                raw.clone(),
                (vec![], vec![expected_warning]),
                42152,
                542,
                vec![
                    2512, 2640, 4053, 5466, 5498, 7071, 8484, 8624, 10197, 10229, 10325, 10396,
                    11971, 12099, 12227, 13960, 15535, 15663, 15695, 17268, 18843, 18908, 19036,
                    19101, 19229, 19261, 19333, 19451, 21026, 21186, 22599, 24012, 24044, 25457,
                    25489, 27062, 28475, 28615, 30188, 30220, 30316, 30387, 31962, 32090, 32218,
                    33951, 35526, 35654, 35686, 37259, 38834, 38899, 39027, 39092, 39220, 39252,
                    39382, 41894, 42024, 42152,
                ],
            ),
            // Nested type5024/56; binding4245/47 + map160/3; argument
            // checker6985/82; bound substitution1573/12; copied check9237/102;
            // verify/cache/nominal boundary/nominal visit4 + output32;
            // applied ancestry9561/111. Two source formal lists add30416/360
            // to the previous36945/421 receipt, giving67361/781. Four classifier
            // checks (Owner/Base application and ancestry) add5652/80:73013/861.
            (
                applied.clone(),
                (vec![expected_bound], vec![]),
                73013,
                911,
                vec![
                    5024, 5152, 6565, 7978, 8010, 9583, 10996, 11136, 12709, 12741, 12837, 12908,
                    14483, 14611, 14739, 16472, 18047, 18175, 18207, 19780, 21355, 21420, 21548,
                    21613, 21741, 21773, 23346, 25858, 26018, 26050, 26178, 28690, 28818, 30231,
                    31644, 33057, 33089, 33219, 35731, 35861, 35989, 36021, 36149, 37562, 42586,
                    44159, 46671, 46799, 46831, 48244, 49657, 49689, 51102, 51134, 52707, 54120,
                    54260, 55833, 55865, 55961, 56032, 57607, 57735, 57863, 59596, 61171, 61299,
                    61331, 62904, 64479, 64544, 64672, 64737, 64865, 64897, 67409, 67441, 67473,
                    67601, 67731, 72755, 72885, 73013,
                ],
            ),
        ] {
            let original = expression.clone();
            assert_lowering_receipt(&expected, bytes, work, &prefixes, |probe| {
                let context = InvocationContext::new(probe.clone());
                let observation = ParserObservation::new(&context);
                observation.finish(resolver.runtime_application_checks_observed(
                    &expression,
                    span,
                    &observation,
                ))
            });
            assert_eq!(expression, original);
        }
        for (expression, message, bytes, work, prefixes) in [
            (
                ty(0, vec![], true),
                "Invalid raw application",
                4053,
                54,
                vec![2512, 2640, 4053],
            ),
            (
                ty(1, vec![], false),
                "Application argument count mismatch",
                19261,
                249,
                vec![
                    2512, 2640, 4053, 5466, 5498, 7071, 8484, 8624, 10197, 10229, 10325, 10396,
                    11971, 12099, 12227, 13960, 15535, 15663, 15695, 17268, 18843, 18908, 19036,
                    19101, 19229, 19261,
                ],
            ),
        ] {
            let context = InvocationContext::default();
            let observation = ParserObservation::new(&context);
            assert_eq!(
                observation
                    .finish(resolver.runtime_application_checks_observed(
                        &expression,
                        span,
                        &observation
                    ))
                    .unwrap_err(),
                ObservedImportError::Import(ImportError {
                    kind: ErrorKind::InvalidDefinition,
                    span: span.clone(),
                    message: message.into()
                })
            );
            assert_lowering_receipt(
                &Err::<(Vec<BoundCheck>, Vec<Warning>), _>(message.to_owned()),
                bytes,
                work,
                &prefixes,
                |probe| {
                    let context = InvocationContext::new(probe.clone());
                    let observation = ParserObservation::new(&context);
                    match observation.finish(resolver.runtime_application_checks_observed(
                        &expression,
                        span,
                        &observation,
                    )) {
                        Ok(value) => Ok(Ok(value)),
                        Err(ObservedImportError::Import(error)) => Ok(Err(error.message)),
                        Err(error) => Err(error),
                    }
                },
            );
        }
    }

    #[test]
    fn observed_nominal_relations_preserve_subtypes_wildcards_invariance_and_limits() {
        use crate::definitions::{
            bundle::DefinitionBundle, generic_resolution::Resolver, generic_types::Type,
            resources::DeclarationId,
        };
        let source = DefinitionSource::new(
            "memory:x",
            SourceKind::Ecore,
            "<e:EPackage xmlns:e='http://www.eclipse.org/emf/2002/Ecore' xmlns:x='http://www.w3.org/2001/XMLSchema-instance' name='p' nsURI='urn:p'><eClassifiers x:type='e:EClass' name='A'/><eClassifiers x:type='e:EClass' name='B' eSuperTypes='#//A'/><eClassifiers x:type='e:EClass' name='Owner'><eTypeParameters name='T'/></eClassifiers></e:EPackage>",
        );
        let document = Document::parse(source.clone()).unwrap();
        let bundle = DefinitionBundle {
            version: 1,
            sources: vec![source.clone()],
            namespaces: vec![],
        };
        let resolver = Resolver::new(&bundle).unwrap();
        let id = |path| DeclarationId {
            source: source.id.clone(),
            path,
        };
        let ty = |index, arguments, raw| Type::Classifier {
            declaration: id(vec![index]),
            arguments,
            raw,
        };
        let a = ty(0, vec![], false);
        let b = ty(1, vec![], false);
        let any = Type::Wildcard {
            upper: None,
            lower: None,
        };
        let span = &document.root.children[0].span;
        for (actual, bound, expected, bytes, work, prefixes) in [
            (a.clone(), a.clone(), true, 0, 1, vec![]),
            // Existing chain ancestry22650/260, plus nominal enter, EObject
            // boundary, one ancestor scan and equal nominal enter:four.
            // Two nominal + two ancestry classifier checks add5652/80.
            (
                b.clone(),
                a.clone(),
                true,
                28302,
                364,
                vec![
                    1413, 2826, 4239, 5652, 5684, 7259, 7291, 8704, 8832, 10245, 11658, 11690,
                    14202, 17158, 17318, 17448, 19960, 20090, 20218, 20250, 22892, 23020, 25532,
                    28174, 28302,
                ],
            ),
            // Leaf ancestry4345/52 plus nominal and EObject boundary.
            // Two nominal + one ancestry classifier checks add4239/60.
            (
                a.clone(),
                b.clone(),
                false,
                8584,
                125,
                vec![1413, 2826, 4239, 5652, 5684, 5814, 8326, 8456, 8584],
            ),
            (
                Type::Wildcard {
                    upper: Some(Box::new(a.clone())),
                    lower: None,
                },
                a.clone(),
                true,
                0,
                2,
                vec![],
            ),
            (any.clone(), a.clone(), false, 0, 1, vec![]),
            (
                Type::Parameter {
                    declaration: id(vec![2, 0]),
                },
                a.clone(),
                false,
                0,
                1,
                vec![],
            ),
            // Concrete generic argument matching is invariant; wildcard
            // matching retains its existing unconstrained acceptance.
            (
                ty(2, vec![a.clone()], false),
                ty(2, vec![any.clone()], false),
                true,
                0,
                3,
                vec![],
            ),
            (
                ty(2, vec![a.clone()], false),
                ty(2, vec![b.clone()], false),
                false,
                4239,
                77,
                vec![1413, 2826, 4239],
            ),
            (
                ty(2, vec![a.clone()], false),
                ty(2, vec![ty(0, vec![], true)], false),
                true,
                0,
                4,
                vec![],
            ),
        ] {
            let original = (actual.clone(), bound.clone());
            assert_lowering_receipt(&expected, bytes, work, &prefixes, |probe| {
                let context = InvocationContext::new(probe.clone());
                let observation = ParserObservation::new(&context);
                let mut remaining = 16_384;
                observation.finish(resolver.nominal_bounded_observed(
                    &actual,
                    &bound,
                    span,
                    0,
                    &mut remaining,
                    &observation,
                ))
            });
            assert_eq!((actual, bound), original);
        }
        for (actual, expected, answer, bytes, work, prefixes) in [
            (a.clone(), ty(0, vec![], true), true, 0, 1, vec![]),
            (
                ty(2, vec![a.clone()], false),
                ty(2, vec![b.clone()], false),
                false,
                4239,
                76,
                vec![1413, 2826, 4239],
            ),
            (
                any,
                Type::Wildcard {
                    upper: Some(Box::new(a.clone())),
                    lower: None,
                },
                false,
                0,
                2,
                vec![],
            ),
        ] {
            assert_lowering_receipt(&answer, bytes, work, &prefixes, |probe| {
                let context = InvocationContext::new(probe.clone());
                let observation = ParserObservation::new(&context);
                let mut remaining = 16_384;
                observation.finish(resolver.invariant_equal_observed(
                    &actual,
                    &expected,
                    span,
                    0,
                    &mut remaining,
                    &observation,
                ))
            });
        }
        let context = InvocationContext::default();
        let observation = ParserObservation::new(&context);
        let mut remaining = 16_384;
        assert_eq!(
            observation
                .finish(resolver.nominal_bounded_observed(
                    &a,
                    &a,
                    span,
                    129,
                    &mut remaining,
                    &observation
                ))
                .unwrap_err(),
            ObservedImportError::Import(ImportError {
                kind: ErrorKind::ResourceLimit,
                span: span.clone(),
                message: "Generic conformance budget exceeded".into()
            })
        );
        assert_eq!(remaining, 16_384);
        assert_lowering_receipt(
            &Err::<bool, _>("Generic conformance budget exceeded".to_owned()),
            0,
            1,
            &[],
            |probe| {
                let context = InvocationContext::new(probe.clone());
                let observation = ParserObservation::new(&context);
                let mut remaining = 16_384;
                match observation.finish(resolver.nominal_bounded_observed(
                    &a,
                    &a,
                    span,
                    129,
                    &mut remaining,
                    &observation,
                )) {
                    Ok(value) => Ok(Ok(value)),
                    Err(ObservedImportError::Import(error)) => Ok(Err(error.message)),
                    Err(error) => Err(error),
                }
            },
        );
    }

    #[test]
    fn observed_generic_parser_preserves_literal_parameter_bounds_payloads_and_errors() {
        use crate::definitions::{
            bundle::DefinitionBundle,
            generics::{GenericKind, GenericType, TypeParameter, TypeReference},
            resources::{DeclarationId, ResourceSet},
        };
        for (body, bytes, work, prefixes) in [
            (
                "<eTypeParameters name='T'/>",
                3272,
                41,
                vec![91, 1504, 3079, 3144, 3272],
            ),
            (
                "<eTypeParameters name='T'><eBounds eClassifier='#//A'/></eTypeParameters>",
                10063,
                121,
                vec![
                    137, 1550, 1582, 1675, 1743, 3318, 3446, 3574, 5147, 6722, 6850, 6882, 8295,
                    9870, 9935, 10063,
                ],
            ),
        ] {
            let source = DefinitionSource::new(
                "memory:x",
                SourceKind::Ecore,
                format!(
                    "<e:EPackage xmlns:e='http://www.eclipse.org/emf/2002/Ecore' name='p' nsURI='urn:p'>{body}</e:EPackage>"
                ),
            );
            let document = Document::parse(source.clone()).unwrap();
            let bundle = DefinitionBundle {
                version: 1,
                sources: vec![source.clone()],
                namespaces: vec![],
            };
            let resources = ResourceSet::new(&bundle).unwrap();
            let parameter = &document.root.children[0];
            let id = |path| DeclarationId {
                source: source.id.clone(),
                path,
            };
            let expected_bound = parameter.children.first().map(|bound| GenericType {
                declaration: id(vec![0, 0]),
                span: bound.span.clone(),
                source_payload: "<eBounds eClassifier='#//A'/>".into(),
                value: GenericKind::Classifier {
                    reference: TypeReference {
                        href: "#//A".into(),
                        kind_hint: None,
                        span: bound.attributes[0].span.clone(),
                    },
                    arguments: vec![],
                },
            });
            let expected = TypeParameter {
                declaration: id(vec![0]),
                span: parameter.span.clone(),
                source_payload: body.into(),
                name: "T".into(),
                bounds: expected_bound.clone().into_iter().collect(),
            };
            // Declaration1413/16 (two path elements1573/18), span1575/18,
            // strings64+UTF8 length, records128, child/vector slots32.
            // Payload lengths27/73 and bound payload29 are independently authored.
            assert_lowering_receipt(&expected, bytes, work, &prefixes, |probe| {
                let context = InvocationContext::new(probe.clone());
                let observation = ParserObservation::new(&context);
                observation.finish(crate::definitions::generics::parse_parameter_observed(
                    &resources,
                    &id(vec![0]),
                    &observation,
                ))
            });
            assert_eq!(
                crate::definitions::generics::parse_parameter(&resources, &id(vec![0])).unwrap(),
                expected
            );
            if let Some(bound) = expected_bound {
                // Bound payload93 + href68 + reference span1575/18 + reference
                // record128 + kind128 + declaration1573/18 + span1575/18 + record128.
                assert_lowering_receipt(
                    &bound,
                    5268,
                    63,
                    &[93, 161, 1736, 1864, 1992, 3565, 5140, 5268],
                    |probe| {
                        let context = InvocationContext::new(probe.clone());
                        let observation = ParserObservation::new(&context);
                        observation.finish(crate::definitions::generics::parse_type_observed(
                            &resources,
                            &id(vec![0, 0]),
                            &observation,
                        ))
                    },
                );
                assert_eq!(
                    crate::definitions::generics::parse_type(&resources, &id(vec![0, 0])).unwrap(),
                    bound
                );
            }
        }
        let source = DefinitionSource::new(
            "memory:x",
            SourceKind::Ecore,
            "<e:EPackage xmlns:e='http://www.eclipse.org/emf/2002/Ecore' name='p' nsURI='urn:p'><eTypeParameters name='9'/></e:EPackage>",
        );
        let document = Document::parse(source.clone()).unwrap();
        let bundle = DefinitionBundle {
            version: 1,
            sources: vec![source.clone()],
            namespaces: vec![],
        };
        let resources = ResourceSet::new(&bundle).unwrap();
        let id = DeclarationId {
            source: source.id.clone(),
            path: vec![0],
        };
        let context = InvocationContext::default();
        let observation = ParserObservation::new(&context);
        assert_eq!(
            observation
                .finish(crate::definitions::generics::parse_parameter_observed(
                    &resources,
                    &id,
                    &observation
                ))
                .unwrap_err(),
            ObservedImportError::Import(ImportError {
                kind: ErrorKind::InvalidDefinition,
                span: document.root.children[0].attributes[0].span.clone(),
                message: "Invalid type parameter name".into()
            })
        );
        assert_lowering_receipt(
            &Err::<TypeParameter, _>("Invalid type parameter name".to_owned()),
            0,
            6,
            &[],
            |probe| {
                let context = InvocationContext::new(probe.clone());
                let observation = ParserObservation::new(&context);
                match observation.finish(crate::definitions::generics::parse_parameter_observed(
                    &resources,
                    &id,
                    &observation,
                )) {
                    Ok(value) => Ok(Ok(value)),
                    Err(ObservedImportError::Import(error)) => Ok(Err(error.message)),
                    Err(error) => Err(error),
                }
            },
        );
    }

    #[test]
    fn observed_runtime_member_receivers_reject_datatypes_and_enums_with_cancellable_validation() {
        use crate::definitions::{
            bundle::DefinitionBundle,
            generic_resolution::Resolver,
            generic_types::{RuntimeType, Type},
            resources::DeclarationId,
        };
        for kind in ["EDataType", "EEnum"] {
            let source = DefinitionSource::new(
                "memory:x",
                SourceKind::Ecore,
                format!(
                    "<e:EPackage xmlns:e='http://www.eclipse.org/emf/2002/Ecore' xmlns:x='http://www.w3.org/2001/XMLSchema-instance' name='p' nsURI='urn:p'><eClassifiers x:type='e:{kind}' name='Value'/></e:EPackage>"
                ),
            );
            let document = Document::parse(source.clone()).unwrap();
            let bundle = DefinitionBundle {
                version: 1,
                sources: vec![source.clone()],
                namespaces: vec![],
            };
            let resolver = Resolver::new(&bundle).unwrap();
            let receiver = RuntimeType::new(Type::Classifier {
                declaration: DeclarationId {
                    source: source.id.clone(),
                    path: vec![0],
                },
                arguments: vec![],
                raw: false,
            })
            .unwrap();
            let expected = ImportError {
                kind: ErrorKind::InvalidDefinition,
                span: document.root.children[0].span.clone(),
                message: "Runtime receiver requires an EClass".into(),
            };
            // Borrowed receiver lookup costs two visits. Classifier validation
            // copies the depth-one parent ID1413/16 and adds seven visits,
            // including its classifier/package lookups: total1413/25.
            // runtime_members first retains its span1575/18: total2988/43.
            for (members, bytes, work, prefixes) in [
                (false, 1413, 25, vec![1413]),
                (true, 2988, 43, vec![1575, 2988]),
            ] {
                assert_lowering_receipt(&expected, bytes, work, &prefixes, |probe| {
                    let context = InvocationContext::new(probe.clone());
                    let observation = ParserObservation::new(&context);
                    let result = if members {
                        observation
                            .finish(resolver.runtime_members_observed(&receiver, &observation))
                            .map(|_| ())
                    } else {
                        observation
                            .finish(
                                resolver
                                    .runtime_member_projections_observed(&receiver, &observation),
                            )
                            .map(|_| ())
                    };
                    match result {
                        Err(ObservedImportError::Import(error)) => Ok(error),
                        Err(error) => Err(error),
                        Ok(()) => panic!("nonclass runtime receiver was accepted"),
                    }
                });
            }
            assert_eq!(
                resolver.runtime_member_projections(&receiver).unwrap_err(),
                expected
            );
            assert_eq!(resolver.runtime_members(&receiver).unwrap_err(), expected);
        }
    }

    #[test]
    fn observed_empty_runtime_member_projections_preserve_class_admission_and_budgets() {
        use crate::definitions::{
            bundle::DefinitionBundle,
            generic_resolution::Resolver,
            generic_types::{RuntimeType, Type},
            resources::DeclarationId,
        };
        let source = DefinitionSource::new(
            "memory:x",
            SourceKind::Ecore,
            "<e:EPackage xmlns:e='http://www.eclipse.org/emf/2002/Ecore' xmlns:x='http://www.w3.org/2001/XMLSchema-instance' name='p' nsURI='urn:p'><eClassifiers x:type='e:EClass' name='Owner'/></e:EPackage>",
        );
        let bundle = DefinitionBundle {
            version: 1,
            sources: vec![source.clone()],
            namespaces: vec![],
        };
        let resolver = Resolver::new(&bundle).unwrap();
        let receiver = RuntimeType::new(Type::Classifier {
            declaration: DeclarationId {
                source: source.id.clone(),
                path: vec![0],
            },
            arguments: vec![],
            raw: false,
        })
        .unwrap();
        // Classifier1413/23 + receiver lookup2 + application9811/132 +
        // owner copy2512/28, slot32 + ancestry5758/77 + owner visit1,
        // empty formals lookup2, substitution1 and owner lookup2.
        // With no members, feature-domain projection adds no further work.
        for domains in [false, true] {
            assert_lowering_receipt(
                &0usize,
                19526,
                268,
                &[1413, 11224, 13736, 13768, 19526],
                |probe| {
                    let context = InvocationContext::new(probe.clone());
                    let observation = ParserObservation::new(&context);
                    if domains {
                        observation
                            .finish(
                                resolver.runtime_feature_domains_observed(&receiver, &observation),
                            )
                            .map(|v| v.len())
                    } else {
                        observation
                            .finish(
                                resolver
                                    .runtime_member_projections_observed(&receiver, &observation),
                            )
                            .map(|v| v.len())
                    }
                },
            );
        }
        assert!(
            resolver
                .runtime_member_projections(&receiver)
                .unwrap()
                .is_empty()
        );
        assert!(
            resolver
                .runtime_feature_domains(&receiver)
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn observed_source_node_lookup_borrows_exact_nodes_and_preserves_failures() {
        use crate::definitions::{
            bundle::{DefinitionBundle, SourceSpan},
            resources::{DeclarationId, ResourceSet},
        };
        let source = DefinitionSource::new(
            "memory:x",
            SourceKind::Ecore,
            "<e:EPackage xmlns:e='http://www.eclipse.org/emf/2002/Ecore' xmlns:x='http://www.w3.org/2001/XMLSchema-instance' name='p' nsURI='urn:p'><eClassifiers x:type='e:EClass' name='Owner'><eTypeParameters name='T'><eBounds eClassifier='#//Owner'/></eTypeParameters></eClassifiers></e:EPackage>",
        );
        let bundle = DefinitionBundle {
            version: 1,
            sources: vec![source.clone()],
            namespaces: vec![],
        };
        let resources = ResourceSet::new(&bundle).unwrap();
        // One exact document lookup, then one callback per child ordinal. The
        // returned node is borrowed from that exact revision; no retention fee.
        for path in [vec![], vec![0], vec![0, 0], vec![0, 0, 0]] {
            let expected = path.iter().fold(
                &resources.document(&source.id).unwrap().root,
                |node, index| &node.children[*index],
            );
            let work = 1 + path.len();
            let selected = DeclarationId {
                source: source.id.clone(),
                path,
            };
            assert_lowering_receipt(&true, 0, work, &[], |probe| {
                let context = InvocationContext::new(probe.clone());
                let observation = ParserObservation::new(&context);
                let actual = observation.finish(resources.node_context(&selected, &observation))?;
                Ok(std::ptr::eq(actual, expected))
            });
            assert!(std::ptr::eq(resources.node(&selected).unwrap(), expected));
        }
        let mut changed = source.id.clone();
        changed.sha256 = "0".repeat(64);
        for (selected, message, work) in [
            (
                DeclarationId {
                    source: source.id.clone(),
                    path: vec![99],
                },
                "Declaration path out of bounds".to_owned(),
                2,
            ),
            (
                DeclarationId {
                    source: source.id.clone(),
                    path: vec![0, 99],
                },
                "Declaration path out of bounds".to_owned(),
                3,
            ),
            (
                DeclarationId {
                    source: changed.clone(),
                    path: vec![0],
                },
                format!("Missing exact Ecore source memory:x@{}", "0".repeat(64)),
                1,
            ),
        ] {
            let context = InvocationContext::default();
            let observation = ParserObservation::new(&context);
            assert_eq!(
                observation
                    .finish(resources.node_context(&selected, &observation))
                    .unwrap_err(),
                ObservedImportError::Import(ImportError {
                    kind: ErrorKind::InvalidDefinition,
                    span: SourceSpan {
                        source: selected.source.clone(),
                        start: 0,
                        end: 0
                    },
                    message: message.clone()
                })
            );
            assert_eq!(resources.node(&selected).unwrap_err(), message);
            assert_lowering_receipt(&Err::<bool, _>(message), 0, work, &[], |probe| {
                let context = InvocationContext::new(probe.clone());
                let observation = ParserObservation::new(&context);
                match observation.finish(resources.node_context(&selected, &observation)) {
                    Ok(_) => Ok(Ok(true)),
                    Err(ObservedImportError::Import(error)) => Ok(Err(error.message)),
                    Err(error) => Err(error),
                }
            });
        }
    }

    #[test]
    fn observed_parameter_owners_preserve_package_paths_operation_rules_and_errors() {
        use crate::definitions::{
            bundle::DefinitionBundle, generic_resolution::Resolver, resources::DeclarationId,
        };
        let package = |body: &str| {
            format!(
                "<e:EPackage xmlns:e='http://www.eclipse.org/emf/2002/Ecore' xmlns:x='http://www.w3.org/2001/XMLSchema-instance' name='p' nsURI='urn:p'>{body}</e:EPackage>"
            )
        };
        let ordinary = "<eClassifiers x:type='e:EClass' name='Owner'><eTypeParameters name='T'/></eClassifiers>";
        // Copying an existing identity before popping its path retains the
        // original allocation shape: depth1=1413/16, depth2=1573/18,
        // depth3=1733/19. Class owners add seven visits (2986/41).
        // Operation and nested-package owners add two visits (4719/62).
        // XMI checks its root directly, saving one visit (4719/61).
        for (text, parameter_path, owner_path, bytes, work, prefixes) in [
            (
                package(ordinary),
                vec![0, 0],
                vec![0],
                2986,
                49,
                vec![1573, 2986],
            ),
            (
                package(
                    "<eClassifiers x:type='e:EClass' name='Owner'><eOperations name='op'><eTypeParameters name='T'/></eOperations></eClassifiers>",
                ),
                vec![0, 0, 0],
                vec![0, 0],
                4719,
                75,
                vec![1733, 3306, 4719],
            ),
            (
                package(&format!(
                    "<eSubpackages name='nested' nsURI='urn:n'>{ordinary}</eSubpackages>"
                )),
                vec![0, 0, 0],
                vec![0, 0],
                4719,
                75,
                vec![1733, 3306, 4719],
            ),
            (
                format!(
                    "<m:XMI xmlns:m='http://www.omg.org/XMI'>{}</m:XMI>",
                    package(ordinary)
                ),
                vec![0, 0, 0],
                vec![0, 0],
                4719,
                74,
                vec![1733, 3306, 4719],
            ),
        ] {
            let source = DefinitionSource::new("memory:x", SourceKind::Ecore, text);
            let bundle = DefinitionBundle {
                version: 1,
                sources: vec![source.clone()],
                namespaces: vec![],
            };
            let resolver = Resolver::new(&bundle).unwrap();
            let selected = DeclarationId {
                source: source.id.clone(),
                path: parameter_path,
            };
            let expected = DeclarationId {
                source: source.id.clone(),
                path: owner_path,
            };
            assert_lowering_receipt(&expected, bytes, work, &prefixes, |probe| {
                let context = InvocationContext::new(probe.clone());
                let observation = ParserObservation::new(&context);
                observation.finish(resolver.parameter_owner_observed(&selected, &observation))
            });
        }
        for (body, selected_path, span_path, attribute, message, bytes, work, prefixes) in [
            (
                ordinary,
                vec![0],
                vec![0],
                None,
                "Reference does not select a type parameter",
                0,
                3,
                vec![],
            ),
            (
                "<eTypeParameters name='T'/>",
                vec![0],
                vec![],
                None,
                "Reference does not select a classifier declaration",
                1413,
                24,
                vec![1413],
            ),
            (
                "<eClassifiers x:type='e:EDataType' name='Owner'><eOperations name='op'><eTypeParameters name='T'/></eOperations></eClassifiers>",
                vec![0, 0, 0],
                vec![0, 0],
                None,
                "Operation requires an EClass owner",
                4719,
                75,
                vec![1733, 3306, 4719],
            ),
            (
                "<eClassifiers x:type='e:EClass' name='Outer'><eClassifiers x:type='e:EClass' name='Inner'><eTypeParameters name='T'/></eClassifiers></eClassifiers>",
                vec![0, 0, 0],
                vec![0],
                None,
                "Classifier requires an actual EPackage owner",
                3306,
                55,
                vec![1733, 3306],
            ),
            (
                "<eClassifiers name='Owner'><eTypeParameters name='T'/></eClassifiers>",
                vec![0, 0],
                vec![0],
                None,
                "Missing Ecore declaration kind",
                2986,
                49,
                vec![1573, 2986],
            ),
            (
                "<eClassifiers xmlns:other='urn:other' x:type='other:EClass' name='Owner'><eTypeParameters name='T'/></eClassifiers>",
                vec![0, 0],
                vec![0],
                Some(1),
                "Invalid Ecore declaration kind",
                2986,
                50,
                vec![1573, 2986],
            ),
            (
                "<eClassifiers x:type='e:EReference' name='Owner'><eTypeParameters name='T'/></eClassifiers>",
                vec![0, 0],
                vec![0],
                None,
                "Unsupported classifier kind",
                2986,
                49,
                vec![1573, 2986],
            ),
        ] {
            let source = DefinitionSource::new("memory:x", SourceKind::Ecore, package(body));
            let document = Document::parse(source.clone()).unwrap();
            let bundle = DefinitionBundle {
                version: 1,
                sources: vec![source.clone()],
                namespaces: vec![],
            };
            let resolver = Resolver::new(&bundle).unwrap();
            let selected = DeclarationId {
                source: source.id.clone(),
                path: selected_path,
            };
            let node = span_path
                .iter()
                .fold(&document.root, |node, index| &node.children[*index]);
            let span = attribute.map_or_else(
                || node.opening.clone(),
                |index| node.attributes[index].span.clone(),
            );
            let context = InvocationContext::default();
            let observation = ParserObservation::new(&context);
            assert_eq!(
                observation
                    .finish(resolver.parameter_owner_observed(&selected, &observation))
                    .unwrap_err(),
                ObservedImportError::Import(ImportError {
                    kind: ErrorKind::InvalidDefinition,
                    span,
                    message: message.into()
                })
            );
            assert_lowering_receipt(
                &Err::<DeclarationId, _>(message.to_owned()),
                bytes,
                work,
                &prefixes,
                |probe| {
                    let context = InvocationContext::new(probe.clone());
                    let observation = ParserObservation::new(&context);
                    match observation
                        .finish(resolver.parameter_owner_observed(&selected, &observation))
                    {
                        Ok(value) => Ok(Ok(value)),
                        Err(ObservedImportError::Import(error)) => Ok(Err(error.message)),
                        Err(error) => Err(error),
                    }
                },
            );
        }
    }

    #[test]
    fn observed_formal_lists_preserve_exact_declarations_and_duplicate_name_errors() {
        use crate::definitions::{
            bundle::DefinitionBundle, generic_resolution::Resolver, generics::TypeParameter,
            resources::DeclarationId,
        };
        for (second, bytes, prefixes) in [
            (
                "U",
                16176,
                vec![
                    1413, 1445, 3018, 4431, 4522, 6095, 7670, 7735, 7863, 7928, 8056, 8088, 9501,
                    9533, 11106, 12519, 12610, 14183, 15758, 15823, 15951, 16016, 16144, 16176,
                ],
            ),
            (
                "T",
                15951,
                vec![
                    1413, 1445, 3018, 4431, 4522, 6095, 7670, 7735, 7863, 7928, 8056, 8088, 9501,
                    9533, 11106, 12519, 12610, 14183, 15758, 15823, 15951,
                ],
            ),
        ] {
            let source = DefinitionSource::new(
                "memory:x",
                SourceKind::Ecore,
                format!(
                    "<e:EPackage xmlns:e='http://www.eclipse.org/emf/2002/Ecore' xmlns:x='http://www.w3.org/2001/XMLSchema-instance' name='p' nsURI='urn:p'><eClassifiers x:type='e:EClass' name='Owner'><eTypeParameters name='T'/><eTypeParameters name='{second}'/></eClassifiers></e:EPackage>"
                ),
            );
            let document = Document::parse(source.clone()).unwrap();
            let bundle = DefinitionBundle {
                version: 1,
                sources: vec![source.clone()],
                namespaces: vec![],
            };
            let resolver = Resolver::new(&bundle).unwrap();
            let owner = DeclarationId {
                source: source.id.clone(),
                path: vec![0],
            };
            let parameters = document.root.children[0]
                .children
                .iter()
                .enumerate()
                .map(|(index, node)| TypeParameter {
                    declaration: DeclarationId {
                        source: source.id.clone(),
                        path: vec![0, index],
                    },
                    span: node.span.clone(),
                    source_payload: format!(
                        "<eTypeParameters name='{}'/>",
                        if index == 0 { "T" } else { second }
                    ),
                    name: if index == 0 {
                        "T".into()
                    } else {
                        second.into()
                    },
                    bounds: vec![],
                })
                .collect::<Vec<_>>();
            // Two lists use literal one-parameter8088/100 components.
            // Parameter owner validation adds ID1573/18 + owner ID1413/16
            // and seven visits:2986/41 per formal. On a
            // duplicate, skip the second name/set/output225 after parsing it.
            let expected = if second == "U" {
                Ok(parameters)
            } else {
                Err("Duplicate type parameter name in the same owner".to_owned())
            };
            assert_lowering_receipt(&expected, bytes, 224, &prefixes, |probe| {
                let context = InvocationContext::new(probe.clone());
                let observation = ParserObservation::new(&context);
                match observation.finish(resolver.formals_observed(&owner, &observation)) {
                    Ok(value) => Ok(Ok(value)),
                    Err(ObservedImportError::Import(error)) => Ok(Err(error.message)),
                    Err(error) => Err(error),
                }
            });
            if second == "T" {
                let context = InvocationContext::default();
                let observation = ParserObservation::new(&context);
                assert_eq!(
                    observation
                        .finish(resolver.formals_observed(&owner, &observation))
                        .unwrap_err(),
                    ObservedImportError::Import(ImportError {
                        kind: ErrorKind::InvalidDefinition,
                        span: document.root.children[0].children[1].span.clone(),
                        message: "Duplicate type parameter name in the same owner".into()
                    })
                );
            }
        }
    }

    pub(crate) fn assert_lowering_receipt<T: PartialEq + std::fmt::Debug>(
        expected: &T,
        bytes: usize,
        work: usize,
        prefixes: &[usize],
        run: impl Fn(&Rc<Probe>) -> Result<T, ObservedImportError>,
    ) {
        let full = Probe::with_work(bytes, work, None);
        assert_eq!(&run(&full).unwrap(), expected);
        assert_eq!((full.storage.get(), full.work.get()), (bytes, work));
        for prefix in prefixes {
            assert_eq!(
                run(&Probe::with_work(prefix - 1, work, None)).unwrap_err(),
                ObservedImportError::Observation("projection storage denied".into())
            );
        }
        assert_eq!(
            run(&Probe::with_work(bytes, work - 1, None)).unwrap_err(),
            ObservedImportError::Observation("projection work denied".into())
        );
        assert_eq!(
            &run(&Probe::with_work(bytes + 1, work + 1, None)).unwrap(),
            expected
        );
        for call in 0..full.calls.get() {
            assert_eq!(
                run(&Probe::with_work(bytes, work, Some(call))).unwrap_err(),
                ObservedImportError::Observation("projection cancelled".into())
            );
            assert_eq!(
                &run(&Probe::with_work(bytes, work, None)).unwrap(),
                expected
            );
        }
    }

    #[test]
    fn datatype_lowering_guards_metadata_binding_reasons_and_exact_source_copies() {
        for (attributes, strings, reason) in [
            ("instanceClassName='int'", vec!["int"], None),
            (
                "",
                vec![],
                Some("No declared instance binding; an explicit provider is required"),
            ),
            (
                "instanceClassName='foreign'",
                vec!["foreign"],
                Some("No native scalar binding for \"foreign\"; an explicit provider is required"),
            ),
            (
                "instanceClassName='int' instanceTypeName='long'",
                vec!["int", "long"],
                Some("Conflicting or extended instance metadata: \"int\", \"long\""),
            ),
        ] {
            let payload =
                format!("<eClassifiers xsi:type='ecore:EDataType' name='Count' {attributes}/>");
            let source = DefinitionSource::new(
                "memory:x",
                SourceKind::Ecore,
                format!(
                    "<ecore:EPackage xmlns:ecore='http://www.eclipse.org/emf/2002/Ecore' xmlns:xsi='http://www.w3.org/2001/XMLSchema-instance' name='p' nsURI='urn:p'>{payload}</ecore:EPackage>"
                ),
            );
            let document = Document::parse(source).unwrap();
            let root = Xml::from_node(&document.root, &document.source.text).unwrap();
            let node = &root.children[0];
            let expected = lower_datatype(node).unwrap();
            assert_eq!(expected.name(), "Count");
            assert_eq!(expected.declaration().path, [0]);
            assert_eq!(expected.source_payload(), Some(payload.as_str()));
            match (reason, expected.binding()) {
                (None, crate::definitions::datatypes::Binding::Scalar { codec }) => {
                    assert_eq!(*codec, crate::definitions::scalars::Builtin::EInt)
                }
                (
                    Some(reason),
                    crate::definitions::datatypes::Binding::Unsupported { reason: actual },
                ) => {
                    assert_eq!(actual, reason)
                }
                _ => panic!("wrong datatype binding"),
            }
            // Classifier ID1413/16 work; Count name69/1; each optional string
            // owns64+UTF8 and one work; reason formatting counts two passes;
            // the exact source payload owns64+UTF8 and one copy quantum.
            let mut prefixes = vec![1413, 1482];
            let mut bytes = 1482;
            let mut work = 17;
            for value in strings {
                bytes += 64 + value.len();
                work += value.len().div_ceil(256).max(1);
                prefixes.push(bytes);
            }
            if let Some(reason) = reason {
                bytes += 64 + reason.len();
                work += 2 * reason.len().div_ceil(256).max(1);
                prefixes.push(bytes);
            }
            bytes += 64 + payload.len();
            work += payload.len().div_ceil(256).max(1);
            prefixes.push(bytes);
            let run = |probe: &Rc<Probe>| {
                let context = InvocationContext::new(probe.clone());
                let observation = ParserObservation::new(&context);
                observation.finish(lower_datatype_observed(node, Some(&observation)))
            };
            let full = Probe::with_work(bytes, work, None);
            assert_eq!(run(&full).unwrap(), expected);
            assert_eq!((full.storage.get(), full.work.get()), (bytes, work));
            for prefix in prefixes {
                assert_eq!(
                    run(&Probe::with_work(prefix - 1, work, None)).unwrap_err(),
                    ObservedImportError::Observation("projection storage denied".into())
                );
            }
            assert_eq!(
                run(&Probe::with_work(bytes, work - 1, None)).unwrap_err(),
                ObservedImportError::Observation("projection work denied".into())
            );
            assert_eq!(
                run(&Probe::with_work(bytes + 1, work + 1, None)).unwrap(),
                expected
            );
            for call in 0..full.calls.get() {
                assert_eq!(
                    run(&Probe::with_work(bytes, work, Some(call))).unwrap_err(),
                    ObservedImportError::Observation("projection cancelled".into())
                );
                assert_eq!(run(&Probe::with_work(bytes, work, None)).unwrap(), expected);
            }
        }
    }

    #[test]
    fn enum_lowering_preflights_literal_identity_indexes_and_source_retention() {
        const PAYLOAD: &str = "<eClassifiers xsi:type='ecore:EEnum' name='Mode'><eLiterals name='One' value='7' literal='λ'/></eClassifiers>";
        let source = DefinitionSource::new(
            "memory:x",
            SourceKind::Ecore,
            format!(
                "<ecore:EPackage xmlns:ecore='http://www.eclipse.org/emf/2002/Ecore' xmlns:xsi='http://www.w3.org/2001/XMLSchema-instance' name='p' nsURI='urn:p'>{PAYLOAD}</ecore:EPackage>"
            ),
        );
        let document = Document::parse(source).unwrap();
        let root = Xml::from_node(&document.root, &document.source.text).unwrap();
        let node = &root.children[0];
        let original = node.clone();
        let expected = lower_enumeration(node).unwrap();
        assert_eq!(expected.name(), "Mode");
        assert_eq!(expected.literals()[0].name, "One");
        assert_eq!(expected.literals()[0].declaration.path, [0, 0]);
        assert_eq!(expected.literals()[0].value, 7);
        assert_eq!(expected.literals()[0].literal.as_deref(), Some("λ"));
        assert_eq!(expected.source_payload(), Some(PAYLOAD));
        let run = |probe: &Rc<Probe>| {
            let context = InvocationContext::new(probe.clone());
            let observation = ParserObservation::new(&context);
            observation.finish(lower_enumeration_observed(node, Some(&observation)))
        };
        // ID[0,0]:11 nodes/165 metadata=1573; ID[0]:10/133=1413.
        // Literal strings67+66/slot32; enum name68; name/spelling indexes
        // 195+194; numeric index128; source payload64+UTF8 bytes.
        let bytes = 3800 + PAYLOAD.len();
        let work = 43 + PAYLOAD.len().div_ceil(256);
        let full = Probe::with_work(bytes, work, None);
        assert_eq!(run(&full).unwrap(), expected);
        assert_eq!((full.storage.get(), full.work.get()), (bytes, work));
        for prefix in [
            1573, 1640, 1706, 1738, 3151, 3219, 3286, 3414, 3480, 3608, 3736, bytes,
        ] {
            assert_eq!(
                run(&Probe::with_work(prefix - 1, work, None)).unwrap_err(),
                ObservedImportError::Observation("projection storage denied".into()),
            );
        }
        for storage in [bytes, bytes + 1] {
            assert_eq!(
                run(&Probe::with_work(storage, work, None)).unwrap(),
                expected
            );
        }
        assert_eq!(
            run(&Probe::with_work(bytes, work - 1, None)).unwrap_err(),
            ObservedImportError::Observation("projection work denied".into()),
        );
        assert_eq!(
            run(&Probe::with_work(bytes, work + 1, None)).unwrap(),
            expected
        );
        for call in 0..full.calls.get() {
            assert_eq!(
                run(&Probe::with_work(bytes, work, Some(call))).unwrap_err(),
                ObservedImportError::Observation("projection cancelled".into()),
            );
            assert_eq!(run(&Probe::with_work(bytes, work, None)).unwrap(), expected);
        }
        assert_eq!(node.attrs, original.attrs);
        assert_eq!(node.declaration, original.declaration);
        assert_eq!(node.source_payload, original.source_payload);
        assert_eq!(node.children[0].attrs, original.children[0].attrs);
        assert_eq!(
            node.children[0].declaration,
            original.children[0].declaration
        );
    }

    #[test]
    fn enum_lowering_numeric_aliases_share_one_index_and_preserve_exact_failures() {
        const PAYLOAD: &str = "<eClassifiers xsi:type='ecore:EEnum' name='Mode'><eLiterals name='One' value='7' literal='λ'/><eLiterals name='Two' value='7'/></eClassifiers>";
        let input = |payload: &str| {
            let source = DefinitionSource::new(
                "memory:x",
                SourceKind::Ecore,
                format!(
                    "<ecore:EPackage xmlns:ecore='http://www.eclipse.org/emf/2002/Ecore' xmlns:xsi='http://www.w3.org/2001/XMLSchema-instance' name='p' nsURI='urn:p'>{payload}</ecore:EPackage>"
                ),
            );
            let document = Document::parse(source).unwrap();
            Xml::from_node(&document.root, &document.source.text).unwrap()
        };
        let root = input(PAYLOAD);
        let node = &root.children[0];
        let bytes = 3800 + 2062 + PAYLOAD.len();
        let work = 43 + 24 + PAYLOAD.len().div_ceil(256);
        let probe = Probe::with_work(bytes, work, None);
        let context = InvocationContext::new(probe.clone());
        let observation = ParserObservation::new(&context);
        let enumeration = observation
            .finish(lower_enumeration_observed(node, Some(&observation)))
            .unwrap();
        assert_eq!((probe.storage.get(), probe.work.get()), (bytes, work));
        assert_eq!(enumeration.by_value(7).unwrap().name, "One");
        assert_eq!(enumeration.by_name("Two").unwrap().declaration.path, [0, 1]);
        assert_eq!(enumeration.by_literal("Two").unwrap().value, 7);
        assert_eq!(enumeration.source_payload(), Some(PAYLOAD));
        assert_eq!(enumeration, lower_enumeration(node).unwrap());

        for (attribute, kind, message) in [
            (
                "xsi:type='ecore:EClass'",
                ErrorKind::InvalidDefinition,
                "Expected EEnumLiteral",
            ),
            (
                "unknown='yes'",
                ErrorKind::Unsupported,
                "Unsupported Ecore attribute eLiterals.unknown",
            ),
        ] {
            let payload = format!(
                "<eClassifiers xsi:type='ecore:EEnum' name='Mode'><eLiterals name='One' {attribute}/></eClassifiers>"
            );
            let root = input(&payload);
            let node = &root.children[0];
            // The literal package opening occupies 145 UTF-8 bytes.
            let start = 145 + payload.find(attribute).unwrap();
            let expected = ImportError {
                kind,
                span: crate::definitions::bundle::SourceSpan {
                    source: node.opening.source.clone(),
                    start,
                    end: start + attribute.len(),
                },
                message: message.into(),
            };
            assert_eq!(lower_enumeration(node).unwrap_err(), expected);
            let probe = Probe::new(usize::MAX, None);
            let context = InvocationContext::new(probe);
            let observation = ParserObservation::new(&context);
            assert_eq!(
                observation
                    .finish(lower_enumeration_observed(node, Some(&observation)))
                    .unwrap_err(),
                ObservedImportError::Import(expected),
            );
        }
    }

    #[test]
    fn retained_conversion_contract_preflights_literal_copy_and_vector_boundaries() {
        use crate::definitions::{conversion::ConversionContract, resources::DeclarationId};
        let source = DefinitionSource::new("memory:x", SourceKind::Ecore, "<a/>");
        let contract = ConversionContract {
            declarations: vec![],
            members: vec![],
            datatype: DeclarationId {
                source: source.id.clone(),
                path: vec![0],
            },
            provider: ProviderId {
                name: "p".into(),
                version: "1".into(),
            },
            serializable: true,
        };
        let run = |retained: &mut Vec<ConversionContract>, probe: &Rc<Probe>| {
            let context = InvocationContext::new(probe.clone());
            let observation = ParserObservation::new(&context);
            observation.finish(retain_compilation_contract(
                &contract,
                retained,
                &source.id,
                &observation,
            ))
        };
        // Contract: root130 + datatype key136/ID1413 + provider key136/value659
        // + serializable key140/bool160 =2774 (20 nodes,214 metadata bytes).
        // The output vector slot32 adds2806 total. Work20 visits +11 copy units.
        let full = Probe::with_work(2806, 31, None);
        let mut retained = Vec::new();
        run(&mut retained, &full).unwrap();
        assert_eq!(retained, vec![contract.clone()]);
        assert_eq!((full.storage.get(), full.work.get()), (2806, 31));
        for storage in [0, 2773, 2805, 2806, 2807] {
            let mut output = Vec::new();
            let result = run(&mut output, &Probe::with_work(storage, 31, None));
            if storage < 2806 {
                assert_eq!(
                    result.unwrap_err(),
                    ObservedImportError::Observation("projection storage denied".into())
                );
                assert!(output.is_empty());
            } else {
                result.unwrap();
                assert_eq!(output, vec![contract.clone()]);
            }
        }
        for work in [0, 30, 31, 32] {
            let mut output = Vec::new();
            let result = run(&mut output, &Probe::with_work(2806, work, None));
            if work < 31 {
                assert_eq!(
                    result.unwrap_err(),
                    ObservedImportError::Observation("projection work denied".into())
                );
                assert!(output.is_empty());
            } else {
                result.unwrap();
                assert_eq!(output, vec![contract.clone()]);
            }
        }
        for call in 0..full.calls.get() {
            let mut output = Vec::new();
            assert_eq!(
                run(&mut output, &Probe::with_work(2806, 31, Some(call))).unwrap_err(),
                ObservedImportError::Observation("projection cancelled".into())
            );
            assert!(output.is_empty());
            run(&mut output, &Probe::with_work(2806, 31, None)).unwrap();
            assert_eq!(output, vec![contract.clone()]);
        }
        for storage in [5611, 5612, 5613] {
            let probe = Probe::with_work(storage, 62, None);
            let mut output = Vec::new();
            run(&mut output, &probe).unwrap();
            let second = run(&mut output, &probe);
            if storage < 5612 {
                assert_eq!(
                    second.unwrap_err(),
                    ObservedImportError::Observation("projection storage denied".into())
                );
                assert_eq!(output, vec![contract.clone()]);
            } else {
                second.unwrap();
                assert_eq!(output, vec![contract.clone(), contract.clone()]);
            }
        }
        assert_eq!(contract.datatype.source, source.id);
        assert_eq!(contract.datatype.path, vec![0]);
        assert_eq!(source.text, "<a/>");
    }

    #[test]
    fn literal_header_copies_preflight_and_cancel_before_publication() {
        let source = DefinitionSource::new("memory:x", SourceKind::Ecore, "<a/>");
        let document = Document::parse(source.clone()).unwrap();
        let run = |probe: &Rc<Probe>| {
            let context = InvocationContext::new(probe.clone());
            let observation = ParserObservation::new(&context);
            observation.finish(Xml::from_node_at_observed(
                &document.root,
                &source.text,
                vec![],
                &observation,
            ))
        };
        // Xml/declaration records 128 each; payload/name strings 64+65;
        // source identity's shared copy policy 727; opening span 328.
        let prefixes = [128, 192, 257, 984, 1112, 1440];
        let full = Probe::new(1440, None);
        let xml = run(&full).unwrap();
        assert_eq!(full.storage.get(), 1440);
        assert_eq!(full.work.get(), 11);
        assert_eq!(xml.name, "a");
        assert_eq!(xml.declaration.source, source.id);
        assert_eq!(xml.opening, document.root.opening);
        assert!(xml.children.is_empty());
        for prefix in prefixes {
            assert!(matches!(
                run(&Probe::new(prefix - 1, None)),
                Err(ObservedImportError::Observation(_))
            ));
        }
        for call in 0..full.calls.get() {
            assert_eq!(
                run(&Probe::new(usize::MAX, Some(call))).err().unwrap(),
                ObservedImportError::Observation("projection cancelled".into())
            );
        }
        assert_eq!(run(&Probe::new(1441, None)).unwrap().name, "a");
        assert_eq!(document.source, source);
    }
}

#[cfg(test)]
#[path = "compiler_enum_preparation_tests.rs"]
mod enum_preparation_tests;
