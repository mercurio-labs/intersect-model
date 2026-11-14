//! Source-preserving generic signature AST. Resolution and substitution are separate stages.
use crate::definitions::{
    bundle::SourceSpan,
    ecore::ParserObservation,
    ecore::{Attribute, ECORE, ErrorKind, ImportError, Node, PayloadKind, XMI, XSI},
    resources::{DeclarationId, ResourceSet},
};
use serde::{Deserialize, Serialize};
#[path = "generics_preparation.rs"]
mod preparation;
pub(crate) use preparation::GenericTypePreparation;
use preparation::payload;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TypeReference {
    pub href: String,
    /// Expanded Ecore kind, independent of the source's namespace prefix.
    pub kind_hint: Option<String>,
    pub span: SourceSpan,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum GenericKind {
    Classifier {
        reference: TypeReference,
        arguments: Vec<GenericType>,
    },
    Parameter {
        reference: TypeReference,
    },
    Wildcard {
        upper: Option<Box<GenericType>>,
        lower: Option<Box<GenericType>>,
    },
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GenericType {
    pub declaration: DeclarationId,
    pub span: SourceSpan,
    pub source_payload: String,
    pub value: GenericKind,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TypeParameter {
    pub declaration: DeclarationId,
    pub span: SourceSpan,
    pub source_payload: String,
    pub name: String,
    pub bounds: Vec<GenericType>,
}
fn attr<'a>(
    node: &'a Node,
    name: &str,
    observation: &ParserObservation<'_, '_>,
) -> Result<Option<&'a Attribute>, ImportError> {
    for attribute in &node.attributes {
        observation.work(1, &node.span.source)?;
        if attribute.name.namespace.is_none() && attribute.name.local == name {
            return Ok(Some(attribute));
        }
    }
    Ok(None)
}
fn error(span: &SourceSpan, message: impl Into<String>) -> ImportError {
    ImportError::new(ErrorKind::InvalidDefinition, span, message)
}
fn node<'a>(
    resources: &'a ResourceSet,
    id: &DeclarationId,
    observation: &ParserObservation<'_, '_>,
) -> Result<&'a Node, ImportError> {
    resources.node_context(id, observation)
}
fn child(
    id: &DeclarationId,
    index: usize,
    observation: &ParserObservation<'_, '_>,
) -> Result<DeclarationId, ImportError> {
    observation.serialized(id, &id.source)?;
    observation.reserve(32, &id.source)?;
    let mut result = id.clone();
    result.path.push(index);
    Ok(result)
}
fn attributes(
    node: &Node,
    allowed: &[&str],
    expected_kind: &str,
    observation: &ParserObservation<'_, '_>,
) -> Result<(), ImportError> {
    for a in &node.attributes {
        observation.work(1, &node.span.source)?;
        check_attribute(a, allowed, expected_kind)?;
    }
    Ok(())
}
fn check_attribute(
    a: &Attribute,
    allowed: &[&str],
    expected_kind: &str,
) -> Result<(), ImportError> {
    match (a.name.namespace.as_deref(), a.name.local.as_str()) {
        (None, name) if allowed.contains(&name) => {}
        (Some(XMI), "id") | (Some("http://www.w3.org/2000/xmlns/"), _) => {}
        (Some(XSI), "type") => {
            if !a
                .type_name
                .as_ref()
                .is_some_and(|n| n.namespace.as_deref() == Some(ECORE) && n.local == expected_kind)
            {
                return Err(error(
                    &a.span,
                    format!("Expected Ecore {expected_kind} kind"),
                ));
            }
        }
        _ => {
            return Err(ImportError::new(
                ErrorKind::Unsupported,
                &a.span,
                format!("Unsupported generic attribute {}", a.lexical_name),
            ));
        }
    }
    Ok(())
}

fn hint(
    attribute: &Attribute,
    parameter: bool,
    observation: &ParserObservation<'_, '_>,
) -> Result<Option<String>, ImportError> {
    observation.work(1, &attribute.span.source)?;
    let Some(name) = &attribute.type_name else {
        return Ok(None);
    };
    let permitted = if parameter {
        name.local == "ETypeParameter"
    } else {
        matches!(
            name.local.as_str(),
            "EClassifier" | "EClass" | "EDataType" | "EEnum"
        )
    };
    if name.namespace.as_deref() != Some(ECORE) || !permitted {
        return Err(error(
            &attribute.span,
            "Generic reference has an incompatible Ecore kind hint",
        ));
    }
    observation.string(name.local.len(), &attribute.span.source)?;
    Ok(Some(name.local.clone()))
}
pub(crate) fn type_position(node: &Node) -> bool {
    node.name.namespace.is_none()
        && matches!(
            node.name.local.as_str(),
            "eGenericType"
                | "eGenericSuperTypes"
                | "eGenericExceptions"
                | "eTypeArguments"
                | "eBounds"
                | "eUpperBound"
                | "eLowerBound"
        )
}
/// Parse shape and retain exact input. This does not resolve references or prove arity/conformance.
pub fn parse_type(resources: &ResourceSet, id: &DeclarationId) -> Result<GenericType, ImportError> {
    parse_type_observed(
        resources,
        id,
        &ParserObservation::new(&crate::definitions::conversion::InvocationContext::default()),
    )
}
pub(crate) fn parse_type_observed(
    resources: &ResourceSet,
    id: &DeclarationId,
    observation: &ParserObservation<'_, '_>,
) -> Result<GenericType, ImportError> {
    let mut preparation = GenericTypePreparation::begin();
    while !preparation.advance(resources, id, observation)? {}
    preparation.finish(id)
}

/// Parse declaration shape; owner/scope/name conflicts are checked during signature resolution.
pub fn parse_parameter(
    resources: &ResourceSet,
    id: &DeclarationId,
) -> Result<TypeParameter, ImportError> {
    parse_parameter_observed(
        resources,
        id,
        &ParserObservation::new(&crate::definitions::conversion::InvocationContext::default()),
    )
}
pub(crate) fn parse_parameter_observed(
    resources: &ResourceSet,
    id: &DeclarationId,
    observation: &ParserObservation<'_, '_>,
) -> Result<TypeParameter, ImportError> {
    observation.work(1, &id.source)?;
    let node = node(resources, id, observation)?;
    if node.name.namespace.is_some() || node.name.local != "eTypeParameters" {
        return Err(error(&node.opening, "Selection is not a type parameter"));
    }
    attributes(node, &["name"], "ETypeParameter", observation)?;
    let name = attr(node, "name", observation)?
        .ok_or_else(|| error(&node.opening, "Type parameter requires a name"))?;
    observation.work(name.value.len().div_ceil(256).max(1), &id.source)?;
    if !crate::definitions::ecore_identifier::valid(&name.value) {
        return Err(error(&name.span, "Invalid type parameter name"));
    }
    let source_payload = payload(resources, node, observation)?;
    let mut bounds = Vec::new();
    for (index, nested) in node.children.iter().enumerate() {
        observation.work(1, &id.source)?;
        match (nested.name.namespace.as_deref(), nested.name.local.as_str()) {
            (None, "eBounds") => {
                let nested_id = child(id, index, observation)?;
                let bound = parse_type_observed(resources, &nested_id, observation)?;
                observation.reserve(32, &id.source)?;
                bounds.push(bound);
            }
            (None, "eAnnotations") => {} // Retained in source_payload; annotation execution is E12.
            _ => {
                return Err(ImportError::new(
                    ErrorKind::Unsupported,
                    &nested.opening,
                    "Unsupported type parameter child",
                ));
            }
        }
    }
    observation.serialized(id, &id.source)?;
    observation.serialized(&node.span, &id.source)?;
    observation.string(name.value.len(), &id.source)?;
    observation.reserve(128, &id.source)?;
    Ok(TypeParameter {
        declaration: id.clone(),
        span: node.span.clone(),
        source_payload,
        name: name.value.clone(),
        bounds,
    })
}

#[cfg(test)]
pub(crate) fn retained_decoder_layout() -> [usize; 5] {
    preparation::measured_layout()
}
#[cfg(test)]
#[path = "generics_decoder_initial_controls.rs"]
mod decoder_initial_controls;

#[cfg(test)]
#[path = "generics_decoder_direct_compatibility_controls.rs"]
mod source_decoder_complete_original_controls;
