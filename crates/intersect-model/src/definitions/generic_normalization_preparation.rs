//! One checked-generic normalization algorithm with moved syntax records.
//! Resolution and individual metadata/allocation/cleanup remain named tails.
use super::*;
use crate::definitions::{ecore::ParserObservation, generic_resolution::ResolvedType};
pub(super) struct NormalizedBinding {
    target: DeclarationId,
    raw: bool,
}
pub(super) type BindingIndex = BTreeMap<DeclarationId, NormalizedBinding>;
#[cfg(test)]
pub(super) fn fixed_bindings(rows: Vec<Binding>) -> BindingIndex {
    rows.into_iter()
        .map(|b| {
            (
                b.usage,
                NormalizedBinding {
                    target: b.target,
                    raw: b.raw,
                },
            )
        })
        .collect()
}
pub(super) trait BindingLookup {
    fn binding(&self, usage: &DeclarationId) -> Option<(&DeclarationId, bool)>;
}
impl BindingLookup for BindingIndex {
    fn binding(&self, usage: &DeclarationId) -> Option<(&DeclarationId, bool)> {
        self.get(usage).map(|b| (&b.target, b.raw))
    }
}
#[cfg(test)]
impl BindingLookup for BTreeMap<&DeclarationId, &Binding> {
    fn binding(&self, usage: &DeclarationId) -> Option<(&DeclarationId, bool)> {
        self.get(usage).map(|b| (&b.target, b.raw))
    }
}
enum NodePhase {
    Enter(GenericKind),
    Admitted(GenericKind),
    Classifier {
        children: std::vec::IntoIter<GenericType>,
        value: Type,
    },
    Wildcard {
        bounds: [Option<Box<GenericType>>; 2],
        next: usize,
        value: Type,
    },
    Complete(Type),
    Failed,
}
struct Frame {
    declaration: DeclarationId,
    span: SourceSpan,
    _source_payload: String,
    phase: NodePhase,
}
// Production frames consume ONLY fresh resolve_type_context output. Original
// parse_type_observed admits two128-byte syntax records per node. Its terminal
// ResolvedType128 covers the moved root before first normalization admission.
// Every nonroot frame is pushed only AFTER its original normalization128 event.
// Thus a304-byte frame transfers live owned input fields within256+128 credits;
// it is not claimed to fit normalization128 alone. No GenericType/identity copy.
// Vec/BTree unused capacity, physical allocation and cleanup remain unqualified.
const _: () = assert!(std::mem::size_of::<Frame>() <= 256 + 128);
// Existing checked Binding160 moves into the index pair; original maprow128
// remains admitted before insertion. No second live owned binding/key exists.
const _: () = assert!(std::mem::size_of::<(DeclarationId, NormalizedBinding)>() <= 160);
impl Frame {
    fn new(syntax: GenericType, admitted: bool) -> Self {
        let GenericType {
            declaration,
            span,
            source_payload,
            value,
        } = syntax;
        Self {
            declaration,
            span,
            _source_payload: source_payload,
            phase: if admitted {
                NodePhase::Admitted(value)
            } else {
                NodePhase::Enter(value)
            },
        }
    }
}
fn error(span: &SourceSpan, message: &str) -> ImportError {
    ImportError::new(ErrorKind::InvalidDefinition, span, message)
}
pub(super) struct NormalizationPreparation {
    frames: Vec<Frame>,
    complete: bool,
    failed: bool,
}
impl NormalizationPreparation {
    pub(super) fn begin(syntax: GenericType) -> Self {
        Self {
            frames: vec![Frame::new(syntax, false)],
            complete: false,
            failed: false,
        }
    }
    // The root frame remains present until consuming finish; no input address,
    // observer or registry is retained, and no early depth/node limit is introduced.
    pub(super) fn span(&self) -> &SourceSpan {
        &self.frames[0].span
    }
    pub(super) fn expression(&self) -> Result<&Type, ImportError> {
        match &self.frames[0].phase {
            NodePhase::Complete(value) if self.complete => Ok(value),
            _ => Err(error(
                self.span(),
                "Normalization output is incomplete or failed",
            )),
        }
    }
    fn enter(
        &self,
        index: usize,
        kind: GenericKind,
        bindings: &impl BindingLookup,
        observation: &ParserObservation<'_, '_>,
        admitted: bool,
    ) -> Result<NodePhase, ImportError> {
        let node = &self.frames[index];
        if !admitted {
            observation.work(1, &node.span.source)?;
            observation.reserve(128, &node.span.source)?;
        }
        Ok(match kind {
            GenericKind::Classifier { arguments, .. } => {
                observation.work(1, &node.span.source)?;
                let (target, raw) = bindings
                    .binding(&node.declaration)
                    .ok_or_else(|| error(&node.span, "Missing checked generic binding"))?;
                observation.serialized(target, &node.span.source)?;
                NodePhase::Classifier {
                    children: arguments.into_iter(),
                    value: Type::Classifier {
                        declaration: target.clone(),
                        arguments: vec![],
                        raw,
                    },
                }
            }
            GenericKind::Parameter { .. } => {
                observation.work(1, &node.span.source)?;
                let (target, _) = bindings
                    .binding(&node.declaration)
                    .ok_or_else(|| error(&node.span, "Missing checked generic binding"))?;
                observation.serialized(target, &node.span.source)?;
                NodePhase::Complete(Type::Parameter {
                    declaration: target.clone(),
                })
            }
            GenericKind::Wildcard { upper, lower } => NodePhase::Wildcard {
                bounds: [upper, lower],
                next: 0,
                value: Type::Wildcard {
                    upper: None,
                    lower: None,
                },
            },
        })
    }
    fn push_child(
        &mut self,
        child: GenericType,
        observation: &ParserObservation<'_, '_>,
    ) -> Result<(), ImportError> {
        // Original parent edge work has just happened. Emit this child's original
        // node work/admission BEFORE frame growth; EnterAdmitted never repeats it.
        observation.work(1, &child.span.source)?;
        observation.reserve(128, &child.span.source)?;
        self.frames.push(Frame::new(child, true));
        Ok(())
    }
    fn return_child(
        &mut self,
        index: usize,
        observation: &ParserObservation<'_, '_>,
    ) -> Result<bool, ImportError> {
        let parent = index - 1;
        let bytes = match &self.frames[parent].phase {
            NodePhase::Classifier { .. } => 32,
            NodePhase::Wildcard { .. } => 128,
            _ => {
                return Err(error(
                    &self.frames[parent].span,
                    "Invalid normalization return phase",
                ));
            }
        };
        // Refusal leaves the completed child value INSIDE its already admitted
        // frame. After admission, consume/drop its metadata before parent insertion.
        observation.reserve(bytes, &self.frames[parent].span.source)?;
        let frame = self.frames.remove(index);
        let value = match frame.phase {
            NodePhase::Complete(value) => value,
            _ => return Err(error(&frame.span, "Missing complete normalization child")),
        };
        drop((frame.declaration, frame.span, frame._source_payload));
        match &mut self.frames[parent].phase {
            NodePhase::Classifier {
                value: Type::Classifier { arguments, .. },
                ..
            } => arguments.push(value),
            NodePhase::Wildcard {
                next,
                value: Type::Wildcard { upper, lower },
                ..
            } => {
                if *next == 1 {
                    *upper = Some(Box::new(value));
                } else {
                    *lower = Some(Box::new(value));
                }
            }
            _ => {
                return Err(error(
                    &self.frames[parent].span,
                    "Invalid normalization insertion phase",
                ));
            }
        }
        self.failed = false;
        Ok(false)
    }
    pub(super) fn advance(
        &mut self,
        bindings: &impl BindingLookup,
        observation: &ParserObservation<'_, '_>,
    ) -> Result<bool, ImportError> {
        if self.failed {
            return Err(error(self.span(), "Normalization preparation has failed"));
        }
        if self.complete {
            return Ok(true);
        }
        self.failed = true;
        let index = self.frames.len() - 1;
        if index > 0 && matches!(self.frames[index].phase, NodePhase::Complete(_)) {
            return self.return_child(index, observation);
        }
        let phase = std::mem::replace(&mut self.frames[index].phase, NodePhase::Failed);
        let next = match phase {
            NodePhase::Enter(kind) => self.enter(index, kind, bindings, observation, false)?,
            NodePhase::Admitted(kind) => self.enter(index, kind, bindings, observation, true)?,
            NodePhase::Classifier {
                mut children,
                value,
            } => {
                if let Some(child) = children.next() {
                    observation.work(1, &self.frames[index].span.source)?;
                    self.frames[index].phase = NodePhase::Classifier { children, value };
                    self.push_child(child, observation)?;
                    self.failed = false;
                    return Ok(false);
                }
                NodePhase::Complete(value)
            }
            NodePhase::Wildcard {
                mut bounds,
                next,
                value,
            } => {
                if next < 2 {
                    observation.work(1, &self.frames[index].span.source)?;
                    let child = bounds[next].take();
                    self.frames[index].phase = NodePhase::Wildcard {
                        bounds,
                        next: next + 1,
                        value,
                    };
                    if let Some(child) = child {
                        self.push_child(*child, observation)?;
                    }
                    self.failed = false;
                    return Ok(false);
                }
                NodePhase::Complete(value)
            }
            NodePhase::Complete(value) => {
                self.complete = true;
                NodePhase::Complete(value)
            }
            NodePhase::Failed => {
                return Err(error(self.span(), "Normalization preparation has failed"));
            }
        };
        self.frames[index].phase = next;
        self.failed = false;
        Ok(self.complete)
    }
    pub(super) fn finish(mut self) -> Result<TypeUse, ImportError> {
        if !self.complete || self.failed {
            return Err(error(
                self.span(),
                "Normalization preparation is incomplete or failed",
            ));
        }
        let frame = self.frames.remove(0);
        let NodePhase::Complete(expression) = frame.phase else {
            return Err(error(&frame.span, "Missing normalized expression"));
        };
        Ok(TypeUse {
            declaration: frame.declaration,
            span: frame.span,
            expression,
        })
    }
}
enum Phase {
    Index(std::vec::IntoIter<Binding>),
    Normalize,
    Shape(ShapePreparation),
    Publish,
    Complete,
    Failed,
}
pub(crate) struct NormalizedTypePreparation {
    bindings: BindingIndex,
    normalization: Option<NormalizationPreparation>,
    phase: Phase,
}
const _: () = assert!(std::mem::size_of::<NormalizedTypePreparation>() <= 144);
fn selected_error(selection: &DeclarationId, message: &str) -> ImportError {
    error(
        &SourceSpan {
            source: selection.source.clone(),
            start: 0,
            end: 0,
        },
        message,
    )
}
impl NormalizedTypePreparation {
    fn begin(checked: ResolvedType) -> Self {
        let mut result = Self {
            bindings: BindingIndex::new(),
            normalization: None,
            phase: Phase::Failed,
        };
        result.phase = result.install(checked);
        result
    }
    fn install(&mut self, checked: ResolvedType) -> Phase {
        self.normalization = Some(NormalizationPreparation::begin(checked.syntax));
        Phase::Index(checked.bindings.into_iter())
    }
    #[cfg(test)]
    pub(super) fn from_fixed(syntax: GenericType, bindings: Vec<Binding>) -> Self {
        Self {
            bindings: BindingIndex::new(),
            normalization: Some(NormalizationPreparation::begin(syntax)),
            phase: Phase::Index(bindings.into_iter()),
        }
    }
    pub(crate) fn advance(
        &mut self,
        _resolver: &Resolver,
        selection: &DeclarationId,
        observation: &ParserObservation<'_, '_>,
    ) -> Result<bool, ImportError> {
        let phase = std::mem::replace(&mut self.phase, Phase::Failed);
        self.phase = match phase {
            Phase::Index(mut pending) => {
                if let Some(binding) = pending.next() {
                    observation.work(1, &selection.source)?;
                    observation.reserve(128, &selection.source)?;
                    let Binding {
                        usage, target, raw, ..
                    } = binding;
                    self.bindings
                        .insert(usage, NormalizedBinding { target, raw });
                    Phase::Index(pending)
                } else {
                    Phase::Normalize
                }
            }
            Phase::Normalize => {
                let work = self
                    .normalization
                    .as_mut()
                    .ok_or_else(|| selected_error(selection, "Missing normalization input"))?;
                if work.advance(&self.bindings, observation)? {
                    Phase::Shape(ShapePreparation::begin())
                } else {
                    Phase::Normalize
                }
            }
            Phase::Shape(mut shape) => {
                let work = self
                    .normalization
                    .as_ref()
                    .ok_or_else(|| selected_error(selection, "Missing normalized input"))?;
                let expression = work.expression()?;
                let done = observation
                    .preparation_at(work.span(), |context| shape.advance(expression, context))
                    .map_err(|mut failure| {
                        if failure.kind == ErrorKind::InvalidDefinition {
                            failure.kind = ErrorKind::ResourceLimit;
                        }
                        failure
                    })?;
                if done {
                    observation.preparation_at(work.span(), |_| shape.finish())?;
                    Phase::Publish
                } else {
                    Phase::Shape(shape)
                }
            }
            Phase::Publish => {
                observation.reserve(128, &selection.source)?;
                Phase::Complete
            }
            Phase::Complete => Phase::Complete,
            Phase::Failed => {
                return Err(selected_error(
                    selection,
                    "Type-expression preparation has failed",
                ));
            }
        };
        Ok(matches!(self.phase, Phase::Complete))
    }
    pub(crate) fn finish(self, selection: &DeclarationId) -> Result<TypeUse, ImportError> {
        if !matches!(self.phase, Phase::Complete) {
            return Err(selected_error(
                selection,
                "Type-expression preparation is incomplete or failed",
            ));
        }
        self.normalization
            .ok_or_else(|| selected_error(selection, "Missing normalized result"))?
            .finish()
    }
}

// Source scanning and normalization are mutually exclusive. There is no second
// AST/binding index during source decoding and no retained cursor after transfer.
pub(crate) enum TypeExpressionPreparation<'a> {
    Resolve,
    ResolveRetained,
    Source(crate::definitions::generics::GenericTypePreparation<'a>),
    RetainedSource(crate::definitions::generics::GenericTypePreparation<'a>),
    Normalized(NormalizedTypePreparation),
    Failed,
}
const _: () = assert!(std::mem::size_of::<TypeExpressionPreparation<'_>>() <= 144);
impl<'a> TypeExpressionPreparation<'a> {
    pub(crate) fn begin() -> Self {
        Self::Resolve
    }
    pub(crate) fn begin_with_retained_source_decoding() -> Self {
        Self::ResolveRetained
    }
    pub(crate) fn is_retained_source_step(&self) -> bool {
        matches!(self, Self::RetainedSource(_))
    }
    #[cfg(test)]
    pub(super) fn from_fixed(syntax: GenericType, bindings: Vec<Binding>) -> Self {
        Self::Normalized(NormalizedTypePreparation::from_fixed(syntax, bindings))
    }
    pub(crate) fn advance(
        &mut self,
        resolver: &'a Resolver,
        selection: &DeclarationId,
        observation: &ParserObservation<'_, '_>,
    ) -> Result<bool, ImportError> {
        let mut state = std::mem::replace(self, Self::Failed);
        loop {
            match state {
                Self::Resolve => {
                    state =
                        Self::Source(crate::definitions::generics::GenericTypePreparation::begin())
                }
                Self::ResolveRetained => {
                    state = Self::RetainedSource(
                        crate::definitions::generics::GenericTypePreparation::begin(),
                    )
                }
                Self::Source(mut preparation) => {
                    while !preparation.advance(
                        resolver.resources(selection),
                        selection,
                        observation,
                    )? {}
                    let syntax = preparation.finish(selection)?;
                    *self = Self::Normalized(NormalizedTypePreparation::begin(
                        resolver.finish_type_syntax_context(selection, syntax, observation)?,
                    ));
                    return Ok(false);
                }
                Self::RetainedSource(mut preparation) => {
                    if preparation.advance(resolver.resources(selection), selection, observation)? {
                        let syntax = preparation.finish(selection)?;
                        *self = Self::Normalized(NormalizedTypePreparation::begin(
                            resolver.finish_type_syntax_context(selection, syntax, observation)?,
                        ));
                    } else {
                        *self = Self::RetainedSource(preparation);
                    }
                    return Ok(false);
                }
                Self::Normalized(mut preparation) => {
                    let done = preparation.advance(resolver, selection, observation)?;
                    *self = Self::Normalized(preparation);
                    return Ok(done);
                }
                Self::Failed => {
                    return Err(selected_error(
                        selection,
                        "Type-expression preparation has failed",
                    ));
                }
            }
        }
    }
    pub(crate) fn finish(self, selection: &DeclarationId) -> Result<TypeUse, ImportError> {
        match self {
            Self::Normalized(preparation) => preparation.finish(selection),
            _ => Err(selected_error(
                selection,
                "Type-expression preparation is incomplete or failed",
            )),
        }
    }
}

#[cfg(test)]
mod layout_tests {
    use super::*;
    #[test]
    fn checked_normalization_layout_and_first_source_profile() {
        println!(
            "GenericType={} Frame={} Binding={} OwnedIndexPair={} TypeExpressionPreparation={} SubstitutionPreparation={}",
            std::mem::size_of::<GenericType>(),
            std::mem::size_of::<Frame>(),
            std::mem::size_of::<Binding>(),
            std::mem::size_of::<(DeclarationId, NormalizedBinding)>(),
            std::mem::size_of::<TypeExpressionPreparation>(),
            std::mem::size_of::<SubstitutionPreparation>()
        );
        let (bundle, fragment) = super::super::normalization_preparation_tests::authored(3);
        let resolver = Resolver::new(&bundle).unwrap();
        let selected = resolver
            .resource_sets()
            .0
            .resolve(&bundle.sources[0].id, &fragment)
            .unwrap();
        let expected = resolver.type_expression(&selected).unwrap();
        let mut job = TypeExpressionPreparation::begin();
        let context = InvocationContext::default();
        let observation = ParserObservation::new(&context);
        while !job.advance(&resolver, &selected, &observation).unwrap() {}
        assert_eq!(job.finish(&selected).unwrap(), expected);
    }
}
