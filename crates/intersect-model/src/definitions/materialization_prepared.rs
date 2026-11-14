//! Retained ordinary Schema assembly over existing generic stage implementations.
//! Current observers and executable registries are supplied per advance only.
use super::*;
#[derive(Clone, Copy, PartialEq, Eq)]
enum Phase {
    Reports,
    FeatureInputs,
    Features,
    FeatureFinish,
    Classes,
    PublicationBegin,
    Publication,
    Revision,
    Final,
    Complete,
}
pub(super) struct PreparedMaterialization<
    'source,
    A: crate::definitions::schema::RuntimeAlternative,
> {
    phase: Phase,
    inventory: Option<crate::definitions::generic_resolution::SignatureInventory>,
    selections: Option<ReportSelections<'source>>,
    inputs: Option<FeaturePreparationInputs<'source>>,
    progress: Option<FeatureDomainProgress<A>>,
    selection: Option<FeatureDomainSelection<A>>,
    publication: Option<ClassedPublicationPreparation<'source, A>>,
    classed: Option<ClassedPublication<'source, A>>,
    output: Option<crate::definitions::schema::SchemaRepresentation<A>>,
    failed: bool,
}
fn invalid(span: &SourceSpan, message: &str) -> ImportError {
    ImportError::new(ErrorKind::InvalidDefinition, span, message)
}
impl<'source, A: crate::definitions::schema::RuntimeAlternative>
    PreparedMaterialization<'source, A>
{
    pub(super) fn begin(
        state: &mut MaterializationContext<'source>,
        inventory: crate::definitions::generic_resolution::SignatureInventory,
        requested: &[RuntimeType],
        fallback: &SourceSpan,
    ) -> Result<Self, ImportError> {
        state.set_reports(ReportPreparation::new(requested, fallback)?);
        Ok(Self {
            phase: Phase::Reports,
            inventory: Some(inventory),
            selections: None,
            inputs: None,
            progress: None,
            selection: None,
            publication: None,
            classed: None,
            output: None,
            failed: false,
        })
    }
    pub(super) fn advance<P: AlternativeStorage<Target = A>>(
        &mut self,
        state: &mut MaterializationContext<'source>,
        storage: &mut P,
        conversions: Option<&crate::definitions::conversion::ConversionRegistry<'_>>,
        fallback: &SourceSpan,
        observation: &crate::definitions::ecore::ParserObservation<'_, '_>,
    ) -> Result<bool, ImportError> {
        if self.failed {
            return Err(invalid(fallback, "Prepared materialization has failed"));
        }
        let result = self.advance_inner(state, storage, conversions, fallback, observation);
        if result.is_err() {
            self.failed = true;
        }
        result
    }
    fn advance_inner<P: AlternativeStorage<Target = A>>(
        &mut self,
        state: &mut MaterializationContext<'source>,
        storage: &mut P,
        conversions: Option<&crate::definitions::conversion::ConversionRegistry<'_>>,
        fallback: &SourceSpan,
        observation: &crate::definitions::ecore::ParserObservation<'_, '_>,
    ) -> Result<bool, ImportError> {
        match self.phase {
            Phase::Reports => {
                let inventory = self
                    .inventory
                    .as_mut()
                    .ok_or_else(|| invalid(fallback, "Missing prepared signature inventory"))?;
                let mut preparation = state
                    .take_reports()
                    .ok_or_else(|| invalid(fallback, "Missing report preparation"))?;
                let result =
                    preparation.advance(&mut Context { state, observation }, inventory, fallback);
                state.set_reports(preparation);
                if result? {
                    self.selections = Some(
                        state
                            .take_reports()
                            .ok_or_else(|| {
                                invalid(fallback, "Missing complete report preparation")
                            })?
                            .finish(fallback)?,
                    );
                    self.phase = Phase::FeatureInputs;
                }
            }
            Phase::FeatureInputs => {
                self.inputs = Some(prepare_feature_inputs(
                    state,
                    self.selections
                        .take()
                        .ok_or_else(|| invalid(fallback, "Missing report selections"))?,
                    conversions,
                    fallback,
                    observation,
                    false,
                )?);
                self.progress = Some(FeatureDomainProgress::new());
                self.phase = Phase::Features;
            }
            Phase::Features => {
                let inputs = self
                    .inputs
                    .as_ref()
                    .ok_or_else(|| invalid(fallback, "Missing feature preparation inputs"))?;
                let progress = self
                    .progress
                    .take()
                    .ok_or_else(|| invalid(fallback, "Missing feature preparation progress"))?;
                let mut preparation =
                    FeatureDomainPreparation::from_inputs(state, inputs, storage, progress);
                let result = preparation.advance(conversions, observation);
                self.progress = Some(preparation.into_progress());
                if result? {
                    self.phase = Phase::FeatureFinish;
                }
            }
            Phase::FeatureFinish => {
                let inputs = self
                    .inputs
                    .take()
                    .ok_or_else(|| invalid(fallback, "Missing complete feature inputs"))?;
                let progress = self
                    .progress
                    .take()
                    .ok_or_else(|| invalid(fallback, "Missing complete feature progress"))?;
                self.selection = Some(
                    FeatureDomainPreparation::from_inputs(state, &inputs, storage, progress)
                        .finish()?,
                );
                let FeaturePreparationInputs {
                    nominal: _,
                    root,
                    reports,
                    candidates,
                    selections,
                } = inputs;
                drop(candidates);
                state.set_class(ClassPreparation::new(root, reports));
                self.selections = Some(selections);
                self.phase = Phase::Classes;
            }
            Phase::Classes => {
                let mut preparation = state
                    .take_class()
                    .ok_or_else(|| invalid(fallback, "Missing class preparation"))?;
                let result = preparation.advance(&mut Context { state, observation });
                state.set_class(preparation);
                if result? {
                    self.phase = Phase::PublicationBegin;
                }
            }
            Phase::PublicationBegin => {
                let classes = state
                    .take_class()
                    .ok_or_else(|| invalid(fallback, "Missing complete class preparation"))?
                    .finish()?;
                let prepared = PreparedClasses {
                    selections: self
                        .selections
                        .take()
                        .ok_or_else(|| invalid(fallback, "Missing class selections"))?,
                    selection: self
                        .selection
                        .take()
                        .ok_or_else(|| invalid(fallback, "Missing complete feature selection"))?,
                    classes,
                };
                let inventory = self
                    .inventory
                    .take()
                    .ok_or_else(|| invalid(fallback, "Missing publication signature inventory"))?;
                self.publication = Some(begin_classed_publication(
                    state,
                    inventory,
                    prepared,
                    conversions,
                    observation,
                )?);
                self.phase = Phase::Publication;
            }
            Phase::Publication => {
                let pending = self
                    .publication
                    .as_mut()
                    .ok_or_else(|| invalid(fallback, "Missing publication preparation"))?;
                if pending.advance(state, storage, conversions, observation)? {
                    self.classed = Some(finish_prepared_classed_publication(
                        state,
                        self.publication.take().ok_or_else(|| {
                            invalid(fallback, "Missing complete publication preparation")
                        })?,
                        true,
                        observation,
                    )?);
                    self.phase = Phase::Revision;
                }
            }
            Phase::Revision => {
                self.classed = Some(hash_runtime_feature_domains(
                    self.classed
                        .take()
                        .ok_or_else(|| invalid(fallback, "Missing classed publication"))?,
                    false,
                    observation,
                )?);
                self.phase = Phase::Final;
            }
            Phase::Final => {
                self.output = Some(finish_classed_publication(
                    self.classed
                        .take()
                        .ok_or_else(|| invalid(fallback, "Missing final publication"))?,
                    conversions,
                    observation,
                )?);
                self.phase = Phase::Complete;
            }
            Phase::Complete => return Ok(true),
        }
        Ok(self.phase == Phase::Complete)
    }
    pub(super) fn finish(
        mut self,
        fallback: &SourceSpan,
    ) -> Result<crate::definitions::schema::SchemaRepresentation<A>, ImportError> {
        if self.failed {
            return Err(invalid(fallback, "Prepared materialization has failed"));
        }
        if self.phase != Phase::Complete {
            return Err(invalid(fallback, "Prepared materialization is incomplete"));
        }
        self.output
            .take()
            .ok_or_else(|| invalid(fallback, "Missing completed prepared schema"))
    }
}
