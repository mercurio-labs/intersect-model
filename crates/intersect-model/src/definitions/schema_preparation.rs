//! Handwritten continuations for the shared schema member-selection authority.
#[cfg(test)]
use super::Schema;
use super::observation::{PreparationError, allocation, reserve, reserve_value};
use super::{
    EffectiveFeature, Feature, MemberIdentity, RuntimeDomainSource, validate_declaration_order,
};
use crate::definitions::conversion::InvocationContext;
use crate::definitions::resources::DeclarationId;
use std::collections::{BTreeMap, BTreeSet};
use std::ops::Bound::{Excluded, Unbounded};

enum ClassKeyStage {
    Header,
    Validate,
    Instantiations,
    Declarations,
    Done,
}
// Owned public continuations and immutable prepared lookups share the same
// ordered candidate selection. The retention policy owns or borrows only keys;
// it cannot skip source identity, ambiguity or instantiated-target checks.
struct ClassKeySelection<'a, K> {
    stage: &'a mut ClassKeyStage,
    cursor: &'a mut Option<K>,
    selected: &'a mut Option<K>,
    legacy_reference: bool,
}
fn class_key_mode(ty: &crate::definitions::generic_types::Type) -> Result<bool, PreparationError> {
    let crate::definitions::generic_types::Type::Classifier { arguments, raw, .. } = ty else {
        return Err("Expected a runtime classifier".into());
    };
    Ok(*raw || !arguments.is_empty())
}
fn advance_class_key_selection<'s, K: AsRef<str>>(
    schema: &'s crate::definitions::schema::SchemaRepresentation<impl super::RuntimeAlternative>,
    ty: &crate::definitions::generic_types::Type,
    state: ClassKeySelection<'_, K>,
    observation: &InvocationContext<'_>,
    mut retain: impl FnMut(&'s str, &InvocationContext<'_>) -> Result<K, PreparationError>,
) -> Result<bool, PreparationError> {
    match *state.stage {
        ClassKeyStage::Instantiations => {
            let next = match &*state.cursor {
                Some(key) => schema
                    .instantiations
                    .range::<str, _>((Excluded(key.as_ref()), Unbounded))
                    .next(),
                None => schema.instantiations.iter().next(),
            };
            if let Some((key, candidate)) = next {
                *state.cursor = Some(retain(key, observation)?);
                if candidate.expression() == ty {
                    *state.selected = Some(retain(key, observation)?);
                    *state.cursor = None;
                    *state.stage = ClassKeyStage::Done;
                }
            } else {
                return Err("Unknown instantiated class identity".into());
            }
        }
        ClassKeyStage::Declarations => {
            let crate::definitions::generic_types::Type::Classifier { declaration, .. } = ty else {
                return Err("Expected a runtime classifier".into());
            };
            let next = match &*state.cursor {
                Some(key) => schema
                    .declarations
                    .range::<str, _>((Excluded(key.as_ref()), Unbounded))
                    .next(),
                None => schema.declarations.iter().next(),
            };
            if let Some((key, value)) = next {
                *state.cursor = Some(retain(key, observation)?);
                if schema.classes.contains_key(key)
                    && (state.legacy_reference || !schema.instantiations.contains_key(key))
                    && value.source == declaration.source
                    && value.path == declaration.path
                {
                    if state.selected.is_some()
                        || (state.legacy_reference && schema.instantiations.contains_key(key))
                    {
                        return Err(if state.legacy_reference {
                            "Generic reference target requires an exact runtime type"
                        } else {
                            "Ambiguous closed class declaration"
                        }
                        .into());
                    }
                    *state.selected = Some(retain(key, observation)?);
                }
            } else {
                if state.selected.is_none() {
                    return Err(if state.legacy_reference {
                        "Reference target is not an exact selected EClass"
                    } else {
                        "Unknown closed class declaration"
                    }
                    .into());
                }
                *state.cursor = None;
                *state.stage = ClassKeyStage::Done;
            }
        }
        ClassKeyStage::Done => {}
        _ => return Err("Class key selection is not ready".into()),
    }
    Ok(matches!(*state.stage, ClassKeyStage::Done))
}

/// Runtime class selection over the same immutable schema as validation. Each
/// advance validates one schema transition or inspects one retained declaration.
/// Type expressions and declaration identities remain atomic clone/equality units.
pub struct ClassKeyPreparation {
    ty: crate::definitions::generic_types::Type,
    instantiated_only: bool,
    legacy_reference: bool,
    validation_complete: bool,
    checked_identity: Option<super::CheckedRuntimeSchemaIdentity>,
    stage: ClassKeyStage,
    validation: Option<Box<super::InstantiationValidation>>,
    cursor: Option<String>,
    selected: Option<String>,
    failed: bool,
}
impl ClassKeyPreparation {
    pub(super) fn checked_observed<A: super::RuntimeAlternative>(
        schema: &super::CheckedRuntimeSchema<A>,
        ty: &impl AsRef<crate::definitions::generic_types::Type>,
        observation: &InvocationContext<'_>,
    ) -> Result<Self, PreparationError> {
        let mut pending = Self::new_observed(ty, observation)?;
        pending.checked_identity = Some(schema.identity_observed(observation)?);
        pending.validation_complete = true;
        Ok(pending)
    }
    pub(super) fn checked_reference_declaration_observed<A: super::RuntimeAlternative>(
        schema: &super::CheckedRuntimeSchema<A>,
        declaration: &DeclarationId,
        observation: &InvocationContext<'_>,
    ) -> Result<Self, PreparationError> {
        let mut pending = Self::reference_declaration_observed(declaration, observation)?;
        pending.checked_identity = Some(schema.identity_observed(observation)?);
        pending.validation_complete = true;
        Ok(pending)
    }
    pub fn advance_checked_observed<A: super::RuntimeAlternative>(
        &mut self,
        schema: &super::CheckedRuntimeSchema<A>,
        observation: &InvocationContext<'_>,
    ) -> Result<bool, PreparationError> {
        if self.failed {
            return Err("Class key preparation has failed".into());
        }
        let result = (|| {
            observation
                .checkpoint()
                .map_err(PreparationError::Observation)?;
            let identity = self
                .checked_identity
                .as_ref()
                .ok_or("Class key preparation requires its ordinary schema path")?;
            if !schema.matches_identity(identity) {
                return Err("Checked class key schema changed".into());
            }
            self.advance_inner(schema.schema(), observation)
        })();
        if result.is_err() {
            self.failed = true;
        }
        result
    }
    pub fn new(ty: &impl AsRef<crate::definitions::generic_types::Type>) -> Self {
        Self::with_mode(ty, false)
    }
    /// Selection only, for a caller that holds an immutable schema whose
    /// instantiation validation already completed (`Metamodel::freeze`). The
    /// mode check and the selection are those of `new`; only the repeated
    /// whole-schema validation is left out.
    pub(super) fn validated(ty: &impl AsRef<crate::definitions::generic_types::Type>) -> Self {
        let mut pending = Self::with_mode(ty, false);
        pending.validation_complete = true;
        pending
    }
    pub fn new_observed(
        ty: &impl AsRef<crate::definitions::generic_types::Type>,
        observation: &InvocationContext<'_>,
    ) -> Result<Self, PreparationError> {
        reserve_value(ty.as_ref(), observation)?;
        Ok(Self::with_mode(ty, false))
    }
    pub fn instantiated(ty: &impl AsRef<crate::definitions::generic_types::Type>) -> Self {
        Self::with_mode(ty, true)
    }
    pub fn instantiated_observed(
        ty: &impl AsRef<crate::definitions::generic_types::Type>,
        observation: &InvocationContext<'_>,
    ) -> Result<Self, PreparationError> {
        reserve_value(ty.as_ref(), observation)?;
        Ok(Self::with_mode(ty, true))
    }
    /// Legacy declaration-only reference selection, preserving explicit rejection
    /// of ambiguous or instantiated targets. No runtime type is inferred.
    pub fn reference_declaration(declaration: &DeclarationId) -> Self {
        Self::with_expression(
            crate::definitions::generic_types::Type::Classifier {
                declaration: declaration.clone(),
                arguments: vec![],
                raw: false,
            },
            false,
            true,
        )
    }
    pub fn reference_declaration_observed(
        declaration: &DeclarationId,
        observation: &InvocationContext<'_>,
    ) -> Result<Self, PreparationError> {
        reserve_value(declaration, observation)?;
        Ok(Self::reference_declaration(declaration))
    }
    fn with_mode(
        ty: &impl AsRef<crate::definitions::generic_types::Type>,
        instantiated_only: bool,
    ) -> Self {
        Self::with_expression(ty.as_ref().clone(), instantiated_only, false)
    }
    fn with_expression(
        ty: crate::definitions::generic_types::Type,
        instantiated_only: bool,
        legacy_reference: bool,
    ) -> Self {
        Self {
            ty,
            instantiated_only,
            legacy_reference,
            validation_complete: false,
            checked_identity: None,
            stage: ClassKeyStage::Header,
            validation: None,
            cursor: None,
            selected: None,
            failed: false,
        }
    }
    pub fn advance(
        &mut self,
        schema: &crate::definitions::schema::SchemaRepresentation<impl super::RuntimeAlternative>,
    ) -> Result<bool, String> {
        self.advance_observed(schema, &InvocationContext::default())
            .map_err(|error| error.to_string())
    }
    pub fn advance_observed(
        &mut self,
        schema: &crate::definitions::schema::SchemaRepresentation<impl super::RuntimeAlternative>,
        observation: &InvocationContext<'_>,
    ) -> Result<bool, PreparationError> {
        if self.checked_identity.is_some() {
            self.failed = true;
            return Err(
                "Checked class key preparation requires its immutable checked schema".into(),
            );
        }
        if self.failed {
            return Err("Class key preparation has failed".into());
        }
        let result = observation
            .checkpoint()
            .map_err(PreparationError::Observation)
            .and_then(|()| self.advance_inner(schema, observation));
        if result.is_err() {
            self.failed = true;
        }
        result
    }
    fn advance_inner(
        &mut self,
        schema: &crate::definitions::schema::SchemaRepresentation<impl super::RuntimeAlternative>,
        observation: &InvocationContext<'_>,
    ) -> Result<bool, PreparationError> {
        match self.stage {
            ClassKeyStage::Header => {
                if self.legacy_reference {
                    self.stage = ClassKeyStage::Declarations;
                    return Ok(false);
                }
                if !self.instantiated_only {
                    self.instantiated_only = class_key_mode(&self.ty)?;
                }
                if self.validation_complete {
                    self.stage = if self.instantiated_only {
                        ClassKeyStage::Instantiations
                    } else {
                        ClassKeyStage::Declarations
                    };
                } else {
                    reserve(observation, 128, &[])?;
                    self.validation = Some(Box::new(super::InstantiationValidation::new()));
                    self.stage = ClassKeyStage::Validate;
                }
            }
            ClassKeyStage::Validate => {
                if self
                    .validation
                    .as_mut()
                    .ok_or("Missing class key validation")?
                    .advance_observed(schema, observation)?
                {
                    self.validation = None;
                    self.stage = if self.instantiated_only {
                        ClassKeyStage::Instantiations
                    } else {
                        ClassKeyStage::Declarations
                    };
                }
            }
            ClassKeyStage::Instantiations | ClassKeyStage::Declarations | ClassKeyStage::Done => {
                return advance_class_key_selection(
                    schema,
                    &self.ty,
                    ClassKeySelection {
                        stage: &mut self.stage,
                        cursor: &mut self.cursor,
                        selected: &mut self.selected,
                        legacy_reference: self.legacy_reference,
                    },
                    observation,
                    |key, observation| {
                        reserve(observation, 0, &[key])?;
                        Ok(key.to_owned())
                    },
                );
            }
        }
        Ok(matches!(self.stage, ClassKeyStage::Done))
    }
    pub fn finish(self) -> Result<String, String> {
        if self.failed || !matches!(self.stage, ClassKeyStage::Done) {
            return Err("Class key preparation is not complete".into());
        }
        self.selected
            .ok_or_else(|| "Missing prepared class key".into())
    }
}

// Only fresh completion of the shared structural validator creates this proof.
// Its immutable borrow fixes the exact schema throughout all dependent lookups.
// Public ClassKeyPreparation constructors cannot opt out of validation.
pub(crate) struct ValidatedRuntimeTypes<'a, A: super::RuntimeAlternative = String> {
    schema: &'a crate::definitions::schema::SchemaRepresentation<A>,
    retained: Option<Box<CompletedPhysicalInputs>>,
}
// A fresh preparation owns only its current validation progress. Its immutable
// schema borrow prevents proof reuse across mutation or another schema instance.
pub(crate) struct RuntimeTypesPreparation<'a, A: super::RuntimeAlternative> {
    schema: &'a crate::definitions::schema::SchemaRepresentation<A>,
    validation: super::InstantiationValidation,
    batch_duplicates: bool,
    reference_arrays: Option<Vec<ReferenceArrayCertificate<'a, A>>>,
    complete: bool,
    failed: bool,
    retained: Option<Box<CompletedPhysicalInputs>>,
}
// Private completed SOURCE inputs, never graph/query results. Every payload
// was already produced/paid by the canonical runtime metadata validator.
struct CompletedPhysicalInputs {
    captured: bool,
    fields: BTreeMap<String, std::sync::Arc<PreparedFeatureInputs>>,
    cache: Option<SharedFeatureInputCache>,
}
// Positive duplicate/class-membership authority for one exact retained array.
// Private construction requires original validator completion through this fixed
// schema borrow. No deserialization, public constructor or mutation is supplied.
pub(super) struct ReferenceArrayCertificate<'schema, A: super::RuntimeAlternative> {
    schema: &'schema crate::definitions::schema::SchemaRepresentation<A>,
    values: &'schema [A],
}
impl<A: super::RuntimeAlternative> ReferenceArrayCertificate<'_, A> {
    pub(super) fn belongs_to(
        &self,
        schema: &crate::definitions::schema::SchemaRepresentation<A>,
        values: &[A],
    ) -> bool {
        std::ptr::eq(self.schema, schema) && std::ptr::eq(self.values, values)
    }
}
impl<'a, A: super::RuntimeAlternative> RuntimeTypesPreparation<'a, A> {
    fn completed_reference_array_certificate(&self) -> Option<ReferenceArrayCertificate<'a, A>> {
        if self.failed {
            return None;
        }
        self.validation
            .completed_reference_array(self.schema)
            .map(|values| ReferenceArrayCertificate {
                schema: self.schema,
                values,
            })
    }
    pub(crate) fn advance_observed(
        &mut self,
        observation: &InvocationContext<'_>,
    ) -> Result<bool, PreparationError> {
        if self.failed {
            return Err("Runtime type preparation has failed".into());
        }
        if self.complete {
            return Ok(true);
        }
        let result = observation
            .work(1)
            .map_err(PreparationError::Observation)
            .and_then(|()| {
                // Capture ONLY after all semantic validation, before the FIRST
                // IdentityClear advance can mutate/release any prepared map.
                if matches!(
                    self.validation.stage,
                    super::InstantiationValidationStage::IdentityClear
                ) && let Some(retained) = &mut self.retained
                    && !retained.captured
                {
                    if self.validation.failed {
                        return Err("Cannot retain failed physical metadata inputs".into());
                    }
                    for (name, fields) in &self.validation.shared_fields {
                        admit_reference_input_lookup(retained.fields.len(), name, observation)?;
                        // Prepay the original clear_one payload/bookkeeping
                        // transitions under THIS observer before alias retention.
                        observation.work(1).map_err(PreparationError::Observation)?;
                        let cleanup = match fields.as_ref() {
                            PreparedFeatureInputs::Owned(value) => value
                                .fields
                                .len()
                                .checked_add(value.owners.len())
                                .and_then(|n| n.checked_add(value.identities.len()))
                                .and_then(|n| n.checked_add(value.order.len())),
                            PreparedFeatureInputs::Shared(value) => {
                                value.fields.len().checked_add(value.order.len())
                            }
                        }
                        .ok_or("Retained physical map cleanup work overflow")?;
                        observation
                            .work(cleanup.max(1))
                            .map_err(PreparationError::Observation)?;
                        reserve(observation, 128 + 32, &[name])?;
                        // Alias copy plus its added retained-map key cleanup.
                        observation.work(2).map_err(PreparationError::Observation)?;
                        retained.fields.insert(name.clone(), fields.clone());
                    }
                    if let Some(cache) = &self.validation.input_cache {
                        observation.work(1).map_err(PreparationError::Observation)?;
                        let cleanup = {
                            let cache = cache.try_lock().map_err(|_| {
                                PreparationError::Schema(
                                    "Physical input cache unavailable during retention".into(),
                                )
                            })?;
                            cache
                                .entries
                                .checked_add(cache.inputs.len())
                                .and_then(|n| n.checked_add(cache.source_ids.len()))
                                .and_then(|n| n.checked_add(cache.owner_names.len()))
                                .and_then(|n| n.checked_add(cache.field_names.len()))
                                .ok_or("Retained physical cache cleanup work overflow")?
                        };
                        // No observer callback runs while the cache lock is held.
                        observation
                            .work(cleanup.max(1))
                            .map_err(PreparationError::Observation)?;
                        reserve(observation, 32, &[])?;
                        observation.work(2).map_err(PreparationError::Observation)?;
                        retained.cache = Some(cache.clone());
                    }
                    observation
                        .checkpoint()
                        .map_err(PreparationError::Observation)?;
                    retained.captured = true;
                    // Ordinary cleanup still drops its own aliases. The existing
                    // Arc alias route preserves already-paid payloads. No refund.
                }
                if let Some(proofs) = &self.reference_arrays
                    && let Some(values) = self.validation.pending_reference_array(self.schema)
                {
                    for proof in proofs {
                        observation.work(1).map_err(PreparationError::Observation)?;
                        if proof.belongs_to(self.schema, values) {
                            self.validation.use_reference_array_certificate(
                                self.schema,
                                proof,
                                observation,
                            )?;
                            return Ok(false);
                        }
                    }
                }
                if self.batch_duplicates && self.validation.duplicate_transition() {
                    // At most sixteen existing index transitions per dispatch. Every
                    // comparison, shift and allocation still runs its original observer.
                    for _ in 0..16 {
                        let done = self.validation.advance_observed(self.schema, observation)?;
                        if done || !self.validation.duplicate_transition() {
                            return Ok(done);
                        }
                    }
                    Ok(false)
                } else {
                    self.validation.advance_observed(self.schema, observation)
                }
            });
        match result {
            Ok(done) => {
                if self.reference_arrays.is_some()
                    && let Some(certificate) = self.completed_reference_array_certificate()
                {
                    let proofs = self
                        .reference_arrays
                        .as_mut()
                        .ok_or("Missing reference array policy")?;
                    let mut found = false;
                    for proof in proofs.iter() {
                        if let Err(error) = observation.work(1) {
                            self.failed = true;
                            return Err(PreparationError::Observation(error));
                        }
                        if proof.belongs_to(self.schema, certificate.values) {
                            found = true;
                            break;
                        }
                    }
                    if !found && proofs.len() < 4096 {
                        if let Err(error) = reserve(observation, 128, &[]) {
                            self.failed = true;
                            return Err(error);
                        }
                        if let Err(error) = allocation(proofs, 1) {
                            self.failed = true;
                            return Err(error);
                        }
                        proofs.push(certificate);
                    }
                }
                self.complete = done;
                Ok(done)
            }
            Err(error) => {
                self.failed = true;
                Err(error)
            }
        }
    }
    pub(crate) fn finish(self) -> Result<ValidatedRuntimeTypes<'a, A>, PreparationError> {
        if self.failed || !self.complete {
            return Err("Runtime type preparation is incomplete or failed".into());
        }
        if self
            .retained
            .as_ref()
            .is_some_and(|inputs| !inputs.captured)
        {
            return Err("Completed physical inputs were not captured".into());
        }
        Ok(ValidatedRuntimeTypes {
            schema: self.schema,
            retained: self.retained,
        })
    }
}
impl<'a, A: super::RuntimeAlternative> ValidatedRuntimeTypes<'a, A> {
    pub(crate) fn begin(
        schema: &'a crate::definitions::schema::SchemaRepresentation<A>,
    ) -> RuntimeTypesPreparation<'a, A> {
        RuntimeTypesPreparation {
            schema,
            validation: super::InstantiationValidation::with_reference_views(),
            batch_duplicates: false,
            reference_arrays: None,
            complete: false,
            failed: false,
            retained: None,
        }
    }
    pub(crate) fn begin_batched_duplicates(
        schema: &'a crate::definitions::schema::SchemaRepresentation<A>,
    ) -> RuntimeTypesPreparation<'a, A> {
        let mut preparation = Self::begin(schema);
        preparation.batch_duplicates = true;
        preparation
    }
    pub(crate) fn begin_reused_reference_arrays(
        schema: &'a crate::definitions::schema::SchemaRepresentation<A>,
    ) -> RuntimeTypesPreparation<'a, A> {
        let mut preparation = Self::begin_batched_duplicates(schema);
        preparation.reference_arrays = Some(Vec::new());
        preparation
    }
    pub(crate) fn prepare_observed(
        schema: &'a crate::definitions::schema::SchemaRepresentation<A>,
        observation: &InvocationContext<'_>,
    ) -> Result<Self, PreparationError> {
        let mut preparation = Self::begin(schema);
        while !preparation.advance_observed(observation)? {}
        preparation.finish()
    }
    // Explicit physical-metadata opt-in; old validation paths stay unchanged.
    pub(crate) fn prepare_with_physical_inputs_observed(
        schema: &'a crate::definitions::schema::SchemaRepresentation<A>,
        observation: &InvocationContext<'_>,
    ) -> Result<Self, PreparationError> {
        const {
            assert!(std::mem::size_of::<CompletedPhysicalInputs>() <= 128);
        }
        reserve(observation, 128, &[])?;
        let mut pending = Self::begin(schema);
        pending.retained = Some(Box::new(CompletedPhysicalInputs {
            captured: false,
            fields: BTreeMap::new(),
            cache: None,
        }));
        while !pending.advance_observed(observation)? {}
        pending.finish()
    }
    pub(crate) fn physical_fields_observed(
        &self,
        schema: &crate::definitions::schema::SchemaRepresentation<A>,
        receiver: &str,
        observation: &InvocationContext<'_>,
    ) -> Result<Option<std::sync::Arc<PreparedFeatureInputs>>, String> {
        observation.work(3)?;
        if !self.belongs_to(schema) {
            return Err("Physical input schema owner changed".into());
        }
        let Some(inputs) = &self.retained else {
            return Ok(None);
        };
        if !inputs.captured {
            return Err("Physical inputs are incomplete".into());
        }
        admit_reference_input_lookup(inputs.fields.len(), receiver, observation)
            .map_err(|e| e.to_string())?;
        let result = if let Some(fields) = inputs.fields.get(receiver) {
            reserve(observation, 32, &[]).map_err(|e| e.to_string())?;
            observation.work(1)?;
            Some(fields.clone())
        } else {
            None
        };
        observation.checkpoint()?;
        if !self.belongs_to(schema) {
            return Err("Physical input schema owner changed".into());
        }
        Ok(result)
    }
    pub(crate) fn physical_cache_observed(
        &self,
        schema: &crate::definitions::schema::SchemaRepresentation<A>,
        observation: &InvocationContext<'_>,
    ) -> Result<Option<SharedFeatureInputCache>, String> {
        observation.work(3)?;
        if !self.belongs_to(schema) {
            return Err("Physical input schema owner changed".into());
        }
        let result = if let Some(inputs) = &self.retained {
            if !inputs.captured {
                return Err("Physical inputs are incomplete".into());
            }
            if let Some(cache) = &inputs.cache {
                reserve(observation, 32, &[]).map_err(|e| e.to_string())?;
                observation.work(1)?;
                Some(cache.clone())
            } else {
                None
            }
        } else {
            None
        };
        observation.checkpoint()?;
        if !self.belongs_to(schema) {
            return Err("Physical input schema owner changed".into());
        }
        Ok(result)
    }
    pub(crate) fn belongs_to(
        &self,
        schema: &crate::definitions::schema::SchemaRepresentation<A>,
    ) -> bool {
        std::ptr::eq(self.schema, schema)
    }
    pub(crate) fn class_key_for_type_observed(
        &self,
        ty: &impl AsRef<crate::definitions::generic_types::Type>,
        observation: &InvocationContext<'_>,
    ) -> Result<&'a str, PreparationError> {
        // Retain the existing owned query expression contract. Only immutable
        // schema cursor and result keys use the borrowed retention policy.
        reserve_value(ty.as_ref(), observation)?;
        let ty = ty.as_ref().clone();
        let mut stage = if class_key_mode(&ty)? {
            ClassKeyStage::Instantiations
        } else {
            ClassKeyStage::Declarations
        };
        let mut cursor = None;
        let mut selected = None;
        loop {
            observation.work(1).map_err(PreparationError::Observation)?;
            observation
                .checkpoint()
                .map_err(PreparationError::Observation)?;
            if advance_class_key_selection(
                self.schema,
                &ty,
                ClassKeySelection {
                    stage: &mut stage,
                    cursor: &mut cursor,
                    selected: &mut selected,
                    legacy_reference: false,
                },
                observation,
                |key, _| Ok(key),
            )? {
                break;
            }
        }
        let key = selected.ok_or("Missing prepared class key")?;
        self.schema
            .classes
            .get_key_value(key)
            .map(|(key, _)| key.as_str())
            .ok_or_else(|| "Missing prepared runtime class key".into())
    }
}

enum ReferenceStructureStage {
    Header,
    ContainerPrepare,
    ContainerCheck,
    ContainerClear,
    ReceiverPrepare,
    OppositePrepare,
    Done,
}

// Internal immutable-schema lookup retains a key, never a reduced feature DTO.
// Public continuations retain their complete independently owned input.
enum ReferenceFeatureInput {
    Owned(Feature),
    Declared(String),
    Prepared(String),
}
impl ReferenceFeatureInput {
    fn resolve<'a>(
        &'a self,
        schema: &'a crate::definitions::schema::SchemaRepresentation<
            impl super::RuntimeAlternative,
        >,
        receiver: &str,
        fields: Option<&'a dyn PreparedFeatureLookup>,
    ) -> Result<&'a Feature, PreparationError> {
        match self {
            Self::Owned(feature) => Ok(feature),
            Self::Prepared(name) => fields
                .filter(|fields| fields.receiver() == receiver)
                .and_then(|fields| fields.get(name))
                .ok_or_else(|| "Missing prepared reference feature".into()),
            Self::Declared(name) => schema
                .classes
                .get(receiver)
                .and_then(|class| class.features.get(name))
                .ok_or_else(|| "Missing declared reference feature".into()),
        }
    }
}

/// Shared structural reference checks. Member inventories, default policies and
/// successful retained-entry cleanup yield separately against an immutable schema.
/// Feature/default measurement and copies remain atomic. Observed entry points
/// precharge retained values and propagate observation into shared member selection.
// Private metadata aliases remain inside one RuntimeTypesPreparation holding the
// exact immutable schema. They are complete canonical member/default inputs,
// never a cached validity result or authority transferable to another owner.
enum ReferenceStructureFields {
    Owned(PreparedFeatures),
    Shared(std::sync::Arc<PreparedFeatureInputs>),
}
impl ReferenceStructureFields {
    fn get(&self, name: &str) -> Option<&Feature> {
        match self {
            Self::Owned(fields) => fields.get(name),
            Self::Shared(fields) => fields.get(name),
        }
    }
    fn clear_one(&mut self) -> bool {
        match self {
            Self::Owned(fields) => fields.clear_one(),
            Self::Shared(_) => true, // Release this alias; the validator owns the map.
        }
    }
}
type ReferenceStructureInputs<'a> = (
    &'a SharedFeatureInputCache,
    &'a mut BTreeMap<String, std::sync::Arc<PreparedFeatureInputs>>,
);
fn admit_reference_input_lookup(
    count: usize,
    name: &str,
    observation: &InvocationContext<'_>,
) -> Result<(), PreparationError> {
    let count = u64::try_from(count).map_err(|_| "Reference input count overflow")?;
    let height = (u64::BITS - count.leading_zeros()) as usize;
    let work = height
        .checked_mul(1 + name.len().div_ceil(64))
        .and_then(|work| work.checked_add(1))
        .ok_or("Reference input lookup work overflow")?;
    observation
        .work(work)
        .map_err(PreparationError::Observation)
}
pub struct ReferenceStructurePreparation {
    receiver: String,
    feature: ReferenceFeatureInput,
    stage: ReferenceStructureStage,
    opposite: Option<(String, String)>,
    map: Option<FeatureMapPreparation>,
    fields: Option<ReferenceStructureFields>,
    owner: Option<FeatureOwnerPreparation>,
    receiver_owner: Option<String>,
    failed: bool,
}
impl ReferenceStructurePreparation {
    pub fn new(receiver: &str, feature: &Feature) -> Self {
        Self::with_inputs(receiver, feature)
    }
    pub fn new_observed(
        receiver: &str,
        feature: &Feature,
        observation: &InvocationContext<'_>,
    ) -> Result<Self, PreparationError> {
        reserve(observation, 0, &[receiver])?;
        reserve_value(feature, observation)?;
        Ok(Self::with_inputs(receiver, feature))
    }
    fn with_inputs(receiver: &str, feature: &Feature) -> Self {
        Self {
            receiver: receiver.into(),
            feature: ReferenceFeatureInput::Owned(feature.clone()),
            stage: ReferenceStructureStage::Header,
            opposite: None,
            map: None,
            fields: None,
            owner: None,
            receiver_owner: None,
            failed: false,
        }
    }
    pub(super) fn declared_observed(
        receiver: &str,
        name: &str,
        observation: &InvocationContext<'_>,
    ) -> Result<Self, PreparationError> {
        reserve(observation, 0, &[receiver, name])?;
        Ok(Self {
            receiver: receiver.into(),
            feature: ReferenceFeatureInput::Declared(name.into()),
            stage: ReferenceStructureStage::Header,
            opposite: None,
            map: None,
            fields: None,
            owner: None,
            receiver_owner: None,
            failed: false,
        })
    }
    pub(crate) fn prepared_observed(
        receiver: &str,
        name: &str,
        observation: &InvocationContext<'_>,
    ) -> Result<Self, PreparationError> {
        let mut result = Self::declared_observed(receiver, name, observation)?;
        result.feature = match result.feature {
            ReferenceFeatureInput::Declared(name) => ReferenceFeatureInput::Prepared(name),
            other => other,
        };
        Ok(result)
    }
    pub(crate) fn advance_prepared_observed(
        &mut self,
        schema: &crate::definitions::schema::SchemaRepresentation<impl super::RuntimeAlternative>,
        fields: &dyn PreparedFeatureLookup,
        observation: &InvocationContext<'_>,
    ) -> Result<bool, PreparationError> {
        self.advance_core_observed(schema, Some(fields), None, observation)
    }
    pub fn advance(
        &mut self,
        schema: &crate::definitions::schema::SchemaRepresentation<impl super::RuntimeAlternative>,
    ) -> Result<bool, String> {
        self.advance_observed(schema, &InvocationContext::default())
            .map_err(|error| error.to_string())
    }
    pub fn advance_observed(
        &mut self,
        schema: &crate::definitions::schema::SchemaRepresentation<impl super::RuntimeAlternative>,
        observation: &InvocationContext<'_>,
    ) -> Result<bool, PreparationError> {
        self.advance_core_observed(schema, None, None, observation)
    }
    // Only the exact-schema runtime validator supplies this private cache. Public
    // continuations retain their old independent member preparations.
    pub(super) fn advance_shared_observed(
        &mut self,
        schema: &crate::definitions::schema::SchemaRepresentation<impl super::RuntimeAlternative>,
        source_inputs: &SharedFeatureInputCache,
        shared_fields: &mut BTreeMap<String, std::sync::Arc<PreparedFeatureInputs>>,
        observation: &InvocationContext<'_>,
    ) -> Result<bool, PreparationError> {
        self.advance_core_observed(
            schema,
            None,
            Some((source_inputs, shared_fields)),
            observation,
        )
    }
    fn advance_core_observed(
        &mut self,
        schema: &crate::definitions::schema::SchemaRepresentation<impl super::RuntimeAlternative>,
        fields: Option<&dyn PreparedFeatureLookup>,
        shared: Option<ReferenceStructureInputs<'_>>,
        observation: &InvocationContext<'_>,
    ) -> Result<bool, PreparationError> {
        if self.failed {
            return Err("Reference structure preparation has failed".into());
        }
        let result = observation
            .checkpoint()
            .map_err(PreparationError::Observation)
            .and_then(|()| self.advance_inner(schema, fields, shared, observation));
        if result.is_err() {
            self.failed = true;
        }
        result
    }
    fn start_self_check(
        &mut self,
        schema: &crate::definitions::schema::SchemaRepresentation<impl super::RuntimeAlternative>,
        fields: Option<&dyn PreparedFeatureLookup>,
        observation: &InvocationContext<'_>,
    ) -> Result<(), PreparationError> {
        let feature = self.feature.resolve(schema, &self.receiver, fields)?;
        if self
            .opposite
            .as_ref()
            .is_some_and(|(_, name)| name == &feature.name)
        {
            self.owner = Some(FeatureOwnerPreparation::new_observed(
                &self.receiver,
                &feature.name,
                observation,
            )?);
            self.stage = ReferenceStructureStage::ReceiverPrepare;
        } else {
            self.stage = ReferenceStructureStage::Done;
        }
        Ok(())
    }
    fn advance_inner(
        &mut self,
        schema: &crate::definitions::schema::SchemaRepresentation<impl super::RuntimeAlternative>,
        fields: Option<&dyn PreparedFeatureLookup>,
        mut shared: Option<ReferenceStructureInputs<'_>>,
        observation: &InvocationContext<'_>,
    ) -> Result<bool, PreparationError> {
        match self.stage {
            ReferenceStructureStage::Header => {
                let feature = self.feature.resolve(schema, &self.receiver, fields)?;
                if feature.reference
                    && !feature.unique
                    && feature.upper != 1
                    && (feature.containment || feature.opposite.is_some())
                {
                    return Err(format!(
                        "Containment or bidirectional reference must be unique when upper bound differs from 1: {}.{}",
                        self.receiver, feature.name
                    ).into());
                }
                if let Some(opposite) = &feature.opposite {
                    let (owner, name) = opposite
                        .split_once('/')
                        .ok_or("Invalid opposite identity")?;
                    reserve(observation, 0, &[owner, name])?;
                    self.opposite = Some((owner.into(), name.into()));
                    if feature.reference && feature.upper != 1 {
                        if let Some((source_inputs, shared_fields)) = shared.as_mut() {
                            admit_reference_input_lookup(shared_fields.len(), owner, observation)?;
                            if let Some(complete) = shared_fields.get(owner) {
                                reserve(observation, 32, &[])?;
                                observation.work(1).map_err(PreparationError::Observation)?;
                                self.fields =
                                    Some(ReferenceStructureFields::Shared(complete.clone()));
                                self.stage = ReferenceStructureStage::ContainerCheck;
                            } else {
                                reserve(observation, 32, &[])?;
                                observation.work(1).map_err(PreparationError::Observation)?;
                                self.map =
                                    Some(FeatureMapPreparation::with_source_inputs_observed(
                                        owner,
                                        (*source_inputs).clone(),
                                        observation,
                                    )?);
                                self.stage = ReferenceStructureStage::ContainerPrepare;
                            }
                        } else {
                            self.map =
                                Some(FeatureMapPreparation::new_observed(owner, observation)?);
                            self.stage = ReferenceStructureStage::ContainerPrepare;
                        }
                    } else {
                        self.start_self_check(schema, fields, observation)?;
                    }
                } else {
                    self.stage = ReferenceStructureStage::Done;
                }
            }
            ReferenceStructureStage::ContainerPrepare => {
                if self
                    .map
                    .as_mut()
                    .ok_or("Missing opposite member preparation")?
                    .advance_observed(schema, observation)?
                {
                    let preparation = self
                        .map
                        .take()
                        .ok_or("Missing opposite member preparation")?;
                    self.fields = Some(if let Some((_, shared_fields)) = shared.as_mut() {
                        let complete = preparation.finish_inputs()?;
                        let (owner, _) =
                            self.opposite.as_ref().ok_or("Missing opposite identity")?;
                        if shared_fields.len() < 4096 {
                            admit_reference_input_lookup(shared_fields.len(), owner, observation)?;
                            reserve(observation, 128 + 128 + 64, &[owner])?;
                            let publication_work = 2usize
                                .checked_add(owner.len().div_ceil(64))
                                .ok_or("Reference input publication work overflow")?;
                            observation
                                .work(publication_work)
                                .map_err(PreparationError::Observation)?;
                            let complete = std::sync::Arc::new(complete);
                            shared_fields.insert(owner.clone(), complete.clone());
                            ReferenceStructureFields::Shared(complete)
                        } else {
                            // Keep the existing finite cache policy. A full cache
                            // still performs the actual canonical preparation.
                            ReferenceStructureFields::Owned(
                                complete.into_owned_observed(observation)?,
                            )
                        }
                    } else {
                        ReferenceStructureFields::Owned(preparation.finish()?)
                    });
                    self.stage = ReferenceStructureStage::ContainerCheck;
                }
            }
            ReferenceStructureStage::ContainerCheck => {
                let (_, name) = self.opposite.as_ref().ok_or("Missing opposite identity")?;
                let complete = self.fields.as_ref().ok_or("Missing opposite members")?;
                if let ReferenceStructureFields::Shared(complete) = complete {
                    complete
                        .admit_lookup_observed(name, 1, observation)
                        .map_err(PreparationError::Observation)?;
                }
                let opposite = complete.get(name).ok_or("Unknown opposite feature")?;
                if opposite.containment {
                    return Err(format!(
                        "Container reference must have upper bound 1: {}.{}",
                        self.receiver,
                        self.feature.resolve(schema, &self.receiver, fields)?.name
                    )
                    .into());
                }
                self.stage = ReferenceStructureStage::ContainerClear;
            }
            ReferenceStructureStage::ContainerClear => {
                if matches!(self.fields, Some(ReferenceStructureFields::Shared(_))) {
                    observation.work(1).map_err(PreparationError::Observation)?;
                }
                if self
                    .fields
                    .as_mut()
                    .ok_or("Missing opposite members")?
                    .clear_one()
                {
                    self.fields = None;
                    self.start_self_check(schema, fields, observation)?;
                }
            }
            ReferenceStructureStage::ReceiverPrepare => {
                if self
                    .owner
                    .as_mut()
                    .ok_or("Missing receiver owner preparation")?
                    .advance_observed(schema, observation)?
                {
                    self.receiver_owner = Some(
                        self.owner
                            .take()
                            .ok_or("Missing receiver owner preparation")?
                            .finish()?,
                    );
                    let (owner, name) =
                        self.opposite.as_ref().ok_or("Missing opposite identity")?;
                    self.owner = Some(FeatureOwnerPreparation::new_observed(
                        owner,
                        name,
                        observation,
                    )?);
                    self.stage = ReferenceStructureStage::OppositePrepare;
                }
            }
            ReferenceStructureStage::OppositePrepare => {
                if self
                    .owner
                    .as_mut()
                    .ok_or("Missing opposite owner preparation")?
                    .advance_observed(schema, observation)?
                {
                    let owner = self
                        .owner
                        .take()
                        .ok_or("Missing opposite owner preparation")?
                        .finish()?;
                    if self.receiver_owner.as_deref() == Some(owner.as_str()) {
                        return Err(format!(
                            "{}.{}: A reference cannot be its own opposite",
                            self.receiver,
                            self.feature.resolve(schema, &self.receiver, fields)?.name
                        )
                        .into());
                    }
                    self.receiver_owner = None;
                    self.stage = ReferenceStructureStage::Done;
                }
            }
            ReferenceStructureStage::Done => {}
        }
        Ok(matches!(self.stage, ReferenceStructureStage::Done))
    }
    pub fn finish(self) -> Result<(), String> {
        if self.failed || !matches!(self.stage, ReferenceStructureStage::Done) {
            return Err("Reference structure preparation is not complete".into());
        }
        Ok(())
    }
}

// Declaring-owner selection uses the effective-member authority without applying
// defaults, matching Schema::feature_owner. Scan/discard one retained member per step.
struct FeatureOwnerPreparation {
    receiver: String,
    name: String,
    members: Option<EffectiveFeaturePreparation>,
    scan: Option<std::vec::IntoIter<SelectedFeature>>,
    selected: Option<String>,
    done: bool,
}
impl FeatureOwnerPreparation {
    fn new_observed(
        receiver: &str,
        name: &str,
        observation: &InvocationContext<'_>,
    ) -> Result<Self, PreparationError> {
        reserve(observation, 0, &[receiver, name])?;
        let members = EffectiveFeaturePreparation::new_observed(receiver, observation)?;
        Ok(Self {
            receiver: receiver.into(),
            name: name.into(),
            members: Some(members),
            scan: None,
            selected: None,
            done: false,
        })
    }
    fn advance_observed(
        &mut self,
        schema: &crate::definitions::schema::SchemaRepresentation<impl super::RuntimeAlternative>,
        observation: &InvocationContext<'_>,
    ) -> Result<bool, PreparationError> {
        if let Some(members) = &mut self.members {
            if members.advance_observed(schema, observation)? {
                self.scan = Some(
                    self.members
                        .take()
                        .ok_or("Missing owner members")?
                        .finish()?
                        .into_iter(),
                );
            }
        } else if let Some(member) = self.scan.as_mut().ok_or("Missing owner scan")?.next() {
            if member.name == self.name {
                let owner = member.resolve(schema)?.owner;
                reserve(observation, 0, &[owner])?;
                self.selected = Some(owner.into());
            }
        } else {
            self.scan = None;
            self.done = true;
        }
        Ok(self.done)
    }
    fn finish(self) -> Result<String, String> {
        if !self.done {
            return Err("Owner preparation is not complete".into());
        }
        self.selected
            .ok_or_else(|| format!("Unknown feature {}.{}", self.receiver, self.name))
    }
}

enum AncestryTask {
    Enter(String),
    Check(String, usize),
    Clear(String),
    Parents(String, usize),
    Exit(String),
}
pub(super) struct AncestryPreparation {
    pending: Vec<AncestryTask>,
    active: BTreeSet<String>,
    complete: BTreeSet<String>,
    direct: BTreeSet<String>,
    ordered: Vec<String>,
    done: bool,
    failed: bool,
}

enum ConformanceStage {
    Header,
    Ancestry,
    Exact,
    Raw,
    Clear,
    Done,
}
/// Shared class conformance over an immutable schema. Inheritance edges and
/// ancestor membership checks advance separately; successful results publish
/// only after the retained ancestor list has been released incrementally.
pub struct ConformancePreparation {
    actual: String,
    expected: String,
    stage: ConformanceStage,
    ancestry: Option<AncestryPreparation>,
    ancestors: Vec<String>,
    index: usize,
    universal: bool,
    accepted: bool,
    failed: bool,
}
impl ConformancePreparation {
    /// Precharge both retained class keys before copying them. Keep the same
    /// observation for every advance; observation failures are not predicates.
    pub fn new_observed(
        actual: &str,
        expected: &str,
        observation: &InvocationContext<'_>,
    ) -> Result<Self, PreparationError> {
        reserve(observation, 0, &[actual, expected])?;
        Ok(Self::new(actual, expected))
    }
    pub fn new(actual: &str, expected: &str) -> Self {
        Self {
            actual: actual.into(),
            expected: expected.into(),
            stage: ConformanceStage::Header,
            ancestry: None,
            ancestors: Vec::new(),
            index: 0,
            universal: false,
            accepted: false,
            failed: false,
        }
    }
    pub fn advance(
        &mut self,
        schema: &crate::definitions::schema::SchemaRepresentation<impl super::RuntimeAlternative>,
    ) -> Result<bool, String> {
        self.advance_observed(schema, &InvocationContext::default())
            .map_err(|error| error.to_string())
    }
    pub fn advance_observed(
        &mut self,
        schema: &crate::definitions::schema::SchemaRepresentation<impl super::RuntimeAlternative>,
        observation: &InvocationContext<'_>,
    ) -> Result<bool, PreparationError> {
        if self.failed {
            return Err(PreparationError::Schema(
                "Conformance preparation has failed".into(),
            ));
        }
        let result = observation
            .checkpoint()
            .map_err(PreparationError::Observation)
            .and_then(|()| self.advance_inner(schema, observation));
        if result.is_err() {
            self.failed = true;
        }
        result
    }
    fn advance_inner(
        &mut self,
        schema: &crate::definitions::schema::SchemaRepresentation<impl super::RuntimeAlternative>,
        observation: &InvocationContext<'_>,
    ) -> Result<bool, PreparationError> {
        match self.stage {
            ConformanceStage::Header => {
                let is_eobject = schema.classes.contains_key(&self.expected)
                    && schema
                        .declarations
                        .get(&self.expected)
                        .is_some_and(|source| {
                            super::eobject_reference_top_declaration().is_ok_and(|top| {
                                source.source == top.source && source.path == top.path
                            })
                        });
                self.universal = is_eobject
                    || (self.expected == super::EOBJECT_REFERENCE_TOP
                        && schema.reference_top.is_some());
                if self.universal {
                    schema.validate_reference_top()?;
                }
                self.ancestry = Some(AncestryPreparation::new_observed(
                    &self.actual,
                    observation,
                )?);
                self.stage = ConformanceStage::Ancestry;
            }
            ConformanceStage::Ancestry => {
                if self
                    .ancestry
                    .as_mut()
                    .ok_or("Missing conformance ancestry")?
                    .advance_observed(schema, observation)?
                {
                    self.ancestors = self
                        .ancestry
                        .take()
                        .ok_or("Missing conformance ancestry")?
                        .finish()?;
                    if self.universal {
                        self.accepted = true;
                        self.stage = ConformanceStage::Clear;
                    } else {
                        self.stage = ConformanceStage::Exact;
                    }
                }
            }
            ConformanceStage::Exact => {
                if let Some(ancestor) = self.ancestors.get(self.index) {
                    if ancestor == &self.expected {
                        self.accepted = true;
                        self.stage = ConformanceStage::Clear;
                    } else {
                        self.index += 1;
                    }
                } else {
                    self.index = 0;
                    self.stage = ConformanceStage::Raw;
                }
            }
            ConformanceStage::Raw => {
                if let Some(crate::definitions::generic_types::Type::Classifier {
                    declaration,
                    raw: true,
                    ..
                }) = schema
                    .instantiations
                    .get(&self.expected)
                    .map(|target| target.expression())
                {
                    if let Some(ancestor) = self.ancestors.get(self.index) {
                        if schema.declarations.get(ancestor).is_some_and(|source| {
                            source.source == declaration.source && source.path == declaration.path
                        }) {
                            self.accepted = true;
                            self.stage = ConformanceStage::Clear;
                        } else {
                            self.index += 1;
                        }
                    } else {
                        self.stage = ConformanceStage::Clear;
                    }
                } else {
                    self.stage = ConformanceStage::Clear;
                }
            }
            ConformanceStage::Clear => {
                self.ancestors.pop();
                if self.ancestors.is_empty() {
                    self.stage = ConformanceStage::Done;
                }
            }
            ConformanceStage::Done => return Ok(true),
        }
        Ok(matches!(self.stage, ConformanceStage::Done))
    }
    pub fn finish(self) -> Result<bool, String> {
        if self.failed || !matches!(self.stage, ConformanceStage::Done) {
            return Err("Conformance preparation is not complete".into());
        }
        Ok(self.accepted)
    }
}
enum ReferenceStage {
    Members,
    Scan,
    Domains,
    Alternatives,
    Unresolved,
    Conformance,
    Bounds,
    Done,
}
/// Shared reference-domain qualification. Member inventories, runtime policies,
/// alternatives, unresolved candidates and raw bounds advance individually.
/// Supply the same immutable schema throughout; errors poison the preparation.
pub struct ReferenceConformancePreparation {
    receiver: String,
    name: String,
    actual: String,
    owner: String,
    target: String,
    stage: ReferenceStage,
    members: Option<EffectiveFeaturePreparation>,
    scan: Option<std::vec::IntoIter<SelectedFeature>>,
    selected: Option<SelectedFeature>,
    domain: usize,
    item: usize,
    matched_domain: bool,
    conformance: Option<ConformancePreparation>,
    accepted: bool,
    failed: bool,
}
impl ReferenceConformancePreparation {
    pub fn new(receiver: &str, name: &str, actual: &str) -> Self {
        Self::with_members(
            receiver,
            name,
            actual,
            EffectiveFeaturePreparation::new(receiver),
        )
    }
    pub fn new_observed(
        receiver: &str,
        name: &str,
        actual: &str,
        observation: &InvocationContext<'_>,
    ) -> Result<Self, PreparationError> {
        reserve(observation, 0, &[receiver, name, actual])?;
        let members = EffectiveFeaturePreparation::new_observed(receiver, observation)?;
        Ok(Self::with_members(receiver, name, actual, members))
    }
    /// Crate-private initializer for a source selection already checked by the
    /// fixed live-metadata pending audit. This does not accept public lookup or
    /// serialized proof authority; all target/domain checks still run below.
    pub(crate) fn from_selected_source_observed(
        receiver: &str,
        name: &str,
        actual: &str,
        selected_owner: &str,
        declared_reference: &Feature,
        observation: &InvocationContext<'_>,
    ) -> Result<Self, PreparationError> {
        observation.work(1).map_err(PreparationError::Observation)?;
        if declared_reference.name != name || !declared_reference.reference {
            return Err("Selected source must be the named reference declaration".into());
        }
        reserve(
            observation,
            0,
            &[
                receiver,
                name,
                actual,
                selected_owner,
                &declared_reference.target,
            ],
        )?;
        Ok(Self {
            receiver: receiver.into(),
            name: name.into(),
            actual: actual.into(),
            owner: selected_owner.into(),
            target: declared_reference.target.clone(),
            stage: ReferenceStage::Domains,
            members: None,
            scan: None,
            selected: None,
            domain: 0,
            item: 0,
            matched_domain: false,
            conformance: None,
            accepted: false,
            failed: false,
        })
    }
    fn with_members(
        receiver: &str,
        name: &str,
        actual: &str,
        members: EffectiveFeaturePreparation,
    ) -> Self {
        Self {
            receiver: receiver.into(),
            name: name.into(),
            actual: actual.into(),
            owner: String::new(),
            target: String::new(),
            stage: ReferenceStage::Members,
            members: Some(members),
            scan: None,
            selected: None,
            domain: 0,
            item: 0,
            matched_domain: false,
            conformance: None,
            accepted: false,
            failed: false,
        }
    }
    pub fn advance(
        &mut self,
        schema: &crate::definitions::schema::SchemaRepresentation<impl super::RuntimeAlternative>,
    ) -> Result<bool, String> {
        self.advance_observed(schema, &InvocationContext::default())
            .map_err(|error| error.to_string())
    }
    pub fn advance_observed(
        &mut self,
        schema: &crate::definitions::schema::SchemaRepresentation<impl super::RuntimeAlternative>,
        observation: &InvocationContext<'_>,
    ) -> Result<bool, PreparationError> {
        if self.failed {
            return Err("Reference conformance preparation has failed".into());
        }
        let result = observation
            .checkpoint()
            .map_err(PreparationError::Observation)
            .and_then(|()| self.advance_inner(schema, observation));
        if result.is_err() {
            self.failed = true;
        }
        result
    }
    fn advance_inner(
        &mut self,
        schema: &crate::definitions::schema::SchemaRepresentation<impl super::RuntimeAlternative>,
        observation: &InvocationContext<'_>,
    ) -> Result<bool, PreparationError> {
        match self.stage {
            ReferenceStage::Members => {
                if self
                    .members
                    .as_mut()
                    .ok_or("Missing reference member preparation")?
                    .advance_observed(schema, observation)?
                {
                    self.scan = Some(
                        self.members
                            .take()
                            .ok_or("Missing reference member preparation")?
                            .finish()?
                            .into_iter(),
                    );
                    self.stage = ReferenceStage::Scan;
                }
            }
            ReferenceStage::Scan => {
                if let Some(member) = self
                    .scan
                    .as_mut()
                    .ok_or("Missing reference member scan")?
                    .next()
                {
                    if member.name == self.name {
                        self.selected = Some(member);
                    }
                } else {
                    self.scan = None;
                    let selected = self.selected.take().ok_or_else(|| {
                        format!("Unknown feature {}.{}", self.receiver, self.name)
                    })?;
                    let field = schema
                        .classes
                        .get(&selected.owner)
                        .and_then(|class| class.features.get(&self.name))
                        .ok_or("Missing reference feature")?;
                    if !field.reference {
                        return Err("Expected a reference feature".into());
                    }
                    reserve(observation, 0, &[&field.target])?;
                    self.owner = selected.owner;
                    self.target = field.target.clone();
                    self.stage = ReferenceStage::Domains;
                }
            }
            ReferenceStage::Domains => {
                if let Some(domain) = schema
                    .runtime_feature_domains
                    .get(&self.receiver)
                    .and_then(|domains| domains.get(self.domain))
                {
                    if domain.source.view().projection.source.name != self.name {
                        self.domain += 1;
                    } else if !schema.classes.contains_key(&self.actual) {
                        self.stage = ReferenceStage::Done;
                    } else {
                        self.matched_domain = true;
                        self.item = 0;
                        self.stage = ReferenceStage::Alternatives;
                    }
                } else if self.matched_domain {
                    self.accepted = true;
                    self.stage = ReferenceStage::Done;
                } else {
                    self.conformance = Some(ConformancePreparation::new_observed(
                        &self.actual,
                        &self.target,
                        observation,
                    )?);
                    self.stage = ReferenceStage::Conformance;
                }
            }
            ReferenceStage::Alternatives | ReferenceStage::Unresolved => {
                let domain = schema
                    .runtime_feature_domains
                    .get(&self.receiver)
                    .and_then(|domains| domains.get(self.domain))
                    .ok_or("Missing reference runtime policy")?;
                if matches!(self.stage, ReferenceStage::Alternatives) {
                    if let Some(target) = domain.alternatives.get(self.item).map(AsRef::as_ref) {
                        if target == self.actual.as_str() {
                            self.domain += 1;
                            self.stage = ReferenceStage::Domains;
                        } else {
                            self.item += 1;
                        }
                    } else {
                        self.item = 0;
                        self.stage = ReferenceStage::Unresolved;
                    }
                } else if let Some(candidate) = domain.unresolved.get(self.item) {
                    if candidate.target == self.actual {
                        return Err(format!(
                            "Unresolved runtime feature membership: {}",
                            candidate.reason
                        )
                        .into());
                    }
                    self.item += 1;
                } else {
                    self.stage = ReferenceStage::Done;
                }
            }
            ReferenceStage::Conformance => {
                let result = self
                    .conformance
                    .as_mut()
                    .ok_or("Missing reference class conformance")?
                    .advance_observed(schema, observation);
                match result {
                    Ok(false) => {}
                    Ok(true) => {
                        if self
                            .conformance
                            .take()
                            .ok_or("Missing reference class conformance")?
                            .finish()?
                        {
                            self.stage = ReferenceStage::Bounds;
                        } else {
                            self.stage = ReferenceStage::Done;
                        }
                    }
                    Err(PreparationError::Schema(_)) => {
                        // The synchronous class predicate historically rejects
                        // invalid ancestry rather than exposing its diagnostic.
                        self.conformance = None;
                        self.stage = ReferenceStage::Done;
                    }
                    Err(error @ PreparationError::Observation(_)) => return Err(error),
                }
            }
            ReferenceStage::Bounds => {
                let bounds = schema
                    .raw_reference_bounds
                    .get(&self.receiver)
                    .and_then(|fields| fields.get(&self.name))
                    .or_else(|| {
                        schema
                            .raw_reference_bounds
                            .get(&self.owner)
                            .and_then(|fields| fields.get(&self.name))
                    });
                if let Some(target) = bounds.and_then(|targets| targets.get(self.item)) {
                    self.conformance = Some(ConformancePreparation::new_observed(
                        &self.actual,
                        target,
                        observation,
                    )?);
                    self.item += 1;
                    self.stage = ReferenceStage::Conformance;
                } else {
                    self.accepted = true;
                    self.stage = ReferenceStage::Done;
                }
            }
            ReferenceStage::Done => return Ok(true),
        }
        Ok(matches!(self.stage, ReferenceStage::Done))
    }
    pub fn finish(self) -> Result<bool, String> {
        self.completed_result()
    }
    // Recycle only an actually completed canonical cursor inside one private,
    // fixed-schema audit. No public result/admission authority is introduced.
    pub(crate) fn completed_result(&self) -> Result<bool, String> {
        if self.failed || !matches!(self.stage, ReferenceStage::Done) {
            return Err("Reference conformance preparation is not complete".into());
        }
        Ok(self.accepted)
    }
    pub(crate) fn selection(&self) -> (&str, &str, &str) {
        (&self.receiver, &self.name, &self.actual)
    }
}
impl AncestryPreparation {
    pub(super) fn new(kind: &str) -> Self {
        let mut value = Self::empty();
        value.pending.push(AncestryTask::Enter(kind.into()));
        value
    }
    pub(super) fn new_observed(
        kind: &str,
        observation: &InvocationContext<'_>,
    ) -> Result<Self, PreparationError> {
        reserve(observation, 32, &[kind])?;
        let mut value = Self::empty();
        allocation(&mut value.pending, 1)?;
        value.pending.push(AncestryTask::Enter(kind.into()));
        Ok(value)
    }
    fn empty() -> Self {
        Self {
            pending: Vec::new(),
            active: BTreeSet::new(),
            complete: BTreeSet::new(),
            direct: BTreeSet::new(),
            ordered: Vec::new(),
            done: false,
            failed: false,
        }
    }
    #[cfg(test)]
    pub(super) fn advance(
        &mut self,
        schema: &crate::definitions::schema::SchemaRepresentation<impl super::RuntimeAlternative>,
    ) -> Result<bool, String> {
        self.advance_observed(schema, &InvocationContext::default())
            .map_err(|error| error.to_string())
    }
    pub(super) fn advance_observed(
        &mut self,
        schema: &crate::definitions::schema::SchemaRepresentation<impl super::RuntimeAlternative>,
        observation: &InvocationContext<'_>,
    ) -> Result<bool, PreparationError> {
        if self.failed {
            return Err("Ancestry preparation has failed".into());
        }
        let result = observation
            .checkpoint()
            .map_err(PreparationError::Observation)
            .and_then(|()| self.advance_inner(schema, observation));
        if result.is_err() {
            self.failed = true;
        }
        result
    }
    fn advance_inner(
        &mut self,
        schema: &crate::definitions::schema::SchemaRepresentation<impl super::RuntimeAlternative>,
        observation: &InvocationContext<'_>,
    ) -> Result<bool, PreparationError> {
        if self.done {
            return Ok(true);
        }
        let Some(task) = self.pending.pop() else {
            self.complete.pop_first();
            self.done = self.complete.is_empty();
            return Ok(self.done);
        };
        match task {
            AncestryTask::Enter(name) => {
                if !self.complete.contains(&name) {
                    if self.active.contains(&name) {
                        return Err(format!("Cyclic inheritance at {name}").into());
                    }
                    if !schema.classes.contains_key(&name) {
                        return Err(format!("Unknown class {name}").into());
                    }
                    reserve(observation, 32, &[&name])?;
                    allocation(&mut self.pending, 1)?;
                    self.active.insert(name.clone());
                    self.pending.push(AncestryTask::Check(name, 0));
                }
            }
            AncestryTask::Check(name, index) => {
                let class = schema.classes.get(&name).ok_or("Missing class")?;
                if let Some(parent) = class.super_type_at(index) {
                    if self.direct.contains(parent) {
                        return Err(format!("Duplicate supertype {parent} of {name}").into());
                    }
                    reserve(observation, 32, &[parent])?;
                    allocation(&mut self.pending, 1)?;
                    self.direct.insert(parent.into());
                    self.pending.push(AncestryTask::Check(name, index + 1));
                } else {
                    reserve(observation, 32, &[])?;
                    allocation(&mut self.pending, 1)?;
                    self.pending.push(AncestryTask::Clear(name));
                }
            }
            AncestryTask::Clear(name) => {
                self.direct.pop_first();
                if self.direct.is_empty() {
                    reserve(observation, 64, &[&name])?;
                    allocation(&mut self.pending, 2)?;
                    self.pending.push(AncestryTask::Exit(name.clone()));
                    self.pending.push(AncestryTask::Parents(name, 0));
                } else {
                    reserve(observation, 32, &[])?;
                    allocation(&mut self.pending, 1)?;
                    self.pending.push(AncestryTask::Clear(name));
                }
            }
            AncestryTask::Parents(name, index) => {
                let class = schema.classes.get(&name).ok_or("Missing class")?;
                if let Some(parent) = class.super_type_at(index) {
                    reserve(observation, 64, &[parent])?;
                    allocation(&mut self.pending, 2)?;
                    self.pending.push(AncestryTask::Parents(name, index + 1));
                    self.pending.push(AncestryTask::Enter(parent.into()));
                }
            }
            AncestryTask::Exit(name) => {
                reserve(observation, 32, &[&name])?;
                allocation(&mut self.ordered, 1)?;
                self.active.remove(&name);
                self.complete.insert(name.clone());
                self.ordered.push(name);
            }
        }
        Ok(false)
    }
    pub(super) fn finish(self) -> Result<Vec<String>, String> {
        if self.failed || !self.done {
            return Err("Ancestry preparation is not complete".into());
        }
        Ok(self.ordered)
    }
}

/// Selection data from an immutable schema; it does not grant authority to a
/// different or mutated schema. Resolve against the same schema used to prepare.
pub struct SelectedFeature {
    pub owner: String,
    pub name: String,
    pub identity: MemberIdentity,
    pub position: usize,
}
impl SelectedFeature {
    pub fn resolve(
        self,
        schema: &crate::definitions::schema::SchemaRepresentation<impl super::RuntimeAlternative>,
    ) -> Result<EffectiveFeature<'_>, String> {
        let (owner, feature) = resolve_selected_feature(schema, &self.owner, &self.name)?;
        Ok(EffectiveFeature {
            owner,
            feature,
            identity: self.identity,
            position: self.position,
        })
    }
}

// Complete immutable identities only; selection and inventory checks remain fresh.
#[derive(Clone, Debug)]
pub(crate) enum FeatureIdentityInput {
    Owned(MemberIdentity),
    Shared(std::sync::Arc<MemberIdentity>),
}
impl std::ops::Deref for FeatureIdentityInput {
    type Target = MemberIdentity;
    fn deref(&self) -> &MemberIdentity {
        match self {
            Self::Owned(value) => value,
            Self::Shared(value) => value,
        }
    }
}
impl FeatureIdentityInput {
    pub(crate) fn clone_observed(
        &self,
        observation: &InvocationContext<'_>,
    ) -> Result<Self, String> {
        match self {
            Self::Owned(value) => {
                crate::definitions::retained_storage::reserve_serialized(value, observation)?;
            }
            Self::Shared(_) => {
                observation.work(1)?;
                observation.retain_storage(32)?;
            }
        }
        observation.checkpoint()?;
        Ok(self.clone())
    }
    pub(crate) fn into_owned_observed(
        self,
        observation: &InvocationContext<'_>,
    ) -> Result<MemberIdentity, String> {
        match self {
            Self::Owned(value) => Ok(value),
            Self::Shared(value) => {
                crate::definitions::retained_storage::reserve_serialized(
                    value.as_ref(),
                    observation,
                )?;
                observation.checkpoint()?;
                Ok((*value).clone())
            }
        }
    }
}
enum FeatureNameInput {
    Owned(String),
    Shared(std::sync::Arc<str>),
}
impl FeatureNameInput {
    fn as_str(&self) -> &str {
        match self {
            Self::Owned(value) => value,
            Self::Shared(value) => value,
        }
    }
}
pub(crate) struct SelectedFeatureInput {
    owner: FeatureOwnerInput,
    name: FeatureNameInput,
    identity: FeatureIdentityInput,
    position: usize,
}
impl SelectedFeatureInput {
    pub(crate) fn from_owned(value: SelectedFeature) -> Self {
        Self {
            owner: FeatureOwnerInput::Owned(value.owner),
            name: FeatureNameInput::Owned(value.name),
            identity: FeatureIdentityInput::Owned(value.identity),
            position: value.position,
        }
    }
    pub(crate) fn owner(&self) -> &str {
        self.owner.as_str()
    }
    pub(crate) fn name(&self) -> &str {
        self.name.as_str()
    }
    pub(crate) fn identity(&self) -> &FeatureIdentityInput {
        &self.identity
    }
}
enum SelectedFeatureInventory {
    Owned(Vec<SelectedFeature>),
    Inputs(Vec<SelectedFeatureInput>),
}
impl SelectedFeatureInventory {
    fn len_observed(
        &self,
        _observation: &InvocationContext<'_>,
    ) -> Result<usize, PreparationError> {
        Ok(match self {
            Self::Owned(values) => values.len(),
            Self::Inputs(values) => values.len(),
        })
    }
    fn identity_observed<'a>(
        &'a self,
        index: usize,
        _observation: &InvocationContext<'_>,
    ) -> Result<Option<&'a MemberIdentity>, PreparationError> {
        Ok(match self {
            Self::Owned(values) => values.get(index).map(|value| &value.identity),
            Self::Inputs(values) => values.get(index).map(|value| &*value.identity),
        })
    }
    fn reserve_one(
        &mut self,
        _observation: &InvocationContext<'_>,
    ) -> Result<(), PreparationError> {
        match self {
            Self::Owned(values) => allocation(values, 1),
            Self::Inputs(values) => allocation(values, 1),
        }
    }
    fn push(&mut self, value: SelectedFeatureInput) -> Result<(), PreparationError> {
        match self {
            Self::Inputs(values) => values.push(value),
            Self::Owned(values) => {
                let FeatureIdentityInput::Owned(identity) = value.identity else {
                    return Err("Shared identity requires complete input inventory".into());
                };
                let FeatureOwnerInput::Owned(owner) = value.owner else {
                    return Err("Shared owner requires complete input inventory".into());
                };
                let FeatureNameInput::Owned(name) = value.name else {
                    return Err("Shared name requires complete input inventory".into());
                };
                values.push(SelectedFeature {
                    owner,
                    name,
                    identity,
                    position: value.position,
                });
            }
        }
        Ok(())
    }
    fn into_owned(self) -> Result<Vec<SelectedFeature>, String> {
        match self {
            Self::Owned(values) => Ok(values),
            Self::Inputs(_) => Err("Selected input inventory requires input publication".into()),
        }
    }
    fn into_scan(self) -> Result<SelectedFeatureScan, String> {
        Ok(match self {
            Self::Owned(values) => SelectedFeatureScan::Owned(values.into_iter()),
            Self::Inputs(values) => SelectedFeatureScan::Inputs(values.into_iter()),
        })
    }
}
enum SelectedFeatureScan {
    Owned(std::vec::IntoIter<SelectedFeature>),
    Inputs(std::vec::IntoIter<SelectedFeatureInput>),
}
impl SelectedFeatureScan {
    fn next(&mut self) -> Option<SelectedFeatureInput> {
        match self {
            Self::Inputs(values) => values.next(),
            Self::Owned(values) => values.next().map(|value| SelectedFeatureInput {
                owner: FeatureOwnerInput::Owned(value.owner),
                name: FeatureNameInput::Owned(value.name),
                identity: FeatureIdentityInput::Owned(value.identity),
                position: value.position,
            }),
        }
    }
}
fn resolve_selected_feature<'a>(
    schema: &'a crate::definitions::schema::SchemaRepresentation<impl super::RuntimeAlternative>,
    owner: &str,
    name: &str,
) -> Result<(&'a str, &'a Feature), String> {
    let (owner, class) = schema.classes.get_key_value(owner).ok_or("Missing class")?;
    let feature = class.features.get(name).ok_or("Missing feature")?;
    Ok((owner.as_str(), feature))
}

enum Stage {
    Ancestry,
    Header,
    Order,
    Declarations,
    OrderClear,
    Members,
    NamesClear,
    IdentitiesClear,
    OwnersClear,
    Done,
}
#[derive(Default)]
struct Names {
    index: usize,
    last: Option<String>,
}
impl Names {
    fn next<'a>(
        &mut self,
        class: &'a super::Class,
        observation: &InvocationContext<'_>,
    ) -> Result<Option<&'a str>, PreparationError> {
        let name = if class.feature_order.is_empty() {
            match &self.last {
                Some(last) => class
                    .features
                    .range::<str, _>((Excluded(last.as_str()), Unbounded))
                    .next(),
                None => class.features.iter().next(),
            }
            .map(|(name, _)| name.as_str())
        } else {
            class.feature_order.get(self.index).map(String::as_str)
        };
        if let Some(name) = name {
            reserve(observation, 0, &[name])?;
        }
        self.index += 1;
        self.last = name.map(str::to_owned);
        Ok(name)
    }
}

/// Shared effective-member construction, advancing one inheritance edge,
/// inventory entry, member or retained bookkeeping key at a time. Keep the
/// schema immutable until completion; errors poison this preparation.
pub struct EffectiveFeaturePreparation {
    input_cache: Option<SharedFeatureInputCache>,
    identity_views: bool,
    identity_positions: Vec<usize>,
    previous_position: Option<usize>,
    kind: String,
    stage: Stage,
    ancestry: Option<AncestryPreparation>,
    owners: Vec<String>,
    owner: usize,
    cursor: Names,
    declaration: Option<String>,
    order: BTreeSet<String>,
    names: BTreeMap<String, usize>,
    identities: BTreeSet<DeclarationId>,
    previous: Option<DeclarationId>,
    members: SelectedFeatureInventory,
    failed: bool,
}
impl EffectiveFeaturePreparation {
    pub fn new(kind: &str) -> Self {
        Self::with_ancestry(kind, AncestryPreparation::new(kind))
    }
    /// Precharge owned receiver/ancestry inputs. Keep the same immutable schema
    /// and operation observation for every advance.
    pub fn new_observed(
        kind: &str,
        observation: &InvocationContext<'_>,
    ) -> Result<Self, PreparationError> {
        reserve(observation, 0, &[kind])?;
        let ancestry = AncestryPreparation::new_observed(kind, observation)?;
        Ok(Self::with_ancestry(kind, ancestry))
    }
    fn with_ancestry(kind: &str, ancestry: AncestryPreparation) -> Self {
        Self {
            input_cache: None,
            identity_views: false,
            identity_positions: Vec::new(),
            previous_position: None,
            kind: kind.into(),
            stage: Stage::Ancestry,
            ancestry: Some(ancestry),
            owners: Vec::new(),
            owner: 0,
            cursor: Names::default(),
            declaration: None,
            order: BTreeSet::new(),
            names: BTreeMap::new(),
            identities: BTreeSet::new(),
            previous: None,
            members: SelectedFeatureInventory::Owned(Vec::new()),
            failed: false,
        }
    }
    pub fn advance(
        &mut self,
        schema: &crate::definitions::schema::SchemaRepresentation<impl super::RuntimeAlternative>,
    ) -> Result<bool, String> {
        self.advance_observed(schema, &InvocationContext::default())
            .map_err(|error| error.to_string())
    }
    pub fn advance_observed(
        &mut self,
        schema: &crate::definitions::schema::SchemaRepresentation<impl super::RuntimeAlternative>,
        observation: &InvocationContext<'_>,
    ) -> Result<bool, PreparationError> {
        if self.failed {
            return Err("Effective feature preparation has failed".into());
        }
        let result = observation
            .checkpoint()
            .map_err(PreparationError::Observation)
            .and_then(|()| self.advance_inner(schema, observation));
        if result.is_err() {
            self.failed = true;
        }
        result
    }
    // A pure index-order policy, no accepted metadata/model authority. The
    // default worker retains its original binary-search path and fees.
    pub(crate) fn advance_sources_observed<S: DefaultSourceInputs>(
        &mut self,
        schema: &crate::definitions::schema::SchemaRepresentation<impl super::RuntimeAlternative>,
        observation: &InvocationContext<'_>,
        sources: &S,
    ) -> Result<bool, PreparationError> {
        if self.failed {
            return Err("Effective feature preparation has failed".into());
        }
        let result = observation
            .checkpoint()
            .map_err(PreparationError::Observation)
            .and_then(|()| {
                self.advance_inner_with_sources(
                    schema,
                    observation,
                    S::ASCENDING_IDENTITY_APPEND,
                    sources,
                )
            });
        if result.is_err() {
            self.failed = true;
        }
        result
    }
    fn advance_inner(
        &mut self,
        schema: &crate::definitions::schema::SchemaRepresentation<impl super::RuntimeAlternative>,
        observation: &InvocationContext<'_>,
    ) -> Result<bool, PreparationError> {
        self.advance_inner_with_sources(schema, observation, false, &NoDefaultSourceInputs)
    }
    fn advance_inner_with_sources<S: DefaultSourceInputs>(
        &mut self,
        schema: &crate::definitions::schema::SchemaRepresentation<impl super::RuntimeAlternative>,
        observation: &InvocationContext<'_>,
        append_identity: bool,
        sources: &S,
    ) -> Result<bool, PreparationError> {
        match self.stage {
            Stage::Ancestry => {
                if self
                    .ancestry
                    .as_mut()
                    .ok_or("Missing ancestry preparation")?
                    .advance_observed(schema, observation)?
                {
                    self.owners = self
                        .ancestry
                        .take()
                        .ok_or("Missing ancestry preparation")?
                        .finish()?;
                    self.stage = Stage::Header;
                }
            }
            Stage::Header => {
                let Some(owner) = self.owners.get(self.owner) else {
                    self.stage = Stage::NamesClear;
                    return Ok(false);
                };
                let class = schema.classes.get(owner).ok_or("Missing class")?;
                if S::REUSE_OWNER_INVENTORY
                    && let Some(cache) = self.input_cache.as_ref()
                    && sources.owner_inventory_complete(schema, cache, owner, observation)?
                {
                    // Only the class-local order/declaration inventory is paid
                    // and immutable. Receiver-wide name/source-identity checks
                    // and each field/default projection still run in Members.
                    self.cursor = Names::default();
                    self.declaration = None;
                    self.previous = None;
                    self.previous_position = None;
                    self.stage = Stage::Members;
                    return Ok(false);
                }
                if (schema.declarations.contains_key(owner)
                    && (class.feature_order.len() != class.features.len()
                        || class.feature_declarations.len() != class.features.len()))
                    || (!class.feature_order.is_empty()
                        && class.feature_order.len() != class.features.len())
                    || (!class.feature_declarations.is_empty()
                        && (class.feature_declarations.len() != class.features.len()
                            || class.feature_order.is_empty()))
                {
                    return Err(format!("Invalid feature declaration inventory for {owner}").into());
                }
                self.cursor = Names::default();
                self.declaration = None;
                self.previous = None;
                self.previous_position = None;
                self.stage = Stage::Order;
            }
            Stage::Order | Stage::Declarations | Stage::Members => {
                let owner = self.owners.get(self.owner).ok_or("Missing member owner")?;
                let class = schema.classes.get(owner).ok_or("Missing class")?;
                if matches!(self.stage, Stage::Declarations) {
                    let next = match &self.declaration {
                        Some(last) => class
                            .feature_declarations
                            .range::<str, _>((Excluded(last.as_str()), Unbounded))
                            .next(),
                        None => class.feature_declarations.iter().next(),
                    };
                    if let Some((name, _)) = next {
                        if !class.features.contains_key(name) {
                            return Err(format!(
                                "Invalid feature declaration inventory for {owner}"
                            )
                            .into());
                        }
                        reserve(observation, 0, &[name])?;
                        self.declaration = Some(name.clone());
                    } else {
                        self.stage = Stage::OrderClear;
                    }
                } else if let Some(name) = self.cursor.next(class, observation)? {
                    if matches!(self.stage, Stage::Order) {
                        if !class.features.contains_key(name) || self.order.contains(name) {
                            return Err(format!(
                                "Invalid feature declaration inventory for {owner}"
                            )
                            .into());
                        }
                        reserve(observation, 0, &[name])?;
                        self.order.insert(name.into());
                    } else {
                        let feature = class.features.get(name).ok_or("Missing feature")?;
                        if feature.name != name {
                            return Err(format!("Feature name/key mismatch: {owner}.{name}").into());
                        }
                        let identity = match class.feature_declarations.get(name) {
                            Some(declaration) => {
                                schema.validate_member_declaration(owner, declaration)?;
                                if self.identity_views {
                                    let selected_declaration =
                                        |index: usize| -> Result<&DeclarationId, PreparationError> {
                                            match self
                                                .members
                                                .identity_observed(index, observation)?
                                            {
                                                Some(MemberIdentity::Source { declaration }) => {
                                                    Ok(declaration)
                                                }
                                                _ => {
                                                    Err("Missing selected feature declaration"
                                                        .into())
                                                }
                                            }
                                        };
                                    // The index is already sorted by complete DeclarationId.
                                    // Pay the actual maximum comparison before append reuse;
                                    // lower/equal IDs retain the exact binary-search fallback.
                                    let append = if append_identity && self.input_cache.is_some() {
                                        if self.identity_positions.is_empty() {
                                            true
                                        } else {
                                            observation
                                                .work(1)
                                                .map_err(PreparationError::Observation)?;
                                            let last = self
                                                .identity_positions
                                                .last()
                                                .ok_or("Missing sorted identity maximum")?;
                                            selected_declaration(*last)?.cmp(declaration)
                                                == std::cmp::Ordering::Less
                                        }
                                    } else {
                                        false
                                    };
                                    let mut low = if append {
                                        self.identity_positions.len()
                                    } else {
                                        0
                                    };
                                    let mut high = self.identity_positions.len();
                                    while low < high {
                                        observation
                                            .work(1)
                                            .map_err(PreparationError::Observation)?;
                                        let middle = low + (high - low) / 2;
                                        match selected_declaration(self.identity_positions[middle])?.cmp(declaration) {
                                            std::cmp::Ordering::Less => low=middle+1,
                                            std::cmp::Ordering::Greater => high=middle,
                                            std::cmp::Ordering::Equal => return Err(format!("Repeated feature declaration identity: {declaration:?}").into()),
                                        }
                                    }
                                    if let Some(previous) = self.previous_position {
                                        validate_declaration_order(
                                            selected_declaration(previous)?,
                                            declaration,
                                        )?;
                                    }
                                    // One complete owned identity remains in the selected result.
                                    // The index and previous cursor refer to that exact retained value.
                                    if self.input_cache.is_none() {
                                        reserve_value(declaration, observation)?;
                                    }
                                    reserve(observation, 32, &[])?;
                                    self.identity_positions.try_reserve_exact(1).map_err(|_| {
                                        PreparationError::Observation(
                                            "Schema preparation allocation failed".into(),
                                        )
                                    })?;
                                    observation
                                        .work(self.identity_positions.len() - low)
                                        .map_err(PreparationError::Observation)?;
                                    self.identity_positions
                                        .insert(low, self.members.len_observed(observation)?);
                                    self.previous_position =
                                        Some(self.members.len_observed(observation)?);
                                } else {
                                    if self.identities.contains(declaration) {
                                        return Err(format!(
                                            "Repeated feature declaration identity: {declaration:?}"
                                        )
                                        .into());
                                    }
                                    if let Some(previous) = &self.previous {
                                        validate_declaration_order(previous, declaration)?;
                                    }
                                    // Public owned continuation retains its established admission policy.
                                    for _ in 0..3 {
                                        reserve_value(declaration, observation)?;
                                    }
                                    self.identities.insert(declaration.clone());
                                    self.previous = Some(declaration.clone());
                                }
                                match &self.input_cache {
                                    Some(cache) => {
                                        FeatureInputCache::retain_source_identity_observed(
                                            cache,
                                            declaration,
                                            observation,
                                        )?
                                    }
                                    None => FeatureIdentityInput::Owned(MemberIdentity::Source {
                                        declaration: declaration.clone(),
                                    }),
                                }
                            }
                            None => {
                                reserve(
                                    observation,
                                    0,
                                    &[&schema.revision, &schema.uri, owner, name],
                                )?;
                                FeatureIdentityInput::Owned(MemberIdentity::Programmatic {
                                    revision: schema.revision.clone(),
                                    namespace: schema.uri.clone(),
                                    owner: owner.clone(),
                                    name: name.into(),
                                })
                            }
                        };
                        if let Some(previous) = self.names.get(name) {
                            return Err(format!(
                                "Unsupported inherited feature redefinition: {}.{name}; declarations {:?} and {:?}",
                                self.kind, self.members.identity_observed(*previous, observation)?.ok_or("Missing selected feature identity")?, *identity
                            ).into());
                        }
                        let (owner_input, name_input) = match &self.input_cache {
                            Some(cache) => {
                                reserve(observation, 32, &[name])?;
                                (
                                    FeatureInputCache::retain_owner_inventory_observed(
                                        cache,
                                        owner,
                                        observation,
                                        S::REUSE_OWNER_INVENTORY && self.cursor.index == 1,
                                    )?,
                                    FeatureNameInput::Shared(
                                        FeatureInputCache::retain_field_name_observed(
                                            cache,
                                            name,
                                            observation,
                                        )?,
                                    ),
                                )
                            }
                            None => {
                                reserve(observation, 32, &[name, name, owner])?;
                                (
                                    FeatureOwnerInput::Owned(owner.clone()),
                                    FeatureNameInput::Owned(name.into()),
                                )
                            }
                        };
                        self.members.reserve_one(observation)?;
                        self.names
                            .insert(name.into(), self.members.len_observed(observation)?);
                        self.members.push(SelectedFeatureInput {
                            owner: owner_input,
                            name: name_input,
                            identity,
                            position: self.members.len_observed(observation)?,
                        })?;
                    }
                } else if matches!(self.stage, Stage::Order) {
                    if self.order.len() != class.features.len() {
                        return Err(
                            format!("Invalid feature declaration inventory for {owner}").into()
                        );
                    }
                    self.stage = Stage::Declarations;
                } else {
                    self.owner += 1;
                    self.stage = Stage::Header;
                }
            }
            Stage::OrderClear => {
                self.order.pop_first();
                if self.order.is_empty() {
                    self.cursor = Names::default();
                    self.stage = Stage::Members;
                }
            }
            Stage::NamesClear => {
                self.names.pop_first();
                if self.names.is_empty() {
                    self.stage = Stage::IdentitiesClear;
                }
            }
            Stage::IdentitiesClear => {
                if self.identity_views {
                    self.identity_positions.pop();
                } else {
                    self.identities.pop_first();
                }
                if self.identities.is_empty() && self.identity_positions.is_empty() {
                    self.previous = None;
                    self.previous_position = None;
                    self.stage = Stage::OwnersClear;
                }
            }
            Stage::OwnersClear => {
                self.owners.pop();
                if self.owners.is_empty() {
                    self.stage = Stage::Done;
                }
            }
            Stage::Done => return Ok(true),
        }
        Ok(matches!(self.stage, Stage::Done))
    }
    pub fn finish(self) -> Result<Vec<SelectedFeature>, String> {
        if self.failed || !matches!(self.stage, Stage::Done) {
            return Err("Effective feature preparation is not complete".into());
        }
        self.members.into_owned()
    }
    fn finish_input_scan(self) -> Result<SelectedFeatureScan, String> {
        if self.failed || !matches!(self.stage, Stage::Done) {
            return Err("Effective feature preparation is not complete".into());
        }
        self.members.into_scan()
    }
}

/// An owned receiver feature projection built by the shared member/default
/// authorities. Keep the same immutable schema; this is not serialized proof.
pub struct PreparedFeatures {
    receiver: String,
    fields: BTreeMap<String, Feature>,
    owners: BTreeMap<String, String>,
    identities: BTreeMap<String, MemberIdentity>,
    order: Vec<String>,
}
impl PreparedFeatures {
    pub fn receiver(&self) -> &str {
        &self.receiver
    }
    pub fn fields(&self) -> &BTreeMap<String, Feature> {
        &self.fields
    }
    pub fn member_identities(&self) -> &BTreeMap<String, MemberIdentity> {
        &self.identities
    }
    pub fn order(&self) -> &[String] {
        &self.order
    }
    pub fn get(&self, name: &str) -> Option<&Feature> {
        self.fields.get(name)
    }
    pub fn owner(&self, name: &str) -> Option<&str> {
        self.owners.get(name).map(String::as_str)
    }
    /// Synchronous consumers take the fields; retained owner-map destruction is
    /// atomic. Staged consumers release this projection with `clear_one`.
    pub fn into_fields(self) -> BTreeMap<String, Feature> {
        self.fields
    }
    pub fn clear_one(&mut self) -> bool {
        if !self.fields.is_empty() {
            self.fields.pop_first();
        } else {
            if !self.owners.is_empty() {
                self.owners.pop_first();
            } else if !self.identities.is_empty() {
                self.identities.pop_first();
            } else {
                self.order.pop();
            }
        }
        self.fields.is_empty()
            && self.owners.is_empty()
            && self.identities.is_empty()
            && self.order.is_empty()
    }
}
mod feature_lookup_sealed {
    pub trait Sealed {}
    impl Sealed for super::PreparedFeatures {}
    impl Sealed for super::PreparedFeatureInputs {}
}
/// Immutable feature lookup produced by the shared member/default preparation.
/// Implementations are sealed; this is metadata, not schema admission authority.
pub trait PreparedFeatureLookup: feature_lookup_sealed::Sealed {
    fn receiver(&self) -> &str;
    fn get(&self, name: &str) -> Option<&Feature>;
}
impl PreparedFeatureLookup for PreparedFeatures {
    fn receiver(&self) -> &str {
        PreparedFeatures::receiver(self)
    }
    fn get(&self, name: &str) -> Option<&Feature> {
        PreparedFeatures::get(self, name)
    }
}
impl PreparedFeatureLookup for PreparedFeatureInputs {
    fn receiver(&self) -> &str {
        PreparedFeatureInputs::receiver(self)
    }
    fn get(&self, name: &str) -> Option<&Feature> {
        PreparedFeatureInputs::get(self, name)
    }
}
// Shared text is presentation input only, never selected member authority.
enum FeatureOwnerInput {
    Owned(String),
    Shared(std::sync::Arc<String>),
}
impl FeatureOwnerInput {
    fn as_str(&self) -> &str {
        match self {
            Self::Owned(value) => value,
            Self::Shared(value) => value.as_str(),
        }
    }
    fn into_owned(self) -> String {
        match self {
            Self::Owned(value) => value,
            Self::Shared(value) => match std::sync::Arc::try_unwrap(value) {
                Ok(value) => value,
                Err(value) => value.as_ref().clone(),
            },
        }
    }
}
struct PreparedInputFeature {
    // Ordinal in THIS complete input row, never a packed object storage slot.
    position: usize,
    field: FeatureInput,
    owner: FeatureOwnerInput,
    identity: FeatureIdentityInput,
}
pub struct PreparedInputFeatures {
    receiver: String,
    fields: BTreeMap<std::sync::Arc<str>, PreparedInputFeature>,
    order: Vec<std::sync::Arc<str>>,
}
/// Complete immutable member/default inputs, retaining shared source payloads.
/// Keep the producing schema immutable while using these inputs.
pub enum PreparedFeatureInputs {
    Owned(PreparedFeatures),
    Shared(PreparedInputFeatures),
}
impl PreparedFeatureInputs {
    /// Visit the SAME complete immutable field map in canonical name order.
    /// This is paid DATA traversal, never schema/provider/source admission.
    /// One real BTree iterator replaces repeated exclusive-range lookups.
    pub(crate) fn visit_fields_observed(
        &self,
        observation: &InvocationContext<'_>,
        mut visit: impl FnMut(&str, &Feature) -> Result<(), String>,
    ) -> Result<(), String> {
        const CURSOR: usize = 128;
        const {
            assert!(
                std::mem::size_of::<std::collections::btree_map::Iter<'_, String, Feature>>()
                    <= CURSOR
            );
            assert!(
                std::mem::size_of::<
                    std::collections::btree_map::Iter<
                        '_,
                        std::sync::Arc<str>,
                        PreparedInputFeature,
                    >,
                >() <= CURSOR
            );
        }
        let count = match self {
            Self::Owned(value) => value.fields.len(),
            Self::Shared(value) => value.fields.len(),
        };
        // Pay complete ordered traversal before constructing its cursor:
        // four units per row cover amortized edge traversal, advance and map;
        // two height descents cover opening/exhaustion, plus fixed setup.
        // No refund on cancellation/short-work/storage/consumer failure.
        let height = (usize::BITS - count.leading_zeros()) as usize;
        let traversal = count
            .checked_mul(4)
            .and_then(|units| height.checked_mul(2).and_then(|h| units.checked_add(h)))
            .and_then(|units| units.checked_add(4))
            .ok_or("Prepared field traversal work overflow")?;
        observation.checkpoint()?;
        observation.work(traversal)?;
        observation.retain_storage(CURSOR)?;
        observation.checkpoint()?;
        fn walk<'a>(
            mut cursor: impl Iterator<Item = (&'a str, &'a Feature)>,
            observation: &InvocationContext<'_>,
            visit: &mut impl FnMut(&str, &Feature) -> Result<(), String>,
        ) -> Result<(), String> {
            loop {
                // Includes the final observer check after the last consumer.
                observation.checkpoint()?;
                let Some((name, field)) = cursor.next() else {
                    return Ok(());
                };
                visit(name, field)?;
            }
        }
        match self {
            Self::Owned(value) => walk(
                value
                    .fields
                    .iter()
                    .map(|(name, field)| (name.as_str(), field)),
                observation,
                &mut visit,
            ),
            Self::Shared(value) => walk(
                value
                    .fields
                    .iter()
                    .map(|(name, input)| (name.as_ref(), &*input.field)),
                observation,
                &mut visit,
            ),
        }
    }
    // The existing ordered-map fee policy, using the actual immutable map size.
    // This admits the exact borrowed probes below; no full inventory pre-scan,
    // additional index, source authority, or independently selected field.
    pub(crate) fn admit_lookup_observed(
        &self,
        name: &str,
        probes: usize,
        observation: &InvocationContext<'_>,
    ) -> Result<(), String> {
        let count = match self {
            Self::Owned(value) => value.fields.len(),
            Self::Shared(value) => value.fields.len(),
        };
        let height = (usize::BITS - count.leading_zeros()) as usize;
        let units = height
            .checked_mul(1 + name.len().div_ceil(64))
            .and_then(|units| units.checked_add(1))
            .and_then(|units| units.checked_mul(probes))
            .ok_or("Prepared member lookup work overflow")?;
        observation.work(units)
    }
    // Borrow both member inputs from ONE actual immutable Shared row. Owned
    // inputs keep their two distinct maps and their existing two-probe fee.
    // This is source-neutral DATA access: no schema/provider admission or alias
    // authority follows from the returned lifetime-bound references.
    pub(crate) fn member_row_observed(
        &self,
        name: &str,
        observation: &InvocationContext<'_>,
    ) -> Result<Option<(&Feature, Option<&MemberIdentity>)>, String> {
        match self {
            Self::Owned(value) => {
                self.admit_lookup_observed(name, 2, observation)?;
                Ok(value
                    .get(name)
                    .map(|field| (field, value.identities.get(name))))
            }
            Self::Shared(value) => {
                self.admit_lookup_observed(name, 1, observation)?;
                Ok(value
                    .fields
                    .get(name)
                    .map(|row| (&*row.field, Some(&*row.identity))))
            }
        }
    }
    // Coverage starts empty and inserts only unique keys from this exact map,
    // so its ordered set has at most F entries. Preserve the existing ordered
    // key tariff (F and inserted UTF8 key), separately from linear cursor work.
    pub(crate) fn admit_coverage_insert_observed(
        &self,
        name: &str,
        observation: &InvocationContext<'_>,
    ) -> Result<(), String> {
        self.admit_lookup_observed(name, 1, observation)
    }
    pub fn receiver(&self) -> &str {
        match self {
            Self::Owned(value) => value.receiver(),
            Self::Shared(value) => &value.receiver,
        }
    }
    pub fn get(&self, name: &str) -> Option<&Feature> {
        match self {
            Self::Owned(value) => value.get(name),
            Self::Shared(value) => value.fields.get(name).map(|value| &*value.field),
        }
    }
    pub fn owner(&self, name: &str) -> Option<&str> {
        match self {
            Self::Owned(value) => value.owner(name),
            Self::Shared(value) => value.fields.get(name).map(|value| value.owner.as_str()),
        }
    }
    /// One effective field after an exclusive name cursor, without copying keys.
    pub fn next_field(&self, after: Option<&str>) -> Option<(&str, &Feature)> {
        match self {
            Self::Owned(value) => match after {
                Some(after) => value
                    .fields
                    .range::<str, _>((Excluded(after), Unbounded))
                    .next(),
                None => value.fields.iter().next(),
            }
            .map(|(name, value)| (name.as_str(), value)),
            Self::Shared(value) => match after {
                Some(after) => value
                    .fields
                    .range::<str, _>((Excluded(after), Unbounded))
                    .next(),
                None => value.fields.iter().next(),
            }
            .map(|(name, value)| (name.as_ref(), &*value.field)),
        }
    }
    pub fn member_identity(&self, name: &str) -> Option<&MemberIdentity> {
        match self {
            Self::Owned(value) => value.identities.get(name),
            Self::Shared(value) => value.fields.get(name).map(|value| &*value.identity),
        }
    }
    /// Resolve the canonical row ordinal by the existing shared name index.
    /// Owned compatibility rows retain their ordered scan. This is immutable
    /// descriptor DATA; it grants no model, provider or physical slot access.
    pub(crate) fn named_position_observed(
        &self,
        name: &str,
        observation: &InvocationContext<'_>,
    ) -> Result<Option<usize>, String> {
        observation.checkpoint()?;
        if let Self::Shared(value) = self {
            self.admit_lookup_observed(name, 1, observation)?;
            return Ok(value.fields.get(name).map(|row| row.position));
        }
        let mut position = 0;
        while let Some(candidate) = self.ordered_name(position) {
            observation.work(1 + candidate.len().max(name.len()).div_ceil(64))?;
            if candidate == name {
                return Ok(Some(position));
            }
            position = position
                .checked_add(1)
                .ok_or("Prepared member position overflow")?;
        }
        Ok(None)
    }
    pub fn ordered_name(&self, index: usize) -> Option<&str> {
        match self {
            Self::Owned(value) => value.order.get(index).map(String::as_str),
            Self::Shared(value) => value.order.get(index).map(|name| name.as_ref()),
        }
    }
    #[cfg(test)]
    pub(crate) fn member_identities(&self) -> BTreeMap<String, MemberIdentity> {
        match self {
            Self::Owned(value) => value.member_identities().clone(),
            Self::Shared(value) => value
                .fields
                .iter()
                .map(|(key, value)| (key.to_string(), (*value.identity).clone()))
                .collect(),
        }
    }
    #[cfg(test)]
    pub(crate) fn order(&self) -> Vec<String> {
        match self {
            Self::Owned(value) => value.order.clone(),
            Self::Shared(value) => value.order.iter().map(|name| name.to_string()).collect(),
        }
    }
    #[cfg(test)]
    fn fields_for_test(&self) -> BTreeMap<String, Feature> {
        self.field_iter()
            .map(|(name, value)| (name.to_owned(), value.clone()))
            .collect()
    }
    #[cfg(test)]
    fn owners_for_test(&self) -> BTreeMap<String, String> {
        match self {
            Self::Owned(value) => value.owners.clone(),
            Self::Shared(value) => value
                .fields
                .iter()
                .map(|(key, input)| (key.to_string(), input.owner.as_str().to_owned()))
                .collect(),
        }
    }
    #[cfg(test)]
    fn bookkeeping_len(&self) -> usize {
        match self {
            Self::Owned(value) => {
                value.fields.len() + value.owners.len() + value.identities.len() + value.order.len()
            }
            Self::Shared(value) => value.fields.len() + value.order.len(),
        }
    }
    #[cfg(test)]
    pub(crate) fn field_iter(&self) -> impl Iterator<Item = (&str, &Feature)> {
        let owned = match self {
            Self::Owned(value) => Some(value.fields()),
            _ => None,
        };
        let shared = match self {
            Self::Shared(value) => Some(&value.fields),
            _ => None,
        };
        owned
            .into_iter()
            .flat_map(|value| value.iter())
            .map(|(name, value)| (name.as_str(), value))
            .chain(
                shared
                    .into_iter()
                    .flat_map(|value| value.iter())
                    .map(|(name, value)| (name.as_ref(), &*value.field)),
            )
    }
    pub(crate) fn into_owned_observed(
        self,
        observation: &InvocationContext<'_>,
    ) -> Result<PreparedFeatures, PreparationError> {
        match self {
            Self::Owned(value) => Ok(value),
            Self::Shared(value) => {
                // Compatibility publication creates three independently owned
                // maps. Precharge every record, extra key and actual payload copy.
                reserve(observation, 128 * 3, &[])?;
                for (name, row) in &value.fields {
                    reserve(
                        observation,
                        128 * 3,
                        &[name.as_ref(), name.as_ref(), name.as_ref()],
                    )?;
                    if let FeatureInput::Shared(input) = &row.field
                        && std::sync::Arc::strong_count(input) > 1
                    {
                        reserve_value(input.as_ref(), observation)?;
                    }
                    if let FeatureIdentityInput::Shared(input) = &row.identity
                        && std::sync::Arc::strong_count(input) > 1
                    {
                        reserve_value(input.as_ref(), observation)?;
                    }
                    if let FeatureOwnerInput::Shared(input) = &row.owner
                        && std::sync::Arc::strong_count(input) > 1
                    {
                        reserve(observation, 0, &[input.as_str()])?;
                    }
                }
                for name in &value.order {
                    reserve(observation, 32, &[name.as_ref()])?;
                }
                let mut fields = BTreeMap::new();
                let mut owners = BTreeMap::new();
                let mut identities = BTreeMap::new();
                for (name, row) in value.fields {
                    let name = name.to_string();
                    let identity = match row.identity {
                        FeatureIdentityInput::Owned(value) => value,
                        FeatureIdentityInput::Shared(value) => {
                            match std::sync::Arc::try_unwrap(value) {
                                Ok(value) => value,
                                Err(value) => value.as_ref().clone(),
                            }
                        }
                    };
                    let field = match row.field {
                        FeatureInput::Owned(value) => value,
                        FeatureInput::Shared(value) => match std::sync::Arc::try_unwrap(value) {
                            Ok(value) => value,
                            Err(value) => value.as_ref().clone(),
                        },
                    };
                    identities.insert(name.clone(), identity);
                    owners.insert(name.clone(), row.owner.into_owned());
                    fields.insert(name, field);
                }
                Ok(PreparedFeatures {
                    receiver: value.receiver,
                    fields,
                    owners,
                    identities,
                    order: value.order.iter().map(|name| name.to_string()).collect(),
                })
            }
        }
    }
    pub fn clear_one(&mut self) -> bool {
        match self {
            Self::Owned(value) => value.clear_one(),
            Self::Shared(value) => {
                if !value.fields.is_empty() {
                    value.fields.pop_first();
                } else {
                    value.order.pop();
                }
                value.fields.is_empty() && value.order.is_empty()
            }
        }
    }
}
#[derive(Default)]
/// Bounded cache of complete source payloads and text; semantic decisions stay fresh.
/// Cache fields are private and no serialized admission evidence is accepted.
pub struct FeatureInputCache {
    inputs: BTreeMap<std::sync::Arc<MemberIdentity>, Vec<std::sync::Arc<Feature>>>,
    source_ids: BTreeMap<DeclarationId, std::sync::Arc<MemberIdentity>>,
    owner_names: BTreeMap<String, (std::sync::Arc<String>, bool)>,
    field_names: BTreeSet<std::sync::Arc<str>>,
    entries: usize,
}
pub type SharedFeatureInputCache = std::sync::Arc<std::sync::Mutex<FeatureInputCache>>;
impl FeatureInputCache {
    // One retained payload or bookkeeping key per successful validation advance.
    // Nested values and cancellation-path destruction remain separate obligations.
    pub fn clear_one(&mut self) -> Result<bool, PreparationError> {
        if let Some(mut entry) = self.inputs.first_entry() {
            if entry.get_mut().pop().is_some() {
                self.entries = self
                    .entries
                    .checked_sub(1)
                    .ok_or("Invalid feature input count during release")?;
            } else {
                entry.remove_entry();
            }
        } else if !self.source_ids.is_empty() {
            self.source_ids.pop_first();
        } else if !self.owner_names.is_empty() {
            self.owner_names.pop_first();
        } else {
            self.field_names.pop_first();
        }
        Ok(self.inputs.is_empty()
            && self.source_ids.is_empty()
            && self.owner_names.is_empty()
            && self.field_names.is_empty())
    }

    fn retain_field_name_observed(
        cache: &SharedFeatureInputCache,
        name: &str,
        observation: &InvocationContext<'_>,
    ) -> Result<std::sync::Arc<str>, PreparationError> {
        observation.work(1).map_err(PreparationError::Observation)?;
        let mut cache = cache.try_lock().map_err(|_| {
            PreparationError::Schema("Feature name cache is unavailable or already borrowed".into())
        })?;
        if let Some(value) = cache.field_names.get(name) {
            reserve(observation, 32, &[])?;
            return Ok(value.clone());
        }
        let retained = cache.field_names.len() < 4096;
        reserve(observation, if retained { 128 + 32 } else { 32 }, &[name])?;
        let value: std::sync::Arc<str> = std::sync::Arc::from(name);
        if retained {
            cache.field_names.insert(value.clone());
        }
        Ok(value)
    }
    fn retain_owner_name_observed(
        cache: &SharedFeatureInputCache,
        owner: &str,
        observation: &InvocationContext<'_>,
    ) -> Result<FeatureOwnerInput, PreparationError> {
        Self::retain_owner_inventory_observed(cache, owner, observation, false)
    }
    fn retain_owner_inventory_observed(
        cache: &SharedFeatureInputCache,
        owner: &str,
        observation: &InvocationContext<'_>,
        complete: bool,
    ) -> Result<FeatureOwnerInput, PreparationError> {
        // Existing fixed 32-byte handle/value admission covers the new private
        // bit on native and wasm; no map/header or public holder is added.
        const { assert!(std::mem::size_of::<(std::sync::Arc<String>, bool)>() <= 32) };
        observation
            .work(if complete { 2 } else { 1 })
            .map_err(PreparationError::Observation)?;
        let mut cache = cache.try_lock().map_err(|_| {
            PreparationError::Schema(
                "Feature owner-name cache is unavailable or already borrowed".into(),
            )
        })?;
        if let Some((value, certified)) = cache.owner_names.get_mut(owner) {
            reserve(observation, 32, &[])?;
            if complete && !*certified {
                *certified = true;
            }
            return Ok(FeatureOwnerInput::Shared(value.clone()));
        }
        if cache.owner_names.len() >= 4096 {
            reserve(observation, 0, &[owner])?;
            return Ok(FeatureOwnerInput::Owned(owner.to_owned()));
        }
        // Own both the lookup key and payload once. Every hot consumer retains
        // only an Arc handle; source/member/default resolution remains fresh.
        reserve(observation, 128 + 32, &[owner, owner])?;
        let value = std::sync::Arc::new(owner.to_owned());
        cache
            .owner_names
            .insert(owner.to_owned(), (value.clone(), complete));
        Ok(FeatureOwnerInput::Shared(value))
    }

    pub(crate) fn owner_inventory_complete_observed(
        cache: &SharedFeatureInputCache,
        owner: &str,
        observation: &InvocationContext<'_>,
    ) -> Result<bool, PreparationError> {
        observation.work(1).map_err(PreparationError::Observation)?;
        let cache = cache.try_lock().map_err(|_| {
            PreparationError::Schema(
                "Feature owner inventory cache is unavailable or already borrowed".into(),
            )
        })?;
        Ok(cache
            .owner_names
            .get(owner)
            .is_some_and(|(_, complete)| *complete))
    }

    fn retain_source_identity_observed(
        cache: &SharedFeatureInputCache,
        declaration: &DeclarationId,
        observation: &InvocationContext<'_>,
    ) -> Result<FeatureIdentityInput, PreparationError> {
        observation.work(1).map_err(PreparationError::Observation)?;
        let mut cache = cache.try_lock().map_err(|_| {
            PreparationError::Schema(
                "Feature input metadata cache is unavailable or already borrowed".into(),
            )
        })?;
        if let Some(value) = cache.source_ids.get(declaration) {
            return Ok(FeatureIdentityInput::Shared(value.clone()));
        }
        if cache.source_ids.len() >= 4096 {
            reserve_value(declaration, observation)?;
            return Ok(FeatureIdentityInput::Owned(MemberIdentity::Source {
                declaration: declaration.clone(),
            }));
        }
        reserve_value(declaration, observation)?;
        reserve_value(declaration, observation)?;
        reserve(observation, 128 + 32, &[])?;
        let value = std::sync::Arc::new(MemberIdentity::Source {
            declaration: declaration.clone(),
        });
        cache.source_ids.insert(declaration.clone(), value.clone());
        Ok(FeatureIdentityInput::Shared(value))
    }

    fn retain_observed(
        cache: &SharedFeatureInputCache,
        identity: &FeatureIdentityInput,
        input: &Feature,
        observation: &InvocationContext<'_>,
    ) -> Result<Option<std::sync::Arc<Feature>>, PreparationError> {
        observation.work(1).map_err(PreparationError::Observation)?;
        let mut cache = cache.try_lock().map_err(|_| {
            PreparationError::Schema(
                "Feature input metadata cache is unavailable or already borrowed".into(),
            )
        })?;
        if let Some(values) = cache.inputs.get(&**identity) {
            for value in values {
                observation.work(1).map_err(PreparationError::Observation)?;
                if value.as_ref() == input {
                    return Ok(Some(value.clone()));
                }
            }
        }
        if cache.entries >= 4096 {
            return Ok(None);
        }
        let missing = !cache.inputs.contains_key(&**identity);
        if missing {
            if let FeatureIdentityInput::Owned(value) = identity {
                reserve_value(value, observation)?;
            }
            reserve(observation, 128 + 32, &[])?;
        }
        reserve_value(input, observation)?;
        reserve(observation, 128 + 128 + 32 + 32, &[])?;
        if missing {
            let key = match identity {
                FeatureIdentityInput::Shared(value) => value.clone(),
                FeatureIdentityInput::Owned(value) => std::sync::Arc::new(value.clone()),
            };
            cache.inputs.insert(key, vec![]);
        }
        let values = cache
            .inputs
            .get_mut(&**identity)
            .ok_or("Missing retained feature input key")?;
        allocation(values, 1)?;
        let value = std::sync::Arc::new(input.clone());
        values.push(value.clone());
        cache.entries += 1;
        Ok(Some(value))
    }
}

// Optional invocation-owned source row selection for the SAME default worker.
// It selects ordered original row indices only; defaults/conflicts remain fresh.
pub(crate) trait DefaultSourceInputs {
    const ENABLED: bool = false;
    // Independent pure sorted-index algorithm; disabled for old/default paths.
    const ASCENDING_IDENTITY_APPEND: bool = false;
    // Private exact-source activation only. A public atom cache is not a
    // schema/inventory certificate and its ordinary callers retain all checks.
    const REUSE_OWNER_INVENTORY: bool = false;
    // Physical constructor inputs only; complete receiver selection still runs.
    // Default/generic workers always retain every effective feature payload.
    const PHYSICAL_INPUTS_ONLY: bool = false;
    fn owner_inventory_complete<A: super::RuntimeAlternative>(
        &self,
        _schema: &super::SchemaRepresentation<A>,
        _cache: &SharedFeatureInputCache,
        _owner: &str,
        _observation: &InvocationContext<'_>,
    ) -> Result<bool, PreparationError> {
        Ok(false)
    }
    fn step<A: super::RuntimeAlternative, T>(
        &self,
        _schema: &super::SchemaRepresentation<A>,
        _observation: &InvocationContext<'_>,
        run: impl FnOnce(&Self) -> Result<T, PreparationError>,
    ) -> Result<T, PreparationError> {
        run(self)
    }
    fn cursor<A: super::RuntimeAlternative>(
        &self,
        _schema: &super::SchemaRepresentation<A>,
        _selection: (&str, &str, &MemberIdentity, &str, usize),
        _observation: &InvocationContext<'_>,
    ) -> Result<Option<usize>, PreparationError> {
        Ok(None)
    }
}
pub(crate) struct NoDefaultSourceInputs;
impl DefaultSourceInputs for NoDefaultSourceInputs {}

/// Member/default projection with one inventory or policy transition per advance.
pub struct FeatureMapPreparation {
    receiver: String,
    shared_receiver: Option<std::sync::Arc<String>>,
    members: Option<EffectiveFeaturePreparation>,
    scan: Option<SelectedFeatureScan>,
    default: Option<FeatureDefaultPreparation>,
    owner: String,
    input_owner: Option<FeatureOwnerInput>,
    identity: Option<FeatureIdentityInput>,
    fields: BTreeMap<String, Feature>,
    input_fields: Option<BTreeMap<std::sync::Arc<str>, PreparedInputFeature>>,
    input_cache: Option<SharedFeatureInputCache>,
    owners: BTreeMap<String, String>,
    identities: BTreeMap<String, MemberIdentity>,
    order: Vec<String>,
    input_order: Option<Vec<std::sync::Arc<str>>>,
    done: bool,
    failed: bool,
}
impl FeatureMapPreparation {
    pub fn new(receiver: &str) -> Self {
        Self::with_members(receiver, EffectiveFeaturePreparation::new(receiver))
    }
    /// Observe the shared ancestry/member/default authority before retaining
    /// inputs or map entries. Keep the same schema and observation throughout.
    pub fn new_observed(
        receiver: &str,
        observation: &InvocationContext<'_>,
    ) -> Result<Self, PreparationError> {
        reserve(observation, 0, &[receiver])?;
        let members = EffectiveFeaturePreparation::new_observed(receiver, observation)?;
        Ok(Self::with_members(receiver, members))
    }
    pub(crate) fn with_identity_views_observed(
        receiver: &str,
        observation: &InvocationContext<'_>,
    ) -> Result<Self, PreparationError> {
        let mut result = Self::new_observed(receiver, observation)?;
        result
            .members
            .as_mut()
            .ok_or("Missing feature map members")?
            .identity_views = true;
        Ok(result)
    }
    pub fn with_source_inputs_observed(
        receiver: &str,
        cache: SharedFeatureInputCache,
        observation: &InvocationContext<'_>,
    ) -> Result<Self, PreparationError> {
        let mut result = Self::with_identity_views_observed(receiver, observation)?;
        // Move the already admitted receiver string into one shared payload.
        reserve(observation, 32, &[])?;
        result.shared_receiver = Some(std::sync::Arc::new(std::mem::take(&mut result.receiver)));
        result.input_fields = Some(BTreeMap::new());
        result.input_order = Some(Vec::new());
        let members = result
            .members
            .as_mut()
            .ok_or("Missing feature input members")?;
        members.input_cache = Some(cache.clone());
        members.members = SelectedFeatureInventory::Inputs(Vec::new());
        result.input_cache = Some(cache);
        Ok(result)
    }
    fn with_members(receiver: &str, members: EffectiveFeaturePreparation) -> Self {
        Self {
            receiver: receiver.into(),
            shared_receiver: None,
            members: Some(members),
            scan: None,
            default: None,
            owner: String::new(),
            input_owner: None,
            identity: None,
            fields: BTreeMap::new(),
            input_fields: None,
            input_cache: None,
            owners: BTreeMap::new(),
            identities: BTreeMap::new(),
            order: vec![],
            input_order: None,
            done: false,
            failed: false,
        }
    }
    fn receiver_name(&self) -> &str {
        self.shared_receiver
            .as_ref()
            .map_or(self.receiver.as_str(), |receiver| receiver.as_str())
    }
    fn publish_receiver(
        receiver: String,
        shared: Option<std::sync::Arc<String>>,
    ) -> Result<String, String> {
        match shared {
            Some(shared) => std::sync::Arc::try_unwrap(shared)
                .map_err(|_| "Feature receiver remains borrowed after complete preparation".into()),
            None => Ok(receiver),
        }
    }
    pub fn advance(
        &mut self,
        schema: &crate::definitions::schema::SchemaRepresentation<impl super::RuntimeAlternative>,
    ) -> Result<bool, String> {
        self.advance_observed(schema, &InvocationContext::default())
            .map_err(|error| error.to_string())
    }
    pub fn advance_observed(
        &mut self,
        schema: &crate::definitions::schema::SchemaRepresentation<impl super::RuntimeAlternative>,
        observation: &InvocationContext<'_>,
    ) -> Result<bool, PreparationError> {
        if self.failed {
            return Err("Feature map preparation has failed".into());
        }
        let result = observation
            .checkpoint()
            .map_err(PreparationError::Observation)
            .and_then(|()| self.advance_inner(schema, observation));
        if result.is_err() {
            self.failed = true;
        }
        result
    }
    pub(crate) fn advance_with_default_sources<S: DefaultSourceInputs>(
        &mut self,
        schema: &crate::definitions::schema::SchemaRepresentation<impl super::RuntimeAlternative>,
        observation: &InvocationContext<'_>,
        sources: &S,
    ) -> Result<bool, PreparationError> {
        if self.failed {
            return Err("Feature map preparation has failed".into());
        }
        let result = observation
            .checkpoint()
            .map_err(PreparationError::Observation)
            .and_then(|()| self.advance_inner_with_defaults(schema, observation, sources));
        if result.is_err() {
            self.failed = true;
        }
        result
    }
    fn advance_inner(
        &mut self,
        schema: &crate::definitions::schema::SchemaRepresentation<impl super::RuntimeAlternative>,
        observation: &InvocationContext<'_>,
    ) -> Result<bool, PreparationError> {
        self.advance_inner_with_defaults(schema, observation, &NoDefaultSourceInputs)
    }
    fn advance_inner_with_defaults<S: DefaultSourceInputs>(
        &mut self,
        schema: &crate::definitions::schema::SchemaRepresentation<impl super::RuntimeAlternative>,
        observation: &InvocationContext<'_>,
        sources: &S,
    ) -> Result<bool, PreparationError> {
        if self.done {
            return Ok(true);
        }
        if let Some(members) = &mut self.members {
            let complete = if S::ASCENDING_IDENTITY_APPEND || S::REUSE_OWNER_INVENTORY {
                members.advance_sources_observed(schema, observation, sources)?
            } else {
                members.advance_observed(schema, observation)?
            };
            if complete {
                self.scan = Some(
                    self.members
                        .take()
                        .ok_or("Missing feature map members")?
                        .finish_input_scan()?,
                );
            }
        } else if let Some(default) = &mut self.default {
            let complete = if S::ENABLED {
                let owner = self
                    .input_owner
                    .as_ref()
                    .map_or(self.owner.as_str(), FeatureOwnerInput::as_str);
                let identity = self
                    .identity
                    .as_deref()
                    .ok_or("Missing feature map source identity")?;
                default.advance_with_sources(schema, observation, sources, owner, identity)?
            } else {
                default.advance_observed(schema, observation)?
            };
            if complete {
                let pending = self.default.take().ok_or("Missing feature map default")?;
                let field = if self.input_fields.is_some() {
                    pending.finish_input_observed(observation)?
                } else {
                    FeatureInput::Owned(pending.finish()?)
                };
                let identity = self.identity.take().ok_or("Missing feature map identity")?;
                if let Some(fields) = &mut self.input_fields {
                    // One map key and one declaration-order key replace three
                    // independent input maps. The complete payloads move as one row.
                    let name = FeatureInputCache::retain_field_name_observed(
                        self.input_cache
                            .as_ref()
                            .ok_or("Missing field name cache")?,
                        &field.name,
                        observation,
                    )?;
                    reserve(
                        observation,
                        32 + 32 + crate::logical_size::LOGICAL_WORD,
                        &[],
                    )?;
                    let order = self
                        .input_order
                        .as_mut()
                        .ok_or("Missing input declaration order")?;
                    allocation(order, 1)?;
                    let owner = self
                        .input_owner
                        .take()
                        .ok_or("Missing feature owner input")?;
                    let position = order.len();
                    order.push(name.clone());
                    fields.insert(
                        name,
                        PreparedInputFeature {
                            position,
                            field,
                            owner,
                            identity,
                        },
                    );
                } else {
                    reserve(
                        observation,
                        32,
                        &[&field.name, &field.name, &field.name, &field.name],
                    )?;
                    allocation(&mut self.order, 1)?;
                    let FeatureIdentityInput::Owned(identity) = identity else {
                        return Err("Shared identity requires complete input map".into());
                    };
                    self.owners
                        .insert(field.name.clone(), std::mem::take(&mut self.owner));
                    self.identities.insert(field.name.clone(), identity);
                    self.order.push(field.name.clone());
                    let FeatureInput::Owned(field) = field else {
                        return Err("Expected owned feature map payload".into());
                    };
                    self.fields.insert(field.name.clone(), field);
                }
            }
        } else if let Some(member) = self.scan.as_mut().ok_or("Missing feature map scan")?.next() {
            let (owner, feature) =
                resolve_selected_feature(schema, member.owner.as_str(), member.name.as_str())?;
            if S::PHYSICAL_INPUTS_ONLY && self.input_fields.is_some() {
                // The unfiltered effective selection above has already checked
                // every owner, name, source identity and declaration order.
                // Omit only derived payload/default projection in this private
                // physical input result; never publish it as generic metadata.
                observation
                    .checkpoint()
                    .map_err(PreparationError::Observation)?;
                observation.work(1).map_err(PreparationError::Observation)?;
                if feature.derived {
                    return Ok(false);
                }
            }
            if let Some(cache) = &self.input_cache {
                self.input_owner = Some(FeatureInputCache::retain_owner_name_observed(
                    cache,
                    owner,
                    observation,
                )?);
            } else {
                reserve(observation, 0, &[owner])?;
                self.owner = owner.into();
            }
            self.identity = Some(member.identity);
            let input = match &self.input_cache {
                Some(cache) => FeatureInputCache::retain_observed(
                    cache,
                    self.identity
                        .as_ref()
                        .ok_or("Missing selected feature identity")?,
                    feature,
                    observation,
                )?,
                None => None,
            };
            self.default = Some(match input {
                Some(input) => match &self.shared_receiver {
                    Some(receiver) => FeatureDefaultPreparation::from_shared_receiver_observed(
                        receiver,
                        input,
                        observation,
                    )?,
                    None => FeatureDefaultPreparation::from_input_observed(
                        &self.receiver,
                        input,
                        observation,
                    )?,
                },
                None => FeatureDefaultPreparation::new_observed(
                    self.receiver_name(),
                    feature,
                    observation,
                )?,
            });
        } else {
            self.scan = None;
            self.done = true;
        }
        Ok(self.done)
    }
    pub fn finish(self) -> Result<PreparedFeatures, String> {
        if self.failed || !self.done {
            return Err("Feature map preparation is not complete".into());
        }
        if self.input_fields.is_some() {
            return Err("Feature input map requires input publication".into());
        }
        Ok(PreparedFeatures {
            receiver: Self::publish_receiver(self.receiver, self.shared_receiver)?,
            fields: self.fields,
            owners: self.owners,
            identities: self.identities,
            order: self.order,
        })
    }
    pub fn finish_inputs(self) -> Result<PreparedFeatureInputs, String> {
        if self.failed || !self.done {
            return Err("Feature map preparation is not complete".into());
        }
        match self.input_fields {
            Some(fields) => Ok(PreparedFeatureInputs::Shared(PreparedInputFeatures {
                receiver: Self::publish_receiver(self.receiver, self.shared_receiver)?,
                fields,
                order: self.input_order.ok_or("Missing complete input order")?,
            })),
            None => Ok(PreparedFeatureInputs::Owned(PreparedFeatures {
                receiver: Self::publish_receiver(self.receiver, self.shared_receiver)?,
                fields: self.fields,
                owners: self.owners,
                identities: self.identities,
                order: self.order,
            })),
        }
    }
}

// Full immutable input values only. Receiver/default decisions remain fresh.
#[derive(Clone)]
pub(crate) enum FeatureInput {
    Shared(std::sync::Arc<Feature>),
    Owned(Feature),
}
impl std::ops::Deref for FeatureInput {
    type Target = Feature;
    fn deref(&self) -> &Feature {
        match self {
            Self::Shared(value) => value,
            Self::Owned(value) => value,
        }
    }
}
// This state does not cache the selected default. Shared variants retain the
// complete declared input and separately own the current receiver's selection.
enum DefaultFeatureInput {
    Owned(Feature),
    Shared {
        input: std::sync::Arc<Feature>,
        selected_default: Option<serde_json::Value>,
    },
}
impl DefaultFeatureInput {
    fn name(&self) -> &str {
        match self {
            Self::Owned(value) => &value.name,
            Self::Shared { input, .. } => &input.name,
        }
    }
    fn default_value(&self) -> &serde_json::Value {
        match self {
            Self::Owned(value) => &value.default,
            Self::Shared {
                input,
                selected_default,
            } => selected_default.as_ref().unwrap_or(&input.default),
        }
    }
    fn set_default(&mut self, value: serde_json::Value) {
        match self {
            Self::Owned(feature) => feature.default = value,
            Self::Shared {
                selected_default, ..
            } => *selected_default = Some(value),
        }
    }
}

/// Receiver-specific default projection over one runtime policy per advance.
/// Individual feature/default values are cloned atomically. The schema must stay
/// immutable. This is shared by synchronous and staged feature consumers.
pub struct FeatureDefaultPreparation {
    receiver: FeatureOwnerInput,
    feature: DefaultFeatureInput,
    index: usize,
    selected: bool,
    done: bool,
    failed: bool,
}
impl FeatureDefaultPreparation {
    /// Measure the borrowed feature through the shared bounded metadata policy
    /// before cloning it. Payload measurement/copy remain synchronous.
    pub fn new_observed(
        receiver: &str,
        feature: &Feature,
        observation: &InvocationContext<'_>,
    ) -> Result<Self, PreparationError> {
        reserve(observation, 0, &[receiver])?;
        reserve_value(feature, observation)?;
        Ok(Self::new(receiver, feature))
    }
    pub fn new(receiver: &str, feature: &Feature) -> Self {
        Self {
            receiver: FeatureOwnerInput::Owned(receiver.into()),
            feature: DefaultFeatureInput::Owned(feature.clone()),
            index: 0,
            selected: false,
            done: false,
            failed: false,
        }
    }
    pub(crate) fn from_input_observed(
        receiver: &str,
        input: std::sync::Arc<Feature>,
        observation: &InvocationContext<'_>,
    ) -> Result<Self, PreparationError> {
        reserve(observation, 0, &[receiver])?;
        Ok(Self {
            receiver: FeatureOwnerInput::Owned(receiver.into()),
            feature: DefaultFeatureInput::Shared {
                input,
                selected_default: None,
            },
            index: 0,
            selected: false,
            done: false,
            failed: false,
        })
    }
    fn from_shared_receiver_observed(
        receiver: &std::sync::Arc<String>,
        input: std::sync::Arc<Feature>,
        observation: &InvocationContext<'_>,
    ) -> Result<Self, PreparationError> {
        reserve(observation, 32, &[])?;
        Ok(Self {
            receiver: FeatureOwnerInput::Shared(receiver.clone()),
            feature: DefaultFeatureInput::Shared {
                input,
                selected_default: None,
            },
            index: 0,
            selected: false,
            done: false,
            failed: false,
        })
    }
    pub(crate) fn finish_input_observed(
        self,
        observation: &InvocationContext<'_>,
    ) -> Result<FeatureInput, PreparationError> {
        if self.failed || !self.done {
            return Err("Feature default preparation is not complete".into());
        }
        match self.feature {
            DefaultFeatureInput::Owned(value) => Ok(FeatureInput::Owned(value)),
            DefaultFeatureInput::Shared {
                input,
                selected_default,
            } => {
                if let Some(default) = selected_default
                    && default != input.default
                {
                    reserve_value(input.as_ref(), observation)?;
                    let mut value = input.as_ref().clone();
                    value.default = default;
                    return Ok(FeatureInput::Owned(value));
                }
                Ok(FeatureInput::Shared(input))
            }
        }
    }
    pub fn advance(
        &mut self,
        schema: &crate::definitions::schema::SchemaRepresentation<impl super::RuntimeAlternative>,
    ) -> Result<bool, String> {
        self.advance_observed(schema, &InvocationContext::default())
            .map_err(|error| error.to_string())
    }
    pub fn advance_observed(
        &mut self,
        schema: &crate::definitions::schema::SchemaRepresentation<impl super::RuntimeAlternative>,
        observation: &InvocationContext<'_>,
    ) -> Result<bool, PreparationError> {
        if self.failed {
            return Err("Feature default preparation has failed".into());
        }
        let result = observation
            .checkpoint()
            .map_err(PreparationError::Observation)
            .and_then(|()| self.advance_inner(schema, observation));
        if result.is_err() {
            self.failed = true;
        }
        result
    }
    pub(crate) fn advance_with_sources<S: DefaultSourceInputs>(
        &mut self,
        schema: &crate::definitions::schema::SchemaRepresentation<impl super::RuntimeAlternative>,
        observation: &InvocationContext<'_>,
        sources: &S,
        owner: &str,
        identity: &MemberIdentity,
    ) -> Result<bool, PreparationError> {
        if !S::ENABLED {
            return self.advance_observed(schema, observation);
        }
        if self.failed {
            return Err("Feature default preparation has failed".into());
        }
        // One actual metadata lookup is paid. Tiny/absent tables retain the
        // literal worker before acquiring an otherwise unprofitable index guard.
        if let Err(error) = observation.work(1).map_err(PreparationError::Observation) {
            // No source step ticket exists yet. Fail this continuation only;
            // the enclosing scoped audit handles its own propagated failure.
            self.failed = true;
            return Err(error);
        }
        if schema
            .runtime_feature_domains
            .get(self.receiver.as_str())
            .map_or(0, Vec::len)
            <= 8
        {
            return self.advance_observed(schema, observation);
        }
        let result = sources.step(schema, observation, |inputs| {
            if self.failed {
                return Err("Feature default preparation has failed".into());
            }
            observation
                .checkpoint()
                .map_err(PreparationError::Observation)?;
            if !self.done
                && let Some(cursor) = inputs.cursor(
                    schema,
                    (
                        self.receiver.as_str(),
                        owner,
                        identity,
                        self.feature.name(),
                        self.index,
                    ),
                    observation,
                )?
            {
                self.index = cursor;
            }
            // Sole canonical default selection/copy/conflict implementation.
            self.advance_inner(schema, observation)
        });
        if result.is_err() {
            self.failed = true;
        }
        result
    }
    fn advance_inner(
        &mut self,
        schema: &crate::definitions::schema::SchemaRepresentation<impl super::RuntimeAlternative>,
        observation: &InvocationContext<'_>,
    ) -> Result<bool, PreparationError> {
        if self.done {
            return Ok(true);
        }
        let domain = schema
            .runtime_feature_domains
            .get(self.receiver.as_str())
            .and_then(|domains| domains.get(self.index));
        if let Some(domain) = domain {
            if domain.source.view().projection.source.name == self.feature.name() {
                if self.selected && self.feature.default_value() != &domain.default {
                    self.failed = true;
                    return Err("Conflicting receiver-specific source defaults".into());
                }
                if !self.selected {
                    reserve_value(&domain.default, observation)?;
                    self.feature.set_default(domain.default.clone());
                    self.selected = true;
                }
            }
            self.index += 1;
        } else {
            self.done = true;
        }
        Ok(self.done)
    }
    pub fn finish(self) -> Result<Feature, String> {
        if self.failed || !self.done {
            return Err("Feature default preparation is not complete".into());
        }
        match self.feature {
            DefaultFeatureInput::Owned(value) => Ok(value),
            DefaultFeatureInput::Shared { .. } => {
                Err("Shared feature input requires observed publication".into())
            }
        }
    }
}

#[cfg(test)]
#[path = "schema_observation_tests.rs"]
mod observation_tests;

#[cfg(test)]
mod reference_array_completion_audit {
    use super::*;
    use crate::definitions::{
        bundle::{DefinitionBundle, DefinitionSource, SourceKind},
        generic_types::{RuntimeType, Type},
    };
    #[derive(Default)]
    struct Cancellation(std::cell::Cell<bool>);
    impl crate::definitions::conversion::InvocationObserver for Cancellation {
        fn before_invoke(
            &self,
            _: &crate::definitions::conversion::ProviderId,
        ) -> Result<(), String> {
            Err("Unexpected provider".into())
        }
        fn checkpoint(&self) -> Result<(), String> {
            if self.0.get() {
                Err("array audit cancelled".into())
            } else {
                Ok(())
            }
        }
        fn work(&self, _: usize) -> Result<(), String> {
            self.checkpoint()
        }
        fn retain_storage(&self, _: usize) -> Result<(), String> {
            self.checkpoint()
        }
    }
    #[test]
    fn reference_array_completion_is_issued_only_after_original_duplicate_and_class_checks() {
        let source = DefinitionSource::new(
            "memory:array-completion.ecore",
            SourceKind::Ecore,
            "<e:EPackage xmlns:e='http://www.eclipse.org/emf/2002/Ecore' xmlns:x='http://www.w3.org/2001/XMLSchema-instance' name='m' nsURI='urn:array-completion'><eClassifiers x:type='e:EClass' name='Root'><eTypeParameters name='T'><eBounds eClassifier='#//Root'/></eTypeParameters><eStructuralFeatures x:type='e:EReference' name='r'><eGenericType eTypeParameter='#//Root/T'/></eStructuralFeatures></eClassifiers><eClassifiers x:type='e:EClass' name='Child' eSuperTypes='#//Root'/></e:EPackage>",
        );
        let (source, ids) = crate::definitions::migration_test_fixture::source_with_ids(source);
        let types = (0..2)
            .map(|i| {
                RuntimeType::new(Type::Classifier {
                    declaration: ids[&vec![i]].clone(),
                    arguments: vec![],
                    raw: i == 0,
                })
                .unwrap()
            })
            .collect::<Vec<_>>();
        let bundle = DefinitionBundle {
            version: 1,
            sources: vec![source.clone()],
            namespaces: vec![],
        };
        let observation = InvocationContext::default();
        let schema =
            crate::definitions::compiler::materialize_runtime_source_array_inputs_observed(
                &bundle,
                &source.id,
                "urn:array-completion",
                &types,
                None,
                &observation,
            )
            .unwrap();
        let mut fixed = ValidatedRuntimeTypes::begin(&schema);
        assert!(fixed.completed_reference_array_certificate().is_none());
        let certificate = loop {
            assert!(!fixed.advance_observed(&observation).unwrap());
            if let Some(certificate) = fixed.completed_reference_array_certificate() {
                break certificate;
            }
        };
        let values = schema.runtime_feature_domains.values().next().unwrap()[0]
            .alternatives
            .as_ref();
        assert!(certificate.belongs_to(&schema, values));
        let foreign = schema.clone();
        let foreign_values = foreign.runtime_feature_domains.values().next().unwrap()[0]
            .alternatives
            .as_ref();
        assert!(std::ptr::eq(values, foreign_values));
        assert!(!certificate.belongs_to(&foreign, foreign_values));
        let mut foreign_pending = ValidatedRuntimeTypes::begin(&foreign);
        loop {
            assert!(!foreign_pending.advance_observed(&observation).unwrap());
            if foreign_pending
                .validation
                .pending_reference_array(&foreign)
                .is_some()
            {
                break;
            }
        }
        assert!(
            foreign_pending
                .validation
                .use_reference_array_certificate(&foreign, &certificate, &observation)
                .is_err()
        );
        assert!(foreign_pending.advance_observed(&observation).is_err());
        assert!(foreign_pending.finish().is_err());
        let decoded: crate::definitions::schema::RuntimeSourceArraySchemaInputs =
            serde_json::from_slice(&serde_json::to_vec(&schema).unwrap()).unwrap();
        let decoded_values = decoded.runtime_feature_domains.values().next().unwrap()[0]
            .alternatives
            .as_ref();
        assert!(!certificate.belongs_to(&schema, decoded_values));
        assert!(!certificate.belongs_to(&decoded, decoded_values));
        // Use the fixed schema throughout; no caller-provided owner can issue proof.
        while !fixed.advance_observed(&observation).unwrap() {}
        assert!(fixed.finish().unwrap().belongs_to(&schema));
        for prefix in [0, 1, 8, 32, 64] {
            let control = std::rc::Rc::new(Cancellation::default());
            let operation = InvocationContext::new(control.clone());
            let mut pending = ValidatedRuntimeTypes::begin(&schema);
            for _ in 0..prefix {
                assert!(!pending.advance_observed(&operation).unwrap());
            }
            control.0.set(true);
            assert!(pending.advance_observed(&operation).is_err());
            assert!(pending.completed_reference_array_certificate().is_none());
            control.0.set(false);
            assert!(pending.advance_observed(&operation).is_err());
            assert!(pending.finish().is_err());
        }
        for variant in 0..3 {
            let mut input = schema.clone();
            if variant > 0 {
                for domain in input.runtime_feature_domains.values_mut().flatten() {
                    let mut values = domain
                        .alternatives
                        .iter()
                        .map(|value| value.as_ref().to_owned())
                        .collect::<Vec<_>>();
                    if variant == 1 {
                        values.push(values[0].clone());
                    } else {
                        values[0] = "missing-class".into();
                    }
                    domain.alternatives =
                        serde_json::from_value(serde_json::json!(values)).unwrap();
                }
            }
            let mut pending = super::super::InstantiationValidation::new();
            assert!(pending.completed_reference_array(&input).is_none());
            let mut completions = 0;
            loop {
                match pending.advance_observed(&input, &observation) {
                    Ok(done) => {
                        if let Some(values) = pending.completed_reference_array(&input) {
                            assert!(
                                values
                                    .iter()
                                    .all(|value| input.classes.contains_key(value.as_ref()))
                            );
                            completions += 1;
                        }
                        if done {
                            assert_eq!(variant, 0);
                            break;
                        }
                    }
                    Err(_) => {
                        assert!(variant > 0);
                        assert_eq!(completions, 0);
                        assert!(pending.completed_reference_array(&input).is_none());
                        assert!(pending.advance_observed(&input, &observation).is_err());
                        break;
                    }
                }
            }
            if variant == 0 {
                assert!(completions >= 2);
            }
        }
    }
}

#[cfg(test)]
mod owner_name_input_controls {
    use super::*;
    use crate::definitions::conversion::{InvocationObserver, ProviderId};
    use std::{
        cell::Cell,
        rc::Rc,
        sync::{Arc, Mutex},
    };
    struct Meter {
        bytes: Cell<usize>,
        work: Cell<usize>,
        cap: usize,
        cancelled: Cell<bool>,
    }
    impl InvocationObserver for Meter {
        fn before_invoke(&self, _: &ProviderId) -> Result<(), String> {
            Err("No provider expected".into())
        }
        fn work(&self, n: usize) -> Result<(), String> {
            if self.cancelled.get() {
                return Err("Cancelled".into());
            }
            self.work.set(self.work.get() + n);
            Ok(())
        }
        fn retain_storage(&self, n: usize) -> Result<(), String> {
            if self.cancelled.get() {
                return Err("Cancelled".into());
            }
            let next = self.bytes.get() + n;
            if next > self.cap {
                return Err("Storage denied".into());
            }
            self.bytes.set(next);
            Ok(())
        }
    }
    fn meter(cap: usize) -> Rc<Meter> {
        Rc::new(Meter {
            bytes: Cell::new(0),
            work: Cell::new(0),
            cap,
            cancelled: Cell::new(false),
        })
    }
    #[test]
    fn owner_text_inputs_share_only_exact_text_and_precharge_cold_hot_denial() {
        let owner = format!("__type_{}", "x".repeat(96));
        let needed = 128 + 32 + 2 * (64 + owner.len());
        for cap in [needed - 1, needed] {
            let cache = Arc::new(Mutex::new(FeatureInputCache::default()));
            let cold_meter = meter(cap);
            let observation = InvocationContext::new(cold_meter.clone());
            let result =
                FeatureInputCache::retain_owner_name_observed(&cache, &owner, &observation);
            if cap < needed {
                assert!(matches!(result, Err(PreparationError::Observation(_))));
                assert!(cache.lock().unwrap().owner_names.is_empty());
            } else {
                let first = result.unwrap();
                assert_eq!(first.as_str(), owner);
                assert_eq!(cold_meter.bytes.get(), needed);
                assert!(
                    FeatureInputCache::retain_owner_name_observed(&cache, &owner, &observation)
                        .is_err()
                );
                let hot = meter(32);
                let hot_observation = InvocationContext::new(hot.clone());
                let second =
                    FeatureInputCache::retain_owner_name_observed(&cache, &owner, &hot_observation)
                        .unwrap();
                let (FeatureOwnerInput::Shared(a), FeatureOwnerInput::Shared(b)) = (first, second)
                else {
                    panic!("Shared text expected")
                };
                assert!(Arc::ptr_eq(&a, &b));
                assert_eq!(hot.bytes.get(), 32);
                hot.cancelled.set(true);
                assert!(
                    FeatureInputCache::retain_owner_name_observed(&cache, &owner, &hot_observation)
                        .is_err()
                );
                let other = FeatureInputCache::retain_owner_name_observed(
                    &cache,
                    &(owner.clone() + "different"),
                    &InvocationContext::default(),
                )
                .unwrap();
                assert_ne!(other.as_str(), owner);
            }
        }
    }
    #[test]
    fn source_input_owner_export_admits_actual_copies_and_incremental_cleanup() {
        let cache = Arc::new(Mutex::new(FeatureInputCache::default()));
        let owner = "Base";
        let input = FeatureInputCache::retain_owner_name_observed(
            &cache,
            owner,
            &InvocationContext::default(),
        )
        .unwrap();
        let FeatureOwnerInput::Shared(value) = input else {
            panic!("Shared input expected")
        };
        let original = value.as_ptr();
        let copied = FeatureOwnerInput::Shared(value).into_owned();
        assert_eq!(copied, owner);
        assert_ne!(copied.as_ptr(), original);
        let mut cache = cache.lock().unwrap();
        assert!(cache.clear_one().unwrap());
        assert!(cache.owner_names.is_empty());
    }
}

#[cfg(test)]
mod selected_feature_inventory_tests {
    use super::*;

    fn item(index: usize) -> SelectedFeatureInput {
        let name = format!("f{index:03}");
        SelectedFeatureInput {
            owner: FeatureOwnerInput::Owned("A".into()),
            name: FeatureNameInput::Owned(name.clone()),
            identity: FeatureIdentityInput::Owned(MemberIdentity::Programmatic {
                revision: "revision".into(),
                namespace: "namespace".into(),
                owner: "A".into(),
                name,
            }),
            position: index,
        }
    }
    #[test]
    fn owned_and_prepared_member_scan_preserve_order() {
        let obs = InvocationContext::default();
        for owned in [false, true] {
            let mut values = if owned {
                SelectedFeatureInventory::Owned(Vec::new())
            } else {
                SelectedFeatureInventory::Inputs(Vec::new())
            };
            for i in 0..9 {
                values.reserve_one(&obs).unwrap();
                values.push(item(i)).unwrap();
            }
            let mut scan = values.into_scan().unwrap();
            for i in 0..9 {
                let got = scan.next().unwrap();
                assert_eq!(got.position, i);
                assert_eq!(&*got.identity, &*item(i).identity);
            }
            assert!(scan.next().is_none());
        }
    }
}

#[cfg(test)]
#[path = "schema_opposite_reuse_tests.rs"]
mod opposite_reuse_tests;
