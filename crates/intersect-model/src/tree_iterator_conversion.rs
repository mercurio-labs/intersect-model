//! Replayable immutable definition-tree cursors, never serialized host iterators.
use intersect_model::definitions::{
    bundle::DefinitionBundle,
    conversion::{ConversionContract, ConversionProvider, InvocationContext},
    resources::{DeclarationId, ResourceSet},
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TreeSnapshot {
    pub roots: Vec<DeclarationId>,
    pub position: u64,
    pub pruned: Vec<u64>,
}
struct Traversal {
    pending: Vec<DeclarationId>,
    current: Option<DeclarationId>,
    descend: bool,
}
impl Traversal {
    fn advance(
        &mut self,
        resources: &ResourceSet,
        observation: &InvocationContext<'_>,
    ) -> Result<Option<DeclarationId>, String> {
        observation.work(1)?;
        if self.descend {
            if let Some(current) = &self.current {
                let node = resources.node(current)?;
                for ordinal in (0..node.children.len()).rev() {
                    observation.work(1)?;
                    let mut child = current.clone();
                    child.path.push(ordinal);
                    self.pending.push(child);
                }
            }
            self.descend = false;
        }
        let next = self.pending.pop();
        if let Some(id) = &next {
            self.current = Some(id.clone());
            self.descend = true;
        }
        Ok(next)
    }
}
pub struct TreeIteratorConverter {
    contract: ConversionContract,
    resources: std::sync::Arc<ResourceSet>,
}
impl TreeIteratorConverter {
    pub fn new(contract: ConversionContract, bundle: &DefinitionBundle) -> Result<Self, String> {
        Self::new_with_resources(contract, || {
            ResourceSet::new(bundle).map(std::sync::Arc::new)
        })
    }
    pub(crate) fn new_with_resources(
        contract: ConversionContract,
        resources: impl FnOnce() -> Result<std::sync::Arc<ResourceSet>, String>,
    ) -> Result<Self, String> {
        if contract.serializable {
            return Err("Tree iterator requires a non-serializable datatype contract".into());
        }
        if contract
            .declarations
            .windows(2)
            .any(|pair| pair[0] >= pair[1])
        {
            return Err("Tree iterator roots must be sorted and unique".into());
        }
        let resources = resources()?;
        for root in &contract.declarations {
            resources.node(root)?;
        }
        Ok(Self {
            contract,
            resources,
        })
    }
    pub fn start(&self) -> TreeSnapshot {
        TreeSnapshot {
            roots: self.contract.declarations.clone(),
            position: 0,
            pruned: vec![],
        }
    }
    fn restore(&self, snapshot: &TreeSnapshot) -> Result<Traversal, String> {
        self.restore_observed(snapshot, &InvocationContext::default())
    }
    fn restore_observed(
        &self,
        snapshot: &TreeSnapshot,
        observation: &InvocationContext<'_>,
    ) -> Result<Traversal, String> {
        observation.work(1)?;
        if snapshot.roots != self.contract.declarations {
            return Err(
                "Tree iterator roots or source revisions differ from its selected scope".into(),
            );
        }
        for (index, position) in snapshot.pruned.iter().enumerate() {
            observation.work(1)?;
            if *position == 0
                || *position > snapshot.position
                || (index > 0 && snapshot.pruned[index - 1] >= *position)
            {
                return Err(
                    "Tree iterator prune positions must be sorted, unique and already visited"
                        .into(),
                );
            }
        }
        let mut pending = Vec::new();
        for root in snapshot.roots.iter().rev() {
            observation.work(1)?;
            pending.push(root.clone());
        }
        let mut state = Traversal {
            pending,
            current: None,
            descend: false,
        };
        for position in 1..=snapshot.position {
            state
                .advance(&self.resources, observation)?
                .ok_or("Tree iterator position exceeds exhaustion")?;
            if snapshot.pruned.binary_search(&position).is_ok() {
                state.descend = false;
            }
        }
        Ok(state)
    }
    pub fn snapshot(&self, value: &Value) -> Result<TreeSnapshot, String> {
        self.snapshot_observed(value, &InvocationContext::default())
    }
    fn snapshot_observed(
        &self,
        value: &Value,
        observation: &InvocationContext<'_>,
    ) -> Result<TreeSnapshot, String> {
        observation.work(1)?;
        let snapshot: TreeSnapshot = serde_json::from_value(value.clone())
            .map_err(|e| format!("Invalid tree iterator snapshot: {e}"))?;
        if serde_json::to_value(&snapshot).map_err(|e| e.to_string())? != *value {
            return Err("Tree iterator snapshot has noncanonical fields".into());
        }
        self.restore_observed(&snapshot, observation)?;
        Ok(snapshot)
    }
    pub fn has_next(&self, snapshot: &TreeSnapshot) -> Result<bool, String> {
        Ok(self
            .restore(snapshot)?
            .advance(&self.resources, &InvocationContext::default())?
            .is_some())
    }
    pub fn next(&self, snapshot: &mut TreeSnapshot) -> Result<Option<DeclarationId>, String> {
        let next = self
            .restore(snapshot)?
            .advance(&self.resources, &InvocationContext::default())?;
        if next.is_some() {
            snapshot.position = snapshot
                .position
                .checked_add(1)
                .ok_or("Tree iterator position overflow")?;
        }
        Ok(next)
    }
    pub fn remove(&self, _snapshot: &mut TreeSnapshot) -> Result<(), String> {
        Err("Removal is unsupported for immutable definition trees".into())
    }
    pub fn prune(&self, snapshot: &mut TreeSnapshot) -> Result<(), String> {
        self.restore(snapshot)?;
        if snapshot.position == 0 {
            return Err("Cannot prune before the first tree iterator result".into());
        }
        if snapshot.pruned.last() != Some(&snapshot.position) {
            snapshot.pruned.push(snapshot.position);
        }
        Ok(())
    }
}
impl ConversionProvider for TreeIteratorConverter {
    fn contract(&self) -> &ConversionContract {
        &self.contract
    }
    fn parse(&self, _text: &str) -> Result<Value, String> {
        Err("ETreeIterator is not lexically serializable".into())
    }
    fn format(&self, _value: &Value) -> Result<String, String> {
        Err("ETreeIterator is not lexically serializable".into())
    }
    fn validate(&self, value: &Value) -> Result<(), String> {
        self.validate_observed(value, &InvocationContext::default())
    }
    fn validate_observed(
        &self,
        value: &Value,
        observation: &InvocationContext<'_>,
    ) -> Result<(), String> {
        self.snapshot_observed(value, observation).map(|_| ())
    }
    fn default_value(&self) -> Result<Option<Value>, String> {
        Ok(None)
    }
}
