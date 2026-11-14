//! Source-bound computed setting declarations. Metadata alone is not executable.
use crate::definitions::{
    bundle::{DefinitionBundle, SourceId, SourceKind, ValidatedBundle},
    conversion::ProviderId,
    schema::{MemberIdentity, Schema},
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SettingOperation {
    Set,
    Append,
    Unset,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ComputedSettingContract {
    pub version: u32,
    pub schema_revision: String,
    pub class: String,
    pub class_identity: String,
    pub feature: String,
    pub member: MemberIdentity,
    pub provider: ProviderId,
    pub sources: Vec<SourceId>,
    pub operations: Vec<SettingOperation>,
}
impl ComputedSettingContract {
    /// Validate exact identities against an already compiled/source-verified schema.
    /// This does not compile source text, install code, or authorize grammar writes.
    pub fn validate(&self, schema: &Schema, bundle: &DefinitionBundle) -> Result<(), String> {
        self.validate_in(schema, &bundle.validated()?)
    }
    /// `validate` against a bundle already validated on this borrow, for
    /// callers that check many contracts of one bundle.
    pub fn validate_in(&self, schema: &Schema, bundle: &ValidatedBundle<'_>) -> Result<(), String> {
        let bundle = bundle.bundle();
        if self.version != 1
            || self.schema_revision != schema.revision
            || !schema.classes.contains_key(&self.class)
            || self.class_identity != schema.identity(&self.class)
            || self.provider.name.trim().is_empty()
            || self.provider.version.trim().is_empty()
        {
            return Err("Invalid or stale computed setting contract".into());
        }
        let field = schema.effective_feature(&self.class, &self.feature)?;
        if field.identity != self.member || !field.feature.derived || !field.feature.changeable {
            return Err("Computed setting requires the exact changeable derived member".into());
        }
        let mut sources = BTreeSet::new();
        for id in &self.sources {
            if !sources.insert(id) || !bundle.sources.iter().any(|s| s.id == *id) {
                return Err("Duplicate, missing or stale computed setting source".into());
            }
        }
        let MemberIdentity::Source { declaration } = &self.member else {
            return Err("Computed setting requires a source declaration identity".into());
        };
        let class = schema
            .declarations
            .get(&self.class)
            .ok_or("Computed setting requires a source class identity")?;
        if !sources.contains(&declaration.source) || !sources.contains(&class.source) {
            return Err("Computed setting sources omit the member or receiver declaration".into());
        }
        for id in [&declaration.source, &class.source] {
            if !bundle
                .sources
                .iter()
                .any(|s| s.id == *id && s.kind == SourceKind::Rcore)
            {
                return Err("Computed setting declaration source must be Rcore".into());
            }
        }
        let mut operations = BTreeSet::new();
        for operation in &self.operations {
            if !operations.insert(operation) {
                return Err("Duplicate computed setting operation".into());
            }
            if *operation == SettingOperation::Append
                && (field.feature.upper == 0 || field.feature.upper == 1)
            {
                return Err("Computed append contract requires a many-valued feature".into());
            }
        }
        if operations.is_empty() {
            return Err("Computed setting contract has no operations".into());
        }
        Ok(())
    }
}

#[cfg(test)]
#[path = "setting_tests.rs"]
mod tests;
