//! Optional inputs for the SAME staged attribute/codec preparations.
//! Only completed immutable source facts may be reused. Live registries,
//! provider metadata, graph state and payload validation are never inputs here.
use super::observation::{PreparationError, reserve_value};
use super::{Feature, RuntimeAlternative, SchemaRepresentation};
use crate::definitions::{conversion::InvocationContext, datatypes::Datatype};

/// Read-only ownership contract for the canonical attribute cursors.
/// Ordinary concrete cursors keep Feature; prepared owners are crate-private.
#[doc(hidden)]
pub trait AttributeFieldOwner: field_owner_sealed::Sealed {
    fn field(&self) -> &Feature;
}
pub(crate) mod field_owner_sealed {
    pub trait Sealed {}
}
impl field_owner_sealed::Sealed for Feature {}
impl AttributeFieldOwner for Feature {
    fn field(&self) -> &Feature {
        self
    }
}
/// Explicit origin; structural equality never promotes a synthetic candidate.
pub(crate) enum AttributeFieldOrigin {
    EffectiveMember,
    Synthetic,
}

/// Private completed canonical serialization preflight. It grants no schema,
/// graph or codec authority. Every actual copy is admitted again before clone.
pub(crate) trait AttributeSourceInputs {
    type Field: AttributeFieldOwner;
    fn acquire_field(
        &self,
        field: &Feature,
        origin: AttributeFieldOrigin,
        observation: &InvocationContext<'_>,
    ) -> Result<Self::Field, PreparationError>;
    fn copy_field(
        &self,
        field: &Self::Field,
        observation: &InvocationContext<'_>,
    ) -> Result<Self::Field, PreparationError>;
    fn check_field_owner<A: RuntimeAlternative>(
        &self,
        schema: &SchemaRepresentation<A>,
        field: &Self::Field,
        observation: &InvocationContext<'_>,
    ) -> Result<(), PreparationError>;

    /// Private chooser boundary, not a general permission to swallow errors.
    /// Disabled callers retain the exact historical Schema retry behavior.
    fn candidate_outcome<T>(
        &self,
        result: Result<T, PreparationError>,
        _observation: &InvocationContext<'_>,
    ) -> AttributeCandidateOutcome<T> {
        match result {
            Ok(value) => AttributeCandidateOutcome::Advanced(value),
            Err(PreparationError::Schema(error)) => AttributeCandidateOutcome::Rejected(error),
            Err(error) => AttributeCandidateOutcome::Terminal(error),
        }
    }
    const ENABLED: bool = false;
    fn complete<T>(
        &self,
        result: Result<T, PreparationError>,
        _observation: &InvocationContext<'_>,
    ) -> Result<T, PreparationError> {
        result
    }
    fn reserve_feature(
        &self,
        field: &Feature,
        observation: &InvocationContext<'_>,
    ) -> Result<(), PreparationError> {
        reserve_value(field, observation)
    }
    fn reserve_datatype(
        &self,
        datatype: &Datatype,
        observation: &InvocationContext<'_>,
    ) -> Result<(), PreparationError> {
        reserve_value(datatype, observation)
    }
    fn domain_at<A: RuntimeAlternative>(
        &self,
        _schema: &SchemaRepresentation<A>,
        _receiver: &str,
        _field: &Feature,
        _cursor: usize,
        _observation: &InvocationContext<'_>,
    ) -> Result<Option<Option<usize>>, PreparationError> {
        Ok(None)
    }
    fn remember_domains<A: RuntimeAlternative>(
        &self,
        _schema: &SchemaRepresentation<A>,
        _receiver: &str,
        _field: &Feature,
        _domains: &[usize],
        _observation: &InvocationContext<'_>,
    ) -> Result<(), PreparationError> {
        Ok(())
    }
    fn alternative<A: RuntimeAlternative>(
        &self,
        schema: &SchemaRepresentation<A>,
        field: &Feature,
        _domain: usize,
        _alternative: usize,
        target: &str,
        observation: &InvocationContext<'_>,
    ) -> Result<Feature, PreparationError> {
        schema.domain_alternative_observed(field, target, observation)
    }
    fn applied_index<A: RuntimeAlternative>(
        &self,
        _schema: &SchemaRepresentation<A>,
        _datatype: &Datatype,
        _observation: &InvocationContext<'_>,
    ) -> Result<Option<usize>, PreparationError> {
        Ok(None)
    }
    fn remember_applied<A: RuntimeAlternative>(
        &self,
        _schema: &SchemaRepresentation<A>,
        _datatype: &Datatype,
        _index: usize,
        _observation: &InvocationContext<'_>,
    ) -> Result<(), PreparationError> {
        Ok(())
    }
    fn ordinary_index<A: RuntimeAlternative>(
        &self,
        _schema: &SchemaRepresentation<A>,
        _field: &Feature,
        _observation: &InvocationContext<'_>,
    ) -> Result<Option<usize>, PreparationError> {
        Ok(None)
    }
    fn observe_ordinary<A: RuntimeAlternative>(
        &self,
        _schema: &SchemaRepresentation<A>,
        _field: &Feature,
        _index: usize,
        _observation: &InvocationContext<'_>,
    ) -> Result<(), PreparationError> {
        Ok(())
    }
    fn remember_ordinary<A: RuntimeAlternative>(
        &self,
        _schema: &SchemaRepresentation<A>,
        _field: &Feature,
        _observation: &InvocationContext<'_>,
    ) -> Result<(), PreparationError> {
        Ok(())
    }
}

/// A result consumed by the existing constrained-alternative chooser. Terminal
/// carries the original error; it is never converted into a candidate mismatch.
pub(crate) enum AttributeCandidateOutcome<T> {
    Advanced(T),
    Rejected(String),
    Terminal(PreparationError),
}

/// Only a nested candidate job may borrow this adapter. All source operations
/// still enter the real source service and retain its immediate failure hooks.
/// Schema errors returned by the candidate worker itself remain available to
/// the canonical chooser; the real cursor decides whether it is still live.
pub(crate) struct CanonicalCandidateInputs<'a, S>(&'a S);
impl<'a, S> CanonicalCandidateInputs<'a, S> {
    pub(crate) fn new(source: &'a S) -> Self {
        Self(source)
    }
}
impl<S: AttributeSourceInputs> AttributeSourceInputs for CanonicalCandidateInputs<'_, S> {
    type Field = S::Field;
    fn acquire_field(
        &self,
        field: &Feature,
        origin: AttributeFieldOrigin,
        observation: &InvocationContext<'_>,
    ) -> Result<Self::Field, PreparationError> {
        self.0.acquire_field(field, origin, observation)
    }
    fn copy_field(
        &self,
        field: &Self::Field,
        observation: &InvocationContext<'_>,
    ) -> Result<Self::Field, PreparationError> {
        self.0.copy_field(field, observation)
    }
    fn check_field_owner<A: RuntimeAlternative>(
        &self,
        schema: &SchemaRepresentation<A>,
        field: &Self::Field,
        observation: &InvocationContext<'_>,
    ) -> Result<(), PreparationError> {
        self.0.check_field_owner(schema, field, observation)
    }

    const ENABLED: bool = S::ENABLED;
    fn complete<T>(
        &self,
        result: Result<T, PreparationError>,
        observation: &InvocationContext<'_>,
    ) -> Result<T, PreparationError> {
        match result {
            // This internal return is not yet a terminal operation. A real
            // source-helper error has ALREADY poisoned its owning source; the
            // candidate boundary below preserves that original terminal error.
            Err(error @ PreparationError::Schema(_)) => Err(error),
            other => self.0.complete(other, observation),
        }
    }
    fn candidate_outcome<T>(
        &self,
        result: Result<T, PreparationError>,
        observation: &InvocationContext<'_>,
    ) -> AttributeCandidateOutcome<T> {
        self.0.candidate_outcome(result, observation)
    }
    fn reserve_feature(
        &self,
        field: &Feature,
        observation: &InvocationContext<'_>,
    ) -> Result<(), PreparationError> {
        self.0.reserve_feature(field, observation)
    }
    fn reserve_datatype(
        &self,
        datatype: &Datatype,
        observation: &InvocationContext<'_>,
    ) -> Result<(), PreparationError> {
        self.0.reserve_datatype(datatype, observation)
    }
    fn domain_at<A: RuntimeAlternative>(
        &self,
        schema: &SchemaRepresentation<A>,
        receiver: &str,
        field: &Feature,
        cursor: usize,
        observation: &InvocationContext<'_>,
    ) -> Result<Option<Option<usize>>, PreparationError> {
        self.0
            .domain_at(schema, receiver, field, cursor, observation)
    }
    fn remember_domains<A: RuntimeAlternative>(
        &self,
        schema: &SchemaRepresentation<A>,
        receiver: &str,
        field: &Feature,
        domains: &[usize],
        observation: &InvocationContext<'_>,
    ) -> Result<(), PreparationError> {
        self.0
            .remember_domains(schema, receiver, field, domains, observation)
    }
    fn alternative<A: RuntimeAlternative>(
        &self,
        schema: &SchemaRepresentation<A>,
        field: &Feature,
        domain: usize,
        alternative: usize,
        target: &str,
        observation: &InvocationContext<'_>,
    ) -> Result<Feature, PreparationError> {
        self.0
            .alternative(schema, field, domain, alternative, target, observation)
    }
    fn applied_index<A: RuntimeAlternative>(
        &self,
        schema: &SchemaRepresentation<A>,
        datatype: &Datatype,
        observation: &InvocationContext<'_>,
    ) -> Result<Option<usize>, PreparationError> {
        self.0.applied_index(schema, datatype, observation)
    }
    fn remember_applied<A: RuntimeAlternative>(
        &self,
        schema: &SchemaRepresentation<A>,
        datatype: &Datatype,
        index: usize,
        observation: &InvocationContext<'_>,
    ) -> Result<(), PreparationError> {
        self.0
            .remember_applied(schema, datatype, index, observation)
    }
    fn ordinary_index<A: RuntimeAlternative>(
        &self,
        schema: &SchemaRepresentation<A>,
        field: &Feature,
        observation: &InvocationContext<'_>,
    ) -> Result<Option<usize>, PreparationError> {
        self.0.ordinary_index(schema, field, observation)
    }
    fn observe_ordinary<A: RuntimeAlternative>(
        &self,
        schema: &SchemaRepresentation<A>,
        field: &Feature,
        index: usize,
        observation: &InvocationContext<'_>,
    ) -> Result<(), PreparationError> {
        self.0.observe_ordinary(schema, field, index, observation)
    }
    fn remember_ordinary<A: RuntimeAlternative>(
        &self,
        schema: &SchemaRepresentation<A>,
        field: &Feature,
        observation: &InvocationContext<'_>,
    ) -> Result<(), PreparationError> {
        self.0.remember_ordinary(schema, field, observation)
    }
}

pub(crate) struct NoAttributeSourceInputs;
impl AttributeSourceInputs for NoAttributeSourceInputs {
    type Field = Feature;
    fn acquire_field(
        &self,
        field: &Feature,
        _: AttributeFieldOrigin,
        observation: &InvocationContext<'_>,
    ) -> Result<Feature, PreparationError> {
        self.reserve_feature(field, observation)?;
        Ok(field.clone())
    }
    fn copy_field(
        &self,
        field: &Feature,
        observation: &InvocationContext<'_>,
    ) -> Result<Feature, PreparationError> {
        self.reserve_feature(field, observation)?;
        Ok(field.clone())
    }
    fn check_field_owner<A: RuntimeAlternative>(
        &self,
        _: &SchemaRepresentation<A>,
        _: &Feature,
        _: &InvocationContext<'_>,
    ) -> Result<(), PreparationError> {
        Ok(())
    }
}
