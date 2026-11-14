use super::machine::State;
use super::*;
use intersect_model::definitions::{
    conversion::{AppliedConvertedValue, InvocationContext},
    datatypes::Binding,
    rcore_behavior::{
        ir_v2::{self, TypeNode},
        model_types::ModelTypes,
    },
};
use serde::de::DeserializeOwned;

fn exact<T: DeserializeOwned + Serialize>(
    payload: &serde_json::Value,
    observation: &intersect_model::definitions::conversion::InvocationContext<'_>,
) -> std::result::Result<T, String> {
    let parsed = T::deserialize(payload).map_err(|e| e.to_string())?;
    if !intersect_model::definitions::json_exact::matches_with_work(&parsed, payload, &|units| {
        observation.work(units)
    })? {
        return Err("Nominal value requires exact canonical fields".into());
    }
    Ok(parsed)
}
struct Counter {
    count: usize,
    limit: usize,
}
impl std::io::Write for Counter {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.count = self
            .count
            .checked_add(bytes.len())
            .filter(|n| *n <= self.limit)
            .ok_or_else(|| std::io::Error::other("Context byte limit exceeded"))?;
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
impl<I: super::inputs::InputFamily> State<'_, '_, '_, I> {
    pub(super) fn owned_domains(&self) -> Result<&super::inputs::OwnedDomains> {
        I::owned(&self.data.domains).map_err(|e| self.invalid(e, None))
    }
    pub(super) fn publish_owned_domain(
        &mut self,
        digest: String,
        ty: ir_v2::TypeId,
        domain: crate::model_reference::RuntimeReferenceDomain,
    ) -> Result<()> {
        let result = I::owned_mut(&mut self.data.domains).map(|domains| {
            domains.insert((digest, ty), domain);
        });
        result.map_err(|error| self.invalid(error, None))
    }
    pub(super) fn begin_stored_projection(
        &self,
        id: super::values::Id,
        member: &intersect_model::definitions::resources::DeclarationId,
        span: &SourceSpan,
    ) -> Result<intersect_model::core::storage::StoredFeaturePreparation> {
        I::begin_stored(self, id, member, span)
    }
    pub(super) fn advance_stored_projection(
        &self,
        id: super::values::Id,
        pending: &mut intersect_model::core::storage::StoredFeaturePreparation,
        span: &SourceSpan,
    ) -> Result<bool> {
        I::advance_stored(self, id, pending, span)
    }
    pub(super) fn selected_stored_projection(
        &self,
        id: super::values::Id,
        selected: &intersect_model::core::storage::StoredFeatureSelection,
        span: &SourceSpan,
    ) -> Result<intersect_model::core::storage::StoredFeatureProjection<'_>> {
        I::stored(self, id, selected, span)
    }

    pub(super) fn admit_stored_receiver(
        &mut self,
        id: super::values::Id,
        span: &SourceSpan,
    ) -> Result<serde_json::Value> {
        self.checkpoint(Some(span))?;
        self.transfer(id, Some(span))?;
        let receiver_ty = self.node(id)?.ty;
        let super::values::Data::Model(payload) = self.node(id)?.data.clone() else {
            return Err(self.invalid("Stored read requires model reference", Some(span)));
        };
        self.nominal(receiver_ty, &payload, Some(span))?;
        Ok(payload)
    }
    fn stored_read_domain(
        &self,
        id: super::values::Id,
        span: &SourceSpan,
    ) -> Result<(
        &crate::model_reference::RuntimeReferenceDomain,
        &serde_json::Value,
    )> {
        let node = self.node(id)?;
        let super::values::Data::Model(payload) = node.data else {
            return Err(self.invalid("Stored read requires model reference", Some(span)));
        };
        let digest = payload
            .get("context")
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| self.invalid("Missing reference context", Some(span)))?;
        let domain = self
            .owned_domains()?
            .get(&(digest.to_owned(), node.ty))
            .ok_or_else(|| self.invalid("Missing admitted reference domain", Some(span)))?;
        Ok((domain, payload))
    }
    pub(super) fn begin_owned_stored_projection(
        &self,
        id: super::values::Id,
        declaration: &intersect_model::definitions::resources::DeclarationId,
        span: &SourceSpan,
    ) -> Result<intersect_model::core::storage::StoredFeaturePreparation> {
        let (domain, payload) = self.stored_read_domain(id, span)?;
        let TypeNode::Model(_) = self.type_node(self.node(id)?.ty)? else {
            return Err(self.invalid("Stored read requires nominal model type", Some(span)));
        };
        let receiver = domain.target_key();
        let observation = self.invocation_observation(None)?;
        intersect_model::definitions::retained_storage::reserve_serialized(
            declaration,
            &observation,
        )
        .map_err(|e| self.provider_error(e, Some(span)))?;
        let identity = intersect_model::definitions::schema::MemberIdentity::Source {
            declaration: declaration.clone(),
        };
        let object = domain
            .resolve(payload)
            .map_err(|e| self.invalid(&e, Some(span)))?;
        intersect_model::core::storage::StoredFeaturePreparation::new_observed(
            receiver,
            &identity,
            &object.id,
            &observation,
        )
        .map_err(|e| match self.check_provider_status(Some(span)) {
            Err(error) => error,
            Ok(()) => self.invalid(&e, Some(span)),
        })
    }
    pub(super) fn advance_owned_stored_projection(
        &self,
        id: super::values::Id,
        pending: &mut intersect_model::core::storage::StoredFeaturePreparation,
        span: &SourceSpan,
    ) -> Result<bool> {
        let (domain, _) = self.stored_read_domain(id, span)?;
        let observation = self.invocation_observation(None)?;
        let done = pending
            .advance_observed(domain.read(), domain.schema(), &observation)
            .map_err(|e| match self.check_provider_status(Some(span)) {
                Err(error) => error,
                Ok(()) => self.invalid(&e, Some(span)),
            })?;
        self.check_provider_status(Some(span))?;
        Ok(done)
    }
    pub(super) fn selected_owned_stored_projection(
        &self,
        id: super::values::Id,
        selected: &intersect_model::core::storage::StoredFeatureSelection,
        span: &SourceSpan,
    ) -> Result<intersect_model::core::storage::StoredFeatureProjection<'_>> {
        let (domain, payload) = self.stored_read_domain(id, span)?;
        let object = domain
            .resolve(payload)
            .map_err(|e| self.invalid(&e, Some(span)))?;
        let observation = self.invocation_observation(None)?;
        selected
            .projection_observed(domain.read(), domain.schema(), &object.id, &observation)
            .map_err(|e| match self.check_provider_status(Some(span)) {
                Err(error) => error,
                Ok(()) => self.invalid(&e, Some(span)),
            })
    }
    pub(super) fn stored_singular_value(
        &mut self,
        raw: serde_json::Value,
        digest: &str,
        ty: ir_v2::TypeId,
        span: &SourceSpan,
    ) -> Result<super::values::Id> {
        use super::values::Data;
        let (value_ty, optional) = match self.type_node(ty)? {
            TypeNode::Option(inner) => (*inner, true),
            _ => (ty, false),
        };
        if raw.is_null() && optional {
            return self.alloc(ty, Data::None, Some(span));
        }
        let value = self.stored_leaf(raw, digest, value_ty, span)?;
        if optional {
            self.alloc(ty, Data::Some(value), Some(span))
        } else {
            Ok(value)
        }
    }
    pub(super) fn stored_leaf(
        &mut self,
        raw: serde_json::Value,
        digest: &str,
        value_ty: ir_v2::TypeId,
        span: &SourceSpan,
    ) -> Result<super::values::Id> {
        use super::values::Data;
        let data = match self.type_node(value_ty)?.clone() {
            TypeNode::Scalar(scalar) => {
                let value = intersect_model::definitions::rcore_behavior::ir::Value {
                    ty: intersect_model::definitions::rcore_behavior::ir::ValueType::Scalar(scalar),
                    payload: raw,
                };
                intersect_model::definitions::rcore_behavior::ir::validate_scalar_value(
                    &value,
                    &self.limits.scalar.values,
                )
                .map_err(|e| self.invalid(&e, Some(span)))?;
                Data::Scalar(value)
            }
            TypeNode::Enum(_) => {
                let observation = self.invocation_observation(None)?;
                let value = exact(&raw, &observation).map_err(|e| {
                    if let Err(error) = self.check_provider_status(Some(span)) {
                        error
                    } else {
                        self.invalid(&e, Some(span))
                    }
                })?;
                self.function_image()?
                    .validate_enum(value_ty, &value)
                    .map_err(|e| self.invalid(&e.to_string(), Some(span)))?;
                Data::Enum(value)
            }
            TypeNode::Datatype(_) | TypeNode::AppliedDatatype(_) => {
                self.nominal(value_ty, &raw, Some(span))?;
                Data::Datatype(raw)
            }
            TypeNode::Model(_) => {
                let element = raw.as_str().ok_or_else(|| {
                    self.invalid("Stored reference requires an element identity", Some(span))
                })?;
                let value = serde_json::json!({
                    "kind": "model_reference", "context": digest, "element": element,
                });
                self.nominal(value_ty, &value, Some(span))?;
                Data::Model(value)
            }
            _ => return Err(self.invalid("Stored singular result type required", Some(span))),
        };
        self.alloc(value_ty, data, Some(span))
    }
    pub(super) fn validate_bundle_context(&mut self) -> Result<()> {
        if self.function_context()?.references.len() > self.limits.max_model_contexts {
            return Err(self.resource("Model context count limit exceeded", None));
        }
        super::preflight::measure(
            self.function_context()?.bundle,
            self.limits.max_value_nodes,
            self.limits.max_value_depth,
            self.limits.max_context_bytes,
        )
        .map_err(|e| self.resource(&e, None))?;
        self.function_image()?
            .check_context_detailed(
                self.function_context()?.bundle,
                self.function_context()?.conversions,
            )
            .map_err(|e| {
                if let Err(error) = self.check_provider_status(None) {
                    return error;
                }
                match e {
                    ir_v2::ContextValidationError::Dependency(cause) => {
                        self.missing_dependency(DependencyCause::ExecutableContext(cause), None)
                    }
                    ir_v2::ContextValidationError::Invalid(e) => self.error(
                        if e.kind
                            == intersect_model::definitions::rcore_behavior::ir::IrErrorKind::ResourceExhausted
                        {
                            ExecutionErrorKind::ResourceExhausted
                        } else {
                            ExecutionErrorKind::Arguments
                        },
                        e.message,
                        None,
                    ),
                }
            })?;
        Ok(())
    }
    pub(super) fn validate_reference_policy(
        &mut self,
        context: &ReferenceContext,
        index: usize,
        span: Option<&SourceSpan>,
    ) -> Result<bool> {
        // Bundle admission has checked the registry manifest. Each later tick
        // checks one selected exact policy, without retaining cloned manifests.
        let empty = ConversionRegistry::default();
        let registry = self.function_context()?.conversions.unwrap_or(&empty);
        let matched = if let Some(contract) = context.conversions.get(index) {
            registry.contains_contract(contract)
        } else if let Some(contract) = context
            .applied_conversions
            .get(index.saturating_sub(context.conversions.len()))
        {
            registry.applied().contains_contract(contract)
        } else if let Some(contract) = context.constructors.get(
            index
                .saturating_sub(context.conversions.len())
                .saturating_sub(context.applied_conversions.len()),
        ) {
            registry.constructors().contains_contract(contract)
        } else {
            return Ok(true);
        }
        .map_err(|e| self.provider_error(e, span))?;
        if !matched {
            return Err(self.error(
                ExecutionErrorKind::Nominal,
                "Reference context policies differ from execution policies",
                span,
            ));
        }
        Ok(false)
    }
    pub(super) fn reference_context_bytes(
        &self,
        context: &ReferenceContext,
        span: Option<&SourceSpan>,
    ) -> Result<usize> {
        let mut counter = Counter {
            count: 0,
            limit: self
                .limits
                .max_context_bytes
                .saturating_sub(self.data.consumed.context_bytes),
        };
        serde_json::to_writer(&mut counter, context)
            .map_err(|_| self.resource("Context retained byte limit exceeded", span))?;
        Ok(counter.count)
    }
    pub(super) fn reserve_reference_membership(
        &mut self,
        count: usize,
        span: Option<&SourceSpan>,
    ) -> Result<()> {
        self.data.consumed.membership_checks = self
            .data
            .consumed
            .membership_checks
            .checked_add(count)
            .filter(|n| *n <= self.limits.max_membership_checks)
            .ok_or_else(|| self.resource("Model membership work limit exceeded", span))?;
        // A target retains only local row positions, never another
        // copy of object IDs or the graph. Reserve a conservative,
        // host-independent membership capacity before allocation.
        let membership_bytes = count
            .checked_mul(32)
            .and_then(|n| n.checked_add(64))
            .ok_or_else(|| self.resource("Reference membership storage overflow", span))?;
        self.retained_storage(membership_bytes, span)?;
        Ok(())
    }
    pub(super) fn prepare_reference_domain(
        &mut self,
        ty: ir_v2::TypeId,
        digest: &str,
        span: Option<&SourceSpan>,
    ) -> Result<Option<crate::model_reference::RuntimeReferenceDomainPreparation>> {
        if self.owned_domains()?.contains_key(&(digest.to_owned(), ty)) {
            return Ok(None);
        }
        if self.owned_domains()?.len() >= self.limits.max_reference_domains {
            return Err(self.resource("Reference domain count limit exceeded", span));
        }
        let TypeNode::Model(target) = self.type_node(ty)?.clone() else {
            return Err(self.invalid("Reference domain requires model type", span));
        };
        let existing_ty = *self
            .data
            .contexts
            .get(digest)
            .ok_or_else(|| self.invalid("Missing admitted reference context", span))?;
        let key = (digest.to_owned(), existing_ty);
        let count = self
            .owned_domains()?
            .get(&key)
            .ok_or_else(|| self.invalid("Missing pooled reference context", span))?
            .context()
            .model
            .get("elements")
            .and_then(|v| v.as_array())
            .map_or(0, Vec::len);
        self.reserve_reference_membership(count, span)?;
        let observation = self.invocation_observation(None)?;
        let pending = self
            .owned_domains()?
            .get(&key)
            .ok_or_else(|| self.invalid("Missing pooled reference context", span))?
            .begin_retarget_runtime(target, &observation)
            .map_err(|e| self.provider_error(e, span))?;
        self.check_provider_status(span)?;
        Ok(Some(pending))
    }
    pub(super) fn advance_reference_domain(
        &mut self,
        pending: &mut Option<crate::model_reference::RuntimeReferenceDomainPreparation>,
        ty: ir_v2::TypeId,
        digest: &str,
        span: Option<&SourceSpan>,
    ) -> Result<bool> {
        let Some(preparation) = pending.as_mut() else {
            return Ok(true);
        };
        self.step_at(span)?;
        let observation = self.invocation_observation(None)?;
        let done = preparation
            .advance(&observation)
            .map_err(|e| self.provider_error(e, span))?;
        self.check_provider_status(span)?;
        if done {
            let preparation = pending
                .take()
                .ok_or_else(|| self.invalid("Missing domain preparation", span))?;
            let domain = preparation
                .finish(&observation)
                .map_err(|e| self.provider_error(e, span))?;
            self.publish_owned_domain(digest.to_owned(), ty, domain)?;
            self.data.consumed.reference_domains = self.owned_domains()?.len();
        }
        Ok(done)
    }
    pub(super) fn prepare_reference_admission(
        &mut self,
        ty: ir_v2::TypeId,
        digest: &str,
        span: Option<&SourceSpan>,
    ) -> Result<crate::model_reference::RuntimeReferenceAdmission> {
        if self.owned_domains()?.len() >= self.limits.max_reference_domains {
            return Err(self.resource("Reference domain count limit exceeded", span));
        }
        let TypeNode::Model(target) = self.type_node(ty)?.clone() else {
            return Err(self.invalid("Reference domain requires model type", span));
        };
        let (index, bytes) =
            *self.data.reference_inputs.get(digest).ok_or_else(|| {
                self.missing_dependency(DependencyCause::ModelContextRevision, span)
            })?;
        let context = match &self.data.owned_reference_inputs {
            Some(inputs) => inputs.get(&index),
            None => self.function_context()?.references.get(index),
        }
        .ok_or_else(|| self.invalid("Missing prepared model context", span))?;
        let count = context
            .model
            .get("elements")
            .and_then(|v| v.as_array())
            .map_or(0, Vec::len);
        self.reserve_reference_membership(count, span)?;
        self.data.consumed.model_elements = self
            .data
            .consumed
            .model_elements
            .checked_add(count)
            .filter(|n| *n <= self.limits.max_model_elements)
            .ok_or_else(|| self.resource("Model element validation limit exceeded", span))?;
        self.data.consumed.context_bytes = self
            .data
            .consumed
            .context_bytes
            .checked_add(bytes)
            .filter(|n| *n <= self.limits.max_context_bytes)
            .ok_or_else(|| self.resource("Retained model context byte limit exceeded", span))?;
        if self.data.contexts.len() >= self.limits.max_model_contexts {
            return Err(self.resource("Model context count limit exceeded", span));
        }

        let empty = ConversionRegistry::default();
        let registry = self.function_context()?.conversions.unwrap_or(&empty);
        let observation = self.invocation_observation(None)?;
        let owning = self.data.owned_reference_inputs.is_some();
        let context = if let Some(inputs) = &mut self.data.owned_reference_inputs {
            inputs
                .remove(&index)
                .ok_or_else(|| self.invalid("Missing owned prepared model context", span))?
        } else {
            let context = self
                .function_context()?
                .references
                .get(index)
                .ok_or_else(|| self.invalid("Missing prepared model context", span))?;
            intersect_model::definitions::retained_storage::reserve_serialized(
                context,
                &observation,
            )
            .map_err(|e| self.provider_error(e, span))?;
            context.clone()
        };
        let pending = if owning {
            observation
                .retain_storage(
                    64usize
                        .checked_add(digest.len())
                        .ok_or_else(|| self.resource("Prepared identity storage overflow", span))?,
                )
                .map_err(|e| self.provider_error(e, span))?;
            let input =
                crate::rcore_behavior_functions::context::PreparedReferenceInput::from_registered_owned(
                    context,
                    digest.to_owned(),
                    index,
                    &self.data.reference_inputs,
                )
                .map_err(|e| self.provider_error(e, span))?;
            crate::model_reference::RuntimeReferenceDomain::begin_prepared_runtime_observed(
                target,
                input,
                registry,
                &observation,
            )
        } else {
            crate::model_reference::RuntimeReferenceDomain::begin_runtime_observed(
                target,
                context,
                registry,
                &observation,
            )
        }
        .map_err(|e| self.provider_error(e, span))?;
        self.check_provider_status(span)?;
        Ok(pending)
    }
    pub(super) fn advance_reference_admission(
        &mut self,
        pending: &mut Option<crate::model_reference::RuntimeReferenceAdmission>,
        ty: ir_v2::TypeId,
        digest: &str,
        span: Option<&SourceSpan>,
    ) -> Result<bool> {
        let Some(preparation) = pending.as_mut() else {
            return Ok(true);
        };
        self.step_at(span)?;
        let empty = ConversionRegistry::default();
        let registry = self.function_context()?.conversions.unwrap_or(&empty);
        let observation = self.invocation_observation(None)?;
        let done = preparation
            .advance(registry, &observation)
            .map_err(|e| self.provider_error(e, span))?;
        self.check_provider_status(span)?;
        if done {
            let preparation = pending
                .take()
                .ok_or_else(|| self.invalid("Missing reference admission", span))?;
            let domain = preparation
                .finish(&observation)
                .map_err(|e| self.provider_error(e, span))?;
            self.retained_storage(domain.ownership().retained_bytes(), span)?;
            self.data.observation.validation(digest, span);
            self.data.contexts.insert(digest.to_owned(), ty);
            self.publish_owned_domain(digest.to_owned(), ty, domain)?;
            self.data.consumed.model_contexts = self.data.contexts.len();
            self.data.consumed.reference_domains = self.owned_domains()?.len();
        }
        Ok(done)
    }
    pub(super) fn ensure_reference_domain(
        &mut self,
        ty: ir_v2::TypeId,
        digest: &str,
        span: Option<&SourceSpan>,
    ) -> Result<()> {
        if !self.owned_domains()?.contains_key(&(digest.to_owned(), ty)) {
            if self.data.contexts.contains_key(digest) {
                let mut pending = self.prepare_reference_domain(ty, digest, span)?;
                while !self.advance_reference_domain(&mut pending, ty, digest, span)? {}
            } else {
                let mut pending = Some(self.prepare_reference_admission(ty, digest, span)?);
                while !self.advance_reference_admission(&mut pending, ty, digest, span)? {}
            }
        }
        self.check_provider_status(span)
    }
    pub fn nominal(
        &mut self,
        ty: intersect_model::definitions::rcore_behavior::ir_v2::TypeId,
        payload: &serde_json::Value,
        span: Option<&SourceSpan>,
    ) -> Result<()> {
        let registry = self.function_context()?.conversions;
        let observation = self.invocation_observation(None)?;
        if let Some(mut job) =
            self.prepare_nominal_observed(ty, payload, registry, &observation, span)?
        {
            while !self.advance_nominal_observed(&mut job, &observation, span)? {}
        }
        self.check_provider_status(span)
    }
    /// Start the same nominal engine in the original decoder Visit. Opaque
    /// validation still completes in that Visit; only explicit cursors suspend.
    pub(super) fn begin_nominal<'value, 'providers>(
        &mut self,
        ty: intersect_model::definitions::rcore_behavior::ir_v2::TypeId,
        payload: &'value serde_json::Value,
        registry: Option<&'value ConversionRegistry<'providers>>,
        span: Option<&SourceSpan>,
    ) -> Result<
        Option<
            intersect_model::definitions::conversion::OwnedConversionValidationPreparation<
                'value,
                'providers,
            >,
        >,
    > {
        let observation = self.invocation_observation(None)?;
        if let Some(mut job) =
            self.prepare_nominal_observed(ty, payload, registry, &observation, span)?
            && !self.advance_nominal_observed(&mut job, &observation, span)?
        {
            return Ok(Some(job));
        }
        self.check_provider_status(span)?;
        Ok(None)
    }
    pub(super) fn advance_nominal(
        &self,
        job: &mut intersect_model::definitions::conversion::OwnedConversionValidationPreparation<
            '_,
            '_,
        >,
        span: Option<&SourceSpan>,
    ) -> Result<bool> {
        let observation = self.invocation_observation(None)?;
        self.advance_nominal_observed(job, &observation, span)
    }
    fn advance_nominal_observed(
        &self,
        job: &mut intersect_model::definitions::conversion::OwnedConversionValidationPreparation<
            '_,
            '_,
        >,
        observation: &InvocationContext<'_>,
        span: Option<&SourceSpan>,
    ) -> Result<bool> {
        use intersect_model::definitions::conversion::ConversionValidationProgress;
        match job.resume(std::num::NonZeroUsize::MIN, observation) {
            ConversionValidationProgress::Pending => Ok(false),
            ConversionValidationProgress::Complete(result) => {
                result.map_err(|e| self.provider_error(e, span))?;
                self.check_provider_status(span)?;
                Ok(true)
            }
            ConversionValidationProgress::Finished => {
                Err(self.invalid("Nominal validation already finished", span))
            }
        }
    }
    fn prepare_nominal_observed<'value, 'providers>(
        &mut self,
        ty: intersect_model::definitions::rcore_behavior::ir_v2::TypeId,
        payload: &'value serde_json::Value,
        authority: Option<&'value ConversionRegistry<'providers>>,
        observation: &InvocationContext<'_>,
        span: Option<&SourceSpan>,
    ) -> Result<
        Option<
            intersect_model::definitions::conversion::OwnedConversionValidationPreparation<
                'value,
                'providers,
            >,
        >,
    > {
        let actual = self
            .function_context()?
            .conversions
            .map(|r| std::ptr::from_ref(r).cast::<()>());
        if actual != authority.map(|r| std::ptr::from_ref(r).cast::<()>()) {
            return Err(self.invalid("Nominal registry source changed", span));
        }
        if I::BORROWED {
            if !matches!(self.type_node(ty)?, TypeNode::Model(_)) {
                return Err(self.error(
                    ExecutionErrorKind::Unsupported,
                    "Borrowed operation does not activate datatype providers",
                    span,
                ));
            }
            crate::definitions::retained_storage::reserve_serialized(
                self.type_node(ty)?,
                observation,
            )
            .map_err(|error| self.provider_error(error, span))?;
        }
        let empty = ConversionRegistry::default();
        let registry = authority.unwrap_or(&empty);
        match self.type_node(ty)?.clone() {
            TypeNode::Datatype(id) => {
                if !self.data.datatypes.contains_key(&ty) {
                    let mut types =
                        ModelTypes::with_conversions(self.function_context()?.bundle, registry)
                            .map_err(|e| self.provider_error(e, span))?;
                    let datatype = types
                        .datatype(&id)
                        .map_err(|e| self.provider_error(e, span))?
                        .clone();
                    self.data.datatypes.insert(ty, datatype);
                }
                let datatype = self
                    .data
                    .datatypes
                    .get(&ty)
                    .ok_or_else(|| self.invalid("Missing checked datatype", span))?;
                match datatype.binding() {
                    Binding::Scalar { .. } => {
                        let value =
                            intersect_model::definitions::datatypes::borrowed_value_with_work(
                                payload,
                                &|units| observation.work(units),
                            )
                            .map_err(|e| self.provider_error(e, span))?;
                        datatype
                            .validate_borrowed_value(&value)
                            .map_err(|e| self.provider_error(e, span))?;
                    }
                    Binding::Unsupported { .. } => {
                        let value =
                            intersect_model::definitions::conversion::borrowed_value_with_work(
                                payload,
                                &|units| observation.work(units),
                            )
                            .map_err(|e| self.provider_error(e, span))?;
                        self.checkpoint(span)?;
                        if let Some(registry) = authority {
                            let job = registry
                                .prepare_owned_header_validation_for_observed(
                                    datatype,
                                    value,
                                    observation,
                                )
                                .map_err(|e| self.provider_error(e, span))?;
                            return Ok(Some(job));
                        }
                        // Preserve the original absent-registry error ordering.
                        registry
                            .validate_payload_for_observed(datatype, &value, observation)
                            .map_err(|e| self.provider_error(e, span))?;
                    }
                }
            }
            TypeNode::AppliedDatatype(application) => {
                if !self.data.datatypes.contains_key(&ty) {
                    let mut types =
                        ModelTypes::with_conversions(self.function_context()?.bundle, registry)
                            .map_err(|e| self.provider_error(e, span))?;
                    let datatype = types
                        .applied_datatype(application.expression())
                        .map_err(|e| self.provider_error(e, span))?
                        .clone();
                    self.data.datatypes.insert(ty, datatype);
                }
                let value: AppliedConvertedValue =
                    exact(payload, observation).map_err(|e| self.provider_error(e, span))?;
                self.checkpoint(span)?;
                registry
                    .applied()
                    .validate_for_observed(&application, &value, observation)
                    .map_err(|e| self.provider_error(e, span))?;
            }
            TypeNode::Model(_) => I::nominal_model(self, ty, payload, span)?,
            _ => return Err(self.invalid("Expected nominal leaf type", span)),
        }
        self.check_provider_status(span)?;
        Ok(None)
    }
}
