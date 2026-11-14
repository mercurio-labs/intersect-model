//! Retained default-policy enum import over the shared projection/lowerer.
use super::*;
use crate::definitions::{
    bundle::SourceSpan,
    conversion::InvocationContext,
    ecore::{ObservedImportError, ParserObservation},
    enums::Enumeration,
    resources::{DeclarationId, ResourceSet, ResourceSetPreparation},
};
struct ProjectionCursor<'a> {
    preparation: Option<XmlProjectionPreparation<'a>>,
}
self_cell::self_cell!(
    struct ProjectionInputs {
        owner: ResourceSet,
        #[covariant]
        dependent: ProjectionCursor,
    }
);
struct LoweringCursor<'a> {
    preparation: Option<EnumLoweringPreparation<'a>>,
}
self_cell::self_cell!(
    struct LoweringInputs {
        owner: Xml,
        #[covariant]
        dependent: LoweringCursor,
    }
);
#[derive(Clone, Copy)]
enum Phase {
    Resources,
    Projection,
    Lowering,
    Complete,
}
pub(crate) struct EnumerationPreparation<'a> {
    phase: Phase,
    declaration: DeclarationId,
    span: SourceSpan,
    resources: Option<Box<ResourceSetPreparation<'a>>>,
    projection: Option<ProjectionInputs>,
    lowering: Option<LoweringInputs>,
    output: Option<Enumeration>,
    failure: Option<ObservedImportError>,
}
const STATE_BYTES: usize = 4096;
const OWNER_BYTES: usize = 2048;
const _: () = assert!(std::mem::size_of::<EnumerationPreparation<'static>>() <= STATE_BYTES);
const _: () = assert!(
    std::mem::size_of::<ResourceSet>() + std::mem::size_of::<ProjectionCursor<'static>>() + 128
        <= OWNER_BYTES
);
const _: () = assert!(
    std::mem::size_of::<Xml>() + std::mem::size_of::<LoweringCursor<'static>>() + 128
        <= OWNER_BYTES
);
fn invalid(span: &SourceSpan, message: &str) -> ObservedImportError {
    ObservedImportError::Import(ImportError::new(
        ErrorKind::InvalidDefinition,
        span,
        message,
    ))
}
impl<'a> EnumerationPreparation<'a> {
    // Without a loaded resource set, enum lookup imports under the default
    // importer policy, independently of the binder's caller limits. Compiles
    // read literals from their loaded resources; this path remains for tests
    // of the default policy.
    #[cfg(test)]
    pub(crate) fn begin(
        bundle: &'a crate::definitions::bundle::DefinitionBundle,
        declaration: DeclarationId,
        context: &InvocationContext<'_>,
    ) -> Result<Self, ObservedImportError> {
        context
            .checkpoint()
            .map_err(ObservedImportError::Observation)?;
        context
            .retain_storage(STATE_BYTES)
            .map_err(ObservedImportError::Observation)?;
        let observation = ParserObservation::new(context);
        observation.finish(observation.serialized(&declaration.source, &declaration.source))?;
        let span = SourceSpan {
            source: declaration.source.clone(),
            start: 0,
            end: 0,
        };
        let resources = Box::new(ResourceSetPreparation::begin_with_retained_bundle(
            bundle,
            Default::default(),
            context,
        )?);
        Ok(Self {
            phase: Phase::Resources,
            declaration,
            span,
            resources: Some(resources),
            projection: None,
            lowering: None,
            output: None,
            failure: None,
        })
    }
    /// Starts from a resource set the caller already loaded under its own
    /// limits, so an enumeration declared in a large metamodel is not imported
    /// again under the default policy.
    pub(crate) fn begin_with_resources(
        resources: ResourceSet,
        declaration: DeclarationId,
        context: &InvocationContext<'_>,
    ) -> Result<Self, ObservedImportError> {
        context
            .checkpoint()
            .map_err(ObservedImportError::Observation)?;
        context
            .retain_storage(STATE_BYTES)
            .map_err(ObservedImportError::Observation)?;
        let observation = ParserObservation::new(context);
        observation.finish(observation.serialized(&declaration.source, &declaration.source))?;
        let span = SourceSpan {
            source: declaration.source.clone(),
            start: 0,
            end: 0,
        };
        let mut preparation = Self {
            phase: Phase::Resources,
            declaration,
            span,
            resources: None,
            projection: None,
            lowering: None,
            output: None,
            failure: None,
        };
        preparation.project(resources, context)?;
        Ok(preparation)
    }
    fn project(
        &mut self,
        resources: ResourceSet,
        context: &InvocationContext<'_>,
    ) -> Result<(), ObservedImportError> {
        let span = &self.span;
        context
            .retain_storage(OWNER_BYTES)
            .map_err(ObservedImportError::Observation)?;
        let observation = ParserObservation::new(context);
        observation.finish(observation.serialized(&self.declaration.path, &span.source))?;
        self.projection = Some(
            ProjectionInputs::try_new(resources, |resources| {
                let node = resources.node(&self.declaration).map_err(|message| {
                    ImportError::new(ErrorKind::InvalidDefinition, span, message)
                })?;
                let document = resources
                    .document(&self.declaration.source)
                    .map_err(|message| {
                        ImportError::new(ErrorKind::InvalidDefinition, span, message)
                    })?;
                Ok::<_, ImportError>(ProjectionCursor {
                    preparation: Some(XmlProjectionPreparation::begin(
                        node,
                        &document.source.text,
                        self.declaration.path.clone(),
                    )),
                })
            })
            .map_err(ObservedImportError::Import)?,
        );
        self.phase = Phase::Projection;
        Ok(())
    }
    pub(crate) fn advance(
        &mut self,
        context: &InvocationContext<'_>,
    ) -> Result<bool, ObservedImportError> {
        if let Some(error) = &self.failure {
            return Err(error.clone());
        }
        let result = self.advance_inner(context);
        if let Err(error) = &result {
            self.failure = Some(error.clone());
        }
        result
    }
    fn advance_inner(
        &mut self,
        context: &InvocationContext<'_>,
    ) -> Result<bool, ObservedImportError> {
        context
            .checkpoint()
            .map_err(ObservedImportError::Observation)?;
        if matches!(self.phase, Phase::Complete) {
            return Ok(true);
        }
        context.work(1).map_err(ObservedImportError::Observation)?;
        let observation = ParserObservation::new(context);
        let span = &self.span;
        match self.phase {
            Phase::Resources => {
                let resources = self
                    .resources
                    .as_mut()
                    .ok_or_else(|| invalid(span, "Missing enum resources"))?;
                if resources.advance(context)? {
                    let resources = self
                        .resources
                        .take()
                        .ok_or_else(|| invalid(span, "Missing completed enum resources"))?
                        .finish()?;
                    self.project(resources, context)?;
                }
            }
            Phase::Projection => {
                let output = self
                    .projection
                    .as_mut()
                    .ok_or_else(|| invalid(span, "Missing enum projection owner"))?
                    .with_dependent_mut(|_, cursor| {
                        let preparation = cursor
                            .preparation
                            .as_mut()
                            .ok_or_else(|| invalid(span, "Missing enum projection"))?;
                        if observation.finish(preparation.advance(&observation, &|_| Ok(())))? {
                            observation
                                .finish(
                                    cursor
                                        .preparation
                                        .take()
                                        .ok_or_else(|| {
                                            invalid(span, "Missing completed enum projection")
                                        })?
                                        .finish(),
                                )
                                .map(Some)
                        } else {
                            Ok(None)
                        }
                    })?;
                if let Some(mut xml) = output {
                    xml.attrs.remove("xmi:id");
                    context
                        .retain_storage(OWNER_BYTES)
                        .map_err(ObservedImportError::Observation)?;
                    self.projection = None;
                    self.lowering = Some(LoweringInputs::new(xml, |xml| LoweringCursor {
                        preparation: Some(EnumLoweringPreparation::begin(xml)),
                    }));
                    self.phase = Phase::Lowering;
                }
            }
            Phase::Lowering => {
                let output = self
                    .lowering
                    .as_mut()
                    .ok_or_else(|| invalid(span, "Missing enum lowering owner"))?
                    .with_dependent_mut(|_, cursor| {
                        let preparation = cursor
                            .preparation
                            .as_mut()
                            .ok_or_else(|| invalid(span, "Missing enum lowerer"))?;
                        if observation.finish(preparation.advance(Some(&observation)))? {
                            observation
                                .finish(
                                    cursor
                                        .preparation
                                        .take()
                                        .ok_or_else(|| {
                                            invalid(span, "Missing completed enum lowerer")
                                        })?
                                        .finish(),
                                )
                                .map(Some)
                        } else {
                            Ok(None)
                        }
                    })?;
                if let Some(output) = output {
                    context
                        .checkpoint()
                        .map_err(ObservedImportError::Observation)?;
                    self.output = Some(output);
                    self.lowering = None;
                    self.phase = Phase::Complete;
                }
            }
            Phase::Complete => return Ok(true),
        }
        Ok(matches!(self.phase, Phase::Complete))
    }
    pub(crate) fn finish(mut self) -> Result<Enumeration, ObservedImportError> {
        if let Some(error) = self.failure.take() {
            return Err(error);
        }
        if !matches!(self.phase, Phase::Complete) {
            return Err(invalid(&self.span, "Enum import is incomplete"));
        }
        self.output
            .take()
            .ok_or_else(|| invalid(&self.span, "Missing complete enum import"))
    }
}
