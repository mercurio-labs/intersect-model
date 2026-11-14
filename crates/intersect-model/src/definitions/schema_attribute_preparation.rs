//! Handwritten continuations for attribute provider metadata qualification.
//! Keep schema and registry immutable between advances. Individual feature,
//! datatype, contract and provider snapshot operations remain atomic.
use super::attribute_source::{
    AttributeCandidateOutcome, AttributeFieldOrigin, AttributeFieldOwner, CanonicalCandidateInputs,
};
use super::observation::{PreparationError, allocation, reserve, reserve_value};
use super::{
    AttributeSourceInputs, Feature, FeatureMapPreparation, NoAttributeSourceInputs,
    PreparedFeatures, RuntimeDomainSource,
};
use crate::definitions::conversion::{
    AppliedConversionContract, ConversionContract, ConversionRegistry, InvocationContext,
};
use crate::definitions::datatypes::{Binding, Datatype};
#[cfg(test)]
use crate::definitions::schema::Schema;
use serde::Deserialize;
use serde_json::Value;

enum AppliedStage {
    Header,
    Scan,
    Check,
    Done,
}

#[cfg(test)]
#[path = "schema_attribute_observation_tests.rs"]
mod observation_tests;
/// Select exactly one compiled applied contract, one table row per advance.
pub struct AppliedDatatypeContractPreparation {
    datatype: Datatype,
    stage: AppliedStage,
    cursor: usize,
    selected: Option<usize>,
    failed: bool,
}
impl AppliedDatatypeContractPreparation {
    fn new_with_sources<S: AttributeSourceInputs>(
        datatype: &Datatype,
        observation: &InvocationContext<'_>,
        sources: &S,
    ) -> Result<Self, PreparationError> {
        if !S::ENABLED {
            return Self::new_observed(datatype, observation);
        }
        sources.complete(
            (|| {
                sources.reserve_datatype(datatype, observation)?;
                Ok(Self::with_datatype(datatype))
            })(),
            observation,
        )
    }
    pub fn new(datatype: &Datatype) -> Self {
        Self::with_datatype(datatype)
    }
    pub fn new_observed(
        datatype: &Datatype,
        observation: &InvocationContext<'_>,
    ) -> Result<Self, PreparationError> {
        reserve_value(datatype, observation)?;
        Ok(Self::with_datatype(datatype))
    }
    fn with_datatype(datatype: &Datatype) -> Self {
        Self {
            datatype: datatype.clone(),
            stage: AppliedStage::Header,
            cursor: 0,
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
        if self.failed {
            return Err("Applied datatype contract preparation has failed".into());
        }
        let result = observation
            .checkpoint()
            .map_err(PreparationError::Observation)
            .and_then(|()| {
                self.advance_inner_with_sources(schema, observation, &NoAttributeSourceInputs)
            });
        if result.is_err() {
            self.failed = true;
        }
        result
    }
    fn advance_inner_with_sources<S: AttributeSourceInputs>(
        &mut self,
        schema: &crate::definitions::schema::SchemaRepresentation<impl super::RuntimeAlternative>,
        observation: &InvocationContext<'_>,
        sources: &S,
    ) -> Result<bool, PreparationError> {
        match self.stage {
            AppliedStage::Header => {
                self.datatype
                    .application()
                    .ok_or("Datatype has no application")?;
                self.stage = AppliedStage::Scan;
            }
            AppliedStage::Scan => {
                if S::ENABLED
                    && self.cursor == 0
                    && let Some(index) =
                        sources.applied_index(schema, &self.datatype, observation)?
                {
                    self.selected = Some(index);
                    self.stage = AppliedStage::Check;
                    return Ok(false);
                }
                if let Some(contract) = schema.applied_conversion_contracts.get(self.cursor) {
                    if Some(&contract.application) == self.datatype.application() {
                        if self.selected.is_some() {
                            return Err("Invalid compiled applied conversion contract".into());
                        }
                        self.selected = Some(self.cursor);
                    }
                    self.cursor += 1;
                } else {
                    self.selected
                        .ok_or("Missing compiled applied conversion contract")?;
                    self.stage = AppliedStage::Check;
                }
            }
            AppliedStage::Check => {
                let contract = schema
                    .applied_conversion_contracts
                    .get(
                        self.selected
                            .ok_or("Missing compiled applied conversion contract")?,
                    )
                    .ok_or("Missing compiled applied conversion contract")?;
                if contract.conversion.datatype != *self.datatype.declaration()
                    || contract.conversion.serializable != self.datatype.serializable()
                    || contract.conversion.provider.name.trim().is_empty()
                    || contract.conversion.provider.version.trim().is_empty()
                {
                    return Err("Invalid compiled applied conversion contract".into());
                }
                if S::ENABLED {
                    sources.remember_applied(
                        schema,
                        &self.datatype,
                        self.selected
                            .ok_or("Missing compiled applied conversion contract")?,
                        observation,
                    )?;
                }
                self.stage = AppliedStage::Done;
            }
            AppliedStage::Done => return Ok(true),
        }
        Ok(matches!(self.stage, AppliedStage::Done))
    }
    pub fn finish(
        self,
        schema: &crate::definitions::schema::SchemaRepresentation<impl super::RuntimeAlternative>,
    ) -> Result<&AppliedConversionContract, String> {
        if self.failed || !matches!(self.stage, AppliedStage::Done) {
            return Err("Applied datatype contract preparation is not complete".into());
        }
        schema
            .applied_conversion_contracts
            .get(
                self.selected
                    .ok_or("Missing compiled applied conversion contract")?,
            )
            .ok_or_else(|| "Missing compiled applied conversion contract".into())
    }
    pub(crate) fn advance_with_sources<S: AttributeSourceInputs>(
        &mut self,
        schema: &crate::definitions::schema::SchemaRepresentation<impl super::RuntimeAlternative>,
        observation: &InvocationContext<'_>,
        sources: &S,
    ) -> Result<bool, PreparationError> {
        if self.failed {
            return sources.complete(
                Err("Applied datatype contract preparation has failed".into()),
                observation,
            );
        }
        let result = observation
            .checkpoint()
            .map_err(PreparationError::Observation)
            .and_then(|()| self.advance_inner_with_sources(schema, observation, sources));
        let result = sources.complete(result, observation);
        if result.is_err() {
            self.failed = true;
        }
        result
    }
}

enum ConversionStage {
    Header,
    Applied,
    Ordinary,
    Done,
}
/// Require an attribute's exact codec independently of its present values.
/// Metadata checks do not invoke provider semantics or create invocation receipts.
pub struct AttributeConversionPreparation<F: AttributeFieldOwner = Feature> {
    field: F,
    stage: ConversionStage,
    applied: Option<Box<AppliedDatatypeContractPreparation>>,
    selected: Option<ConversionContract>,
    cursor: usize,
    matched: bool,
    failed: bool,
}
impl AttributeConversionPreparation {
    pub fn new(field: &Feature) -> Self {
        Self::with_field(field)
    }
    pub fn new_observed(
        field: &Feature,
        observation: &InvocationContext<'_>,
    ) -> Result<Self, PreparationError> {
        reserve_value(field, observation)?;
        Ok(Self::with_field(field))
    }
    fn with_field(field: &Feature) -> Self {
        Self {
            field: field.clone(),
            stage: ConversionStage::Header,
            applied: None,
            selected: None,
            cursor: 0,
            matched: false,
            failed: false,
        }
    }
    pub fn advance(
        &mut self,
        schema: &crate::definitions::schema::SchemaRepresentation<impl super::RuntimeAlternative>,
        conversions: Option<&ConversionRegistry<'_>>,
    ) -> Result<bool, String> {
        self.advance_observed(schema, conversions, &InvocationContext::default())
            .map_err(|error| error.to_string())
    }
    pub fn advance_observed(
        &mut self,
        schema: &crate::definitions::schema::SchemaRepresentation<impl super::RuntimeAlternative>,
        conversions: Option<&ConversionRegistry<'_>>,
        observation: &InvocationContext<'_>,
    ) -> Result<bool, PreparationError> {
        if self.failed {
            return Err("Attribute conversion preparation has failed".into());
        }
        let result = observation
            .checkpoint()
            .map_err(PreparationError::Observation)
            .and_then(|()| {
                self.advance_inner_with_sources(
                    schema,
                    conversions,
                    observation,
                    &NoAttributeSourceInputs,
                )
            });
        if result.is_err() {
            self.failed = true;
        }
        result
    }
}
impl<F: AttributeFieldOwner> AttributeConversionPreparation<F> {
    fn with_owner(field: F) -> Self {
        Self {
            field,
            stage: ConversionStage::Header,
            applied: None,
            selected: None,
            cursor: 0,
            matched: false,
            failed: false,
        }
    }
    fn from_owner_with_sources<S: AttributeSourceInputs<Field = F>>(
        field: &F,
        observation: &InvocationContext<'_>,
        sources: &S,
    ) -> Result<Self, PreparationError> {
        sources.complete(
            (|| {
                let field = sources.copy_field(field, observation)?;
                Ok(Self::with_owner(field))
            })(),
            observation,
        )
    }
    fn new_with_sources<S: AttributeSourceInputs<Field = F>>(
        field: &Feature,
        observation: &InvocationContext<'_>,
        sources: &S,
    ) -> Result<Self, PreparationError> {
        sources.complete(
            (|| {
                let field =
                    sources.acquire_field(field, AttributeFieldOrigin::Synthetic, observation)?;
                Ok(Self::with_owner(field))
            })(),
            observation,
        )
    }
    fn advance_inner_with_sources<S: AttributeSourceInputs<Field = F>>(
        &mut self,
        schema: &crate::definitions::schema::SchemaRepresentation<impl super::RuntimeAlternative>,
        conversions: Option<&ConversionRegistry<'_>>,
        observation: &InvocationContext<'_>,
        sources: &S,
    ) -> Result<bool, PreparationError> {
        sources.check_field_owner(schema, &self.field, observation)?;
        match self.stage {
            ConversionStage::Header => {
                if !self.field.field().reference
                    && let Some(id) = &self.field.field().datatype
                    && let Some(datatype) = schema.datatypes.get(&self.field.field().target)
                {
                    if datatype.declaration() != id {
                        return Err("Attribute datatype identity mismatch".into());
                    }
                    if datatype.application().is_some() {
                        conversions
                            .ok_or("Explicit applied conversion registry required")?
                            .require_datatype(datatype)?;
                        self.applied = Some(Box::new(
                            AppliedDatatypeContractPreparation::new_with_sources(
                                datatype,
                                observation,
                                sources,
                            )?,
                        ));
                        self.stage = ConversionStage::Applied;
                        return Ok(false);
                    }
                    if matches!(datatype.binding(), Binding::Unsupported { .. }) {
                        let contract = conversions
                            .ok_or("Explicit conversion registry required for custom datatype")?
                            .require(datatype)?;
                        reserve_value(contract, observation)?;
                        self.selected = Some(contract.clone());
                        self.stage = ConversionStage::Ordinary;
                        return Ok(false);
                    }
                }
                self.stage = ConversionStage::Done;
            }
            ConversionStage::Applied => {
                if self
                    .applied
                    .as_mut()
                    .ok_or("Missing applied contract preparation")?
                    .advance_with_sources(schema, observation, sources)?
                {
                    let contract = self
                        .applied
                        .take()
                        .ok_or("Missing applied contract preparation")?
                        .finish(schema)?;
                    if contract
                        != conversions
                            .ok_or("Explicit applied conversion registry required")?
                            .applied()
                            .require(&contract.application)?
                    {
                        return Err(
                            "Runtime applied provider differs from compiled schema contract".into(),
                        );
                    }
                    self.stage = ConversionStage::Done;
                }
            }
            ConversionStage::Ordinary => {
                if S::ENABLED
                    && self.cursor == 0
                    && let Some(index) =
                        sources.ordinary_index(schema, self.field.field(), observation)?
                {
                    let compiled = schema
                        .conversion_contracts
                        .get(index)
                        .ok_or("Missing compiled source conversion contract")?;
                    if Some(&compiled.datatype) != self.field.field().datatype.as_ref()
                        || Some(compiled) != self.selected.as_ref()
                    {
                        return Err(
                            "Runtime conversion provider differs from compiled schema contract"
                                .into(),
                        );
                    }
                    self.matched = true;
                    self.cursor = schema.conversion_contracts.len();
                }
                if let Some(contract) = schema.conversion_contracts.get(self.cursor) {
                    if Some(&contract.datatype) == self.field.field().datatype.as_ref() {
                        if self.matched || Some(contract) != self.selected.as_ref() {
                            return Err(
                                "Runtime conversion provider differs from compiled schema contract"
                                    .into(),
                            );
                        }
                        if S::ENABLED {
                            sources.observe_ordinary(
                                schema,
                                self.field.field(),
                                self.cursor,
                                observation,
                            )?;
                        }
                        self.matched = true;
                    }
                    self.cursor += 1;
                } else {
                    if !self.matched {
                        return Err(
                            "Runtime conversion provider differs from compiled schema contract"
                                .into(),
                        );
                    }
                    // A native adapter may change its metadata while this scan
                    // is suspended. Recheck the registered snapshot before
                    // publishing codec qualification, including absent values.
                    let datatype = schema
                        .datatypes
                        .get(&self.field.field().target)
                        .ok_or("Missing qualified attribute datatype")?;
                    let current = conversions
                        .ok_or("Explicit conversion registry required for custom datatype")?
                        .require(datatype)?;
                    if Some(current) != self.selected.as_ref() {
                        return Err(
                            "Runtime conversion provider differs from compiled schema contract"
                                .into(),
                        );
                    }
                    if S::ENABLED {
                        sources.remember_ordinary(schema, self.field.field(), observation)?;
                    }
                    self.selected = None;
                    self.stage = ConversionStage::Done;
                }
            }
            ConversionStage::Done => return Ok(true),
        }
        Ok(matches!(self.stage, ConversionStage::Done))
    }
    pub fn finish(self) -> Result<(), String> {
        if self.failed || !matches!(self.stage, ConversionStage::Done) {
            return Err("Attribute conversion preparation is not complete".into());
        }
        Ok(())
    }
    pub(crate) fn advance_with_sources<S: AttributeSourceInputs<Field = F>>(
        &mut self,
        schema: &crate::definitions::schema::SchemaRepresentation<impl super::RuntimeAlternative>,
        conversions: Option<&ConversionRegistry<'_>>,
        observation: &InvocationContext<'_>,
        sources: &S,
    ) -> Result<bool, PreparationError> {
        if self.failed {
            return sources.complete(
                Err("Attribute conversion preparation has failed".into()),
                observation,
            );
        }
        let result = observation
            .checkpoint()
            .map_err(PreparationError::Observation)
            .and_then(|()| {
                self.advance_inner_with_sources(schema, conversions, observation, sources)
            });
        let result = sources.complete(result, observation);
        if result.is_err() {
            self.failed = true;
        }
        result
    }
}

enum RuntimeStage {
    Members,
    Check,
    Clear,
    Domains,
    Ready,
    Alternatives,
    Codec,
    ClearDomains,
    Done,
}
/// Qualify the full effective field and all matching runtime codec policies.
/// Receiver member/default selection, cleanup, policy rows and alternatives yield.
pub struct RuntimeAttributeConversionPreparation<F: AttributeFieldOwner = Feature> {
    receiver: String,
    field: F,
    stage: RuntimeStage,
    members: Option<Box<FeatureMapPreparation>>,
    fields: Option<PreparedFeatures>,
    domains: Vec<usize>,
    cursor: usize,
    domain: usize,
    alternative: usize,
    codec: Option<Box<AttributeConversionPreparation<F>>>,
    unconstrained: bool,
    retain_domains: bool,
    failed: bool,
}
impl RuntimeAttributeConversionPreparation {
    pub fn new(receiver: &str, field: &Feature) -> Self {
        Self::with_members(receiver, field, Some(FeatureMapPreparation::new(receiver)))
    }
    pub fn new_observed(
        receiver: &str,
        field: &Feature,
        observation: &InvocationContext<'_>,
    ) -> Result<Self, PreparationError> {
        reserve(observation, 0, &[receiver])?;
        reserve_value(field, observation)?;
        let members = FeatureMapPreparation::new_observed(receiver, observation)?;
        Ok(Self::with_members(receiver, field, Some(members)))
    }
    fn with_members(
        receiver: &str,
        field: &Feature,
        members: Option<FeatureMapPreparation>,
    ) -> Self {
        Self {
            receiver: receiver.into(),
            field: field.clone(),
            stage: if members.is_some() {
                RuntimeStage::Members
            } else {
                RuntimeStage::Domains
            },
            members: members.map(Box::new),
            fields: None,
            domains: vec![],
            cursor: 0,
            domain: 0,
            alternative: 0,
            codec: None,
            unconstrained: false,
            retain_domains: false,
            failed: false,
        }
    }
    /// Reuse completed shared member/default selection. Keep the same immutable
    /// schema that produced `fields` throughout this preparation; a projection
    /// is an in-memory preparation result, not serialized admission evidence.
    pub fn from_prepared(
        receiver: &str,
        field: &Feature,
        fields: &dyn super::PreparedFeatureLookup,
    ) -> Result<Self, String> {
        Self::from_prepared_observed(receiver, field, fields, &InvocationContext::default())
            .map_err(|error| error.to_string())
    }
    pub fn from_prepared_observed(
        receiver: &str,
        field: &Feature,
        fields: &dyn super::PreparedFeatureLookup,
        observation: &InvocationContext<'_>,
    ) -> Result<Self, PreparationError> {
        if fields.receiver() != receiver || fields.get(&field.name) != Some(field) {
            return Err("Attribute does not belong to the receiver's effective features".into());
        }
        reserve(observation, 0, &[receiver])?;
        reserve_value(field, observation)?;
        Ok(Self::with_members(receiver, field, None))
    }
    pub fn advance(
        &mut self,
        schema: &crate::definitions::schema::SchemaRepresentation<impl super::RuntimeAlternative>,
        conversions: Option<&ConversionRegistry<'_>>,
    ) -> Result<bool, String> {
        self.advance_observed(schema, conversions, &InvocationContext::default())
            .map_err(|error| error.to_string())
    }
    pub fn advance_observed(
        &mut self,
        schema: &crate::definitions::schema::SchemaRepresentation<impl super::RuntimeAlternative>,
        conversions: Option<&ConversionRegistry<'_>>,
        observation: &InvocationContext<'_>,
    ) -> Result<bool, PreparationError> {
        if self.failed {
            return Err("Runtime attribute conversion preparation has failed".into());
        }
        let result = observation
            .checkpoint()
            .map_err(PreparationError::Observation)
            .and_then(|()| {
                self.advance_inner_with_sources(
                    schema,
                    conversions,
                    observation,
                    &NoAttributeSourceInputs,
                )
            });
        if result.is_err() {
            self.failed = true;
        }
        result
    }
}
impl<F: AttributeFieldOwner> RuntimeAttributeConversionPreparation<F> {
    fn with_owner_members(
        receiver: &str,
        field: F,
        members: Option<FeatureMapPreparation>,
    ) -> Self {
        Self {
            receiver: receiver.into(),
            field,
            stage: if members.is_some() {
                RuntimeStage::Members
            } else {
                RuntimeStage::Domains
            },
            members: members.map(Box::new),
            fields: None,
            domains: vec![],
            cursor: 0,
            domain: 0,
            alternative: 0,
            codec: None,
            unconstrained: false,
            retain_domains: false,
            failed: false,
        }
    }
    pub(crate) fn from_prepared_with_sources<S: AttributeSourceInputs<Field = F>>(
        receiver: &str,
        field: &Feature,
        fields: &dyn super::PreparedFeatureLookup,
        observation: &InvocationContext<'_>,
        sources: &S,
    ) -> Result<Self, PreparationError> {
        sources.complete(
            (|| {
                if fields.receiver() != receiver || fields.get(&field.name) != Some(field) {
                    return Err(
                        "Attribute does not belong to the receiver's effective features".into(),
                    );
                }
                reserve(observation, 0, &[receiver])?;
                let field = sources.acquire_field(
                    field,
                    AttributeFieldOrigin::EffectiveMember,
                    observation,
                )?;
                Ok(Self::with_owner_members(receiver, field, None))
            })(),
            observation,
        )
    }
    fn from_selection_with_sources<S: AttributeSourceInputs<Field = F>>(
        receiver: &str,
        field: &F,
        domains: Vec<usize>,
        observation: &InvocationContext<'_>,
        sources: &S,
    ) -> Result<Self, PreparationError> {
        sources.complete(
            (|| {
                reserve(observation, 0, &[receiver])?;
                let field = sources.copy_field(field, observation)?;
                let mut pending = Self::with_owner_members(receiver, field, None);
                pending.domains = domains;
                pending.retain_domains = true;
                pending.stage = RuntimeStage::Ready;
                Ok(pending)
            })(),
            observation,
        )
    }
    fn advance_inner_with_sources<S: AttributeSourceInputs<Field = F>>(
        &mut self,
        schema: &crate::definitions::schema::SchemaRepresentation<impl super::RuntimeAlternative>,
        conversions: Option<&ConversionRegistry<'_>>,
        observation: &InvocationContext<'_>,
        sources: &S,
    ) -> Result<bool, PreparationError> {
        sources.check_field_owner(schema, &self.field, observation)?;
        match self.stage {
            RuntimeStage::Members => {
                if self
                    .members
                    .as_mut()
                    .ok_or("Missing runtime attribute members")?
                    .advance_observed(schema, observation)?
                {
                    self.fields = Some(
                        self.members
                            .take()
                            .ok_or("Missing runtime attribute members")?
                            .finish()?,
                    );
                    self.stage = RuntimeStage::Check;
                }
            }
            RuntimeStage::Check => {
                if self
                    .fields
                    .as_ref()
                    .ok_or("Missing runtime attribute fields")?
                    .get(&self.field.field().name)
                    != Some(self.field.field())
                {
                    return Err(
                        "Attribute does not belong to the receiver's effective features".into(),
                    );
                }
                self.stage = RuntimeStage::Clear;
            }
            RuntimeStage::Clear => {
                if self
                    .fields
                    .as_mut()
                    .ok_or("Missing runtime attribute fields")?
                    .clear_one()
                {
                    self.fields = None;
                    self.stage = RuntimeStage::Domains;
                }
            }
            RuntimeStage::Domains => {
                if S::ENABLED
                    && let Some(next) = sources.domain_at(
                        schema,
                        &self.receiver,
                        self.field.field(),
                        self.cursor,
                        observation,
                    )?
                {
                    if let Some(index) = next {
                        reserve(observation, 32, &[])?;
                        allocation(&mut self.domains, 1)?;
                        self.domains.push(index);
                        self.cursor = index
                            .checked_add(1)
                            .ok_or("Runtime source domain cursor overflow")?;
                    } else {
                        self.stage = RuntimeStage::Ready;
                    }
                    return Ok(false);
                }
                if let Some(domain) = schema
                    .runtime_feature_domains
                    .get(&self.receiver)
                    .and_then(|domains| domains.get(self.cursor))
                {
                    if domain.source.view().projection.source.name == self.field.field().name {
                        reserve(observation, 32, &[])?;
                        allocation(&mut self.domains, 1)?;
                        self.domains.push(self.cursor);
                    }
                    self.cursor += 1;
                } else {
                    if S::ENABLED {
                        sources.remember_domains(
                            schema,
                            &self.receiver,
                            self.field.field(),
                            &self.domains,
                            observation,
                        )?;
                    }
                    self.stage = RuntimeStage::Ready;
                }
            }
            RuntimeStage::Ready => {
                if self.domains.is_empty() {
                    self.unconstrained = true;
                    self.codec = Some(Box::new(
                        AttributeConversionPreparation::from_owner_with_sources(
                            &self.field,
                            observation,
                            sources,
                        )?,
                    ));
                    self.stage = RuntimeStage::Codec;
                } else if self.field.field().reference {
                    self.stage = if self.retain_domains {
                        RuntimeStage::Done
                    } else {
                        RuntimeStage::ClearDomains
                    };
                } else {
                    self.stage = RuntimeStage::Alternatives;
                }
            }
            RuntimeStage::Alternatives => {
                if let Some(index) = self.domains.get(self.domain) {
                    let domain = schema
                        .runtime_feature_domains
                        .get(&self.receiver)
                        .and_then(|domains| domains.get(*index))
                        .ok_or("Missing runtime attribute domain")?;
                    if let Some(target) =
                        domain.alternatives.get(self.alternative).map(AsRef::as_ref)
                    {
                        let alternative = sources.alternative(
                            schema,
                            self.field.field(),
                            *index,
                            self.alternative,
                            target,
                            observation,
                        )?;
                        self.codec =
                            Some(Box::new(AttributeConversionPreparation::new_with_sources(
                                &alternative,
                                observation,
                                sources,
                            )?));
                        self.stage = RuntimeStage::Codec;
                    } else {
                        self.domain += 1;
                        self.alternative = 0;
                    }
                } else {
                    self.stage = if self.retain_domains {
                        RuntimeStage::Done
                    } else {
                        RuntimeStage::ClearDomains
                    };
                }
            }
            RuntimeStage::Codec => {
                if self
                    .codec
                    .as_mut()
                    .ok_or("Missing attribute codec preparation")?
                    .advance_with_sources(schema, conversions, observation, sources)?
                {
                    self.codec
                        .take()
                        .ok_or("Missing attribute codec preparation")?
                        .finish()?;
                    if self.unconstrained {
                        self.stage = RuntimeStage::Done;
                    } else {
                        self.alternative += 1;
                        self.stage = RuntimeStage::Alternatives;
                    }
                }
            }
            RuntimeStage::ClearDomains => {
                self.domains.pop();
                if self.domains.is_empty() {
                    self.stage = RuntimeStage::Done;
                }
            }
            RuntimeStage::Done => return Ok(true),
        }
        Ok(matches!(self.stage, RuntimeStage::Done))
    }
    pub fn finish(self) -> Result<(), String> {
        if self.failed || !matches!(self.stage, RuntimeStage::Done) {
            return Err("Runtime attribute conversion preparation is not complete".into());
        }
        Ok(())
    }
    fn selection_ready(&self) -> bool {
        !self.failed && matches!(self.stage, RuntimeStage::Ready)
    }
    fn finish_selection(self) -> Result<Vec<usize>, String> {
        if !self.selection_ready() {
            return Err("Runtime attribute selection is not complete".into());
        }
        Ok(self.domains)
    }
    fn finish_domains(self) -> Result<Vec<usize>, String> {
        if self.failed || !matches!(self.stage, RuntimeStage::Done) {
            return Err("Runtime attribute conversion preparation is not complete".into());
        }
        Ok(self.domains)
    }
    pub(crate) fn advance_with_sources<S: AttributeSourceInputs<Field = F>>(
        &mut self,
        schema: &crate::definitions::schema::SchemaRepresentation<impl super::RuntimeAlternative>,
        conversions: Option<&ConversionRegistry<'_>>,
        observation: &InvocationContext<'_>,
        sources: &S,
    ) -> Result<bool, PreparationError> {
        if self.failed {
            return sources.complete(
                Err("Runtime attribute conversion preparation has failed".into()),
                observation,
            );
        }
        let result = observation
            .checkpoint()
            .map_err(PreparationError::Observation)
            .and_then(|()| {
                self.advance_inner_with_sources(schema, conversions, observation, sources)
            });
        let result = sources.complete(result, observation);
        if result.is_err() {
            self.failed = true;
        }
        result
    }
}

enum AttributeValueStage {
    Codec,
    Payload,
    Done,
}
/// Validate one immutable present value with shared retained codec selection.
/// Payload decoding and individual native codec calls remain atomic.
pub struct AttributeValidationPreparation<F: AttributeFieldOwner = Feature> {
    field: F,
    codec: Option<Box<AttributeConversionPreparation<F>>>,
    stage: AttributeValueStage,
    failed: bool,
}
impl AttributeValidationPreparation {
    pub fn new(field: &Feature) -> Self {
        Self::with_codec(field, AttributeConversionPreparation::new(field))
    }
    pub fn new_observed(
        field: &Feature,
        observation: &InvocationContext<'_>,
    ) -> Result<Self, PreparationError> {
        reserve_value(field, observation)?;
        let codec = AttributeConversionPreparation::new_observed(field, observation)?;
        Ok(Self::with_codec(field, codec))
    }
    fn with_codec(field: &Feature, codec: AttributeConversionPreparation) -> Self {
        Self {
            field: field.clone(),
            codec: Some(Box::new(codec)),
            stage: AttributeValueStage::Codec,
            failed: false,
        }
    }
    /// Keep schema, value and registry immutable between advances.
    pub fn advance(
        &mut self,
        schema: &crate::definitions::schema::SchemaRepresentation<impl super::RuntimeAlternative>,
        value: &Value,
        conversions: Option<&ConversionRegistry<'_>>,
        observation: Option<&InvocationContext<'_>>,
    ) -> Result<bool, String> {
        let unobserved = InvocationContext::default();
        self.advance_observed(
            schema,
            value,
            conversions,
            observation.unwrap_or(&unobserved),
        )
        .map_err(|error| error.to_string())
    }
    pub fn advance_observed(
        &mut self,
        schema: &crate::definitions::schema::SchemaRepresentation<impl super::RuntimeAlternative>,
        value: &Value,
        conversions: Option<&ConversionRegistry<'_>>,
        observation: &InvocationContext<'_>,
    ) -> Result<bool, PreparationError> {
        if self.failed {
            return Err("Attribute validation preparation has failed".into());
        }
        let result = self.advance_inner_with_sources(
            schema,
            value,
            conversions,
            observation,
            &NoAttributeSourceInputs,
        );
        if result.is_err() {
            self.failed = true;
        }
        result
    }
}
impl<F: AttributeFieldOwner> AttributeValidationPreparation<F> {
    fn with_owner_codec(field: F, codec: AttributeConversionPreparation<F>) -> Self {
        Self {
            field,
            codec: Some(Box::new(codec)),
            stage: AttributeValueStage::Codec,
            failed: false,
        }
    }
    fn from_owner_with_sources<S: AttributeSourceInputs<Field = F>>(
        field: &F,
        observation: &InvocationContext<'_>,
        sources: &S,
    ) -> Result<Self, PreparationError> {
        sources.complete(
            (|| {
                let field = sources.copy_field(field, observation)?;
                let codec = AttributeConversionPreparation::from_owner_with_sources(
                    &field,
                    observation,
                    sources,
                )?;
                Ok(Self::with_owner_codec(field, codec))
            })(),
            observation,
        )
    }
    fn new_with_sources<S: AttributeSourceInputs<Field = F>>(
        field: &Feature,
        observation: &InvocationContext<'_>,
        sources: &S,
    ) -> Result<Self, PreparationError> {
        sources.complete(
            (|| {
                let field =
                    sources.acquire_field(field, AttributeFieldOrigin::Synthetic, observation)?;
                let codec = AttributeConversionPreparation::from_owner_with_sources(
                    &field,
                    observation,
                    sources,
                )?;
                Ok(Self::with_owner_codec(field, codec))
            })(),
            observation,
        )
    }
    fn advance_inner_with_sources<S: AttributeSourceInputs<Field = F>>(
        &mut self,
        schema: &crate::definitions::schema::SchemaRepresentation<impl super::RuntimeAlternative>,
        value: &Value,
        conversions: Option<&ConversionRegistry<'_>>,
        observation: &InvocationContext<'_>,
        sources: &S,
    ) -> Result<bool, PreparationError> {
        sources.check_field_owner(schema, &self.field, observation)?;
        observation
            .checkpoint()
            .map_err(PreparationError::Observation)?;
        if matches!(self.stage, AttributeValueStage::Done) {
            return Ok(true);
        }
        observation.work(1).map_err(PreparationError::Observation)?;
        match self.stage {
            AttributeValueStage::Codec => {
                if self
                    .codec
                    .as_mut()
                    .ok_or("Missing attribute codec")?
                    .advance_with_sources(schema, conversions, observation, sources)?
                {
                    self.codec
                        .take()
                        .ok_or("Missing attribute codec")?
                        .finish()?;
                    self.stage = AttributeValueStage::Payload;
                }
            }
            AttributeValueStage::Payload => {
                let result = schema.validate_attribute_payload(
                    self.field.field(),
                    value,
                    conversions,
                    observation.has_observer().then_some(observation),
                );
                // Native providers may swallow terminal observer failures.
                observation
                    .checkpoint()
                    .map_err(PreparationError::Observation)?;
                result?;
                self.stage = AttributeValueStage::Done;
            }
            AttributeValueStage::Done => return Ok(true),
        }
        Ok(matches!(self.stage, AttributeValueStage::Done))
    }
    pub fn finish(self) -> Result<(), String> {
        if self.failed || !matches!(self.stage, AttributeValueStage::Done) {
            return Err("Attribute validation preparation is not complete".into());
        }
        Ok(())
    }
    /// SAME nested worker, with an explicit result for the canonical chooser.
    /// The adapter lives only for this call and stores no graph/provider facts.
    fn advance_candidate_with_sources<S: AttributeSourceInputs<Field = F>>(
        &mut self,
        schema: &crate::definitions::schema::SchemaRepresentation<impl super::RuntimeAlternative>,
        value: &Value,
        conversions: Option<&ConversionRegistry<'_>>,
        observation: &InvocationContext<'_>,
        sources: &S,
    ) -> AttributeCandidateOutcome<bool> {
        if !S::ENABLED {
            // Same ordinary failure wrapper and canonical worker; a runtime
            // const branch cannot narrow generic ownership to Feature.
            let result = if self.failed {
                Err("Attribute validation preparation has failed".into())
            } else {
                let result = self.advance_inner_with_sources(
                    schema,
                    value,
                    conversions,
                    observation,
                    sources,
                );
                if result.is_err() {
                    self.failed = true;
                }
                result
            };
            return sources.candidate_outcome(result, observation);
        }
        if self.failed {
            return match sources.complete(
                Err("Attribute validation preparation has failed".into()),
                observation,
            ) {
                Err(error) => AttributeCandidateOutcome::Terminal(error),
                Ok(value) => AttributeCandidateOutcome::Advanced(value),
            };
        }
        let candidate = CanonicalCandidateInputs::new(sources);
        let result =
            self.advance_inner_with_sources(schema, value, conversions, observation, &candidate);
        let result = sources.candidate_outcome(result, observation);
        if matches!(
            &result,
            AttributeCandidateOutcome::Rejected(_) | AttributeCandidateOutcome::Terminal(_)
        ) {
            self.failed = true;
        }
        result
    }
}

enum RuntimeValueStage {
    Select,
    Header,
    Qualify,
    Domain,
    Alternative,
    Validate,
    Unresolved,
    ValidateUnresolved,
    Clear,
    Done,
}
/// Validate one immutable value against every receiver-specific domain.
/// Member/default selection, policy rows, codec alternatives, rejected value
/// alternatives and unresolved candidates yield. Only finish publishes success.
pub struct RuntimeAttributeValidationPreparation<F: AttributeFieldOwner = Feature> {
    receiver: String,
    field: F,
    stage: RuntimeValueStage,
    qualification: Option<Box<RuntimeAttributeConversionPreparation<F>>>,
    validation: Option<Box<AttributeValidationPreparation<F>>>,
    domains: Vec<usize>,
    domain: usize,
    alternative: usize,
    unresolved: usize,
    unconstrained: bool,
    failed: bool,
}
impl RuntimeAttributeValidationPreparation {
    pub fn new(receiver: &str, field: &Feature) -> Self {
        Self::with_selection(
            receiver,
            field,
            RuntimeAttributeConversionPreparation::new(receiver, field),
        )
    }
    pub fn new_observed(
        receiver: &str,
        field: &Feature,
        observation: &InvocationContext<'_>,
    ) -> Result<Self, PreparationError> {
        reserve(observation, 0, &[receiver])?;
        reserve_value(field, observation)?;
        let selection =
            RuntimeAttributeConversionPreparation::new_observed(receiver, field, observation)?;
        Ok(Self::with_selection(receiver, field, selection))
    }
    /// Reuse a completed projection from the same immutable schema. Its receiver
    /// and full feature/default equality are checked by the shared selection.
    pub fn from_prepared(
        receiver: &str,
        field: &Feature,
        fields: &dyn super::PreparedFeatureLookup,
    ) -> Result<Self, String> {
        Self::from_prepared_observed(receiver, field, fields, &InvocationContext::default())
            .map_err(|error| error.to_string())
    }
    pub fn from_prepared_observed(
        receiver: &str,
        field: &Feature,
        fields: &dyn super::PreparedFeatureLookup,
        observation: &InvocationContext<'_>,
    ) -> Result<Self, PreparationError> {
        let selection = RuntimeAttributeConversionPreparation::from_prepared_observed(
            receiver,
            field,
            fields,
            observation,
        )?;
        reserve(observation, 0, &[receiver])?;
        reserve_value(field, observation)?;
        Ok(Self::with_selection(receiver, field, selection))
    }
    fn with_selection(
        receiver: &str,
        field: &Feature,
        selection: RuntimeAttributeConversionPreparation,
    ) -> Self {
        Self {
            receiver: receiver.into(),
            field: field.clone(),
            stage: RuntimeValueStage::Select,
            qualification: Some(Box::new(selection)),
            validation: None,
            domains: vec![],
            domain: 0,
            alternative: 0,
            unresolved: 0,
            unconstrained: false,
            failed: false,
        }
    }
    /// Keep the schema, value and provider registry immutable between advances.
    pub fn advance(
        &mut self,
        schema: &crate::definitions::schema::SchemaRepresentation<impl super::RuntimeAlternative>,
        value: &Value,
        conversions: Option<&ConversionRegistry<'_>>,
        observation: Option<&InvocationContext<'_>>,
    ) -> Result<bool, String> {
        let unobserved = InvocationContext::default();
        self.advance_observed(
            schema,
            value,
            conversions,
            observation.unwrap_or(&unobserved),
        )
        .map_err(|error| error.to_string())
    }
    pub fn advance_observed(
        &mut self,
        schema: &crate::definitions::schema::SchemaRepresentation<impl super::RuntimeAlternative>,
        value: &Value,
        conversions: Option<&ConversionRegistry<'_>>,
        observation: &InvocationContext<'_>,
    ) -> Result<bool, PreparationError> {
        if self.failed {
            return Err("Runtime attribute validation preparation has failed".into());
        }
        let result = self.advance_inner_with_sources(
            schema,
            value,
            conversions,
            observation,
            &NoAttributeSourceInputs,
        );
        if result.is_err() {
            self.failed = true;
        }
        result
    }
}
impl<F: AttributeFieldOwner> RuntimeAttributeValidationPreparation<F> {
    fn with_owner_selection(
        receiver: &str,
        field: F,
        selection: RuntimeAttributeConversionPreparation<F>,
    ) -> Self {
        Self {
            receiver: receiver.into(),
            field,
            stage: RuntimeValueStage::Select,
            qualification: Some(Box::new(selection)),
            validation: None,
            domains: vec![],
            domain: 0,
            alternative: 0,
            unresolved: 0,
            unconstrained: false,
            failed: false,
        }
    }
    pub(crate) fn from_prepared_with_sources<S: AttributeSourceInputs<Field = F>>(
        receiver: &str,
        field: &Feature,
        fields: &dyn super::PreparedFeatureLookup,
        observation: &InvocationContext<'_>,
        sources: &S,
    ) -> Result<Self, PreparationError> {
        sources.complete(
            (|| {
                let selection = RuntimeAttributeConversionPreparation::from_prepared_with_sources(
                    receiver,
                    field,
                    fields,
                    observation,
                    sources,
                )?;
                reserve(observation, 0, &[receiver])?;
                let field = sources.acquire_field(
                    field,
                    AttributeFieldOrigin::EffectiveMember,
                    observation,
                )?;
                Ok(Self::with_owner_selection(receiver, field, selection))
            })(),
            observation,
        )
    }
    fn current_domain<'a, A: super::RuntimeAlternative>(
        &self,
        schema: &'a crate::definitions::schema::SchemaRepresentation<A>,
    ) -> Result<&'a super::CheckedRuntimeFeatureDomainRepresentation<A>, String> {
        schema
            .runtime_feature_domains
            .get(&self.receiver)
            .and_then(|domains| {
                self.domains
                    .get(self.domain)
                    .and_then(|index| domains.get(*index))
            })
            .ok_or_else(|| "Missing runtime attribute domain".into())
    }
    fn advance_inner_with_sources<S: AttributeSourceInputs<Field = F>>(
        &mut self,
        schema: &crate::definitions::schema::SchemaRepresentation<impl super::RuntimeAlternative>,
        value: &Value,
        conversions: Option<&ConversionRegistry<'_>>,
        observation: &InvocationContext<'_>,
        sources: &S,
    ) -> Result<bool, PreparationError> {
        sources.check_field_owner(schema, &self.field, observation)?;
        observation
            .checkpoint()
            .map_err(PreparationError::Observation)?;
        if matches!(self.stage, RuntimeValueStage::Done) {
            return Ok(true);
        }
        // Nested validation charges its own transitions. Every other stage is
        // one selection, policy, candidate or cleanup transition.
        if !matches!(
            self.stage,
            RuntimeValueStage::Validate | RuntimeValueStage::ValidateUnresolved
        ) {
            observation.work(1).map_err(PreparationError::Observation)?;
        }
        match self.stage {
            RuntimeValueStage::Select => {
                let selection = self
                    .qualification
                    .as_mut()
                    .ok_or("Missing attribute selection")?;
                selection.advance_with_sources(schema, conversions, observation, sources)?;
                if selection.selection_ready() {
                    self.domains = self
                        .qualification
                        .take()
                        .ok_or("Missing attribute selection")?
                        .finish_selection()?;
                    self.stage = RuntimeValueStage::Header;
                }
            }
            RuntimeValueStage::Header => {
                if self.domains.is_empty() {
                    self.unconstrained = true;
                    self.validation = Some(Box::new(
                        AttributeValidationPreparation::from_owner_with_sources(
                            &self.field,
                            observation,
                            sources,
                        )?,
                    ));
                    self.stage = RuntimeValueStage::Validate;
                } else {
                    // Constrained fields reject reference/null before codec
                    // requirements; unconstrained fields require the codec first.
                    if self.field.field().reference {
                        return Err("Reference is not an attribute".into());
                    }
                    if value.is_null() {
                        return Err("Null is absence, not a present attribute value".into());
                    }
                    self.qualification = Some(Box::new(
                        RuntimeAttributeConversionPreparation::from_selection_with_sources(
                            &self.receiver,
                            &self.field,
                            std::mem::take(&mut self.domains),
                            observation,
                            sources,
                        )?,
                    ));
                    self.stage = RuntimeValueStage::Qualify;
                }
            }
            RuntimeValueStage::Qualify => {
                if self
                    .qualification
                    .as_mut()
                    .ok_or("Missing attribute qualification")?
                    .advance_with_sources(schema, conversions, observation, sources)?
                {
                    self.domains = self
                        .qualification
                        .take()
                        .ok_or("Missing attribute qualification")?
                        .finish_domains()?;
                    self.stage = RuntimeValueStage::Domain;
                }
            }
            RuntimeValueStage::Domain => {
                if self.domain < self.domains.len() {
                    self.alternative = 0;
                    self.unresolved = 0;
                    self.stage = RuntimeValueStage::Alternative;
                } else {
                    self.stage = RuntimeValueStage::Clear;
                }
            }
            RuntimeValueStage::Alternative => {
                let domain = self.current_domain(schema)?;
                if let Some(target) = domain.alternatives.get(self.alternative).map(AsRef::as_ref) {
                    let alternative = sources.alternative(
                        schema,
                        self.field.field(),
                        self.domains[self.domain],
                        self.alternative,
                        target,
                        observation,
                    )?;
                    self.validation =
                        Some(Box::new(AttributeValidationPreparation::new_with_sources(
                            &alternative,
                            observation,
                            sources,
                        )?));
                    self.stage = RuntimeValueStage::Validate;
                } else {
                    self.stage = RuntimeValueStage::Unresolved;
                }
            }
            RuntimeValueStage::Validate => {
                let result = self
                    .validation
                    .as_mut()
                    .ok_or("Missing attribute value validation")?
                    .advance_candidate_with_sources(
                        schema,
                        value,
                        conversions,
                        observation,
                        sources,
                    );
                observation
                    .checkpoint()
                    .map_err(PreparationError::Observation)?;
                match result {
                    AttributeCandidateOutcome::Advanced(false) => {}
                    AttributeCandidateOutcome::Advanced(true) => {
                        self.validation
                            .take()
                            .ok_or("Missing attribute value validation")?
                            .finish()?;
                        if self.unconstrained {
                            self.stage = RuntimeValueStage::Done;
                        } else {
                            self.domain += 1;
                            self.stage = RuntimeValueStage::Domain;
                        }
                    }
                    AttributeCandidateOutcome::Rejected(error) if self.unconstrained => {
                        return Err(PreparationError::Schema(error));
                    }
                    AttributeCandidateOutcome::Terminal(error) => return Err(error),
                    AttributeCandidateOutcome::Rejected(_) => {
                        self.validation = None;
                        self.alternative += 1;
                        self.stage = RuntimeValueStage::Alternative;
                    }
                }
            }
            RuntimeValueStage::Unresolved => {
                let domain = self.current_domain(schema)?;
                let candidate = domain
                    .unresolved
                    .get(self.unresolved)
                    .ok_or("Value does not satisfy the receiver-specific feature domain")?;
                reserve_value(value, observation)?;
                let actual_matches = if let Ok(applied) =
                    crate::definitions::conversion::AppliedConvertedValue::deserialize(value)
                {
                    applied.application().expression() == candidate.candidate.expression()
                } else {
                    let actual = if let Some(id) = value.get("datatype") {
                        // The failed applied decode may have copied partial input.
                        // Reserve this independent fallback before decoding it.
                        reserve_value(id, observation)?;
                        crate::definitions::resources::DeclarationId::deserialize(id).ok()
                    } else {
                        None
                    };
                    actual.as_ref()
                        == match candidate.candidate.expression() {
                            crate::definitions::generic_types::Type::Classifier {
                                declaration,
                                ..
                            } => Some(declaration),
                            _ => None,
                        }
                };
                if actual_matches {
                    let alternative = schema.domain_alternative_observed(
                        self.field.field(),
                        &candidate.target,
                        observation,
                    )?;
                    self.validation =
                        Some(Box::new(AttributeValidationPreparation::new_with_sources(
                            &alternative,
                            observation,
                            sources,
                        )?));
                    self.stage = RuntimeValueStage::ValidateUnresolved;
                } else {
                    self.unresolved += 1;
                }
            }
            RuntimeValueStage::ValidateUnresolved => {
                let result = self
                    .validation
                    .as_mut()
                    .ok_or("Missing unresolved attribute validation")?
                    .advance_candidate_with_sources(
                        schema,
                        value,
                        conversions,
                        observation,
                        sources,
                    );
                observation
                    .checkpoint()
                    .map_err(PreparationError::Observation)?;
                let complete = match result {
                    AttributeCandidateOutcome::Advanced(complete) => complete,
                    AttributeCandidateOutcome::Rejected(error) => {
                        return Err(PreparationError::Schema(error));
                    }
                    AttributeCandidateOutcome::Terminal(error) => return Err(error),
                };
                if complete {
                    self.validation
                        .take()
                        .ok_or("Missing unresolved attribute validation")?
                        .finish()?;
                    let candidate = self
                        .current_domain(schema)?
                        .unresolved
                        .get(self.unresolved)
                        .ok_or("Missing unresolved attribute candidate")?;
                    return Err(format!(
                        "Unresolved runtime feature membership: {}",
                        candidate.reason
                    )
                    .into());
                }
            }
            RuntimeValueStage::Clear => {
                self.domains.pop();
                if self.domains.is_empty() {
                    self.stage = RuntimeValueStage::Done;
                }
            }
            RuntimeValueStage::Done => return Ok(true),
        }
        Ok(matches!(self.stage, RuntimeValueStage::Done))
    }
    pub fn finish(self) -> Result<(), String> {
        if self.failed || !matches!(self.stage, RuntimeValueStage::Done) {
            return Err("Runtime attribute validation preparation is not complete".into());
        }
        Ok(())
    }
    pub(crate) fn advance_with_sources<S: AttributeSourceInputs<Field = F>>(
        &mut self,
        schema: &crate::definitions::schema::SchemaRepresentation<impl super::RuntimeAlternative>,
        value: &Value,
        conversions: Option<&ConversionRegistry<'_>>,
        observation: &InvocationContext<'_>,
        sources: &S,
    ) -> Result<bool, PreparationError> {
        if self.failed {
            return sources.complete(
                Err("Runtime attribute validation preparation has failed".into()),
                observation,
            );
        }
        let result =
            self.advance_inner_with_sources(schema, value, conversions, observation, sources);
        let result = sources.complete(result, observation);
        if result.is_err() {
            self.failed = true;
        }
        result
    }
}

// Exact pre-change default holder field lists; no new ordinary owner allocation.
#[cfg(any(test, target_arch = "wasm32"))]
#[allow(dead_code)]
struct LegacyAttributeConversionPreparation {
    field: Feature,
    stage: ConversionStage,
    applied: Option<Box<AppliedDatatypeContractPreparation>>,
    selected: Option<ConversionContract>,
    cursor: usize,
    matched: bool,
    failed: bool,
}
#[cfg(any(test, target_arch = "wasm32"))]
const _: () = {
    assert!(
        std::mem::size_of::<AttributeConversionPreparation>()
            == std::mem::size_of::<LegacyAttributeConversionPreparation>()
    );
    assert!(
        std::mem::align_of::<AttributeConversionPreparation>()
            == std::mem::align_of::<LegacyAttributeConversionPreparation>()
    );
};
#[cfg(any(test, target_arch = "wasm32"))]
#[allow(dead_code)]
struct LegacyRuntimeAttributeConversionPreparation {
    receiver: String,
    field: Feature,
    stage: RuntimeStage,
    members: Option<Box<FeatureMapPreparation>>,
    fields: Option<PreparedFeatures>,
    domains: Vec<usize>,
    cursor: usize,
    domain: usize,
    alternative: usize,
    codec: Option<Box<AttributeConversionPreparation>>,
    unconstrained: bool,
    retain_domains: bool,
    failed: bool,
}
#[cfg(any(test, target_arch = "wasm32"))]
const _: () = {
    assert!(
        std::mem::size_of::<RuntimeAttributeConversionPreparation>()
            == std::mem::size_of::<LegacyRuntimeAttributeConversionPreparation>()
    );
    assert!(
        std::mem::align_of::<RuntimeAttributeConversionPreparation>()
            == std::mem::align_of::<LegacyRuntimeAttributeConversionPreparation>()
    );
};
#[cfg(any(test, target_arch = "wasm32"))]
#[allow(dead_code)]
struct LegacyAttributeValidationPreparation {
    field: Feature,
    codec: Option<Box<AttributeConversionPreparation>>,
    stage: AttributeValueStage,
    failed: bool,
}
#[cfg(any(test, target_arch = "wasm32"))]
const _: () = {
    assert!(
        std::mem::size_of::<AttributeValidationPreparation>()
            == std::mem::size_of::<LegacyAttributeValidationPreparation>()
    );
    assert!(
        std::mem::align_of::<AttributeValidationPreparation>()
            == std::mem::align_of::<LegacyAttributeValidationPreparation>()
    );
};
#[cfg(any(test, target_arch = "wasm32"))]
#[allow(dead_code)]
struct LegacyRuntimeAttributeValidationPreparation {
    receiver: String,
    field: Feature,
    stage: RuntimeValueStage,
    qualification: Option<Box<RuntimeAttributeConversionPreparation>>,
    validation: Option<Box<AttributeValidationPreparation>>,
    domains: Vec<usize>,
    domain: usize,
    alternative: usize,
    unresolved: usize,
    unconstrained: bool,
    failed: bool,
}
#[cfg(any(test, target_arch = "wasm32"))]
const _: () = {
    assert!(
        std::mem::size_of::<RuntimeAttributeValidationPreparation>()
            == std::mem::size_of::<LegacyRuntimeAttributeValidationPreparation>()
    );
    assert!(
        std::mem::align_of::<RuntimeAttributeValidationPreparation>()
            == std::mem::align_of::<LegacyRuntimeAttributeValidationPreparation>()
    );
};
