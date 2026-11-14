//! Prepared ancestry cache admission over the one shared traversal. Fixed root
//! inputs are supplied by each current driver operation, never cloned or stored.
use super::*;
// Move the existing traversal inline without introducing a new heap allocation or changing ordinary observed ancestry fees.
#[allow(clippy::large_enum_variant)]
enum Phase<'a> {
    Start,
    Traverse(AncestryTraversal<'a>),
    Inspect {
        report: Vec<InheritedType>,
        index: usize,
    },
    Install(Vec<InheritedType>),
    Complete(AncestryView),
    Failed,
}
pub(crate) struct AncestryViewPreparation<'a> {
    resolver: &'a Resolver,
    phase: Phase<'a>,
    retain: bool,
}
fn cache<'a>(
    resolver: &'a Resolver,
    ty: &Type,
) -> Option<&'a std::rc::Rc<StructuralAncestryCache>> {
    resolver.prepared_ancestry.get().filter(|_| {
        resolver.constructor_contracts.is_empty()
            && resolver.constructor_relations.is_empty()
            && resolver.provider_relations.is_empty()
            && resolver.rigid_parameters.is_empty()
            && matches!(ty,Type::Classifier{arguments,..} if arguments.is_empty())
    })
}
impl<'a> AncestryViewPreparation<'a> {
    pub(crate) fn begin(resolver: &'a Resolver) -> Self {
        Self {
            resolver,
            phase: Phase::Start,
            retain: false,
        }
    }
    pub(crate) fn is_silent_agreement_step(&self) -> bool {
        match &self.phase {
            Phase::Traverse(preparation) => preparation.is_silent_agreement_step(),
            _ => false,
        }
    }
    #[cfg(test)]
    pub(crate) fn is_source_decoding_step(&self) -> bool {
        match &self.phase {
            Phase::Traverse(preparation) => preparation.is_source_decoding_step(),
            _ => false,
        }
    }
    pub(crate) fn advance(
        &mut self,
        ty: &Type,
        span: &SourceSpan,
        observation: &crate::definitions::ecore::ParserObservation<'_, '_>,
    ) -> Result<bool, ImportError> {
        self.advance_with_source_decoding(ty, span, observation, false)
    }
    pub(crate) fn advance_with_source_decoding(
        &mut self,
        ty: &Type,
        span: &SourceSpan,
        observation: &crate::definitions::ecore::ParserObservation<'_, '_>,
        retain_source_decoding: bool,
    ) -> Result<bool, ImportError> {
        // A failed transition leaves Phase::Failed installed. Cache borrows never
        // cross an advance; callbacks and publication use the current operation.
        let phase = std::mem::replace(&mut self.phase, Phase::Failed);
        self.phase = match phase {
            Phase::Start => {
                if let Some(cache) = cache(self.resolver, ty) {
                    observation.work(1, &span.source)?;
                    let hit = cache
                        .try_borrow()
                        .map_err(|_| error(span, "Prepared ancestry is already borrowed"))?
                        .get(ty)
                        .cloned();
                    if let Some(report) = hit {
                        Phase::Complete(AncestryView::Shared(report))
                    } else {
                        self.retain = true;
                        Phase::Traverse(AncestryTraversal::begin(self.resolver))
                    }
                } else {
                    Phase::Traverse(AncestryTraversal::begin(self.resolver))
                }
            }
            Phase::Traverse(mut traversal) => {
                if traversal.advance_with_source_decoding(
                    ty,
                    span,
                    observation,
                    retain_source_decoding,
                )? {
                    let report = traversal.finish(span)?;
                    if self.retain {
                        Phase::Inspect { report, index: 0 }
                    } else {
                        Phase::Complete(AncestryView::Owned(report))
                    }
                } else {
                    Phase::Traverse(traversal)
                }
            }
            Phase::Inspect { report, index } => {
                if let Some(ancestor) = report.get(index) {
                    observation.work(1, &span.source)?;
                    if !matches!(&ancestor.expression,Type::Classifier{arguments,..} if arguments.is_empty())
                    {
                        Phase::Complete(AncestryView::Owned(report))
                    } else {
                        Phase::Inspect {
                            report,
                            index: index + 1,
                        }
                    }
                } else {
                    Phase::Install(report)
                }
            }
            Phase::Install(report) => {
                let cache = cache(self.resolver, ty)
                    .ok_or_else(|| error(span, "Missing fixed prepared ancestry cache"))?;
                if cache
                    .try_borrow()
                    .map_err(|_| error(span, "Prepared ancestry is already borrowed"))?
                    .len()
                    >= 4096
                {
                    Phase::Complete(AncestryView::Owned(report))
                } else {
                    observation.serialized(ty, &span.source)?;
                    observation.reserve(128 + 32, &span.source)?;
                    let identity = ty.clone();
                    let report = std::rc::Rc::new(report);
                    cache
                        .try_borrow_mut()
                        .map_err(|_| error(span, "Prepared ancestry is already borrowed"))?
                        .insert(identity, std::rc::Rc::clone(&report));
                    Phase::Complete(AncestryView::Shared(report))
                }
            }
            complete @ Phase::Complete(_) => complete,
            Phase::Failed => return Err(error(span, "Ancestry view preparation has failed")),
        };
        Ok(matches!(self.phase, Phase::Complete(_)))
    }
    pub(crate) fn finish(self, span: &SourceSpan) -> Result<AncestryView, ImportError> {
        match self.phase {
            Phase::Complete(view) => Ok(view),
            Phase::Failed => Err(error(span, "Ancestry view preparation has failed")),
            _ => Err(error(span, "Ancestry view preparation is incomplete")),
        }
    }
}
