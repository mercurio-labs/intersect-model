//! Shared source-member projection over fixed resolver/root inputs. Each
//! operation supplies current observation; no root copy, observer or registry
//! is retained. Nested agreement/type-expression/parameter helpers remain.
use super::super::inheritance::{
    AncestryTraversal, AncestryView, AncestryViewPreparation, InheritedType,
};
use super::*;
use crate::definitions::generic_types::{SubstitutionPreparation, TypeExpressionPreparation};
use std::borrow::Cow;
enum Owners {
    Empty,
    Owned(Vec<Type>),
    View(AncestryView),
}
impl Owners {
    fn get<'s>(&'s self, root: &'s Type, index: usize) -> Option<&'s Type> {
        match self {
            Self::Empty => None,
            Self::Owned(rows) => rows.get(index),
            Self::View(rows) => {
                if index == 0 {
                    Some(root)
                } else {
                    rows.as_slice().get(index - 1).map(|row| &row.expression)
                }
            }
        }
    }
}
// Retained child/signature/bound preparation stays in the admitted inline owner.
// Avoid an unadmitted Box or higher fee; the original 2,048-byte owner assertion still applies.
#[allow(clippy::large_enum_variant)]
enum Phase<'a> {
    Begin,
    Ancestry(AncestryTraversal<'a>),
    Ancestors(std::vec::IntoIter<InheritedType>),
    View(AncestryViewPreparation<'a>),
    Owner,
    Bindings {
        formals: Cow<'a, [TypeParameter]>,
        bindings: Vec<ParameterBinding>,
        index: usize,
    },
    Children(Children<'a>),
    Complete,
    Failed,
}
struct Children<'a> {
    node: &'a Node,
    environment: Substitution,
    index: usize,
    member: Option<PendingMember<'a>>,
    step: MemberStep<'a>,
}
struct PendingMember<'a> {
    id: DeclarationId,
    name: Cow<'a, str>,
    signatures: ErasedSignatures,
    formals: Vec<SpecializedFormal>,
}
// The resolved formal is already admitted. Keep it inline across bound advances rather than add a boxed allocation and new fees.
enum BoundSpecialization<'a> {
    Declaration(TypeExpressionPreparation<'a>),
    Substitution(SubstitutionPreparation),
}
#[allow(clippy::large_enum_variant)]
enum MemberStep<'a> {
    Scan,
    Signatures(SignaturePreparation<'a>),
    ParameterSignatures {
        index: usize,
        owner: DeclarationId,
        preparation: SignaturePreparation<'a>,
    },
    Formals(std::vec::IntoIter<TypeParameter>),
    Bounds {
        formals: std::vec::IntoIter<TypeParameter>,
        syntax: TypeParameter,
        pending: std::vec::IntoIter<ResolvedType>,
        bounds: Vec<Type>,
        current: Option<BoundSpecialization<'a>>,
    },
    Parameters(usize),
    Targets {
        reference: bool,
        signature: usize,
        ty: usize,
    },
    Publish,
}
pub(super) struct SourceMembersPreparation<'a> {
    resolver: &'a Resolver,
    runtime: bool,
    views: bool,
    owners: Owners,
    owner: usize,
    members: SourceMembers<'a>,
    phase: Phase<'a>,
}
impl<'a> SourceMembersPreparation<'a> {
    pub(super) fn begin(resolver: &'a Resolver, runtime: bool, views: bool) -> Self {
        Self {
            resolver,
            runtime,
            views,
            owners: Owners::Empty,
            owner: 0,
            members: if views {
                SourceMembers::Views(vec![])
            } else {
                SourceMembers::Owned(vec![])
            },
            phase: Phase::Begin,
        }
    }
    // The private caller supplies the same immutable root and diagnostic span on
    // each advance. Source rows may borrow the resolver, never movable root state.
    pub(crate) fn is_silent_agreement_step(&self) -> bool {
        match &self.phase {
            Phase::Ancestry(preparation) => preparation.is_silent_agreement_step(),
            Phase::View(preparation) => preparation.is_silent_agreement_step(),
            Phase::Children(children) => match &children.step {
                MemberStep::Signatures(preparation)
                | MemberStep::ParameterSignatures { preparation, .. } => {
                    preparation.is_silent_agreement_step()
                }
                MemberStep::Bounds {
                    current: Some(BoundSpecialization::Declaration(preparation)),
                    ..
                } => preparation.is_retained_source_step(),
                _ => false,
            },
            _ => false,
        }
    }
    #[cfg(test)]
    pub(crate) fn is_source_decoding_step(&self) -> bool {
        match &self.phase {
            Phase::Ancestry(preparation) => preparation.is_source_decoding_step(),
            Phase::View(preparation) => preparation.is_source_decoding_step(),
            Phase::Children(children) => match &children.step {
                MemberStep::Signatures(preparation)
                | MemberStep::ParameterSignatures { preparation, .. } => {
                    preparation.is_source_decoding_step()
                }
                MemberStep::Bounds {
                    current: Some(BoundSpecialization::Declaration(preparation)),
                    ..
                } => preparation.is_retained_source_step(),
                _ => false,
            },
            _ => false,
        }
    }
    pub(super) fn advance(
        &mut self,
        expression: &Type,
        span: &SourceSpan,
        observation: &crate::definitions::ecore::ParserObservation<'_, '_>,
    ) -> Result<bool, ImportError> {
        self.advance_with_source_decoding(expression, span, observation, false)
    }
    pub(super) fn advance_with_source_decoding(
        &mut self,
        expression: &Type,
        span: &SourceSpan,
        observation: &crate::definitions::ecore::ParserObservation<'_, '_>,
        retain_source_decoding: bool,
    ) -> Result<bool, ImportError> {
        let resolver: &'a Resolver = self.resolver;
        let phase = std::mem::replace(&mut self.phase, Phase::Failed);
        self.phase = match phase {
            Phase::Begin => {
                if resolver.prepared_ancestry.get().is_some() {
                    Phase::View(AncestryViewPreparation::begin(resolver))
                } else {
                    observation.serialized(expression, &span.source)?;
                    observation.reserve(32, &span.source)?;
                    self.owners = Owners::Owned(Vec::new());
                    if let Owners::Owned(owners) = &mut self.owners {
                        owners.push(expression.clone());
                    }
                    Phase::Ancestry(AncestryTraversal::begin(resolver))
                }
            }
            Phase::Ancestry(mut traversal) => {
                if traversal.advance_with_source_decoding(
                    expression,
                    span,
                    observation,
                    retain_source_decoding,
                )? {
                    Phase::Ancestors(traversal.finish(span)?.into_iter())
                } else {
                    Phase::Ancestry(traversal)
                }
            }
            Phase::Ancestors(mut pending) => {
                if let Some(ancestor) = pending.next() {
                    observation.work(1, &span.source)?;
                    observation.reserve(32, &span.source)?;
                    let Owners::Owned(owners) = &mut self.owners else {
                        return Err(error(span, "Missing owned projection ancestry"));
                    };
                    owners.push(ancestor.expression);
                    Phase::Ancestors(pending)
                } else {
                    Phase::Owner
                }
            }
            Phase::View(mut preparation) => {
                if preparation.advance_with_source_decoding(
                    expression,
                    span,
                    observation,
                    retain_source_decoding,
                )? {
                    self.owners = Owners::View(preparation.finish(span)?);
                    Phase::Owner
                } else {
                    Phase::View(preparation)
                }
            }
            Phase::Owner => {
                if let Some(owner) = self.owners.get(expression, self.owner) {
                    observation.work(1, &span.source)?;
                    let Type::Classifier { declaration, .. } = owner else {
                        return Err(error(
                            span,
                            "Member receiver requires an EClass application",
                        ));
                    };
                    let formals = resolver.formal_info_observed(declaration, observation)?;
                    Phase::Bindings {
                        formals,
                        bindings: vec![],
                        index: 0,
                    }
                } else {
                    Phase::Complete
                }
            }
            Phase::Bindings {
                formals,
                mut bindings,
                index,
            } => {
                let owner = self
                    .owners
                    .get(expression, self.owner)
                    .ok_or_else(|| error(span, "Missing projection owner"))?;
                let Type::Classifier {
                    declaration,
                    arguments,
                    ..
                } = owner
                else {
                    return Err(error(
                        span,
                        "Member receiver requires an EClass application",
                    ));
                };
                if let (Some(formal), Some(argument)) = (formals.get(index), arguments.get(index)) {
                    observation.work(1, &span.source)?;
                    observation.serialized(&formal.declaration, &span.source)?;
                    observation.serialized(argument, &span.source)?;
                    observation.reserve(32, &span.source)?;
                    bindings.push(ParameterBinding {
                        parameter: formal.declaration.clone(),
                        argument: argument.clone(),
                    });
                    Phase::Bindings {
                        formals,
                        bindings,
                        index: index + 1,
                    }
                } else {
                    let environment = observation.preparation_at(span, |context| {
                        Substitution::new_observed(bindings, context)
                    })?;
                    let node = resolver.node_observed(declaration, observation)?;
                    Phase::Children(Children {
                        node,
                        environment,
                        index: 0,
                        member: None,
                        step: MemberStep::Scan,
                    })
                }
            }
            Phase::Children(mut children) => {
                let owner = self
                    .owners
                    .get(expression, self.owner)
                    .ok_or_else(|| error(span, "Missing member projection owner"))?;
                let step = std::mem::replace(&mut children.step, MemberStep::Scan);
                if matches!(step, MemberStep::Scan)
                    && children.node.children.get(children.index).is_none()
                {
                    self.owner += 1;
                    self.phase = Phase::Owner;
                    return Ok(false);
                }
                let node = children
                    .node
                    .children
                    .get(children.index)
                    .ok_or_else(|| error(span, "Missing retained member node"))?;
                match step {
                    MemberStep::Scan => {
                        observation.work(1, &span.source)?;
                        if !named(node, "eStructuralFeatures") && !named(node, "eOperations") {
                            children.index += 1;
                        } else {
                            if self.members.len() >= 16_384 {
                                return Err(ImportError::new(
                                    ErrorKind::ResourceLimit,
                                    &node.span,
                                    "Generic member projection budget exceeded",
                                ));
                            }
                            let Type::Classifier { declaration, .. } = owner else {
                                return Err(error(
                                    span,
                                    "Member receiver requires an EClass application",
                                ));
                            };
                            observation.serialized(declaration, &span.source)?;
                            observation.reserve(32, &span.source)?;
                            let mut id = declaration.clone();
                            id.path.push(children.index);
                            let name = attr(node, "name")
                                .ok_or_else(|| error(&node.span, "Missing member name"))?;
                            let name = if self.views {
                                Cow::Borrowed(name)
                            } else {
                                observation.string(name.len(), &span.source)?;
                                Cow::Owned(name.to_owned())
                            };
                            children.step = MemberStep::Signatures(SignaturePreparation::begin(
                                resolver,
                                self.views && named(node, "eStructuralFeatures"),
                            ));
                            children.member = Some(PendingMember {
                                id,
                                name,
                                signatures: ErasedSignatures::Owned(vec![]),
                                formals: vec![],
                            });
                        }
                    }
                    MemberStep::Signatures(mut preparation) => {
                        let member = children
                            .member
                            .as_mut()
                            .ok_or_else(|| error(span, "Missing member signature inputs"))?;
                        if if retain_source_decoding {
                            preparation.advance_with_source_decoding(
                                &member.id,
                                &children.environment,
                                observation,
                                true,
                            )
                        } else {
                            preparation.advance(&member.id, &children.environment, observation)
                        }? {
                            member.signatures = preparation.finish(&member.id)?;
                            children.step = if named(node, "eOperations") {
                                MemberStep::Formals(
                                    resolver
                                        .formals_observed(&member.id, observation)?
                                        .into_iter(),
                                )
                            } else {
                                MemberStep::Targets {
                                    reference: kind_observed(node, observation)? == "EReference",
                                    signature: 0,
                                    ty: 0,
                                }
                            };
                        } else {
                            children.step = MemberStep::Signatures(preparation);
                        }
                    }
                    MemberStep::ParameterSignatures {
                        index,
                        owner,
                        mut preparation,
                    } => {
                        if if retain_source_decoding {
                            preparation.advance_with_source_decoding(
                                &owner,
                                &children.environment,
                                observation,
                                true,
                            )
                        } else {
                            preparation.advance(&owner, &children.environment, observation)
                        }? {
                            let ErasedSignatures::Owned(additions) = preparation.finish(&owner)?
                            else {
                                return Err(error(
                                    &node.span,
                                    "Operation parameter signatures require owned projection",
                                ));
                            };
                            let member = children
                                .member
                                .as_mut()
                                .ok_or_else(|| error(span, "Missing operation signatures"))?;
                            let ErasedSignatures::Owned(rows) = &mut member.signatures else {
                                return Err(error(
                                    &node.span,
                                    "Operation signatures require owned projection",
                                ));
                            };
                            rows.extend(additions);
                            children.step = MemberStep::Parameters(index + 1);
                        } else {
                            children.step = MemberStep::ParameterSignatures {
                                index,
                                owner,
                                preparation,
                            };
                        }
                    }
                    MemberStep::Formals(mut formals) => {
                        if let Some(formal) = formals.next() {
                            observation.work(1, &span.source)?;
                            let checked = resolver.resolve_parameter(&formal.declaration)?;
                            children.step = MemberStep::Bounds {
                                formals,
                                syntax: checked.syntax,
                                pending: checked.bounds.into_iter(),
                                bounds: vec![],
                                current: None,
                            };
                        } else {
                            children.step = MemberStep::Parameters(0);
                        }
                    }
                    MemberStep::Bounds {
                        formals,
                        syntax,
                        mut pending,
                        mut bounds,
                        current,
                    } => {
                        if let Some(current) = current {
                            let selected = pending
                                .as_mut_slice()
                                .first_mut()
                                .ok_or_else(|| error(span, "Missing bound selection"))?;
                            let current = match current {
                                BoundSpecialization::Declaration(mut preparation) => {
                                    let context =
                                        crate::definitions::conversion::InvocationContext::default(
                                        );
                                    let nested =
                                        crate::definitions::ecore::ParserObservation::new(&context);
                                    if preparation.advance(
                                        resolver,
                                        &selected.syntax.declaration,
                                        &nested,
                                    )? {
                                        let declared =
                                            preparation.finish(&selected.syntax.declaration)?;
                                        observation.reserve(32, &span.source)?;
                                        bounds.push(declared.expression);
                                        // Reuse the existing admitted span slot, preserving
                                        // the fresh normalization diagnostic identity exactly.
                                        selected.syntax.span = declared.span;
                                        Some(BoundSpecialization::Substitution(
                                            SubstitutionPreparation::simultaneous(),
                                        ))
                                    } else {
                                        Some(BoundSpecialization::Declaration(preparation))
                                    }
                                }
                                BoundSpecialization::Substitution(mut preparation) => {
                                    let location = &selected.syntax.span;
                                    let input = bounds.last().ok_or_else(|| {
                                        error(span, "Missing bound specialization input")
                                    })?;
                                    if observation.preparation_at(location, |context| {
                                        preparation.advance(input, &children.environment, context)
                                    })? {
                                        let output = observation
                                            .preparation_at(location, |_| preparation.finish())?;
                                        *bounds.last_mut().ok_or_else(|| {
                                            error(span, "Missing bound specialization slot")
                                        })? = output;
                                        pending.next();
                                        None
                                    } else {
                                        Some(BoundSpecialization::Substitution(preparation))
                                    }
                                }
                            };
                            children.step = MemberStep::Bounds {
                                formals,
                                syntax,
                                pending,
                                bounds,
                                current,
                            };
                        } else if !pending.as_slice().is_empty() {
                            observation.work(1, &span.source)?;
                            children.step = MemberStep::Bounds {
                                formals,
                                syntax,
                                pending,
                                bounds,
                                current: Some(BoundSpecialization::Declaration(
                                    if retain_source_decoding {
                                        TypeExpressionPreparation::begin_with_retained_source_decoding()
                                    } else {
                                        TypeExpressionPreparation::begin()
                                    },
                                )),
                            };
                        } else {
                            observation.reserve(32, &span.source)?;
                            children
                                .member
                                .as_mut()
                                .ok_or_else(|| error(span, "Missing projected operation"))?
                                .formals
                                .push(SpecializedFormal {
                                    declaration: syntax,
                                    bounds,
                                });
                            children.step = MemberStep::Formals(formals);
                        }
                    }
                    MemberStep::Parameters(index) => {
                        if let Some(parameter) = node.children.get(index) {
                            observation.work(1, &span.source)?;
                            if named(parameter, "eParameters") {
                                let member = children
                                    .member
                                    .as_mut()
                                    .ok_or_else(|| error(span, "Missing operation signatures"))?;
                                observation.serialized(&member.id, &span.source)?;
                                observation.reserve(32, &span.source)?;
                                let mut parameter_id = member.id.clone();
                                parameter_id.path.push(index);
                                if !matches!(member.signatures, ErasedSignatures::Owned(_)) {
                                    return Err(error(
                                        &node.span,
                                        "Operation signatures require owned projection",
                                    ));
                                }
                                children.step = MemberStep::ParameterSignatures {
                                    index,
                                    owner: parameter_id,
                                    preparation: SignaturePreparation::begin(resolver, false),
                                };
                            } else {
                                children.step = MemberStep::Parameters(index + 1);
                            }
                        } else {
                            children.step = MemberStep::Publish;
                        }
                    }
                    MemberStep::Targets {
                        reference,
                        signature,
                        ty,
                    } => {
                        let member = children
                            .member
                            .as_ref()
                            .ok_or_else(|| error(span, "Missing feature signatures"))?;
                        if let Some(row) = member.signatures.get(signature) {
                            if let Some(target) = row.specialized.get(ty) {
                                match target {
                                    Type::Classifier { declaration, .. } => {
                                        if (resolver
                                            .classifier_observed(declaration, observation)?
                                            == TargetKind::Class)
                                            != reference
                                        {
                                            return Err(error(
                                                &node.span,
                                                "Substituted target disagrees with attribute/reference kind",
                                            ));
                                        }
                                    }
                                    _ if self.runtime => {}
                                    _ => {
                                        return Err(ImportError::new(
                                            ErrorKind::Unsupported,
                                            &node.span,
                                            "Open or wildcard feature target requires E08 specialization policy",
                                        ));
                                    }
                                }
                                children.step = MemberStep::Targets {
                                    reference,
                                    signature,
                                    ty: ty + 1,
                                };
                            } else {
                                children.step = MemberStep::Targets {
                                    reference,
                                    signature: signature + 1,
                                    ty: 0,
                                };
                            }
                        } else {
                            children.step = MemberStep::Publish;
                        }
                    }
                    MemberStep::Publish => {
                        let member = children
                            .member
                            .take()
                            .ok_or_else(|| error(span, "Missing complete projected member"))?;
                        publish_member(
                            resolver,
                            owner,
                            node,
                            member,
                            &mut self.members,
                            span,
                            observation,
                        )?;
                        children.index += 1;
                    }
                }
                Phase::Children(children)
            }
            Phase::Complete => Phase::Complete,
            Phase::Failed => return Err(error(span, "Source member preparation has failed")),
        };
        Ok(matches!(self.phase, Phase::Complete))
    }
    pub(super) fn finish(self, span: &SourceSpan) -> Result<SourceMembers<'a>, ImportError> {
        match self.phase {
            Phase::Complete => Ok(self.members),
            Phase::Failed => Err(error(span, "Source member preparation has failed")),
            _ => Err(error(span, "Source member preparation is incomplete")),
        }
    }
}
fn publish_member<'a>(
    resolver: &'a Resolver,
    owner: &Type,
    node: &'a Node,
    member: PendingMember<'a>,
    members: &mut SourceMembers<'a>,
    span: &SourceSpan,
    observation: &crate::definitions::ecore::ParserObservation<'_, '_>,
) -> Result<(), ImportError> {
    let PendingMember {
        id,
        name,
        signatures,
        formals,
    } = member;
    let document = resolver
        .resources(&id)
        .document(&id.source)
        .map_err(|message| error(&node.span, message))?;
    let source_payload = document
        .source
        .text
        .get(node.span.start..node.span.end)
        .ok_or_else(|| error(&node.span, "Invalid retained member source span"))?;
    observation.work(source_payload.len().div_ceil(256).max(1), &span.source)?;
    match members {
        SourceMembers::Owned(rows) => {
            observation.string(source_payload.len(), &span.source)?;
            let source_payload = source_payload.to_owned();
            observation.serialized(owner, &span.source)?;
            observation.serialized(&node.span, &span.source)?;
            observation.reserve(32, &span.source)?;
            rows.push(SpecializedMember {
                declaration: id,
                owner: owner.clone(),
                name: name.into_owned(),
                span: node.span.clone(),
                source_payload,
                signatures: signatures.into_owned(),
                formals,
            });
        }
        SourceMembers::Views(rows) => {
            let input = MemberSourceRef {
                declaration: &id,
                owner,
                name: &name,
                span: &node.span,
                source_payload,
                signatures: &signatures,
                formals: &formals,
            };
            let (declaration, owner) = match resolver.shared_member_identity(input, observation)? {
                Some((declaration, owner)) => (
                    RetainedMemberValue::Shared(declaration),
                    RetainedMemberValue::Shared(owner),
                ),
                None => {
                    observation.serialized(owner, &span.source)?;
                    (
                        RetainedMemberValue::Owned(id),
                        RetainedMemberValue::Owned(owner.clone()),
                    )
                }
            };
            observation.reserve(32, &span.source)?;
            rows.push(ProjectedSourceMember {
                declaration,
                owner,
                name,
                span: std::borrow::Cow::Borrowed(&node.span),
                source_payload: std::borrow::Cow::Borrowed(source_payload),
                signatures,
                formals,
            });
        }
    }

    Ok(())
}

#[cfg(test)]
#[path = "generic_source_members_preparation_tests.rs"]
mod tests;

#[cfg(test)]
mod substitution_consumption {
    use super::super::source_members_capture_tests::inputs;
    use super::*;
    use crate::definitions::{conversion::InvocationContext, ecore::ParserObservation};
    #[test]
    fn operation_bound_owner_advances_individual_substitution_before_publication() {
        let (bundle, root, span) = inputs(35);
        let resolver = Resolver::new(&bundle).unwrap();
        // This frozen fixture is a raw runtime receiver with symbolic feature targets.
        let mut job = SourceMembersPreparation::begin(&resolver, true, false);
        let mut steps = 0;
        let mut normalization_steps = 0;
        loop {
            job = std::hint::black_box(job);
            if let Phase::Children(children) = &job.phase {
                if matches!(
                    children.step,
                    MemberStep::Bounds {
                        current: Some(BoundSpecialization::Declaration(_)),
                        ..
                    }
                ) {
                    normalization_steps += 1;
                }
                if matches!(
                    children.step,
                    MemberStep::Bounds {
                        current: Some(_),
                        ..
                    }
                ) {
                    steps += 1;
                }
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
            "operation-bound specialization was synchronously drained"
        );
        assert!(
            normalization_steps >= 6,
            "operation-bound normalization was synchronously drained"
        );
        let SourceMembers::Owned(rows) = job.finish(&span).unwrap() else {
            panic!("owned report required")
        };
        let operation = rows.iter().find(|row| row.name == "pick").unwrap();
        assert_eq!(operation.formals.len(), 1);
        assert_eq!(
            operation.formals[0].bounds[0],
            Type::Classifier {
                declaration: DeclarationId {
                    source: bundle.sources[0].id.clone(),
                    path: vec![0]
                },
                arguments: vec![],
                raw: false
            }
        );
    }
}
