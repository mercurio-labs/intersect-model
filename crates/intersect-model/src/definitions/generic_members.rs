//! Receiver-specific member type projection; selection and invocation are separate.
use super::*;
use crate::definitions::generic_types::{
    ClosedType, ParameterBinding, RuntimeType, Substitution, Type,
};
#[path = "generic_feature_signature_inputs.rs"]
mod feature_signature_inputs;
pub(super) use feature_signature_inputs::CompiledFeatureSignatureInputs;
use feature_signature_inputs::SignaturePreparation;

// These refs expose every public DTO field. They carry source metadata, never
// an independently reusable membership or conformance proof.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct MemberSourceRef<'a> {
    pub declaration: &'a DeclarationId,
    pub owner: &'a Type,
    pub name: &'a str,
    pub span: &'a SourceSpan,
    pub source_payload: &'a str,
    pub signatures: &'a [SpecializedSignature],
    pub formals: &'a [SpecializedFormal],
}
impl<'a> From<&'a SpecializedMember> for MemberSourceRef<'a> {
    fn from(source: &'a SpecializedMember) -> Self {
        Self {
            declaration: &source.declaration,
            owner: &source.owner,
            name: &source.name,
            span: &source.span,
            source_payload: &source.source_payload,
            signatures: &source.signatures,
            formals: &source.formals,
        }
    }
}
impl MemberSourceRef<'_> {
    pub(crate) fn owned_clone(self) -> SpecializedMember {
        SpecializedMember {
            declaration: self.declaration.clone(),
            owner: self.owner.clone(),
            name: self.name.to_owned(),
            span: self.span.clone(),
            source_payload: self.source_payload.to_owned(),
            signatures: self.signatures.to_vec(),
            formals: self.formals.to_vec(),
        }
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct RuntimeProjectionRef<'a> {
    pub receiver: &'a RuntimeType,
    pub source: MemberSourceRef<'a>,
    pub storage_owner: &'a RuntimeType,
    pub bindings: &'a [ParameterBinding],
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct RuntimeDomainRef<'a> {
    pub projection: RuntimeProjectionRef<'a>,
    pub reference: bool,
    pub expression: &'a Type,
    pub erasure: &'a RuntimeType,
}
impl<'a> From<&'a RuntimeFeatureDomain> for RuntimeDomainRef<'a> {
    fn from(domain: &'a RuntimeFeatureDomain) -> Self {
        Self {
            projection: RuntimeProjectionRef {
                receiver: &domain.projection.receiver,
                source: (&domain.projection.source).into(),
                storage_owner: &domain.projection.storage_owner,
                bindings: &domain.projection.bindings,
            },
            reference: domain.reference,
            expression: &domain.expression,
            erasure: &domain.erasure,
        }
    }
}
impl RuntimeDomainRef<'_> {
    #[cfg(test)]
    pub(crate) fn owned_copy_observed(
        self,
        observation: &crate::definitions::ecore::ParserObservation<'_, '_>,
    ) -> Result<RuntimeFeatureDomain, ImportError> {
        observation.serialized(&self, &self.projection.source.span.source)?;
        Ok(RuntimeFeatureDomain {
            projection: RuntimeMemberProjection {
                receiver: self.projection.receiver.clone(),
                source: self.projection.source.owned_clone(),
                storage_owner: self.projection.storage_owner.clone(),
                bindings: self.projection.bindings.to_vec(),
            },
            reference: self.reference,
            expression: self.expression.clone(),
            erasure: self.erasure.clone(),
        })
    }
}
impl PartialEq<RuntimeFeatureDomain> for RuntimeDomainRef<'_> {
    fn eq(&self, other: &RuntimeFeatureDomain) -> bool {
        *self == RuntimeDomainRef::from(other)
    }
}
#[derive(Debug, Serialize)]
struct ProjectedSourceMember<'a> {
    declaration: RetainedMemberValue<DeclarationId>,
    owner: RetainedMemberValue<Type>,
    name: std::borrow::Cow<'a, str>,
    span: std::borrow::Cow<'a, SourceSpan>,
    source_payload: std::borrow::Cow<'a, str>,
    signatures: ErasedSignatures,
    formals: Vec<SpecializedFormal>,
}
impl ProjectedSourceMember<'_> {
    fn source(&self) -> MemberSourceRef<'_> {
        MemberSourceRef {
            declaration: self.declaration.value(),
            owner: self.owner.value(),
            name: &self.name,
            span: &self.span,
            source_payload: &self.source_payload,
            signatures: &self.signatures,
            formals: &self.formals,
        }
    }
    fn into_owned_observed(
        self,
        observation: &crate::definitions::ecore::ParserObservation<'_, '_>,
    ) -> Result<SpecializedMember, ImportError> {
        let source = &self.span.source;
        if self.declaration.copies_on_adoption() {
            observation.serialized(self.declaration.value(), source)?;
        }
        if self.owner.copies_on_adoption() {
            observation.serialized(self.owner.value(), source)?;
        }
        if matches!(&self.name, std::borrow::Cow::Borrowed(_)) {
            observation.string(self.name.len(), source)?;
        }
        if matches!(&self.span, std::borrow::Cow::Borrowed(_)) {
            observation.serialized(self.span.as_ref(), source)?;
        }
        if matches!(&self.source_payload, std::borrow::Cow::Borrowed(_)) {
            observation.string(self.source_payload.len(), source)?;
        }
        let signatures = self.signatures.into_owned_observed(source, observation)?;
        Ok(SpecializedMember {
            declaration: self.declaration.into_owned(),
            owner: self.owner.into_owned(),
            name: self.name.into_owned(),
            span: self.span.into_owned(),
            source_payload: self.source_payload.into_owned(),
            signatures,
            formals: self.formals,
        })
    }
}
enum SourceMembers<'a> {
    Owned(Vec<SpecializedMember>),
    Views(Vec<ProjectedSourceMember<'a>>),
}
impl SourceMembers<'_> {
    fn get(&self, index: usize) -> Option<MemberSourceRef<'_>> {
        match self {
            Self::Owned(rows) => rows.get(index).map(MemberSourceRef::from),
            Self::Views(rows) => rows.get(index).map(ProjectedSourceMember::source),
        }
    }
    fn len(&self) -> usize {
        match self {
            Self::Owned(rows) => rows.len(),
            Self::Views(rows) => rows.len(),
        }
    }
}
enum SourceMemberItem<'a> {
    Owned(SpecializedMember),
    View(ProjectedSourceMember<'a>),
}
impl SourceMemberItem<'_> {
    fn source(&self) -> MemberSourceRef<'_> {
        match self {
            Self::Owned(source) => source.into(),
            Self::View(source) => source.source(),
        }
    }
}
impl<'a> SourceMembers<'a> {
    fn into_items(self) -> impl Iterator<Item = SourceMemberItem<'a>> {
        let (owned, views) = match self {
            Self::Owned(rows) => (Some(rows), None),
            Self::Views(rows) => (None, Some(rows)),
        };
        owned
            .into_iter()
            .flatten()
            .map(SourceMemberItem::Owned)
            .chain(views.into_iter().flatten().map(SourceMemberItem::View))
    }
}
#[derive(Debug, Serialize)]
struct ProjectionSourceView<'a> {
    receiver: RetainedMemberValue<RuntimeType>,
    source: ProjectedSourceMember<'a>,
    storage_owner: RuntimeType,
    bindings: Vec<ParameterBinding>,
}
impl ProjectionSourceView<'_> {
    fn source(&self) -> RuntimeProjectionRef<'_> {
        RuntimeProjectionRef {
            receiver: self.receiver.value(),
            source: self.source.source(),
            storage_owner: &self.storage_owner,
            bindings: &self.bindings,
        }
    }
}
enum SourceProjections<'a> {
    Owned(Vec<RuntimeMemberProjection>),
    Views(Vec<ProjectionSourceView<'a>>),
}
#[derive(Debug, Serialize)]
pub(crate) struct FeatureDomainSourceView<'a> {
    projection: ProjectionSourceView<'a>,
    reference: bool,
    expression: RetainedMemberValue<Type>,
    erasure: RuntimeType,
}
impl FeatureDomainSourceView<'_> {
    fn into_shared_observed(
        self,
        inputs: &mut crate::definitions::schema::RuntimeDomainInputs,
        observation: &crate::definitions::ecore::ParserObservation<'_, '_>,
    ) -> Result<crate::definitions::schema::SharedRuntimeDomain, ImportError> {
        let source_id = &self.projection.source.span.source;
        let (source, expression) = observation.preparation(source_id, |context| {
            inputs
                .prepare_source_observed(self.source(), context)
                .map_err(crate::definitions::schema::PreparationError::Observation)
        })?;
        if self.projection.receiver.copies_on_adoption() {
            observation.reserve(32, source_id)?;
        }
        observation.reserve(128, source_id)?;
        Ok(crate::definitions::schema::SharedRuntimeDomain::from_parts(
            self.projection.receiver.into_owned(),
            source,
            self.projection.storage_owner,
            self.projection.bindings,
            self.reference,
            expression,
            self.erasure,
        ))
    }
    fn source(&self) -> RuntimeDomainRef<'_> {
        RuntimeDomainRef {
            projection: self.projection.source(),
            reference: self.reference,
            expression: self.expression.value(),
            erasure: &self.erasure,
        }
    }
    fn into_owned_observed(
        self,
        observation: &crate::definitions::ecore::ParserObservation<'_, '_>,
    ) -> Result<RuntimeFeatureDomain, ImportError> {
        let source = &self.projection.source.span.source;
        if self.projection.receiver.copies_on_adoption() {
            observation.preparation(source, |context| {
                self.projection
                    .receiver
                    .value()
                    .retained_clone_observed(context)
                    .map(|_| ())
            })?;
        }
        if self.expression.copies_on_adoption() {
            observation.serialized(self.expression.value(), source)?;
        }
        let member = self.projection.source.into_owned_observed(observation)?;
        Ok(RuntimeFeatureDomain {
            projection: RuntimeMemberProjection {
                receiver: self.projection.receiver.into_owned(),
                source: member,
                storage_owner: self.projection.storage_owner,
                bindings: self.projection.bindings,
            },
            reference: self.reference,
            expression: self.expression.into_owned(),
            erasure: self.erasure,
        })
    }
}
#[derive(Debug)]
pub(crate) enum PreparedSourceDomains<'a> {
    Owned(Vec<RuntimeFeatureDomain>),
    Views(Vec<FeatureDomainSourceView<'a>>),
}
impl Serialize for PreparedSourceDomains<'_> {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            Self::Owned(rows) => rows.serialize(serializer),
            Self::Views(rows) => rows.serialize(serializer),
        }
    }
}
impl PartialEq<Vec<RuntimeFeatureDomain>> for PreparedSourceDomains<'_> {
    fn eq(&self, other: &Vec<RuntimeFeatureDomain>) -> bool {
        self.len() == other.len() && self.iter().zip(other).all(|(a, b)| a == *b)
    }
}
impl PreparedSourceDomains<'_> {
    pub(crate) fn len(&self) -> usize {
        match self {
            Self::Owned(rows) => rows.len(),
            Self::Views(rows) => rows.len(),
        }
    }
    #[cfg(test)]
    pub(crate) fn is_empty(&self) -> bool {
        self.len() == 0
    }
    pub(crate) fn get(&self, index: usize) -> Option<RuntimeDomainRef<'_>> {
        match self {
            Self::Owned(rows) => rows.get(index).map(RuntimeDomainRef::from),
            Self::Views(rows) => rows.get(index).map(FeatureDomainSourceView::source),
        }
    }
    pub(crate) fn iter(&self) -> impl Iterator<Item = RuntimeDomainRef<'_>> {
        let owned = match self {
            Self::Owned(rows) => Some(rows.as_slice()),
            _ => None,
        };
        let views = match self {
            Self::Views(rows) => Some(rows.as_slice()),
            _ => None,
        };
        owned
            .into_iter()
            .flatten()
            .map(RuntimeDomainRef::from)
            .chain(
                views
                    .into_iter()
                    .flatten()
                    .map(FeatureDomainSourceView::source),
            )
    }
    pub(crate) fn into_shared_observed(
        self,
        inputs: &mut crate::definitions::schema::RuntimeDomainInputs,
        observation: &crate::definitions::ecore::ParserObservation<'_, '_>,
    ) -> Result<Vec<crate::definitions::schema::SharedRuntimeDomain>, ImportError> {
        let mut domains = Vec::new();
        match self {
            Self::Owned(rows) => {
                for source in rows {
                    let source_id = &source.projection.source.span.source;
                    observation.work(1, source_id)?;
                    observation.reserve(32, source_id)?;
                    let domain = observation.preparation(source_id, |context| {
                        inputs
                            .retain_observed((&source).into(), context)
                            .map_err(crate::definitions::schema::PreparationError::Observation)
                    })?;
                    domains.push(domain);
                }
            }
            Self::Views(rows) => {
                for source in rows {
                    let source_id = &source.projection.source.span.source;
                    observation.work(1, source_id)?;
                    observation.reserve(32, source_id)?;
                    domains.push(source.into_shared_observed(inputs, observation)?);
                }
            }
        }
        Ok(domains)
    }
    pub(crate) fn into_owned_observed(
        self,
        observation: &crate::definitions::ecore::ParserObservation<'_, '_>,
    ) -> Result<Vec<RuntimeFeatureDomain>, ImportError> {
        match self {
            Self::Owned(rows) => Ok(rows),
            Self::Views(rows) => {
                if let Some(first) = rows.first() {
                    observation
                        .reserve(128 + rows.len() * 32, &first.projection.source.span.source)?;
                }
                rows.into_iter()
                    .map(|row| row.into_owned_observed(observation))
                    .collect()
            }
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SpecializedSignature {
    pub owner: DeclarationId,
    pub feature: String,
    pub declared: Vec<Type>,
    pub specialized: Vec<Type>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SpecializedFormal {
    pub declaration: TypeParameter,
    pub bounds: Vec<Type>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SpecializedMember {
    pub declaration: DeclarationId,
    pub owner: Type,
    pub name: String,
    pub span: SourceSpan,
    /// Multiplicity, flags, annotations and defaults remain available for schema lowering.
    pub source_payload: String,
    pub signatures: Vec<SpecializedSignature>,
    /// Operation-local parameters retain their exact identity and remain open.
    pub formals: Vec<SpecializedFormal>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MemberTypes {
    pub receiver_bounds: ApplicationBounds,
    /// Includes inherited declarations; does not select overrides or hide collisions.
    pub members: Vec<SpecializedMember>,
}
/// A source-rechecked request, not a compiled runtime schema or selection proof.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RequestedMembers {
    pub parents: Vec<Type>,
    pub receiver: ClosedType,
    pub declaration_span: SourceSpan,
    pub bounds: Vec<BoundCheck>,
    pub members: Vec<SpecializedMember>,
}
// Complete signature metadata may be shared only inside checked immutable
// preparation. Public DTOs continue to own every declared/specialized field.
#[derive(Debug)]
pub(crate) enum ErasedSignatures {
    Owned(Vec<SpecializedSignature>),
    Shared(std::rc::Rc<Vec<SpecializedSignature>>),
}
impl std::ops::Deref for ErasedSignatures {
    type Target = [SpecializedSignature];
    fn deref(&self) -> &Self::Target {
        match self {
            Self::Owned(rows) => rows,
            Self::Shared(rows) => rows,
        }
    }
}
impl Serialize for ErasedSignatures {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.deref_rows().serialize(serializer)
    }
}
impl ErasedSignatures {
    fn deref_rows(&self) -> &[SpecializedSignature] {
        self
    }
    fn into_owned(self) -> Vec<SpecializedSignature> {
        match self {
            Self::Owned(rows) => rows,
            Self::Shared(rows) => match std::rc::Rc::try_unwrap(rows) {
                Ok(rows) => rows,
                Err(rows) => rows.as_ref().clone(),
            },
        }
    }
    fn into_owned_observed(
        self,
        source: &crate::definitions::bundle::SourceId,
        observation: &crate::definitions::ecore::ParserObservation<'_, '_>,
    ) -> Result<Vec<SpecializedSignature>, ImportError> {
        if matches!(&self, Self::Shared(rows) if std::rc::Rc::strong_count(rows) > 1) {
            observation.serialized(self.deref_rows(), source)?;
        }
        Ok(self.into_owned())
    }
    pub(crate) fn owned_copy_observed(
        &self,
        source: &crate::definitions::bundle::SourceId,
        observation: &crate::definitions::ecore::ParserObservation<'_, '_>,
    ) -> Result<Vec<SpecializedSignature>, ImportError> {
        observation.serialized(self.deref_rows(), source)?;
        Ok(self.to_vec())
    }
}
// Full immutable values retain their existing serialization and public ownership.
#[derive(Debug)]
pub(crate) enum RetainedMemberValue<T> {
    Owned(T),
    Shared(std::rc::Rc<T>),
}
impl<T> std::ops::Deref for RetainedMemberValue<T> {
    type Target = T;
    fn deref(&self) -> &T {
        match self {
            Self::Owned(value) => value,
            Self::Shared(value) => value,
        }
    }
}
impl<T: Serialize> Serialize for RetainedMemberValue<T> {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.value().serialize(serializer)
    }
}
impl<T: PartialEq> PartialEq<T> for RetainedMemberValue<T> {
    fn eq(&self, value: &T) -> bool {
        self.value() == value
    }
}
impl<T: Clone> RetainedMemberValue<T> {
    fn into_owned(self) -> T {
        match self {
            Self::Owned(value) => value,
            Self::Shared(value) => match std::rc::Rc::try_unwrap(value) {
                Ok(value) => value,
                Err(value) => value.as_ref().clone(),
            },
        }
    }
}
impl<T> RetainedMemberValue<T> {
    pub(crate) fn value(&self) -> &T {
        self
    }
    fn copies_on_adoption(&self) -> bool {
        matches!(self, Self::Shared(value) if std::rc::Rc::strong_count(value) > 1)
    }
}
type SharedMemberIdentity = (std::rc::Rc<DeclarationId>, std::rc::Rc<Type>);
#[derive(Debug, Default)]
pub(super) struct ErasedSignatureCache {
    owners: std::collections::BTreeMap<
        Type,
        std::collections::BTreeMap<DeclarationId, std::rc::Rc<Vec<SpecializedSignature>>>,
    >,
    entries: usize,
    identities: std::collections::BTreeMap<
        Type,
        std::collections::BTreeMap<DeclarationId, SharedMemberIdentity>,
    >,
    identity_entries: usize,
    bound_inputs: std::collections::BTreeMap<Type, std::rc::Rc<Type>>,
}

/// Private erasure view. Source metadata stays borrowed from the exact immutable
/// resolver; owned expressions and public inspection copies remain explicit.
#[derive(Debug, Serialize)]
pub(crate) struct ErasedMember<'a> {
    pub declaration: RetainedMemberValue<DeclarationId>,
    pub owner: RetainedMemberValue<Type>,
    pub name: std::borrow::Cow<'a, str>,
    pub span: std::borrow::Cow<'a, SourceSpan>,
    pub source_payload: std::borrow::Cow<'a, str>,
    pub signatures: ErasedSignatures,
    pub formals: Vec<SpecializedFormal>,
}
impl<'a> From<SpecializedMember> for ErasedMember<'a> {
    fn from(member: SpecializedMember) -> Self {
        Self {
            declaration: RetainedMemberValue::Owned(member.declaration),
            owner: RetainedMemberValue::Owned(member.owner),
            name: member.name.into(),
            span: std::borrow::Cow::Owned(member.span),
            source_payload: member.source_payload.into(),
            signatures: ErasedSignatures::Owned(member.signatures),
            formals: member.formals,
        }
    }
}
impl ErasedMember<'_> {
    fn into_owned(self) -> SpecializedMember {
        SpecializedMember {
            declaration: self.declaration.into_owned(),
            owner: self.owner.into_owned(),
            name: self.name.into_owned(),
            span: self.span.into_owned(),
            source_payload: self.source_payload.into_owned(),
            signatures: self.signatures.into_owned(),
            formals: self.formals,
        }
    }
    pub(crate) fn owned_copy_observed(
        &self,
        observation: &crate::definitions::ecore::ParserObservation<'_, '_>,
    ) -> Result<SpecializedMember, ImportError> {
        observation.serialized(self, &self.span.source)?;
        Ok(SpecializedMember {
            declaration: self.declaration.value().clone(),
            owner: self.owner.value().clone(),
            name: self.name.to_string(),
            span: self.span.as_ref().clone(),
            source_payload: self.source_payload.to_string(),
            signatures: self.signatures.to_vec(),
            formals: self.formals.clone(),
        })
    }
    pub(crate) fn requires_runtime_template_observed(
        &self,
        observation: &crate::definitions::conversion::InvocationContext<'_>,
    ) -> Result<bool, crate::definitions::schema::PreparationError> {
        requires_runtime_template(&self.formals, &self.signatures, observation)
    }
    pub(crate) fn requires_schema_template_observed(
        &self,
        observation: &crate::definitions::conversion::InvocationContext<'_>,
    ) -> Result<bool, crate::definitions::schema::PreparationError> {
        requires_schema_template(
            &self.declaration,
            &self.formals,
            &self.signatures,
            observation,
        )
    }
}
impl PartialEq<SpecializedMember> for ErasedMember<'_> {
    fn eq(&self, member: &SpecializedMember) -> bool {
        self.declaration == member.declaration
            && self.owner == member.owner
            && self.name.as_ref() == member.name
            && self.span.as_ref() == &member.span
            && self.source_payload.as_ref() == member.source_payload
            && self.signatures.deref_rows() == member.signatures.as_slice()
            && self.formals == member.formals
    }
}
#[derive(Debug, Serialize)]
pub(crate) struct MaterializationMembers<'a> {
    pub warnings: Vec<Warning>,
    pub reference_bounds: Vec<RetainedReferenceBounds>,
    pub feature_domains: PreparedSourceDomains<'a>,
    pub receiver: RetainedMemberValue<RuntimeType>,
    pub parents: Vec<Type>,
    pub declaration_span: std::borrow::Cow<'a, SourceSpan>,
    pub bounds: Vec<BoundCheck>,
    pub source_members: Vec<SpecializedMember>,
    pub members: Vec<ErasedMember<'a>>,
}
impl MaterializationMembers<'_> {
    fn into_owned_observed(
        self,
        observation: &crate::definitions::ecore::ParserObservation<'_, '_>,
    ) -> Result<RuntimeMembers, ImportError> {
        observation.reserve(128 + self.members.len() * 32, &self.declaration_span.source)?;
        for member in &self.members {
            observation.work(1, &self.declaration_span.source)?;
            if member.declaration.copies_on_adoption() {
                observation
                    .serialized(member.declaration.value(), &self.declaration_span.source)?;
            }
            if member.owner.copies_on_adoption() {
                observation.serialized(member.owner.value(), &self.declaration_span.source)?;
            }
            if matches!(&member.signatures, ErasedSignatures::Shared(rows) if std::rc::Rc::strong_count(rows) > 1)
            {
                observation.serialized(
                    member.signatures.deref_rows(),
                    &self.declaration_span.source,
                )?;
            }
            if matches!(&member.name, std::borrow::Cow::Borrowed(_)) {
                observation.string(member.name.len(), &self.declaration_span.source)?;
            }
            if matches!(&member.source_payload, std::borrow::Cow::Borrowed(_)) {
                observation.string(member.source_payload.len(), &self.declaration_span.source)?;
            }
            if matches!(&member.span, std::borrow::Cow::Borrowed(_)) {
                observation.serialized(member.span.as_ref(), &self.declaration_span.source)?;
            }
        }
        // The public adapter constructs owned bound/container DTOs. Admit those
        // result records before conversion, independently of shared leaf copies.
        if !self.reference_bounds.is_empty() {
            observation.reserve(
                128 + self.reference_bounds.len() * 32,
                &self.declaration_span.source,
            )?;
        }
        for bound in &self.reference_bounds {
            if !bound.bounds.is_empty() {
                observation
                    .reserve(128 + bound.bounds.len() * 32, &self.declaration_span.source)?;
            }
            if bound.feature.copies_on_adoption() {
                observation.work(1, &self.declaration_span.source)?;
                observation.serialized(bound.feature.value(), &self.declaration_span.source)?;
            }
            for value in &bound.bounds {
                if value.copies_on_adoption() {
                    observation.work(1, &self.declaration_span.source)?;
                    observation.serialized(value.value(), &self.declaration_span.source)?;
                }
            }
        }
        if matches!(&self.declaration_span, std::borrow::Cow::Borrowed(_)) {
            observation.serialized(
                self.declaration_span.as_ref(),
                &self.declaration_span.source,
            )?;
        }
        if self.receiver.copies_on_adoption() {
            observation.preparation(&self.declaration_span.source, |context| {
                self.receiver
                    .value()
                    .retained_clone_observed(context)
                    .map(|_| ())
            })?;
        }
        Ok(RuntimeMembers {
            warnings: self.warnings,
            reference_bounds: self
                .reference_bounds
                .into_iter()
                .map(|bound| RuntimeReferenceBounds {
                    feature: bound.feature.into_owned(),
                    bounds: bound
                        .bounds
                        .into_iter()
                        .map(RetainedMemberValue::into_owned)
                        .collect(),
                })
                .collect(),
            feature_domains: self.feature_domains.into_owned_observed(observation)?,
            receiver: self.receiver.into_owned(),
            parents: self.parents,
            declaration_span: self.declaration_span.into_owned(),
            bounds: self.bounds,
            source_members: self.source_members,
            members: self
                .members
                .into_iter()
                .map(ErasedMember::into_owned)
                .collect(),
        })
    }
}
impl PartialEq<RuntimeMembers> for MaterializationMembers<'_> {
    fn eq(&self, report: &RuntimeMembers) -> bool {
        self.warnings == report.warnings
            && self.reference_bounds == report.reference_bounds
            && self.feature_domains == report.feature_domains
            && self.receiver == report.receiver
            && self.parents == report.parents
            && self.declaration_span.as_ref() == &report.declaration_span
            && self.bounds == report.bounds
            && self.source_members == report.source_members
            && self.members.len() == report.members.len()
            && self
                .members
                .iter()
                .zip(&report.members)
                .all(|(a, b)| a == b)
    }
}

/// Runtime projection retains both the source expressions and the structural
/// classifier erasures used by the model. Raw warnings are not bound proofs.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuntimeMembers {
    pub warnings: Vec<Warning>,
    pub reference_bounds: Vec<RuntimeReferenceBounds>,
    pub feature_domains: Vec<RuntimeFeatureDomain>,
    pub receiver: RuntimeType,
    pub parents: Vec<Type>,
    pub declaration_span: SourceSpan,
    pub bounds: Vec<BoundCheck>,
    pub source_members: Vec<SpecializedMember>,
    pub members: Vec<SpecializedMember>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuntimeReferenceBounds {
    pub feature: DeclarationId,
    pub bounds: Vec<Type>,
}
// Complete immutable declaration metadata is retained by identity; bound expressions
// still come from fresh receiver checks, never from a cached conformance result.
#[derive(Debug, Serialize)]
pub(crate) struct RetainedReferenceBounds {
    pub feature: RetainedMemberValue<DeclarationId>,
    pub bounds: Vec<RetainedMemberValue<Type>>,
}
impl PartialEq<RuntimeReferenceBounds> for RetainedReferenceBounds {
    fn eq(&self, other: &RuntimeReferenceBounds) -> bool {
        self.feature == other.feature && self.bounds == other.bounds
    }
}
/// Source member and structural owner remain distinct for partially open inheritance.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuntimeMemberProjection {
    pub receiver: RuntimeType,
    pub source: SpecializedMember,
    pub storage_owner: RuntimeType,
    /// Only bindings established by the receiver/owner, never captured from a value.
    pub bindings: Vec<ParameterBinding>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuntimeFeatureDomain {
    pub projection: RuntimeMemberProjection,
    pub reference: bool,
    pub expression: Type,
    pub erasure: RuntimeType,
}

impl SpecializedMember {
    /// Schema expression metadata also retains pinned EObject reflection signatures.
    /// Their targets need not be constructible classes or installed value codecs.
    #[cfg(test)]
    pub(crate) fn requires_schema_template(&self) -> Result<bool, String> {
        self.requires_schema_template_observed(
            &crate::definitions::conversion::InvocationContext::default(),
        )
        .map_err(|error| error.to_string())
    }
    pub(crate) fn requires_schema_template_observed(
        &self,
        observation: &crate::definitions::conversion::InvocationContext<'_>,
    ) -> Result<bool, crate::definitions::schema::PreparationError> {
        requires_schema_template(
            &self.declaration,
            &self.formals,
            &self.signatures,
            observation,
        )
    }

    #[cfg(test)]
    pub(crate) fn requires_runtime_template(&self) -> bool {
        self.requires_runtime_template_observed(
            &crate::definitions::conversion::InvocationContext::default(),
        )
        .unwrap_or(true)
    }
    #[cfg(test)]
    pub(crate) fn requires_runtime_template_observed(
        &self,
        observation: &crate::definitions::conversion::InvocationContext<'_>,
    ) -> Result<bool, crate::definitions::schema::PreparationError> {
        requires_runtime_template(&self.formals, &self.signatures, observation)
    }
}

fn requires_schema_template(
    declaration: &DeclarationId,
    formals: &[SpecializedFormal],
    signatures: &[SpecializedSignature],
    observation: &crate::definitions::conversion::InvocationContext<'_>,
) -> Result<bool, crate::definitions::schema::PreparationError> {
    if requires_runtime_template(formals, signatures, observation)? {
        return Ok(true);
    }
    observation
        .work(1)
        .map_err(crate::definitions::schema::PreparationError::Observation)?;
    let owner = crate::definitions::schema::eobject_reference_top_declaration_ref()?;
    Ok(declaration.source == owner.source
        && declaration.path[..declaration.path.len().saturating_sub(1)] == owner.path)
}
fn requires_runtime_template(
    formals: &[SpecializedFormal],
    signatures: &[SpecializedSignature],
    observation: &crate::definitions::conversion::InvocationContext<'_>,
) -> Result<bool, crate::definitions::schema::PreparationError> {
    use crate::definitions::schema::PreparationError;
    observation.work(1).map_err(PreparationError::Observation)?;
    if !formals.is_empty() {
        return Ok(true);
    }
    for signature in signatures {
        observation.work(1).map_err(PreparationError::Observation)?;
        for expression in &signature.specialized {
            observation.work(1).map_err(PreparationError::Observation)?;
            match RuntimeType::validate_expression_observed(expression, observation) {
                Ok(()) => {}
                Err(PreparationError::Schema(_)) => return Ok(true),
                Err(error @ PreparationError::Observation(_)) => return Err(error),
            }
        }
    }
    Ok(false)
}

/// Explicit operation application. This checks signatures, not invocation or overload inference.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AppliedOperation {
    pub receiver: ClosedType,
    pub arguments: Vec<ClosedType>,
    pub receiver_bounds: Vec<BoundCheck>,
    pub argument_bounds: Vec<BoundCheck>,
    pub signature_bounds: Vec<BoundCheck>,
    pub operation: SpecializedMember,
}
/// Explicit runtime method application; residual class domains are not invocation proof.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuntimeAppliedOperation {
    pub projection: RuntimeMemberProjection,
    pub arguments: Vec<RuntimeType>,
    pub receiver_bounds: Vec<BoundCheck>,
    pub argument_bounds: Vec<BoundCheck>,
    pub signature_bounds: Vec<BoundCheck>,
    pub warnings: Vec<Warning>,
    pub operation: SpecializedMember,
}

impl Resolver {
    /// Resolve a provider's source declaration before choosing concrete generic
    /// applications. Runtime value domains remain a materialization obligation.
    #[cfg(test)]
    pub(crate) fn declared_feature(
        &self,
        receiver: &Type,
        name: &str,
        span: &SourceSpan,
    ) -> Result<DeclarationId, ImportError> {
        let invocation = crate::definitions::conversion::InvocationContext::default();
        let observation = crate::definitions::ecore::ParserObservation::new(&invocation);
        let mut preparation = DeclaredFeaturePreparation::begin(self, receiver, name, span);
        while !preparation.advance(&observation)? {}
        preparation.finish()
    }
    pub fn instantiate_runtime_operation_in_owner(
        &self,
        receiver: &RuntimeType,
        operation: &DeclarationId,
        owner: Option<&Type>,
        arguments: &[RuntimeType],
    ) -> Result<RuntimeAppliedOperation, ImportError> {
        let operation_span = self.node(operation)?.span.clone();
        let projections = self.runtime_member_projections(receiver)?;
        let mut selected = projections.into_iter().filter(|projection| {
            &projection.source.declaration == operation
                && owner.is_none_or(|owner| owner == &projection.source.owner)
        });
        let projection = selected.next().ok_or_else(|| {
            error(
                &operation_span,
                "Operation is not a member of the requested runtime receiver/owner projection",
            )
        })?;
        if selected.next().is_some() || !named(self.node(operation)?, "eOperations") {
            return Err(error(
                &projection.source.span,
                "Expected one exact runtime operation owner projection",
            ));
        }
        let (receiver_bounds, mut warnings) =
            self.runtime_application_checks(receiver.expression(), &projection.source.span)?;
        let (environment, argument_bounds, argument_warnings) =
            self.runtime_operation_bindings(&projection, arguments)?;
        warnings.extend(argument_warnings);
        let mut operation = projection.source.clone();
        let mut signature_bounds = Vec::new();
        for signature in &mut operation.signatures {
            for ty in &mut signature.specialized {
                *ty = environment
                    .apply_simultaneous(ty)
                    .map_err(|e| error(&operation.span, e))?;
                let (checks, signature_warnings) =
                    self.runtime_application_checks(ty, &operation.span)?;
                if checks.iter().any(|check| !check.satisfied) {
                    return Err(error(
                        &operation.span,
                        "Specialized runtime operation signature violates generic bounds",
                    ));
                }
                signature_bounds.extend(checks);
                warnings.extend(signature_warnings);
            }
        }
        for formal in &mut operation.formals {
            for bound in &mut formal.bounds {
                *bound = environment
                    .apply_simultaneous(bound)
                    .map_err(|e| error(&operation.span, e))?;
            }
        }
        Ok(RuntimeAppliedOperation {
            projection,
            arguments: arguments.to_vec(),
            receiver_bounds,
            argument_bounds,
            signature_bounds,
            warnings,
            operation,
        })
    }

    /// Source-checked projections independent of the erased structural lowerer.
    pub fn runtime_member_projections(
        &self,
        receiver: &RuntimeType,
    ) -> Result<Vec<RuntimeMemberProjection>, ImportError> {
        self.runtime_member_projections_observed(
            receiver,
            &crate::definitions::ecore::ParserObservation::new(
                &crate::definitions::conversion::InvocationContext::default(),
            ),
        )
    }
    pub(crate) fn runtime_member_projections_observed(
        &self,
        receiver: &RuntimeType,
        observation: &crate::definitions::ecore::ParserObservation<'_, '_>,
    ) -> Result<Vec<RuntimeMemberProjection>, ImportError> {
        let Type::Classifier { declaration, .. } = receiver.expression() else {
            return Err(error(
                &SourceSpan {
                    source: self.standard_id.clone(),
                    start: 0,
                    end: 0,
                },
                "Runtime receiver requires a classifier",
            ));
        };
        let span = &self.node_observed(declaration, observation)?.span;
        if self.classifier_observed(declaration, observation)? != TargetKind::Class {
            return Err(error(span, "Runtime receiver requires an EClass"));
        }
        let (bounds, _) =
            self.runtime_application_checks_observed(receiver.expression(), span, observation)?;
        for check in &bounds {
            observation.work(1, &span.source)?;
            if !check.satisfied {
                return Err(error(span, "Runtime receiver violates generic bounds"));
            }
        }
        let members =
            self.project_member_types_observed(receiver.expression(), span, true, observation)?;
        self.runtime_projections_from_members(receiver, members, span, observation)
    }
    // The caller must have checked this exact receiver and projected these
    // members with this resolver. Reuse the immutable source projection rather
    // than resolving the same ancestry and signatures a second time.
    fn runtime_projections_from_members(
        &self,
        receiver: &RuntimeType,
        members: Vec<SpecializedMember>,
        span: &SourceSpan,
        observation: &crate::definitions::ecore::ParserObservation<'_, '_>,
    ) -> Result<Vec<RuntimeMemberProjection>, ImportError> {
        match self.runtime_projections_from_sources(
            receiver,
            SourceMembers::Owned(members),
            span,
            observation,
        )? {
            SourceProjections::Owned(rows) => Ok(rows),
            SourceProjections::Views(_) => Err(error(span, "Expected owned runtime projections")),
        }
    }
    fn runtime_projections_from_sources<'a>(
        &self,
        receiver: &RuntimeType,
        members: SourceMembers<'a>,
        span: &SourceSpan,
        observation: &crate::definitions::ecore::ParserObservation<'_, '_>,
    ) -> Result<SourceProjections<'a>, ImportError> {
        let views = matches!(&members, SourceMembers::Views(_));
        let shared_receiver = if views {
            observation.preparation(&span.source, |context| {
                receiver.retained_clone_observed(context).map(|_| ())
            })?;
            observation.reserve(128 + 32, &span.source)?;
            Some(std::rc::Rc::new(receiver.clone()))
        } else {
            None
        };
        let mut projections = if views {
            SourceProjections::Views(Vec::new())
        } else {
            SourceProjections::Owned(Vec::new())
        };
        for source in members.into_items() {
            observation.work(1, &span.source)?;
            let metadata = source.source();
            let owner = self.runtime_classifier_projection_expression_observed(
                metadata.owner,
                metadata.span,
                observation,
            )?;
            let storage_owner = self.runtime_input_observed(owner, metadata.span, observation)?;
            let bindings = self.runtime_projection_bindings_observed(
                receiver.expression(),
                metadata.owner,
                metadata.span,
                observation,
            )?;
            match (&mut projections, source) {
                (SourceProjections::Owned(rows), SourceMemberItem::Owned(source)) => {
                    observation.preparation(&span.source, |context| {
                        receiver.retained_clone_observed(context).map(|_| ())
                    })?;
                    observation.reserve(128 + 32, &span.source)?;
                    rows.push(RuntimeMemberProjection {
                        receiver: receiver.clone(),
                        storage_owner,
                        source,
                        bindings,
                    });
                }
                (SourceProjections::Views(rows), SourceMemberItem::View(source)) => {
                    observation.reserve(128 + 32, &span.source)?;
                    let receiver = shared_receiver
                        .as_ref()
                        .ok_or_else(|| error(span, "Missing retained projection receiver"))?
                        .clone();
                    rows.push(ProjectionSourceView {
                        receiver: RetainedMemberValue::Shared(receiver),
                        source,
                        storage_owner,
                        bindings,
                    });
                }
                _ => return Err(error(span, "Runtime projection source policy mismatch")),
            }
        }
        Ok(projections)
    }
    pub(crate) fn runtime_projection_bindings_observed(
        &self,
        receiver: &Type,
        owner: &Type,
        span: &SourceSpan,
        observation: &crate::definitions::ecore::ParserObservation<'_, '_>,
    ) -> Result<Vec<ParameterBinding>, ImportError> {
        let mut bindings = std::collections::BTreeMap::new();
        for ty in [receiver, owner] {
            observation.work(1, &span.source)?;
            if let Type::Classifier {
                declaration,
                arguments,
                ..
            } = ty
            {
                for (formal, argument) in self
                    .formal_info_observed(declaration, observation)?
                    .iter()
                    .zip(arguments)
                {
                    observation.work(1, &span.source)?;
                    if let Some(previous) = bindings.get(&formal.declaration) {
                        if previous != argument {
                            return Err(error(span, "Conflicting receiver/owner bindings"));
                        }
                    } else {
                        observation.serialized(&formal.declaration, &span.source)?;
                        observation.serialized(argument, &span.source)?;
                        observation.reserve(128, &span.source)?;
                        bindings.insert(formal.declaration.clone(), argument.clone());
                    }
                }
            }
        }
        let mut retained_bindings = Vec::new();
        for (parameter, argument) in bindings {
            observation.work(1, &span.source)?;
            observation.reserve(128 + 32, &span.source)?;
            retained_bindings.push(ParameterBinding {
                parameter,
                argument,
            });
        }
        Ok(retained_bindings)
    }

    pub fn runtime_feature_domains(
        &self,
        receiver: &RuntimeType,
    ) -> Result<Vec<RuntimeFeatureDomain>, ImportError> {
        self.runtime_feature_domains_observed(
            receiver,
            &crate::definitions::ecore::ParserObservation::new(
                &crate::definitions::conversion::InvocationContext::default(),
            ),
        )
    }
    pub(crate) fn runtime_feature_domains_observed(
        &self,
        receiver: &RuntimeType,
        observation: &crate::definitions::ecore::ParserObservation<'_, '_>,
    ) -> Result<Vec<RuntimeFeatureDomain>, ImportError> {
        let projections = self.runtime_member_projections_observed(receiver, observation)?;
        self.runtime_domains_from_projections(projections, observation)
    }
    fn runtime_domains_from_projections(
        &self,
        projections: Vec<RuntimeMemberProjection>,
        observation: &crate::definitions::ecore::ParserObservation<'_, '_>,
    ) -> Result<Vec<RuntimeFeatureDomain>, ImportError> {
        self.runtime_domains_from_source_projections(
            SourceProjections::Owned(projections),
            observation,
        )?
        .into_owned_observed(observation)
    }
    fn runtime_domains_from_source_projections<'a>(
        &self,
        projections: SourceProjections<'a>,
        observation: &crate::definitions::ecore::ParserObservation<'_, '_>,
    ) -> Result<PreparedSourceDomains<'a>, ImportError> {
        enum ProjectionItem<'a> {
            Owned(RuntimeMemberProjection),
            View(ProjectionSourceView<'a>),
        }
        impl ProjectionItem<'_> {
            fn source(&self) -> RuntimeProjectionRef<'_> {
                match self {
                    Self::Owned(projection) => RuntimeProjectionRef {
                        receiver: &projection.receiver,
                        source: (&projection.source).into(),
                        storage_owner: &projection.storage_owner,
                        bindings: &projection.bindings,
                    },
                    Self::View(projection) => projection.source(),
                }
            }
        }
        let views = matches!(&projections, SourceProjections::Views(_));
        let (owned, retained) = match projections {
            SourceProjections::Owned(rows) => (Some(rows), None),
            SourceProjections::Views(rows) => (None, Some(rows)),
        };
        let mut domains = if views {
            PreparedSourceDomains::Views(Vec::new())
        } else {
            PreparedSourceDomains::Owned(Vec::new())
        };
        let items = owned
            .into_iter()
            .flatten()
            .map(ProjectionItem::Owned)
            .chain(retained.into_iter().flatten().map(ProjectionItem::View));
        for projection in items {
            let source = projection.source().source;
            observation.work(1, &source.span.source)?;
            let node = self.node_observed(source.declaration, observation)?;
            if !named(node, "eStructuralFeatures") {
                continue;
            }
            let mut selected = None;
            for signature in source.signatures {
                observation.work(1, &source.span.source)?;
                if &signature.owner == source.declaration && signature.feature == "eType" {
                    selected = Some(signature);
                    break;
                }
            }
            let signature =
                selected.ok_or_else(|| error(source.span, "Missing feature type signature"))?;
            if signature.specialized.len() != 1 {
                return Err(error(source.span, "Feature requires one domain expression"));
            }
            let input = &signature.specialized[0];
            let shared = if views {
                self.shared_bound_input(input, source.span, observation)?
            } else {
                None
            };
            let expression = match shared {
                Some(value) => RetainedMemberValue::Shared(value),
                None => {
                    observation.serialized(input, &source.span.source)?;
                    RetainedMemberValue::Owned(input.clone())
                }
            };
            let erasure = self.runtime_feature_erasure_observed(
                expression.value(),
                source.declaration,
                source.span,
                observation,
            )?;
            let reference = kind_observed(node, observation)? == "EReference";
            observation.reserve(128 + 32, &source.span.source)?;
            match (&mut domains, projection) {
                (PreparedSourceDomains::Owned(rows), ProjectionItem::Owned(projection)) => {
                    rows.push(RuntimeFeatureDomain {
                        reference,
                        projection,
                        expression: expression.into_owned(),
                        erasure,
                    });
                }
                (PreparedSourceDomains::Views(rows), ProjectionItem::View(projection)) => {
                    rows.push(FeatureDomainSourceView {
                        reference,
                        projection,
                        expression,
                        erasure,
                    });
                }
                _ => return Err(error(&node.span, "Runtime domain source policy mismatch")),
            }
        }
        Ok(domains)
    }
    pub(crate) fn runtime_feature_erasure_observed(
        &self,
        expression: &Type,
        feature: &DeclarationId,
        span: &SourceSpan,
        observation: &crate::definitions::ecore::ParserObservation<'_, '_>,
    ) -> Result<RuntimeType, ImportError> {
        let expression = if observation.preparation_at(span, |context| {
            RuntimeType::expression_is_concrete_observed(expression, context)
        })? {
            std::borrow::Cow::Borrowed(expression)
        } else {
            let raw = self.check_legacy(feature)?;
            let mut selected = None;
            for signature in &raw {
                observation.work(1, &span.source)?;
                if signature.feature == "eType" {
                    selected = signature.raw_types.first();
                    break;
                }
            }
            let declaration =
                selected.ok_or_else(|| error(span, "Missing structural feature erasure"))?;
            let prepared_raw = if self.compiled_feature_inputs_selected(span)? {
                let raw = !self
                    .formal_info_observed(declaration, observation)?
                    .is_empty();
                if let Some(input) =
                    self.compiled_nominal_type_input(declaration, raw, span, observation)?
                {
                    return self.runtime_input_observed(
                        std::borrow::Cow::Borrowed(input.as_ref()),
                        span,
                        observation,
                    );
                }
                Some(raw)
            } else {
                None
            };
            observation.serialized(declaration, &span.source)?;
            observation.reserve(128, &span.source)?;
            std::borrow::Cow::Owned(Type::Classifier {
                raw: match prepared_raw {
                    Some(raw) => raw,
                    None => !self
                        .formal_info_observed(declaration, observation)?
                        .is_empty(),
                },
                declaration: declaration.clone(),
                arguments: vec![],
            })
        };
        self.runtime_input_observed(expression, span, observation)
    }

    pub fn instantiate_operation(
        &self,
        receiver: &ClosedType,
        operation: &DeclarationId,
        arguments: &[ClosedType],
    ) -> Result<AppliedOperation, ImportError> {
        self.instantiate_operation_in_owner(receiver, operation, None, arguments)
    }
    /// Check one exact inherited projection. The owner is matched against freshly
    /// resolved source, never trusted as a substitution or an override decision.
    /// Omitting it retains the requirement for an unambiguous declaration.
    pub fn instantiate_operation_in_owner(
        &self,
        receiver: &ClosedType,
        operation: &DeclarationId,
        owner: Option<&Type>,
        arguments: &[ClosedType],
    ) -> Result<AppliedOperation, ImportError> {
        let report = self.instantiate_members(receiver)?;
        let mut members = report
            .members
            .into_iter()
            .filter(|m| &m.declaration == operation && owner.is_none_or(|owner| owner == &m.owner));
        let mut member = members.next().ok_or_else(|| {
            error(
                &report.declaration_span,
                "Operation is not a member of the requested receiver/owner projection",
            )
        })?;
        if members.next().is_some() || !named(self.node(operation)?, "eOperations") {
            return Err(error(
                &member.span,
                "Expected one exact operation declaration",
            ));
        }
        let (environment, argument_bounds) = self.operation_bindings(&member, arguments)?;
        if let Some(failed) = argument_bounds.iter().find(|b| !b.satisfied) {
            return Err(error(
                &failed.declared_bound.span,
                "Operation application violates generic bounds",
            ));
        }
        let mut signature_bounds = Vec::new();
        for signature in &mut member.signatures {
            for ty in &mut signature.specialized {
                *ty = environment
                    .apply_simultaneous(ty)
                    .map_err(|e| error(&member.span, e))?;
                let checks = self.check_expression_bounds(ty, &member.span)?;
                if checks.iter().any(|c| !c.satisfied) {
                    return Err(error(
                        &member.span,
                        "Specialized operation signature violates generic bounds",
                    ));
                }
                signature_bounds.extend(checks);
            }
        }
        for formal in &mut member.formals {
            for bound in &mut formal.bounds {
                *bound = environment
                    .apply_simultaneous(bound)
                    .map_err(|e| error(&member.span, e))?;
            }
        }
        Ok(AppliedOperation {
            receiver: receiver.clone(),
            arguments: arguments.to_vec(),
            receiver_bounds: report.bounds,
            argument_bounds,
            signature_bounds,
            operation: member,
        })
    }
    /// Resolve explicit actual identities against retained declarations on demand.
    /// A structurally valid ClosedType alone is never trusted as semantic proof.
    pub fn instantiate_members(
        &self,
        receiver: &ClosedType,
    ) -> Result<RequestedMembers, ImportError> {
        let Type::Classifier { declaration, .. } = receiver.expression() else {
            return Err(error(
                &SourceSpan {
                    source: self.standard_id.clone(),
                    start: 0,
                    end: 0,
                },
                "Requested receiver must be a closed classifier",
            ));
        };
        let declaration_span = self.node(declaration)?.span.clone();
        if self.classifier(declaration)? != TargetKind::Class {
            return Err(error(
                &declaration_span,
                "Requested receiver must be an EClass",
            ));
        }
        let bounds = self.check_expression_bounds(receiver.expression(), &declaration_span)?;
        if let Some(failed) = bounds.iter().find(|check| !check.satisfied) {
            return Err(error(
                &failed.declared_bound.span,
                "Requested application violates generic bounds",
            ));
        }
        let members = self.project_member_types(receiver.expression(), &declaration_span, false)?;
        let parents = self
            .direct_parent_types(receiver.expression(), &declaration_span)?
            .into_iter()
            .map(|(ty, _)| ty)
            .collect();
        Ok(RequestedMembers {
            parents,
            receiver: receiver.clone(),
            declaration_span,
            bounds,
            members,
        })
    }

    /// Project an explicit runtime receiver. Missing generic arguments stay raw;
    /// erased structural signatures do not become actual type arguments.
    pub fn runtime_members(&self, receiver: &RuntimeType) -> Result<RuntimeMembers, ImportError> {
        self.runtime_members_observed(
            receiver,
            &crate::definitions::ecore::ParserObservation::new(
                &crate::definitions::conversion::InvocationContext::default(),
            ),
        )
    }
    pub(crate) fn runtime_members_observed(
        &self,
        receiver: &RuntimeType,
        observation: &crate::definitions::ecore::ParserObservation<'_, '_>,
    ) -> Result<RuntimeMembers, ImportError> {
        self.runtime_members_adopted_observed(receiver, observation, true)?
            .into_owned_observed(observation)
    }
    // Materialization consumes the checked source projections directly. Its
    // private report does not need the additional public inspection snapshot.
    pub(super) fn runtime_members_for_materialization_observed<'a>(
        &'a self,
        receiver: &RuntimeType,
        observation: &crate::definitions::ecore::ParserObservation<'_, '_>,
    ) -> Result<MaterializationMembers<'a>, ImportError> {
        self.runtime_members_adopted_observed(receiver, observation, false)
    }
    fn erased_signature_cache(&self) -> Option<&std::cell::RefCell<ErasedSignatureCache>> {
        self.prepared_erased_signatures
            .get()
            .map(|value| &**value)
            .filter(|_| {
                self.constructor_contracts.is_empty()
                    && self.constructor_relations.is_empty()
                    && self.provider_relations.is_empty()
                    && self.rigid_parameters.is_empty()
            })
    }
    fn shared_member_identity(
        &self,
        source: MemberSourceRef<'_>,
        observation: &crate::definitions::ecore::ParserObservation<'_, '_>,
    ) -> Result<Option<SharedMemberIdentity>, ImportError> {
        let Some(cache) = self.erased_signature_cache() else {
            return Ok(None);
        };
        observation.work(1, &source.span.source)?;
        let missing_owner = {
            let cache = cache.try_borrow().map_err(|_| {
                error(
                    source.span,
                    "Prepared member identities are already borrowed",
                )
            })?;
            if let Some(value) = cache
                .identities
                .get(source.owner)
                .and_then(|members| members.get(source.declaration))
            {
                return Ok(Some(value.clone()));
            }
            if cache.entries >= 4096 || cache.identity_entries >= 4096 {
                return Ok(None);
            }
            !cache.identities.contains_key(source.owner)
        };
        // Admission covers map keys, full payload objects, Rc headers and records
        // before any copies or allocations. Never store a semantic outcome.
        if missing_owner {
            observation.serialized(&source.owner, &source.span.source)?;
            observation.reserve(128, &source.span.source)?;
        }
        observation.serialized(&source.declaration, &source.span.source)?;
        observation.serialized(&(&source.declaration, &source.owner), &source.span.source)?;
        observation.reserve(128 + 2 * (128 + 32), &source.span.source)?;
        let mut cache = cache.try_borrow_mut().map_err(|_| {
            error(
                source.span,
                "Prepared member identities are already borrowed",
            )
        })?;
        let value = (
            std::rc::Rc::new(source.declaration.clone()),
            std::rc::Rc::new(source.owner.clone()),
        );
        if missing_owner {
            cache
                .identities
                .insert(source.owner.clone(), Default::default());
        }
        cache
            .identities
            .get_mut(source.owner)
            .ok_or_else(|| error(source.span, "Missing prepared member identity owner"))?
            .insert(source.declaration.clone(), value.clone());
        cache.identity_entries += 1;
        Ok(Some(value))
    }
    fn shared_erased_signatures(
        &self,
        source: MemberSourceRef<'_>,
        observation: &crate::definitions::ecore::ParserObservation<'_, '_>,
    ) -> Result<Option<std::rc::Rc<Vec<SpecializedSignature>>>, ImportError> {
        let Some(cache) = self.erased_signature_cache() else {
            return Ok(None);
        };
        observation.work(1, &source.span.source)?;
        let cache = cache.try_borrow().map_err(|_| {
            error(
                source.span,
                "Prepared erased signatures are already borrowed",
            )
        })?;
        Ok(cache
            .owners
            .get(source.owner)
            .and_then(|members| members.get(source.declaration))
            .cloned())
    }
    fn share_completed_erased_signatures(
        &self,
        source: MemberSourceRef<'_>,
        output: &mut ErasedSignatures,
        observation: &crate::definitions::ecore::ParserObservation<'_, '_>,
    ) -> Result<(), ImportError> {
        if !matches!(output, ErasedSignatures::Owned(_)) {
            return Ok(());
        }
        let Some(cache) = self.erased_signature_cache() else {
            return Ok(());
        };
        observation.work(1, &source.span.source)?;
        let missing_owner = {
            let cache = cache.try_borrow().map_err(|_| {
                error(
                    source.span,
                    "Prepared erased signatures are already borrowed",
                )
            })?;
            if cache.entries >= 4096 {
                return Ok(());
            }
            !cache.owners.contains_key(source.owner)
        };
        if missing_owner {
            observation.serialized(&source.owner, &source.span.source)?;
            observation.reserve(128, &source.span.source)?;
        }
        observation.serialized(&source.declaration, &source.span.source)?;
        observation.reserve(128 + 32, &source.span.source)?;
        let mut cache = cache.try_borrow_mut().map_err(|_| {
            error(
                source.span,
                "Prepared erased signatures are already borrowed",
            )
        })?;
        let ErasedSignatures::Owned(rows) =
            std::mem::replace(output, ErasedSignatures::Owned(Vec::new()))
        else {
            return Err(error(source.span, "Missing owned erased signatures"));
        };
        let rows = std::rc::Rc::new(rows);
        if !cache.owners.contains_key(source.owner) {
            cache
                .owners
                .insert(source.owner.clone(), Default::default());
        }
        if cache
            .owners
            .get_mut(source.owner)
            .ok_or_else(|| error(source.span, "Missing erased signature owner"))?
            .insert(source.declaration.clone(), rows.clone())
            .is_none()
        {
            cache.entries += 1;
        }
        *output = ErasedSignatures::Shared(rows);
        Ok(())
    }
    fn runtime_members_adopted_observed<'a>(
        &'a self,
        receiver: &RuntimeType,
        observation: &crate::definitions::ecore::ParserObservation<'_, '_>,
        retain_source_snapshot: bool,
    ) -> Result<MaterializationMembers<'a>, ImportError> {
        let mut preparation = RuntimeMembersPreparation::begin(
            self,
            std::borrow::Cow::Borrowed(receiver),
            retain_source_snapshot,
        );
        while !preparation.advance(observation)? {}
        preparation.finish_parts().map(|(report, _)| report)
    }
    fn retain_runtime_domain(
        &self,
        domain: RuntimeDomainRef<'_>,
        span: &SourceSpan,
        observation: &crate::definitions::ecore::ParserObservation<'_, '_>,
    ) -> Result<bool, ImportError> {
        Ok(!observation.preparation_at(span, |context| {
            RuntimeType::expression_is_concrete_observed(domain.expression, context)
        })? || domain.projection.source.owner != domain.projection.storage_owner.expression())
    }
    // Retain only the full immutable input expression. The caller still traverses
    // source bounds, checks depth and tests concreteness on every use.
    fn shared_bound_input(
        &self,
        ty: &Type,
        span: &SourceSpan,
        observation: &crate::definitions::ecore::ParserObservation<'_, '_>,
    ) -> Result<Option<std::rc::Rc<Type>>, ImportError> {
        let Some(cache) = self.erased_signature_cache() else {
            return Ok(None);
        };
        observation.work(1, &span.source)?;
        {
            let cache = cache
                .try_borrow()
                .map_err(|_| error(span, "Prepared bound inputs are already borrowed"))?;
            if let Some(value) = cache.bound_inputs.get(ty) {
                return Ok(Some(value.clone()));
            }
            if cache.bound_inputs.len() >= 4096 {
                return Ok(None);
            }
        }
        observation.serialized(ty, &span.source)?;
        observation.serialized(ty, &span.source)?;
        observation.reserve(128 + 128 + 32, &span.source)?;
        let mut cache = cache
            .try_borrow_mut()
            .map_err(|_| error(span, "Prepared bound inputs are already borrowed"))?;
        let value = std::rc::Rc::new(ty.clone());
        cache.bound_inputs.insert(ty.clone(), value.clone());
        Ok(Some(value))
    }
    fn runtime_feature_bounds(
        &self,
        ty: &Type,
        span: &SourceSpan,
        depth: usize,
        result: &mut Vec<RetainedMemberValue<Type>>,
        identity_views: bool,
        observation: &crate::definitions::ecore::ParserObservation<'_, '_>,
    ) -> Result<(), ImportError> {
        observation.work(1, &span.source)?;
        if depth >= 128 || result.len() >= 1024 {
            return Err(error(span, "Raw feature bound expansion budget exceeded"));
        }
        match ty {
            Type::RigidParameter { .. } => {
                return Err(error(
                    span,
                    "Rigid parameters cannot enter runtime feature bounds",
                ));
            }
            Type::Parameter { declaration } => {
                for bound in self.parameter_bound_types(declaration)? {
                    observation.work(1, &span.source)?;
                    self.runtime_feature_bounds(
                        &bound.expression,
                        &bound.span,
                        depth + 1,
                        result,
                        identity_views,
                        observation,
                    )?;
                }
            }
            Type::Classifier { .. } => {
                // Symbolic nested obligations are retained in feature_domains and
                // compiled against actual candidate identities, not erased here.
                if !observation.preparation_at(span, |context| {
                    RuntimeType::expression_is_concrete_observed(ty, context)
                })? {
                    return Ok(());
                }
                let value = if identity_views {
                    self.shared_bound_input(ty, span, observation)?
                } else {
                    None
                };
                let value = match value {
                    Some(value) => RetainedMemberValue::Shared(value),
                    None => {
                        observation.serialized(ty, &span.source)?;
                        RetainedMemberValue::Owned(ty.clone())
                    }
                };
                observation.reserve(32, &span.source)?;
                result.push(value);
            }
            Type::Wildcard { .. } => return Err(error(span, "Wildcard structural feature target")),
        }
        Ok(())
    }
    #[cfg(test)]
    pub(crate) fn runtime_classifier_projection_observed(
        &self,
        ty: &Type,
        span: &SourceSpan,
        observation: &crate::definitions::ecore::ParserObservation<'_, '_>,
    ) -> Result<Type, ImportError> {
        match self.runtime_classifier_projection_expression_observed(ty, span, observation)? {
            std::borrow::Cow::Borrowed(expression) => {
                observation.serialized(expression, &span.source)?;
                Ok(expression.clone())
            }
            std::borrow::Cow::Owned(expression) => Ok(expression),
        }
    }
    // A caller that already owns the expression can adopt a concrete projection
    // unchanged. The public borrowed-input path still charges its actual clone.
    fn runtime_classifier_projection_expression_observed<'a>(
        &self,
        ty: &'a Type,
        span: &SourceSpan,
        observation: &crate::definitions::ecore::ParserObservation<'_, '_>,
    ) -> Result<std::borrow::Cow<'a, Type>, ImportError> {
        if observation.preparation_at(span, |context| {
            RuntimeType::expression_is_concrete_observed(ty, context)
        })? {
            return Ok(std::borrow::Cow::Borrowed(ty));
        }
        let Type::Classifier { declaration, .. } = ty else {
            return Err(error(span, "Runtime owner requires a classifier"));
        };
        observation.serialized(declaration, &span.source)?;
        observation.reserve(128, &span.source)?;
        Ok(std::borrow::Cow::Owned(Type::Classifier {
            declaration: declaration.clone(),
            arguments: vec![],
            raw: !self
                .formal_info_observed(declaration, observation)?
                .is_empty(),
        }))
    }

    pub fn member_types(&self, usage: &DeclarationId) -> Result<MemberTypes, ImportError> {
        let receiver_bounds = self.application_bounds(usage)?;
        if !receiver_bounds.valid {
            return Err(error(
                &receiver_bounds.selected.span,
                "Receiver application violates generic bounds",
            ));
        }
        let members = self.project_member_types(
            &receiver_bounds.selected.expression,
            &receiver_bounds.selected.span,
            false,
        )?;
        Ok(MemberTypes {
            receiver_bounds,
            members,
        })
    }
    pub(super) fn project_member_types(
        &self,
        expression: &Type,
        span: &SourceSpan,
        runtime: bool,
    ) -> Result<Vec<SpecializedMember>, ImportError> {
        self.project_member_types_observed(
            expression,
            span,
            runtime,
            &crate::definitions::ecore::ParserObservation::new(
                &crate::definitions::conversion::InvocationContext::default(),
            ),
        )
    }
    fn project_member_types_observed(
        &self,
        expression: &Type,
        span: &SourceSpan,
        runtime: bool,
        observation: &crate::definitions::ecore::ParserObservation<'_, '_>,
    ) -> Result<Vec<SpecializedMember>, ImportError> {
        match self.project_member_sources_observed(expression, span, runtime, false, observation)? {
            SourceMembers::Owned(rows) => Ok(rows),
            SourceMembers::Views(_) => Err(error(span, "Expected owned member projection")),
        }
    }
    fn project_member_sources_observed<'a>(
        &'a self,
        expression: &Type,
        span: &SourceSpan,
        runtime: bool,
        views: bool,
        observation: &crate::definitions::ecore::ParserObservation<'_, '_>,
    ) -> Result<SourceMembers<'a>, ImportError> {
        let mut preparation = SourceMembersPreparation::begin(self, runtime, views);
        while !preparation.advance(expression, span, observation)? {}
        preparation.finish(span)
    }
    #[cfg(test)]
    fn specialize_signatures_observed(
        &self,
        owner: &DeclarationId,
        environment: &Substitution,
        observation: &crate::definitions::ecore::ParserObservation<'_, '_>,
    ) -> Result<Vec<SpecializedSignature>, ImportError> {
        let mut preparation = SignaturePreparation::begin(self, false);
        while !preparation.advance(owner, environment, observation)? {}
        match preparation.finish(owner)? {
            ErasedSignatures::Owned(rows) => Ok(rows),
            ErasedSignatures::Shared(_) => Err(error(
                &fallback(owner),
                "Expected owned signature specialization",
            )),
        }
    }
}

#[cfg(test)]
mod owned_projection_tests {
    use super::*;
    use crate::definitions::{
        bundle::{DefinitionBundle, DefinitionSource, SourceKind},
        conversion::{InvocationContext, InvocationObserver, ProviderId},
        ecore::{ObservedImportError, ParserObservation},
    };
    use std::{borrow::Cow, cell::Cell, rc::Rc};

    struct Storage {
        bytes: Cell<usize>,
        limit: usize,
    }
    impl InvocationObserver for Storage {
        fn before_invoke(&self, _: &ProviderId) -> Result<(), String> {
            panic!("projection cannot invoke providers")
        }
        fn retain_storage(&self, bytes: usize) -> Result<(), String> {
            let next = self.bytes.get() + bytes;
            if next > self.limit {
                return Err("projection storage denied".into());
            }
            self.bytes.set(next);
            Ok(())
        }
    }

    #[test]
    fn owned_classifier_projection_adopts_concrete_types_and_charges_new_erasure() {
        let source = DefinitionSource::new(
            "memory:projection.ecore",
            SourceKind::Ecore,
            "<e:EPackage xmlns:e='http://www.eclipse.org/emf/2002/Ecore' xmlns:x='http://www.w3.org/2001/XMLSchema-instance' name='p' nsURI='urn:projection'><eClassifiers x:type='e:EClass' name='Root'><eTypeParameters name='T'/></eClassifiers><eClassifiers x:type='e:EClass' name='Child' eSuperTypes='#//Root'/></e:EPackage>",
        );
        let resolver = Resolver::new(&DefinitionBundle {
            version: 1,
            sources: vec![source.clone()],
            namespaces: vec![],
        })
        .unwrap();
        let declaration = DeclarationId {
            source: source.id.clone(),
            path: vec![0],
        };
        let span = &resolver.node(&declaration).unwrap().span;
        let raw = Type::Classifier {
            declaration: declaration.clone(),
            arguments: vec![],
            raw: true,
        };
        let child = Type::Classifier {
            declaration: DeclarationId {
                source: source.id.clone(),
                path: vec![1],
            },
            arguments: vec![],
            raw: false,
        };
        let open = Type::Classifier {
            declaration,
            arguments: vec![Type::Parameter {
                declaration: DeclarationId {
                    source: source.id.clone(),
                    path: vec![0, 0],
                },
            }],
            raw: false,
        };
        for input in [&raw, &child, &open] {
            let run = |limit, owned: bool| {
                let retained_input = input.clone();
                let receipt = Rc::new(Storage {
                    bytes: Cell::new(0),
                    limit,
                });
                let context = InvocationContext::new(receipt.clone());
                let observation = ParserObservation::new(&context);
                // The input is already owned before admission. Traversal
                // storage remains charged; adoption avoids only the deep copy.
                let result = if owned {
                    match observation.finish(
                        resolver.runtime_classifier_projection_expression_observed(
                            &retained_input,
                            span,
                            &observation,
                        ),
                    ) {
                        Ok(Cow::Borrowed(_)) => Ok(retained_input),
                        Ok(Cow::Owned(projected)) => Ok(projected),
                        Err(error) => Err(error),
                    }
                } else {
                    observation.finish(resolver.runtime_classifier_projection_observed(
                        input,
                        span,
                        &observation,
                    ))
                };
                (result, receipt.bytes.get())
            };
            let (public, public_bytes) = run(usize::MAX, false);
            let (adopted, adopted_bytes) = run(usize::MAX, true);
            assert_eq!(adopted, public);
            if input == &open {
                assert_eq!(adopted.unwrap(), raw);
                assert_eq!(adopted_bytes, public_bytes);
                assert!(adopted_bytes > 0);
                assert_eq!(
                    run(adopted_bytes - 1, true).0.unwrap_err(),
                    ObservedImportError::Observation("projection storage denied".into()),
                );
            } else {
                assert!(adopted_bytes > 0);
                assert!(public_bytes > adopted_bytes);
                assert_eq!(run(adopted_bytes, true).0.unwrap(), *input);
                assert_eq!(
                    run(public_bytes - 1, false).0.unwrap_err(),
                    ObservedImportError::Observation("projection storage denied".into()),
                );
            }
            assert_eq!(run(adopted_bytes, true).0.unwrap(), public.unwrap());
        }
    }
    #[test]
    fn prepared_member_owner_views_preserve_complete_fresh_results_and_owned_admission() {
        let source = DefinitionSource::new(
            "memory:member-owner-views.ecore",
            SourceKind::Ecore,
            include_str!("../../assets/fixtures/ecore-generics/raw-feature-domains.ecore"),
        );
        let bundle = DefinitionBundle {
            version: 1,
            sources: vec![source.clone()],
            namespaces: vec![],
        };
        let fresh = Resolver::new(&bundle).unwrap();
        let mut prepared = Resolver::new(&bundle).unwrap();
        let context = InvocationContext::default();
        let observation = ParserObservation::new(&context);
        prepared
            .resolve_materialization_inventory_observed(&observation)
            .unwrap();
        for (index, raw) in [(8, true), (9, true), (14, false), (16, true)] {
            let declaration = DeclarationId {
                source: source.id.clone(),
                path: vec![index],
            };
            let expression = Type::Classifier {
                declaration: declaration.clone(),
                arguments: vec![],
                raw,
            };
            let span = &fresh.node(&declaration).unwrap().span;
            // The compiler's receiver checks prepare this exact ancestry before
            // projection; preserve both shared and applied-ancestor fallback rows.
            prepared
                .ancestry_view_observed(&expression, span, &observation)
                .unwrap();
            let run = |resolver: &Resolver, limit| {
                let meter = Rc::new(Storage {
                    bytes: Cell::new(0),
                    limit,
                });
                let context = InvocationContext::new(meter.clone());
                let observation = ParserObservation::new(&context);
                let result = observation.finish(resolver.project_member_types_observed(
                    &expression,
                    span,
                    true,
                    &observation,
                ));
                (result, meter.bytes.get())
            };
            let (fresh_result, fresh_bytes) = run(&fresh, usize::MAX);
            let (prepared_result, prepared_bytes) = run(&prepared, usize::MAX);
            assert_eq!(prepared_result, fresh_result);
            assert_eq!(
                serde_json::to_value(prepared_result.as_ref().unwrap()).unwrap(),
                serde_json::to_value(fresh_result.as_ref().unwrap()).unwrap()
            );
            assert!(fresh_bytes > prepared_bytes);
            assert!(
                prepared_bytes > 0,
                "owned member outputs still require admission"
            );
            assert_eq!(run(&prepared, prepared_bytes).0, prepared_result);
            assert_eq!(
                run(&prepared, prepared_bytes - 1).0.unwrap_err(),
                ObservedImportError::Observation("projection storage denied".into())
            );
            let mut altered = prepared_result.unwrap();
            if let Some(member) = altered.first_mut() {
                member.source_payload.clear();
                member.signatures.clear();
                member.formals.clear();
                member.name.clear();
            }
            assert_eq!(run(&prepared, prepared_bytes).0, fresh_result);
        }
        assert_eq!(bundle.sources[0], source);
    }

    #[test]
    fn complete_member_identities_share_exact_source_values_with_admitted_copies() {
        struct Meter {
            bytes: Cell<usize>,
            calls: Cell<usize>,
            limit: usize,
            stop: usize,
        }
        impl Meter {
            fn step(&self) -> Result<(), String> {
                let call = self.calls.get();
                if call == self.stop {
                    return Err("identity interrupted".into());
                }
                self.calls.set(call + 1);
                Ok(())
            }
        }
        impl crate::definitions::conversion::InvocationObserver for Meter {
            fn before_invoke(
                &self,
                _: &crate::definitions::conversion::ProviderId,
            ) -> Result<(), String> {
                panic!("identity metadata cannot invoke providers")
            }
            fn work(&self, _: usize) -> Result<(), String> {
                self.step()
            }
            fn retain_storage(&self, bytes: usize) -> Result<(), String> {
                self.step()?;
                let next = self
                    .bytes
                    .get()
                    .checked_add(bytes)
                    .filter(|next| *next <= self.limit)
                    .ok_or("identity storage denied")?;
                self.bytes.set(next);
                Ok(())
            }
        }
        let (bundle, raw, concrete) = shared_signature_fixture();
        let before = serde_json::to_value(&bundle).unwrap();
        let source = Resolver::new(&bundle)
            .unwrap()
            .runtime_members(&concrete)
            .unwrap()
            .source_members
            .remove(0);
        let run = |limit, stop| {
            let resolver = prepared_signature_resolver(&bundle);
            let meter = Rc::new(Meter {
                bytes: Cell::new(0),
                calls: Cell::new(0),
                limit,
                stop,
            });
            let context = InvocationContext::new(meter.clone());
            let observation = ParserObservation::new(&context);
            let result =
                observation.finish(resolver.shared_member_identity((&source).into(), &observation));
            let entries = resolver
                .prepared_erased_signatures
                .get()
                .unwrap()
                .borrow()
                .identity_entries;
            if let Ok(Some((declaration, owner))) = &result {
                assert_eq!(declaration.as_ref(), &source.declaration);
                assert_eq!(owner.as_ref(), &source.owner);
            }
            (result, meter.bytes.get(), meter.calls.get(), entries)
        };
        let (complete, bytes, calls, entries) = run(usize::MAX, usize::MAX);
        assert!(complete.unwrap().is_some());
        assert_eq!(entries, 1);
        assert!(bytes > 0);
        assert!(run(bytes, usize::MAX).0.unwrap().is_some());
        assert!(run(bytes - 1, usize::MAX).0.is_err());
        for stop in 0..calls {
            let (denied, _, _, entries) = run(usize::MAX, stop);
            assert!(denied.is_err());
            assert_eq!(entries, 0);
            assert!(run(bytes, usize::MAX).0.unwrap().is_some());
        }
        let resolver = prepared_signature_resolver(&bundle);
        let context = InvocationContext::default();
        let observation = ParserObservation::new(&context);
        let cold = resolver
            .runtime_members_for_materialization_observed(&raw, &observation)
            .unwrap();
        let warm = resolver
            .runtime_members_for_materialization_observed(&concrete, &observation)
            .unwrap();
        for (left, right) in cold.members.iter().zip(&warm.members) {
            let (RetainedMemberValue::Shared(a), RetainedMemberValue::Shared(b)) =
                (&left.declaration, &right.declaration)
            else {
                panic!("member declaration was copied");
            };
            assert!(Rc::ptr_eq(a, b));
            let public = right.owned_copy_observed(&observation).unwrap();
            assert_eq!(
                serde_json::to_value(right).unwrap(),
                serde_json::to_value(&public).unwrap()
            );
        }
        let fallback = prepared_signature_resolver(&bundle);
        fallback
            .prepared_erased_signatures
            .get()
            .unwrap()
            .borrow_mut()
            .identity_entries = 4096;
        assert!(
            fallback
                .shared_member_identity((&source).into(), &observation)
                .unwrap()
                .is_none()
        );
        assert_eq!(serde_json::to_value(&bundle).unwrap(), before);
    }

    fn shared_signature_fixture() -> (DefinitionBundle, RuntimeType, RuntimeType) {
        let source = DefinitionSource::new(
            "memory:signature-sharing.ecore",
            SourceKind::Ecore,
            r##"<e:EPackage xmlns:e="http://www.eclipse.org/emf/2002/Ecore" xmlns:x="http://www.w3.org/2001/XMLSchema-instance" name="s" nsURI="urn:signatures">
<eClassifiers x:type="e:EClass" name="Plain"/>
<eClassifiers x:type="e:EClass" name="G"><eTypeParameters name="T"><eBounds eClassifier="#//Plain"/></eTypeParameters><eStructuralFeatures x:type="e:EReference" name="value"><eGenericType eTypeParameter="#//G/T"/></eStructuralFeatures><eOperations name="echo" eType="#//Plain"><eParameters name="v" eType="#//Plain"/><eAnnotations source="urn:unicode"><details key="méta" value="café"/></eAnnotations></eOperations></eClassifiers>
<eClassifiers x:type="e:EClass" name="Child"><eTypeParameters name="U"/><eGenericSuperTypes eClassifier="#//G"><eTypeArguments eClassifier="#//Plain"/></eGenericSuperTypes></eClassifiers>
<eClassifiers x:type="e:EClass" name="Other"><eGenericSuperTypes eClassifier="#//G"><eTypeArguments eClassifier="#//Plain"/></eGenericSuperTypes></eClassifiers>
</e:EPackage>"##,
        );
        let receiver = |index, raw| {
            RuntimeType::new(Type::Classifier {
                declaration: DeclarationId {
                    source: source.id.clone(),
                    path: vec![index],
                },
                arguments: vec![],
                raw,
            })
            .unwrap()
        };
        let raw = receiver(2, true);
        let concrete = receiver(3, false);
        (
            DefinitionBundle {
                version: 1,
                sources: vec![source],
                namespaces: vec![],
            },
            raw,
            concrete,
        )
    }
    fn prepared_signature_resolver(bundle: &DefinitionBundle) -> Resolver {
        let mut resolver = Resolver::new(bundle).unwrap();
        resolver
            .resolve_materialization_inventory_observed(&ParserObservation::new(
                &InvocationContext::default(),
            ))
            .unwrap();
        resolver
    }
    #[test]
    fn complete_erased_signature_views_share_metadata_but_recheck_receiver_constraints() {
        let (bundle, raw, concrete) = shared_signature_fixture();
        let before = serde_json::to_value(&bundle).unwrap();
        let plain = Resolver::new(&bundle).unwrap();
        let resolver = prepared_signature_resolver(&bundle);
        let context = InvocationContext::default();
        let observation = ParserObservation::new(&context);
        let cold = resolver
            .runtime_members_for_materialization_observed(&raw, &observation)
            .unwrap();
        let warm = resolver
            .runtime_members_for_materialization_observed(&concrete, &observation)
            .unwrap();
        assert!(!cold.reference_bounds.is_empty());
        assert!(warm.reference_bounds.is_empty());
        assert!(!cold.warnings.is_empty());
        assert!(warm.warnings.is_empty());
        for (view, receiver) in [(&cold, &raw), (&warm, &concrete)] {
            let mut public = plain.runtime_members(receiver).unwrap();
            public.source_members.clear();
            assert_eq!(*view, public);
            assert_eq!(
                serde_json::to_value(view).unwrap(),
                serde_json::to_value(&public).unwrap()
            );
            for (member, owned) in view.members.iter().zip(&public.members) {
                assert_eq!(member.owned_copy_observed(&observation).unwrap(), *owned);
            }
        }
        assert_eq!(cold.members.len(), 2);
        for (left, right) in cold.members.iter().zip(&warm.members) {
            let (ErasedSignatures::Shared(left), ErasedSignatures::Shared(right)) =
                (&left.signatures, &right.signatures)
            else {
                panic!("complete signatures were copied");
            };
            assert!(Rc::ptr_eq(left, right));
        }
        assert!(
            cold.members
                .iter()
                .any(|member| member.source_payload.contains("méta"))
        );
        assert_eq!(serde_json::to_value(&bundle).unwrap(), before);
        let conversions = crate::definitions::conversion::ConversionRegistry::default();
        let constructors = crate::definitions::conversion::ConstructorRegistry::default();
        for changed in [
            resolver.clone().with_conversions(&conversions).unwrap(),
            resolver.clone().with_constructors(&constructors).unwrap(),
            resolver.clone().with_rigid_parameters(vec![]).unwrap(),
        ] {
            assert!(changed.prepared_erased_signatures.get().is_none());
            let result = changed
                .runtime_members_for_materialization_observed(&concrete, &observation)
                .unwrap();
            assert!(
                result
                    .members
                    .iter()
                    .all(|member| matches!(&member.signatures, ErasedSignatures::Owned(_)))
            );
        }
    }
    #[test]
    fn shared_signature_owned_inspection_precharges_complete_copies_and_growth_falls_back() {
        let (bundle, _, receiver) = shared_signature_fixture();
        let plain = Resolver::new(&bundle).unwrap();
        let resolver = prepared_signature_resolver(&bundle);
        let context = InvocationContext::default();
        let observation = ParserObservation::new(&context);
        let report = resolver
            .runtime_members_for_materialization_observed(&receiver, &observation)
            .unwrap();
        let mut expected = plain.runtime_members(&receiver).unwrap();
        expected.source_members.clear();
        for (member, expected_member) in report.members.iter().zip(&expected.members) {
            let run = |limit| {
                let meter = Rc::new(Storage {
                    bytes: Cell::new(0),
                    limit,
                });
                let context = InvocationContext::new(meter.clone());
                let observation = ParserObservation::new(&context);
                let result = observation.finish(member.owned_copy_observed(&observation));
                (result, meter.bytes.get())
            };
            let (full, bytes) = run(usize::MAX);
            assert_eq!(full.as_ref().unwrap(), expected_member);
            assert!(bytes > 0);
            assert_eq!(run(bytes).0.unwrap(), *expected_member);
            assert_eq!(
                run(bytes - 1).0.unwrap_err(),
                ObservedImportError::Observation("projection storage denied".into())
            );
            let mut altered = full.unwrap();
            altered.signatures.clear();
            altered.source_payload.clear();
            altered.name.clear();
            assert_eq!(run(bytes).0.unwrap(), *expected_member);
        }
        let fallback = prepared_signature_resolver(&bundle);
        // Exercise the private growth-policy boundary without constructing a
        // second broad model fixture; this is not scale qualification.
        fallback
            .prepared_erased_signatures
            .get()
            .unwrap()
            .borrow_mut()
            .entries = 4096;
        let view = fallback
            .runtime_members_for_materialization_observed(&receiver, &observation)
            .unwrap();
        assert!(
            view.members
                .iter()
                .all(|member| matches!(&member.signatures, ErasedSignatures::Owned(_)))
        );
        assert_eq!(view, expected);
        assert_eq!(
            serde_json::to_value(view).unwrap(),
            serde_json::to_value(expected).unwrap()
        );
    }

    #[test]
    fn signature_publication_admits_keys_before_retention_and_cancels_every_checkpoint() {
        struct Meter {
            bytes: Cell<usize>,
            calls: Cell<usize>,
            limit: usize,
            stop: Option<usize>,
        }
        impl InvocationObserver for Meter {
            fn before_invoke(&self, _: &ProviderId) -> Result<(), String> {
                panic!("metadata cannot invoke providers");
            }
            fn checkpoint(&self) -> Result<(), String> {
                let call = self.calls.get();
                if self.stop == Some(call) {
                    return Err("signature view cancelled".into());
                }
                self.calls.set(call + 1);
                Ok(())
            }
            fn retain_storage(&self, bytes: usize) -> Result<(), String> {
                let next = self.bytes.get() + bytes;
                if next > self.limit {
                    return Err("signature view storage denied".into());
                }
                self.bytes.set(next);
                Ok(())
            }
        }
        let (bundle, raw, _) = shared_signature_fixture();
        let original = Resolver::new(&bundle)
            .unwrap()
            .runtime_members(&raw)
            .unwrap()
            .source_members
            .remove(0);
        let run = |limit, stop| {
            let resolver = prepared_signature_resolver(&bundle);
            let meter = Rc::new(Meter {
                bytes: Cell::new(0),
                calls: Cell::new(0),
                limit,
                stop,
            });
            let context = InvocationContext::new(meter.clone());
            let observation = ParserObservation::new(&context);
            // This boundary adopts an already-owned complete signature list.
            let mut output = ErasedSignatures::Owned(original.signatures.clone());
            let result = observation.finish(resolver.share_completed_erased_signatures(
                (&original).into(),
                &mut output,
                &observation,
            ));
            let entries = resolver
                .prepared_erased_signatures
                .get()
                .unwrap()
                .borrow()
                .entries;
            if result.is_ok() {
                assert_eq!(output.deref_rows(), original.signatures.as_slice());
            }
            (result, meter.bytes.get(), meter.calls.get(), entries)
        };
        let (full, bytes, calls, entries) = run(usize::MAX, None);
        full.unwrap();
        assert!(bytes > 0);
        assert_eq!(entries, 1);
        assert!(run(bytes, None).0.is_ok());
        let denied = run(bytes - 1, None);
        assert_eq!(
            denied.0.unwrap_err(),
            ObservedImportError::Observation("signature view storage denied".into())
        );
        assert_eq!(denied.3, 0);
        for stop in 0..calls {
            let cancelled = run(bytes, Some(stop));
            assert_eq!(
                cancelled.0.unwrap_err(),
                ObservedImportError::Observation("signature view cancelled".into())
            );
            assert_eq!(cancelled.3, 0);
            assert!(run(bytes, None).0.is_ok());
        }
    }

    #[test]
    fn private_erasure_metadata_borrows_exact_source_and_serializes_as_the_public_report() {
        let source = DefinitionSource::new(
            "memory:borrowed-member.ecore",
            SourceKind::Ecore,
            include_str!("../../assets/fixtures/ecore-generics/raw-feature-domains.ecore"),
        );
        let resolver = Resolver::new(&DefinitionBundle {
            version: 1,
            sources: vec![source.clone()],
            namespaces: vec![],
        })
        .unwrap();
        let receiver = RuntimeType::new(Type::Classifier {
            declaration: DeclarationId {
                source: source.id.clone(),
                path: vec![8],
            },
            arguments: vec![],
            raw: true,
        })
        .unwrap();
        let context = InvocationContext::default();
        let observation = crate::definitions::ecore::ParserObservation::new(&context);
        let private = resolver
            .runtime_members_for_materialization_observed(&receiver, &observation)
            .unwrap();
        let mut public = resolver.runtime_members(&receiver).unwrap();
        assert!(private.source_members.is_empty());
        assert!(!private.members.is_empty());
        public.source_members.clear();
        assert_eq!(private, public);
        assert_eq!(
            serde_json::to_value(&private).unwrap(),
            serde_json::to_value(&public).unwrap()
        );
        for member in &private.members {
            let node = resolver.node(&member.declaration).unwrap();
            assert!(matches!(&member.name, std::borrow::Cow::Borrowed(_)));
            assert!(matches!(
                &member.source_payload,
                std::borrow::Cow::Borrowed(_)
            ));
            let std::borrow::Cow::Borrowed(span) = &member.span else {
                panic!("private source span was copied")
            };
            assert!(std::ptr::eq(*span, &node.span));
            let document = resolver
                .resources(&member.declaration)
                .document(&member.declaration.source)
                .unwrap();
            let payload = &document.source.text[node.span.start..node.span.end];
            assert!(std::ptr::eq(member.source_payload.as_ref(), payload));
        }
    }
    #[test]
    fn private_bounds_share_complete_declarations_and_receiver_spans() {
        use std::rc::Rc;
        let (bundle, raw, _) = shared_signature_fixture();
        let before = serde_json::to_value(&bundle).unwrap();
        let resolver = prepared_signature_resolver(&bundle);
        let context = InvocationContext::default();
        let observation = crate::definitions::ecore::ParserObservation::new(&context);
        let private = resolver
            .runtime_members_for_materialization_observed(&raw, &observation)
            .unwrap();
        let mut public = Resolver::new(&bundle)
            .unwrap()
            .runtime_members(&raw)
            .unwrap();
        public.source_members.clear();
        assert_eq!(private, public);
        assert_eq!(
            serde_json::to_value(&private).unwrap(),
            serde_json::to_value(&public).unwrap()
        );
        let std::borrow::Cow::Borrowed(span) = &private.declaration_span else {
            panic!("private receiver span was copied");
        };
        let Type::Classifier { declaration, .. } = raw.expression() else {
            panic!("expected classifier");
        };
        assert!(std::ptr::eq(
            *span,
            &resolver.node(declaration).unwrap().span
        ));
        assert!(!private.reference_bounds.is_empty());
        for bound in &private.reference_bounds {
            let member = private
                .members
                .iter()
                .find(|member| member.declaration.value() == bound.feature.value())
                .unwrap();
            let (RetainedMemberValue::Shared(left), RetainedMemberValue::Shared(right)) =
                (&member.declaration, &bound.feature)
            else {
                panic!("private bound declaration was copied");
            };
            assert!(Rc::ptr_eq(left, right));
            assert_eq!(
                left.as_ref(),
                &public
                    .reference_bounds
                    .iter()
                    .find(|other| &other.feature == left.as_ref())
                    .unwrap()
                    .feature
            );
        }
        let fallback = prepared_signature_resolver(&bundle);
        fallback
            .prepared_erased_signatures
            .get()
            .unwrap()
            .borrow_mut()
            .identity_entries = 4096;
        let fallback = fallback
            .runtime_members_for_materialization_observed(&raw, &observation)
            .unwrap();
        assert!(
            fallback
                .reference_bounds
                .iter()
                .all(|bound| matches!(&bound.feature, RetainedMemberValue::Owned(_)))
        );
        assert_eq!(
            serde_json::to_value(&fallback).unwrap(),
            serde_json::to_value(&public).unwrap()
        );
        assert_eq!(serde_json::to_value(&bundle).unwrap(), before);
    }

    #[test]
    fn private_bound_and_span_adoption_admits_complete_copies_before_inspection() {
        use crate::definitions::{
            conversion::{InvocationObserver, ProviderId},
            ecore::ParserObservation,
        };
        use std::{cell::Cell, rc::Rc};
        struct Meter {
            bytes: Cell<usize>,
            calls: Cell<usize>,
            limit: usize,
            stop: usize,
        }
        impl Meter {
            fn step(&self) -> Result<(), String> {
                let call = self.calls.get();
                self.calls.set(call + 1);
                if call == self.stop {
                    Err("adoption cancelled".into())
                } else {
                    Ok(())
                }
            }
        }
        impl InvocationObserver for Meter {
            fn before_invoke(&self, _: &ProviderId) -> Result<(), String> {
                Err("no providers".into())
            }
            fn work(&self, _: usize) -> Result<(), String> {
                self.step()
            }
            fn retain_storage(&self, bytes: usize) -> Result<(), String> {
                self.step()?;
                let next = self
                    .bytes
                    .get()
                    .checked_add(bytes)
                    .filter(|next| *next <= self.limit)
                    .ok_or("adoption storage denied")?;
                self.bytes.set(next);
                Ok(())
            }
        }
        let (bundle, raw, _) = shared_signature_fixture();
        let before = serde_json::to_value(&bundle).unwrap();
        let mut expected = Resolver::new(&bundle)
            .unwrap()
            .runtime_members(&raw)
            .unwrap();
        expected.source_members.clear();
        let run = |limit, stop| {
            let resolver = prepared_signature_resolver(&bundle);
            let context = InvocationContext::default();
            let observation = ParserObservation::new(&context);
            let report = resolver
                .runtime_members_for_materialization_observed(&raw, &observation)
                .unwrap();
            let meter = Rc::new(Meter {
                bytes: Cell::new(0),
                calls: Cell::new(0),
                limit,
                stop,
            });
            let context = InvocationContext::new(meter.clone());
            let observation = ParserObservation::new(&context);
            let result = observation.finish(report.into_owned_observed(&observation));
            if let Ok(value) = &result {
                assert_eq!(value, &expected);
                assert_eq!(
                    serde_json::to_value(value).unwrap(),
                    serde_json::to_value(&expected).unwrap()
                );
            }
            (result, meter.bytes.get(), meter.calls.get())
        };
        let (complete, bytes, calls) = run(usize::MAX, usize::MAX);
        complete.unwrap();
        assert!(bytes > 0);
        assert!(run(bytes, usize::MAX).0.is_ok());
        assert!(run(bytes - 1, usize::MAX).0.is_err());
        for stop in 0..calls {
            assert!(run(usize::MAX, stop).0.is_err());
            assert!(run(bytes, usize::MAX).0.is_ok());
        }
        assert_eq!(serde_json::to_value(&bundle).unwrap(), before);
    }
    #[test]
    fn complete_bound_inputs_observe_cold_storage_hits_and_fresh_source_scans() {
        use crate::definitions::{
            conversion::{InvocationObserver, ProviderId},
            ecore::ParserObservation,
        };
        use std::{cell::Cell, rc::Rc};
        struct Meter {
            bytes: Cell<usize>,
            calls: Cell<usize>,
            limit: usize,
            stop: usize,
        }
        impl Meter {
            fn step(&self) -> Result<(), String> {
                let call = self.calls.get();
                self.calls.set(call + 1);
                if call == self.stop {
                    Err("bound input cancelled".into())
                } else {
                    Ok(())
                }
            }
        }
        impl InvocationObserver for Meter {
            fn before_invoke(&self, _: &ProviderId) -> Result<(), String> {
                Err("no providers".into())
            }
            fn work(&self, _: usize) -> Result<(), String> {
                self.step()
            }
            fn retain_storage(&self, bytes: usize) -> Result<(), String> {
                self.step()?;
                let next = self
                    .bytes
                    .get()
                    .checked_add(bytes)
                    .filter(|next| *next <= self.limit)
                    .ok_or("bound storage denied")?;
                self.bytes.set(next);
                Ok(())
            }
        }
        let (bundle, raw, _) = shared_signature_fixture();
        let before = serde_json::to_value(&bundle).unwrap();
        let resolver = prepared_signature_resolver(&bundle);
        let context = InvocationContext::default();
        let observation = ParserObservation::new(&context);
        let first = resolver
            .runtime_members_for_materialization_observed(&raw, &observation)
            .unwrap();
        let second = resolver
            .runtime_members_for_materialization_observed(&raw, &observation)
            .unwrap();
        assert!(!first.reference_bounds[0].bounds.is_empty());
        for (left, right) in first.reference_bounds.iter().zip(&second.reference_bounds) {
            for (left, right) in left.bounds.iter().zip(&right.bounds) {
                let (RetainedMemberValue::Shared(left), RetainedMemberValue::Shared(right)) =
                    (left, right)
                else {
                    panic!("bound input was copied");
                };
                assert!(Rc::ptr_eq(left, right));
            }
        }
        let input = first.reference_bounds[0].bounds[0].value().clone();
        let span = first.declaration_span.as_ref().clone();
        let run = |limit, stop| {
            let resolver = prepared_signature_resolver(&bundle);
            let meter = Rc::new(Meter {
                bytes: Cell::new(0),
                calls: Cell::new(0),
                limit,
                stop,
            });
            let context = InvocationContext::new(meter.clone());
            let observation = ParserObservation::new(&context);
            let result =
                observation.finish(resolver.shared_bound_input(&input, &span, &observation));
            let entries = resolver
                .prepared_erased_signatures
                .get()
                .unwrap()
                .borrow()
                .bound_inputs
                .len();
            if let Ok(Some(value)) = &result {
                assert_eq!(value.as_ref(), &input);
                assert_eq!(
                    serde_json::to_value(value.as_ref()).unwrap(),
                    serde_json::to_value(&input).unwrap()
                );
            }
            (result, meter.bytes.get(), meter.calls.get(), entries)
        };
        let (complete, bytes, calls, entries) = run(usize::MAX, usize::MAX);
        assert!(complete.unwrap().is_some());
        assert_eq!(entries, 1);
        assert!(run(bytes, usize::MAX).0.unwrap().is_some());
        assert!(run(bytes - 1, usize::MAX).0.is_err());
        for stop in 0..calls {
            let denied = run(usize::MAX, stop);
            assert!(denied.0.is_err());
            assert_eq!(denied.3, 0);
            assert!(run(bytes, usize::MAX).0.unwrap().is_some());
        }
        let meter = Rc::new(Meter {
            bytes: Cell::new(0),
            calls: Cell::new(0),
            limit: 0,
            stop: 0,
        });
        let denied_context = InvocationContext::new(meter);
        assert!(
            resolver
                .shared_bound_input(&input, &span, &ParserObservation::new(&denied_context))
                .is_err()
        );
        let fallback = prepared_signature_resolver(&bundle);
        {
            let mut cache = fallback
                .prepared_erased_signatures
                .get()
                .unwrap()
                .borrow_mut();
            for index in 0..4096 {
                let mut unused = input.clone();
                let Type::Classifier { declaration, .. } = &mut unused else {
                    panic!("expected classifier");
                };
                declaration.path.push(10_000 + index);
                cache.bound_inputs.insert(unused.clone(), Rc::new(unused));
            }
        }
        assert!(
            fallback
                .shared_bound_input(&input, &span, &observation)
                .unwrap()
                .is_none()
        );
        let actual = fallback
            .runtime_members_for_materialization_observed(&raw, &observation)
            .unwrap();
        assert!(
            actual
                .reference_bounds
                .iter()
                .flat_map(|bound| &bound.bounds)
                .all(|bound| matches!(bound, RetainedMemberValue::Owned(_)))
        );
        let mut expected = Resolver::new(&bundle)
            .unwrap()
            .runtime_members(&raw)
            .unwrap();
        expected.source_members.clear();
        assert_eq!(
            serde_json::to_value(actual).unwrap(),
            serde_json::to_value(expected).unwrap()
        );
        let mut changed = bundle.clone();
        changed.sources[0] = DefinitionSource::new(
            changed.sources[0].id.uri.clone(),
            changed.sources[0].kind,
            changed.sources[0].text.replace(
                "<eBounds eClassifier=\"#//Plain\"/>",
                "<eBounds eClassifier=\"#//Other\"/>",
            ),
        );
        let mut expression = raw.expression().clone();
        let Type::Classifier { declaration, .. } = &mut expression else {
            panic!("expected classifier");
        };
        declaration.source = changed.sources[0].id.clone();
        let receiver = RuntimeType::new(expression).unwrap();
        let changed_resolver = prepared_signature_resolver(&changed);
        let current =
            changed_resolver.runtime_members_for_materialization_observed(&receiver, &observation);
        let expected = Resolver::new(&changed).unwrap().runtime_members(&receiver);
        match (current, expected) {
            (Ok(current), Ok(mut expected)) => {
                expected.source_members.clear();
                assert_eq!(
                    serde_json::to_value(&current).unwrap(),
                    serde_json::to_value(expected).unwrap()
                );
                assert_ne!(
                    serde_json::to_value(&current.reference_bounds).unwrap(),
                    serde_json::to_value(&first.reference_bounds).unwrap()
                );
            }
            (Err(current), Err(expected)) => assert_eq!(current.to_string(), expected.to_string()),
            _ => panic!("private bound scan differed from fresh source checks"),
        }
        assert_eq!(serde_json::to_value(&bundle).unwrap(), before);
    }
    fn projection_view_fixture() -> (DefinitionBundle, RuntimeType, RuntimeType) {
        let (mut bundle, raw, concrete) = shared_signature_fixture();
        let text = bundle.sources[0].text.replace("<eOperations name=\"echo\"", "<eStructuralFeatures x:type=\"e:EReference\" name=\"other\"><eGenericType eTypeParameter=\"#//G/T\"/></eStructuralFeatures><eOperations name=\"echo\"");
        bundle.sources[0] = DefinitionSource::new(
            bundle.sources[0].id.uri.clone(),
            bundle.sources[0].kind,
            text,
        );
        let current = |receiver: RuntimeType| {
            let mut expression = receiver.expression().clone();
            let Type::Classifier { declaration, .. } = &mut expression else {
                panic!("expected classifier");
            };
            declaration.source = bundle.sources[0].id.clone();
            RuntimeType::new(expression).unwrap()
        };
        let mut raw_expression = current(raw).expression().clone();
        let Type::Classifier { declaration, .. } = &mut raw_expression else {
            panic!("expected classifier");
        };
        declaration.path = vec![1];
        let raw = RuntimeType::new(raw_expression).unwrap();
        let concrete = current(concrete);
        (bundle, raw, concrete)
    }

    #[test]
    fn complete_private_source_and_domain_views_preserve_all_fields_and_allocations() {
        use std::rc::Rc;
        let (bundle, raw, concrete) = projection_view_fixture();
        let before = serde_json::to_value(&bundle).unwrap();
        let resolver = prepared_signature_resolver(&bundle);
        let context = InvocationContext::default();
        let observation = crate::definitions::ecore::ParserObservation::new(&context);
        for receiver in [&raw, &concrete] {
            let private = resolver
                .runtime_members_for_materialization_observed(receiver, &observation)
                .unwrap();
            let mut public = Resolver::new(&bundle)
                .unwrap()
                .runtime_members(receiver)
                .unwrap();
            public.source_members.clear();
            assert_eq!(private, public);
            assert_eq!(
                serde_json::to_value(&private).unwrap(),
                serde_json::to_value(&public).unwrap()
            );
            if !private.feature_domains.is_empty() {
                let PreparedSourceDomains::Views(rows) = &private.feature_domains else {
                    panic!("private domain metadata was owned");
                };
                assert!(rows.len() >= 2);
                for row in rows {
                    let source = &row.projection.source;
                    let node = resolver.node(source.declaration.value()).unwrap();
                    let std::borrow::Cow::Borrowed(span) = &source.span else {
                        panic!("source span was copied");
                    };
                    assert!(std::ptr::eq(*span, &node.span));
                    assert!(matches!(&source.name, std::borrow::Cow::Borrowed(_)));
                    assert!(matches!(
                        &source.source_payload,
                        std::borrow::Cow::Borrowed(_)
                    ));
                    let document = resolver
                        .resources(source.declaration.value())
                        .document(&source.declaration.source)
                        .unwrap();
                    assert!(std::ptr::eq(
                        source.source_payload.as_ref(),
                        &document.source.text[node.span.start..node.span.end]
                    ));
                    assert_eq!(
                        serde_json::to_value(row.source()).unwrap(),
                        serde_json::to_value(
                            public
                                .feature_domains
                                .iter()
                                .find(|other| other.projection.source.declaration
                                    == *source.declaration.value())
                                .unwrap()
                        )
                        .unwrap()
                    );
                }
                let (RetainedMemberValue::Shared(first), RetainedMemberValue::Shared(second)) =
                    (&rows[0].projection.receiver, &rows[1].projection.receiver)
                else {
                    panic!("receiver input was copied");
                };
                assert!(Rc::ptr_eq(first, second));
                let RetainedMemberValue::Shared(report_receiver) = &private.receiver else {
                    panic!("report receiver was duplicated");
                };
                assert!(Rc::ptr_eq(first, report_receiver));
                let (RetainedMemberValue::Shared(first), RetainedMemberValue::Shared(second)) =
                    (&rows[0].expression, &rows[1].expression)
                else {
                    panic!("domain expression input was copied");
                };
                assert!(Rc::ptr_eq(first, second));
            }
        }
        assert_eq!(serde_json::to_value(&bundle).unwrap(), before);
    }

    #[test]
    fn deferred_domain_publication_observes_full_owned_copies_limits_and_recovery() {
        use crate::definitions::{
            conversion::{InvocationObserver, ProviderId},
            ecore::ParserObservation,
        };
        use std::{cell::Cell, rc::Rc};
        struct Meter {
            bytes: Cell<usize>,
            calls: Cell<usize>,
            limit: usize,
            stop: usize,
        }
        impl Meter {
            fn step(&self) -> Result<(), String> {
                let call = self.calls.get();
                self.calls.set(call + 1);
                if call == self.stop {
                    Err("domain publication cancelled".into())
                } else {
                    Ok(())
                }
            }
        }
        impl InvocationObserver for Meter {
            fn before_invoke(&self, _: &ProviderId) -> Result<(), String> {
                Err("no providers".into())
            }
            fn work(&self, _: usize) -> Result<(), String> {
                self.step()
            }
            fn retain_storage(&self, bytes: usize) -> Result<(), String> {
                self.step()?;
                let next = self
                    .bytes
                    .get()
                    .checked_add(bytes)
                    .filter(|next| *next <= self.limit)
                    .ok_or("domain publication storage denied")?;
                self.bytes.set(next);
                Ok(())
            }
        }
        let (bundle, raw, _) = projection_view_fixture();
        let before = serde_json::to_value(&bundle).unwrap();
        let expected = Resolver::new(&bundle)
            .unwrap()
            .runtime_members(&raw)
            .unwrap()
            .feature_domains;
        assert!(expected.len() >= 2);
        let run = |limit, stop| {
            let resolver = prepared_signature_resolver(&bundle);
            let context = InvocationContext::default();
            let observation = ParserObservation::new(&context);
            let private = resolver
                .runtime_members_for_materialization_observed(&raw, &observation)
                .unwrap();
            assert!(matches!(
                &private.feature_domains,
                PreparedSourceDomains::Views(_)
            ));
            let meter = Rc::new(Meter {
                bytes: Cell::new(0),
                calls: Cell::new(0),
                limit,
                stop,
            });
            let context = InvocationContext::new(meter.clone());
            let observation = ParserObservation::new(&context);
            let result =
                observation.finish(private.feature_domains.into_owned_observed(&observation));
            if let Ok(value) = &result {
                assert_eq!(value, &expected);
                assert_eq!(
                    serde_json::to_value(value).unwrap(),
                    serde_json::to_value(&expected).unwrap()
                );
            }
            (result, meter.bytes.get(), meter.calls.get())
        };
        let (complete, bytes, calls) = run(usize::MAX, usize::MAX);
        complete.unwrap();
        assert!(bytes > 0);
        assert!(run(bytes, usize::MAX).0.is_ok());
        assert!(run(bytes - 1, usize::MAX).0.is_err());
        for stop in 0..calls {
            assert!(run(usize::MAX, stop).0.is_err());
            assert!(run(bytes, usize::MAX).0.is_ok());
        }
        assert_eq!(serde_json::to_value(&bundle).unwrap(), before);
    }

    #[test]
    fn source_domain_views_keep_owned_fallback_and_fresh_invalid_actual_checks() {
        let (bundle, raw, _) = projection_view_fixture();
        let before = serde_json::to_value(&bundle).unwrap();
        let context = InvocationContext::default();
        let observation = crate::definitions::ecore::ParserObservation::new(&context);
        let fallback = prepared_signature_resolver(&bundle);
        fallback
            .prepared_erased_signatures
            .get()
            .unwrap()
            .borrow_mut()
            .identity_entries = 4096;
        let private = fallback
            .runtime_members_for_materialization_observed(&raw, &observation)
            .unwrap();
        let PreparedSourceDomains::Views(rows) = &private.feature_domains else {
            panic!("private source views missing");
        };
        assert!(rows.iter().all(|row| matches!(
            &row.projection.source.declaration,
            RetainedMemberValue::Owned(_)
        ) && matches!(
            &row.projection.source.owner,
            RetainedMemberValue::Owned(_)
        )));
        let mut public = Resolver::new(&bundle)
            .unwrap()
            .runtime_members(&raw)
            .unwrap();
        public.source_members.clear();
        assert_eq!(
            serde_json::to_value(private).unwrap(),
            serde_json::to_value(public).unwrap()
        );
        let mut invalid = raw.expression().clone();
        let Type::Classifier {
            arguments,
            raw: is_raw,
            ..
        } = &mut invalid
        else {
            panic!("expected classifier");
        };
        *is_raw = false;
        arguments.push(Type::Classifier {
            declaration: DeclarationId {
                source: bundle.sources[0].id.clone(),
                path: vec![3],
            },
            arguments: vec![],
            raw: false,
        });
        let invalid = RuntimeType::new(invalid).unwrap();
        let resolver = prepared_signature_resolver(&bundle);
        let private = resolver
            .runtime_members_for_materialization_observed(&invalid, &observation)
            .unwrap_err();
        let public = Resolver::new(&bundle)
            .unwrap()
            .runtime_members(&invalid)
            .unwrap_err();
        assert_eq!(private.to_string(), public.to_string());
        assert!(
            resolver
                .runtime_members_for_materialization_observed(&raw, &observation)
                .is_ok()
        );
        assert_eq!(serde_json::to_value(&bundle).unwrap(), before);
    }
}

#[cfg(test)]
#[path = "generic_runtime_members_capture_tests.rs"]
mod runtime_members_capture_tests;

#[path = "generic_runtime_members_preparation.rs"]
mod runtime_members_preparation;
pub(crate) use runtime_members_preparation::RuntimeMembersPreparation;

#[cfg(test)]
#[path = "generic_source_members_capture_tests.rs"]
mod source_members_capture_tests;

#[path = "generic_source_members_preparation.rs"]
mod source_members_preparation;
use source_members_preparation::SourceMembersPreparation;

#[cfg(test)]
#[path = "generic_legacy_owner_controls.rs"]
mod legacy_owner_controls;
