//! Retained legacy decoding and raw/generic agreement. Only immutable resolver
//! nodes are borrowed; movable owner identities and observers are supplied anew.
//! Individual reference/type resolution and admitted record copies remain named
//! synchronous operations. Scans do not manufacture additional observation fees.
use super::*;
use crate::definitions::ecore::{Attribute, ParserObservation};

const CHUNK: usize = 256;
const NONE: usize = usize::MAX;

struct WordScan {
    position: usize,
    start: usize,
    first_colon: usize,
    last_colon: usize,
}
struct Word {
    start: usize,
    end: usize,
    first_colon: usize,
    last_colon: usize,
}
enum WordProgress {
    More,
    End,
    Word(Word),
}
impl WordScan {
    fn at(position: usize) -> Self {
        Self {
            position,
            start: NONE,
            first_colon: NONE,
            last_colon: NONE,
        }
    }
    fn advance(&mut self, text: &str) -> WordProgress {
        for character in text[self.position..].chars().take(CHUNK) {
            let at = self.position;
            self.position += character.len_utf8();
            if character.is_whitespace() {
                if self.start != NONE {
                    return self.word(at);
                }
            } else {
                if self.start == NONE {
                    self.start = at;
                }
                if character == ':' {
                    if self.first_colon == NONE {
                        self.first_colon = at;
                    }
                    self.last_colon = at;
                }
            }
        }
        if self.position == text.len() {
            if self.start == NONE {
                WordProgress::End
            } else {
                self.word(text.len())
            }
        } else {
            WordProgress::More
        }
    }
    fn word(&mut self, end: usize) -> WordProgress {
        let word = Word {
            start: self.start,
            end,
            first_colon: self.first_colon,
            last_colon: self.last_colon,
        };
        self.start = NONE;
        self.first_colon = NONE;
        self.last_colon = NONE;
        WordProgress::Word(word)
    }
}
struct Hint<'a> {
    attribute: &'a Attribute,
    start: usize,
    end: usize,
    first_colon: usize,
    resume: usize,
    depth: usize,
    node: Option<&'a Node>,
    binding: usize,
    comparison: usize,
}
enum LegacyPhase<'a> {
    Begin,
    Attribute(usize),
    Words {
        attribute: &'a Attribute,
        scan: WordScan,
        hint: Option<String>,
    },
    Hint(Hint<'a>),
    Children(usize),
    ChildAttributes {
        child: usize,
        attribute: usize,
    },
    Payloads {
        child: usize,
        payload: usize,
    },
    Text {
        child: usize,
        payload: usize,
        text: &'a str,
        position: usize,
    },
    Href {
        child: usize,
        attribute: usize,
    },
    HrefWhitespace {
        child: usize,
        href: &'a Attribute,
        position: usize,
    },
    Kind {
        child: usize,
        href: &'a Attribute,
        attribute: usize,
    },
    Complete,
    Failed,
}
pub(in super::super) struct LegacyPreparation<'a> {
    node: Option<&'a Node>,
    result: Option<Vec<TypeReference>>,
    phase: LegacyPhase<'a>,
}
impl<'a> LegacyPreparation<'a> {
    pub(in super::super) fn begin() -> Self {
        Self {
            node: None,
            result: None,
            phase: LegacyPhase::Begin,
        }
    }
    // `owner` and `name` are fixed inputs supplied by the private driver. No
    // borrow into those movable inputs or into this partial result is retained.
    pub(in super::super) fn advance(
        &mut self,
        resolver: &'a Resolver,
        owner: &DeclarationId,
        name: &str,
        observation: &ParserObservation<'_, '_>,
    ) -> Result<bool, ImportError> {
        let phase = std::mem::replace(&mut self.phase, LegacyPhase::Failed);
        if matches!(phase, LegacyPhase::Begin) {
            self.node = Some(resolver.node_observed(owner, observation)?);
            self.phase = LegacyPhase::Attribute(0);
            return Ok(false);
        }
        let node = self
            .node
            .ok_or_else(|| error(&fallback(owner), "Missing legacy node"))?;
        self.phase = match phase {
            LegacyPhase::Attribute(index) => {
                if let Some(attribute) = node.attributes.get(index) {
                    observation.work(1, &node.span.source)?;
                    if attribute.name.namespace.is_none() && attribute.name.local == name {
                        observation
                            .work(attribute.value.len().div_ceil(256).max(1), &owner.source)?;
                        self.result = Some(vec![]);
                        LegacyPhase::Words {
                            attribute,
                            scan: WordScan::at(0),
                            hint: None,
                        }
                    } else {
                        LegacyPhase::Attribute(index + 1)
                    }
                } else {
                    LegacyPhase::Children(0)
                }
            }
            LegacyPhase::Words {
                attribute,
                mut scan,
                hint,
            } => match scan.advance(&attribute.value) {
                WordProgress::More => LegacyPhase::Words {
                    attribute,
                    scan,
                    hint,
                },
                WordProgress::End => {
                    if hint.is_some() {
                        return Err(error(
                            &attribute.span,
                            "Missing legacy reference URI after kind hint",
                        ));
                    }
                    LegacyPhase::Children(0)
                }
                WordProgress::Word(word) => {
                    if hint.is_none() {
                        observation.work(1, &owner.source)?;
                        let local_start = if word.last_colon == NONE {
                            word.start
                        } else {
                            word.last_colon + 1
                        };
                        let local = &attribute.value[local_start..word.end];
                        if matches!(local, "EClass" | "EClassifier" | "EDataType" | "EEnum") {
                            observation.work(
                                (word.end - word.start).div_ceil(256).max(1),
                                &owner.source,
                            )?;
                            self.phase = LegacyPhase::Hint(Hint {
                                attribute,
                                start: word.start,
                                end: word.end,
                                first_colon: word.first_colon,
                                resume: scan.position,
                                depth: owner.path.len(),
                                node: None,
                                binding: 0,
                                comparison: NONE,
                            });
                            return Ok(false);
                        }
                    }
                    let reference = resolver.legacy_reference_context(
                        &attribute.value[word.start..word.end],
                        hint,
                        &attribute.span,
                        &owner.source,
                        observation,
                    )?;
                    self.result
                        .as_mut()
                        .ok_or_else(|| error(&node.opening, "Missing legacy attribute result"))?
                        .push(reference);
                    LegacyPhase::Words {
                        attribute,
                        scan,
                        hint: None,
                    }
                }
            },
            LegacyPhase::Hint(mut hint) => {
                if let Some(namespace_node) = hint.node {
                    if let Some(binding) = namespace_node.attributes.get(hint.binding) {
                        if hint.comparison == NONE {
                            observation.work(1, &owner.source)?;
                            hint.comparison = 0;
                        }
                        let selected = if hint.first_colon == NONE {
                            Some(binding.lexical_name == "xmlns")
                        } else if let Some(prefix) = binding.lexical_name.strip_prefix("xmlns:") {
                            let expected =
                                &hint.attribute.value.as_bytes()[hint.start..hint.first_colon];
                            let actual = prefix.as_bytes();
                            if actual.len() != expected.len() {
                                Some(false)
                            } else {
                                let end = hint.comparison.saturating_add(CHUNK).min(actual.len());
                                if actual[hint.comparison..end] != expected[hint.comparison..end] {
                                    Some(false)
                                } else if end == actual.len() {
                                    Some(true)
                                } else {
                                    hint.comparison = end;
                                    None
                                }
                            }
                        } else {
                            Some(false)
                        };
                        match selected {
                            Some(true) => {
                                if binding.value != ECORE {
                                    return Err(error(
                                        &hint.attribute.span,
                                        "Legacy reference kind hint is not in the Ecore namespace",
                                    ));
                                }
                                let local_start = if hint.first_colon == NONE {
                                    hint.start
                                } else {
                                    hint.first_colon + 1
                                };
                                let local = &hint.attribute.value[local_start..hint.end];
                                observation.string(local.len(), &owner.source)?;
                                LegacyPhase::Words {
                                    attribute: hint.attribute,
                                    scan: WordScan::at(hint.resume),
                                    hint: Some(local.to_owned()),
                                }
                            }
                            Some(false) => {
                                hint.binding += 1;
                                hint.comparison = NONE;
                                LegacyPhase::Hint(hint)
                            }
                            None => LegacyPhase::Hint(hint),
                        }
                    } else {
                        if hint.depth == 0 {
                            return Err(error(
                                &hint.attribute.span,
                                "Unbound legacy reference kind hint",
                            ));
                        }
                        hint.depth -= 1;
                        hint.node = None;
                        hint.binding = 0;
                        hint.comparison = NONE;
                        LegacyPhase::Hint(hint)
                    }
                } else {
                    observation.work(1, &owner.source)?;
                    hint.node = Some(resolver.resources(owner).node_path_context(
                        &owner.source,
                        &owner.path[..hint.depth],
                        observation,
                    )?);
                    LegacyPhase::Hint(hint)
                }
            }
            LegacyPhase::Children(index) => {
                if let Some(child) = node.children.get(index) {
                    observation.work(1, &owner.source)?;
                    if !named(child, name) {
                        LegacyPhase::Children(index + 1)
                    } else {
                        if name == "eType" && self.result.is_some() {
                            return Err(error(&child.opening, "Duplicate legacy eType encoding"));
                        }
                        LegacyPhase::ChildAttributes {
                            child: index,
                            attribute: 0,
                        }
                    }
                } else {
                    if name == "eType" && self.result.as_ref().is_some_and(|refs| refs.len() != 1) {
                        return Err(error(
                            &node.opening,
                            "Legacy eType requires exactly one classifier",
                        ));
                    }
                    LegacyPhase::Complete
                }
            }
            LegacyPhase::ChildAttributes { child, attribute } => {
                let selected = &node.children[child];
                if let Some(value) = selected.attributes.get(attribute) {
                    observation.work(1, &owner.source)?;
                    if !((value.name.namespace.is_none() && value.name.local == "href")
                        || (value.name.namespace.as_deref() == Some(XSI)
                            && value.name.local == "type")
                        || value.namespace_declaration())
                    {
                        return Err(ImportError::new(
                            ErrorKind::Unsupported,
                            &value.span,
                            "Unsupported legacy reference attribute",
                        ));
                    }
                    LegacyPhase::ChildAttributes {
                        child,
                        attribute: attribute + 1,
                    }
                } else {
                    if !selected.children.is_empty() {
                        return Err(error(
                            &selected.opening,
                            "Legacy reference requires href, not inline content",
                        ));
                    }
                    // Preserve the original document lookup before the payload
                    // loop, including its diagnostic when that loop is empty.
                    resolver
                        .resources(owner)
                        .document(&owner.source)
                        .map_err(|e| error(&selected.span, e))?;
                    LegacyPhase::Payloads { child, payload: 0 }
                }
            }
            LegacyPhase::Payloads { child, payload } => {
                let selected = &node.children[child];
                if let Some(value) = selected.payloads.get(payload) {
                    observation.work(1, &owner.source)?;
                    if matches!(value.kind, PayloadKind::Comment) {
                        LegacyPhase::Payloads {
                            child,
                            payload: payload + 1,
                        }
                    } else if matches!(value.kind, PayloadKind::Text) {
                        let source = &resolver
                            .resources(owner)
                            .document(&owner.source)
                            .map_err(|e| error(&selected.span, e))?
                            .source
                            .text;
                        let text =
                            source
                                .get(value.span.start..value.span.end)
                                .ok_or_else(|| {
                                    ImportError::new(
                                        ErrorKind::Unsupported,
                                        &value.span,
                                        "Unsupported legacy reference content",
                                    )
                                })?;
                        observation.work(text.len().div_ceil(256).max(1), &owner.source)?;
                        LegacyPhase::Text {
                            child,
                            payload,
                            text,
                            position: 0,
                        }
                    } else {
                        return Err(ImportError::new(
                            ErrorKind::Unsupported,
                            &value.span,
                            "Unsupported legacy reference content",
                        ));
                    }
                } else {
                    LegacyPhase::Href {
                        child,
                        attribute: 0,
                    }
                }
            }
            LegacyPhase::Text {
                child,
                payload,
                text,
                mut position,
            } => {
                for character in text[position..].chars().take(CHUNK) {
                    if !character.is_whitespace() {
                        return Err(ImportError::new(
                            ErrorKind::Unsupported,
                            &node.children[child].payloads[payload].span,
                            "Unsupported legacy reference content",
                        ));
                    }
                    position += character.len_utf8();
                }
                if position == text.len() {
                    LegacyPhase::Payloads {
                        child,
                        payload: payload + 1,
                    }
                } else {
                    LegacyPhase::Text {
                        child,
                        payload,
                        text,
                        position,
                    }
                }
            }
            LegacyPhase::Href { child, attribute } => {
                let selected = &node.children[child];
                if let Some(href) = selected.attributes.get(attribute) {
                    observation.work(1, &selected.span.source)?;
                    if href.name.namespace.is_none() && href.name.local == "href" {
                        observation.work(href.value.len().div_ceil(256).max(1), &owner.source)?;
                        if href.value.is_empty() {
                            return Err(error(&href.span, "Invalid legacy href"));
                        }
                        LegacyPhase::HrefWhitespace {
                            child,
                            href,
                            position: 0,
                        }
                    } else {
                        LegacyPhase::Href {
                            child,
                            attribute: attribute + 1,
                        }
                    }
                } else {
                    return Err(error(&selected.opening, "Missing legacy href"));
                }
            }
            LegacyPhase::HrefWhitespace {
                child,
                href,
                mut position,
            } => {
                for character in href.value[position..].chars().take(CHUNK) {
                    if character.is_whitespace() {
                        return Err(error(&href.span, "Invalid legacy href"));
                    }
                    position += character.len_utf8();
                }
                if position == href.value.len() {
                    LegacyPhase::Kind {
                        child,
                        href,
                        attribute: 0,
                    }
                } else {
                    LegacyPhase::HrefWhitespace {
                        child,
                        href,
                        position,
                    }
                }
            }
            LegacyPhase::Kind {
                child,
                href,
                attribute,
            } => {
                let selected = &node.children[child];
                let mut hint = None;
                if let Some(value) = selected.attributes.get(attribute) {
                    observation.work(1, &selected.span.source)?;
                    if value.name.namespace.as_deref() != Some(XSI) || value.name.local != "type" {
                        self.phase = LegacyPhase::Kind {
                            child,
                            href,
                            attribute: attribute + 1,
                        };
                        return Ok(false);
                    }
                    let expanded = value
                        .type_name
                        .as_ref()
                        .filter(|name| name.namespace.as_deref() == Some(ECORE))
                        .ok_or_else(|| {
                            error(
                                &value.span,
                                "Legacy reference kind hint is not in the Ecore namespace",
                            )
                        })?;
                    observation.string(expanded.local.len(), &owner.source)?;
                    hint = Some(expanded.local.clone());
                }
                let reference = resolver.legacy_reference_context(
                    &href.value,
                    hint,
                    &href.span,
                    &owner.source,
                    observation,
                )?;
                self.result.get_or_insert_with(Vec::new).push(reference);
                LegacyPhase::Children(child + 1)
            }
            LegacyPhase::Complete => LegacyPhase::Complete,
            LegacyPhase::Begin | LegacyPhase::Failed => {
                return Err(error(&node.opening, "Legacy preparation has failed"));
            }
        };
        Ok(matches!(self.phase, LegacyPhase::Complete))
    }
    pub(in super::super) fn finish(
        self,
        owner: &DeclarationId,
    ) -> Result<Option<Vec<TypeReference>>, ImportError> {
        if matches!(self.phase, LegacyPhase::Complete) {
            Ok(self.result)
        } else {
            Err(error(&fallback(owner), "Legacy preparation is incomplete"))
        }
    }
}

// The selecting generic-child transition already serialized its parent ID.
// Its distinct DeclarationId-root128 + nested SourceId128 record credits own
// exactly this moved selection+cursor entry. Path/string allocations keep their
// separate original admissions. No published owner/result record uses these credits.
enum GenericInput<'a> {
    Parsing {
        id: DeclarationId,
        preparation: crate::definitions::generics::GenericTypePreparation<'a>,
    },
    Complete(DeclarationId),
}
const _: () = assert!(std::mem::size_of::<GenericInput<'_>>() <= 256);
struct Projection<'a> {
    legacy: Option<Vec<TypeReference>>,
    legacy_targets: Option<Vec<DeclarationId>>,
    generic: Vec<GenericInput<'a>>,
    generic_targets: Vec<DeclarationId>,
}
struct CheckedProjection<'a> {
    legacy: Option<Vec<TypeReference>>,
    generic: Vec<GenericInput<'a>>,
    raw_types: Vec<DeclarationId>,
}
impl<'a> Projection<'a> {
    fn checked(self) -> CheckedProjection<'a> {
        let raw_types = if self.generic.is_empty() {
            self.legacy_targets.unwrap_or_default()
        } else {
            self.generic_targets
        };
        CheckedProjection {
            legacy: self.legacy,
            generic: self.generic,
            raw_types,
        }
    }
}
#[derive(Clone, Copy)]
enum Projections {
    None,
    Feature,
    Operation,
    Class,
}
impl Projections {
    fn at(self, index: usize) -> Option<(&'static str, &'static str)> {
        match (self, index) {
            (Self::Feature | Self::Operation, 0) => Some(("eType", "eGenericType")),
            (Self::Operation, 1) => Some(("eExceptions", "eGenericExceptions")),
            (Self::Class, 0) => Some(("eSuperTypes", "eGenericSuperTypes")),
            _ => None,
        }
    }
}
// The pair's original work fee is admitted once (part=0), before any bytes or
// path elements are compared. Comparison storage is an inline position only.
struct IdentityScan {
    part: u8,
    position: usize,
}
enum AgreementPhase<'a> {
    Begin,
    Projection,
    Legacy(LegacyPreparation<'a>),
    Targets {
        legacy: Option<Vec<TypeReference>>,
        targets: Vec<DeclarationId>,
        index: usize,
    },
    Generic {
        row: Projection<'a>,
        index: usize,
    },
    Compare {
        row: Projection<'a>,
        index: usize,
        scan: IdentityScan,
    },
    Validate {
        row: CheckedProjection<'a>,
        index: usize,
    },
    Publish(CheckedProjection<'a>),
    Convert {
        legacy: Option<Vec<TypeReference>>,
        pending: std::vec::IntoIter<GenericInput<'a>>,
        generic: Vec<DeclarationId>,
        raw_types: Vec<DeclarationId>,
    },
    Complete,
    Failed,
}
pub(crate) struct AgreementPreparation<'a> {
    node: Option<&'a Node>,
    result: Vec<RawSignature>,
    projections: Projections,
    projection: u8,
    retain_source_decoding: bool,
    phase: AgreementPhase<'a>,
}
impl<'a> AgreementPreparation<'a> {
    pub(crate) fn begin() -> Self {
        Self::begin_with_result_storage(Vec::new())
    }
    pub(crate) fn begin_with_result_storage(result: Vec<RawSignature>) -> Self {
        Self {
            node: None,
            result,
            projections: Projections::None,
            projection: 0,
            retain_source_decoding: false,
            phase: AgreementPhase::Begin,
        }
    }
    pub(crate) fn begin_with_retained_source_decoding() -> Self {
        let mut result = Self::begin();
        result.retain_source_decoding = true;
        result
    }
    pub(crate) fn begin_with_retained_source_decoding_and_result_storage(
        result: Vec<RawSignature>,
    ) -> Self {
        let mut preparation = Self::begin_with_result_storage(result);
        preparation.retain_source_decoding = true;
        preparation
    }
    pub(crate) fn is_source_decoding_step(&self) -> bool {
        matches!(&self.phase, AgreementPhase::Generic { row, .. } if matches!(row.generic.last(), Some(GenericInput::Parsing { .. })))
            || matches!(self.phase, AgreementPhase::Convert { .. })
    }
    #[cfg(test)]
    pub(crate) fn measured_decoder_entry_layout() -> [usize; 3] {
        [
            std::mem::size_of::<GenericInput<'_>>(),
            std::mem::size_of::<Projection<'_>>(),
            std::mem::size_of::<Self>(),
        ]
    }
    pub(crate) fn prepared_node(&self) -> Option<&'a Node> {
        self.node
    }
    pub(crate) fn advance(
        &mut self,
        resolver: &'a Resolver,
        owner: &DeclarationId,
        observation: &ParserObservation<'_, '_>,
    ) -> Result<bool, ImportError> {
        let mut done = self.advance_inner(resolver, owner, observation)?;
        if !self.retain_source_decoding {
            while self.is_source_decoding_step() {
                done = self.advance_inner(resolver, owner, observation)?;
            }
        }
        Ok(done)
    }
    fn advance_inner(
        &mut self,
        resolver: &'a Resolver,
        owner: &DeclarationId,
        observation: &ParserObservation<'_, '_>,
    ) -> Result<bool, ImportError> {
        let phase = std::mem::replace(&mut self.phase, AgreementPhase::Failed);
        if matches!(phase, AgreementPhase::Begin) {
            let node = resolver.node_observed(owner, observation)?;
            self.node = Some(node);
            self.projections = if named(node, "eStructuralFeatures") {
                if !matches!(
                    kind_observed(node, observation)?,
                    "EAttribute" | "EReference"
                ) || resolver
                    .classifier_observed(&parent_observed(owner, observation)?, observation)?
                    != TargetKind::Class
                {
                    return Err(error(&node.opening, "Invalid typed feature owner"));
                }
                Projections::Feature
            } else if named(node, "eParameters") {
                resolver.operation_observed(&parent_observed(owner, observation)?, observation)?;
                Projections::Feature
            } else if named(node, "eOperations") {
                resolver.operation_observed(owner, observation)?;
                Projections::Operation
            } else if named(node, "eClassifiers")
                && resolver.classifier_observed(owner, observation)? == TargetKind::Class
            {
                Projections::Class
            } else {
                Projections::None
            };
            self.phase = AgreementPhase::Projection;
            return Ok(false);
        }
        let node = self
            .node
            .ok_or_else(|| error(&fallback(owner), "Missing agreement node"))?;
        if matches!(phase, AgreementPhase::Complete) {
            self.phase = phase;
            return Ok(true);
        }
        let Some((feature, generic_name)) = self.projections.at(self.projection as usize) else {
            if matches!(phase, AgreementPhase::Projection) {
                self.phase = AgreementPhase::Complete;
                return Ok(true);
            }
            return Err(error(&node.opening, "Missing agreement projection"));
        };
        self.phase = match phase {
            AgreementPhase::Projection => {
                observation.work(1, &owner.source)?;
                AgreementPhase::Legacy(LegacyPreparation::begin())
            }
            AgreementPhase::Legacy(mut preparation) => {
                if preparation.advance(resolver, owner, feature, observation)? {
                    AgreementPhase::Targets {
                        legacy: preparation.finish(owner)?,
                        targets: vec![],
                        index: 0,
                    }
                } else {
                    AgreementPhase::Legacy(preparation)
                }
            }
            AgreementPhase::Targets {
                legacy,
                mut targets,
                index,
            } => {
                if let Some(reference) = legacy.as_ref().and_then(|refs| refs.get(index)) {
                    observation.work(1, &owner.source)?;
                    // Individual checked reference resolution remains a named
                    // synchronous boundary, using its original fees and errors.
                    let target = resolver.legacy_target_context(owner, reference, observation)?;
                    observation.reserve(32, &owner.source)?;
                    targets.push(target);
                    AgreementPhase::Targets {
                        legacy,
                        targets,
                        index: index + 1,
                    }
                } else {
                    let legacy_targets = legacy.as_ref().map(|_| targets);
                    AgreementPhase::Generic {
                        row: Projection {
                            legacy,
                            legacy_targets,
                            generic: vec![],
                            generic_targets: vec![],
                        },
                        index: 0,
                    }
                }
            }
            AgreementPhase::Generic { mut row, index } => {
                if let Some(GenericInput::Parsing { id, preparation }) = row.generic.last_mut() {
                    if preparation.advance(resolver.resources(id), id, observation)? {
                        let Some(GenericInput::Parsing { id, preparation }) = row.generic.pop()
                        else {
                            return Err(error(&node.opening, "Missing source generic input"));
                        };
                        let syntax = preparation.finish(&id)?;
                        // The source algorithm genuinely suspends; checked target,
                        // binding/formal and erasure algorithms remain named tails.
                        let checked =
                            resolver.finish_type_syntax_context(&id, syntax, observation)?;
                        observation.reserve(32 + 32, &owner.source)?;
                        row.generic.push(GenericInput::Complete(id));
                        row.generic_targets.push(checked.raw_type);
                        AgreementPhase::Generic {
                            row,
                            index: index + 1,
                        }
                    } else {
                        AgreementPhase::Generic { row, index }
                    }
                } else if let Some(child) = node.children.get(index) {
                    observation.work(1, &owner.source)?;
                    if named(child, generic_name) {
                        observation.serialized(owner, &owner.source)?;
                        observation.reserve(32, &owner.source)?;
                        let mut id = owner.clone();
                        id.path.push(index);
                        row.generic.push(GenericInput::Parsing {
                            id,
                            preparation:
                                crate::definitions::generics::GenericTypePreparation::begin(),
                        });
                        AgreementPhase::Generic { row, index }
                    } else {
                        AgreementPhase::Generic {
                            row,
                            index: index + 1,
                        }
                    }
                } else if !row.generic.is_empty() && row.legacy_targets.is_some() {
                    observation.work(1, &owner.source)?;
                    if row
                        .legacy_targets
                        .as_ref()
                        .is_none_or(|targets| targets.len() != row.generic_targets.len())
                    {
                        return Err(mismatch(node, &row.legacy, feature));
                    }
                    AgreementPhase::Compare {
                        row,
                        index: 0,
                        scan: IdentityScan {
                            part: 0,
                            position: 0,
                        },
                    }
                } else {
                    AgreementPhase::Validate {
                        row: row.checked(),
                        index: 0,
                    }
                }
            }
            AgreementPhase::Compare {
                row,
                index,
                mut scan,
            } => {
                let targets = row
                    .legacy_targets
                    .as_ref()
                    .ok_or_else(|| error(&node.opening, "Missing legacy targets"))?;
                if let Some(left) = targets.get(index) {
                    let right = row
                        .generic_targets
                        .get(index)
                        .ok_or_else(|| error(&node.opening, "Missing generic target"))?;
                    if scan.part == 0 {
                        observation.work(
                            1 + left.source.uri.len().div_ceil(256)
                                + right.source.uri.len().div_ceil(256)
                                + left.path.len()
                                + right.path.len(),
                            &owner.source,
                        )?;
                        scan.part = 1;
                    }
                    let (same, complete) = if scan.part == 3 {
                        if left.path.len() != right.path.len() {
                            (false, true)
                        } else {
                            let end = scan.position.saturating_add(CHUNK).min(left.path.len());
                            let same =
                                left.path[scan.position..end] == right.path[scan.position..end];
                            scan.position = end;
                            (same, end == left.path.len())
                        }
                    } else {
                        let (left, right) = if scan.part == 1 {
                            (left.source.uri.as_bytes(), right.source.uri.as_bytes())
                        } else {
                            (
                                left.source.sha256.as_bytes(),
                                right.source.sha256.as_bytes(),
                            )
                        };
                        if left.len() != right.len() {
                            (false, true)
                        } else {
                            let end = scan.position.saturating_add(CHUNK).min(left.len());
                            let same = left[scan.position..end] == right[scan.position..end];
                            scan.position = end;
                            (same, end == left.len())
                        }
                    };
                    if !same {
                        return Err(mismatch(node, &row.legacy, feature));
                    }
                    if complete {
                        if scan.part == 3 {
                            AgreementPhase::Compare {
                                row,
                                index: index + 1,
                                scan: IdentityScan {
                                    part: 0,
                                    position: 0,
                                },
                            }
                        } else {
                            scan.part += 1;
                            scan.position = 0;
                            AgreementPhase::Compare { row, index, scan }
                        }
                    } else {
                        AgreementPhase::Compare { row, index, scan }
                    }
                } else {
                    AgreementPhase::Validate {
                        row: row.checked(),
                        index: 0,
                    }
                }
            }
            AgreementPhase::Validate { row, index } => {
                if let Some(target) = row.raw_types.get(index) {
                    observation.work(1, &owner.source)?;
                    let class =
                        resolver.classifier_observed(target, observation)? == TargetKind::Class;
                    if (feature == "eSuperTypes" && !class)
                        || (named(node, "eStructuralFeatures")
                            && (kind_observed(node, observation)? == "EReference") != class)
                    {
                        return Err(error(
                            &node.opening,
                            "Raw classifier disagrees with structural feature or supertype kind",
                        ));
                    }
                    AgreementPhase::Validate {
                        row,
                        index: index + 1,
                    }
                } else {
                    AgreementPhase::Publish(row)
                }
            }
            AgreementPhase::Publish(row) => {
                observation.serialized(owner, &owner.source)?;
                observation.string(feature.len(), &owner.source)?;
                observation.reserve(128 + 32, &owner.source)?;
                // Inline headers stay inside the already-admitted owner envelope.
                // Each destination ID slot retains its old per-generic32 credit.
                // Publish's serialized owner credit is distinct from GenericInput's
                // child-selection credit, and funds only the sole published owner.
                AgreementPhase::Convert {
                    legacy: row.legacy,
                    pending: row.generic.into_iter(),
                    generic: Vec::new(),
                    raw_types: row.raw_types,
                }
            }
            AgreementPhase::Convert {
                legacy,
                mut pending,
                mut generic,
                raw_types,
            } => {
                if let Some(input) = pending.next() {
                    let GenericInput::Complete(id) = input else {
                        return Err(error(&node.opening, "Unfinished source generic input"));
                    };
                    generic.push(id);
                    AgreementPhase::Convert {
                        legacy,
                        pending,
                        generic,
                        raw_types,
                    }
                } else {
                    self.result.push(RawSignature {
                        owner: owner.clone(),
                        feature: feature.into(),
                        legacy,
                        generic,
                        raw_types,
                    });
                    self.projection += 1;
                    AgreementPhase::Projection
                }
            }
            AgreementPhase::Complete => AgreementPhase::Complete,
            AgreementPhase::Begin | AgreementPhase::Failed => {
                return Err(error(&node.opening, "Agreement preparation has failed"));
            }
        };
        Ok(matches!(self.phase, AgreementPhase::Complete))
    }
    pub(crate) fn finish(self, owner: &DeclarationId) -> Result<Vec<RawSignature>, ImportError> {
        if matches!(self.phase, AgreementPhase::Complete) {
            Ok(self.result)
        } else {
            Err(error(
                &fallback(owner),
                "Agreement preparation is incomplete",
            ))
        }
    }
}
fn mismatch(node: &Node, legacy: &Option<Vec<TypeReference>>, feature: &str) -> ImportError {
    let span = legacy
        .as_ref()
        .and_then(|refs| refs.first())
        .map_or(&node.opening, |reference| &reference.span);
    error(
        span,
        format!("Legacy {feature} disagrees with generic raw types or order"),
    )
}
