//! Handwritten residency orchestration over existing transactions and codecs.
use super::*;
use crate::core::{
    resource_cache::{CacheIdentity, PreparedModelCache},
    resources::{UnloadedContents, validate_records},
    storage::InvocationContext,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum UnloadPolicy {
    RequireSaved,
    DiscardChanges,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ResourceChangeKind {
    Created,
    Edited,
    Replaced,
    Unloaded,
    Reloaded,
    CacheLoaded,
    Removed,
}

/// A committed change. Invalidated resources conservatively cover the closed
/// context; consumers must discard derived/link/query results for that context.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResourceChange {
    pub kind: ResourceChangeKind,
    pub resources: Vec<String>,
    pub invalidated: Vec<String>,
}

/// Unpublished decoded content tied to an exact resource-set incarnation/base.
/// The holder owns incoming values only while preparation/publication is active.
pub struct PreparedReload {
    scope: Arc<()>,
    base: String,
    selected: BTreeSet<String>,
    candidate: Model,
    readonly: Option<storage::ReadonlyDecodeProof>,
}

impl ResourceSet<FrozenStore> {
    pub fn from_resident_model(
        schema: Schema,
        context: ResourceContext,
        model: &Model,
    ) -> StorageResult<Self> {
        let store = FrozenStore::from_materialized(&schema, model)?;
        Self::from_resident_store(schema, context, store)
    }
}
impl<S: ModelStore> ResourceSet<S> {
    /// Adopt an explicit partial-residency snapshot without claiming semantic
    /// validity. Full checked editing remains unavailable until validated reload.
    pub fn from_resident_store(
        schema: Schema,
        context: ResourceContext,
        store: S,
    ) -> StorageResult<Self> {
        context.check_schema(&schema)?;
        if store
            .resource_records()?
            .values()
            .all(|record| record.load_state == ResourceLoadState::Loaded)
        {
            return Self::from_store(schema, context, store);
        }
        let model = TransactionalModel::new(BasicModel::from_resident_store(schema, store)?);
        Ok(Self {
            model,
            context,
            scope: Arc::new(()),
            saved: BTreeMap::new(),
            fragments: BTreeMap::new(),
            changes: Vec::new(),
        })
    }

    pub fn into_store(self) -> S {
        self.model.into_basic().into_store()
    }

    pub fn validate(&self) -> StorageResult<()> {
        self.require_resident()?;
        self.model.validate()
    }
    pub fn validate_with_conversions(
        &self,
        conversions: &ConversionRegistry<'_>,
    ) -> StorageResult<()> {
        self.require_resident()?;
        self.model.validate_with_conversions(conversions)
    }

    pub(super) fn require_resident(&self) -> StorageResult<()> {
        for (uri, record) in self.reader().resource_records()? {
            if record.load_state != ResourceLoadState::Loaded {
                return Err(format!(
                    "Resource content unavailable: {uri}; reload the closed resource set before checked editing or validation"
                ));
            }
        }
        Ok(())
    }

    pub(super) fn record_change(&mut self, change: ResourceChange) {
        if !change.resources.is_empty() {
            self.changes.push(change);
        }
    }
    pub fn drain_changes(&mut self) -> Vec<ResourceChange> {
        std::mem::take(&mut self.changes)
    }

    /// Release selected values. Cross-boundary containment must be unloaded as
    /// one selection; ordinary noncontainment references retain external IDs.
    pub fn unload(
        &mut self,
        registry: &FormatRegistry,
        selected: &[String],
        policy: UnloadPolicy,
    ) -> StorageResult<()> {
        self.unload_observed(registry, selected, policy, &InvocationContext::default())
    }
    pub fn unload_observed(
        &mut self,
        registry: &FormatRegistry,
        selected: &[String],
        policy: UnloadPolicy,
        observation: &InvocationContext<'_>,
    ) -> StorageResult<()> {
        self.unload_with_format_context(
            registry,
            selected,
            policy,
            &FormatContext::default(),
            observation,
        )
    }
    pub fn unload_with_format_context(
        &mut self,
        registry: &FormatRegistry,
        selected: &[String],
        policy: UnloadPolicy,
        formats: &FormatContext<'_>,
        observation: &InvocationContext<'_>,
    ) -> StorageResult<()> {
        observation.checkpoint()?;
        self.require_resident()?;
        formats.validate_store_observed(self.schema(), self.reader(), observation)?;
        let current = self.consistent_snapshot()?;
        let selection = Self::selection(&current, selected, true)?;
        for uri in &selection {
            observation.work(1)?;
            if policy == UnloadPolicy::RequireSaved
                && self.is_dirty_with_format_context(registry, uri, formats)?
            {
                return Err(format!(
                    "Dirty resource requires an explicit discard policy: {uri}"
                ));
            }
        }
        for element in &current.elements {
            observation.work(1)?;
            for (name, feature) in self.schema().features(&element.class)? {
                if !feature.containment || feature.derived {
                    continue;
                }
                let value = element
                    .values
                    .get(&name)
                    .ok_or("Missing containment field")?;
                let targets: Vec<&str> = match value {
                    serde_json::Value::String(id) => vec![id],
                    serde_json::Value::Array(values) => values
                        .iter()
                        .filter_map(serde_json::Value::as_str)
                        .collect(),
                    _ => Vec::new(),
                };
                for target in targets {
                    observation.work(1)?;
                    let target = current
                        .elements
                        .iter()
                        .find(|candidate| candidate.id == target)
                        .ok_or("Missing containment target")?;
                    if selection.contains(&element.resource) != selection.contains(&target.resource)
                    {
                        return Err(
                            "Unload selection crosses containment; include both resources".into(),
                        );
                    }
                }
            }
        }
        let mut candidate = Self::retained(&current, &selection);
        for uri in &selection {
            observation.work(1)?;
            let mut record = current
                .resources
                .get(uri)
                .ok_or("Selected resource disappeared")?
                .clone();
            let members: Vec<_> = current
                .elements
                .iter()
                .filter(|element| &element.resource == uri)
                .collect();
            let identities = members.iter().map(|element| element.id.clone()).collect();
            let content_digest = digest(&(&record, &members))?;
            record.content_revision = record
                .content_revision
                .checked_add(1)
                .ok_or("Resource revision exhausted")?;
            record.unloaded = Some(UnloadedContents {
                identities,
                roots: record.roots.clone(),
                content_digest,
            });
            record.roots = Arc::new(Vec::new());
            record.load_state = ResourceLoadState::Unloaded;
            candidate.resources.insert(uri.clone(), record);
        }
        let schema = self.schema().clone();
        storage::atomic(self.model.store_mut(), |store| {
            if storage::export(store)? != current {
                return Err("Stale resource unload base".into());
            }
            let mut removed_values = false;
            for element in &current.elements {
                if selection.contains(&element.resource) {
                    observation.work(1)?;
                    store.remove(&element.id)?;
                    removed_values = true;
                }
            }
            for record in candidate.resources.values() {
                store.set_resource_record(record.clone())?;
            }
            validate_records(store, &schema)?;
            if storage::export(store)? != candidate {
                return Err("Unload changed retained model content".into());
            }
            observation.checkpoint()?;
            if removed_values {
                store.release_unloaded_values()?;
            }
            Ok(())
        })?;
        self.record_change(ResourceChange {
            kind: ResourceChangeKind::Unloaded,
            resources: selection.into_iter().collect(),
            invalidated: candidate.resources.keys().cloned().collect(),
        });
        Ok(())
    }

    /// Decode against retained objects without publishing. Codec calls are
    /// synchronous boundaries bracketed by the operation's cancellation checks.
    pub fn prepare_reload(
        &mut self,
        registry: &FormatRegistry,
        format: &str,
        selected: &[String],
        bytes: &[Resource],
        context: &ResourceContext,
        observation: &InvocationContext<'_>,
    ) -> StorageResult<PreparedReload> {
        self.prepare_reload_with_format_context(
            registry,
            format,
            selected,
            bytes,
            context,
            &FormatContext::default(),
            observation,
        )
    }
    #[allow(clippy::too_many_arguments)] // Existing reload selection/context plus explicitly borrowed format adapters.
    pub fn prepare_reload_with_format_context(
        &mut self,
        registry: &FormatRegistry,
        format: &str,
        selected: &[String],
        bytes: &[Resource],
        context: &ResourceContext,
        formats: &FormatContext<'_>,
        observation: &InvocationContext<'_>,
    ) -> StorageResult<PreparedReload> {
        observation.checkpoint()?;
        self.check_context(context)?;
        let current = self.consistent_snapshot()?;
        let selection = Self::selection(&current, selected, false)?;
        let retained = Self::retained(&current, &selection);
        observation.work(bytes.len().max(1))?;
        let decoded = registry.import_with_format_context_and_proof(
            format,
            self.schema(),
            bytes,
            Some(&retained),
            formats,
        )?;
        observation.checkpoint()?;
        Ok(PreparedReload {
            scope: self.scope.clone(),
            base: digest(&current)?,
            selected: selection,
            candidate: decoded.model,
            readonly: decoded.readonly,
        })
    }
    pub fn commit_reload(
        &mut self,
        prepared: PreparedReload,
        validate_language: impl FnOnce(&Model) -> StorageResult<()>,
        observation: &InvocationContext<'_>,
    ) -> StorageResult<()> {
        self.commit_reload_with_conversions(prepared, None, validate_language, observation)
    }
    pub fn commit_reload_with_conversions(
        &mut self,
        prepared: PreparedReload,
        conversions: Option<&ConversionRegistry<'_>>,
        validate_language: impl FnOnce(&Model) -> StorageResult<()>,
        observation: &InvocationContext<'_>,
    ) -> StorageResult<()> {
        self.commit_reload_with_format_context(
            prepared,
            &FormatContext {
                conversions,
                ..Default::default()
            },
            validate_language,
            observation,
        )
    }
    pub fn commit_reload_with_format_context(
        &mut self,
        prepared: PreparedReload,
        formats: &FormatContext<'_>,
        validate_language: impl FnOnce(&Model) -> StorageResult<()>,
        observation: &InvocationContext<'_>,
    ) -> StorageResult<()> {
        observation.checkpoint()?;
        let current = self.consistent_snapshot()?;
        if !Arc::ptr_eq(&prepared.scope, &self.scope) || digest(&current)? != prepared.base {
            return Err("Stale or foreign prepared resource reload".into());
        }
        self.publish_restored(
            current,
            &prepared.selected,
            prepared.candidate,
            ResourceChangeKind::Reloaded,
            formats,
            prepared.readonly.as_ref(),
            |candidate| {
                validate_language(candidate)?;
                observation.checkpoint()
            },
            observation,
        )
    }

    #[allow(clippy::too_many_arguments)] // Preserve the restore kind, validator and borrowed conversion/observation context.
    fn publish_restored(
        &mut self,
        current: Model,
        selected: &BTreeSet<String>,
        candidate: Model,
        kind: ResourceChangeKind,
        formats: &FormatContext<'_>,
        readonly: Option<&storage::ReadonlyDecodeProof>,
        validate_language: impl FnOnce(&Model) -> StorageResult<()>,
        observation: &InvocationContext<'_>,
    ) -> StorageResult<()> {
        let fragments = self.fragments.clone();
        self.publish_with_validation(
            current,
            selected,
            candidate,
            validate_language,
            formats,
            readonly,
            observation,
        )?;
        for (key, ids) in fragments {
            if selected.contains(&key.0) {
                self.fragments.insert(key, ids);
            }
        }
        if let Some(change) = self.changes.last_mut() {
            change.kind = kind;
        }
        Ok(())
    }

    pub fn prepare_cache(
        &mut self,
        registry: &FormatRegistry,
        selected: &[String],
        identity: CacheIdentity,
        validate_language: impl FnOnce(&Model) -> StorageResult<()>,
    ) -> StorageResult<PreparedModelCache> {
        self.require_resident()?;
        let model = self.consistent_snapshot()?;
        PreparedModelCache::prepare(
            registry,
            self.schema(),
            &self.context,
            &model,
            selected,
            identity,
            validate_language,
        )
    }

    pub fn load_cache(
        &mut self,
        registry: &FormatRegistry,
        cache: &PreparedModelCache,
        expected: &CacheIdentity,
        validate_language: impl FnOnce(&Model) -> StorageResult<()>,
        observation: &InvocationContext<'_>,
    ) -> StorageResult<()> {
        let current = self.consistent_snapshot()?;
        let selection: BTreeSet<_> = cache
            .resources()
            .iter()
            .map(|resource| resource.uri.clone())
            .collect();
        let retained = Self::retained(&current, &selection);
        let candidate = cache.decode(
            expected,
            &self.context,
            self.schema(),
            registry,
            &retained,
            observation,
        )?;
        self.publish_restored(
            current,
            &selection,
            candidate,
            ResourceChangeKind::CacheLoaded,
            &FormatContext::default(),
            None,
            |candidate| {
                validate_language(candidate)?;
                observation.checkpoint()
            },
            observation,
        )
    }

    /// Deletion removes the resource and its objects, with full survivor checks.
    /// Unload is a separate operation and never calls this path.
    pub fn remove_resource(&mut self, uri: &str) -> StorageResult<()> {
        self.remove_resource_with(uri, |_| Ok(()))
    }
    pub fn remove_resource_with(
        &mut self,
        uri: &str,
        validate_language: impl FnOnce(&Model) -> StorageResult<()>,
    ) -> StorageResult<()> {
        self.require_resident()?;
        let current = self.consistent_snapshot()?;
        let selected = Self::selection(&current, &[uri.into()], true)?;
        let candidate = Self::retained(&current, &selected);
        candidate.validate(self.schema())?;
        validate_language(&candidate)?;
        let schema = self.schema().clone();
        storage::atomic(self.model.store_mut(), |store| {
            if storage::export(store)? != current {
                return Err("Stale resource removal base".into());
            }
            for element in &current.elements {
                if element.resource == uri {
                    store.remove(&element.id)?;
                }
            }
            store.remove_resource_record(uri)?;
            for record in candidate.resources.values() {
                store.set_resource_record(record.clone())?;
            }
            storage::validate(store, &schema)
        })?;
        self.fragments.retain(|(resource, _), _| resource != uri);
        self.saved.remove(uri);
        self.record_change(ResourceChange {
            kind: ResourceChangeKind::Removed,
            resources: vec![uri.into()],
            invalidated: candidate.resources.keys().cloned().collect(),
        });
        Ok(())
    }
}
