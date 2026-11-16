//! Handwritten, serializable renderer-neutral visualization contracts.
//! This crate knows no language classes, parser, filesystem or UI framework.
use serde::{Deserialize, Serialize};

pub const VERSION: u32 = 1;
pub const MAX_WIRE_INTEGER: u64 = 9_007_199_254_740_991;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Position {
    pub line: usize,
    pub character: usize,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Range {
    pub start: Position,
    pub end: Position,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Binding {
    /// Stable within the supported declaration path contract; ambiguous paths fail.
    pub key: String,
    /// Snapshot-local engine identity. Never use this as a persisted symbol key.
    pub element_id: String,
    pub context: Vec<String>,
    pub uri: String,
    pub range: Range,
    pub read_only: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct View {
    pub key: String,
    pub name: String,
    pub kind: String,
    pub binding: Binding,
    pub owners: Vec<Owner>,
    pub details: Vec<Field>,
    pub problems: Vec<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Owner {
    pub key: String,
    pub label: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Compartment {
    pub label: String,
    pub rows: Vec<Field>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Field {
    pub label: String,
    pub binding: Binding,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Node {
    pub id: String,
    pub notation: String,
    pub title: String,
    pub subtitle: String,
    pub binding: Binding,
    pub compartments: Vec<Compartment>,
    pub children: Vec<Node>,
    pub ports: Vec<Port>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Port {
    pub id: String,
    pub label: String,
    pub side: String,
    pub binding: Binding,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Edge {
    pub id: String,
    pub notation: String,
    pub label: String,
    pub source: String,
    pub target: String,
    pub binding: Binding,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Scene {
    pub schema_version: u32,
    pub revision: u64,
    #[serde(default)]
    pub layout: LayoutHints,
    pub view: View,
    pub nodes: Vec<Node>,
    pub edges: Vec<Edge>,
    pub palette: Palette,
    pub problems: Vec<String>,
}

/// Rendering hints, not language semantics. The provider chooses the direction.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LayoutHints {
    pub direction: String,
}
impl Default for LayoutHints {
    fn default() -> Self {
        Self {
            direction: "RIGHT".into(),
        }
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ViewCatalog {
    pub schema_version: u32,
    pub revision: u64,
    pub views: Vec<View>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ToolCapability {
    pub tool_id: String,
    pub enabled: bool,
    pub reason: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SceneResponse {
    pub scene: Scene,
    pub capabilities: Vec<ToolCapability>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PrepareCommand {
    pub revision: u64,
    pub view_key: String,
    pub owner_key: String,
    pub tool_id: String,
    pub arguments: std::collections::BTreeMap<String, String>,
    #[serde(default)]
    pub palette: Option<Palette>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PreparedEdit {
    pub schema_version: u32,
    pub token: String,
    pub base_revision: u64,
    pub next_revision: u64,
    pub patches: Vec<SourcePatch>,
    pub created: String,
}

/// Finite, caller-adjustable presentation budgets. Domain providers can impose
/// stricter profiles without adding domain dependencies to this contract crate.
#[derive(Clone, Copy, Debug)]
pub struct SceneLimits {
    pub nodes: usize,
    pub ports: usize,
    pub edges: usize,
    pub rows: usize,
    pub depth: usize,
}
impl Default for SceneLimits {
    fn default() -> Self {
        Self {
            nodes: 500,
            ports: 1000,
            edges: 2000,
            rows: 5000,
            depth: 16,
        }
    }
}
fn identity(id: &str) -> Result<(), String> {
    if id.is_empty() {
        Err("A wire identity is empty".into())
    } else {
        Ok(())
    }
}
fn revision(value: u64) -> Result<(), String> {
    if value > MAX_WIRE_INTEGER {
        Err("Revision exceeds the portable JSON integer range".into())
    } else {
        Ok(())
    }
}
impl Range {
    pub fn validate(&self) -> Result<(), String> {
        if [
            self.start.line,
            self.start.character,
            self.end.line,
            self.end.character,
        ]
        .iter()
        .any(|n| *n as u128 > MAX_WIRE_INTEGER as u128)
            || (self.end.line, self.end.character) < (self.start.line, self.start.character)
        {
            return Err("Invalid UTF-16 source range".into());
        }
        Ok(())
    }
}
impl Binding {
    pub fn validate(&self) -> Result<(), String> {
        identity(&self.key)?;
        identity(&self.element_id)?;
        identity(&self.uri)?;
        self.range.validate()?;
        for context in &self.context {
            identity(context)?;
        }
        Ok(())
    }
}
impl View {
    pub fn validate(&self) -> Result<(), String> {
        identity(&self.key)?;
        identity(&self.name)?;
        identity(&self.kind)?;
        self.binding.validate()?;
        let mut owners = std::collections::BTreeSet::new();
        for owner in &self.owners {
            identity(&owner.key)?;
            identity(&owner.label)?;
            if !owners.insert(&owner.key) {
                return Err("Duplicate view owner identity".into());
            }
        }
        for detail in &self.details {
            detail.binding.validate()?;
        }
        Ok(())
    }
}
impl ViewCatalog {
    pub fn validate(&self) -> Result<(), String> {
        if self.schema_version != VERSION {
            return Err("Unsupported view catalog version".into());
        }
        revision(self.revision)?;
        let mut views = std::collections::BTreeSet::new();
        for view in &self.views {
            view.validate()?;
            if !views.insert(&view.key) {
                return Err("Duplicate view identity".into());
            }
        }
        Ok(())
    }
}
impl Scene {
    pub fn validate(&self) -> Result<(), String> {
        self.validate_with_limits(SceneLimits::default())
    }
    pub fn validate_with_limits(&self, limits: SceneLimits) -> Result<(), String> {
        if self.schema_version != VERSION {
            return Err("Unsupported scene version".into());
        }
        revision(self.revision)?;
        self.view.validate()?;
        self.palette.validate()?;
        if !["RIGHT", "LEFT", "UP", "DOWN"].contains(&self.layout.direction.as_str()) {
            return Err("Unsupported layout direction".into());
        }
        struct Audit {
            limits: SceneLimits,
            counts: [usize; 3],
            ids: std::collections::BTreeSet<String>,
            endpoints: std::collections::BTreeSet<String>,
        }
        impl Audit {
            fn add(&mut self, id: &str, endpoint: bool) -> Result<(), String> {
                identity(id)?;
                if !self.ids.insert(id.into()) {
                    return Err(format!("Duplicate scene identity: {id}"));
                }
                if endpoint {
                    self.endpoints.insert(id.into());
                }
                Ok(())
            }
            fn node(&mut self, node: &Node, depth: usize) -> Result<(), String> {
                self.counts[0] += 1;
                if depth > self.limits.depth || self.counts[0] > self.limits.nodes {
                    return Err("Scene node or nesting budget exceeded".into());
                }
                self.add(&node.id, true)?;
                node.binding.validate()?;
                for port in &node.ports {
                    self.counts[1] += 1;
                    if self.counts[1] > self.limits.ports {
                        return Err("Scene port budget exceeded".into());
                    }
                    self.add(&port.id, true)?;
                    port.binding.validate()?;
                    if !["NORTH", "SOUTH", "WEST", "EAST"].contains(&port.side.as_str()) {
                        return Err("Unsupported port side".into());
                    }
                }
                for compartment in &node.compartments {
                    for row in &compartment.rows {
                        self.counts[2] += 1;
                        if self.counts[2] > self.limits.rows {
                            return Err("Scene row budget exceeded".into());
                        }
                        row.binding.validate()?;
                    }
                }
                for child in &node.children {
                    self.node(child, depth + 1)?;
                }
                Ok(())
            }
        }
        let mut audit = Audit {
            limits,
            counts: [0; 3],
            ids: Default::default(),
            endpoints: Default::default(),
        };
        for node in &self.nodes {
            audit.node(node, 0)?;
        }
        if self.edges.len() > limits.edges {
            return Err("Scene edge budget exceeded".into());
        }
        for edge in &self.edges {
            audit.add(&edge.id, false)?;
            edge.binding.validate()?;
            if !audit.endpoints.contains(&edge.source) || !audit.endpoints.contains(&edge.target) {
                return Err(format!("Unbound scene edge: {}", edge.id));
            }
        }
        Ok(())
    }
}
impl SceneResponse {
    pub fn validate(&self) -> Result<(), String> {
        self.scene.validate()?;
        let tools: std::collections::BTreeSet<_> = self
            .scene
            .palette
            .groups
            .iter()
            .flat_map(|g| g.entries.iter().map(|t| t.id.as_str()))
            .collect();
        let mut capabilities = std::collections::BTreeSet::new();
        for capability in &self.capabilities {
            if !tools.contains(capability.tool_id.as_str())
                || !capabilities.insert(capability.tool_id.as_str())
                || (!capability.enabled && capability.reason.is_empty())
            {
                return Err("Invalid tool capability coverage or missing disabled reason".into());
            }
        }
        if tools != capabilities {
            return Err("Missing tool capabilities".into());
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Palette {
    pub schema_version: u32,
    pub id: String,
    pub label: String,
    pub groups: Vec<Group>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Group {
    pub id: String,
    pub label: String,
    pub entries: Vec<Tool>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Tool {
    pub id: String,
    pub label: String,
    pub icon: String,
    pub gesture: String,
    pub command: String,
    #[serde(default)]
    pub parameters: serde_json::Map<String, serde_json::Value>,
    #[serde(default)]
    pub inputs: Vec<Input>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Input {
    pub id: String,
    pub label: String,
    #[serde(default)]
    pub default: String,
}
impl Palette {
    /// Structural checks only. Command support/applicability is language-owned.
    pub fn validate(&self) -> Result<(), String> {
        if self.schema_version != VERSION || self.id.is_empty() || self.label.is_empty() {
            return Err("Unsupported palette version or missing identity/label".into());
        }
        if self.groups.len() > 32 {
            return Err("Palette exceeds 32 groups".into());
        }
        let mut ids = std::collections::BTreeSet::new();
        let mut entries = 0;
        for group in &self.groups {
            if group.id.is_empty() || group.label.is_empty() || !ids.insert(&group.id) {
                return Err("Palette group identifiers must be nonempty and unique".into());
            }
            for tool in &group.entries {
                entries += 1;
                if entries > 256
                    || tool.id.is_empty()
                    || tool.command.is_empty()
                    || tool.label.is_empty()
                    || !ids.insert(&tool.id)
                {
                    return Err(
                        "Palette exceeds its entry limit or has missing/duplicate tool identities"
                            .into(),
                    );
                }
                if !["select", "place", "action"].contains(&tool.gesture.as_str()) {
                    return Err(format!("Unsupported palette gesture: {}", tool.gesture));
                }
                let mut inputs = std::collections::BTreeSet::new();
                if tool.inputs.len() > 16
                    || tool
                        .inputs
                        .iter()
                        .any(|i| i.id.is_empty() || i.label.is_empty() || !inputs.insert(&i.id))
                {
                    return Err(
                        "Tool inputs must have unique nonempty identities and labels (maximum 16)"
                            .into(),
                    );
                }
            }
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SourcePatch {
    pub uri: String,
    pub range: Range,
    pub original: String,
    pub replacement: String,
    pub before_hash: String,
    pub after_hash: String,
}
impl SourcePatch {
    pub fn validate(&self) -> Result<(), String> {
        identity(&self.uri)?;
        self.range.validate()?;
        for hash in [&self.before_hash, &self.after_hash] {
            if hash.len() != 64 || !hash.bytes().all(|b| b.is_ascii_hexdigit()) {
                return Err("Invalid source buffer SHA-256".into());
            }
        }
        if self.range.start == self.range.end && !self.original.is_empty() {
            return Err("Insertion patch has nonempty original text".into());
        }
        Ok(())
    }
}
impl PreparedEdit {
    pub fn validate(&self) -> Result<(), String> {
        if self.schema_version != VERSION {
            return Err("Unsupported prepared edit version".into());
        }
        identity(&self.token)?;
        revision(self.base_revision)?;
        revision(self.next_revision)?;
        if self.next_revision <= self.base_revision || self.patches.is_empty() {
            return Err("Prepared edit does not advance a source revision".into());
        }
        for patch in &self.patches {
            patch.validate()?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn palette_rejects_ambiguous_entries_and_unknown_contracts() -> Result<(), String> {
        let mut p = Palette {
            schema_version: 1,
            id: "test".into(),
            label: "Test".into(),
            groups: vec![Group {
                id: "g".into(),
                label: "Group".into(),
                entries: vec![Tool {
                    id: "t".into(),
                    label: "Tool".into(),
                    icon: "box".into(),
                    gesture: "place".into(),
                    command: "a.command".into(),
                    parameters: Default::default(),
                    inputs: vec![],
                }],
            }],
        };
        p.validate()?;
        let duplicate = p.groups[0].entries[0].clone();
        p.groups[0].entries.push(duplicate);
        assert!(p.validate().is_err());
        p.groups[0].entries.pop();
        p.schema_version = 2;
        assert!(p.validate().is_err());
        Ok(())
    }
}
