//! Source syntax decoding. The ordinary facade drains this same continuation.
//! Metadata lookup/copy, checked resolution, allocation and cleanup remain tails.
use super::*;

const CHUNK: usize = 256;

/// Payload validation and copying share the original order and grouped fees.
/// The original `string(len)` admits the sole destination before its growth.
struct PayloadPreparation<'a> {
    node: &'a Node,
    phase: PayloadPhase<'a>,
}
enum PayloadPhase<'a> {
    Begin,
    Visit {
        text: &'a String,
        index: usize,
        offset: usize,
    },
    Copy {
        text: &'a String,
        value: String,
        offset: usize,
    },
    Complete(String),
    Failed,
}
impl<'a> PayloadPreparation<'a> {
    fn begin(node: &'a Node) -> Self {
        Self {
            node,
            phase: PayloadPhase::Begin,
        }
    }
    fn advance(
        &mut self,
        resources: &'a ResourceSet,
        observation: &ParserObservation<'_, '_>,
    ) -> Result<bool, ImportError> {
        let phase = std::mem::replace(&mut self.phase, PayloadPhase::Failed);
        self.phase = match phase {
            PayloadPhase::Begin => {
                let text = &resources
                    .document(&self.node.span.source)
                    .map_err(|e| error(&self.node.span, e))?
                    .source
                    .text;
                PayloadPhase::Visit {
                    text,
                    index: 0,
                    offset: 0,
                }
            }
            PayloadPhase::Visit {
                text,
                index,
                mut offset,
            } => {
                if let Some(content) = self.node.payloads.get(index) {
                    if offset == 0 {
                        observation.work(1, &self.node.span.source)?;
                        observation.work(
                            content
                                .span
                                .end
                                .saturating_sub(content.span.start)
                                .div_ceil(CHUNK)
                                .max(1),
                            &self.node.span.source,
                        )?;
                    }
                    let unsupported = || {
                        ImportError::new(
                            ErrorKind::Unsupported,
                            &content.span,
                            "Unsupported content in generic signature",
                        )
                    };
                    match content.kind {
                        PayloadKind::Comment => PayloadPhase::Visit {
                            text,
                            index: index + 1,
                            offset: 0,
                        },
                        PayloadKind::Text => {
                            let value = text
                                .get(content.span.start..content.span.end)
                                .ok_or_else(unsupported)?;
                            let mut count = 0;
                            for character in value[offset..].chars() {
                                if !character.is_whitespace() {
                                    return Err(unsupported());
                                }
                                offset += character.len_utf8();
                                count += 1;
                                if count == CHUNK {
                                    break;
                                }
                            }
                            if offset == value.len() {
                                PayloadPhase::Visit {
                                    text,
                                    index: index + 1,
                                    offset: 0,
                                }
                            } else {
                                PayloadPhase::Visit {
                                    text,
                                    index,
                                    offset,
                                }
                            }
                        }
                        _ => return Err(unsupported()),
                    }
                } else {
                    let value = text
                        .get(self.node.span.start..self.node.span.end)
                        .ok_or_else(|| error(&self.node.span, "Invalid generic source span"))?;
                    observation.work(value.len().div_ceil(CHUNK).max(1), &self.node.span.source)?;
                    observation.string(value.len(), &self.node.span.source)?;
                    PayloadPhase::Copy {
                        text,
                        value: String::with_capacity(value.len()),
                        offset: 0,
                    }
                }
            }
            PayloadPhase::Copy {
                text,
                mut value,
                offset,
            } => {
                let source = &text[self.node.span.start..self.node.span.end];
                let end = byte_end(source, offset);
                value.push_str(&source[offset..end]);
                if end == source.len() {
                    PayloadPhase::Complete(value)
                } else {
                    PayloadPhase::Copy {
                        text,
                        value,
                        offset: end,
                    }
                }
            }
            PayloadPhase::Complete(value) => PayloadPhase::Complete(value),
            PayloadPhase::Failed => {
                return Err(error(
                    &self.node.span,
                    "Generic payload decoding is incomplete or failed",
                ));
            }
        };
        Ok(matches!(self.phase, PayloadPhase::Complete(_)))
    }
    fn finish(self) -> Result<String, ImportError> {
        match self.phase {
            PayloadPhase::Complete(value) => Ok(value),
            _ => Err(error(
                &self.node.span,
                "Generic payload decoding is incomplete or failed",
            )),
        }
    }
}
fn byte_end(value: &str, offset: usize) -> usize {
    let mut end = offset.saturating_add(CHUNK).min(value.len());
    while !value.is_char_boundary(end) {
        end -= 1;
    }
    end
}
pub(super) fn payload(
    resources: &ResourceSet,
    node: &Node,
    observation: &ParserObservation<'_, '_>,
) -> Result<String, ImportError> {
    let mut preparation = PayloadPreparation::begin(node);
    while !preparation.advance(resources, observation)? {}
    preparation.finish()
}

// Exactly the old payload string header allowance. The copied payload is moved
// into this record; its bytes and this64 header are not charged twice.
struct Frame {
    payload: String,
    arguments: Vec<GenericType>,
    next_child: usize,
    slot_start: usize,
}
// Reference128 admits one TypeReference112 plus discriminant. The Child record
// moves the sole fresh ID from the old serialized(parent)+slot32 transition.
// Bound slots move the sole old Box admitted by bound128. No extra Box exists.
enum Slot<'a> {
    Classifier(TypeReference),
    Parameter(TypeReference),
    Upper(Box<GenericType>),
    Lower(Box<GenericType>),
    Child {
        id: DeclarationId,
        node: Option<&'a Node>,
    },
}
#[derive(Clone, Copy)]
enum Role {
    Classifier,
    Parameter,
}
impl Role {
    fn name(self) -> &'static str {
        match self {
            Self::Classifier => "eClassifier",
            Self::Parameter => "eTypeParameter",
        }
    }
    fn parameter(self) -> bool {
        matches!(self, Self::Parameter)
    }
}
enum Phase<'a> {
    Begin,
    Attributes {
        node: &'a Node,
        next: usize,
    },
    PayloadBuild(PayloadPreparation<'a>),
    PayloadDiscard(PayloadPreparation<'a>),
    Attribute {
        role: Role,
        next: usize,
    },
    Tokens {
        attribute: &'a Attribute,
        offset: usize,
        first: [usize; 2],
        second: [usize; 2],
        flags: u8,
    },
    ReferenceChildren {
        role: Role,
        next: usize,
    },
    ChildAttributes {
        role: Role,
        next: usize,
        attribute: usize,
    },
    ChildHref {
        role: Role,
        next: usize,
        attribute: usize,
    },
    ValidateHref {
        role: Role,
        next: usize,
        href: &'a Attribute,
        offset: usize,
    },
    ChildHint {
        role: Role,
        next: usize,
        href: &'a Attribute,
        attribute: usize,
    },
    CopyReference {
        role: Role,
        attribute: &'a Attribute,
        start: usize,
        end: usize,
        offset: usize,
        next: usize,
    },
    Children,
    Complete,
    Failed,
}
// A consumed node's already-admitted components; no new representation or Box.
type CompletedParts = (
    Frame,
    Option<(Role, TypeReference)>,
    Option<Box<GenericType>>,
    Option<Box<GenericType>>,
);
/// No owned selection, observer, registry or self-address is stored. Root IDs
/// remain in the owning admitted selection slot; recursive IDs move into Slot.
pub(crate) struct GenericTypePreparation<'a> {
    root: Option<&'a Node>,
    frames: Vec<Frame>,
    slots: Vec<Slot<'a>>,
    phase: Phase<'a>,
}
const _: () = assert!(std::mem::size_of::<Frame>() <= 64);
const _: () = assert!(std::mem::size_of::<Slot<'_>>() <= 128);
const _: () = assert!(std::mem::size_of::<GenericTypePreparation<'_>>() <= 128);

impl<'a> GenericTypePreparation<'a> {
    pub(crate) fn begin() -> Self {
        Self {
            root: None,
            frames: vec![],
            slots: vec![],
            phase: Phase::Begin,
        }
    }
    fn node(&self, root: &DeclarationId) -> Result<&'a Node, ImportError> {
        if let Some(frame) = self.frames.last()
            && frame.slot_start > 0
            && let Slot::Child {
                node: Some(node), ..
            } = &self.slots[frame.slot_start - 1]
        {
            return Ok(node);
        }
        self.root
            .ok_or_else(|| internal(root, "Missing generic decoder node"))
    }
    fn id<'b>(&'b self, root: &'b DeclarationId) -> &'b DeclarationId {
        if let Some(frame) = self.frames.last()
            && frame.slot_start > 0
            && let Slot::Child { id, .. } = &self.slots[frame.slot_start - 1]
        {
            return id;
        }
        root
    }
    fn reference(&self, role: Role) -> bool {
        let start = self.frames.last().map_or(0, |f| f.slot_start);
        self.slots[start..].iter().any(|s| {
            matches!(
                (s, role),
                (Slot::Classifier(_), Role::Classifier) | (Slot::Parameter(_), Role::Parameter)
            )
        })
    }
    fn push_reference(
        &mut self,
        root: &DeclarationId,
        resume: (Role, usize),
        a: &'a Attribute,
        range: [usize; 2],
        kind_hint: Option<String>,
        observation: &ParserObservation<'_, '_>,
    ) -> Result<Phase<'a>, ImportError> {
        let (role, next) = resume;
        let [start, end] = range;
        let node = self.node(root)?;
        observation.string(end - start, &node.span.source)?;
        observation.serialized(&a.span, &node.span.source)?;
        observation.reserve(128, &node.span.source)?;
        let reference = TypeReference {
            href: String::with_capacity(end - start),
            kind_hint,
            span: a.span.clone(),
        };
        self.slots.push(match role {
            Role::Classifier => Slot::Classifier(reference),
            Role::Parameter => Slot::Parameter(reference),
        });
        Ok(Phase::CopyReference {
            role,
            attribute: a,
            start,
            end,
            offset: start,
            next,
        })
    }
    /// One source scan chunk or structural transition. Errors poison before
    /// returning; no result can be finished after a refused original callback.
    pub(crate) fn advance(
        &mut self,
        resources: &'a ResourceSet,
        root: &DeclarationId,
        observation: &ParserObservation<'_, '_>,
    ) -> Result<bool, ImportError> {
        let phase = std::mem::replace(&mut self.phase, Phase::Failed);
        self.phase = self.step(phase, resources, root, observation)?;
        Ok(matches!(self.phase, Phase::Complete))
    }
    fn step(
        &mut self,
        phase: Phase<'a>,
        resources: &'a ResourceSet,
        root: &DeclarationId,
        o: &ParserObservation<'_, '_>,
    ) -> Result<Phase<'a>, ImportError> {
        Ok(match phase {
            Phase::Begin => {
                let id = match self.slots.last() {
                    Some(Slot::Child { id, .. }) => id,
                    _ => root,
                };
                o.work(1, &id.source)?;
                let n = node(resources, id, o)?;
                if !type_position(n) {
                    return Err(error(
                        &n.opening,
                        "Selection is not a generic type position",
                    ));
                }
                if let Some(Slot::Child { node, .. }) = self.slots.last_mut() {
                    *node = Some(n);
                } else {
                    self.root = Some(n);
                }
                Phase::Attributes { node: n, next: 0 }
            }
            Phase::Attributes { node, next } => {
                if let Some(a) = node.attributes.get(next) {
                    o.work(1, &node.span.source)?;
                    check_attribute(a, &["eClassifier", "eTypeParameter"], "EGenericType")?;
                    Phase::Attributes {
                        node,
                        next: next + 1,
                    }
                } else {
                    Phase::PayloadBuild(PayloadPreparation::begin(node))
                }
            }
            Phase::PayloadBuild(mut p) => {
                if p.advance(resources, o)? {
                    // No frame exists before the original payload string64 fee.
                    self.frames.push(Frame {
                        payload: p.finish()?,
                        arguments: vec![],
                        next_child: 0,
                        slot_start: self.slots.len(),
                    });
                    Phase::Attribute {
                        role: Role::Classifier,
                        next: 0,
                    }
                } else {
                    Phase::PayloadBuild(p)
                }
            }
            Phase::PayloadDiscard(mut p) => {
                if p.advance(resources, o)? {
                    drop(p.finish()?);
                    Phase::Children
                } else {
                    Phase::PayloadDiscard(p)
                }
            }
            Phase::Attribute { role, next } => {
                let n = self.node(root)?;
                if let Some(a) = n.attributes.get(next) {
                    o.work(1, &n.span.source)?;
                    if a.name.namespace.is_none() && a.name.local == role.name() {
                        o.work(a.value.len().div_ceil(CHUNK).max(1), &n.span.source)?;
                        Phase::Tokens {
                            attribute: a,
                            offset: 0,
                            first: [0; 2],
                            second: [0; 2],
                            flags: u8::from(role.parameter()) << 4,
                        }
                    } else {
                        Phase::Attribute {
                            role,
                            next: next + 1,
                        }
                    }
                } else {
                    Phase::ReferenceChildren { role, next: 0 }
                }
            }
            Phase::Tokens {
                attribute: a,
                mut offset,
                mut first,
                mut second,
                mut flags,
            } => {
                let mut count = flags & 3;
                let mut inside = flags & 4 != 0;
                for ch in a.value[offset..].chars().take(CHUNK) {
                    if ch.is_whitespace() {
                        inside = false;
                    } else {
                        if !inside {
                            count += 1;
                            if count > 2 {
                                return Err(error(
                                    &a.span,
                                    "Generic reference requires exactly one URI",
                                ));
                            }
                            if count == 1 {
                                first[0] = offset;
                            } else {
                                second[0] = offset;
                            }
                        }
                        if count == 1 {
                            first[1] = offset + ch.len_utf8();
                        } else {
                            second[1] = offset + ch.len_utf8();
                        }
                        inside = true;
                    }
                    offset += ch.len_utf8();
                }
                if offset < a.value.len() {
                    flags = (flags & 16) | count | if inside { 4 } else { 0 };
                    Phase::Tokens {
                        attribute: a,
                        offset,
                        first,
                        second,
                        flags,
                    }
                } else {
                    let role = if flags & 16 == 0 {
                        Role::Classifier
                    } else {
                        Role::Parameter
                    };
                    let (range, kind_hint) = match count {
                        1 => (first, None),
                        2 => (
                            second,
                            Some(hint(a, role.parameter(), o)?.ok_or_else(|| {
                                error(&a.span, "Missing expanded generic kind hint")
                            })?),
                        ),
                        _ => {
                            return Err(error(
                                &a.span,
                                "Generic reference requires exactly one URI",
                            ));
                        }
                    };
                    self.push_reference(root, (role, 0), a, range, kind_hint, o)?
                }
            }
            Phase::ReferenceChildren { role, next } => {
                let n = self.node(root)?;
                if let Some(child) = n.children.get(next) {
                    o.work(1, &n.span.source)?;
                    if child.name.namespace.is_some() || child.name.local != role.name() {
                        Phase::ReferenceChildren {
                            role,
                            next: next + 1,
                        }
                    } else {
                        if self.reference(role) {
                            return Err(error(&child.opening, "Duplicate generic reference"));
                        }
                        Phase::ChildAttributes {
                            role,
                            next,
                            attribute: 0,
                        }
                    }
                } else {
                    match role {
                        Role::Classifier => Phase::Attribute {
                            role: Role::Parameter,
                            next: 0,
                        },
                        Role::Parameter => Phase::Children,
                    }
                }
            }
            Phase::ChildAttributes {
                role,
                next,
                attribute,
            } => {
                let n = self.node(root)?;
                let child = &n.children[next];
                if let Some(a) = child.attributes.get(attribute) {
                    o.work(1, &n.span.source)?;
                    if !((a.name.namespace.is_none() && a.name.local == "href")
                        || (a.name.namespace.as_deref() == Some(XSI) && a.name.local == "type")
                        || a.name.namespace.as_deref() == Some("http://www.w3.org/2000/xmlns/"))
                    {
                        return Err(ImportError::new(
                            ErrorKind::Unsupported,
                            &a.span,
                            "Unsupported generic href attribute",
                        ));
                    }
                    Phase::ChildAttributes {
                        role,
                        next,
                        attribute: attribute + 1,
                    }
                } else {
                    if !child.children.is_empty() {
                        return Err(error(
                            &child.opening,
                            "Generic reference requires href, not an inline declaration",
                        ));
                    }
                    Phase::ChildHref {
                        role,
                        next,
                        attribute: 0,
                    }
                }
            }
            Phase::ChildHref {
                role,
                next,
                attribute,
            } => {
                let n = self.node(root)?;
                let child = &n.children[next];
                let a = child
                    .attributes
                    .get(attribute)
                    .ok_or_else(|| error(&child.opening, "Missing generic href"))?;
                o.work(1, &child.span.source)?;
                if a.name.namespace.is_none() && a.name.local == "href" {
                    if a.value.is_empty() {
                        return Err(error(&a.span, "Invalid generic href"));
                    }
                    Phase::ValidateHref {
                        role,
                        next,
                        href: a,
                        offset: 0,
                    }
                } else {
                    Phase::ChildHref {
                        role,
                        next,
                        attribute: attribute + 1,
                    }
                }
            }
            Phase::ValidateHref {
                role,
                next,
                href,
                mut offset,
            } => {
                let n = self.node(root)?;
                for ch in href.value[offset..].chars().take(CHUNK) {
                    o.work(1, &n.span.source)?;
                    if ch.is_whitespace() {
                        return Err(error(&href.span, "Invalid generic href"));
                    }
                    offset += ch.len_utf8();
                }
                if offset < href.value.len() {
                    Phase::ValidateHref {
                        role,
                        next,
                        href,
                        offset,
                    }
                } else {
                    Phase::ChildHint {
                        role,
                        next,
                        href,
                        attribute: 0,
                    }
                }
            }
            Phase::ChildHint {
                role,
                next,
                href,
                attribute,
            } => {
                let n = self.node(root)?;
                let child = &n.children[next];
                if let Some(a) = child.attributes.get(attribute) {
                    o.work(1, &n.span.source)?;
                    if a.name.namespace.as_deref() == Some(XSI) && a.name.local == "type" {
                        let kind_hint = hint(a, role.parameter(), o)?;
                        self.push_reference(
                            root,
                            (role, next + 1),
                            href,
                            [0, href.value.len()],
                            kind_hint,
                            o,
                        )?
                    } else {
                        Phase::ChildHint {
                            role,
                            next,
                            href,
                            attribute: attribute + 1,
                        }
                    }
                } else {
                    self.push_reference(
                        root,
                        (role, next + 1),
                        href,
                        [0, href.value.len()],
                        None,
                        o,
                    )?
                }
            }
            Phase::CopyReference {
                role,
                attribute,
                start,
                end,
                offset,
                next,
            } => {
                let stop = byte_end(&attribute.value[..end], offset);
                let r = match self.slots.last_mut() {
                    Some(Slot::Classifier(r) | Slot::Parameter(r)) => r,
                    _ => return Err(internal(root, "Missing admitted generic reference")),
                };
                r.href.push_str(&attribute.value[offset..stop]);
                if stop == end {
                    Phase::ReferenceChildren { role, next }
                } else {
                    Phase::CopyReference {
                        role,
                        attribute,
                        start,
                        end,
                        offset: stop,
                        next,
                    }
                }
            }
            Phase::Children => {
                let n = self.node(root)?;
                let frame = self
                    .frames
                    .last_mut()
                    .ok_or_else(|| internal(root, "Missing admitted generic frame"))?;
                let index = frame.next_child;
                if let Some(child_node) = n.children.get(index) {
                    o.work(1, &self.id(root).source)?;
                    if child_node.name.namespace.is_some() {
                        return Err(error(
                            &child_node.opening,
                            "Generic child must use an Ecore containment feature",
                        ));
                    }
                    match child_node.name.local.as_str() {
                        "eClassifier" | "eTypeParameter" => {
                            self.frames
                                .last_mut()
                                .ok_or_else(|| internal(root, "Missing generic frame"))?
                                .next_child += 1;
                            Phase::PayloadDiscard(PayloadPreparation::begin(child_node))
                        }
                        "eTypeArguments" | "eUpperBound" | "eLowerBound" => {
                            let duplicate = self.slots[self
                                .frames
                                .last()
                                .ok_or_else(|| internal(root, "Missing generic frame"))?
                                .slot_start..]
                                .iter()
                                .any(|s| {
                                    matches!(
                                        (s, child_node.name.local.as_str()),
                                        (Slot::Upper(_), "eUpperBound")
                                            | (Slot::Lower(_), "eLowerBound")
                                    )
                                });
                            if duplicate {
                                return Err(error(&child_node.opening, "Duplicate generic bound"));
                            }
                            let id = child(self.id(root), index, o)?;
                            self.frames
                                .last_mut()
                                .ok_or_else(|| internal(root, "Missing generic frame"))?
                                .next_child += 1;
                            self.slots.push(Slot::Child { id, node: None });
                            Phase::Begin
                        }
                        _ => {
                            return Err(ImportError::new(
                                ErrorKind::Unsupported,
                                &child_node.opening,
                                "Unsupported generic child",
                            ));
                        }
                    }
                } else {
                    self.validate(root, o)?;
                    if self.frames.len() == 1 {
                        Phase::Complete
                    } else {
                        // Consume the sole child frame/ID before insertion. Original
                        // parent slot/Box admission remains after child completion.
                        let (frame, reference, upper, lower) = self.take_parts(root)?;
                        let (id, child_node) = match self.slots.pop() {
                            Some(Slot::Child {
                                id,
                                node: Some(node),
                            }) => (id, node),
                            _ => return Err(internal(root, "Missing generic child selection")),
                        };
                        let value = construct(frame, reference, upper, lower, id, child_node)?;
                        let source = &self.id(root).source;
                        if child_node.name.local == "eTypeArguments" {
                            o.reserve(32, source)?;
                            self.frames
                                .last_mut()
                                .ok_or_else(|| internal(root, "Missing generic parent"))?
                                .arguments
                                .push(value);
                        } else {
                            o.reserve(128, source)?;
                            let bound = Box::new(value);
                            self.slots.push(if child_node.name.local == "eUpperBound" {
                                Slot::Upper(bound)
                            } else {
                                Slot::Lower(bound)
                            });
                        }
                        Phase::Children
                    }
                }
            }
            Phase::Complete => Phase::Complete,
            Phase::Failed => {
                return Err(internal(
                    root,
                    "Generic source decoding is incomplete or failed",
                ));
            }
        })
    }
    fn validate(
        &self,
        root: &DeclarationId,
        o: &ParserObservation<'_, '_>,
    ) -> Result<(), ImportError> {
        let node = self.node(root)?;
        let frame = self
            .frames
            .last()
            .ok_or_else(|| internal(root, "Missing generic frame"))?;
        let mut classifier = false;
        let mut parameter = false;
        let mut upper = false;
        let mut lower = false;
        for s in &self.slots[frame.slot_start..] {
            match s {
                Slot::Classifier(_) => classifier = true,
                Slot::Parameter(_) => parameter = true,
                Slot::Upper(_) => upper = true,
                Slot::Lower(_) => lower = true,
                Slot::Child { .. } => return Err(internal(root, "Unfinished generic child")),
            }
        }
        if classifier && parameter {
            return Err(error(
                &node.opening,
                "Generic type cannot select both classifier and parameter",
            ));
        }
        if upper && lower {
            return Err(error(
                &node.opening,
                "Generic wildcard cannot have both upper and lower bounds",
            ));
        }
        if (classifier || parameter) && (upper || lower) {
            return Err(error(
                &node.opening,
                "Only wildcard arguments may have upper/lower bounds",
            ));
        }
        let id = self.id(root);
        o.reserve(128, &id.source)?;
        if !classifier && !frame.arguments.is_empty() {
            return Err(error(&node.opening, "Type arguments require a classifier"));
        }
        if !classifier && !parameter && node.name.local != "eTypeArguments" {
            return Err(error(
                &node.opening,
                "Wildcard is only allowed in a type argument position",
            ));
        }
        o.serialized(id, &id.source)?;
        o.serialized(&node.span, &id.source)?;
        o.reserve(128, &id.source)?;
        Ok(())
    }
    fn take_parts(&mut self, root: &DeclarationId) -> Result<CompletedParts, ImportError> {
        let frame = self
            .frames
            .pop()
            .ok_or_else(|| internal(root, "Missing complete generic frame"))?;
        let mut reference = None;
        let mut upper = None;
        let mut lower = None;
        while self.slots.len() > frame.slot_start {
            match self.slots.pop() {
                Some(Slot::Classifier(r)) => reference = Some((Role::Classifier, r)),
                Some(Slot::Parameter(r)) => reference = Some((Role::Parameter, r)),
                Some(Slot::Upper(b)) => upper = Some(b),
                Some(Slot::Lower(b)) => lower = Some(b),
                _ => return Err(internal(root, "Invalid complete generic slots")),
            }
        }
        Ok((frame, reference, upper, lower))
    }
    pub(crate) fn finish(mut self, root: &DeclarationId) -> Result<GenericType, ImportError> {
        if !matches!(self.phase, Phase::Complete) || self.frames.len() != 1 {
            return Err(internal(
                root,
                "Generic source decoding is incomplete or failed",
            ));
        }
        let node = self.node(root)?;
        // The root clone occurs only after its original serialized ID admission.
        let (frame, reference, upper, lower) = self.take_parts(root)?;
        construct(frame, reference, upper, lower, root.clone(), node)
    }
}
fn construct(
    frame: Frame,
    reference: Option<(Role, TypeReference)>,
    upper: Option<Box<GenericType>>,
    lower: Option<Box<GenericType>>,
    declaration: DeclarationId,
    node: &Node,
) -> Result<GenericType, ImportError> {
    let value = match reference {
        Some((Role::Classifier, reference)) => GenericKind::Classifier {
            reference,
            arguments: frame.arguments,
        },
        Some((Role::Parameter, reference)) => GenericKind::Parameter { reference },
        None => GenericKind::Wildcard { upper, lower },
    };
    Ok(GenericType {
        declaration,
        span: node.span.clone(),
        source_payload: frame.payload,
        value,
    })
}
fn internal(root: &DeclarationId, message: &str) -> ImportError {
    // Lifecycle errors retain the supplied fixed source identity. 0..0 is an
    // honest fallback for an incomplete cursor with no authored token to blame.
    error(
        &SourceSpan {
            source: root.source.clone(),
            start: 0,
            end: 0,
        },
        message,
    )
}

#[cfg(test)]
pub(super) fn measured_layout() -> [usize; 5] {
    [
        std::mem::size_of::<GenericTypePreparation<'_>>(),
        std::mem::size_of::<Phase<'_>>(),
        std::mem::size_of::<Frame>(),
        std::mem::size_of::<Slot<'_>>(),
        std::mem::size_of::<PayloadPreparation<'_>>(),
    ]
}
