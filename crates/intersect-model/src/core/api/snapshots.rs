//! Handwritten portable state snapshots, independent of host revisions and I/O.
use super::{ModelSnapshot, versioned::snapshot_draft};
use crate::{
    core::{frozen::FrozenStore, model::Model, storage},
    definitions::schema::Schema,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{fmt, sync::Arc};

const FORMAT: &str = "intersect-model-snapshot-v1";

/// Content identities are separate from owner-scoped, in-memory VersionId.
/// The schema digest covers the full serialized schema, including its revision.
/// The content digest covers canonical exported model data, including row order,
/// assignment/initialization evidence and logical resource metadata.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SnapshotIdentity {
    pub schema: String,
    pub content: String,
}

/// Input/output admission bounds, not a CPU or allocation budget.
#[derive(Clone, Copy, Debug)]
pub struct SnapshotLimits {
    pub max_bytes: usize,
    pub max_objects: usize,
}
impl Default for SnapshotLimits {
    fn default() -> Self {
        Self {
            max_bytes: 16 * 1024 * 1024,
            max_objects: 100_000,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SnapshotError {
    UnsupportedFormat,
    SchemaMismatch,
    ContentMismatch,
    LimitExceeded,
    Invalid(String),
}
impl fmt::Display for SnapshotError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnsupportedFormat => f.write_str("Unsupported model snapshot format"),
            Self::SchemaMismatch => f.write_str("Model snapshots require the exact schema"),
            Self::ContentMismatch => f.write_str("Model snapshot content digest differs"),
            Self::LimitExceeded => f.write_str("Model snapshot admission limit exceeded"),
            Self::Invalid(error) => f.write_str(error),
        }
    }
}
impl std::error::Error for SnapshotError {}
impl From<String> for SnapshotError {
    fn from(error: String) -> Self {
        Self::Invalid(error)
    }
}
impl From<serde_json::Error> for SnapshotError {
    fn from(error: serde_json::Error) -> Self {
        Self::Invalid(error.to_string())
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Wire {
    format: String,
    identity: SnapshotIdentity,
    model: Model,
}

fn digest(value: &impl Serialize) -> Result<String, SnapshotError> {
    Ok(format!(
        "sha256:{:x}",
        Sha256::digest(serde_json::to_vec(value)?)
    ))
}
pub(super) fn identity(schema: &Schema, model: &Model) -> Result<SnapshotIdentity, SnapshotError> {
    Ok(SnapshotIdentity {
        schema: digest(schema)?,
        content: digest(model)?,
    })
}

impl ModelSnapshot {
    pub fn identity(&self) -> Result<SnapshotIdentity, SnapshotError> {
        identity(self.schema(), &storage::export(self)?)
    }

    /// Completeness is assessed separately; capturing a draft does not publish it.
    pub fn validate(&self) -> storage::StorageResult<()> {
        storage::validate(self, self.schema())
    }

    /// Reopen an independent checked editable model, preserving durable object
    /// identities while issuing fresh owner-scoped handles/version identities.
    pub fn to_model(&self) -> storage::StorageResult<super::BasicModel> {
        let data = storage::export(self)?;
        let store = FrozenStore::from_materialized(self.schema(), &data)?;
        super::BasicModel::from_store(self.schema().clone(), store)
    }

    /// Portable authoritative state data, distinct from the frozen binary cache.
    /// The host retains/acquires the exact schema separately and owns physical I/O.
    pub fn to_bytes(&self, limits: SnapshotLimits) -> Result<Vec<u8>, SnapshotError> {
        let model = storage::export(self)?;
        if model.elements.len() > limits.max_objects {
            return Err(SnapshotError::LimitExceeded);
        }
        let bytes = serde_json::to_vec(&Wire {
            format: FORMAT.into(),
            identity: identity(self.schema(), &model)?,
            model,
        })?;
        if bytes.len() > limits.max_bytes {
            return Err(SnapshotError::LimitExceeded);
        }
        Ok(bytes)
    }

    /// Decode data, verify identities, and rerun native draft-integrity checks.
    /// Serialized validity claims and schema/provider implementations are never trusted.
    pub fn from_bytes(
        schema: &Schema,
        bytes: &[u8],
        limits: SnapshotLimits,
    ) -> Result<Self, SnapshotError> {
        if bytes.len() > limits.max_bytes {
            return Err(SnapshotError::LimitExceeded);
        }
        let raw: serde_json::Value = serde_json::from_slice(bytes)?;
        let wire: Wire = serde_json::from_slice(bytes)?;
        if wire.format != FORMAT {
            return Err(SnapshotError::UnsupportedFormat);
        }
        if wire.model.elements.len() > limits.max_objects {
            return Err(SnapshotError::LimitExceeded);
        }
        // Refuse unrecognized nested fields or implicit DTO defaults; comparison
        // is structural JSON equality, independent of whitespace/key ordering.
        if serde_json::to_value(&wire)? != raw {
            return Err(SnapshotError::Invalid(
                "Snapshot requires exact explicit model fields".into(),
            ));
        }
        let expected = identity(schema, &wire.model)?;
        if wire.identity.schema != expected.schema || wire.model.schema_revision != schema.revision
        {
            return Err(SnapshotError::SchemaMismatch);
        }
        if wire.identity.content != expected.content {
            return Err(SnapshotError::ContentMismatch);
        }
        // Check the actual incoming graph before arena import can normalize it.
        let reader = storage::IndexedModel::new(&wire.model)?;
        storage::validate_draft(&reader, schema)?;
        let store = FrozenStore::from_materialized(schema, &wire.model)?;
        let snapshot = snapshot_draft(Arc::new(schema.clone()), store)?;
        if snapshot.identity()? != wire.identity {
            return Err(SnapshotError::ContentMismatch);
        }
        Ok(snapshot)
    }
}
