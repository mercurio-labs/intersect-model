//! Shared runtime/source-member orchestration. Nested signature/bound/erasure
//! algorithms, domain construction and public DTO adoption retain named tails.
use super::super::inheritance::DirectParentPreparation;
use super::*;
use std::borrow::Cow;
#[derive(Clone, Copy, PartialEq, Eq)]
enum Phase {
    Start,
    Bounds,
    Sources,
    RawStart,
    RawScan,
    BuildStart,
    Build,
    Member,
    Signature,
    SignaturePrepare,
    Type,
    MemberFinish,
    ParentsStart,
    ParentsPrepare,
    Parents,
    DomainsStart,
    Domains,
    Finish,
    Complete,
}
enum DomainFilter<'a> {
    Owned {
        pending: std::vec::IntoIter<RuntimeFeatureDomain>,
        kept: Vec<RuntimeFeatureDomain>,
    },
    Views {
        pending: std::vec::IntoIter<FeatureDomainSourceView<'a>>,
        kept: Vec<FeatureDomainSourceView<'a>>,
    },
}
// Source projection and report assembly occupy the same admitted inline slot.
// Source projection reuses the admitted inline assembly slot. Keep the owner size assertions and avoid an additional boxed allocation.
#[allow(clippy::large_enum_variant)]
enum MemberProgress<'a> {
    Empty,
    Sources(SourceMembersPreparation<'a>),
    Assembly(MemberAssembly<'a>),
}
// Retained parent specialization shares the existing admitted inline assembly slot.
// Boxing it would add storage/fees; the original 2,048-byte owner assertion still applies.
#[allow(clippy::large_enum_variant)]
enum ParentInputs<'a> {
    // Agreement and parent preparation never coexist. Reuse this already
    // admitted inline slot rather than allocate or grow the assembly header.
    Agreement(AgreementPreparation<'a>),
    Preparing(DirectParentPreparation<'a>),
    Ready(std::vec::IntoIter<(Type, SourceSpan)>),
}
struct MemberAssembly<'a> {
    source_members: Option<SourceMembers<'a>>,
    source_snapshot: Vec<SpecializedMember>,
    members: Vec<ErasedMember<'a>>,
    reference_bounds: Vec<RetainedReferenceBounds>,
    raw_context: bool,
    member: usize,
    signature: usize,
    ty: usize,
    raw: Vec<RawSignature>,
    parent_inputs: Option<ParentInputs<'a>>,
    parents: Vec<Type>,
    domains: Option<DomainFilter<'a>>,
    feature_domains: Option<PreparedSourceDomains<'a>>,
    retained_receiver: Option<RetainedMemberValue<RuntimeType>>,
}
pub(crate) struct RuntimeMembersPreparation<'a, 'input> {
    resolver: &'a Resolver,
    receiver: Cow<'input, RuntimeType>,
    retain_source_snapshot: bool,
    phase: Phase,
    span: Option<Cow<'a, SourceSpan>>,
    bounds: Vec<BoundCheck>,
    warnings: Vec<Warning>,
    failed: bool,
    checked_bound: usize,
    progress: MemberProgress<'a>,
}
impl<'a> MemberAssembly<'a> {
    fn begin(sources: SourceMembers<'a>) -> Self {
        Self {
            source_members: Some(sources),
            source_snapshot: vec![],
            members: vec![],
            reference_bounds: vec![],
            raw_context: false,
            member: 0,
            signature: 0,
            ty: 0,
            raw: vec![],
            parent_inputs: None,
            parents: vec![],
            domains: None,
            feature_domains: None,
            retained_receiver: None,
        }
    }
}
impl<'a, 'input> RuntimeMembersPreparation<'a, 'input> {
    pub(crate) fn begin(
        resolver: &'a Resolver,
        receiver: Cow<'input, RuntimeType>,
        retain_source_snapshot: bool,
    ) -> Self {
        Self {
            resolver,
            receiver,
            retain_source_snapshot,
            phase: Phase::Start,
            span: None,
            bounds: vec![],
            warnings: vec![],
            failed: false,
            checked_bound: 0,
            progress: MemberProgress::Empty,
        }
    }
    pub(crate) fn resolver(&self) -> &'a Resolver {
        self.resolver
    }
    pub(crate) fn is_silent_agreement_step(&self) -> bool {
        if self.failed {
            return false;
        }
        match (&self.phase, &self.progress) {
            (Phase::Sources, MemberProgress::Sources(preparation)) => {
                preparation.is_silent_agreement_step()
            }
            (Phase::SignaturePrepare, MemberProgress::Assembly(_)) => true,
            (Phase::ParentsPrepare, MemberProgress::Assembly(assembly)) => {
                match &assembly.parent_inputs {
                    Some(ParentInputs::Preparing(preparation)) => {
                        preparation.is_silent_agreement_step()
                    }
                    _ => false,
                }
            }
            _ => false,
        }
    }
    #[cfg(test)]
    pub(crate) fn is_source_decoding_step(&self) -> bool {
        if self.failed {
            return false;
        }
        match (&self.phase, &self.progress) {
            (Phase::Sources, MemberProgress::Sources(preparation)) => {
                preparation.is_source_decoding_step()
            }
            (Phase::SignaturePrepare, MemberProgress::Assembly(assembly)) => {
                matches!(&assembly.parent_inputs, Some(ParentInputs::Agreement(preparation)) if preparation.is_source_decoding_step())
            }
            (Phase::ParentsPrepare, MemberProgress::Assembly(assembly)) => {
                match &assembly.parent_inputs {
                    Some(ParentInputs::Preparing(preparation)) => {
                        preparation.is_source_decoding_step()
                    }
                    _ => false,
                }
            }
            _ => false,
        }
    }
    pub(crate) fn advance(
        &mut self,
        observation: &crate::definitions::ecore::ParserObservation<'_, '_>,
    ) -> Result<bool, ImportError> {
        self.advance_with_source_decoding(observation, false)
    }
    pub(crate) fn advance_with_source_decoding(
        &mut self,
        observation: &crate::definitions::ecore::ParserObservation<'_, '_>,
        retain_source_decoding: bool,
    ) -> Result<bool, ImportError> {
        if self.failed {
            return Err(error(
                &fallback(&DeclarationId {
                    source: self.resolver.standard_id.clone(),
                    path: vec![],
                }),
                "Runtime member preparation has failed",
            ));
        }
        let result = self.advance_inner(observation, retain_source_decoding);
        if result.is_err() {
            self.failed = true;
        }
        result
    }
    fn advance_inner(
        &mut self,
        observation: &crate::definitions::ecore::ParserObservation<'_, '_>,
        retain_source_decoding: bool,
    ) -> Result<bool, ImportError> {
        let resolver: &'a Resolver = self.resolver;
        if self.phase == Phase::Complete {
            return Ok(true);
        }
        if self.phase == Phase::Start {
            let Type::Classifier { declaration, .. } = self.receiver.expression() else {
                return Err(error(
                    &SourceSpan {
                        source: resolver.standard_id.clone(),
                        start: 0,
                        end: 0,
                    },
                    "Runtime receiver requires a classifier",
                ));
            };
            let borrowed_span = &resolver.node_observed(declaration, observation)?.span;
            let span = if self.retain_source_snapshot {
                observation.serialized(borrowed_span, &declaration.source)?;
                Cow::Owned(borrowed_span.clone())
            } else {
                Cow::Borrowed(borrowed_span)
            };
            if resolver.classifier_observed(declaration, observation)? != TargetKind::Class {
                return Err(error(&span, "Runtime receiver requires an EClass"));
            }
            let (bounds, warnings) = resolver.runtime_application_checks_observed(
                self.receiver.expression(),
                &span,
                observation,
            )?;
            self.span = Some(span);
            self.bounds = bounds;
            self.warnings = warnings;
            self.phase = Phase::Bounds;
            return Ok(false);
        }
        let span = self.span.as_ref().ok_or_else(|| {
            error(
                &fallback(&DeclarationId {
                    source: resolver.standard_id.clone(),
                    path: vec![],
                }),
                "Missing runtime member span",
            )
        })?;
        let retain_source_snapshot = self.retain_source_snapshot;
        if self.phase == Phase::Bounds {
            if let Some(bound) = self.bounds.get(self.checked_bound) {
                observation.work(1, &span.source)?;
                if !bound.satisfied {
                    return Err(error(span, "Runtime receiver violates generic bounds"));
                }
                self.checked_bound += 1;
            } else {
                self.progress = MemberProgress::Sources(SourceMembersPreparation::begin(
                    resolver,
                    true,
                    !retain_source_snapshot,
                ));
                self.phase = Phase::Sources;
            }
            return Ok(false);
        }
        if self.phase == Phase::Sources {
            let MemberProgress::Sources(preparation) = &mut self.progress else {
                return Err(error(span, "Missing source-member preparation"));
            };
            if preparation.advance_with_source_decoding(
                self.receiver.expression(),
                span,
                observation,
                retain_source_decoding,
            )? {
                let MemberProgress::Sources(preparation) =
                    std::mem::replace(&mut self.progress, MemberProgress::Empty)
                else {
                    return Err(error(span, "Missing completed source-member preparation"));
                };
                self.progress =
                    MemberProgress::Assembly(MemberAssembly::begin(preparation.finish(span)?));
                self.phase = Phase::RawStart;
            }
            return Ok(false);
        }
        let MemberProgress::Assembly(assembly) = &mut self.progress else {
            return Err(error(span, "Missing runtime-member assembly"));
        };
        match self.phase {
            Phase::RawStart => {
                assembly.raw_context = observation
                    .preparation_at(span, |context| self.receiver.has_raw_observed(context))?;
                self.phase = if assembly.raw_context {
                    Phase::BuildStart
                } else {
                    Phase::RawScan
                };
            }
            Phase::RawScan => {
                let sources = assembly
                    .source_members
                    .as_ref()
                    .ok_or_else(|| error(span, "Missing runtime source members"))?;
                if let Some(member) = sources.get(assembly.member) {
                    observation.work(1, &span.source)?;
                    if !observation.preparation_at(member.span, |context| {
                        RuntimeType::expression_is_concrete_observed(member.owner, context)
                    })? || observation.preparation_at(member.span, |context| {
                        RuntimeType::expression_has_raw_observed(member.owner, context)
                    })? {
                        assembly.raw_context = true;
                        self.phase = Phase::BuildStart;
                    } else {
                        assembly.member += 1;
                    }
                } else {
                    self.phase = Phase::BuildStart;
                }
            }
            Phase::BuildStart => {
                if retain_source_snapshot {
                    let Some(SourceMembers::Owned(rows)) = &assembly.source_members else {
                        return Err(error(span, "Expected owned source snapshot"));
                    };
                    observation.serialized(rows, &span.source)?;
                } else {
                    observation.reserve(128, &span.source)?;
                }
                assembly.member = 0;
                self.phase = Phase::Build;
            }
            Phase::Build => {
                let sources = assembly
                    .source_members
                    .as_ref()
                    .ok_or_else(|| error(span, "Missing runtime source members"))?;
                if let Some(member) = sources.get(assembly.member) {
                    assembly.members.push(if retain_source_snapshot {
                        ErasedMember::from(member.owned_clone())
                    } else {
                        borrowed_member(resolver, member, span, observation)?
                    });
                    assembly.member += 1;
                } else {
                    assembly.member = 0;
                    self.phase = Phase::Member;
                }
            }
            Phase::Member => {
                let original = assembly
                    .source_members
                    .as_ref()
                    .and_then(|sources| sources.get(assembly.member));
                let Some(member) = assembly.members.get_mut(assembly.member) else {
                    self.phase = Phase::ParentsStart;
                    return Ok(false);
                };
                let original =
                    original.ok_or_else(|| error(span, "Missing runtime member source"))?;
                observation.work(1, &span.source)?;
                if let std::borrow::Cow::Owned(projected_owner) = resolver
                    .runtime_classifier_projection_expression_observed(
                        &member.owner,
                        &member.span,
                        observation,
                    )?
                {
                    member.owner = RetainedMemberValue::Owned(projected_owner);
                }
                // Generic methods retain their method-local expressions and remain templates.
                if observation.preparation_at(&member.span, |context| match &member.signatures {
                    ErasedSignatures::Owned(_) => {
                        member.requires_runtime_template_observed(context)
                    }
                    ErasedSignatures::Shared(_) => {
                        requires_runtime_template(&member.formals, original.signatures, context)
                    }
                })? && named(
                    resolver.node_observed(&member.declaration, observation)?,
                    "eOperations",
                ) {
                    assembly.member += 1;
                    return Ok(false);
                }

                assembly.signature = 0;
                self.phase = Phase::Signature;
            }
            Phase::Signature => {
                let original = assembly
                    .source_members
                    .as_ref()
                    .and_then(|s| s.get(assembly.member))
                    .ok_or_else(|| error(span, "Missing runtime member source"))?;
                if original.signatures.get(assembly.signature).is_some() {
                    observation.work(1, &span.source)?;
                    assembly.parent_inputs =
                        Some(ParentInputs::Agreement(if retain_source_decoding {
                            AgreementPreparation::begin_with_retained_source_decoding()
                        } else {
                            AgreementPreparation::begin()
                        }));
                    self.phase = Phase::SignaturePrepare;
                } else {
                    self.phase = Phase::MemberFinish;
                }
            }
            Phase::SignaturePrepare => {
                let owner = &assembly
                    .source_members
                    .as_ref()
                    .and_then(|s| s.get(assembly.member))
                    .and_then(|member| member.signatures.get(assembly.signature))
                    .ok_or_else(|| error(span, "Missing runtime signature owner"))?
                    .owner;
                let Some(ParentInputs::Agreement(preparation)) = assembly.parent_inputs.as_mut()
                else {
                    return Err(error(span, "Missing runtime agreement preparation"));
                };
                let context = crate::definitions::conversion::InvocationContext::default();
                let nested = crate::definitions::ecore::ParserObservation::new(&context);
                if preparation.advance(resolver, owner, &nested)? {
                    let Some(ParentInputs::Agreement(preparation)) = assembly.parent_inputs.take()
                    else {
                        return Err(error(span, "Missing completed runtime agreement"));
                    };
                    assembly.raw = preparation.finish(owner)?;
                    assembly.ty = 0;
                    self.phase = Phase::Type;
                }
            }
            Phase::Type => {
                let original = assembly
                    .source_members
                    .as_ref()
                    .and_then(|s| s.get(assembly.member))
                    .ok_or_else(|| error(span, "Missing runtime member source"))?;
                let signature = original
                    .signatures
                    .get(assembly.signature)
                    .ok_or_else(|| error(span, "Missing runtime signature"))?;
                let Some(ty) = signature.specialized.get(assembly.ty) else {
                    assembly.signature += 1;
                    self.phase = Phase::Signature;
                    return Ok(false);
                };
                let signature_index = assembly.signature;
                let index = assembly.ty;
                let member = assembly
                    .members
                    .get_mut(assembly.member)
                    .ok_or_else(|| error(span, "Missing runtime erased member"))?;
                let raw = &assembly.raw;
                let raw_context = assembly.raw_context;
                let reference_bounds = &mut assembly.reference_bounds;
                observation.work(1, &span.source)?;
                let concrete = observation.preparation_at(&member.span, |context| {
                    RuntimeType::expression_is_concrete_observed(ty, context)
                })?;
                if raw_context
                    && named(
                        resolver.node_observed(&member.declaration, observation)?,
                        "eStructuralFeatures",
                    )
                {
                    let mut constraints = Vec::new();
                    resolver.runtime_feature_bounds(
                        ty,
                        &member.span,
                        0,
                        &mut constraints,
                        !retain_source_snapshot,
                        observation,
                    )?;
                    if kind_observed(
                        resolver.node_observed(&member.declaration, observation)?,
                        observation,
                    )? == "EReference"
                    {
                        let feature = match &member.declaration {
                            RetainedMemberValue::Shared(value) => {
                                RetainedMemberValue::Shared(value.clone())
                            }
                            RetainedMemberValue::Owned(value) => {
                                observation.serialized(value, &span.source)?;
                                RetainedMemberValue::Owned(value.clone())
                            }
                        };
                        observation.reserve(128 + 32, &span.source)?;
                        reference_bounds.push(RetainedReferenceBounds {
                            feature,
                            bounds: constraints,
                        });
                    }
                }
                if concrete {
                    assembly.ty += 1;
                    return Ok(false);
                }
                let mut matching = None;
                for candidate in raw {
                    observation.work(1, &span.source)?;
                    if candidate.feature == signature.feature {
                        matching = Some(candidate);
                        break;
                    }
                }
                let declaration =
                    matching
                        .and_then(|r| r.raw_types.get(index))
                        .ok_or_else(|| {
                            error(&member.span, "Missing source erasure for runtime signature")
                        })?;
                match &mut member.signatures {
                    ErasedSignatures::Owned(rows) => {
                        observation.serialized(declaration, &span.source)?;
                        observation.reserve(128, &span.source)?;
                        let declaration = declaration.clone();
                        let target = rows
                            .get_mut(signature_index)
                            .and_then(|row| row.specialized.get_mut(index))
                            .ok_or_else(|| error(&member.span, "Missing owned erased signature"))?;
                        *target = Type::Classifier {
                            raw: !resolver
                                .formal_info_observed(&declaration, observation)?
                                .is_empty(),
                            declaration,
                            arguments: vec![],
                        };
                    }
                    ErasedSignatures::Shared(rows) => {
                        let raw = !resolver
                            .formal_info_observed(declaration, observation)?
                            .is_empty();
                        if !matches!(rows.get(signature_index).and_then(|row| row.specialized.get(index)), Some(Type::Classifier {
                                declaration: retained, arguments, raw: retained_raw
                            }) if retained == declaration && arguments.is_empty() && *retained_raw == raw)
                        {
                            return Err(error(
                                &member.span,
                                "Prepared erased signature differs from source",
                            ));
                        }
                    }
                }

                assembly.ty += 1;
            }
            Phase::MemberFinish => {
                if !retain_source_snapshot {
                    let original = assembly
                        .source_members
                        .as_ref()
                        .and_then(|s| s.get(assembly.member))
                        .ok_or_else(|| error(span, "Missing completed runtime member source"))?;
                    let member = assembly
                        .members
                        .get_mut(assembly.member)
                        .ok_or_else(|| error(span, "Missing completed runtime erased member"))?;
                    resolver.share_completed_erased_signatures(
                        original,
                        &mut member.signatures,
                        observation,
                    )?;
                }
                assembly.member += 1;
                self.phase = Phase::Member;
            }
            Phase::ParentsStart => {
                assembly.parent_inputs = Some(ParentInputs::Preparing(
                    DirectParentPreparation::begin(resolver),
                ));
                self.phase = Phase::ParentsPrepare;
            }
            Phase::ParentsPrepare => {
                let Some(ParentInputs::Preparing(preparation)) = assembly.parent_inputs.as_mut()
                else {
                    return Err(error(span, "Missing direct-parent preparation"));
                };
                if preparation.advance_with_source_decoding(
                    self.receiver.expression(),
                    span,
                    observation,
                    retain_source_decoding,
                )? {
                    let Some(ParentInputs::Preparing(preparation)) = assembly.parent_inputs.take()
                    else {
                        return Err(error(span, "Missing completed direct-parent preparation"));
                    };
                    assembly.parent_inputs =
                        Some(ParentInputs::Ready(preparation.finish(span)?.into_iter()));
                    self.phase = Phase::Parents;
                }
            }
            Phase::Parents => {
                if let Some((ty, location)) =
                    assembly
                        .parent_inputs
                        .as_mut()
                        .and_then(|inputs| match inputs {
                            ParentInputs::Ready(inputs) => inputs.next(),
                            ParentInputs::Preparing(_) | ParentInputs::Agreement(_) => None,
                        })
                {
                    observation.work(1, &span.source)?;
                    let projected = match resolver
                        .runtime_classifier_projection_expression_observed(
                            &ty,
                            &location,
                            observation,
                        )? {
                        Cow::Borrowed(_) => ty,
                        Cow::Owned(projected) => projected,
                    };
                    observation.reserve(32, &span.source)?;
                    assembly.parents.push(projected);
                } else {
                    self.phase = Phase::DomainsStart;
                }
            }
            Phase::DomainsStart => {
                let source_members = assembly
                    .source_members
                    .take()
                    .ok_or_else(|| error(span, "Missing runtime projection input"))?;
                let projection_input = match source_members {
                    SourceMembers::Owned(rows) => {
                        observation.serialized(&rows, &span.source)?;
                        let input = rows.clone();
                        assembly.source_snapshot = rows;
                        SourceMembers::Owned(input)
                    }
                    SourceMembers::Views(rows) => SourceMembers::Views(rows),
                };
                let projections = resolver.runtime_projections_from_sources(
                    &self.receiver,
                    projection_input,
                    span,
                    observation,
                )?;
                assembly.retained_receiver = match &projections {
                    SourceProjections::Views(rows) => {
                        rows.first().and_then(|row| match &row.receiver {
                            RetainedMemberValue::Shared(value) => {
                                Some(RetainedMemberValue::Shared(value.clone()))
                            }
                            RetainedMemberValue::Owned(_) => None,
                        })
                    }
                    SourceProjections::Owned(_) => None,
                };
                let domains =
                    resolver.runtime_domains_from_source_projections(projections, observation)?;
                assembly.domains = Some(match domains {
                    PreparedSourceDomains::Owned(rows) => DomainFilter::Owned {
                        pending: rows.into_iter(),
                        kept: vec![],
                    },
                    PreparedSourceDomains::Views(rows) => DomainFilter::Views {
                        pending: rows.into_iter(),
                        kept: vec![],
                    },
                });
                self.phase = Phase::Domains;
            }
            Phase::Domains => {
                let filter = assembly
                    .domains
                    .as_mut()
                    .ok_or_else(|| error(span, "Missing runtime domain filter"))?;
                let complete = match filter {
                    DomainFilter::Owned { pending, kept } => {
                        if let Some(domain) = pending.next() {
                            observation.work(1, &span.source)?;
                            if resolver.retain_runtime_domain(
                                RuntimeDomainRef::from(&domain),
                                span,
                                observation,
                            )? {
                                observation.reserve(32, &span.source)?;
                                kept.push(domain);
                            }
                            false
                        } else {
                            true
                        }
                    }
                    DomainFilter::Views { pending, kept } => {
                        if let Some(domain) = pending.next() {
                            observation.work(1, &span.source)?;
                            if resolver.retain_runtime_domain(domain.source(), span, observation)? {
                                observation.reserve(32, &span.source)?;
                                kept.push(domain);
                            }
                            false
                        } else {
                            true
                        }
                    }
                };
                if complete {
                    assembly.feature_domains = Some(
                        match assembly
                            .domains
                            .take()
                            .ok_or_else(|| error(span, "Missing completed runtime domains"))?
                        {
                            DomainFilter::Owned { kept, .. } => PreparedSourceDomains::Owned(kept),
                            DomainFilter::Views { kept, .. } => PreparedSourceDomains::Views(kept),
                        },
                    );
                    self.phase = Phase::Finish;
                }
            }
            Phase::Finish => {
                let receiver = match assembly.retained_receiver.take() {
                    Some(value) => value,
                    None => {
                        observation.preparation(&span.source, |context| {
                            self.receiver.retained_clone_observed(context).map(|_| ())
                        })?;
                        RetainedMemberValue::Owned(self.receiver.as_ref().clone())
                    }
                };
                observation.reserve(128, &span.source)?;
                assembly.retained_receiver = Some(receiver);
                self.phase = Phase::Complete;
            }
            Phase::Start | Phase::Bounds | Phase::Sources | Phase::Complete => {
                return Err(error(span, "Invalid runtime member phase"));
            }
        }
        Ok(self.phase == Phase::Complete)
    }
    pub(crate) fn finish_parts(
        mut self,
    ) -> Result<(MaterializationMembers<'a>, Cow<'input, RuntimeType>), ImportError> {
        if self.failed || self.phase != Phase::Complete {
            return Err(error(
                &fallback(&DeclarationId {
                    source: self.resolver.standard_id.clone(),
                    path: vec![],
                }),
                "Runtime member preparation is incomplete or failed",
            ));
        }
        let fallback = fallback(&DeclarationId {
            source: self.resolver.standard_id.clone(),
            path: vec![],
        });
        let MemberProgress::Assembly(mut assembly) = self.progress else {
            return Err(error(&fallback, "Missing completed member assembly"));
        };
        let output = MaterializationMembers {
            feature_domains: assembly
                .feature_domains
                .take()
                .ok_or_else(|| error(&fallback, "Missing completed runtime domains"))?,
            warnings: self.warnings,
            reference_bounds: assembly.reference_bounds,
            receiver: assembly
                .retained_receiver
                .take()
                .ok_or_else(|| error(&fallback, "Missing completed runtime receiver"))?,
            parents: assembly.parents,
            declaration_span: self
                .span
                .take()
                .ok_or_else(|| error(&fallback, "Missing completed runtime span"))?,
            bounds: self.bounds,
            source_members: assembly.source_snapshot,
            members: assembly.members,
        };
        Ok((output, self.receiver))
    }
    pub(crate) fn finish_owned(
        self,
        observation: &crate::definitions::ecore::ParserObservation<'_, '_>,
    ) -> Result<RuntimeMembers, ImportError> {
        self.finish_parts()?.0.into_owned_observed(observation)
    }
}
fn borrowed_member<'a>(
    resolver: &'a Resolver,
    member: MemberSourceRef<'_>,
    span: &SourceSpan,
    observation: &crate::definitions::ecore::ParserObservation<'_, '_>,
) -> Result<ErasedMember<'a>, ImportError> {
    let node = resolver.node_observed(member.declaration, observation)?;
    let mut name = None;
    for attribute in &node.attributes {
        observation.work(1, &span.source)?;
        if attribute.name.namespace.is_none() && attribute.name.local == "name" {
            name = Some(attribute.value.as_str());
            break;
        }
    }
    let name = name.ok_or_else(|| error(&node.span, "Missing member name"))?;
    let document = resolver
        .resources(member.declaration)
        .document(&member.declaration.source)
        .map_err(|e| error(&node.span, e))?;
    let source_payload = document
        .source
        .text
        .get(node.span.start..node.span.end)
        .ok_or_else(|| error(&node.span, "Invalid retained member source span"))?;
    // Only these fields are copied. Cow metadata and its lifetime
    // retain the exact admitted source rather than duplicating it.
    let (declaration, owner) = match resolver.shared_member_identity(member, observation)? {
        Some((declaration, owner)) => {
            observation.serialized(&member.formals, &span.source)?;
            (
                RetainedMemberValue::Shared(declaration),
                RetainedMemberValue::Shared(owner),
            )
        }
        None => {
            observation.serialized(
                &(&member.declaration, &member.owner, &member.formals),
                &span.source,
            )?;
            (
                RetainedMemberValue::Owned(member.declaration.clone()),
                RetainedMemberValue::Owned(member.owner.clone()),
            )
        }
    };
    let signatures = match resolver.shared_erased_signatures(member, observation)? {
        Some(rows) => ErasedSignatures::Shared(rows),
        None => {
            observation.serialized(&member.signatures, &span.source)?;
            ErasedSignatures::Owned(member.signatures.to_vec())
        }
    };
    observation.reserve(128 + 32, &span.source)?;
    Ok(ErasedMember {
        declaration,
        owner,
        name: std::borrow::Cow::Borrowed(name),
        span: std::borrow::Cow::Borrowed(&node.span),
        source_payload: std::borrow::Cow::Borrowed(source_payload),
        signatures,
        formals: member.formals.to_vec(),
    })
}

#[cfg(test)]
#[path = "generic_runtime_members_preparation_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "generic_direct_parent_consumption_tests.rs"]
mod direct_parent_consumption_tests;

#[cfg(test)]
#[path = "generic_legacy_runtime_consumption_tests.rs"]
mod legacy_consumption_tests;
