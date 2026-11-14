//! Retained execution of the same source-preserving XML parser used by Document.
//! One input chunk or XML event advances at a time. Source ownership copying,
//! reader scanning of a single event, attribute decoding and cleanup remain
//! synchronous boundaries.
use super::*;
use crate::definitions::bundle::{SourceValidation, input_chunk_end};

struct InputValidation<'source> {
    identity: Option<SourceValidation<'source>>,
    characters: usize,
    limits: ParseLimits,
}

pub(super) struct DocumentCursor<'source> {
    source: &'source DefinitionSource,
    reader: NsReader<&'source [u8]>,
    base: usize,
    stack: Vec<Node>,
    root: Option<Node>,
    misc: Vec<Payload>,
    declaration: bool,
    input: Option<InputValidation<'source>>,
    complete: bool,
    failed: bool,
}
impl<'source> DocumentCursor<'source> {
    pub(super) fn new(
        source: &'source DefinitionSource,
        observation: &ParserObservation<'_, '_>,
        limits: ParseLimits,
    ) -> Result<Self, ImportError> {
        let mut cursor = Self::new_with_retained_input(source, observation, limits)?;
        while cursor.input.is_some() {
            cursor.advance_input(observation, false)?;
        }
        Ok(cursor)
    }
    fn new_with_retained_input(
        source: &'source DefinitionSource,
        observation: &ParserObservation<'_, '_>,
        limits: ParseLimits,
    ) -> Result<Self, ImportError> {
        // Precharge hashing, XML-character validation and reader scanning.
        observation.work(3 * source.text.len().div_ceil(256).max(1), &source.id)?;
        // Digest-format scratch, document record and two source-sized parser
        // scratch bounds cover reader namespace/attribute decoding temporaries.
        observation.scratch(source.text.len(), &source.id)?;
        let base = if source.text.starts_with('\u{feff}') {
            3
        } else {
            0
        };
        let mut reader = NsReader::from_str(&source.text[base..]);
        reader.config_mut().check_comments = true;
        let stack: Vec<Node> = Vec::new();
        let root = None;
        let misc = Vec::new();
        let declaration = false;
        Ok(Self {
            source,
            reader,
            base,
            stack,
            root,
            misc,
            declaration,
            input: Some(InputValidation {
                identity: Some(SourceValidation::new(source)),
                characters: 0,
                limits,
            }),
            complete: false,
            failed: false,
        })
    }
    fn advance_input(
        &mut self,
        observation: &ParserObservation<'_, '_>,
        observe_chunk: bool,
    ) -> Result<(), ImportError> {
        let Some(input) = self.input.as_mut() else {
            return Ok(());
        };
        let source = self.source;
        // Initialization already charged the complete scans. Each retained
        // chunk still checks the current observer before doing that work.
        if observe_chunk {
            observation.work(0, &source.id)?;
        }
        if let Some(identity) = input.identity.as_mut() {
            let ready = identity.advance().map_err(|message| {
                ImportError::new(
                    ErrorKind::InvalidDefinition,
                    &SourceSpan {
                        source: source.id.clone(),
                        start: 0,
                        end: 0,
                    },
                    message,
                )
            })?;
            if ready {
                if source.kind != SourceKind::Ecore {
                    return Err(ImportError::new(
                        ErrorKind::InvalidDefinition,
                        &observation.span(&source.id, 0, 0)?,
                        "Expected an Ecore source",
                    ));
                }
                if source.text.len() > input.limits.source_bytes {
                    return Err(ImportError::new(
                        ErrorKind::ResourceLimit,
                        &observation.span(&source.id, 0, 0)?,
                        "Definition size budget exceeded",
                    ));
                }
                input.identity = None;
            }
            return Ok(());
        }
        let end = input_chunk_end(&source.text, input.characters);
        if let Some((offset, ch)) = source.text[input.characters..end]
            .char_indices()
            .find(|(_, ch)| !xml_char(*ch))
        {
            let offset = input.characters + offset;
            return Err(ImportError::new(
                ErrorKind::MalformedXml,
                &observation.span(&source.id, offset, offset + ch.len_utf8())?,
                "Invalid XML character",
            ));
        }
        input.characters = end;
        if end == source.text.len() {
            self.input = None;
        }
        Ok(())
    }
    pub(super) fn advance(
        &mut self,
        observation: &ParserObservation<'_, '_>,
    ) -> Result<bool, ImportError> {
        if self.failed {
            return Err(self.error("XML document preparation has failed"));
        }
        if self.complete {
            return Ok(true);
        }
        let result = if self.input.is_some() {
            self.advance_input(observation, true).map(|()| false)
        } else {
            self.advance_inner(observation)
        };
        if result.is_err() {
            self.failed = true;
        }
        result
    }
    fn error(&self, message: &str) -> ImportError {
        ImportError::new(
            ErrorKind::InvalidDefinition,
            &SourceSpan {
                source: self.source.id.clone(),
                start: 0,
                end: self.source.text.len(),
            },
            message,
        )
    }
    fn advance_inner(
        &mut self,
        observation: &ParserObservation<'_, '_>,
    ) -> Result<bool, ImportError> {
        let Self {
            source,
            reader,
            base,
            stack,
            root,
            misc,
            declaration,
            complete,
            ..
        } = self;
        let span = |start, end| observation.span(&source.id, start, end);
        observation.work(1, &source.id)?;
        let start = *base + reader.buffer_position() as usize;
        let event = reader.read_event().map_err(|e| {
            let end = (*base + reader.buffer_position() as usize).min(source.text.len());
            ImportError::new(
                ErrorKind::MalformedXml,
                &SourceSpan {
                    source: source.id.clone(),
                    start,
                    end,
                },
                e.to_string(),
            )
        })?;
        let end = *base + reader.buffer_position() as usize;
        let here = span(start, end)?;
        let malformed = |message| ImportError::new(ErrorKind::MalformedXml, &here, message);
        let empty = matches!(event, Event::Empty(_));
        match event {
            Event::Start(e) | Event::Empty(e) => {
                if stack.len() >= 63 {
                    return Err(ImportError::new(
                        ErrorKind::ResourceLimit,
                        &here,
                        "XML nesting budget exceeded",
                    ));
                }
                observation.reserve(RECORD_BYTES, &source.id)?;
                observation.reserve_span(&source.id)?;
                observation.string(e.name().as_ref().len(), &source.id)?;
                let lexical_name = std::str::from_utf8(e.name().as_ref())
                    .map_err(|e| malformed(e.to_string()))?
                    .to_owned();
                let name = expanded(reader, &lexical_name, false, &here, observation)?;
                let typed_ecore = name.namespace.is_none()
                    && matches!(
                        name.local.as_str(),
                        "eStructuralFeatures" | "eOperations" | "eParameters"
                    );
                let generic_ecore = name.namespace.is_none()
                    && matches!(
                        name.local.as_str(),
                        "eGenericType"
                            | "eGenericSuperTypes"
                            | "eGenericExceptions"
                            | "eTypeArguments"
                            | "eBounds"
                            | "eUpperBound"
                            | "eLowerBound"
                    );
                let mut attributes = Vec::new();
                let mut seen = BTreeSet::new();
                // Attribute iteration borrows the source-backed start event. Track each
                // lexical extent in order; never search for a diagnostic after lowering.
                let mut cursor = start + 1 + lexical_name.len();
                for attr in e.attributes() {
                    observation.work(1, &source.id)?;
                    let attr = attr.map_err(|e| malformed(e.to_string()))?;
                    observation.reserve(RECORD_BYTES + 32, &source.id)?;
                    observation.string(attr.key.as_ref().len(), &source.id)?;
                    let lexical_name = std::str::from_utf8(attr.key.as_ref())
                        .map_err(|e| malformed(e.to_string()))?
                        .to_owned();
                    while source
                        .text
                        .as_bytes()
                        .get(cursor)
                        .is_some_and(u8::is_ascii_whitespace)
                    {
                        cursor += 1;
                    }
                    let attr_start = cursor;
                    cursor += lexical_name.len();
                    while source
                        .text
                        .as_bytes()
                        .get(cursor)
                        .is_some_and(|c| c.is_ascii_whitespace() || *c == b'=')
                    {
                        cursor += 1;
                    }
                    let quote = *source
                        .text
                        .as_bytes()
                        .get(cursor)
                        .ok_or_else(|| malformed("Missing attribute quote".into()))?;
                    cursor += 1;
                    while source
                        .text
                        .as_bytes()
                        .get(cursor)
                        .is_some_and(|c| *c != quote)
                    {
                        cursor += 1;
                    }
                    cursor += 1;
                    let location = span(attr_start, cursor)?;
                    observation.work(2 * attr.value.len().div_ceil(256).max(1), &source.id)?;
                    observation.string(attr.value.len(), &source.id)?;
                    let value = attr
                        .decoded_and_normalized_value(
                            quick_xml::XmlVersion::Implicit1_0,
                            reader.decoder(),
                        )
                        .map_err(|e| {
                            ImportError::new(ErrorKind::MalformedXml, &location, e.to_string())
                        })?
                        .into_owned();
                    if !value.chars().all(xml_char) || attr.value.contains(&b'<') {
                        return Err(ImportError::new(
                            ErrorKind::MalformedXml,
                            &location,
                            "Invalid XML attribute value",
                        ));
                    }
                    let name = if lexical_name == "xmlns" || lexical_name.starts_with("xmlns:") {
                        if !valid_qname(&lexical_name) {
                            return Err(ImportError::new(
                                ErrorKind::MalformedXml,
                                &location,
                                "Invalid namespace declaration",
                            ));
                        }
                        let xml = "http://www.w3.org/XML/1998/namespace";
                        if value == XMLNS
                            || (value == xml && lexical_name != "xmlns:xml")
                            || (lexical_name == "xmlns:xml" && value != xml)
                            || (value.is_empty() && lexical_name != "xmlns")
                            || value.chars().any(char::is_whitespace)
                        {
                            return Err(ImportError::new(
                                ErrorKind::MalformedXml,
                                &location,
                                "Invalid reserved or empty namespace binding",
                            ));
                        }
                        observation.string(XMLNS.len(), &source.id)?;
                        observation.string(lexical_name.len(), &source.id)?;
                        ExpandedName {
                            namespace: Some(XMLNS.into()),
                            local: lexical_name.clone(),
                        }
                    } else {
                        expanded(reader, &lexical_name, true, &location, observation)?
                    };
                    observation.reserve(RECORD_BYTES, &source.id)?;
                    observation.string(name.local.len(), &source.id)?;
                    if let Some(namespace) = &name.namespace {
                        observation.string(namespace.len(), &source.id)?;
                    }
                    if !seen.insert(name.clone()) {
                        return Err(ImportError::new(
                            ErrorKind::MalformedXml,
                            &location,
                            "Duplicate expanded XML attribute",
                        ));
                    }
                    let qname = if name.namespace.as_deref() == Some(XSI) && name.local == "type" {
                        Some(value.trim())
                    } else if name.namespace.is_none()
                        && ((name.local == "eType" && typed_ecore)
                            || (matches!(name.local.as_str(), "eClassifier" | "eTypeParameter")
                                && generic_ecore))
                    {
                        value.split_once(char::is_whitespace).map(|(q, _)| q)
                    } else if name.namespace.is_none() && name.local == "eSuperTypes" {
                        value
                            .split_once(char::is_whitespace)
                            .map(|(q, _)| q)
                            .filter(|q| q.contains(':') && !q.contains('/') && !q.contains('#'))
                    } else {
                        None
                    };
                    let type_name = qname
                        .map(|q| expanded(reader, q, false, &location, observation))
                        .transpose()?;
                    attributes.push(Attribute {
                        name,
                        lexical_name,
                        value,
                        span: location,
                        type_name,
                    });
                }
                let node = Node {
                    name,
                    lexical_name,
                    attributes,
                    children: Vec::new(),
                    payloads: Vec::new(),
                    opening: here.clone(),
                    span: here,
                };
                if empty {
                    attach(node, stack, root, observation)?;
                } else {
                    observation.reserve(32, &source.id)?;
                    stack.push(node);
                }
            }
            Event::End(_) => {
                let mut node = stack
                    .pop()
                    .ok_or_else(|| malformed("Unexpected closing tag".into()))?;
                node.span.end = end;
                attach(node, stack, root, observation)?;
            }
            Event::Eof => {
                if let Some(node) = stack.last() {
                    return Err(ImportError::new(
                        ErrorKind::MalformedXml,
                        &node.opening,
                        "Unclosed XML element",
                    ));
                }
                if root.is_none() {
                    return Err(ImportError::new(
                        ErrorKind::MalformedXml,
                        &SourceSpan {
                            source: source.id.clone(),
                            start: 0,
                            end: source.text.len(),
                        },
                        "Exactly one XML root is required",
                    ));
                }
                *complete = true;
                return Ok(true);
            }
            Event::DocType(_) => {
                return Err(ImportError::new(
                    ErrorKind::Unsupported,
                    &here,
                    "DTD declarations are unsupported; no external entities are loaded",
                ));
            }
            event => {
                let kind = match event {
                    Event::Text(e) => {
                        let text = e.decode().map_err(|e| malformed(e.to_string()))?;
                        if text.contains("]]>")
                            || (stack.is_empty()
                                && !text.trim_matches([' ', '\t', '\r', '\n']).is_empty())
                        {
                            return Err(malformed("Unexpected XML text".into()));
                        }
                        PayloadKind::Text
                    }
                    Event::Comment(_) => PayloadKind::Comment,
                    Event::CData(_) if !stack.is_empty() => PayloadKind::CData,
                    Event::PI(e) => {
                        let target = std::str::from_utf8(e.target())
                            .map_err(|e| malformed(e.to_string()))?;
                        if target.eq_ignore_ascii_case("xml") || !valid_qname(target) {
                            return Err(malformed("Invalid processing instruction target".into()));
                        }
                        PayloadKind::ProcessingInstruction
                    }
                    Event::Decl(e) => {
                        if *declaration
                            || start > 3
                            || root.is_some()
                            || !stack.is_empty()
                            || !misc.is_empty()
                        {
                            return Err(malformed("Misplaced XML declaration".into()));
                        }
                        if e.version().map_err(|e| malformed(e.to_string()))?.as_ref() != b"1.0" {
                            return Err(ImportError::new(
                                ErrorKind::Unsupported,
                                &here,
                                "Only XML 1.0 is supported",
                            ));
                        }
                        if let Some(encoding) = e.encoding()
                            && !encoding
                                .map_err(|e| malformed(e.to_string()))?
                                .eq_ignore_ascii_case(b"UTF-8")
                        {
                            return Err(ImportError::new(
                                ErrorKind::Unsupported,
                                &here,
                                "Only UTF-8 XML is supported",
                            ));
                        }
                        let raw = std::str::from_utf8(&e).map_err(|e| malformed(e.to_string()))?;
                        let xml_decl = quick_xml::events::BytesStart::from_content(raw, 3);
                        let mut previous = 0;
                        for attr in xml_decl.attributes() {
                            let attr = attr.map_err(|e| malformed(e.to_string()))?;
                            let rank = match attr.key.as_ref() {
                                b"version" => 1,
                                b"encoding" => 2,
                                b"standalone"
                                    if [b"yes".as_slice(), b"no"]
                                        .contains(&attr.value.as_ref()) =>
                                {
                                    3
                                }
                                _ => {
                                    return Err(malformed(
                                        "Invalid XML declaration attribute".into(),
                                    ));
                                }
                            };
                            if rank <= previous {
                                return Err(malformed(
                                    "Invalid XML declaration attribute order".into(),
                                ));
                            }
                            previous = rank;
                        }
                        *declaration = true;
                        PayloadKind::Declaration
                    }
                    Event::GeneralRef(e) if !stack.is_empty() => {
                        let value = e.resolve_char_ref().map_err(|e| malformed(e.to_string()))?;
                        if value.is_some_and(|c| !xml_char(c))
                            || (value.is_none()
                                && ![b"amp".as_slice(), b"lt", b"gt", b"apos", b"quot"]
                                    .contains(&e.as_ref()))
                        {
                            return Err(malformed(
                                "Unknown or invalid XML entity reference".into(),
                            ));
                        }
                        PayloadKind::EntityReference
                    }
                    _ => return Err(malformed("Unexpected XML construct".into())),
                };
                observation.reserve(RECORD_BYTES + 32, &source.id)?;
                let payload = Payload { kind, span: here };
                if let Some(node) = stack.last_mut() {
                    node.payloads.push(payload);
                } else {
                    misc.push(payload);
                }
            }
        }
        Ok(false)
    }
    pub(super) fn take_output(&mut self) -> Result<(Node, Vec<Payload>), ImportError> {
        if self.failed || !self.complete {
            return Err(self.error("XML document preparation is not complete"));
        }
        let root = self
            .root
            .take()
            .ok_or_else(|| self.error("Missing completed XML root"))?;
        Ok((root, std::mem::take(&mut self.misc)))
    }
}
self_cell::self_cell!(
    struct DocumentInputs {
        owner: DefinitionSource,
        #[covariant]
        dependent: DocumentCursor,
    }
);
/// Owned source parsing with retained event progress and fresh per-advance observers.
/// Parsed XML preserves source information; it does not certify Ecore semantics.
pub struct OwnedDocumentPreparation {
    holder: DocumentInputs,
    failure: Option<ObservedImportError>,
}
const STATE_BYTES: usize = 2048;
const _: () = assert!(
    std::mem::size_of::<DefinitionSource>()
        + std::mem::size_of::<DocumentCursor<'static>>()
        + std::mem::size_of::<Option<ObservedImportError>>()
        + 128
        <= STATE_BYTES
);
impl OwnedDocumentPreparation {
    /// Admit the owned source and parser holder before retention. Whole-source
    /// identity/character validation still runs synchronously during begin.
    pub fn begin(
        source: &DefinitionSource,
        limits: ParseLimits,
        context: &crate::definitions::conversion::InvocationContext<'_>,
    ) -> Result<Self, ObservedImportError> {
        Self::begin_inner(source, limits, context, false)
    }
    /// Retain URI, digest and XML-character validation between input chunks,
    /// before parsing events. Each advance inspects at most 256 input bytes or
    /// parses one XML event. Source-copy admission remains synchronous.
    /// Validation failures are reported by advance; finish never publishes an
    /// incomplete or invalid document. The existing begin validates immediately.
    pub fn begin_with_retained_input(
        source: &DefinitionSource,
        limits: ParseLimits,
        context: &crate::definitions::conversion::InvocationContext<'_>,
    ) -> Result<Self, ObservedImportError> {
        Self::begin_inner(source, limits, context, true)
    }
    fn begin_inner(
        source: &DefinitionSource,
        limits: ParseLimits,
        context: &crate::definitions::conversion::InvocationContext<'_>,
        retain_input: bool,
    ) -> Result<Self, ObservedImportError> {
        context
            .checkpoint()
            .map_err(ObservedImportError::Observation)?;
        if context.has_observer() {
            let bytes = crate::definitions::retained_storage::reserve_serialized(source, context)
                .map_err(ObservedImportError::Observation)?;
            context
                .work(bytes.div_ceil(256))
                .map_err(ObservedImportError::Observation)?;
        }
        let observation = ParserObservation::new(context);
        observation.finish(observation.reserve(STATE_BYTES, &source.id))?;
        Self::begin_owned(source.clone(), limits, &observation, retain_input)
    }
    pub(crate) fn begin_with_prepared_source(
        source: crate::definitions::bundle::AdmittedSourceCopy,
        limits: ParseLimits,
        context: &crate::definitions::conversion::InvocationContext<'_>,
    ) -> Result<Self, ObservedImportError> {
        context
            .checkpoint()
            .map_err(ObservedImportError::Observation)?;
        let source = source.into_source();
        let observation = ParserObservation::new(context);
        observation.finish(observation.reserve(STATE_BYTES, &source.id))?;
        Self::begin_owned(source, limits, &observation, true)
    }
    fn begin_owned(
        source: DefinitionSource,
        limits: ParseLimits,
        observation: &ParserObservation<'_, '_>,
        retain_input: bool,
    ) -> Result<Self, ObservedImportError> {
        let holder = observation.finish(DocumentInputs::try_new(source, |source| {
            if retain_input {
                DocumentCursor::new_with_retained_input(source, observation, limits)
            } else {
                DocumentCursor::new(source, observation, limits)
            }
        }))?;
        Ok(Self {
            holder,
            failure: None,
        })
    }
    /// Validate one retained input chunk or parse one event, preserving the
    /// original diagnostics and source spans.
    /// Every error is terminal; replacing the observer cannot resume failed work.
    pub fn advance(
        &mut self,
        context: &crate::definitions::conversion::InvocationContext<'_>,
    ) -> Result<bool, ObservedImportError> {
        if let Some(failure) = &self.failure {
            return Err(failure.clone());
        }
        let observation = ParserObservation::new(context);
        let result = context
            .checkpoint()
            .map_err(ObservedImportError::Observation)
            .and_then(|()| {
                observation.finish(
                    self.holder
                        .with_dependent_mut(|_, cursor| cursor.advance(&observation)),
                )
            });
        if let Err(failure) = &result {
            self.failure = Some(failure.clone());
        }
        result
    }
    /// Transfer the exact admitted source and complete parsed tree. Partial or
    /// failed parsing cannot publish a document. Cleanup is still synchronous.
    pub fn finish(mut self) -> Result<Document, ObservedImportError> {
        if let Some(failure) = self.failure {
            return Err(failure);
        }
        let (root, misc) = self
            .holder
            .with_dependent_mut(|_, cursor| cursor.take_output())
            .map_err(ObservedImportError::Import)?;
        Ok(Document {
            source: self.holder.into_owner(),
            root,
            misc,
        })
    }
}

#[cfg(test)]
#[path = "ecore_document_preparation_tests.rs"]
mod tests;
