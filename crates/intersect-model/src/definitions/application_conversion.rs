//! Application identity around the existing conversion engine; no second payload codec.
use super::*;
use crate::definitions::{
    generic_resolution::CheckedDatatypeApplication,
    generic_types::{ClosedType, Type},
};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AppliedConversionContract {
    pub application: ClosedType,
    pub conversion: ConversionContract,
}

/// An installed implementation specialized for an exact application.
/// Algorithms and argument-to-member policy belong to the language package.
pub trait AppliedConversionProvider {
    fn contract(&self) -> &AppliedConversionContract;
    fn implementation(&self) -> &dyn ConversionProvider;
}

/// A distinct versioned tag; ordinary ConvertedValue serialization is unchanged.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", deny_unknown_fields)]
pub enum AppliedConvertedValue {
    #[serde(rename = "converted_application_v1")]
    Applied {
        application: ClosedType,
        converted: ConvertedValue,
    },
}
impl AppliedConvertedValue {
    pub fn application(&self) -> &ClosedType {
        let Self::Applied { application, .. } = self;
        application
    }
}

#[derive(Clone)]
struct AppliedRegistration<'a> {
    contract: AppliedConversionContract,
    provider: ProviderRef<'a, dyn AppliedConversionProvider + 'a>,
    registry: ConversionRegistry<'a>,
    datatype: Datatype,
}
impl AppliedRegistration<'_> {
    fn check(&self) -> Result<(), String> {
        if self.provider.contract() != &self.contract {
            return Err("Applied provider changed its registered contract".into());
        }
        if self.registry.require(&self.datatype)? != &self.contract.conversion {
            return Err("Applied provider implementation differs from its contract".into());
        }
        Ok(())
    }
}

#[derive(Clone, Default)]
pub struct AppliedConversionRegistry<'a> {
    observation: InvocationContext<'a>,
    providers: BTreeMap<String, AppliedRegistration<'a>>,
}
impl<'a> AppliedConversionRegistry<'a> {
    pub(super) fn measure_observation_copy(
        &self,
        budget: &mut ObservationCopyBudget,
    ) -> Result<(), String> {
        for (key, registration) in &self.providers {
            budget.add(key)?;
            budget.add(&registration.contract)?;
            budget.add(&registration.datatype)?;
            registration.registry.measure_observation_copy(budget)?;
        }
        Ok(())
    }

    pub fn with_observation(mut self, context: InvocationContext<'a>) -> Self {
        self.set_observation(context);
        self
    }
    pub(super) fn set_observation(&mut self, context: InvocationContext<'a>) {
        self.observation = context.clone();
        for registration in self.providers.values_mut() {
            registration.registry.observation = context.clone();
        }
    }
    pub fn register(
        &mut self,
        datatype: &Datatype,
        checked: &CheckedDatatypeApplication,
        provider: &'a dyn AppliedConversionProvider,
    ) -> Result<(), String> {
        self.register_provider(datatype, checked, ProviderRef::Borrowed(provider))
    }
    pub fn register_shared(
        &mut self,
        datatype: &Datatype,
        checked: &CheckedDatatypeApplication,
        provider: std::rc::Rc<dyn AppliedConversionProvider + 'a>,
    ) -> Result<(), String> {
        self.register_provider(datatype, checked, ProviderRef::Shared(provider))
    }
    fn register_provider(
        &mut self,
        datatype: &Datatype,
        checked: &CheckedDatatypeApplication,
        provider: ProviderRef<'a, dyn AppliedConversionProvider + 'a>,
    ) -> Result<(), String> {
        let contract = provider.contract();
        let application = checked.application();
        let Type::Classifier {
            declaration,
            arguments,
            ..
        } = application.expression()
        else {
            return Err("Applied provider requires a classifier application".into());
        };
        if arguments.is_empty() || datatype.parameters().is_empty() {
            return Err("Applied provider requires explicit generic datatype arguments".into());
        }
        if declaration != datatype.declaration()
            || contract.application != *application
            || contract.conversion.datatype != *declaration
            || datatype.parameters()
                != checked
                    .parameters()
                    .iter()
                    .map(|p| p.syntax.clone())
                    .collect::<Vec<_>>()
            || provider.implementation().contract() != &contract.conversion
        {
            return Err("Applied provider does not match the checked datatype application".into());
        }
        if self.providers.contains_key(application.identity()) {
            return Err("Provider already registered for this exact datatype application".into());
        }
        let mut registry = ConversionRegistry::default().with_observation(self.observation.clone());
        registry.register_specialized(
            datatype,
            ConversionImplementation::Applied(provider.clone()),
        )?;
        let registration = AppliedRegistration {
            contract: contract.clone(),
            provider,
            registry,
            datatype: datatype.clone(),
        };
        registration.check()?;
        self.providers
            .insert(application.identity().to_owned(), registration);
        Ok(())
    }
    fn provider(&self, application: &ClosedType) -> Result<&AppliedRegistration<'a>, String> {
        self.provider_observed(application, &self.observation)
    }
    fn provider_observed(
        &self,
        application: &ClosedType,
        observation: &InvocationContext<'_>,
    ) -> Result<&AppliedRegistration<'a>, String> {
        let registration = self.providers.get(application.identity()).ok_or_else(|| {
            observation.dependency_unavailable(
                crate::definitions::conversion::ProviderDependencyKind::AppliedConversion,
            );
            "No provider for this exact datatype application"
        })?;
        registration.check()?;
        if registration.contract.application != *application {
            return Err("Datatype application identity mismatch".into());
        }
        Ok(registration)
    }
    /// Check one exact registered policy without cloning the registry manifest.
    /// This validates its provider snapshot but does not invoke semantic work.
    pub fn contains_contract(&self, contract: &AppliedConversionContract) -> Result<bool, String> {
        if !self.providers.contains_key(contract.application.identity()) {
            return Ok(false);
        }
        Ok(&self.provider(&contract.application)?.contract == contract)
    }

    /// Exact, mutation-checked snapshots of selected application providers.
    pub fn registered_contracts(&self) -> Result<Vec<AppliedConversionContract>, String> {
        self.registered_contract_refs()
            .map(|value| value.cloned())
            .collect()
    }
    pub(crate) fn registered_contract_refs(
        &self,
    ) -> impl Iterator<Item = Result<&AppliedConversionContract, String>> + '_ {
        self.providers.values().map(|registration| {
            self.provider(&registration.contract.application)
                .map(|checked| &checked.contract)
        })
    }
    pub fn require(&self, application: &ClosedType) -> Result<&AppliedConversionContract, String> {
        Ok(&self.provider(application)?.contract)
    }
    pub fn bind(
        &self,
        application: &ClosedType,
        value: Value,
    ) -> Result<AppliedConvertedValue, String> {
        let registration = self.provider(application)?;
        let converted = registration
            .registry
            .bind(&registration.contract.conversion.datatype, value)?;
        registration.check()?;
        Ok(AppliedConvertedValue::Applied {
            application: application.clone(),
            converted,
        })
    }
    pub fn validate_for<'v>(
        &self,
        application: &ClosedType,
        value: &'v AppliedConvertedValue,
    ) -> Result<&'v Value, String> {
        self.validate_for_observed(application, value, &self.observation)
    }
    pub fn validate_for_observed<'v>(
        &self,
        application: &ClosedType,
        value: &'v AppliedConvertedValue,
        observation: &InvocationContext<'_>,
    ) -> Result<&'v Value, String> {
        if value.application() != application {
            return Err(
                "Converted value does not belong to the destination datatype application".into(),
            );
        }
        self.validate_observed(value, observation)
    }
    pub fn validate<'v>(&self, value: &'v AppliedConvertedValue) -> Result<&'v Value, String> {
        self.validate_observed(value, &self.observation)
    }
    pub fn validate_observed<'v>(
        &self,
        value: &'v AppliedConvertedValue,
        observation: &InvocationContext<'_>,
    ) -> Result<&'v Value, String> {
        let AppliedConvertedValue::Applied {
            application,
            converted,
        } = value;
        let registration = self.provider_observed(application, observation)?;
        let payload = registration.registry.validate_for_observed(
            &registration.datatype,
            converted,
            observation,
        )?;
        registration.check()?;
        Ok(payload)
    }
    pub fn parse(
        &self,
        application: &ClosedType,
        text: &str,
    ) -> Result<AppliedConvertedValue, String> {
        let registration = self.provider(application)?;
        let converted = registration
            .registry
            .parse(&registration.contract.conversion.datatype, text)?;
        registration.check()?;
        Ok(AppliedConvertedValue::Applied {
            application: application.clone(),
            converted,
        })
    }
    pub fn format(&self, value: &AppliedConvertedValue) -> Result<String, String> {
        self.validate(value)?;
        let AppliedConvertedValue::Applied {
            application,
            converted,
        } = value;
        let registration = self.provider(application)?;
        let result = registration.registry.format(converted)?;
        registration.check()?;
        Ok(result)
    }
    pub fn default_value(
        &self,
        application: &ClosedType,
    ) -> Result<Option<AppliedConvertedValue>, String> {
        let registration = self.provider(application)?;
        let converted = registration
            .registry
            .default_value(&registration.contract.conversion.datatype)?;
        registration.check()?;
        Ok(converted.map(|converted| AppliedConvertedValue::Applied {
            application: application.clone(),
            converted,
        }))
    }
}
