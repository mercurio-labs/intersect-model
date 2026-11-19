//! Experimental, opt-in mounting of closed, prepared model components.
//!
//! A component owns one immutable arena. Projects retain that arena through Arc;
//! only project records and reciprocal relationship overlays are writable.
//! This prototype supports one shared metamodel and uses the existing checked
//! mutation services. Mounted construction and snapshot restore are opt-in;
//! hosts and languages supply their own preparation context. Qualified strings
//! are adapter spellings; original component element identities remain available.

use super::{
    api,
    fingerprint::digest,
    frozen::{FrozenStore, Metamodel, Model as FrozenModel},
    project_usages::{DependencyLock, PackageIdentity},
    resources::ResourceRecord,
    storage::{
        self, Checkpoint, ModelRead, ModelStore, ObjectHandle, ObjectView, Query, QueryCandidates,
        ReferenceView, Row, SchemaDescriptor, StorageResult,
    },
};
use crate::definitions::schema::Schema;
use serde_json::Value;
use std::{
    cell::RefCell,
    collections::{BTreeMap, HashMap},
    sync::{Arc, OnceLock},
};

const PREFIX: &str = "urn:intersect:component:";
const ROW_BITS: u32 = 32;
const ROW_MASK: u64 = u32::MAX as u64;

mod draft;
pub use draft::MountedDraft;
#[cfg(test)]
mod validation_tests;

/// Explicitly shared metamodel, descriptors and checked-edit validation plan.
pub struct SharedSchema {
    pub(crate) metamodel: Arc<Metamodel>,
    pub(crate) descriptors: Arc<OnceLock<StorageResult<SchemaDescriptor>>>,
    pub(crate) digest: [u8; 32],
    validation: storage::DraftValidationPlan,
}

impl SharedSchema {
    pub fn new(schema: &Schema) -> StorageResult<Arc<Self>> {
        let metamodel = Arc::new(Metamodel::freeze(schema).map_err(|e| e.to_string())?);
        Self::from_metamodel(metamodel)
    }

    /// Retain an already-frozen language metamodel, including its admitted
    /// redefinition layout, rather than freezing a second approximation.
    pub fn from_metamodel(metamodel: Arc<Metamodel>) -> StorageResult<Arc<Self>> {
        let schema = metamodel.schema();
        let digest = metamodel.digest().map_err(|e| e.to_string())?;
        let descriptors = OnceLock::new();
        let _ = descriptors.set(SchemaDescriptor::new(schema));
        descriptors
            .get()
            .ok_or("Missing schema descriptors")?
            .as_ref()
            .map_err(Clone::clone)?;
        let validation = storage::DraftValidationPlan::for_metamodel(metamodel.clone())?;
        Ok(Arc::new(Self {
            metamodel,
            digest,
            descriptors: Arc::new(descriptors),
            validation,
        }))
    }

    pub fn schema(&self) -> &Schema {
        self.metamodel.schema()
    }

    pub fn metamodel(&self) -> Arc<Metamodel> {
        self.metamodel.clone()
    }

    pub fn descriptors(&self) -> StorageResult<&SchemaDescriptor> {
        self.descriptors
            .get()
            .ok_or("Missing schema descriptors")?
            .as_ref()
            .map_err(Clone::clone)
    }
}

/// Prepared closed graph. Its private arena cannot be obtained mutably.
pub struct ComponentImage {
    context: Arc<SharedSchema>,
    uri: String,
    identity: PackageIdentity,
    key: String,
    arena: FrozenStore,
    objects: BTreeMap<String, Arc<str>>,
    qualified: BTreeMap<Arc<str>, String>,
    resources: BTreeMap<String, String>,
    topology: draft::ComponentTopology,
}

/// Durable ownership of an object, separate from its mounted runtime handle.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ComponentTarget<'a> {
    pub component_uri: &'a str,
    pub identity: &'a PackageIdentity,
    pub element_id: &'a str,
}

/// Local prepared-cache descriptor. It carries no executable authority; restore
/// checks the exact runtime, schema, graph digest and bounded snapshot encoding.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ComponentDescriptor {
    version: u32,
    uri: String,
    identity: PackageIdentity,
    key: String,
}

impl ComponentImage {
    /// Consume a linked, validated closed model; preparation hashes its data once.
    /// The image retains the original arena, not a materialized duplicate.
    pub fn prepare(
        context: Arc<SharedSchema>,
        uri: &str,
        model: FrozenModel,
    ) -> StorageResult<Arc<Self>> {
        Self::prepare_inner(context, uri, model, false).map(|(image, _)| image)
    }

    /// Encode the already-resolved model during explicit preparation. Callers
    /// persist these bytes and the descriptor; images retain no payload copy.
    pub fn prepare_with_snapshot(
        context: Arc<SharedSchema>,
        uri: &str,
        model: FrozenModel,
    ) -> StorageResult<(Arc<Self>, Vec<u8>)> {
        let (image, snapshot) = Self::prepare_inner(context, uri, model, true)?;
        Ok((
            image,
            snapshot.ok_or("Missing prepared component snapshot")?,
        ))
    }

    fn prepare_inner(
        context: Arc<SharedSchema>,
        uri: &str,
        model: FrozenModel,
        save: bool,
    ) -> StorageResult<(Arc<Self>, Option<Vec<u8>>)> {
        crate::core::resources::validate_resource_uri(uri)?;
        let materialized = model.to_model().map_err(|e| e.to_string())?;
        if materialized
            .resources
            .values()
            .any(|record| record.package.is_some())
        {
            return Err(
                "Closed component preparation requires source objects without prior package tags"
                    .into(),
            );
        }
        let content_digest = digest(&materialized)?;
        drop(materialized);
        let identity = PackageIdentity {
            content_digest,
            dependency_lock_digest: DependencyLock::empty().digest()?,
            engine: crate::SOURCE_IDENTITY.into(),
            schema: hex(&context.digest),
        };
        let key = digest(&(uri, &identity))?;
        // Frozen builders need not retain explicit resource records. The
        // canonical immutable-model adapter derives roots and attaches package
        // provenance before the arena becomes a shared component.
        let model = model
            .with_package_identity(&identity)
            .map_err(|e| e.to_string())?;
        let snapshot = if save {
            Some(
                model
                    .to_snapshot(
                        &crate::definitions::library_part::key_bytes(&key)
                            .map_err(|e| e.to_string())?,
                    )
                    .map_err(|e| e.to_string())?,
            )
        } else {
            None
        };
        Ok((
            Self::from_prepared(context, uri, identity, key, model, true)?,
            snapshot,
        ))
    }

    pub fn descriptor(&self) -> ComponentDescriptor {
        ComponentDescriptor {
            version: 1,
            uri: self.uri.clone(),
            identity: self.identity.clone(),
            key: self.key.clone(),
        }
    }

    /// Restore a locally prepared closed component without semantic linking.
    /// A host must independently check its language/provider cache context.
    pub fn restore(
        context: Arc<SharedSchema>,
        descriptor: &ComponentDescriptor,
        snapshot: &[u8],
    ) -> StorageResult<Arc<Self>> {
        Self::restore_with_trust(
            context,
            descriptor,
            snapshot,
            super::frozen::SnapshotTrust::Revalidate,
        )
    }

    /// Restore the host's own prepared component cache. `HostCache` trusts the
    /// original canonical admission proof; it checks snapshot digests and all
    /// decoding bounds. `VerifiedHostCache` additionally requires the host to
    /// have verified the exact enclosing payload's digest before calling here.
    /// Neither mode is for downloaded model interchange or untrusted KPARs.
    /// Runtime/schema/descriptor keys and resource ownership are always checked.
    /// `Revalidate` retains restore()'s full graph digest and canonical audit.
    pub fn restore_with_trust(
        context: Arc<SharedSchema>,
        descriptor: &ComponentDescriptor,
        snapshot: &[u8],
        trust: super::frozen::SnapshotTrust,
    ) -> StorageResult<Arc<Self>> {
        crate::core::resources::validate_resource_uri(&descriptor.uri)?;
        if descriptor.version != 1
            || descriptor.identity.engine != crate::SOURCE_IDENTITY
            || descriptor.identity.schema != hex(&context.digest)
            || descriptor.identity.dependency_lock_digest != DependencyLock::empty().digest()?
        {
            return Err(
                "Prepared component runtime, format, schema or dependency context is incompatible"
                    .into(),
            );
        }
        if descriptor.key != digest(&(&descriptor.uri, &descriptor.identity))? {
            return Err("Prepared component key differs from its exact identity".into());
        }
        let model = FrozenModel::from_snapshot(
            context.metamodel(),
            snapshot,
            &crate::definitions::library_part::key_bytes(&descriptor.key)
                .map_err(|e| e.to_string())?,
            trust,
        )
        .map_err(|e| e.to_string())?;
        for record in model
            .resource_records()
            .map_err(|e| e.to_string())?
            .values()
        {
            if record.package.as_ref() != Some(&descriptor.identity) {
                return Err("Prepared component resource identity mismatch".into());
            }
        }
        let revalidate = trust == super::frozen::SnapshotTrust::Revalidate;
        if revalidate {
            let mut content = model.to_model().map_err(|e| e.to_string())?;
            for record in content.resources.values_mut() {
                record.package = None;
            }
            if digest(&content)? != descriptor.identity.content_digest {
                return Err("Prepared component graph digest mismatch".into());
            }
        }
        Self::from_prepared(
            context,
            &descriptor.uri,
            descriptor.identity.clone(),
            descriptor.key.clone(),
            model,
            revalidate,
        )
    }

    fn from_prepared(
        context: Arc<SharedSchema>,
        uri: &str,
        identity: PackageIdentity,
        key: String,
        model: FrozenModel,
        revalidate: bool,
    ) -> StorageResult<Arc<Self>> {
        let topology = draft::ComponentTopology::new(&model, &key)?;
        let objects = model
            .objects()
            .zip(&topology.ids)
            .map(|(object, id)| (object.identity().to_owned(), id.clone()))
            .collect::<BTreeMap<_, _>>();
        let mut arena = FrozenStore::from_model(model);
        arena.share_schema(&context)?;
        // Native frozen models carry their schema in the metamodel, not the
        // draft-store revision field. Bind that field to the same checked
        // metamodel before using the canonical store validator.
        storage::atomic(&mut arena, |store| {
            store.set_revision(&context.schema().revision)
        })?;
        // Establish the canonical structural invariant once, over the closed
        // native arena. Qualification is a bijective renaming; private images
        // cannot acquire new objects, resources or assignments after this point.
        if revalidate {
            context.validation.validate_all(
                &NativeComponentRead(&arena),
                context.schema(),
                true,
            )?;
        }
        for (_, id) in arena.id_page(None, usize::MAX)? {
            arena.inspect_component_native(&id, &mut |object| {
                // Retained receipts name their native owner. The existing
                // mounted view does not rebase that evidence, so its full
                // audit would reject it after identity qualification.
                if object.initialization().is_some() {
                    return Err(
                        "Component images do not yet support object-bound initialization receipts"
                            .into(),
                    );
                }
                Ok(())
            })?;
        }
        let qualified = objects
            .iter()
            .map(|(local, qualified)| (qualified.clone(), local.clone()))
            .collect();
        let resources = arena
            .resource_records()?
            .into_keys()
            .map(|uri| {
                let qualified = qualify(&key, "resource", &uri);
                (uri, qualified)
            })
            .collect();
        Ok(Arc::new(Self {
            context,
            uri: uri.into(),
            identity,
            key,
            arena,
            objects,
            qualified,
            resources,
            topology,
        }))
    }

    pub fn identity(&self) -> &PackageIdentity {
        &self.identity
    }
    /// Exact prepared image identity, independent of acquisition location.
    pub fn key(&self) -> &str {
        &self.key
    }
    pub fn uri(&self) -> &str {
        &self.uri
    }
    pub fn object_count(&self) -> usize {
        self.objects.len()
    }

    /// Get the explicit qualified adapter ID without changing the external ID.
    pub fn object_id(&self, element_id: &str) -> StorageResult<&str> {
        self.objects
            .get(element_id)
            .map(AsRef::as_ref)
            .ok_or_else(|| format!("Unknown component object: {element_id}"))
    }
}

fn qualify(key: &str, kind: &str, local: &str) -> String {
    format!("{PREFIX}{key}:{kind}:{}", hex(local.as_bytes()))
}

fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write;
    let mut id = String::with_capacity(bytes.len().saturating_mul(2));
    for byte in bytes {
        let _ = write!(id, "{byte:02x}");
    }
    id
}

#[derive(Clone, Copy)]
pub(crate) struct ReadContext<'a> {
    store: &'a MountedStore,
    image: Option<&'a ComponentImage>,
    overlays: bool,
}

impl<'a> ReadContext<'a> {
    pub(crate) fn identity(self, local: &'a str) -> &'a str {
        self.image
            .and_then(|image| image.objects.get(local))
            .map_or(local, AsRef::as_ref)
    }
    pub(crate) fn resource(self, local: &'a str) -> &'a str {
        self.image
            .and_then(|image| image.resources.get(local))
            .map_or(local, String::as_str)
    }
    pub(crate) fn overlay(self, id: &str, feature: &str) -> Option<&'a Value> {
        if !self.overlays {
            return None;
        }
        self.store.overlays.get(id)?.get(feature)
    }
    pub(crate) fn reference(self, identity: &'a str) -> ReferenceView<'a> {
        self.store.handle(identity).map_or(
            ReferenceView::Unavailable { identity },
            ReferenceView::Resolved,
        )
    }
}

struct HandleEntry {
    raw: ObjectHandle,
    mounted: ObjectHandle,
}
type Overlays = BTreeMap<String, BTreeMap<String, Value>>;
struct Frame {
    token: Checkpoint,
    overlays: Overlays,
}

// The raw ModelStore is private: only checked project edits can cause reciprocal
// writes to library overlays. No API exposes raw inverse-write authority.
struct MountedStore {
    context: Arc<SharedSchema>,
    project: FrozenStore,
    images: Vec<Arc<ComponentImage>>,
    overlays: Overlays,
    frames: Vec<Frame>,
    handles: RefCell<HashMap<String, HandleEntry>>,
}

impl MountedStore {
    fn validate_project(&self, require_lower_bounds: bool) -> StorageResult<()> {
        // A project may contain a library object when no opposite prevents it.
        // That changes global ownership/root membership, so retain the complete
        // canonical audit for this uncommon case.
        let mut external_containment = false;
        for (_, id) in self.project.id_page(None, usize::MAX)? {
            self.inspect(&id, &mut |object| {
                for (name, field) in self.context.schema().features(object.class())? {
                    if !field.containment {
                        continue;
                    }
                    if let Some(value) = object.value(&name) {
                        let value = value.to_json();
                        let values = value
                            .as_array()
                            .map(Vec::as_slice)
                            .unwrap_or(std::slice::from_ref(&value));
                        external_containment |= values
                            .iter()
                            .any(|v| v.as_str().is_some_and(|id| id.starts_with(PREFIX)));
                    }
                }
                Ok(())
            })?;
        }
        let schema = self.context.schema();
        if external_containment {
            return self
                .context
                .validation
                .validate_all(self, schema, require_lower_bounds);
        }
        // Images are closed and already checked. Only project resources and
        // project containment can have changed. Target and opposite queries
        // still see the complete mounted graph, including inverse overlays.
        self.context
            .validation
            .validate_all(&ProjectRead(self), schema, require_lower_bounds)?;
        // Overlays are restricted to non-containment reciprocal references and
        // cannot remove base relationships. Recheck every affected object with
        // the canonical validator (upper bounds, uniqueness, types, opposites).
        let affected = self.overlays.keys().map(String::as_str).collect();
        self.context
            .validation
            .validate_objects(self, schema, &affected)
    }

    fn locate(&self, id: &str) -> Option<(&ComponentImage, &str)> {
        self.images.iter().find_map(|image| {
            image
                .qualified
                .get(id)
                .map(|local| (image.as_ref(), local.as_str()))
        })
    }

    fn raw_handle(&self, id: &str) -> Option<ObjectHandle> {
        match self.locate(id) {
            Some((image, local)) => image.arena.handle(local),
            None => self.project.handle(id),
        }
    }

    fn handle(&self, id: &str) -> Option<ObjectHandle> {
        let raw = self.raw_handle(id)?;
        let mut handles = self.handles.borrow_mut();
        if let Some(entry) = handles.get(id)
            && entry.raw == raw
        {
            return Some(entry.mounted.clone());
        }
        let mounted = ObjectHandle {
            scope: Arc::new(()),
            row: raw.row,
        };
        handles.insert(
            id.into(),
            HandleEntry {
                raw,
                mounted: mounted.clone(),
            },
        );
        Some(mounted)
    }

    fn inspect_handle(
        &self,
        handle: &ObjectHandle,
        f: &mut dyn FnMut(ObjectView<'_>) -> StorageResult<()>,
    ) -> StorageResult<()> {
        let id = self
            .handles
            .borrow()
            .iter()
            .find(|(_, entry)| entry.mounted == *handle)
            .map(|(id, _)| id.clone())
            .ok_or("Foreign or stale mounted handle")?;
        if self.handle(&id).as_ref() != Some(handle) {
            return Err("Stale mounted handle".into());
        }
        if !self.inspect(&id, f)? {
            return Err("Stale mounted handle".into());
        }
        Ok(())
    }

    fn author_project(&self, id: &str) -> StorageResult<()> {
        if id.starts_with(PREFIX) {
            return Err("Component objects are read-only".into());
        }
        Ok(())
    }

    fn base_value(
        &self,
        image: &ComponentImage,
        local: &str,
        field: &str,
    ) -> StorageResult<Option<Value>> {
        let mut value = None;
        image.arena.inspect_mounted(
            local,
            ReadContext {
                store: self,
                image: Some(image),
                overlays: false,
            },
            &mut |object| {
                value = object.value(field).map(|v| v.to_json());
                Ok(())
            },
        )?;
        Ok(value)
    }

    fn write_overlay(&mut self, id: &str, field: &str, value: Option<Value>) -> StorageResult<()> {
        if self.frames.is_empty() {
            return Err("Storage write requires a transaction".into());
        }
        let (image, local) = self.locate(id).ok_or("Missing mounted object")?;
        let class = image
            .arena
            .class(local)?
            .ok_or("Missing component object")?;
        let fields = self.context.schema().features(&class)?;
        let feature = fields.get(field).ok_or("Unknown component feature")?;
        if !feature.reference
            || feature.opposite.is_none()
            || feature.containment
            || feature.unsettable
            || self.context.schema().is_container_feature(feature)?
        {
            return Err("This component feature cannot receive an inverse overlay".into());
        }
        let base = self.base_value(image, local, field)?.unwrap_or(Value::Null);
        let value = value.unwrap_or(Value::Null);
        let ids = |v: &Value| -> StorageResult<Vec<String>> {
            match v {
                Value::Null => Ok(vec![]),
                Value::String(s) => Ok(vec![s.clone()]),
                Value::Array(a) => a
                    .iter()
                    .map(|v| {
                        v.as_str()
                            .map(str::to_owned)
                            .ok_or_else(|| "Invalid inverse reference".into())
                    })
                    .collect(),
                _ => Err("Invalid inverse reference".into()),
            }
        };
        let base_ids = ids(&base)?;
        let effective = ids(&value)?;
        if base_ids.iter().any(|id| !effective.contains(id)) {
            return Err("Cannot displace an authored component relationship".into());
        }
        for target in effective.iter().filter(|id| !base_ids.contains(id)) {
            self.author_project(target)?;
            if self.project.class(target)?.is_none() {
                return Err("Inverse overlay requires a project object".into());
            }
        }
        if value == base {
            if let Some(fields) = self.overlays.get_mut(id) {
                fields.remove(field);
                if fields.is_empty() {
                    self.overlays.remove(id);
                }
            }
        } else {
            self.overlays
                .entry(id.into())
                .or_default()
                .insert(field.into(), value);
        }
        Ok(())
    }
}

// Validation traversal is confined to the mutable partition; target inspection
// is deliberately not confined. This adapter has no mutation capability.
struct NativeComponentRead<'a>(&'a FrozenStore);
impl ModelRead for NativeComponentRead<'_> {
    fn revision(&self) -> StorageResult<String> {
        self.0.revision()
    }
    fn resource_records(&self) -> StorageResult<BTreeMap<String, ResourceRecord>> {
        self.0.resource_records()
    }
    fn inspect(
        &self,
        id: &str,
        f: &mut dyn FnMut(ObjectView<'_>) -> StorageResult<()>,
    ) -> StorageResult<bool> {
        self.0.inspect_component_native(id, f)
    }
    fn get(&self, id: &str) -> StorageResult<Option<super::model::Element>> {
        self.0.get(id)
    }
    fn class(&self, id: &str) -> StorageResult<Option<String>> {
        self.0.class(id)
    }
    fn value(&self, id: &str, field: &str) -> StorageResult<Option<Value>> {
        self.0.value(id, field)
    }
    fn id_page(&self, after: Option<u64>, limit: usize) -> StorageResult<Vec<(u64, String)>> {
        self.0.id_page(after, limit)
    }
    fn page(&self, after: Option<u64>, limit: usize) -> StorageResult<Vec<Row>> {
        self.0.page(after, limit)
    }
    fn query(&self, query: Query<'_>) -> StorageResult<Vec<String>> {
        self.0.query(query)
    }
    fn matches(&self, id: &str, query: Query<'_>) -> StorageResult<bool> {
        self.0.matches(id, query)
    }
}

struct ProjectRead<'a>(&'a MountedStore);
impl ModelRead for ProjectRead<'_> {
    fn revision(&self) -> StorageResult<String> {
        self.0.revision()
    }
    fn resource_records(&self) -> StorageResult<BTreeMap<String, ResourceRecord>> {
        self.0.project.resource_records()
    }
    fn inspect(
        &self,
        id: &str,
        f: &mut dyn FnMut(ObjectView<'_>) -> StorageResult<()>,
    ) -> StorageResult<bool> {
        self.0.inspect(id, f)
    }
    fn get(&self, id: &str) -> StorageResult<Option<super::model::Element>> {
        self.0.get(id)
    }
    fn class(&self, id: &str) -> StorageResult<Option<String>> {
        self.0.class(id)
    }
    fn value(&self, id: &str, field: &str) -> StorageResult<Option<Value>> {
        self.0.value(id, field)
    }
    fn id_page(&self, after: Option<u64>, limit: usize) -> StorageResult<Vec<(u64, String)>> {
        self.0.project.id_page(after, limit)
    }
    fn page(&self, after: Option<u64>, limit: usize) -> StorageResult<Vec<Row>> {
        self.0.project.page(after, limit)
    }
    fn query(&self, query: Query<'_>) -> StorageResult<Vec<String>> {
        self.0.query(query)
    }
    fn matches(&self, id: &str, query: Query<'_>) -> StorageResult<bool> {
        self.0.matches(id, query)
    }
}

impl ModelRead for MountedStore {
    fn revision(&self) -> StorageResult<String> {
        self.project.revision()
    }

    fn resource_records(&self) -> StorageResult<BTreeMap<String, ResourceRecord>> {
        let mut records = self.project.resource_records()?;
        for image in &self.images {
            for (local, mut record) in image.arena.resource_records()? {
                record.uri = image
                    .resources
                    .get(&local)
                    .ok_or("Missing component resource identity")?
                    .clone();
                record.roots = Arc::new(
                    record
                        .roots
                        .iter()
                        .map(|root| image.object_id(root).map(str::to_owned))
                        .collect::<StorageResult<_>>()?,
                );
                record.package = Some(image.identity.clone());
                records.insert(record.uri.clone(), record);
            }
        }
        Ok(records)
    }

    fn inspect(
        &self,
        id: &str,
        f: &mut dyn FnMut(ObjectView<'_>) -> StorageResult<()>,
    ) -> StorageResult<bool> {
        if let Some((image, local)) = self.locate(id) {
            image.arena.inspect_mounted(
                local,
                ReadContext {
                    store: self,
                    image: Some(image),
                    overlays: true,
                },
                f,
            )
        } else {
            self.project.inspect_mounted(
                id,
                ReadContext {
                    store: self,
                    image: None,
                    overlays: true,
                },
                f,
            )
        }
    }

    fn get(&self, id: &str) -> StorageResult<Option<super::model::Element>> {
        let mut result = None;
        self.inspect(id, &mut |object| {
            result = Some(object.to_element());
            Ok(())
        })?;
        Ok(result)
    }

    fn class(&self, id: &str) -> StorageResult<Option<String>> {
        match self.locate(id) {
            Some((image, local)) => image.arena.class(local),
            None => self.project.class(id),
        }
    }

    fn value(&self, id: &str, field: &str) -> StorageResult<Option<Value>> {
        let mut result = None;
        self.inspect(id, &mut |object| {
            result = object.value(field).map(|v| v.to_json());
            Ok(())
        })?;
        Ok(result)
    }

    fn id_page(&self, after: Option<u64>, limit: usize) -> StorageResult<Vec<(u64, String)>> {
        let mut rows = Vec::new();
        for index in 0..=self.images.len() {
            let group = u64::try_from(index).map_err(|_| "Too many component mounts")?;
            if group > ROW_MASK {
                return Err("Too many component mounts".into());
            }
            let base = group << ROW_BITS;
            if after.is_some_and(|after| after >> ROW_BITS > group) {
                continue;
            }
            let cursor = after
                .filter(|after| after >> ROW_BITS == group)
                .map(|after| after & ROW_MASK);
            let arena = if index == 0 {
                &self.project
            } else {
                &self.images[index - 1].arena
            };
            for (row, id) in arena.id_page(cursor, limit.saturating_sub(rows.len()))? {
                if row > ROW_MASK {
                    return Err("Component proof supports at most 32-bit arena rows".into());
                }
                let id = if index == 0 {
                    id
                } else {
                    self.images[index - 1].object_id(&id)?.into()
                };
                rows.push((base | row, id));
            }
            if rows.len() == limit {
                break;
            }
        }
        Ok(rows)
    }

    fn page(&self, after: Option<u64>, limit: usize) -> StorageResult<Vec<Row>> {
        self.id_page(after, limit)?
            .into_iter()
            .map(|(row, id)| Ok((row, self.get(&id)?.ok_or("Missing mounted row")?)))
            .collect()
    }

    fn matches(&self, id: &str, query: Query<'_>) -> StorageResult<bool> {
        let mut result = false;
        self.inspect(id, &mut |object| {
            result = match query {
                Query::Class(class) => object.class() == class,
                Query::Resource(uri) => object.resource() == uri,
                Query::Value(field, target) => object.value(field).is_some_and(|value| {
                    value.collection().map_or_else(
                        || value.equals_json(target),
                        |items| items.iter().any(|value| value.equals_json(target)),
                    )
                }),
            };
            Ok(())
        })?;
        Ok(result)
    }

    fn query(&self, query: Query<'_>) -> StorageResult<Vec<String>> {
        let mut result = Vec::new();
        let mut after = None;
        loop {
            let rows = self.id_page(after, 128)?;
            if rows.is_empty() {
                break;
            }
            for (row, id) in rows {
                after = Some(row);
                if self.matches(&id, query)? {
                    result.push(id);
                }
            }
        }
        Ok(result)
    }

    fn query_candidates(
        &self,
        _query: Query<'_>,
        after: Option<u64>,
        limit: usize,
    ) -> StorageResult<QueryCandidates> {
        if limit == 0 {
            return Err("Candidate limit must be positive".into());
        }
        let mut rows = self.id_page(after, limit.saturating_add(1))?;
        let exhausted = rows.len() <= limit;
        rows.truncate(limit);
        Ok(QueryCandidates {
            after: rows.last().map(|(row, _)| *row).or(after),
            rows,
            exhausted,
            exact: false,
        })
    }
}

impl ModelStore for MountedStore {
    fn is_transaction_active(&self) -> bool {
        self.project.is_transaction_active()
    }
    fn begin(&mut self) -> StorageResult<Checkpoint> {
        let token = self.project.begin()?;
        self.frames.push(Frame {
            token: token.clone(),
            overlays: self.overlays.clone(),
        });
        Ok(token)
    }
    fn commit(&mut self, token: &Checkpoint) -> StorageResult<()> {
        if self.frames.last().is_none_or(|frame| frame.token != *token) {
            return Err("Invalid mounted checkpoint".into());
        }
        self.project.commit(token)?;
        self.frames.pop();
        Ok(())
    }
    fn rollback(&mut self, token: &Checkpoint) -> StorageResult<()> {
        if self.frames.last().is_none_or(|frame| frame.token != *token) {
            return Err("Invalid mounted checkpoint".into());
        }
        self.project.rollback(token)?;
        let frame = self.frames.pop().ok_or("Missing mounted checkpoint")?;
        self.overlays = frame.overlays;
        // Do not retain cache entries for tentative incarnations after rollback.
        self.handles
            .borrow_mut()
            .retain(|id, entry| self.raw_handle(id).as_ref() == Some(&entry.raw));
        Ok(())
    }
    fn set_revision(&mut self, revision: &str) -> StorageResult<()> {
        self.project.set_revision(revision)
    }
    fn set_resource_record(&mut self, record: ResourceRecord) -> StorageResult<()> {
        self.author_project(&record.uri)?;
        self.project.set_resource_record(record)
    }
    fn remove_resource_record(&mut self, uri: &str) -> StorageResult<()> {
        self.author_project(uri)?;
        self.project.remove_resource_record(uri)
    }
    fn put(&mut self, element: super::model::Element) -> StorageResult<()> {
        self.author_project(&element.id)?;
        self.author_project(&element.resource)?;
        self.project.put(element)
    }
    fn remove(&mut self, id: &str) -> StorageResult<()> {
        self.author_project(id)?;
        self.project.remove(id)
    }
    fn set_value(&mut self, id: &str, field: &str, value: Option<Value>) -> StorageResult<()> {
        self.author_project(id)?;
        self.project.set_value(id, field, value)
    }
    fn set_resolved_value(
        &mut self,
        id: &str,
        field: &str,
        value: Option<Value>,
    ) -> StorageResult<()> {
        if id.starts_with(PREFIX) {
            self.write_overlay(id, field, value)
        } else {
            self.project.set_resolved_value(id, field, value)
        }
    }
}

/// Checked project facade over shared immutable components. Implements ModelRead
/// so ordinary reflective queries and borrowed inspection use the mounted view.
pub struct MountedModel {
    store: MountedStore,
}

impl MountedModel {
    pub fn new(
        context: Arc<SharedSchema>,
        images: Vec<Arc<ComponentImage>>,
    ) -> StorageResult<Self> {
        let mut seen = std::collections::BTreeSet::new();
        let mut uris = std::collections::BTreeSet::new();
        for image in &images {
            if !Arc::ptr_eq(&context, &image.context) {
                return Err("Components must use this shared schema context".into());
            }
            if !seen.insert(image.key.clone()) || !uris.insert(image.uri.clone()) {
                return Err("Duplicate or conflicting component mount".into());
            }
        }
        let mut project = FrozenStore::with_index_policy(
            context.metamodel.clone(),
            storage::IndexPolicy::default(),
        );
        project.share_schema(&context)?;
        storage::atomic(&mut project, |store| {
            store.set_revision(&context.schema().revision)
        })?;
        let result = Self {
            store: MountedStore {
                context,
                project,
                images,
                overlays: BTreeMap::new(),
                frames: Vec::new(),
                handles: RefCell::new(HashMap::new()),
            },
        };
        // Each private, closed image has passed canonical validation at
        // preparation/restore. Disjoint qualified identities preserve that
        // invariant; the new project arena has no objects or resources yet.
        Ok(result)
    }

    pub fn schema(&self) -> &Schema {
        self.store.context.schema()
    }
    pub fn shared_schema(&self) -> &Arc<SharedSchema> {
        &self.store.context
    }
    pub fn handle(&self, id: &str) -> Option<ObjectHandle> {
        self.store.handle(id)
    }
    pub fn inspect_handle(
        &self,
        handle: &ObjectHandle,
        f: &mut dyn FnMut(ObjectView<'_>) -> StorageResult<()>,
    ) -> StorageResult<()> {
        self.store.inspect_handle(handle, f)
    }
    pub fn component_target(&self, id: &str) -> Option<ComponentTarget<'_>> {
        let (image, local) = self.store.locate(id)?;
        Some(ComponentTarget {
            component_uri: &image.uri,
            identity: &image.identity,
            element_id: local,
        })
    }
    pub fn shares_component_with(&self, other: &Self, uri: &str) -> bool {
        self.store
            .images
            .iter()
            .filter(|image| image.uri == uri)
            .any(|image| {
                other
                    .store
                    .images
                    .iter()
                    .any(|other| Arc::ptr_eq(image, other))
            })
    }
    pub fn project_object_count(&self) -> StorageResult<usize> {
        Ok(self.store.project.id_page(None, usize::MAX)?.len())
    }
    pub fn validate(&self) -> StorageResult<()> {
        self.store.validate_project(true)
    }

    pub fn create(&mut self, id: &str, class: &str, resource: &str) -> StorageResult<()> {
        self.transaction(|edit| edit.create(id, class, resource))
    }
    pub fn set(&mut self, id: &str, feature: &str, value: Value) -> StorageResult<()> {
        self.transaction(|edit| edit.set(id, feature, value))
    }
    pub fn set_many(&mut self, edits: &[(&str, &str, Value)]) -> StorageResult<()> {
        self.transaction(|edit| edit.set_many(edits))
    }
    pub fn transaction<T>(
        &mut self,
        f: impl FnOnce(&mut MountedEdit<'_>) -> StorageResult<T>,
    ) -> StorageResult<T> {
        storage::atomic(&mut self.store, |store| f(&mut MountedEdit { store }))
    }
    /// Copy only project state and overlays. Library images and schema stay shared.
    /// Handles from the original mounted view are not valid in the fork.
    pub fn fork(&self) -> StorageResult<Self> {
        if self.store.is_transaction_active() {
            return Err("Cannot fork an active mounted transaction".into());
        }
        Ok(Self {
            store: MountedStore {
                context: self.store.context.clone(),
                project: self.store.project.fork_arena()?,
                images: self.store.images.clone(),
                overlays: self.store.overlays.clone(),
                frames: vec![],
                handles: RefCell::new(HashMap::new()),
            },
        })
    }
}

/// Transaction-scoped project mutation through the canonical checked services.
pub struct MountedEdit<'a> {
    store: &'a mut MountedStore,
}
impl MountedEdit<'_> {
    /// Handles issued for tentative objects become stale if the edit rolls back.
    pub fn handle(&self, id: &str) -> Option<ObjectHandle> {
        self.store.handle(id)
    }
    pub fn create(&mut self, id: &str, class: &str, resource: &str) -> StorageResult<()> {
        self.store.author_project(id)?;
        self.store.author_project(resource)?;
        let context = self.store.context.clone();
        api::create_context(
            self.store,
            context.schema(),
            &context.validation,
            id,
            class,
            resource,
            None,
        )
    }
    pub fn set(&mut self, id: &str, feature: &str, value: Value) -> StorageResult<()> {
        self.set_many(&[(id, feature, value)])
    }
    pub fn set_many(&mut self, edits: &[(&str, &str, Value)]) -> StorageResult<()> {
        for (id, _, _) in edits {
            self.store.author_project(id)?;
        }
        let context = self.store.context.clone();
        api::set_many_context(
            self.store,
            context.schema(),
            &context.validation,
            edits,
            None,
        )
    }
}

impl ModelRead for MountedModel {
    fn revision(&self) -> StorageResult<String> {
        self.store.revision()
    }
    fn resource_records(&self) -> StorageResult<BTreeMap<String, ResourceRecord>> {
        self.store.resource_records()
    }
    fn get(&self, id: &str) -> StorageResult<Option<super::model::Element>> {
        self.store.get(id)
    }
    fn class(&self, id: &str) -> StorageResult<Option<String>> {
        self.store.class(id)
    }
    fn page(&self, after: Option<u64>, limit: usize) -> StorageResult<Vec<Row>> {
        self.store.page(after, limit)
    }
    fn id_page(&self, after: Option<u64>, limit: usize) -> StorageResult<Vec<(u64, String)>> {
        self.store.id_page(after, limit)
    }
    fn query(&self, query: Query<'_>) -> StorageResult<Vec<String>> {
        self.store.query(query)
    }
    fn query_candidates(
        &self,
        query: Query<'_>,
        after: Option<u64>,
        limit: usize,
    ) -> StorageResult<QueryCandidates> {
        self.store.query_candidates(query, after, limit)
    }
    fn matches(&self, id: &str, query: Query<'_>) -> StorageResult<bool> {
        self.store.matches(id, query)
    }
    fn value(&self, id: &str, field: &str) -> StorageResult<Option<Value>> {
        self.store.value(id, field)
    }
    fn inspect(
        &self,
        id: &str,
        f: &mut dyn FnMut(ObjectView<'_>) -> StorageResult<()>,
    ) -> StorageResult<bool> {
        self.store.inspect(id, f)
    }
}
