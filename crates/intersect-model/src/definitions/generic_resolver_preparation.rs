//! Retained whole-bundle admission and source loading for a fresh resolver.
//! Parsed inputs are not semantic authority. Existing direct observed constructor
//! profiles stay separate; this job has explicit retained source/state fees.
use super::*;
use crate::definitions::{
    conversion::InvocationContext,
    ecore::{ObservedImportError, ParseLimits, ParserObservation},
    resources::{ResourceBundleValidation, ResourceSetPreparation},
};

struct StandardCursor<'source> {
    preparation: Option<ResourceSetPreparation<'source>>,
}
self_cell::self_cell!(
    struct StandardResources {
        owner: DefinitionBundle,
        #[covariant]
        dependent: StandardCursor,
    }
);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ResolverPreparationPhase {
    Admission,
    Matching,
    Primary,
    StandardSource,
    Standard,
    Install,
    Complete,
}

/// Fresh source inputs with resumable admission/parsing and operation-local
/// observers. No registry, observer or prepared semantic result is retained.
/// Metadata matching/installation and remaining individual field operations are
/// still synchronous; this is not full compiler or physical-latency qualification.
pub struct ResolverPreparation<'bundle> {
    bundle: &'bundle DefinitionBundle,
    limits: ParseLimits,
    span: SourceSpan,
    phase: ResolverPreparationPhase,
    validation: Option<ResourceBundleValidation<'bundle>>,
    primary: Option<Box<ResourceSetPreparation<'bundle>>>,
    resources: Option<ResourceSet>,
    standard: Option<StandardResources>,
    standard_resources: Option<ResourceSet>,
    output: Option<Box<Resolver>>,
    failure: Option<ObservedImportError>,
}
const STATE_BYTES: usize = 4096;
const STANDARD_BYTES: usize = 2048;
const RESOLVER_BYTES: usize = 2048;
const _: () = assert!(std::mem::size_of::<ResolverPreparation<'static>>() <= STATE_BYTES);
const _: () = assert!(
    std::mem::size_of::<DefinitionBundle>() + std::mem::size_of::<StandardCursor<'static>>() + 128
        <= STANDARD_BYTES
);
const _: () = assert!(std::mem::size_of::<Resolver>() + 128 <= RESOLVER_BYTES);

impl<'bundle> ResolverPreparation<'bundle> {
    /// The primary set may be reused only after complete bundle admission, exact
    /// resource matching and compatible parse-limit provenance. A mismatch takes
    /// the existing full retained parser path, without a reduced-feature fallback.
    pub fn begin(
        bundle: &'bundle DefinitionBundle,
        parsed: Option<ResourceSet>,
        limits: ParseLimits,
        context: &InvocationContext<'_>,
    ) -> Result<Self, ObservedImportError> {
        context
            .checkpoint()
            .map_err(ObservedImportError::Observation)?;
        context
            .retain_storage(STATE_BYTES)
            .map_err(ObservedImportError::Observation)?;
        let observation = ParserObservation::new(context);
        let (span, validation) =
            observation.finish(ResourceBundleValidation::begin(bundle, &observation))?;
        Ok(Self {
            bundle,
            limits,
            span,
            phase: ResolverPreparationPhase::Admission,
            validation: Some(validation),
            primary: None,
            resources: parsed,
            standard: None,
            standard_resources: None,
            output: None,
            failure: None,
        })
    }

    pub fn phase(&self) -> ResolverPreparationPhase {
        self.phase
    }

    fn invalid(&self, message: &str) -> ObservedImportError {
        ObservedImportError::Import(ImportError::new(
            ErrorKind::InvalidDefinition,
            &self.span,
            message,
        ))
    }

    /// One source/admission transition with the current observer. Cancellation
    /// is terminal, and incomplete work can never publish a resolver.
    pub fn advance(
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
        if self.phase == ResolverPreparationPhase::Complete {
            return Ok(true);
        }
        context.work(1).map_err(ObservedImportError::Observation)?;
        let observation = ParserObservation::new(context);
        match self.phase {
            ResolverPreparationPhase::Admission => {
                let validation = self.validation.as_mut().ok_or_else(|| {
                    ObservedImportError::Import(ImportError::new(
                        ErrorKind::InvalidDefinition,
                        &self.span,
                        "Missing whole-bundle admission",
                    ))
                })?;
                if observation.finish(validation.advance(&self.span, &observation))? {
                    self.validation = None;
                    self.phase = ResolverPreparationPhase::Matching;
                }
            }
            ResolverPreparationPhase::Matching => {
                let reuse = match &self.resources {
                    Some(resources) => {
                        observation
                            .finish(resources.matches_bundle_observed(self.bundle, &observation))?
                            && resources.admitted_for(self.limits)
                    }
                    None => false,
                };
                if reuse {
                    self.phase = ResolverPreparationPhase::StandardSource;
                } else {
                    // The old policy/parser decides every rejection and error
                    // order. Never infer stronger admission from a looser set.
                    self.resources = None;
                    self.primary = Some(Box::new(
                        ResourceSetPreparation::begin_with_retained_bundle(
                            self.bundle,
                            self.limits,
                            context,
                        )?,
                    ));
                    self.phase = ResolverPreparationPhase::Primary;
                }
            }
            ResolverPreparationPhase::Primary => {
                let preparation = self.primary.as_mut().ok_or_else(|| {
                    ObservedImportError::Import(ImportError::new(
                        ErrorKind::InvalidDefinition,
                        &self.span,
                        "Missing primary source preparation",
                    ))
                })?;
                if preparation.advance(context)? {
                    self.resources = Some(
                        self.primary
                            .take()
                            .ok_or_else(|| self.invalid("Missing completed primary preparation"))?
                            .finish()?,
                    );
                    self.phase = ResolverPreparationPhase::StandardSource;
                }
            }
            ResolverPreparationPhase::StandardSource => {
                let source = observation.finish(standard_datatype_source_observed(&observation))?;
                context
                    .retain_storage(STANDARD_BYTES)
                    .map_err(ObservedImportError::Observation)?;
                let bundle = DefinitionBundle {
                    version: 1,
                    sources: vec![source],
                    namespaces: vec![],
                };
                self.standard = Some(StandardResources::try_new(bundle, |bundle| {
                    ResourceSetPreparation::begin_with_retained_bundle(
                        bundle,
                        ParseLimits::default(),
                        context,
                    )
                    .map(|preparation| StandardCursor {
                        preparation: Some(preparation),
                    })
                })?);
                self.phase = ResolverPreparationPhase::Standard;
            }
            ResolverPreparationPhase::Standard => {
                let standard = self.standard.as_mut().ok_or_else(|| {
                    ObservedImportError::Import(ImportError::new(
                        ErrorKind::InvalidDefinition,
                        &self.span,
                        "Missing standard source owner",
                    ))
                })?;
                let result = standard.with_dependent_mut(
                    |_, state| -> Result<Option<ResourceSet>, ObservedImportError> {
                        let preparation = state.preparation.as_mut().ok_or_else(|| {
                            ObservedImportError::Import(ImportError::new(
                                ErrorKind::InvalidDefinition,
                                &self.span,
                                "Missing standard source preparation",
                            ))
                        })?;
                        if preparation.advance(context)? {
                            Ok(Some(
                                state
                                    .preparation
                                    .take()
                                    .ok_or_else(|| {
                                        ObservedImportError::Import(ImportError::new(
                                            ErrorKind::InvalidDefinition,
                                            &self.span,
                                            "Missing completed standard source preparation",
                                        ))
                                    })?
                                    .finish()?,
                            ))
                        } else {
                            Ok(None)
                        }
                    },
                )?;
                if let Some(resources) = result {
                    self.standard_resources = Some(resources);
                    self.phase = ResolverPreparationPhase::Install;
                }
            }
            ResolverPreparationPhase::Install => {
                context
                    .retain_storage(RESOLVER_BYTES)
                    .map_err(ObservedImportError::Observation)?;
                let primary = self
                    .resources
                    .take()
                    .ok_or_else(|| self.invalid("Missing admitted primary resources"))?;
                let standard = self
                    .standard_resources
                    .take()
                    .ok_or_else(|| self.invalid("Missing admitted standard resources"))?;
                let bundle = self
                    .standard
                    .as_ref()
                    .ok_or_else(|| self.invalid("Missing standard source identity"))?
                    .borrow_owner();
                let resolver = observation.finish(Resolver::with_prepared_resource_inputs(
                    self.bundle,
                    bundle,
                    primary,
                    standard,
                    &observation,
                ))?;
                context
                    .checkpoint()
                    .map_err(ObservedImportError::Observation)?;
                self.output = Some(Box::new(resolver));
                self.phase = ResolverPreparationPhase::Complete;
            }
            ResolverPreparationPhase::Complete => return Ok(true),
        }
        Ok(self.phase == ResolverPreparationPhase::Complete)
    }

    /// Transfer the fresh resolver only after complete admission and loading.
    pub fn finish(mut self) -> Result<Resolver, ObservedImportError> {
        if let Some(error) = self.failure.take() {
            return Err(error);
        }
        if self.phase != ResolverPreparationPhase::Complete {
            return Err(self.invalid("Resolver preparation is not complete"));
        }
        self.output
            .take()
            .map(|resolver| *resolver)
            .ok_or_else(|| self.invalid("Missing completed resolver"))
    }
}
