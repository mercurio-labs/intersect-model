//! Handwritten closed resource sets over the shared model store. No host I/O.
use crate::core::{
    api::{BasicModel, EditSession, TransactionalModel},
    frozen::FrozenStore,
    interchange::{FormatContext, FormatRegistry, ReferenceClosure, Resource},
    model::{Element, Model},
    project_usages::DependencyLock,
    resources::{ResourceLoadState, ResourceRecord, resolve_resource_uri},
    storage::{self, ModelStore, StorageResult},
};
use crate::definitions::{conversion::ConversionRegistry, schema::Schema};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
};

use crate::core::fingerprint::digest;

/// Exact structural and caller-declared language/dependency identities. This
/// does not infer language visibility or install semantic implementations.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResourceContext {
    schema_revision: String,
    schema_digest: String,
    dependencies: BTreeMap<String, String>,
}
impl ResourceContext {
    /// Use the complete direct and transitive package identities from a lock.
    pub fn from_lock(schema: &Schema, lock: &DependencyLock) -> StorageResult<Self> {
        let dependencies = lock
            .entries()
            .iter()
            .map(|(uri, entry)| Ok((uri.clone(), digest(&entry.identity)?)))
            .collect::<Result<_, String>>()?;
        Self::new(schema, dependencies)
    }

    pub fn new(schema: &Schema, dependencies: BTreeMap<String, String>) -> StorageResult<Self> {
        if schema.revision.is_empty()
            || dependencies
                .iter()
                .any(|(id, revision)| id.is_empty() || revision.is_empty())
        {
            return Err(
                "Resource context requires exact nonempty schema/dependency identities".into(),
            );
        }
        Ok(Self {
            schema_revision: schema.revision.clone(),
            schema_digest: digest(schema)?,
            dependencies,
        })
    }
    pub(crate) fn check_schema(&self, schema: &Schema) -> StorageResult<()> {
        // Deserialization bypasses `new`, so admission repeats the identity gate.
        if self.schema_revision.is_empty()
            || self.schema_digest.is_empty()
            || schema.revision.is_empty()
            || self
                .dependencies
                .iter()
                .any(|(id, revision)| id.is_empty() || revision.is_empty())
        {
            return Err(
                "Resource context requires exact nonempty schema/dependency identities".into(),
            );
        }
        if self.schema_revision != schema.revision || self.schema_digest != digest(schema)? {
            return Err("Incompatible resource schema context".into());
        }
        Ok(())
    }
    pub fn dependencies(&self) -> &BTreeMap<String, String> {
        &self.dependencies
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TargetResolution {
    Resolved {
        resource: String,
        element: String,
    },
    MissingResource(String),
    UnavailableResource {
        resource: String,
        state: ResourceLoadState,
    },
    MissingFragment {
        resource: String,
        fragment: String,
    },
    Ambiguous {
        resource: String,
        fragment: String,
        candidates: Vec<String>,
    },
}

/// A host writes these bytes, then acknowledges this exact nonserializable
/// ticket. Tickets belong to one resource-set instance and never contain handles.
#[derive(Clone, Debug)]
pub struct PreparedSave {
    resources: Vec<Resource>,
    scope: Arc<()>,
    fingerprints: BTreeMap<String, String>,
    format: String,
    selection: Vec<String>,
    artifacts: BTreeMap<String, String>,
}

impl PreparedSave {
    pub fn resources(&self) -> &[Resource] {
        &self.resources
    }
}

/// One graph, owned by the existing transactional facade. ResourceSet adds
/// explicit context and lifecycle orchestration, not a second object store.
#[derive(Clone)]
struct SavedState {
    fingerprint: String,
    format: String,
    selection: Vec<String>,
    artifact: String,
}

pub struct ResourceSet<S: ModelStore = FrozenStore> {
    model: TransactionalModel<S>,
    context: ResourceContext,
    scope: Arc<()>,
    saved: BTreeMap<String, SavedState>,
    fragments: BTreeMap<(String, String), BTreeSet<String>>,
    changes: Vec<ResourceChange>,
}
impl<S: storage::DraftConstructionStore> ResourceSet<S> {
    /// Explicit concrete immutable admission, with one operation-local query
    /// context. The generic arbitrary-storage entry retains literal semantics.
    #[allow(clippy::too_many_arguments)] // SAME caller-owned admission services.
    pub fn from_native_with_structural_values(
        schema: Schema,
        context: ResourceContext,
        store: S,
        conversions: Option<&ConversionRegistry<'_>>,
        values: &dyn storage::StructuralValueSelector,
        budget: &mut crate::core::query::QueryBudget,
        control: &storage::InvocationContext<'_>,
        validate_language: impl FnOnce(&S, &mut crate::core::query::QueryBudget) -> StorageResult<()>,
    ) -> StorageResult<Self> {
        context.check_schema(&schema)?;
        let basic = BasicModel::from_native_with_structural_values(
            schema,
            store,
            conversions,
            values,
            budget,
            control,
            validate_language,
        )?;
        Ok(Self::from_admitted_basic(context, basic))
    }
}

impl ResourceSet<FrozenStore> {
    pub fn new(schema: Schema, context: ResourceContext) -> StorageResult<Self> {
        let store = FrozenStore::for_schema(&schema).map_err(|error| error.to_string())?;
        Self::from_store(schema, context, store)
    }
    pub fn from_model(
        schema: Schema,
        context: ResourceContext,
        model: &Model,
    ) -> StorageResult<Self> {
        let store = FrozenStore::from_materialized(&schema, model)?;
        Self::from_store(schema, context, store)
    }
}
impl<S: ModelStore> ResourceSet<S> {
    pub fn from_store(schema: Schema, context: ResourceContext, store: S) -> StorageResult<Self> {
        Self::from_store_with_validation(schema, context, store, None, |_| Ok(()))
    }
    /// Adopt the same store after complete structural checks and an explicit
    /// language validation callback. Providers are borrowed for this admission;
    /// the set does not retain them or infer semantic validity from dependency IDs.
    pub fn from_store_with_validation(
        schema: Schema,
        context: ResourceContext,
        store: S,
        conversions: Option<&ConversionRegistry<'_>>,
        validate_language: impl FnOnce(&S) -> StorageResult<()>,
    ) -> StorageResult<Self> {
        context.check_schema(&schema)?;
        let basic = match conversions {
            Some(registry) => BasicModel::from_store_with_conversions(schema, store, registry)?,
            None => BasicModel::from_store(schema, store)?,
        };
        let model = TransactionalModel::new(basic);
        match conversions {
            Some(registry) => model.validate_with_conversions(registry)?,
            None => model.validate()?,
        }
        validate_language(model.reader())?;
        Ok(Self {
            model,
            context,
            scope: Arc::new(()),
            saved: BTreeMap::new(),
            fragments: BTreeMap::new(),
            changes: Vec::new(),
        })
    }
    /// Admit this SAME store using explicit source/language effective values.
    /// Binding, full multiplicities/types and the language callback run in one
    /// atomic admission with the original caller budget and control.
    /// Providers are borrowed, never retained; default/raw APIs remain strict.
    #[allow(clippy::too_many_arguments)] // Explicit same-store schema/context plus borrowed operation services.
    pub fn from_store_with_structural_values(
        schema: Schema,
        context: ResourceContext,
        store: S,
        conversions: Option<&ConversionRegistry<'_>>,
        values: &dyn storage::StructuralValueSelector,
        budget: &mut crate::core::query::QueryBudget,
        control: &storage::InvocationContext<'_>,
        validate_language: impl FnOnce(&S, &mut crate::core::query::QueryBudget) -> StorageResult<()>,
    ) -> StorageResult<Self> {
        context.check_schema(&schema)?;
        let basic = BasicModel::from_store_with_structural_values(
            schema,
            store,
            conversions,
            values,
            budget,
            control,
            validate_language,
        )?;
        Ok(Self::from_admitted_basic(context, basic))
    }
    fn from_admitted_basic(context: ResourceContext, basic: BasicModel<S>) -> Self {
        Self {
            model: TransactionalModel::new(basic),
            context,
            scope: Arc::new(()),
            saved: BTreeMap::new(),
            fragments: BTreeMap::new(),
            changes: Vec::new(),
        }
    }
    /// Full checked reread using the explicitly selected source/language values.
    pub fn validate_with_structural_values(
        &self,
        conversions: Option<&ConversionRegistry<'_>>,
        values: &dyn storage::StructuralValueSelector,
        budget: &mut crate::core::query::QueryBudget,
        control: &storage::InvocationContext<'_>,
    ) -> StorageResult<()> {
        self.require_resident()?;
        storage::validate_with_structural_values(
            self.reader(),
            self.schema(),
            conversions,
            values,
            budget,
            control,
        )
    }
    pub fn context(&self) -> &ResourceContext {
        &self.context
    }
    pub fn schema(&self) -> &Schema {
        self.model.schema()
    }
    pub fn reader(&self) -> &S {
        self.model.reader()
    }
    pub fn snapshot(&mut self) -> StorageResult<Model> {
        self.consistent_snapshot()
    }
    fn consistent_snapshot(&mut self) -> StorageResult<Model> {
        storage::atomic(self.model.store_mut(), |store| storage::export(store))
    }
    pub fn resources(&self) -> StorageResult<BTreeMap<String, ResourceRecord>> {
        self.reader().resource_records()
    }
    pub fn transaction<T>(
        &mut self,
        edit: impl FnOnce(&mut EditSession<'_, S>) -> StorageResult<T>,
    ) -> StorageResult<T> {
        self.transaction_with(edit, |_| Ok(()))
    }
    pub fn transaction_with<T>(
        &mut self,
        edit: impl FnOnce(&mut EditSession<'_, S>) -> StorageResult<T>,
        validate: impl FnOnce(&EditSession<'_, S>) -> StorageResult<()>,
    ) -> StorageResult<T> {
        self.transaction_context(&FormatContext::default(), edit, validate)
    }
    /// Use one borrowed conversion registry for all edits and final validation.
    pub fn transaction_with_conversions<T>(
        &mut self,
        conversions: &ConversionRegistry<'_>,
        edit: impl FnOnce(&mut EditSession<'_, S>) -> StorageResult<T>,
    ) -> StorageResult<T> {
        self.transaction_context(
            &FormatContext {
                conversions: Some(conversions),
                ..Default::default()
            },
            edit,
            |_| Ok(()),
        )
    }
    pub fn transaction_with_conversions_and_validation<T>(
        &mut self,
        conversions: &ConversionRegistry<'_>,
        edit: impl FnOnce(&mut EditSession<'_, S>) -> StorageResult<T>,
        validate: impl FnOnce(&EditSession<'_, S>) -> StorageResult<()>,
    ) -> StorageResult<T> {
        self.transaction_context(
            &FormatContext {
                conversions: Some(conversions),
                ..Default::default()
            },
            edit,
            validate,
        )
    }
    /// Checked edits and final audit use one explicitly borrowed provider context.
    pub fn transaction_with_format_context<T>(
        &mut self,
        formats: &FormatContext<'_>,
        edit: impl FnOnce(&mut EditSession<'_, S>) -> StorageResult<T>,
        validate: impl FnOnce(&EditSession<'_, S>) -> StorageResult<()>,
    ) -> StorageResult<T> {
        self.transaction_context(formats, edit, validate)
    }
    fn transaction_context<T>(
        &mut self,
        formats: &FormatContext<'_>,
        edit: impl FnOnce(&mut EditSession<'_, S>) -> StorageResult<T>,
        validate: impl FnOnce(&EditSession<'_, S>) -> StorageResult<()>,
    ) -> StorageResult<T> {
        self.require_resident()?;
        let edit = |session: &mut EditSession<'_, S>| {
            use crate::core::storage::ModelRead;
            let before = session.resource_records()?;
            let result = edit(session)?;
            let after = session.resource_records()?;
            let changed = after
                .iter()
                .filter(|(uri, record)| before.get(*uri) != Some(*record))
                .map(|(uri, _)| uri.clone())
                .collect();
            Ok((
                result,
                ResourceChange {
                    kind: ResourceChangeKind::Edited,
                    resources: changed,
                    invalidated: after.keys().cloned().collect(),
                },
            ))
        };
        let (result, change) = if formats.structural_values.is_some() {
            self.model
                .transaction_with_format_context(formats, edit, validate)?
        } else {
            match formats.conversions {
                Some(registry) => self
                    .model
                    .transaction_with_conversions_and_validation(registry, edit, validate)?,
                None => self.model.transaction_with(edit, validate)?,
            }
        };
        self.record_change(change);
        Ok(result)
    }
    pub fn create_resource(&mut self, uri: &str) -> StorageResult<()> {
        use crate::core::api::ModelEdit;
        self.transaction(|session| session.create_resource(uri))?;
        if let Some(change) = self.changes.last_mut() {
            change.kind = ResourceChangeKind::Created;
        }
        Ok(())
    }
    pub fn reorder_roots(&mut self, uri: &str, roots: &[String]) -> StorageResult<()> {
        use crate::core::api::ModelEdit;
        self.transaction(|session| session.reorder_roots(uri, roots))
    }
    fn check_context(&self, context: &ResourceContext) -> StorageResult<()> {
        if context != &self.context {
            return Err("Incompatible resource dependency context".into());
        }
        context.check_schema(self.schema())
    }
    fn selection(
        model: &Model,
        selected: &[String],
        require_loaded: bool,
    ) -> StorageResult<BTreeSet<String>> {
        if selected.is_empty() {
            return Err("Select at least one resource".into());
        }
        let mut set = BTreeSet::new();
        for uri in selected {
            if !set.insert(uri.clone()) {
                return Err(format!("Duplicate selected resource: {uri}"));
            }
            let resource = model
                .resources
                .get(uri)
                .ok_or_else(|| format!("Unknown selected resource: {uri}"))?;
            if require_loaded && resource.load_state != ResourceLoadState::Loaded {
                return Err(format!("Selected resource is unavailable: {uri}"));
            }
        }
        Ok(set)
    }
    fn retained(current: &Model, selected: &BTreeSet<String>) -> Model {
        let mut model = current.clone();
        model.elements.retain(|e| !selected.contains(&e.resource));
        model.resources.retain(|uri, _| !selected.contains(uri));
        model
    }
    /// Replace exactly the supplied selection. Incoming objects may refer to
    /// retained objects; validation runs against the combined candidate. A stored
    /// opposite requiring a retained edit must be included in the selection.
    /// Existing empty unavailable resources may be replaced explicitly to retry
    /// loading; no resource loading or host I/O is performed here.
    pub fn replace(
        &mut self,
        selected: &[String],
        incoming: Model,
        context: &ResourceContext,
    ) -> StorageResult<()> {
        self.replace_with(selected, incoming, context, |_| Ok(()))
    }
    pub fn replace_with(
        &mut self,
        selected: &[String],
        incoming: Model,
        context: &ResourceContext,
        validate_language: impl FnOnce(&Model) -> StorageResult<()>,
    ) -> StorageResult<()> {
        self.replace_context(
            selected,
            incoming,
            context,
            &FormatContext::default(),
            validate_language,
        )
    }
    pub fn replace_with_conversions(
        &mut self,
        selected: &[String],
        incoming: Model,
        context: &ResourceContext,
        conversions: &ConversionRegistry<'_>,
        validate_language: impl FnOnce(&Model) -> StorageResult<()>,
    ) -> StorageResult<()> {
        self.replace_context(
            selected,
            incoming,
            context,
            &FormatContext {
                conversions: Some(conversions),
                ..Default::default()
            },
            validate_language,
        )
    }
    /// Replace selected resources under the same explicitly supplied structural authority.
    pub fn replace_with_format_context(
        &mut self,
        selected: &[String],
        incoming: Model,
        context: &ResourceContext,
        formats: &FormatContext<'_>,
        validate_language: impl FnOnce(&Model) -> StorageResult<()>,
    ) -> StorageResult<()> {
        self.replace_context(selected, incoming, context, formats, validate_language)
    }
    fn replace_context(
        &mut self,
        selected: &[String],
        incoming: Model,
        context: &ResourceContext,
        formats: &FormatContext<'_>,
        validate_language: impl FnOnce(&Model) -> StorageResult<()>,
    ) -> StorageResult<()> {
        self.check_context(context)?;
        let current = self.consistent_snapshot()?;
        let selection = Self::selection(&current, selected, false)?;
        if incoming.schema_revision != self.schema().revision
            || incoming.resources.keys().cloned().collect::<BTreeSet<_>>() != selection
            || incoming
                .elements
                .iter()
                .any(|e| !selection.contains(&e.resource))
        {
            return Err(
                "Replacement must contain exactly the selected resources under the same schema"
                    .into(),
            );
        }
        let mut candidate = Self::retained(&current, &selection);
        candidate.elements.extend(incoming.elements);
        candidate.resources.extend(incoming.resources);
        self.publish_with_validation(
            current,
            &selection,
            candidate,
            validate_language,
            formats,
            None,
            &storage::InvocationContext::default(),
        )
    }
    /// Decode selected resource bytes with retained resources available for
    /// references. The codec cannot change unselected resources implicitly.
    pub fn import_with(
        &mut self,
        registry: &FormatRegistry,
        format: &str,
        selected: &[String],
        bytes: &[Resource],
        context: &ResourceContext,
        validate_language: impl FnOnce(&Model) -> StorageResult<()>,
    ) -> StorageResult<()> {
        self.import_with_format_context(
            registry,
            format,
            selected,
            bytes,
            context,
            &FormatContext::default(),
            validate_language,
        )
    }
    /// Decode and publish using the same explicitly borrowed lexical providers.
    #[allow(clippy::too_many_arguments)] // Existing resource selection/context plus explicit format adapters.
    pub fn import_with_format_context(
        &mut self,
        registry: &FormatRegistry,
        format: &str,
        selected: &[String],
        bytes: &[Resource],
        context: &ResourceContext,
        formats: &FormatContext<'_>,
        validate_language: impl FnOnce(&Model) -> StorageResult<()>,
    ) -> StorageResult<()> {
        self.check_context(context)?;
        let current = self.consistent_snapshot()?;
        let selection = Self::selection(&current, selected, false)?;
        let retained = Self::retained(&current, &selection);
        let decoded = registry.import_with_format_context_and_proof(
            format,
            self.schema(),
            bytes,
            Some(&retained),
            formats,
        )?;
        self.publish_with_validation(
            current,
            &selection,
            decoded.model,
            validate_language,
            formats,
            decoded.readonly.as_ref(),
            &storage::InvocationContext::default(),
        )
    }
    #[allow(clippy::too_many_arguments)] // Existing publication context plus opaque fresh readonly evidence.
    fn publish_with_validation(
        &mut self,
        current: Model,
        selection: &BTreeSet<String>,
        mut candidate: Model,
        validate_language: impl FnOnce(&Model) -> StorageResult<()>,
        formats: &FormatContext<'_>,
        readonly: Option<&storage::ReadonlyDecodeProof>,
        observation: &storage::InvocationContext<'_>,
    ) -> StorageResult<()> {
        observation.checkpoint()?;
        let retained = Self::retained(&current, selection);
        let candidate_retained: Vec<_> = candidate
            .elements
            .iter()
            .filter(|e| !selection.contains(&e.resource))
            .collect();
        if candidate_retained != retained.elements.iter().collect::<Vec<_>>()
            || candidate
                .resources
                .iter()
                .filter(|(uri, _)| !selection.contains(*uri))
                .collect::<BTreeMap<_, _>>()
                != retained.resources.iter().collect::<BTreeMap<_, _>>()
            || candidate.resources.keys().cloned().collect::<BTreeSet<_>>()
                != current
                    .resources
                    .keys()
                    .cloned()
                    .chain(selection.iter().cloned())
                    .collect::<BTreeSet<_>>()
        {
            return Err("Replacement changed resources outside its explicit selection".into());
        }
        if let Some(proof) = readonly {
            formats
                .structural_values
                .ok_or("Readonly decode proof requires current operation authority")?
                .check_decode_proof(&candidate, self.schema(), proof, selection)?;
            if formats.snapshot_initializers.is_none() {
                return Err("Readonly decode proof requires current initializer selection".into());
            }
        }
        if formats.snapshot_initializers.is_some() || readonly.is_some() {
            for object in candidate
                .elements
                .iter()
                .filter(|object| selection.contains(&object.resource))
            {
                for (name, feature) in self.schema().features(&object.class)? {
                    if !feature.changeable
                        && !feature.derived
                        && !feature.transient
                        && object
                            .values
                            .get(&name)
                            .is_some_and(|value| !value.is_null())
                        && !readonly.is_some_and(|proof| proof.covers(&object.id, &name))
                    {
                        return Err(
                            "Readonly replacement requires opaque checked fresh decode proof"
                                .into(),
                        );
                    }
                }
            }
        }
        for uri in selection {
            observation.work(1)?;
            let previous_revision = current
                .resources
                .get(uri)
                .map_or(0, |record| record.content_revision);
            let new = candidate
                .resources
                .get_mut(uri)
                .ok_or("Replacement omitted a selected resource")?;
            if let Some(package) = current
                .resources
                .get(uri)
                .and_then(|record| record.package.as_ref())
            {
                if new
                    .package
                    .as_ref()
                    .is_some_and(|incoming| incoming != package)
                {
                    return Err(format!(
                        "Replacement cannot change the package origin of {uri}"
                    ));
                }
                new.package = Some(package.clone());
            }
            new.content_revision = previous_revision
                .checked_add(1)
                .ok_or("Resource revision exhausted")?;
        }
        formats.validate(self.schema(), &candidate)?;
        validate_language(&candidate)?;
        observation.checkpoint()?;
        let change = ResourceChange {
            kind: ResourceChangeKind::Replaced,
            resources: selection.iter().cloned().collect(),
            invalidated: candidate.resources.keys().cloned().collect(),
        };
        let schema = self.schema().clone();
        storage::atomic(self.model.store_mut(), |store| {
            if storage::export(store)? != current {
                return Err("Stale resource replacement base".into());
            }
            // Explicit recovery may insert into an existing unavailable empty
            // resource. Enable raw writes only inside this checkpoint; failure
            // restores the old load state, diagnostics, roots and revision.
            for uri in selection {
                observation.work(1)?;
                let old = current.resources.get(uri);
                let new = candidate
                    .resources
                    .get(uri)
                    .ok_or("Replacement omitted a selected resource")?;
                if old.is_some_and(|old| old.load_state != ResourceLoadState::Loaded)
                    && new.load_state == ResourceLoadState::Loaded
                {
                    let mut writable = old.cloned().ok_or("Selected resource disappeared")?;
                    writable.load_state = ResourceLoadState::Loaded;
                    writable.unloaded = None;
                    store.set_resource_record(writable)?;
                }
            }
            for element in &current.elements {
                if selection.contains(&element.resource) {
                    observation.work(1)?;
                    store.remove(&element.id)?;
                }
            }
            for element in &candidate.elements {
                if selection.contains(&element.resource) {
                    observation.work(1)?;
                    storage::insert(store, &schema, element.clone())?;
                }
            }
            // Raw insert/remove may update resource bookkeeping; publish exactly
            // the validated candidate metadata after all graph writes.
            for record in candidate.resources.values() {
                observation.work(1)?;
                store.set_resource_record(record.clone())?;
            }
            formats.validate_store_observed(&schema, store, observation)?;
            if let Some(proof) = readonly {
                if storage::export(store)? != candidate {
                    return Err("Readonly decoded candidate changed during publication".into());
                }
                formats
                    .structural_values
                    .ok_or("Readonly decode proof requires current operation authority")?
                    .revalidate_decode_proof(
                        store,
                        &schema,
                        proof,
                        formats.snapshot_initializers.ok_or(
                            "Readonly decode proof requires current initializer selection",
                        )?,
                    )?;
            }
            observation.checkpoint()
        })?;
        self.fragments
            .retain(|(uri, _), _| !selection.contains(uri));
        self.record_change(change);
        Ok(())
    }
    /// Bind an explicitly supplied resource fragment to an existing global ID.
    /// Multiple candidates stay ambiguous; lookup never selects by insertion order.
    pub fn bind_fragment(
        &mut self,
        resource: &str,
        fragment: &str,
        element: &str,
    ) -> StorageResult<()> {
        if fragment.is_empty() || fragment.contains('#') {
            return Err("Invalid resource fragment".into());
        }
        let object = self
            .reader()
            .get(element)?
            .ok_or("Fragment target is unavailable")?;
        if object.resource != resource || self.reader().resource_record(resource)?.is_none() {
            return Err("Fragment target belongs to a different resource".into());
        }
        self.fragments
            .entry((resource.into(), fragment.into()))
            .or_default()
            .insert(element.into());
        Ok(())
    }
    pub fn resolve(&self, from: &str, href: &str) -> StorageResult<TargetResolution> {
        if self.reader().resource_record(from)?.is_none() {
            return Err("Unknown referring resource".into());
        }
        let (address, fragment) = href
            .split_once('#')
            .ok_or("A resource reference requires an explicit fragment mapping")?;
        let uri = resolve_resource_uri(from, address)?;
        let Some(record) = self.reader().resource_record(&uri)? else {
            return Ok(TargetResolution::MissingResource(uri));
        };
        if record.load_state != ResourceLoadState::Loaded {
            return Ok(TargetResolution::UnavailableResource {
                resource: uri,
                state: record.load_state,
            });
        }
        let mut candidates = Vec::new();
        if let Some(ids) = self.fragments.get(&(uri.clone(), fragment.into())) {
            for id in ids {
                if self.reader().get(id)?.is_some_and(|e| e.resource == uri) {
                    candidates.push(id.clone());
                }
            }
        }
        Ok(match candidates.len() {
            0 => TargetResolution::MissingFragment {
                resource: uri,
                fragment: fragment.into(),
            },
            1 => TargetResolution::Resolved {
                resource: uri,
                element: candidates.remove(0),
            },
            _ => TargetResolution::Ambiguous {
                resource: uri,
                fragment: fragment.into(),
                candidates,
            },
        })
    }
    fn fingerprint(model: &Model, uri: &str, context: &ResourceContext) -> StorageResult<String> {
        let record = model.resources.get(uri).ok_or("Unknown resource")?;
        let elements: Vec<&Element> = model
            .elements
            .iter()
            .filter(|e| e.resource == uri)
            .collect();
        digest(&(context, record, elements))
    }
    pub fn prepare_save(
        &mut self,
        registry: &FormatRegistry,
        format: &str,
        selected: &[String],
        closure: ReferenceClosure,
    ) -> StorageResult<PreparedSave> {
        self.prepare_save_with_format_context(
            registry,
            format,
            selected,
            closure,
            &FormatContext::default(),
        )
    }
    pub fn prepare_save_with_format_context(
        &mut self,
        registry: &FormatRegistry,
        format: &str,
        selected: &[String],
        closure: ReferenceClosure,
        formats: &FormatContext<'_>,
    ) -> StorageResult<PreparedSave> {
        let model = self.consistent_snapshot()?;
        Self::selection(&model, selected, true)?;
        let resources = registry.export_with_format_context(
            format,
            self.schema(),
            &model,
            Some((selected, closure)),
            formats,
        )?;
        let fingerprints = resources
            .iter()
            .map(|resource| {
                Ok((
                    resource.uri.clone(),
                    Self::fingerprint(&model, &resource.uri, &self.context)?,
                ))
            })
            .collect::<StorageResult<_>>()?;
        let artifacts = resources
            .iter()
            .map(|resource| Ok((resource.uri.clone(), digest(resource)?)))
            .collect::<StorageResult<BTreeMap<_, _>>>()?;
        let selection = artifacts.keys().cloned().collect();
        Ok(PreparedSave {
            resources,
            scope: self.scope.clone(),
            fingerprints,
            format: format.into(),
            selection,
            artifacts,
        })
    }
    /// Acknowledge only after the host successfully publishes every byte resource
    /// in the ticket. Partial publication must not acknowledge this batch.
    pub fn acknowledge_save(
        &mut self,
        registry: &FormatRegistry,
        ticket: &PreparedSave,
    ) -> StorageResult<()> {
        self.acknowledge_save_with_format_context(registry, ticket, &FormatContext::default())
    }
    pub fn acknowledge_save_with_format_context(
        &mut self,
        registry: &FormatRegistry,
        ticket: &PreparedSave,
        formats: &FormatContext<'_>,
    ) -> StorageResult<()> {
        if !Arc::ptr_eq(&ticket.scope, &self.scope) {
            return Err("Save ticket belongs to another resource set".into());
        }
        let model = self.consistent_snapshot()?;
        for (uri, expected) in &ticket.fingerprints {
            if &Self::fingerprint(&model, uri, &self.context)? != expected {
                return Err(format!("Stale saved resource revision: {uri}"));
            }
        }
        let encoded = registry.export_with_format_context(
            &ticket.format,
            self.schema(),
            &model,
            Some((&ticket.selection, ReferenceClosure::SelectedOnly)),
            formats,
        )?;
        let artifacts = encoded
            .iter()
            .map(|resource| Ok((resource.uri.clone(), digest(resource)?)))
            .collect::<StorageResult<BTreeMap<_, _>>>()?;
        if artifacts != ticket.artifacts {
            return Err("Stale saved reference or serialization context".into());
        }
        for (uri, fingerprint) in &ticket.fingerprints {
            let artifact = artifacts.get(uri).ok_or("Saved artifact disappeared")?;
            self.saved.insert(
                uri.clone(),
                SavedState {
                    fingerprint: fingerprint.clone(),
                    format: ticket.format.clone(),
                    selection: ticket.selection.clone(),
                    artifact: artifact.clone(),
                },
            );
        }
        Ok(())
    }
    pub fn is_dirty(&mut self, registry: &FormatRegistry, uri: &str) -> StorageResult<bool> {
        self.is_dirty_with_format_context(registry, uri, &FormatContext::default())
    }
    pub fn is_dirty_with_format_context(
        &mut self,
        registry: &FormatRegistry,
        uri: &str,
        formats: &FormatContext<'_>,
    ) -> StorageResult<bool> {
        let model = self.snapshot()?;
        let fingerprint = Self::fingerprint(&model, uri, &self.context)?;
        let Some(saved) = self.saved.get(uri) else {
            return Ok(true);
        };
        if saved.fingerprint != fingerprint {
            return Ok(true);
        }
        let encoded = registry.export_with_format_context(
            &saved.format,
            self.schema(),
            &model,
            Some((&saved.selection, ReferenceClosure::SelectedOnly)),
            formats,
        )?;
        let artifact = encoded
            .iter()
            .find(|resource| resource.uri == uri)
            .ok_or("Saved artifact disappeared")?;
        Ok(saved.artifact != digest(artifact)?)
    }
}

mod lifecycle;
pub use lifecycle::{PreparedReload, ResourceChange, ResourceChangeKind, UnloadPolicy};
