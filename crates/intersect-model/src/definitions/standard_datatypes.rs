//! Exact pinned standard datatype metadata and raw provider routing.
//! Retaining a generic source parameter does not instantiate its generic semantics.
use super::{Xml, lower_datatype};
use crate::definitions::{
    bundle::{DefinitionSource, SourceId, SourceKind},
    datatypes::Datatype,
    ecore::{Document, ErrorKind, ImportError},
};
use std::collections::BTreeMap;

/// The pinned standard source, built (and its content digest computed) once
/// per process; every caller receives the same immutable source.
fn installed_standard_datatype_source() -> &'static DefinitionSource {
    static SOURCE: std::sync::OnceLock<DefinitionSource> = std::sync::OnceLock::new();
    SOURCE.get_or_init(|| {
        DefinitionSource::new(
            "http://www.eclipse.org/emf/2002/Ecore",
            SourceKind::Ecore,
            include_str!(
                "../../assets/fixtures/definition-conformance/upstream/ecore/model/Ecore.ecore"
            ),
        )
    })
}

pub fn standard_datatype_source() -> DefinitionSource {
    installed_standard_datatype_source().clone()
}

/// Identity of the pinned standard source, without copying or rehashing it.
/// Identity checks on semantic paths use this rather than building a source.
pub fn standard_datatype_source_id() -> &'static SourceId {
    &installed_standard_datatype_source().id
}
pub(crate) fn standard_datatype_source_observed(
    observation: &crate::definitions::ecore::ParserObservation<'_, '_>,
) -> Result<DefinitionSource, ImportError> {
    observation.static_source(
        crate::definitions::ecore::ECORE,
        include_str!(
            "../../assets/fixtures/definition-conformance/upstream/ecore/model/Ecore.ecore"
        ),
    )
}

/// Raw class descriptor metadata from the exact shipped upstream declaration.
/// The original payload (including T) is retained; generic instantiation is E07.
pub fn standard_datatype(name: &str) -> Result<Datatype, String> {
    if !matches!(
        name,
        "EJavaClass"
            | "EJavaObject"
            | "EEnumerator"
            | "EInvocationTargetException"
            | "EDiagnosticChain"
            | "EResource"
            | "EResourceSet"
            | "EEList"
            | "EMap"
            | "EFeatureMap"
            | "EFeatureMapEntry"
            | "ETreeIterator"
    ) {
        return Err(format!(
            "No installed standard provider contract for {name}"
        ));
    }
    let source = standard_datatype_source();
    let document = Document::parse(source.clone()).map_err(|error| error.to_string())?;
    let root = Xml::from_node(&document.root, &source.text).map_err(|error| error.to_string())?;
    let mut node = root
        .children
        .into_iter()
        .find(|node| node.attrs.get("name").map(String::as_str) == Some(name))
        .ok_or("Missing pinned standard datatype")?;
    // Only exact pinned unbounded signatures may use these raw provider contracts.
    // Preserve the original source payload; do not instantiate a generic argument.
    let parameters: &[&str] = match name {
        "EJavaClass" => &["T"],
        "ETreeIterator" | "EEList" => &["E"],
        "EMap" => &["K", "V"],
        _ => &[],
    };
    if !parameters.is_empty() {
        if node.children.len() != parameters.len()
            || node
                .children
                .iter()
                .zip(parameters)
                .any(|(child, parameter)| {
                    child.name != "eTypeParameters"
                        || child.attrs != BTreeMap::from([("name".into(), (*parameter).into())])
                        || !child.children.is_empty()
                })
        {
            return Err(format!("Pinned {name} raw provider signature changed"));
        }
        node.children.clear();
    }
    lower_datatype(&node).map_err(|error| error.to_string())
}

/// Resolve only exact installed standard identities; names alone never select a revision.
pub fn standard_datatype_by_id(
    id: &crate::definitions::resources::DeclarationId,
) -> Result<Datatype, String> {
    if id.source != *standard_datatype_source_id() {
        return Err("Unknown pinned standard datatype source revision".into());
    }
    for name in [
        "EJavaClass",
        "EJavaObject",
        "EEnumerator",
        "EInvocationTargetException",
        "EDiagnosticChain",
        "EResource",
        "EResourceSet",
        "ETreeIterator",
        "EEList",
        "EMap",
        "EFeatureMap",
        "EFeatureMapEntry",
    ] {
        let datatype = standard_datatype(name)?;
        if datatype.declaration() == id {
            return Ok(datatype);
        }
    }
    Err("Unknown pinned standard datatype declaration".into())
}

pub(super) fn route(
    root: &mut Xml,
    datatypes: &mut BTreeMap<String, Datatype>,
) -> Result<(), ImportError> {
    if matches!(
        root.name.as_str(),
        "eStructuralFeatures" | "eOperations" | "eParameters"
    ) && let Some(ty) = root.attrs.get("eType")
    {
        let href = ty.strip_prefix("ecore:EDataType ").unwrap_or(ty);
        if let Some(
            name @ ("EJavaClass"
            | "EJavaObject"
            | "EEnumerator"
            | "EInvocationTargetException"
            | "EDiagnosticChain"
            | "EResource"
            | "EResourceSet"
            | "EEList"
            | "EMap"
            | "EFeatureMap"
            | "EFeatureMapEntry"
            | "ETreeIterator"),
        ) = href.strip_prefix("http://www.eclipse.org/emf/2002/Ecore#//")
        {
            let datatype = standard_datatype(name).map_err(|error| root.error(error))?;
            let key = format!("__standard_{name}");
            if let Some(previous) = datatypes.get(&key) {
                if previous != &datatype {
                    return Err(root.error("Standard datatype key collision"));
                }
            } else {
                datatypes.insert(key.clone(), datatype);
            }
            root.attrs.insert("eType".into(), format!("#//{key}"));
        }
    }
    let parent = root.name.as_str();
    for node in &mut root.children {
        let executable_child = match parent {
            "ecore:EPackage" => {
                node.name == "eClassifiers"
                    && node.attrs.get("xsi:type").map(String::as_str) == Some("ecore:EClass")
            }
            "eClassifiers" => matches!(node.name.as_str(), "eStructuralFeatures" | "eOperations"),
            "eOperations" => node.name == "eParameters",
            _ => false,
        };
        if executable_child {
            route(node, datatypes)?;
        }
    }
    if root.name == "ecore:EPackage" {
        for node in &root.children {
            if node.attrs.get("name").is_some_and(|name| {
                matches!(
                    name.as_str(),
                    "__standard_EJavaClass"
                        | "__standard_EJavaObject"
                        | "__standard_EEnumerator"
                        | "__standard_EInvocationTargetException"
                        | "__standard_EDiagnosticChain"
                        | "__standard_EResource"
                        | "__standard_EResourceSet"
                        | "__standard_ETreeIterator"
                        | "__standard_EEList"
                        | "__standard_EMap"
                        | "__standard_EFeatureMap"
                        | "__standard_EFeatureMapEntry"
                ) && datatypes.contains_key(name)
            }) {
                return Err(node.at(
                    "name",
                    ErrorKind::InvalidDefinition,
                    "Standard datatype key collision",
                ));
            }
        }
    }
    Ok(())
}
