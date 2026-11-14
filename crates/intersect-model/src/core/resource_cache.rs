//! Handwritten portable prepared-model artifacts. Hosts supply exact identities
//! and bytes; codecs and fresh validation share the ordinary model services.
use super::{
    interchange::{FormatRegistry, ReferenceClosure, Resource},
    model::Model,
    resource_set::ResourceContext,
    resources::{ResourceLoadState, ResourceRecord},
    storage::{InvocationContext, StorageResult},
};
use crate::definitions::schema::Schema;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};

pub fn content_digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

/// An explicit expected build context, supplied independently by the consumer.
/// Source entries include transitive sources; implementation entries pin the
/// selected semantic providers. These identities never install implementations.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CacheIdentity {
    pub sources: BTreeMap<String, String>,
    pub implementations: BTreeMap<String, String>,
    pub runtime: String,
    pub format: String,
    pub format_revision: String,
}
impl CacheIdentity {
    pub fn validate(&self) -> StorageResult<()> {
        if self.sources.is_empty()
            || self
                .sources
                .iter()
                .any(|(uri, hash)| uri.trim().is_empty() || !is_digest(hash))
            || self
                .implementations
                .iter()
                .any(|(id, revision)| id.trim().is_empty() || revision.trim().is_empty())
            || [&self.runtime, &self.format, &self.format_revision]
                .iter()
                .any(|id| id.trim().is_empty())
        {
            return Err("Prepared cache requires exact source, runtime, format and implementation identities".into());
        }
        Ok(())
    }
}
fn is_digest(text: &str) -> bool {
    text.len() == 64
        && text
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
}

fn check_cached_record(record: &ResourceRecord, identity: &CacheIdentity) -> StorageResult<()> {
    if record.load_state != ResourceLoadState::Loaded || !record.diagnostics.is_empty() {
        return Err(format!(
            "Prepared cache requires loaded content without unresolved diagnostics: {}",
            record.uri
        ));
    }
    if let Some(source) = &record.source
        && identity.sources.get(&source.uri) != Some(&source.revision)
    {
        return Err(format!(
            "Prepared cache omits exact source correspondence: {}",
            source.uri
        ));
    }
    Ok(())
}

#[derive(Clone, Copy, Debug)]
pub struct CacheLimits {
    pub max_bytes: usize,
    pub max_resources: usize,
}
impl Default for CacheLimits {
    fn default() -> Self {
        Self {
            max_bytes: 16_777_216,
            max_resources: 1024,
        }
    }
}

/// Model data only. No executable package, provider, pointer or local handle.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PreparedModelCache {
    version: u32,
    context: ResourceContext,
    identity: CacheIdentity,
    resources: Vec<Resource>,
    digests: BTreeMap<String, String>,
}
impl PreparedModelCache {
    pub fn identity(&self) -> &CacheIdentity {
        &self.identity
    }
    pub fn context(&self) -> &ResourceContext {
        &self.context
    }
    pub fn resources(&self) -> &[Resource] {
        &self.resources
    }

    pub fn prepare(
        registry: &FormatRegistry,
        schema: &Schema,
        context: &ResourceContext,
        model: &Model,
        selected: &[String],
        identity: CacheIdentity,
        validate_language: impl FnOnce(&Model) -> StorageResult<()>,
    ) -> StorageResult<Self> {
        identity.validate()?;
        context.check_schema(schema)?;
        model.validate(schema)?;
        validate_language(model)?;
        let resources = registry.export_selected(
            &identity.format,
            schema,
            model,
            selected,
            ReferenceClosure::IncludeDependencies,
        )?;
        for resource in &resources {
            let record = model
                .resources
                .get(&resource.uri)
                .ok_or("Cache resource is missing")?;
            check_cached_record(record, &identity)?;
        }
        let digests = resources
            .iter()
            .map(|resource| (resource.uri.clone(), content_digest(&resource.bytes)))
            .collect();
        Ok(Self {
            version: 1,
            context: context.clone(),
            identity,
            resources,
            digests,
        })
    }

    pub fn to_bytes(&self) -> StorageResult<Vec<u8>> {
        serde_json::to_vec(self).map_err(|error| format!("Prepared cache encoding: {error}"))
    }

    /// Admission is bounded before JSON decoding. The versioned canonical wire
    /// rejects alternate/duplicate-key encodings instead of silently normalizing.
    pub fn from_bytes(bytes: &[u8], limits: CacheLimits) -> StorageResult<Self> {
        if bytes.len() > limits.max_bytes {
            return Err("Prepared cache exceeds its byte limit".into());
        }
        let cache: Self = serde_json::from_slice(bytes)
            .map_err(|error| format!("Invalid prepared cache: {error}"))?;
        if cache.resources.len() > limits.max_resources || cache.to_bytes()? != bytes {
            return Err("Prepared cache exceeds its resource limit or is not canonical".into());
        }
        Ok(cache)
    }

    pub(crate) fn decode(
        &self,
        expected: &CacheIdentity,
        context: &ResourceContext,
        schema: &Schema,
        registry: &FormatRegistry,
        retained: &Model,
        observation: &InvocationContext<'_>,
    ) -> StorageResult<Model> {
        observation.checkpoint()?;
        expected.validate()?;
        self.identity.validate()?;
        context.check_schema(schema)?;
        if self.version != 1 || &self.identity != expected || &self.context != context {
            return Err("Stale or incompatible prepared model cache".into());
        }
        if self.resources.is_empty() || self.resources.len() != self.digests.len() {
            return Err("Prepared cache has an invalid resource manifest".into());
        }
        let mut seen = BTreeSet::new();
        for resource in &self.resources {
            observation.work(1 + resource.bytes.len().div_ceil(256))?;
            if !seen.insert(&resource.uri)
                || self.digests.get(&resource.uri) != Some(&content_digest(&resource.bytes))
            {
                return Err(format!(
                    "Corrupt or duplicate prepared resource: {}",
                    resource.uri
                ));
            }
        }
        observation.checkpoint()?;
        let model =
            registry.import_with_context(&expected.format, schema, &self.resources, retained)?;
        for resource in &self.resources {
            check_cached_record(
                model
                    .resources
                    .get(&resource.uri)
                    .ok_or("Cache omitted a resource record")?,
                expected,
            )?;
        }
        observation.checkpoint()?;
        Ok(model)
    }
}
