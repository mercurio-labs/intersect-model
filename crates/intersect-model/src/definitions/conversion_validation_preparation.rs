//! Explicit provider-owned validation continuations. The registry owns dispatch;
//! language providers own validation semantics and their retained state.
use super::{
    ConversionPayload, ConversionProvider, ConversionRegistry, ConvertedValue, InvocationContext,
};
use serde_json::Value;
use std::num::NonZeroUsize;

/// One bounded, provider-defined validation transition.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConversionValidationStep {
    Pending,
    Complete,
}

/// Provider-owned state for the exact immutable input passed to its factory.
/// Do not retain an InvocationContext: each transition receives the current lease.
/// Every transition must bound its work and check control before semantic work;
/// child state and payload ownership must be admitted before allocation/copying.
pub trait ConversionValidationCursor {
    fn advance(
        &mut self,
        context: &InvocationContext<'_>,
    ) -> Result<ConversionValidationStep, String>;
}

/// An explicit cooperative implementation, never inferred from an observed hook.
/// This capability must implement the same validation and ordered fees/errors as
/// the provider's ordinary validation contract. Factory work is bounded. Declared
/// bytes cover its concrete boxed cursor; additional ownership is separately
/// precharged through the current context. Provider declarations do not prove
/// physical allocator, destructor, or foreign-code preemption guarantees.
pub trait ConversionValidationAdapter {
    fn cursor_bytes(&self) -> usize;
    fn begin<'a>(
        &'a self,
        value: &'a Value,
        context: &InvocationContext<'_>,
    ) -> Result<Box<dyn ConversionValidationCursor + 'a>, String>;
}

enum ProviderValidation<'a> {
    Start,
    Running(Box<dyn ConversionValidationCursor + 'a>),
    Finished,
}
impl<'a> ProviderValidation<'a> {
    fn advance(
        &mut self,
        provider: &'a dyn ConversionProvider,
        value: &'a Value,
        context: &InvocationContext<'_>,
    ) -> Result<ConversionValidationStep, String> {
        match self {
            Self::Start => {
                let Some(adapter) = provider.retained_validation() else {
                    provider.validate_observed(value, context)?;
                    *self = Self::Finished;
                    return Ok(ConversionValidationStep::Complete);
                };
                let bytes = adapter.cursor_bytes();
                context.retain_storage(bytes)?;
                let cursor = adapter.begin(value, context)?;
                if std::mem::size_of_val(cursor.as_ref()) > bytes {
                    return Err("Conversion validation cursor exceeds its declared state".into());
                }
                *self = Self::Running(cursor);
                Ok(ConversionValidationStep::Pending)
            }
            Self::Running(cursor) => cursor.advance(context),
            Self::Finished => Err("Conversion validation already finished".into()),
        }
    }
}

/// Complete output is exposed once. A later resume has no provider/observer work.
pub enum ConversionValidationProgress<'a> {
    Pending,
    Complete(Result<&'a Value, String>),
    Finished,
}

/// Only the two canonical input representations can authorize a retained job.
/// The by-value form owns metadata and borrows payload from the external source;
/// no cursor may borrow metadata stored inside the movable job itself.
mod input {
    pub trait Sealed {}
    impl<Payload: super::ConversionPayload> Sealed for &super::ConvertedValue<Payload> {}
    impl Sealed for super::ConvertedValue<&serde_json::Value> {}
}

/// Stable metadata and external payload authority for a validation job.
/// Implementations are sealed: a user-defined adapter cannot change the payload
/// identity between polls or forge a longer lifetime for movable metadata.
pub trait ConversionValidationInput<'a>: input::Sealed {
    fn metadata(&self) -> (&super::DeclarationId, &super::ProviderId);
    fn payload(&self) -> &'a Value;
}

impl<'a, Payload: ConversionPayload> ConversionValidationInput<'a> for &'a ConvertedValue<Payload> {
    fn metadata(&self) -> (&super::DeclarationId, &super::ProviderId) {
        let ConvertedValue::Converted {
            datatype, provider, ..
        } = *self;
        (datatype, provider)
    }

    fn payload(&self) -> &'a Value {
        let ConvertedValue::Converted { value, .. } = *self;
        value.borrow()
    }
}

impl<'a> ConversionValidationInput<'a> for ConvertedValue<&'a Value> {
    fn metadata(&self) -> (&super::DeclarationId, &super::ProviderId) {
        let ConvertedValue::Converted {
            datatype, provider, ..
        } = self;
        (datatype, provider)
    }

    fn payload(&self) -> &'a Value {
        let ConvertedValue::Converted { value, .. } = self;
        value
    }
}

/// Existing borrowed-header API, with its concrete caller-owned inline header.
pub type ConversionValidationPreparation<'a, 'providers, Payload = Value> =
    ConversionValidationJob<'a, 'providers, &'a ConvertedValue<Payload>>;

/// Own official parsed metadata while borrowing the exact external payload.
/// Callers must admit the entire concrete job/header inside their actual owner
/// before moving metadata here. This supplies no allocation or storage credit.
pub type OwnedConversionValidationPreparation<'a, 'providers> =
    ConversionValidationJob<'a, 'providers, ConvertedValue<&'a Value>>;

/// One shared validation engine for either canonical metadata representation.
/// Opaque provider methods remain indivisible. The separately admitted cursor
/// borrows the registry and external payload, never this movable input header.
pub struct ConversionValidationJob<'a, 'providers, Input> {
    registry: &'a ConversionRegistry<'providers>,
    input: Input,
    provider: ProviderValidation<'a>,
}

impl<'a, 'providers, Input: ConversionValidationInput<'a>>
    ConversionValidationJob<'a, 'providers, Input>
{
    pub(super) fn new(registry: &'a ConversionRegistry<'providers>, input: Input) -> Self {
        Self {
            registry,
            input,
            provider: ProviderValidation::Start,
        }
    }

    /// Inspect capability without invoking a provider or adapter factory.
    pub fn is_cooperative(&self) -> Result<bool, String> {
        let (datatype, provider) = self.input.metadata();
        let implementation = self.registry.validation_implementation(
            datatype,
            provider,
            self.input.payload(),
            &InvocationContext::default(),
        )?;
        Ok(implementation.retained_validation().is_some())
    }

    /// The entire concrete inline job, including any owned metadata header.
    /// This excludes provider cursor allocation and metadata/payload buffers.
    pub const fn retained_header_bytes(&self) -> usize {
        std::mem::size_of::<Self>()
    }

    pub fn resume(
        &mut self,
        quantum: NonZeroUsize,
        observation: &InvocationContext<'_>,
    ) -> ConversionValidationProgress<'a> {
        let result = self.resume_unreported(quantum, observation);
        if let ConversionValidationProgress::Complete(Err(error)) = &result {
            observation.failed_result(error);
        }
        result
    }
    pub(super) fn resume_unreported(
        &mut self,
        quantum: NonZeroUsize,
        observation: &InvocationContext<'_>,
    ) -> ConversionValidationProgress<'a> {
        if matches!(self.provider, ProviderValidation::Finished) {
            return ConversionValidationProgress::Finished;
        }
        for _ in 0..quantum.get() {
            let result = self.advance(observation);
            match result {
                Ok(ConversionValidationStep::Pending) => {}
                result => {
                    self.provider = ProviderValidation::Finished;
                    return ConversionValidationProgress::Complete(
                        result.map(|_| self.input.payload()),
                    );
                }
            }
        }
        ConversionValidationProgress::Pending
    }

    fn advance(
        &mut self,
        observation: &InvocationContext<'_>,
    ) -> Result<ConversionValidationStep, String> {
        // Every transition checks the exact registry contract, independently
        // of quantum grouping. Metadata is borrowed only for this transition.
        let (datatype, provider) = self.input.metadata();
        let implementation = self.registry.validation_implementation(
            datatype,
            provider,
            self.input.payload(),
            observation,
        )?;
        if matches!(self.provider, ProviderValidation::Start) {
            observation.invoke(&implementation.contract().provider)?;
        }
        match self
            .provider
            .advance(implementation, self.input.payload(), observation)?
        {
            ConversionValidationStep::Pending => Ok(ConversionValidationStep::Pending),
            ConversionValidationStep::Complete => {
                self.registry.provider_observed(datatype, observation)?;
                observation.checkpoint()?;
                Ok(ConversionValidationStep::Complete)
            }
        }
    }
}

impl<'a, 'providers> ConversionValidationJob<'a, 'providers, &'a ConvertedValue> {
    /// Existing owned-payload borrowed-header measurement.
    pub const fn inline_bytes() -> usize {
        std::mem::size_of::<Self>()
    }
}

/// Existing registry parse/bind/format validation sites use this same provider
/// engine after their existing invocation receipt. Opaque hooks remain atomic.
pub(super) fn validate_provider_observed(
    provider: &dyn ConversionProvider,
    value: &Value,
    observation: &InvocationContext<'_>,
) -> Result<(), String> {
    let mut preparation = ProviderValidation::Start;
    loop {
        if preparation.advance(provider, value, observation)? == ConversionValidationStep::Complete
        {
            return Ok(());
        }
    }
}
