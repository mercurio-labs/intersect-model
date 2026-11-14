//! Source-preserving XML import. A parsed document is not an executable Ecore schema.
use crate::definitions::bundle::{DefinitionSource, SourceKind, SourceSpan};
use quick_xml::{
    events::Event,
    name::{QName, ResolveResult},
    reader::NsReader,
};
use std::collections::BTreeSet;
#[path = "ecore_observation.rs"]
mod observation;
pub use observation::ObservedImportError;
pub(crate) use observation::ParserObservation;
#[path = "ecore_document_preparation.rs"]
mod document_preparation;
pub use document_preparation::OwnedDocumentPreparation;
use observation::RECORD_BYTES;

pub const ECORE: &str = "http://www.eclipse.org/emf/2002/Ecore";
pub const XSI: &str = "http://www.w3.org/2001/XMLSchema-instance";
pub const XMI: &str = "http://www.omg.org/XMI";
const XMLNS: &str = "http://www.w3.org/2000/xmlns/";

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct ExpandedName {
    pub namespace: Option<String>,
    pub local: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ErrorKind {
    MalformedXml,
    Unsupported,
    InvalidDefinition,
    ResourceLimit,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ImportError {
    pub kind: ErrorKind,
    pub span: SourceSpan,
    pub message: String,
}
impl ImportError {
    pub(crate) fn new(kind: ErrorKind, span: &SourceSpan, message: impl Into<String>) -> Self {
        Self {
            kind,
            span: span.clone(),
            message: message.into(),
        }
    }
}
impl std::fmt::Display for ImportError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:?}: {}", self.kind, self.message)
    }
}
impl std::error::Error for ImportError {}

#[derive(Clone, Debug)]
pub struct Attribute {
    pub name: ExpandedName,
    pub lexical_name: String,
    pub value: String,
    pub span: SourceSpan,
    /// Resolved QName for xsi:type, or an optional kind prefix in an Ecore type reference.
    pub type_name: Option<ExpandedName>,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PayloadKind {
    Text,
    Comment,
    CData,
    ProcessingInstruction,
    Declaration,
    EntityReference,
}
#[derive(Clone, Debug)]
pub struct Payload {
    pub kind: PayloadKind,
    pub span: SourceSpan,
}
#[derive(Clone, Debug)]
pub struct Node {
    pub name: ExpandedName,
    pub lexical_name: String,
    pub attributes: Vec<Attribute>,
    pub children: Vec<Node>,
    /// Non-element content, including whitespace. Source spans preserve ordering.
    pub payloads: Vec<Payload>,
    pub opening: SourceSpan,
    pub span: SourceSpan,
}
#[derive(Clone, Debug)]
pub struct Document {
    pub source: DefinitionSource,
    pub root: Node,
    pub misc: Vec<Payload>,
}

fn name_start(c: char) -> bool {
    matches!(c, 'A'..='Z' | '_' | 'a'..='z' | '\u{c0}'..='\u{d6}' | '\u{d8}'..='\u{f6}' | '\u{f8}'..='\u{2ff}' | '\u{370}'..='\u{37d}' | '\u{37f}'..='\u{1fff}' | '\u{200c}'..='\u{200d}' | '\u{2070}'..='\u{218f}' | '\u{2c00}'..='\u{2fef}' | '\u{3001}'..='\u{d7ff}' | '\u{f900}'..='\u{fdcf}' | '\u{fdf0}'..='\u{fffd}' | '\u{10000}'..='\u{effff}')
}
fn valid_qname(name: &str) -> bool {
    let mut parts = name.split(':');
    let valid = |p: &str| {
        let mut chars = p.chars();
        chars.next().is_some_and(name_start) && chars.all(|c| name_start(c) || matches!(c, '-' | '.' | '0'..='9' | '\u{b7}' | '\u{300}'..='\u{36f}' | '\u{203f}'..='\u{2040}'))
    };
    parts.next().is_some_and(valid) && parts.next().is_none_or(valid) && parts.next().is_none()
}
fn xml_char(c: char) -> bool {
    matches!(c, '\t' | '\n' | '\r' | '\u{20}'..='\u{d7ff}' | '\u{e000}'..='\u{fffd}' | '\u{10000}'..='\u{10ffff}')
}
fn expanded(
    reader: &NsReader<&[u8]>,
    name: &str,
    attribute: bool,
    span: &SourceSpan,
    observation: &ParserObservation<'_, '_>,
) -> Result<ExpandedName, ImportError> {
    observation.work(name.len().div_ceil(256).max(1), &span.source)?;
    if !valid_qname(name) {
        return Err(ImportError::new(
            ErrorKind::MalformedXml,
            span,
            "Invalid XML qualified name",
        ));
    }
    let (ns, local) = reader
        .resolver()
        .resolve(QName(name.as_bytes()), !attribute);
    let namespace = match ns {
        ResolveResult::Unbound => None,
        ResolveResult::Bound(ns) => {
            let raw = std::str::from_utf8(ns.as_ref())
                .map_err(|e| ImportError::new(ErrorKind::MalformedXml, span, e.to_string()))?;
            observation.string(raw.len(), &span.source)?;
            Some(
                quick_xml::escape::unescape(raw)
                    .map_err(|e| ImportError::new(ErrorKind::MalformedXml, span, e.to_string()))?
                    .into_owned(),
            )
        }
        ResolveResult::Unknown(prefix) => {
            return Err(ImportError::new(
                ErrorKind::MalformedXml,
                span,
                format!(
                    "Unbound XML namespace prefix {}",
                    String::from_utf8_lossy(&prefix)
                ),
            ));
        }
    };
    observation.string(local.as_ref().len(), &span.source)?;
    Ok(ExpandedName {
        namespace,
        local: String::from_utf8_lossy(local.as_ref()).into_owned(),
    })
}

/// Explicit host policy for retained source parsing. This does not change XML
/// nesting limits, schema admission, grammar limits, or observation budgets.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ParseLimits {
    pub source_bytes: usize,
}
impl Default for ParseLimits {
    fn default() -> Self {
        Self {
            source_bytes: 256_000,
        }
    }
}

impl Document {
    pub fn parse(source: DefinitionSource) -> Result<Self, ImportError> {
        Self::parse_with_limits(source, ParseLimits::default())
    }
    /// Parse through the same source-preserving importer with explicit size policy.
    /// Accepting a document does not establish executable Ecore or Xtext support.
    pub fn parse_with_limits(
        source: DefinitionSource,
        limits: ParseLimits,
    ) -> Result<Self, ImportError> {
        Self::parse_context(
            source,
            &ParserObservation::new(&crate::definitions::conversion::InvocationContext::default()),
            limits,
        )
    }
    /// Copy a borrowed source and parse it under a cumulative logical allocation
    /// receipt. This observes the same parser; it does not provide host yielding.
    pub fn parse_observed(
        source: &DefinitionSource,
        context: &crate::definitions::conversion::InvocationContext<'_>,
    ) -> Result<Self, ObservedImportError> {
        Self::parse_observed_with_limits(source, context, ParseLimits::default())
    }
    /// Explicit source policy preserves the same cumulative work/storage receipts
    /// and cancellation path as `parse_observed`.
    pub fn parse_observed_with_limits(
        source: &DefinitionSource,
        context: &crate::definitions::conversion::InvocationContext<'_>,
        limits: ParseLimits,
    ) -> Result<Self, ObservedImportError> {
        context
            .checkpoint()
            .map_err(ObservedImportError::Observation)?;
        if context.has_observer() {
            let retained =
                crate::definitions::retained_storage::reserve_serialized(source, context)
                    .map_err(ObservedImportError::Observation)?;
            // This portable retained bound also bounds source-copy bytes, so
            // deny work before cloning text, identities or dependency strings.
            context
                .work(retained.div_ceil(256))
                .map_err(ObservedImportError::Observation)?;
        }
        let observation = ParserObservation::new(context);
        observation.finish(Self::parse_context(source.clone(), &observation, limits))
    }
    fn parse_context(
        source: DefinitionSource,
        observation: &ParserObservation<'_, '_>,
        limits: ParseLimits,
    ) -> Result<Self, ImportError> {
        let mut cursor = document_preparation::DocumentCursor::new(&source, observation, limits)?;
        while !cursor.advance(observation)? {}
        let (root, misc) = cursor.take_output()?;
        Ok(Self { source, root, misc })
    }
}

fn attach(
    node: Node,
    stack: &mut [Node],
    root: &mut Option<Node>,
    observation: &ParserObservation<'_, '_>,
) -> Result<(), ImportError> {
    if let Some(parent) = stack.last_mut() {
        observation.reserve(32, &node.opening.source)?;
        parent.children.push(node);
    } else if root.is_some() {
        return Err(ImportError::new(
            ErrorKind::MalformedXml,
            &node.opening,
            "Multiple XML roots",
        ));
    } else {
        *root = Some(node);
    }
    Ok(())
}

impl ExpandedName {
    pub(crate) fn profile_name(&self) -> String {
        self.profile().to_string()
    }
    pub(crate) fn profile(&self) -> impl std::fmt::Display + '_ {
        struct Profile<'a>(&'a ExpandedName);
        impl std::fmt::Display for Profile<'_> {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                let name = self.0;
                match name.namespace.as_deref() {
                    None => f.write_str(&name.local),
                    Some(ECORE) => write!(f, "ecore:{}", name.local),
                    Some(XSI) => write!(f, "xsi:{}", name.local),
                    Some(XMI) => write!(f, "xmi:{}", name.local),
                    Some(uri) => write!(f, "{{{uri}}}{}", name.local),
                }
            }
        }
        Profile(self)
    }
}
impl Attribute {
    pub(crate) fn namespace_declaration(&self) -> bool {
        self.name.namespace.as_deref() == Some(XMLNS)
    }
}
