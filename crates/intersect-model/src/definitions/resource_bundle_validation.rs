//! Retained resource-layer source prepass and shared bundle validation.
//! Resource admission preserves its historical source-first diagnostic priority.
use super::*;
use crate::definitions::bundle::{BundleValidation, SourceValidation};

enum Phase {
    Sources,
    SourceInput,
    Bundle,
    Complete,
}
pub(crate) struct ResourceBundleValidation<'bundle> {
    bundle: &'bundle DefinitionBundle,
    phase: Phase,
    index: usize,
    input: Option<SourceValidation<'bundle>>,
    bundle_validation: BundleValidation<'bundle>,
}
impl<'bundle> ResourceBundleValidation<'bundle> {
    pub(crate) fn begin(
        bundle: &'bundle DefinitionBundle,
        observation: &ParserObservation<'_, '_>,
    ) -> Result<(SourceSpan, Self), ImportError> {
        let span = if let Some(source) = bundle.sources.first() {
            observation.span(&source.id, 0, 0)?
        } else {
            observation.empty_span()?
        };
        observation.work(1, &span.source)?;
        observation.reserve(128, &span.source)?;
        Ok((
            span,
            Self {
                bundle,
                phase: Phase::Sources,
                index: 0,
                input: None,
                bundle_validation: BundleValidation::new(bundle),
            },
        ))
    }
    pub(crate) fn advance(
        &mut self,
        span: &SourceSpan,
        observation: &ParserObservation<'_, '_>,
    ) -> Result<bool, ImportError> {
        match self.phase {
            Phase::Sources => {
                if let Some(source) = self.bundle.sources.get(self.index) {
                    observation.work(source.text.len().div_ceil(256).max(1), &source.id)?;
                    observation.reserve(128, &source.id)?;
                    self.input = Some(SourceValidation::new(source));
                    self.phase = Phase::SourceInput;
                } else {
                    self.phase = Phase::Bundle;
                }
            }
            Phase::SourceInput => {
                let source = self.bundle.sources.get(self.index).ok_or_else(|| {
                    ImportError::new(
                        ErrorKind::InvalidDefinition,
                        span,
                        "Missing bundle prepass source",
                    )
                })?;
                let input = self.input.as_mut().ok_or_else(|| {
                    ImportError::new(
                        ErrorKind::InvalidDefinition,
                        span,
                        "Missing bundle prepass validation",
                    )
                })?;
                let complete = input.advance().map_err(|message| {
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
                if complete {
                    self.input = None;
                    self.index += 1;
                    self.phase = Phase::Sources;
                }
            }
            Phase::Bundle => {
                if self
                    .bundle_validation
                    .advance(&|step| observe(step, observation))
                    .map_err(|message| {
                        ImportError::new(ErrorKind::InvalidDefinition, span, message)
                    })?
                {
                    self.phase = Phase::Complete;
                }
            }
            Phase::Complete => return Ok(true),
        }
        Ok(matches!(self.phase, Phase::Complete))
    }
}
fn observe(
    step: ValidationStep<'_>,
    observation: &ParserObservation<'_, '_>,
) -> Result<(), String> {
    let result = match step {
        ValidationStep::Source(source) => observation
            .work(source.text.len().div_ceil(256).max(1), &source.id)
            .and_then(|()| observation.reserve(128, &source.id)),
        ValidationStep::Identity(id) | ValidationStep::Dependency(id) => observation
            .work(1, id)
            .and_then(|()| observation.reserve(128, id)),
        ValidationStep::Namespace(binding) => observation
            .work(1 + binding.namespace.len().div_ceil(256), &binding.source)
            .and_then(|()| observation.reserve(128, &binding.source)),
        ValidationStep::Lookup(id) => observation.work(1, id),
    };
    result.map_err(|error| error.message)
}
