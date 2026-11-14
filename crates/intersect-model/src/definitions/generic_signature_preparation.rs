//! One signature-specialization implementation for ordinary and retained callers.
//! Current owner/environment/observation inputs are borrowed for each advance.
use super::*;
use crate::definitions::generic_types::{SubstitutionPreparation, TypeExpressionPreparation};

enum Phase<'a> {
    Begin,
    Decode,
    Agreement(AgreementPreparation<'a>),
    Row,
    Declared,
    DeclaredType(TypeExpressionPreparation<'a>),
    SpecializeStart,
    Specialize(SubstitutionPreparation),
    Publish,
    Install,
    Complete,
    Failed,
}
struct Row {
    feature: String,
    inputs: std::vec::IntoIter<DeclarationId>,
    generic: bool,
    declared: Vec<Type>,
    specialized: Vec<Type>,
    index: usize,
}
enum Rows {
    Building(Vec<SpecializedSignature>),
    Complete(ErasedSignatures),
}
pub(in super::super) struct SignaturePreparation<'a> {
    resolver: &'a Resolver,
    compiled: bool,
    prepared: Option<Rc<RefCell<CompiledFeatureSignatureInputs>>>,
    retain: bool,
    span: Option<&'a SourceSpan>,
    pending: std::vec::IntoIter<RawSignature>,
    row: Option<Row>,
    rows: Rows,
    phase: Phase<'a>,
}
impl<'a> SignaturePreparation<'a> {
    pub(in super::super) fn begin(resolver: &'a Resolver, compiled: bool) -> Self {
        Self {
            resolver,
            compiled,
            prepared: None,
            retain: false,
            span: None,
            pending: vec![].into_iter(),
            row: None,
            rows: Rows::Building(vec![]),
            phase: Phase::Begin,
        }
    }
    // The private driver supplies the same immutable owner and environment at
    // every advance. No borrow of movable caller state survives the operation.
    // Only new subdivisions of the old independently silent decode operation.
    // Compatibility public drivers drain these within that original macrostep;
    // retained_generic_agreement drivers expose them as cancellable advances.
    #[cfg(test)]
    pub(crate) fn is_source_decoding_step(&self) -> bool {
        match &self.phase {
            Phase::Agreement(preparation) => preparation.is_source_decoding_step(),
            Phase::DeclaredType(preparation) => preparation.is_retained_source_step(),
            _ => false,
        }
    }
    pub(crate) fn is_silent_agreement_step(&self) -> bool {
        matches!(self.phase, Phase::Agreement(_))
            || matches!(&self.phase, Phase::DeclaredType(preparation) if preparation.is_retained_source_step())
    }
    pub(in super::super) fn advance(
        &mut self,
        owner: &DeclarationId,
        environment: &Substitution,
        observation: &crate::definitions::ecore::ParserObservation<'_, '_>,
    ) -> Result<bool, ImportError> {
        self.advance_with_source_decoding(owner, environment, observation, false)
    }
    pub(in super::super) fn advance_with_source_decoding(
        &mut self,
        owner: &DeclarationId,
        environment: &Substitution,
        observation: &crate::definitions::ecore::ParserObservation<'_, '_>,
        retain_source_decoding: bool,
    ) -> Result<bool, ImportError> {
        let phase = std::mem::replace(&mut self.phase, Phase::Failed);
        self.phase = match phase {
            Phase::Begin => {
                if self.compiled {
                    let prepared = self
                        .resolver
                        .prepared_feature_signatures
                        .try_borrow()
                        .map_err(|_| {
                            error(
                                &fallback(owner),
                                "Feature signature inputs are already borrowed",
                            )
                        })?
                        .clone();
                    let native = self.resolver.constructor_contracts.is_empty()
                        && self.resolver.constructor_relations.is_empty()
                        && self.resolver.provider_relations.is_empty()
                        && self.resolver.rigid_parameters.is_empty();
                    // Preserve the original eligibility policy and its exact
                    // evaluation order. This table stores descriptor data only.
                    self.prepared = prepared.filter(|_| native && environment.entries().is_empty());
                    if let Some(prepared) = &self.prepared {
                        let span = &self.resolver.node_observed(owner, observation)?.span;
                        self.span = Some(span);
                        observation.work(
                            1 + owner.path.len()
                                + owner.source.uri.len().div_ceil(256)
                                + owner.source.sha256.len().div_ceil(256),
                            &owner.source,
                        )?;
                        let (cached, retain) = {
                            let cache = prepared.try_borrow().map_err(|_| {
                                error(span, "Feature signature code is already borrowed")
                            })?;
                            (
                                cache.signatures.get(owner).cloned(),
                                cache.signatures.len() < 4096,
                            )
                        };
                        self.retain = retain;
                        if let Some(rows) = cached {
                            self.rows = Rows::Complete(ErasedSignatures::Shared(rows));
                            self.phase = Phase::Complete;
                            return Ok(true);
                        }
                    }
                }
                Phase::Decode
            }
            Phase::Decode => {
                self.span = Some(&self.resolver.node_observed(owner, observation)?.span);
                Phase::Agreement(if retain_source_decoding {
                    AgreementPreparation::begin_with_retained_source_decoding()
                } else {
                    AgreementPreparation::begin()
                })
            }
            Phase::Agreement(mut preparation) => {
                // Preserve the original independent silent nested policy. Each
                // owner operation advances the actual shared agreement scan.
                let context = crate::definitions::conversion::InvocationContext::default();
                let nested = crate::definitions::ecore::ParserObservation::new(&context);
                if preparation.advance(self.resolver, owner, &nested)? {
                    self.pending = preparation.finish(owner)?.into_iter();
                    Phase::Row
                } else {
                    Phase::Agreement(preparation)
                }
            }
            Phase::Row => {
                if let Some(signature) = self.pending.next() {
                    let span = self
                        .span
                        .ok_or_else(|| error(&fallback(owner), "Missing signature span"))?;
                    observation.work(1, &span.source)?;
                    let generic = !signature.generic.is_empty();
                    self.row = Some(Row {
                        feature: signature.feature,
                        inputs: if generic {
                            signature.generic
                        } else {
                            signature.raw_types
                        }
                        .into_iter(),
                        generic,
                        declared: vec![],
                        specialized: vec![],
                        index: 0,
                    });
                    Phase::Declared
                } else {
                    Phase::Install
                }
            }
            Phase::Declared => {
                let span = self
                    .span
                    .ok_or_else(|| error(&fallback(owner), "Missing signature span"))?;
                let row = self
                    .row
                    .as_mut()
                    .ok_or_else(|| error(span, "Missing signature row"))?;
                if !row.inputs.as_slice().is_empty() {
                    observation.work(1, &span.source)?;
                    observation.reserve(32, &span.source)?;
                    if row.generic {
                        Phase::DeclaredType(if retain_source_decoding {
                            TypeExpressionPreparation::begin_with_retained_source_decoding()
                        } else {
                            TypeExpressionPreparation::begin()
                        })
                    } else {
                        let input = row
                            .inputs
                            .next()
                            .ok_or_else(|| error(span, "Missing signature input"))?;
                        row.declared.push(Type::Classifier {
                            raw: !self
                                .resolver
                                .formal_info_observed(&input, observation)?
                                .is_empty(),
                            declaration: input,
                            arguments: vec![],
                        });
                        Phase::Declared
                    }
                } else {
                    Phase::SpecializeStart
                }
            }
            Phase::DeclaredType(mut preparation) => {
                let span = self
                    .span
                    .ok_or_else(|| error(&fallback(owner), "Missing signature span"))?;
                let row = self
                    .row
                    .as_mut()
                    .ok_or_else(|| error(span, "Missing signature row"))?;
                let selected = row
                    .inputs
                    .as_slice()
                    .first()
                    .ok_or_else(|| error(span, "Missing signature selection"))?;
                let context = crate::definitions::conversion::InvocationContext::default();
                let nested = crate::definitions::ecore::ParserObservation::new(&context);
                if preparation.advance(self.resolver, selected, &nested)? {
                    let declared = preparation.finish(selected)?;
                    row.inputs.next();
                    row.declared.push(declared.expression);
                    Phase::Declared
                } else {
                    Phase::DeclaredType(preparation)
                }
            }
            Phase::SpecializeStart => {
                let span = self
                    .span
                    .ok_or_else(|| error(&fallback(owner), "Missing signature span"))?;
                let row = self
                    .row
                    .as_mut()
                    .ok_or_else(|| error(span, "Missing signature row"))?;
                if row.declared.get(row.index).is_some() {
                    observation.reserve(32, &span.source)?;
                    Phase::Specialize(SubstitutionPreparation::simultaneous())
                } else {
                    Phase::Publish
                }
            }
            Phase::Specialize(mut preparation) => {
                let span = self
                    .span
                    .ok_or_else(|| error(&fallback(owner), "Missing signature span"))?;
                let row = self
                    .row
                    .as_mut()
                    .ok_or_else(|| error(span, "Missing signature row"))?;
                let input = row
                    .declared
                    .get(row.index)
                    .ok_or_else(|| error(span, "Missing declared signature type"))?;
                if observation.preparation_at(span, |context| {
                    preparation.advance(input, environment, context)
                })? {
                    row.specialized
                        .push(observation.preparation_at(span, |_| preparation.finish())?);
                    row.index += 1;
                    Phase::SpecializeStart
                } else {
                    Phase::Specialize(preparation)
                }
            }
            Phase::Publish => {
                let span = self
                    .span
                    .ok_or_else(|| error(&fallback(owner), "Missing signature span"))?;
                let row = self
                    .row
                    .take()
                    .ok_or_else(|| error(span, "Missing complete signature row"))?;
                observation.serialized(owner, &span.source)?;
                observation.reserve(32, &span.source)?;
                let Rows::Building(rows) = &mut self.rows else {
                    return Err(error(span, "Signature rows already completed"));
                };
                rows.push(SpecializedSignature {
                    owner: owner.clone(),
                    feature: row.feature,
                    declared: row.declared,
                    specialized: row.specialized,
                });
                Phase::Row
            }
            Phase::Install => {
                let Rows::Building(rows) =
                    std::mem::replace(&mut self.rows, Rows::Building(vec![]))
                else {
                    return Err(error(&fallback(owner), "Signature rows already completed"));
                };
                self.rows = Rows::Complete(
                    if let Some(prepared) = self.prepared.as_ref().filter(|_| self.retain) {
                        let span = self
                            .span
                            .ok_or_else(|| error(&fallback(owner), "Missing signature span"))?;
                        observation.serialized(owner, &owner.source)?;
                        observation.reserve(128 + 32 + 128, &owner.source)?;
                        let rows = Rc::new(rows);
                        prepared
                            .try_borrow_mut()
                            .map_err(|_| error(span, "Feature signature code is already borrowed"))?
                            .signatures
                            .insert(owner.clone(), Rc::clone(&rows));
                        ErasedSignatures::Shared(rows)
                    } else {
                        ErasedSignatures::Owned(rows)
                    },
                );
                Phase::Complete
            }
            Phase::Complete => Phase::Complete,
            Phase::Failed => {
                return Err(error(&fallback(owner), "Signature preparation has failed"));
            }
        };
        Ok(matches!(self.phase, Phase::Complete))
    }
    pub(in super::super) fn finish(
        self,
        owner: &DeclarationId,
    ) -> Result<ErasedSignatures, ImportError> {
        if !matches!(self.phase, Phase::Complete) {
            return Err(error(
                &fallback(owner),
                "Signature preparation is incomplete or failed",
            ));
        }
        match self.rows {
            Rows::Complete(rows) => Ok(rows),
            Rows::Building(_) => Err(error(&fallback(owner), "Missing complete signature output")),
        }
    }
}

#[cfg(test)]
#[path = "generic_signature_preparation_tests.rs"]
mod tests;

#[cfg(test)]
mod substitution_consumption {
    use super::super::tests::{fixture, prepared};
    use super::*;
    use crate::definitions::{conversion::InvocationContext, ecore::ParserObservation};
    #[test]
    fn signature_owner_advances_individual_substitution_before_publication() {
        let (bundle, types) = fixture();
        let resolver = prepared(&bundle, true);
        let owner = DeclarationId {
            source: bundle.sources[0].id.clone(),
            path: vec![0, 1],
        };
        let environment = Substitution::new(vec![ParameterBinding {
            parameter: DeclarationId {
                source: owner.source.clone(),
                path: vec![0, 0],
            },
            argument: types[1].expression().clone(),
        }])
        .unwrap();
        let mut job = SignaturePreparation::begin(&resolver, false);
        let mut steps = 0;
        let mut normalization_steps = 0;
        let mut legacy_steps = 0;
        loop {
            job = std::hint::black_box(job);
            if matches!(job.phase, Phase::Agreement(_)) {
                legacy_steps += 1;
            }
            if matches!(job.phase, Phase::DeclaredType(_)) {
                normalization_steps += 1;
            }
            if matches!(job.phase, Phase::Specialize(_)) {
                steps += 1;
            }
            let context = InvocationContext::default();
            if job
                .advance(&owner, &environment, &ParserObservation::new(&context))
                .unwrap()
            {
                break;
            }
        }
        assert!(
            steps >= 6,
            "signature specialization was synchronously drained"
        );
        assert!(
            normalization_steps >= 6,
            "signature normalization was synchronously drained"
        );
        assert!(
            legacy_steps >= 6,
            "actual owner drained legacy agreement synchronously"
        );
        let output = job.finish(&owner).unwrap();
        let rows = serde_json::to_value(output).unwrap();
        assert_eq!(
            rows[0]["specialized"][0]["declaration"]["path"],
            serde_json::json!([1])
        );
    }
}
