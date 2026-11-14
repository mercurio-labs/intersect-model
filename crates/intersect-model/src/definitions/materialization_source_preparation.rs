//! Retained source loading for the final materialization profile. Exact source
//! ownership survives every failure; readiness never comes from placeholder XML.
use super::*;
use crate::definitions::{bundle::SourceId, resources::ResourceSetPreparation};

pub(super) struct SourceLoadingInputs {
    entry: SourceSpan,
    namespace: String,
    requested: Vec<RuntimeType>,
    standard_bundle: std::cell::OnceCell<Box<DefinitionBundle>>,
}
#[derive(Clone, Copy)]
enum LoadingPhase {
    Canonical(resource_compiler::CanonicalSourcePreparation),
    User,
    Standard,
    Install,
}
pub(super) struct SourceLoading<'source> {
    phase: LoadingPhase,
    resources: Option<ResourceSetPreparation<'source>>,
    standard: Option<ResourceSet>,
    selected: Option<Box<SelectedMaterialization>>,
    policy_error: Option<ImportError>,
}
pub(super) enum MaterializationState<'source> {
    Loading(Box<SourceLoading<'source>>),
    Ready(Box<MaterializationContext<'source>>),
}
impl<'source> MaterializationState<'source> {
    pub(super) fn ready_mut(
        &mut self,
        span: &SourceSpan,
    ) -> Result<&mut MaterializationContext<'source>, ImportError> {
        match self {
            Self::Ready(state) => Ok(state),
            Self::Loading(_) => Err(ImportError::new(
                ErrorKind::InvalidDefinition,
                span,
                "Source resources are not ready",
            )),
        }
    }
}
impl MaterializationSourceInputs {
    pub(super) fn resolver(&self) -> Result<&Resolver, ImportError> {
        self.resolver.get().ok_or_else(|| {
            let span = self
                .loading
                .as_ref()
                .map(|inputs| inputs.entry.clone())
                .unwrap_or_else(|| SourceSpan {
                    source: self
                        .bundle
                        .sources
                        .first()
                        .map(|source| source.id.clone())
                        .unwrap_or_else(|| SourceId {
                            uri: "memory:bundle".into(),
                            sha256: String::new(),
                        }),
                    start: 0,
                    end: 0,
                });
            ImportError::new(
                ErrorKind::InvalidDefinition,
                &span,
                "Source resolver is not ready",
            )
        })
    }
}
impl OwnedMaterializationPreparation {
    pub(super) fn error(&self, message: impl Into<String>) -> ImportError {
        ImportError::new(ErrorKind::InvalidDefinition, &self.fallback, message)
    }
    pub(super) fn advance_sources(
        &mut self,
        conversions: Option<&crate::definitions::conversion::ConversionRegistry<'_>>,
        observation: &crate::definitions::ecore::ParserObservation<'_, '_>,
    ) -> Result<Option<Xml>, ImportError> {
        if self.failed {
            return Err(ImportError::new(
                ErrorKind::InvalidDefinition,
                &self.fallback,
                "Owned source preparation has failed",
            ));
        }
        let result = self.holder.with_dependent_mut(|owner, state| {
            observation.work(0, &self.fallback.source)?;
            let MaterializationState::Loading(loading) = state else {
                return Err(ImportError::new(
                    ErrorKind::InvalidDefinition,
                    &self.fallback,
                    "Source preparation is already complete",
                ));
            };
            let inputs = owner.loading.as_ref().ok_or_else(|| {
                ImportError::new(
                    ErrorKind::InvalidDefinition,
                    &self.fallback,
                    "Missing source selection inputs",
                )
            })?;
            let invalid =
                |message| ImportError::new(ErrorKind::InvalidDefinition, &self.fallback, message);
            match loading.phase {
                LoadingPhase::Canonical(mut scan) => {
                    if scan.advance(&owner.bundle)? {
                        loading.resources =
                            Some(observation.imported(&inputs.entry.source, |context| {
                                ResourceSetPreparation::begin_with_retained_bundle(
                                    &owner.bundle,
                                    crate::definitions::ecore::ParseLimits::default(),
                                    context,
                                )
                            })?);
                        loading.phase = LoadingPhase::User;
                    } else {
                        loading.phase = LoadingPhase::Canonical(scan);
                    }
                    return Ok(None);
                }
                LoadingPhase::User => {
                    let resources = loading
                        .resources
                        .as_mut()
                        .ok_or_else(|| invalid("Missing resource preparation"))?;
                    if !observation
                        .imported(&inputs.entry.source, |context| resources.advance(context))?
                    {
                        return Ok(None);
                    }
                    let preparation = loading
                        .resources
                        .take()
                        .ok_or_else(|| invalid("Missing completed resource preparation"))?;
                    let resources =
                        observation.imported_result(&inputs.entry.source, preparation.finish())?;
                    let entry_span = observation.span(
                        &inputs.entry.source,
                        inputs.entry.start,
                        inputs.entry.end,
                    )?;
                    let selected = resource_compiler::selected_context_from_resources(
                        &owner.bundle,
                        &inputs.entry.source,
                        &inputs.namespace,
                        resources,
                        entry_span,
                        observation,
                    )?;
                    let mut selection = finish_materialization_input_selection(
                        &owner.bundle,
                        &inputs.entry.source,
                        selected,
                        observation,
                        None,
                        true,
                    )?;
                    if let Some(mut error) = loading.policy_error.take() {
                        error.span = observation.span(
                            &selection.root.opening.source,
                            selection.root.opening.start,
                            selection.root.opening.end,
                        )?;
                        return Err(error);
                    }
                    self.policy
                        .verify(conversions, &selection.root.opening, observation)?;
                    if let Some(subset) = selection.subset.take() {
                        owner
                            .subset
                            .set(subset)
                            .map_err(|_| invalid("Source subset already installed"))?;
                    }
                    let standard_source = standard_datatype_source_observed(observation)?;
                    let fallback = observation.span(&standard_source.id, 0, 0)?;
                    check_requested_materialization_types(&inputs.requested, &fallback)?;
                    observation.reserve(128 + 32, &standard_source.id)?;
                    inputs
                        .standard_bundle
                        .set(Box::new(DefinitionBundle {
                            version: 1,
                            sources: vec![standard_source],
                            namespaces: vec![],
                        }))
                        .map_err(|_| invalid("Standard source already installed"))?;
                    let bundle = inputs
                        .standard_bundle
                        .get()
                        .ok_or_else(|| invalid("Missing standard source bundle"))?;
                    loading.resources =
                        Some(observation.imported(&fallback.source, |context| {
                            ResourceSetPreparation::begin_with_retained_bundle(
                                bundle,
                                crate::definitions::ecore::ParseLimits::default(),
                                context,
                            )
                        })?);
                    observation.reserve(SELECTED_SOURCE_BYTES, &selection.root.opening.source)?;
                    loading.selected = Some(Box::new(selection));
                    loading.phase = LoadingPhase::Standard;
                    return Ok(None);
                }
                LoadingPhase::Standard => {
                    let preparation = loading
                        .resources
                        .as_mut()
                        .ok_or_else(|| invalid("Missing standard source preparation"))?;
                    if observation
                        .imported(&inputs.entry.source, |context| preparation.advance(context))?
                    {
                        let preparation = loading.resources.take().ok_or_else(|| {
                            invalid("Missing completed standard source preparation")
                        })?;
                        loading.standard = Some(
                            observation
                                .imported_result(&inputs.entry.source, preparation.finish())?,
                        );
                        loading.phase = LoadingPhase::Install;
                    }
                    return Ok(None);
                }
                LoadingPhase::Install => {}
            }
            let selection = *loading
                .selected
                .take()
                .ok_or_else(|| invalid("Missing selected source resources"))?;
            self.policy
                .verify(conversions, &selection.root.opening, observation)?;
            let standard_bundle = inputs
                .standard_bundle
                .get()
                .ok_or_else(|| invalid("Missing prepared standard source"))?;
            let standard_source = standard_bundle
                .sources
                .first()
                .ok_or_else(|| invalid("Empty prepared standard source"))?;
            let fallback = observation.span(&standard_source.id, 0, 0)?;
            let resolver = Resolver::with_prepared_resource_inputs(
                owner.selected_bundle(),
                standard_bundle,
                selection
                    .resources
                    .ok_or_else(|| invalid("Missing selected parsed resources"))?,
                loading
                    .standard
                    .take()
                    .ok_or_else(|| invalid("Missing prepared standard resources"))?,
                observation,
            )?;
            let resolver = bind_materialization_resolver::<
                SharedArrayAlternatives<crate::definitions::schema::SharedRuntimeSourceArrayTarget>,
            >(resolver, conversions)?;
            resolver.enable_source_reference_inputs(observation)?;
            resolver.enable_parameter_bound_inputs(observation)?;
            resolver.enable_attempt_classifiers();
            resolver.enable_compiled_feature_signatures_observed(observation)?;
            owner.resolver.set(resolver).map_err(|_| {
                ImportError::new(
                    ErrorKind::InvalidDefinition,
                    &self.fallback,
                    "Source resolver already installed",
                )
            })?;
            let resolver = owner.resolver()?;
            let mut ready =
                new_materialization_context(owner.selected_bundle(), resolver, observation)?;
            let begin_inventory = if self.generic_agreement_polling == GenericAgreementPolling::RetainedGenericSourceDecoding {
                crate::definitions::generic_resolution::SourceInventoryPreparation::new_with_retained_generic_source_decoding
            } else if self.generic_agreement_polling == GenericAgreementPolling::RetainedSourceInventory {
                crate::definitions::generic_resolution::SourceInventoryPreparation::new_with_retained_source_inventory_agreement
            } else {
                crate::definitions::generic_resolution::SourceInventoryPreparation::new
            };
            ready.source_inventory = Some(Box::new(begin_inventory(resolver,observation)?));
            ready.set_reports(ReportPreparation::new_observed(
                &inputs.requested,
                &fallback,
                observation,
            )?);
            ready.reuse_compiler_type_inputs = true;
            *state = MaterializationState::Ready(Box::new(ready));
            self.fallback = fallback;
            Ok(Some(selection.root))
        });
        if result.is_err() {
            self.failed = true;
        }
        result
    }
}
// Both boxes and the dependent holder are covered before retention.
const SELECTED_SOURCE_BYTES: usize = 1024;
const _: () =
    assert!(std::mem::size_of::<SelectedMaterialization>() + 128 <= SELECTED_SOURCE_BYTES);
const LOADING_STATE_BYTES: usize = 2048;
const _: () = assert!(
    std::mem::size_of::<SourceLoading<'static>>()
        + std::mem::size_of::<SourceLoadingInputs>()
        + std::mem::size_of::<DefinitionBundle>()
        + 128
        <= LOADING_STATE_BYTES
);

pub(super) fn begin(
    bundle: DefinitionBundle,
    entry: &SourceId,
    namespace: &str,
    requested: &[RuntimeType],
    conversions: Option<&crate::definitions::conversion::ConversionRegistry<'_>>,
    operation: &crate::definitions::conversion::InvocationContext<'_>,
    retain_canonical: bool,
) -> Result<OwnedMaterializationPreparation, Box<OwnedRuntimeSourceArrayMaterialization>> {
    let observation = crate::definitions::ecore::ParserObservation::new(operation);
    let fixed = (|| {
        if !retain_canonical {
            resource_compiler::require_canonical_sources(&bundle)?;
        }
        observation.work(0, entry)?;
        let entry_span = observation.span(entry, 0, 0)?;
        let fallback = observation.span(entry, 0, 0)?;
        observation.string(namespace.len(), entry)?;
        observation.reserve(OWNED_CONTEXT_BYTES + LOADING_STATE_BYTES, entry)?;
        let mut retained = Vec::new();
        // Invalid request counts keep the existing post-selection diagnostic;
        // no arbitrarily large vector is copied before that check.
        if (1..=1024).contains(&requested.len()) {
            observation.reserve(128, entry)?;
            let bytes = observation.size(requested.len().checked_mul(32), entry)?;
            observation.reserve(bytes, entry)?;
            retained.try_reserve_exact(requested.len()).map_err(|_| {
                ImportError::new(
                    ErrorKind::ResourceLimit,
                    &entry_span,
                    "Requested type allocation failed",
                )
            })?;
            for ty in requested {
                observation.work(1, entry)?;
                retained.push(ty.clone());
            }
        }
        Ok((
            SourceLoadingInputs {
                entry: entry_span,
                namespace: namespace.to_owned(),
                requested: retained,
                standard_bundle: Default::default(),
            },
            fallback,
        ))
    })();
    let (loading, fallback) = match fixed {
        Ok(value) => value,
        Err(error) => {
            return Err(Box::new(OwnedRuntimeSourceArrayMaterialization {
                bundle,
                result: observation.finish(Err(error)),
            }));
        }
    };
    let owner = MaterializationSourceInputs {
        bundle,
        subset: Default::default(),
        resolver: Default::default(),
        loading: Some(Box::new(loading)),
    };
    let mut policy = CompilationPolicyInputs::default();
    let holder = match OwnedMaterializationContext::try_new_or_recover(owner, |owner| {
        let resources = if retain_canonical {
            None
        } else {
            Some(observation.imported(entry, |context| {
                ResourceSetPreparation::begin_with_retained_bundle(
                    &owner.bundle,
                    crate::definitions::ecore::ParseLimits::default(),
                    context,
                )
            })?)
        };
        let policy_error = match observation.finish(CompilationPolicyInputs::capture(
            conversions,
            &fallback,
            &observation,
        )) {
            Ok(captured) => {
                policy = captured;
                None
            }
            Err(crate::definitions::ecore::ObservedImportError::Import(error)) => {
                observation.serialized(&error.span, &error.span.source)?;
                observation.string(error.message.len(), &error.span.source)?;
                observation.reserve(128, &error.span.source)?;
                Some(error)
            }
            Err(error) => return observation.imported_result(entry, Err(error)),
        };
        Ok(MaterializationState::Loading(Box::new(SourceLoading {
            phase: if retain_canonical {
                LoadingPhase::Canonical(Default::default())
            } else {
                LoadingPhase::User
            },
            resources,
            standard: None,
            selected: None,
            policy_error,
        })))
    }) {
        Ok(holder) => holder,
        Err((owner, error)) => {
            return Err(Box::new(OwnedRuntimeSourceArrayMaterialization {
                bundle: owner.bundle,
                result: observation.finish(Err(error)),
            }));
        }
    };
    let mut storage = SharedArrayAlternatives::default();
    if let Err(message) = storage.domains.enable_indexed_inputs() {
        let owner = holder.into_owner();
        return Err(Box::new(OwnedRuntimeSourceArrayMaterialization {
            bundle: owner.bundle,
            result: Err(crate::definitions::ecore::ObservedImportError::Import(
                ImportError::new(ErrorKind::InvalidDefinition, &fallback, message),
            )),
        }));
    }
    Ok(OwnedMaterializationPreparation {
        generic_agreement_polling: GenericAgreementPolling::Compatibility,
        nominal_bitmaps: true,
        observe_membership: true,
        buffered_revision: true,
        incremental_revision: true,
        publication_started: false,
        policy,
        holder,
        inventory: None,
        fallback,
        reports_complete: false,
        features_complete: false,
        classes_complete: false,
        feature_progress: None,
        storage,
        failed: false,
    })
}

#[cfg(test)]
#[path = "materialization_source_preparation_tests.rs"]
mod tests;
