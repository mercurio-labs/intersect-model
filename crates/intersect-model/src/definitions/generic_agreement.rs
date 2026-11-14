//! Raw Ecore projections retain legacy references alongside checked generic syntax.
use super::*;
#[path = "generic_legacy_preparation.rs"]
mod preparation;
use crate::definitions::ecore::PayloadKind;
pub(crate) use preparation::AgreementPreparation;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RawSignature {
    pub owner: DeclarationId,
    /// eType, eSuperTypes or eExceptions, with ordered raw classifier identities.
    pub feature: String,
    /// None means no legacy encoding was supplied (distinct from an empty list).
    pub legacy: Option<Vec<TypeReference>>,
    pub generic: Vec<DeclarationId>,
    pub raw_types: Vec<DeclarationId>,
}
impl Resolver {
    fn standard_raw_observed(
        &self,
        name: &str,
        span: &SourceSpan,
        observation: &crate::definitions::ecore::ParserObservation<'_, '_>,
    ) -> Result<DeclarationId, ImportError> {
        let href = observation.formatted(&format_args!("#//{name}"), &span.source)?;
        self.reference_input_context(true, &self.standard_id, &href, span, observation)
            .map_err(|e| error(span, e.message))
    }
    pub(super) fn erasure_observed(
        &self,
        syntax: &GenericType,
        active: &mut BTreeSet<DeclarationId>,
        observation: &crate::definitions::ecore::ParserObservation<'_, '_>,
    ) -> Result<DeclarationId, ImportError> {
        observation.work(1, &syntax.span.source)?;
        if active.len() >= 128 {
            return Err(ImportError::new(
                ErrorKind::ResourceLimit,
                &syntax.span,
                "Raw type derivation depth exceeds 128",
            ));
        }
        if active.contains(&syntax.declaration) {
            return Err(error(&syntax.span, "Cyclic raw type derivation"));
        }
        observation.serialized(&syntax.declaration, &syntax.span.source)?;
        observation.reserve(128, &syntax.span.source)?;
        active.insert(syntax.declaration.clone());
        let result = (|| {
            match &syntax.value {
                GenericKind::Classifier { reference, .. } => {
                    self.reference_observed(&syntax.declaration, reference, observation)
                }
                GenericKind::Wildcard { .. } => {
                    self.standard_raw_observed("EJavaObject", &syntax.span, observation)
                }
                GenericKind::Parameter { reference } => {
                    let target =
                        self.reference_observed(&syntax.declaration, reference, observation)?;
                    let container = self.node_observed(
                        &parent_observed(&syntax.declaration, observation)?,
                        observation,
                    )?;
                    let required_kind = if named(container, "eStructuralFeatures") {
                        Some(kind_observed(container, observation)? == "EReference")
                    } else {
                        None
                    };
                    let parameter = crate::definitions::generics::parse_parameter_observed(
                        self.resources(&target),
                        &target,
                        observation,
                    )?;
                    for bound in &parameter.bounds {
                        observation.work(1, &syntax.span.source)?;
                        // Resolve the bound in its declaration context, not in the usage context.
                        self.walk_observed(bound, &mut Vec::new(), &mut Vec::new(), observation)?;
                        let raw = self.erasure_observed(bound, active, observation)?;
                        let is_class =
                            self.classifier_observed(&raw, observation)? == TargetKind::Class;
                        if required_kind.is_none_or(|required| required == is_class) {
                            return Ok(raw);
                        }
                    }
                    self.standard_raw_observed(
                        if required_kind == Some(true) {
                            "EObject"
                        } else {
                            "EJavaObject"
                        },
                        &syntax.span,
                        observation,
                    )
                }
            }
        })();
        active.remove(&syntax.declaration);
        result
    }
    fn legacy_reference_context(
        &self,
        href: &str,
        kind_hint: Option<String>,
        span: &SourceSpan,
        source: &SourceId,
        observation: &crate::definitions::ecore::ParserObservation<'_, '_>,
    ) -> Result<TypeReference, ImportError> {
        observation.string(href.len(), source)?;
        let span = observation.span(&span.source, span.start, span.end)?;
        // One reference record and its vector slot, before either is retained.
        observation.reserve(128 + 32, source)?;
        Ok(TypeReference {
            href: href.to_owned(),
            kind_hint,
            span,
        })
    }
    #[cfg(test)]
    fn legacy_references_context(
        &self,
        owner: &DeclarationId,
        name: &str,
        observation: &crate::definitions::ecore::ParserObservation<'_, '_>,
    ) -> Result<Option<Vec<TypeReference>>, ImportError> {
        let mut preparation = preparation::LegacyPreparation::begin();
        while !preparation.advance(self, owner, name, observation)? {}
        preparation.finish(owner)
    }
    fn legacy_target_context(
        &self,
        owner: &DeclarationId,
        reference: &TypeReference,
        observation: &crate::definitions::ecore::ParserObservation<'_, '_>,
    ) -> Result<DeclarationId, ImportError> {
        let target = self.reference_observed(owner, reference, observation)?;
        let target_kind = self.classifier_observed(&target, observation)?;
        let agrees = match reference.kind_hint.as_deref() {
            None | Some("EClassifier") => true,
            Some("EClass") => target_kind == TargetKind::Class,
            Some("EDataType") => {
                matches!(target_kind, TargetKind::Datatype | TargetKind::Enumeration)
            }
            Some("EEnum") => target_kind == TargetKind::Enumeration,
            _ => false,
        };
        if !agrees {
            return Err(error(
                &reference.span,
                "Legacy kind hint disagrees with resolved classifier",
            ));
        }
        Ok(target)
    }
    pub(super) fn check_legacy(
        &self,
        owner: &DeclarationId,
    ) -> Result<Vec<RawSignature>, ImportError> {
        self.check_legacy_context(
            owner,
            &crate::definitions::ecore::ParserObservation::new(
                &crate::definitions::conversion::InvocationContext::default(),
            ),
        )
    }
    pub(super) fn check_legacy_context(
        &self,
        owner: &DeclarationId,
        observation: &crate::definitions::ecore::ParserObservation<'_, '_>,
    ) -> Result<Vec<RawSignature>, ImportError> {
        let mut preparation = AgreementPreparation::begin();
        while !preparation.advance(self, owner, observation)? {}
        preparation.finish(owner)
    }
}

#[cfg(test)]
#[path = "generic_legacy_observation_tests.rs"]
mod observation_tests;

#[cfg(test)]
#[path = "generic_legacy_preparation_tests.rs"]
mod retained_legacy_tests;
