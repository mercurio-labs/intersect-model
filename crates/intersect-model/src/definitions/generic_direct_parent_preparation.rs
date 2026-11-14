//! Direct-parent orchestration shared by ordinary and cooperative consumers.
//! The driver supplies fixed receiver/span inputs and a fresh observer per step.
//! Formal/individual type decoding and the duplicate predicate
//! keep explicit synchronous boundaries; no observer or registry is retained.
use super::*;
use crate::definitions::generic_types::{SubstitutionPreparation, TypeExpressionPreparation};
use std::borrow::Cow;
enum FormalInputs<'a> {
    Owned(std::vec::IntoIter<TypeParameter>),
    Borrowed(std::slice::Iter<'a, TypeParameter>),
}
impl FormalInputs<'_> {
    fn next(&mut self) -> Option<Cow<'_, DeclarationId>> {
        match self {
            Self::Owned(formals) => formals.next().map(|formal| Cow::Owned(formal.declaration)),
            Self::Borrowed(formals) => formals
                .next()
                .map(|formal| Cow::Borrowed(&formal.declaration)),
        }
    }
}
struct ParentRows {
    environment: Substitution,
    signatures: std::vec::IntoIter<RawSignature>,
    inputs: std::vec::IntoIter<DeclarationId>,
    generic: bool,
}
// Keep partial specialization inline under the unchanged owning holder fee.
#[allow(clippy::large_enum_variant)]
enum Phase<'a> {
    Begin,
    Bindings {
        formals: FormalInputs<'a>,
        bindings: Vec<ParameterBinding>,
        index: usize,
    },
    Environment(Vec<ParameterBinding>),
    Agreement {
        environment: Substitution,
        preparation: AgreementPreparation<'a>,
    },
    Rows(ParentRows),
    Declared {
        rows: ParentRows,
        preparation: TypeExpressionPreparation<'a>,
    },
    Specialize {
        rows: ParentRows,
        input: Type,
        location: SourceSpan,
        preparation: SubstitutionPreparation,
    },
    Duplicates,
    Complete,
    Failed,
}
pub(crate) struct DirectParentPreparation<'a> {
    resolver: &'a Resolver,
    result: Vec<(Type, SourceSpan)>,
    phase: Phase<'a>,
}
impl<'a> DirectParentPreparation<'a> {
    pub(crate) fn begin(resolver: &'a Resolver) -> Self {
        Self {
            resolver,
            result: vec![],
            phase: Phase::Begin,
        }
    }
    #[cfg(test)]
    pub(crate) fn is_source_decoding_step(&self) -> bool {
        match &self.phase {
            Phase::Agreement { preparation, .. } => preparation.is_source_decoding_step(),
            Phase::Declared { preparation, .. } => preparation.is_retained_source_step(),
            _ => false,
        }
    }
    pub(crate) fn is_silent_agreement_step(&self) -> bool {
        matches!(self.phase, Phase::Agreement { .. })
            || matches!(&self.phase, Phase::Declared { preparation, .. } if preparation.is_retained_source_step())
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
        let phase = std::mem::replace(&mut self.phase, Phase::Failed);
        // Only Begin may issue the original receiver diagnostic. Every later
        // step uses the same immutable input supplied by the private driver.
        let declaration = match ty {
            Type::Classifier { declaration, .. } => Some(declaration),
            _ => None,
        };
        self.phase = match phase {
            Phase::Begin => {
                observation.work(1, &span.source)?;
                let Type::Classifier {
                    declaration,
                    arguments,
                    raw,
                } = ty
                else {
                    return Err(error(span, "Parent lookup requires a classifier"));
                };
                let formals = self
                    .resolver
                    .formal_info_observed(declaration, observation)?;
                if *raw {
                    if formals.is_empty() || !arguments.is_empty() {
                        return Err(error(span, "Invalid raw generic inheritance application"));
                    }
                } else if formals.len() != arguments.len() {
                    return Err(error(span, "Generic inheritance argument count mismatch"));
                }
                if arguments.is_empty() {
                    Phase::Environment(vec![])
                } else {
                    Phase::Bindings {
                        formals: match formals {
                            Cow::Owned(formals) => FormalInputs::Owned(formals.into_iter()),
                            Cow::Borrowed(formals) => FormalInputs::Borrowed(formals.iter()),
                        },
                        bindings: vec![],
                        index: 0,
                    }
                }
            }
            Phase::Bindings {
                mut formals,
                mut bindings,
                mut index,
            } => {
                if let Some(parameter) = formals.next() {
                    let Type::Classifier { arguments, .. } = ty else {
                        return Err(error(span, "Missing direct-parent receiver"));
                    };
                    let argument = arguments
                        .get(index)
                        .ok_or_else(|| error(span, "Missing direct-parent argument"))?;
                    observation.work(1, &span.source)?;
                    let parameter = match parameter {
                        Cow::Owned(parameter) => parameter,
                        Cow::Borrowed(parameter) => {
                            observation.serialized(parameter, &span.source)?;
                            parameter.clone()
                        }
                    };
                    observation.serialized(argument, &span.source)?;
                    observation.reserve(32, &span.source)?;
                    bindings.push(ParameterBinding {
                        parameter,
                        argument: argument.clone(),
                    });
                    index += 1;
                    Phase::Bindings {
                        formals,
                        bindings,
                        index,
                    }
                } else {
                    Phase::Environment(bindings)
                }
            }
            Phase::Environment(bindings) => {
                declaration.ok_or_else(|| error(span, "Missing direct-parent receiver"))?;
                let environment = observation.preparation_at(span, |context| {
                    Substitution::new_observed(bindings, context)
                })?;
                Phase::Agreement {
                    environment,
                    preparation: if retain_source_decoding {
                        AgreementPreparation::begin_with_retained_source_decoding()
                    } else {
                        AgreementPreparation::begin()
                    },
                }
            }
            Phase::Agreement {
                environment,
                mut preparation,
            } => {
                let declaration =
                    declaration.ok_or_else(|| error(span, "Missing direct-parent receiver"))?;
                let context = crate::definitions::conversion::InvocationContext::default();
                let nested = crate::definitions::ecore::ParserObservation::new(&context);
                if preparation.advance(self.resolver, declaration, &nested)? {
                    Phase::Rows(ParentRows {
                        environment,
                        signatures: preparation.finish(declaration)?.into_iter(),
                        inputs: vec![].into_iter(),
                        generic: false,
                    })
                } else {
                    Phase::Agreement {
                        environment,
                        preparation,
                    }
                }
            }
            Phase::Rows(mut rows) => {
                let declaration =
                    declaration.ok_or_else(|| error(span, "Missing direct-parent receiver"))?;
                if !rows.inputs.as_slice().is_empty() {
                    observation.work(1, &span.source)?;
                    if rows.generic {
                        Phase::Declared {
                            rows,
                            preparation: if retain_source_decoding {
                                TypeExpressionPreparation::begin_with_retained_source_decoding()
                            } else {
                                TypeExpressionPreparation::begin()
                            },
                        }
                    } else {
                        let parent = rows
                            .inputs
                            .next()
                            .ok_or_else(|| error(span, "Missing direct-parent input"))?;
                        let raw = !self
                            .resolver
                            .formal_info_observed(&parent, observation)?
                            .is_empty();
                        let parent_type = Type::Classifier {
                            declaration: parent,
                            arguments: vec![],
                            raw,
                        };
                        let location = &self
                            .resolver
                            .node_observed(declaration, observation)?
                            .opening;
                        observation.serialized(location, &span.source)?;
                        observation.reserve(32, &span.source)?;
                        self.result.push((parent_type, location.clone()));
                        Phase::Rows(rows)
                    }
                } else if let Some(signature) = rows.signatures.next() {
                    observation.work(1, &span.source)?;
                    if signature.feature == "eSuperTypes" {
                        rows.generic = !signature.generic.is_empty();
                        rows.inputs = if rows.generic {
                            signature.generic
                        } else {
                            signature.raw_types
                        }
                        .into_iter();
                    }
                    Phase::Rows(rows)
                } else {
                    Phase::Duplicates
                }
            }
            Phase::Declared {
                mut rows,
                mut preparation,
            } => {
                let selected = rows
                    .inputs
                    .as_slice()
                    .first()
                    .ok_or_else(|| error(span, "Missing direct-parent selection"))?;
                // Preserve the original silent nested policy. Only this advance's
                // default context is borrowed; normalization itself is retained.
                let context = crate::definitions::conversion::InvocationContext::default();
                let nested = crate::definitions::ecore::ParserObservation::new(&context);
                if preparation.advance(self.resolver, selected, &nested)? {
                    let declared = preparation.finish(selected)?;
                    rows.inputs.next();
                    Phase::Specialize {
                        rows,
                        input: declared.expression,
                        location: declared.span,
                        preparation: SubstitutionPreparation::simultaneous(),
                    }
                } else {
                    Phase::Declared { rows, preparation }
                }
            }
            Phase::Specialize {
                rows,
                input,
                location,
                mut preparation,
            } => {
                if observation.preparation_at(&location, |context| {
                    preparation.advance(&input, &rows.environment, context)
                })? {
                    let specialized =
                        observation.preparation_at(&location, |_| preparation.finish())?;
                    observation.reserve(32, &span.source)?;
                    self.result.push((specialized, location));
                    Phase::Rows(rows)
                } else {
                    Phase::Specialize {
                        rows,
                        input,
                        location,
                        preparation,
                    }
                }
            }
            Phase::Duplicates => {
                self.resolver.check_direct_parent_duplicates_observed(
                    &self.result,
                    span,
                    observation,
                )?;
                Phase::Complete
            }
            Phase::Complete => Phase::Complete,
            Phase::Failed => return Err(error(span, "Direct-parent preparation has failed")),
        };
        Ok(matches!(self.phase, Phase::Complete))
    }
    pub(crate) fn finish(self, span: &SourceSpan) -> Result<Vec<(Type, SourceSpan)>, ImportError> {
        if !matches!(self.phase, Phase::Complete) {
            return Err(error(
                span,
                "Direct-parent preparation is incomplete or failed",
            ));
        }
        Ok(self.result)
    }
}

#[cfg(test)]
mod substitution_consumption {
    use super::*;
    use crate::definitions::{
        bundle::DefinitionSource, conversion::InvocationContext, ecore::ParserObservation,
    };
    #[test]
    fn parent_owner_advances_individual_substitution_before_publication() {
        let source = DefinitionSource::new(
            "memory:substitution-parent-owner.ecore",
            SourceKind::Ecore,
            "<e:EPackage xmlns:e='http://www.eclipse.org/emf/2002/Ecore' xmlns:x='http://www.w3.org/2001/XMLSchema-instance' name='p' nsURI='urn:parent'><eClassifiers x:type='e:EClass' name='Leaf'/><eClassifiers x:type='e:EClass' name='Base'><eTypeParameters name='T'/></eClassifiers><eClassifiers x:type='e:EClass' name='Child'><eTypeParameters name='U'/><eGenericSuperTypes eClassifier='#//Base'><eTypeArguments eTypeParameter='#//Child/U'/></eGenericSuperTypes></eClassifiers></e:EPackage>",
        );
        let id = |n| DeclarationId {
            source: source.id.clone(),
            path: vec![n],
        };
        let leaf = Type::Classifier {
            declaration: id(0),
            arguments: vec![],
            raw: false,
        };
        let root = Type::Classifier {
            declaration: id(2),
            arguments: vec![leaf.clone()],
            raw: false,
        };
        let resolver = Resolver::new(&DefinitionBundle {
            version: 1,
            sources: vec![source.clone()],
            namespaces: vec![],
        })
        .unwrap();
        let span = SourceSpan {
            source: source.id.clone(),
            start: 11,
            end: 19,
        };
        let mut job = DirectParentPreparation::begin(&resolver);
        let mut steps = 0;
        let mut normalization_steps = 0;
        let mut legacy_steps = 0;
        loop {
            job = std::hint::black_box(job);
            if matches!(job.phase, Phase::Agreement { .. }) {
                legacy_steps += 1;
            }
            if matches!(job.phase, Phase::Declared { .. }) {
                normalization_steps += 1;
            }
            if matches!(job.phase, Phase::Specialize { .. }) {
                steps += 1;
            }
            let context = InvocationContext::default();
            if job
                .advance(&root, &span, &ParserObservation::new(&context))
                .unwrap()
            {
                break;
            }
        }
        assert!(
            steps >= 6,
            "parent specialization was synchronously drained"
        );
        assert!(
            normalization_steps >= 6,
            "parent normalization was synchronously drained"
        );
        assert!(
            legacy_steps >= 6,
            "actual owner drained legacy agreement synchronously"
        );
        let output = job.finish(&span).unwrap();
        assert_eq!(output.len(), 1);
        assert_eq!(
            output[0].0,
            Type::Classifier {
                declaration: id(1),
                arguments: vec![leaf],
                raw: false
            }
        );
    }
}
