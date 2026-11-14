//! Source-checked constructor policies, independent of concrete value providers.
use super::{ClassifierRelation, ProviderId};
use crate::definitions::{
    bundle::DefinitionBundle, compiler::import_datatype_bundle, datatypes::Datatype,
    resources::DeclarationId,
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Retained selection data. Neither this record nor matching names supplies policy.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConstructorContract {
    pub datatype: Datatype,
    pub provider: ProviderId,
}

/// An installed language policy for uninstantiated generic datatype constructors.
/// A relation only compares constructors. Consumers must separately check ordered
/// arguments with the shared generic resolver; it never authorizes value retagging.
/// Providers must inspect the complete source-checked target contract, including
/// version, formal parameters and instance metadata. No Java reflection is used.
pub trait ConstructorProvider {
    fn contract(&self) -> &ConstructorContract;
    fn constructor_relation(
        &self,
        target: &ConstructorContract,
    ) -> Result<Option<ClassifierRelation>, String>;
}

#[derive(Clone)]
struct Registration<'a> {
    contract: ConstructorContract,
    provider: super::ProviderRef<'a, dyn ConstructorProvider + 'a>,
}

/// Kept separate from ordinary and applied payload codec registration. In
/// particular, bounds can require this policy before an application can exist.
#[derive(Clone, Default)]
pub struct ConstructorRegistry<'a> {
    observation: super::InvocationContext<'a>,
    providers: BTreeMap<DeclarationId, Registration<'a>>,
}
impl<'a> ConstructorRegistry<'a> {
    pub(super) fn measure_observation_copy(
        &self,
        budget: &mut super::ObservationCopyBudget,
    ) -> Result<(), String> {
        for (key, registration) in &self.providers {
            budget.add(key)?;
            budget.add(&registration.contract)?;
        }
        Ok(())
    }

    pub(super) fn set_observation(&mut self, context: super::InvocationContext<'a>) {
        self.observation = context;
    }
    pub fn register(
        &mut self,
        bundle: &DefinitionBundle,
        provider: &'a dyn ConstructorProvider,
    ) -> Result<(), String> {
        self.register_provider(bundle, super::ProviderRef::Borrowed(provider))
    }
    pub fn register_shared(
        &mut self,
        bundle: &DefinitionBundle,
        provider: std::rc::Rc<dyn ConstructorProvider + 'a>,
    ) -> Result<(), String> {
        self.register_provider(bundle, super::ProviderRef::Shared(provider))
    }
    fn register_provider(
        &mut self,
        bundle: &DefinitionBundle,
        provider: super::ProviderRef<'a, dyn ConstructorProvider + 'a>,
    ) -> Result<(), String> {
        let contract = provider.contract().clone();
        if contract.provider.name.trim().is_empty() || contract.provider.version.trim().is_empty() {
            return Err("Constructor provider requires a nonempty name and version".into());
        }
        let datatype = import_datatype_bundle(bundle, contract.datatype.declaration())
            .map_err(|e| e.to_string())?;
        if datatype != contract.datatype {
            return Err("Constructor contract differs from imported source datatype".into());
        }
        if datatype.parameters().is_empty() || datatype.application().is_some() {
            return Err("Constructor provider requires an uninstantiated generic datatype".into());
        }
        if self.providers.contains_key(datatype.declaration()) {
            return Err("Constructor provider already registered for this exact datatype".into());
        }
        if self.providers.len() >= 128 {
            return Err("Constructor provider budget exceeded (128)".into());
        }
        if provider.contract() != &contract {
            return Err("Constructor provider changed its contract during registration".into());
        }
        self.providers.insert(
            datatype.declaration().clone(),
            Registration { contract, provider },
        );
        Ok(())
    }

    fn provider(&self, id: &DeclarationId) -> Result<&dyn ConstructorProvider, String> {
        let registration = self.providers.get(id).ok_or_else(|| {
            self.observation.dependency_unavailable(
                crate::definitions::conversion::ProviderDependencyKind::Constructor,
            );
            "No constructor provider for this exact datatype revision"
        })?;
        if registration.provider.contract() != &registration.contract {
            return Err("Constructor provider changed its registered contract".into());
        }
        Ok(&*registration.provider)
    }

    /// Check one exact registered policy without cloning the registry manifest.
    /// This validates its provider snapshot but does not invoke semantic work.
    pub fn contains_contract(&self, contract: &ConstructorContract) -> Result<bool, String> {
        if !self.providers.contains_key(contract.datatype.declaration()) {
            return Ok(false);
        }
        Ok(self.provider(contract.datatype.declaration())?.contract() == contract)
    }

    pub fn registered_contracts(&self) -> Result<Vec<ConstructorContract>, String> {
        self.registered_contract_refs()
            .map(|contract| contract.cloned())
            .collect()
    }

    pub(crate) fn registered_contract_refs(
        &self,
    ) -> impl Iterator<Item = Result<&ConstructorContract, String>> + '_ {
        self.providers
            .keys()
            .map(|id| self.provider(id).map(|p| p.contract()))
    }

    /// Require reciprocal assertions and stable contracts after either callback.
    /// Unknown remains distinct from an explicit unrelated result. The result is
    /// not an application compatibility judgment or a payload validation result.
    pub fn constructor_relation(
        &self,
        source: &DeclarationId,
        target: &DeclarationId,
    ) -> Result<Option<ClassifierRelation>, String> {
        let a = self.provider(source)?;
        let b = self.provider(target)?;
        if source == target {
            return Ok(Some(ClassifierRelation::Equivalent));
        }
        self.observation.invoke(&a.contract().provider)?;
        let forward = a.constructor_relation(b.contract());
        self.provider(source)?;
        self.provider(target)?;
        let forward = forward?;
        self.observation.invoke(&b.contract().provider)?;
        let reverse = b.constructor_relation(a.contract());
        self.provider(source)?;
        self.provider(target)?;
        match (forward, reverse?) {
            (Some(a), Some(b)) if a.reverse() == b => Ok(Some(a)),
            (Some(_), Some(_)) => {
                Err("Constructor providers disagree on classifier relation".into())
            }
            _ => Ok(None),
        }
    }
}
