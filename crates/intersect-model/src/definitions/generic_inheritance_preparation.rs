use super::*;

// Retained scheduling over immutable inputs only. Each transition borrows its
// current observer; no invocation context or semantic registry is stored here.
// Nested decoding/substitution and one equivalence predicate remain synchronous.
pub(crate) struct AncestryTraversal<'a> {
    resolver: &'a Resolver,
    traversal: Traversal,
    frames: Vec<AncestryFrame<'a>>,
    returned: Option<(Vec<Type>, Type)>,
    output: Option<Vec<InheritedType>>,
    failed: bool,
}
enum FrameInput {
    Root,
    Parent { ty: Type, span: SourceSpan },
}
struct AncestryFrame<'a> {
    input: FrameInput,
    phase: AncestryPhase<'a>,
}

type ParentInputs = std::vec::IntoIter<(Type, SourceSpan)>;
enum AncestryPhase<'a> {
    DirectParents(DirectParentPreparation<'a>),
    Enter,
    Parents {
        parents: ParentInputs,
        ancestors: Vec<Type>,
    },
    Append {
        parents: ParentInputs,
        ancestors: Vec<Type>,
        returned: std::vec::IntoIter<Type>,
        parent: Type,
    },
    Compare {
        ancestors: Vec<Type>,
        context: Vec<Type>,
        remaining: usize,
        earlier: usize,
    },
    Finish(Vec<Type>),
}
impl<'a> AncestryTraversal<'a> {
    pub(crate) fn begin(resolver: &'a Resolver) -> Self {
        Self {
            resolver,
            traversal: Traversal {
                path: vec![],
                completed: BTreeMap::new(),
                result: vec![],
                remaining: 16_384,
            },
            frames: vec![AncestryFrame {
                input: FrameInput::Root,
                phase: AncestryPhase::Enter,
            }],
            returned: None,
            output: None,
            failed: false,
        }
    }
    pub(crate) fn is_silent_agreement_step(&self) -> bool {
        self.frames.last().is_some_and(|frame| match &frame.phase {
            AncestryPhase::DirectParents(preparation) => preparation.is_silent_agreement_step(),
            _ => false,
        })
    }
    #[cfg(test)]
    pub(crate) fn is_source_decoding_step(&self) -> bool {
        self.frames.last().is_some_and(|frame| match &frame.phase {
            AncestryPhase::DirectParents(preparation) => preparation.is_source_decoding_step(),
            _ => false,
        })
    }
    pub(crate) fn advance(
        &mut self,
        root: &Type,
        origin: &SourceSpan,
        observation: &crate::definitions::ecore::ParserObservation<'_, '_>,
    ) -> Result<bool, ImportError> {
        self.advance_with_source_decoding(root, origin, observation, false)
    }
    pub(crate) fn advance_with_source_decoding(
        &mut self,
        root: &Type,
        origin: &SourceSpan,
        observation: &crate::definitions::ecore::ParserObservation<'_, '_>,
        retain_source_decoding: bool,
    ) -> Result<bool, ImportError> {
        if self.failed {
            return Err(error(origin, "Ancestry preparation has failed"));
        }
        if self.output.is_some() {
            return Ok(true);
        }
        match self.advance_inner(root, origin, observation, retain_source_decoding) {
            Ok(()) => Ok(self.output.is_some()),
            Err(error) => {
                self.failed = true;
                Err(error)
            }
        }
    }
    pub(crate) fn finish(self, origin: &SourceSpan) -> Result<Vec<InheritedType>, ImportError> {
        if self.failed {
            return Err(error(origin, "Ancestry preparation has failed"));
        }
        self.output
            .ok_or_else(|| error(origin, "Ancestry preparation is incomplete"))
    }
    fn return_frame(
        &mut self,
        frame: AncestryFrame<'a>,
        ancestors: Vec<Type>,
        origin: &SourceSpan,
    ) -> Result<(), ImportError> {
        if self.frames.is_empty() {
            self.output = Some(std::mem::take(&mut self.traversal.result));
        } else {
            let FrameInput::Parent { ty, .. } = frame.input else {
                return Err(error(origin, "Nested ancestry root is invalid"));
            };
            self.returned = Some((ancestors, ty));
        }
        Ok(())
    }
    fn advance_inner(
        &mut self,
        root: &Type,
        origin: &SourceSpan,
        observation: &crate::definitions::ecore::ParserObservation<'_, '_>,
        retain_source_decoding: bool,
    ) -> Result<(), ImportError> {
        let mut frame = self
            .frames
            .pop()
            .ok_or_else(|| error(origin, "Missing ancestry frame"))?;
        let phase = std::mem::replace(&mut frame.phase, AncestryPhase::Enter);
        let (ty, span) = match &frame.input {
            FrameInput::Root => (root, origin),
            FrameInput::Parent { ty, span } => (ty, span),
        };
        match phase {
            AncestryPhase::Enter => {
                inheritance_budget(&mut self.traversal, span, self.frames.len(), observation)?;
                let Type::Classifier { declaration, .. } = ty else {
                    return Err(error(
                        span,
                        "Generic inheritance requires an EClass application",
                    ));
                };
                if self
                    .resolver
                    .classifier_observed(declaration, observation)?
                    != TargetKind::Class
                {
                    return Err(error(
                        span,
                        "Generic inheritance requires an EClass application",
                    ));
                }
                for active in &self.traversal.path {
                    observation.work(1, &span.source)?;
                    if active == declaration {
                        return Err(error(span, "Cyclic generic inheritance"));
                    }
                }
                if let Some(completed) = self.traversal.completed.get(ty) {
                    observation.serialized(completed, &span.source)?;
                    let completed = completed.clone();
                    self.return_frame(frame, completed, origin)?;
                    return Ok(());
                }
                observation.serialized(declaration, &span.source)?;
                observation.reserve(32, &span.source)?;
                self.traversal.path.push(declaration.clone());
                if self.traversal.path.len() > 1 {
                    observation.serialized(ty, &span.source)?;
                    observation.serialized(&self.traversal.path, &span.source)?;
                    observation.reserve(128 + 32, &span.source)?;
                    self.traversal.result.push(InheritedType {
                        expression: ty.clone(),
                        path: self.traversal.path.clone(),
                    });
                }
                frame.phase =
                    AncestryPhase::DirectParents(DirectParentPreparation::begin(self.resolver));
                self.frames.push(frame);
            }
            AncestryPhase::DirectParents(mut preparation) => {
                frame.phase = if preparation.advance_with_source_decoding(
                    ty,
                    span,
                    observation,
                    retain_source_decoding,
                )? {
                    AncestryPhase::Parents {
                        parents: preparation.finish(span)?.into_iter(),
                        ancestors: vec![],
                    }
                } else {
                    AncestryPhase::DirectParents(preparation)
                };
                self.frames.push(frame);
            }
            AncestryPhase::Parents {
                mut parents,
                ancestors,
            } => {
                if let Some((returned, parent)) = self.returned.take() {
                    frame.phase = AncestryPhase::Append {
                        parents,
                        ancestors,
                        returned: returned.into_iter(),
                        parent,
                    };
                    self.frames.push(frame);
                } else if let Some((parent, location)) = parents.next() {
                    observation.work(1, &span.source)?;
                    frame.phase = AncestryPhase::Parents { parents, ancestors };
                    self.frames.push(frame);
                    self.frames.push(AncestryFrame {
                        input: FrameInput::Parent {
                            ty: parent,
                            span: location,
                        },
                        phase: AncestryPhase::Enter,
                    });
                } else {
                    // Preserve the original snapshot context and reverse removal:
                    // raw/applied equivalence is deliberately nontransitive.
                    let context =
                        if self.resolver.prepared_formals.get().is_some() && ancestors.len() < 2 {
                            vec![]
                        } else {
                            observation.serialized(&ancestors, &span.source)?;
                            ancestors.clone()
                        };
                    let remaining = ancestors.len();
                    frame.phase = AncestryPhase::Compare {
                        ancestors,
                        context,
                        remaining,
                        earlier: 0,
                    };
                    self.frames.push(frame);
                }
            }
            AncestryPhase::Append {
                parents,
                mut ancestors,
                mut returned,
                parent,
            } => {
                if let Some(ancestor) = returned.next() {
                    observation.work(1, &span.source)?;
                    observation.reserve(32, &span.source)?;
                    ancestors.push(ancestor);
                    frame.phase = AncestryPhase::Append {
                        parents,
                        ancestors,
                        returned,
                        parent,
                    };
                } else {
                    observation.reserve(32, &span.source)?;
                    ancestors.push(parent);
                    frame.phase = AncestryPhase::Parents { parents, ancestors };
                }
                self.frames.push(frame);
            }
            AncestryPhase::Compare {
                mut ancestors,
                context,
                mut remaining,
                mut earlier,
            } => {
                if remaining == 0 {
                    frame.phase = AncestryPhase::Finish(ancestors);
                } else {
                    let current = remaining - 1;
                    if earlier == current {
                        remaining -= 1;
                        earlier = 0;
                    } else if (AncestorEquivalence {
                        resolver: self.resolver,
                        context: &context,
                        traversal: &mut self.traversal,
                        span,
                        observation,
                    })
                    .equivalent(
                        &ancestors[current],
                        &ancestors[earlier],
                        0,
                    )? {
                        observation.work(ancestors.len() - current - 1, &span.source)?;
                        ancestors.remove(current);
                        remaining -= 1;
                        earlier = 0;
                    } else {
                        earlier += 1;
                    }
                    frame.phase = AncestryPhase::Compare {
                        ancestors,
                        context,
                        remaining,
                        earlier,
                    };
                }
                self.frames.push(frame);
            }
            AncestryPhase::Finish(ancestors) => {
                let mut classifiers = std::collections::BTreeSet::new();
                for ancestor in &ancestors {
                    observation.work(1, &span.source)?;
                    if let Type::Classifier { declaration, .. } = ancestor {
                        observation.reserve(128, &span.source)?;
                        if !classifiers.insert(declaration) {
                            return Err(error(
                                span,
                                format!(
                                    "Incompatible generic diamond for {} {:?}",
                                    declaration.source.uri, declaration.path
                                ),
                            ));
                        }
                    }
                }
                self.traversal.path.pop();
                if self.resolver.prepared_formals.get().is_none() || !self.traversal.path.is_empty()
                {
                    observation.serialized(ty, &span.source)?;
                    observation.serialized(&ancestors, &span.source)?;
                    observation.reserve(128, &span.source)?;
                    self.traversal
                        .completed
                        .insert(ty.clone(), ancestors.clone());
                }
                self.return_frame(frame, ancestors, origin)?;
            }
        }
        Ok(())
    }
}

// Ordinary/declaration callers keep their exact borrowed-root API. Nested owners
// may drive the same traversal with their fixed current inputs, without copies
// or self-references into movable preparation storage.
pub(crate) struct AncestryPreparation<'a> {
    root: &'a Type,
    origin: &'a SourceSpan,
    traversal: AncestryTraversal<'a>,
}
impl<'a> AncestryPreparation<'a> {
    pub(crate) fn begin(resolver: &'a Resolver, root: &'a Type, origin: &'a SourceSpan) -> Self {
        Self {
            root,
            origin,
            traversal: AncestryTraversal::begin(resolver),
        }
    }
    pub(crate) fn advance(
        &mut self,
        observation: &crate::definitions::ecore::ParserObservation<'_, '_>,
    ) -> Result<bool, ImportError> {
        self.traversal.advance(self.root, self.origin, observation)
    }
    pub(crate) fn finish(self) -> Result<Vec<InheritedType>, ImportError> {
        self.traversal.finish(self.origin)
    }
}
