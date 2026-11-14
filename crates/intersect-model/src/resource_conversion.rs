//! Explicit immutable resource adapters over the shared E02 resolver.
use intersect_model::definitions::{
    bundle::{DefinitionBundle, SourceId},
    conversion::{ConversionContract, ConversionProvider, InvocationContext},
    ecore::Document,
    resources::{DeclarationId, ResourceSet},
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeSet;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ResourceKind {
    Resource,
    Set,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResourceHandle {
    pub resource: SourceId,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResourceSetHandle {
    pub resources: Vec<SourceId>,
}
pub struct ResourceConverter {
    contract: ConversionContract,
    kind: ResourceKind,
    selected: BTreeSet<SourceId>,
    resources: std::sync::Arc<ResourceSet>,
}
impl ResourceConverter {
    pub fn new(
        contract: ConversionContract,
        kind: ResourceKind,
        bundle: &DefinitionBundle,
    ) -> Result<Self, String> {
        Self::new_with_resources(contract, kind, || {
            ResourceSet::new(bundle).map(std::sync::Arc::new)
        })
    }
    pub(crate) fn new_with_resources(
        contract: ConversionContract,
        kind: ResourceKind,
        resources: impl FnOnce() -> Result<std::sync::Arc<ResourceSet>, String>,
    ) -> Result<Self, String> {
        if contract.serializable {
            return Err("Resource adapter requires a non-serializable datatype contract".into());
        }
        if contract
            .declarations
            .windows(2)
            .any(|pair| pair[0] >= pair[1])
        {
            return Err("Resource scope must be sorted and unique".into());
        }
        let resources = resources()?;
        let mut selected = BTreeSet::new();
        for declaration in &contract.declarations {
            if !declaration.path.is_empty() {
                return Err("Resource scope must select document roots".into());
            }
            resources.document(&declaration.source)?;
            selected.insert(declaration.source.clone());
        }
        Ok(Self {
            contract,
            kind,
            selected,
            resources,
        })
    }
    pub fn members(&self, value: &Value) -> Result<Vec<SourceId>, String> {
        self.members_observed(value, &InvocationContext::default())
    }
    fn members_observed(
        &self,
        value: &Value,
        observation: &InvocationContext<'_>,
    ) -> Result<Vec<SourceId>, String> {
        observation.work(1)?;
        let members = match self.kind {
            ResourceKind::Resource => vec![
                serde_json::from_value::<ResourceHandle>(value.clone())
                    .map_err(|e| format!("Invalid resource handle: {e}"))?
                    .resource,
            ],
            ResourceKind::Set => {
                serde_json::from_value::<ResourceSetHandle>(value.clone())
                    .map_err(|e| format!("Invalid resource-set handle: {e}"))?
                    .resources
            }
        };
        let mut seen = BTreeSet::new();
        for source in &members {
            observation.work(1)?;
            if !self.selected.contains(source) {
                return Err("Resource handle is outside the exact selected scope".into());
            }
            if !seen.insert(source) {
                return Err("Duplicate resource identity in resource-set handle".into());
            }
        }
        Ok(members)
    }
    pub fn document(&self, value: &Value, source: &SourceId) -> Result<&Document, String> {
        if !self.members(value)?.contains(source) {
            return Err("Resource is not a member of this handle".into());
        }
        self.resources.document(source)
    }
    pub fn resolve(
        &self,
        value: &Value,
        from: &SourceId,
        href: &str,
    ) -> Result<DeclarationId, String> {
        let members = self.members(value)?;
        if !members.contains(from) {
            return Err("Referring resource is not a member of this handle".into());
        }
        let target = self.resources.resolve(from, href)?;
        if !members.contains(&target.source) {
            return Err("Resolved resource is not a member of this handle".into());
        }
        Ok(target)
    }
}
impl ConversionProvider for ResourceConverter {
    fn contract(&self) -> &ConversionContract {
        &self.contract
    }
    fn parse(&self, _text: &str) -> Result<Value, String> {
        Err("Resource handles are not lexically serializable".into())
    }
    fn format(&self, _value: &Value) -> Result<String, String> {
        Err("Resource handles are not lexically serializable".into())
    }
    fn validate(&self, value: &Value) -> Result<(), String> {
        self.validate_observed(value, &InvocationContext::default())
    }
    fn validate_observed(
        &self,
        value: &Value,
        observation: &InvocationContext<'_>,
    ) -> Result<(), String> {
        self.members_observed(value, observation).map(|_| ())
    }
    fn default_value(&self) -> Result<Option<Value>, String> {
        Ok(None)
    }
}
