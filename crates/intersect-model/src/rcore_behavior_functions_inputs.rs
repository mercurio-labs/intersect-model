//! Sealed input families for the SAME decoder/evaluator. Ordinary holders keep
//! their concrete owning domain map; explicit borrowed inputs never JSON-import.
use super::{machine::State, *};
use crate::{
    core::storage::{StoredFeaturePreparation, StoredFeatureProjection, StoredFeatureSelection},
    definitions::{
        conversion::InvocationContext, generic_types::RuntimeType, resources::DeclarationId,
    },
};
use intersect_model::definitions::rcore_behavior::ir_v2::{self, TypeId};
use std::{collections::BTreeMap, marker::PhantomData};

pub(super) type OwnedDomains =
    BTreeMap<(String, TypeId), crate::model_reference::RuntimeReferenceDomain>;
pub(super) struct NoBorrowedModelInputs;
pub(super) struct ScopedModelInputs<'input>(PhantomData<&'input ()>);

pub(super) trait InputFamily: Sized {
    const NESTED_MODELS: bool = true;
    const BORROWED: bool = false;
    type Domains;
    type Pending;
    fn source_scope(_: &Self::Domains) -> bool {
        false
    }
    fn direct_ownership(
        _: &State<'_, '_, '_, Self>,
        _: &str,
        _: ir_v2::OwnershipOp,
        _: &RuntimeType,
        _: usize,
        _: &SourceSpan,
    ) -> Result<Option<Option<serde_json::Value>>> {
        Ok(None)
    }
    fn direct_stored(
        _: &State<'_, '_, '_, Self>,
        _: super::values::Id,
        _: &DeclarationId,
        _: Option<usize>,
        _: &SourceSpan,
    ) -> Result<Option<Option<serde_json::Value>>> {
        Ok(None)
    }
    fn advance_stored_target(_: &mut State<'_, '_, '_, Self>, _: &SourceSpan) -> Result<bool> {
        Ok(false)
    }
    fn begin_stored_target(
        _: &mut State<'_, '_, '_, Self>,
        _: TypeId,
        _: &str,
        _: &str,
        _: &SourceSpan,
    ) -> Result<bool> {
        Ok(false)
    }
    fn observation<'a>(
        domains: &Self::Domains,
        receipts: &std::rc::Rc<super::providers::Receipts>,
        control: Option<&'a dyn ExecutionControl>,
    ) -> std::result::Result<InvocationContext<'a>, String>
    where
        Self: 'a;
    #[cfg(test)]
    fn diagnostic_phase(_: &Self::Pending) -> Option<&'static str> {
        None
    }
    fn owned(domains: &Self::Domains) -> std::result::Result<&OwnedDomains, &'static str>;
    fn owned_mut(
        domains: &mut Self::Domains,
    ) -> std::result::Result<&mut OwnedDomains, &'static str>;
    fn begin_model(
        state: &mut State<'_, '_, '_, Self>,
        ty: TypeId,
        payload: &serde_json::Value,
        span: Option<&SourceSpan>,
    ) -> Result<Option<Self::Pending>>;
    fn advance_model(
        state: &mut State<'_, '_, '_, Self>,
        pending: &mut Self::Pending,
        span: Option<&SourceSpan>,
    ) -> Result<bool>;
    fn nominal_model(
        state: &mut State<'_, '_, '_, Self>,
        ty: TypeId,
        payload: &serde_json::Value,
        span: Option<&SourceSpan>,
    ) -> Result<()>;
    fn begin_stored(
        state: &State<'_, '_, '_, Self>,
        id: super::values::Id,
        member: &DeclarationId,
        span: &SourceSpan,
    ) -> Result<StoredFeaturePreparation>;
    fn advance_stored(
        state: &State<'_, '_, '_, Self>,
        id: super::values::Id,
        pending: &mut StoredFeaturePreparation,
        span: &SourceSpan,
    ) -> Result<bool>;
    fn stored<'s>(
        state: &'s State<'_, '_, '_, Self>,
        id: super::values::Id,
        selected: &StoredFeatureSelection,
        span: &SourceSpan,
    ) -> Result<StoredFeatureProjection<'s>>;
}

impl InputFamily for NoBorrowedModelInputs {
    type Domains = OwnedDomains;
    type Pending = super::decoding::PendingDomain;
    fn observation<'a>(
        _: &OwnedDomains,
        receipts: &std::rc::Rc<super::providers::Receipts>,
        control: Option<&'a dyn ExecutionControl>,
    ) -> std::result::Result<InvocationContext<'a>, String>
    where
        Self: 'a,
    {
        Ok(receipts.scoped(control))
    }
    #[cfg(test)]
    fn diagnostic_phase(pending: &Self::Pending) -> Option<&'static str> {
        pending
            .admission
            .as_ref()
            .map(|admission| admission.diagnostic_phase())
    }
    fn owned(d: &OwnedDomains) -> std::result::Result<&OwnedDomains, &'static str> {
        Ok(d)
    }
    fn owned_mut(d: &mut OwnedDomains) -> std::result::Result<&mut OwnedDomains, &'static str> {
        Ok(d)
    }
    fn begin_model(
        state: &mut State<'_, '_, '_, Self>,
        ty: TypeId,
        payload: &serde_json::Value,
        span: Option<&SourceSpan>,
    ) -> Result<Option<Self::Pending>> {
        let (digest, _) = crate::model_reference::reference_identity(payload)
            .map_err(|e| state.error(ExecutionErrorKind::Nominal, e, span))?;
        let (cursor, admission) = if state.data.contexts.contains_key(digest) {
            (state.prepare_reference_domain(ty, digest, span)?, None)
        } else {
            (
                None,
                Some(state.prepare_reference_admission(ty, digest, span)?),
            )
        };
        if cursor.is_none() && admission.is_none() {
            return Ok(None);
        }
        state.retained_storage(128usize.saturating_add(digest.len()), span)?;
        Ok(Some(super::decoding::PendingDomain {
            admission,
            cursor,
            ty,
            digest: digest.into(),
        }))
    }
    fn advance_model(
        state: &mut State<'_, '_, '_, Self>,
        pending: &mut Self::Pending,
        span: Option<&SourceSpan>,
    ) -> Result<bool> {
        if pending.admission.is_some() {
            state.advance_reference_admission(
                &mut pending.admission,
                pending.ty,
                &pending.digest,
                span,
            )
        } else {
            state.advance_reference_domain(&mut pending.cursor, pending.ty, &pending.digest, span)
        }
    }
    fn nominal_model(
        state: &mut State<'_, '_, '_, Self>,
        ty: TypeId,
        payload: &serde_json::Value,
        span: Option<&SourceSpan>,
    ) -> Result<()> {
        let digest = payload
            .get("context")
            .and_then(|v| v.as_str())
            .ok_or_else(|| {
                state.error(
                    ExecutionErrorKind::Nominal,
                    "Missing model context identity",
                    span,
                )
            })?;
        state.ensure_reference_domain(ty, digest, span)?;
        state
            .data
            .domains
            .get(&(digest.to_owned(), ty))
            .ok_or_else(|| state.invalid("Missing checked reference domain", span))?
            .resolve(payload)
            .map_err(|e| state.provider_error(e, span))?;
        Ok(())
    }
    fn begin_stored(
        state: &State<'_, '_, '_, Self>,
        id: super::values::Id,
        member: &DeclarationId,
        span: &SourceSpan,
    ) -> Result<StoredFeaturePreparation> {
        state.begin_owned_stored_projection(id, member, span)
    }
    fn advance_stored(
        state: &State<'_, '_, '_, Self>,
        id: super::values::Id,
        pending: &mut StoredFeaturePreparation,
        span: &SourceSpan,
    ) -> Result<bool> {
        state.advance_owned_stored_projection(id, pending, span)
    }
    fn stored<'s>(
        state: &'s State<'_, '_, '_, Self>,
        id: super::values::Id,
        selected: &StoredFeatureSelection,
        span: &SourceSpan,
    ) -> Result<StoredFeatureProjection<'s>> {
        state.selected_owned_stored_projection(id, selected, span)
    }
}

// Private in-process authority. Only the exact invocation binder implements it;
// there is no public trait implementation, raw graph, header or model fallback.
pub(crate) trait BorrowedInputAuthority {
    fn source_scope(&self) -> bool {
        false
    }
    fn direct_ownership(
        &self,
        _: &str,
        _: ir_v2::OwnershipOp,
        _: &RuntimeType,
        _: usize,
        _: &InvocationContext<'_>,
    ) -> std::result::Result<Option<Option<serde_json::Value>>, String> {
        Ok(None)
    }
    fn direct_stored(
        &self,
        _: &str,
        _: &DeclarationId,
        _: Option<usize>,
        _: &InvocationContext<'_>,
    ) -> std::result::Result<Option<Option<serde_json::Value>>, String> {
        Ok(None)
    }
    fn original_observation(&self) -> InvocationContext<'_>;
    fn context_key(&self) -> &str;
    fn check(&self, observation: &InvocationContext<'_>) -> std::result::Result<(), String>;
    fn object_count(&self) -> usize;
    fn begin_target<'a>(
        &'a self,
        target: &RuntimeType,
        observation: &InvocationContext<'_>,
    ) -> std::result::Result<Box<dyn BorrowedTarget<'a> + 'a>, String>;
}
pub(crate) trait BorrowedTarget<'input> {
    fn advance(&mut self, observation: &InvocationContext<'_>)
    -> std::result::Result<bool, String>;
    fn finish(
        self: Box<Self>,
        observation: &InvocationContext<'_>,
    ) -> std::result::Result<Box<dyn BorrowedDomain + 'input>, String>;
}
pub(crate) trait BorrowedDomain {
    fn resolve(
        &self,
        object: &str,
        observation: &InvocationContext<'_>,
    ) -> std::result::Result<(), String>;
    fn begin_stored(
        &self,
        object: &str,
        member: &DeclarationId,
        observation: &InvocationContext<'_>,
    ) -> std::result::Result<StoredFeaturePreparation, String>;
    fn advance_stored(
        &self,
        pending: &mut StoredFeaturePreparation,
        observation: &InvocationContext<'_>,
    ) -> std::result::Result<bool, String>;
    fn stored<'a>(
        &'a self,
        object: &'a str,
        selected: &StoredFeatureSelection,
        observation: &InvocationContext<'_>,
    ) -> std::result::Result<StoredFeatureProjection<'a>, String>;
}
pub(super) struct BorrowedDomains<'input> {
    pub inputs: &'input dyn BorrowedInputAuthority,
    original: InvocationContext<'input>,
    pub completed: BTreeMap<TypeId, Box<dyn BorrowedDomain + 'input>>,
    stored_target: Option<BorrowedPending<'input>>,
}
pub(super) struct BorrowedPending<'input> {
    cursor: Option<Box<dyn BorrowedTarget<'input> + 'input>>,
    ty: TypeId,
    digest: String,
}
impl<'input> BorrowedDomains<'input> {
    pub fn new(inputs: &'input dyn BorrowedInputAuthority) -> Self {
        Self {
            inputs,
            original: inputs.original_observation(),
            completed: BTreeMap::new(),
            stored_target: None,
        }
    }
}
impl<'input> InputFamily for ScopedModelInputs<'input> {
    const NESTED_MODELS: bool = false;
    const BORROWED: bool = true;
    type Domains = BorrowedDomains<'input>;
    type Pending = BorrowedPending<'input>;
    fn source_scope(domains: &Self::Domains) -> bool {
        domains.inputs.source_scope()
    }
    fn direct_ownership(
        state: &State<'_, '_, '_, Self>,
        object: &str,
        operation: ir_v2::OwnershipOp,
        selected: &RuntimeType,
        cursor: usize,
        span: &SourceSpan,
    ) -> Result<Option<Option<serde_json::Value>>> {
        let observation = state.invocation_observation(Some(span))?;
        state
            .data
            .domains
            .inputs
            .direct_ownership(object, operation, selected, cursor, &observation)
            .map_err(|e| state.provider_error(e, Some(span)))
    }
    fn direct_stored(
        state: &State<'_, '_, '_, Self>,
        receiver: super::values::Id,
        member: &DeclarationId,
        cursor: Option<usize>,
        span: &SourceSpan,
    ) -> Result<Option<Option<serde_json::Value>>> {
        if !state.data.domains.inputs.source_scope() {
            return Ok(None);
        }
        let (_, object) = borrowed_stored(state, receiver, span)?;
        let observation = state.invocation_observation(Some(span))?;
        state
            .data
            .domains
            .inputs
            .direct_stored(object, member, cursor, &observation)
            .map_err(|e| state.provider_error(e, Some(span)))
    }
    fn advance_stored_target(
        state: &mut State<'_, '_, '_, Self>,
        span: &SourceSpan,
    ) -> Result<bool> {
        let Some(mut pending) = state.data.domains.stored_target.take() else {
            return Ok(false);
        };
        if !Self::advance_model(state, &mut pending, Some(span))? {
            state.data.domains.stored_target = Some(pending);
        }
        Ok(true)
    }
    fn begin_stored_target(
        state: &mut State<'_, '_, '_, Self>,
        ty: TypeId,
        digest: &str,
        object: &str,
        span: &SourceSpan,
    ) -> Result<bool> {
        let payload =
            serde_json::json!({"kind":"model_reference", "context":digest,"element":object});
        let pending = Self::begin_model(state, ty, &payload, Some(span))?;
        let preparing = pending.is_some();
        state.data.domains.stored_target = pending;
        Ok(preparing)
    }
    fn observation<'a>(
        domains: &Self::Domains,
        receipts: &std::rc::Rc<super::providers::Receipts>,
        control: Option<&'a dyn ExecutionControl>,
    ) -> std::result::Result<InvocationContext<'a>, String>
    where
        Self: 'a,
    {
        receipts.scoped_borrowed(domains.original.clone(), control)
    }
    fn owned(_: &Self::Domains) -> std::result::Result<&OwnedDomains, &'static str> {
        Err("Unsupported owning-domain operation on borrowed inputs")
    }
    fn owned_mut(_: &mut Self::Domains) -> std::result::Result<&mut OwnedDomains, &'static str> {
        Err("Unsupported owning-domain operation on borrowed inputs")
    }
    fn begin_model(
        state: &mut State<'_, '_, '_, Self>,
        ty: TypeId,
        payload: &serde_json::Value,
        span: Option<&SourceSpan>,
    ) -> Result<Option<Self::Pending>> {
        let (digest, _) = crate::model_reference::reference_identity(payload)
            .map_err(|e| state.error(ExecutionErrorKind::Nominal, e, span))?;
        let inputs = state.data.domains.inputs;
        let observation = state.invocation_observation(None)?;
        inputs
            .check(&observation)
            .map_err(|e| state.provider_error(e, span))?;
        if digest != inputs.context_key() {
            return Err(state.invalid(
                "Reference belongs to another borrowed operation context",
                span,
            ));
        }
        if state.data.domains.completed.contains_key(&ty) {
            return Ok(None);
        }
        if state.data.domains.completed.len() >= state.limits.max_reference_domains {
            return Err(state.resource("Reference domain count limit exceeded", span));
        }
        let ir_v2::TypeNode::Model(target) = state.type_node(ty)? else {
            return Err(state.invalid("Reference domain requires model type", span));
        };
        crate::definitions::retained_storage::reserve_serialized(target, &observation)
            .map_err(|e| state.provider_error(e, span))?;
        let target = target.clone();
        if state.data.contexts.is_empty() && state.limits.max_model_contexts == 0 {
            return Err(state.resource("Model context count limit exceeded", span));
        }
        observation
            .work(1)
            .map_err(|e| state.provider_error(e, span))?;
        let count = inputs.object_count();
        state.reserve_reference_membership(count, span)?;
        state.data.consumed.model_elements = state
            .data
            .consumed
            .model_elements
            .checked_add(count)
            .filter(|n| *n <= state.limits.max_model_elements)
            .ok_or_else(|| state.resource("Model element validation limit exceeded", span))?;
        // Actual pending cursor/identity storage is new, never credited to old
        // domain preparation or hidden behind a reset allowance.
        state.retained_storage(192usize.saturating_add(digest.len()), span)?;
        let cursor = inputs
            .begin_target(&target, &observation)
            .map_err(|e| state.provider_error(e, span))?;
        state.check_provider_status(span)?;
        Ok(Some(BorrowedPending {
            cursor: Some(cursor),
            ty,
            digest: digest.into(),
        }))
    }
    fn advance_model(
        state: &mut State<'_, '_, '_, Self>,
        pending: &mut Self::Pending,
        span: Option<&SourceSpan>,
    ) -> Result<bool> {
        state.step_at(span)?;
        let observation = state.invocation_observation(None)?;
        state
            .data
            .domains
            .inputs
            .check(&observation)
            .map_err(|e| state.provider_error(e, span))?;
        let cursor = pending
            .cursor
            .as_mut()
            .ok_or_else(|| state.invalid("Missing borrowed domain preparation", span))?;
        let done = cursor
            .advance(&observation)
            .map_err(|e| state.provider_error(e, span))?;
        state.check_provider_status(span)?;
        if done {
            let domain = pending
                .cursor
                .take()
                .ok_or_else(|| state.invalid("Missing borrowed domain preparation", span))?
                .finish(&observation)
                .map_err(|e| state.provider_error(e, span))?;
            observation
                .work(1)
                .map_err(|e| state.provider_error(e, span))?;
            state.retained_storage(64usize.saturating_add(pending.digest.len()), span)?;
            state.data.domains.completed.insert(pending.ty, domain);
            state.data.consumed.reference_domains = state.data.domains.completed.len();
            if state.data.contexts.is_empty() {
                state.data.observation.validation(&pending.digest, span);
                state
                    .data
                    .contexts
                    .insert(pending.digest.clone(), pending.ty);
                state.data.consumed.model_contexts = 1;
            }
        }
        Ok(done)
    }
    fn nominal_model(
        state: &mut State<'_, '_, '_, Self>,
        ty: TypeId,
        payload: &serde_json::Value,
        span: Option<&SourceSpan>,
    ) -> Result<()> {
        let (digest, object) = crate::model_reference::reference_identity(payload)
            .map_err(|e| state.provider_error(e, span))?;
        let observation = state.invocation_observation(None)?;
        state
            .data
            .domains
            .inputs
            .check(&observation)
            .map_err(|e| state.provider_error(e, span))?;
        if digest != state.data.domains.inputs.context_key() {
            return Err(state.invalid(
                "Reference belongs to another borrowed operation context",
                span,
            ));
        }
        // A model value can reach a type whose domain no read has prepared yet,
        // e.g. a Feature passed where a Namespace is expected. Prepare it here
        // through the same admission, limits and charges as a read would.
        if !state.data.domains.completed.contains_key(&ty)
            && let Some(mut pending) = Self::begin_model(state, ty, payload, span)?
        {
            while !Self::advance_model(state, &mut pending, span)? {}
        }
        state
            .data
            .domains
            .completed
            .get(&ty)
            .ok_or_else(|| state.invalid("Missing checked reference domain", span))?
            .resolve(object, &observation)
            .map_err(|e| state.provider_error(e, span))?;
        state.check_provider_status(span)
    }
    fn begin_stored(
        state: &State<'_, '_, '_, Self>,
        id: super::values::Id,
        member: &DeclarationId,
        span: &SourceSpan,
    ) -> Result<StoredFeaturePreparation> {
        let (domain, object) = borrowed_stored(state, id, span)?;
        let observation = state.invocation_observation(None)?;
        domain
            .begin_stored(object, member, &observation)
            .map_err(|e| state.provider_error(e, Some(span)))
    }
    fn advance_stored(
        state: &State<'_, '_, '_, Self>,
        id: super::values::Id,
        pending: &mut StoredFeaturePreparation,
        span: &SourceSpan,
    ) -> Result<bool> {
        let (domain, _) = borrowed_stored(state, id, span)?;
        let observation = state.invocation_observation(None)?;
        domain
            .advance_stored(pending, &observation)
            .map_err(|e| state.provider_error(e, Some(span)))
    }
    fn stored<'s>(
        state: &'s State<'_, '_, '_, Self>,
        id: super::values::Id,
        selected: &StoredFeatureSelection,
        span: &SourceSpan,
    ) -> Result<StoredFeatureProjection<'s>> {
        let (domain, object) = borrowed_stored(state, id, span)?;
        let observation = state.invocation_observation(None)?;
        domain
            .stored(object, selected, &observation)
            .map_err(|e| state.provider_error(e, Some(span)))
    }
}
fn borrowed_stored<'s>(
    state: &'s State<'_, '_, '_, ScopedModelInputs<'_>>,
    id: super::values::Id,
    span: &SourceSpan,
) -> Result<(&'s dyn BorrowedDomain, &'s str)> {
    let node = state.node(id)?;
    let super::values::Data::Model(payload) = node.data else {
        return Err(state.invalid("Stored read requires model reference", Some(span)));
    };
    let (digest, object) = crate::model_reference::reference_identity(payload)
        .map_err(|e| state.invalid(&e, Some(span)))?;
    let observation = state.invocation_observation(None)?;
    state
        .data
        .domains
        .inputs
        .check(&observation)
        .map_err(|e| state.provider_error(e, Some(span)))?;
    if digest != state.data.domains.inputs.context_key() {
        return Err(state.error(
            ExecutionErrorKind::Unsupported,
            "Reference belongs to another borrowed operation context",
            Some(span),
        ));
    }
    observation
        .work(1)
        .map_err(|e| state.provider_error(e, Some(span)))?;
    let domain = state
        .data
        .domains
        .completed
        .get(&node.ty)
        .ok_or_else(|| state.invalid("Missing admitted reference domain", Some(span)))?;
    Ok((domain.as_ref(), object))
}
