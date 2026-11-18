//! `ModelStore` over a frozen-engine builder, so the existing parser can
//! construct into the arena directly.
//!
//! Writes follow the shared element-level contract: `put` replaces an
//! object's stored values and keeps resource records consistent through the
//! shared `element_delta`. Nested transactions journal each object the first
//! time a frame touches it; rollback restores those snapshots and drops
//! objects created after the checkpoint. Unresolved references use the
//! store wire convention `?spelling`; a reference to an identity that does
//! not exist yet is kept as an identity proxy and resolved by
//! `into_builder`.
//!
//! Removal and class replacement leave arena positions stable until
//! construction finishes.
use super::{
    FrozenError,
    builder::{Layer, ModelBuilder, encode_value, new_list},
    intern::FxHashMap,
    metamodel::{FeatureKind, Metamodel},
    model::{Carried, Header, ModelData, ObjId, PreparedFacts, Proxy, Slot},
};
use crate::core::{
    model::Element,
    resources::ResourceRecord,
    storage::{
        self, Checkpoint, CheckpointIssuer, IndexPolicy, InvocationContext, ModelRead, ModelStore,
        ObjectHandle, Query, QueryCandidates, Row, SchemaDescriptor, StorageResult,
    },
};
use serde_json::Value;
use std::{
    collections::{BTreeMap, BTreeSet},
    ops::Bound::{Excluded, Unbounded},
    sync::{Arc, OnceLock},
};

#[path = "store_index.rs"]
mod index;
use index::{Indexes, value_members};
#[path = "store_views.rs"]
mod views;
pub(in crate::core) use views::{FrozenAtom, FrozenItems, FrozenObject, FrozenValue};
#[path = "store_append.rs"]
mod append;
use append::AppendList;
#[path = "store_resolve.rs"]
mod resolve;
#[path = "store_resource_journal.rs"]
mod resource_journal;
#[path = "store_structure.rs"]
mod structure;
use resource_journal::ResourceTouches;

type IdentityProxies = std::collections::HashSet<u32, super::intern::FxBuild>;
type ResourceUris = std::collections::HashSet<String, super::intern::FxBuild>;

#[derive(Clone, Copy, PartialEq, Eq)]
enum ReferenceEncoding {
    Wire,
    Identity,
}

struct Saved {
    header: Header,
    slots: Vec<Slot>,
    carried: Option<Box<Carried>>,
    implicit: bool,
}
struct Frame {
    token: Checkpoint,
    headers: usize,
    slots: usize,
    lists: usize,
    json: usize,
    proxies: usize,
    strings: usize,
    resources: usize,
    saved: FxHashMap<ObjId, Saved>,
    /// Original lengths of append-owned lists that predate this frame.
    list_lengths: FxHashMap<u32, usize>,
    /// Prior cells of slot-owned lists, in mutation order. Replayed backwards.
    list_items: Vec<((u32, usize), Slot)>,
    records: FxHashMap<String, Option<ResourceRecord>>,
    touches: ResourceTouches,
    revision: Option<String>,
    release_unloaded_values: bool,
    /// `FrozenStore::prepared_written` when the frame began.
    prepared_written: bool,
}

/// The validated model a store started from (`FrozenStore::from_model`),
/// while its objects keep their indices and arena positions.
#[derive(Clone)]
struct Prefix {
    /// Arena extents of the model: its objects' slots, lists and retained
    /// values lie below them, and everything a later write allocates above.
    slots: usize,
    lists: usize,
    json: usize,
    resources: usize,
    facts: Arc<PreparedFacts>,
}

pub struct FrozenStore {
    builder: ModelBuilder,
    revision: String,
    records: BTreeMap<String, ResourceRecord>,
    /// Exact strings admitted through the validating setter. Raw materialized
    /// imports do not establish this proof. Retention is bounded by records.
    validated_resource_uris: ResourceUris,
    checkpoints: CheckpointIssuer,
    frames: Vec<Frame>,
    indexes: Indexes,
    /// Lists created for one mutable slot; imported/possibly shared lists are
    /// copied once before append. Not part of snapshots or public identities.
    append_lists: FxHashMap<u32, AppendList>,
    descriptors: Arc<OnceLock<StorageResult<SchemaDescriptor>>>,
    incarnations: Vec<Arc<()>>,
    archive_defaults: bool,
    /// Proxy indexes that name an identity rather than a scope spelling.
    identity_proxies: IdentityProxies,
    /// See `Prefix`; None for a store that did not start from a model.
    prefix: Option<Prefix>,
    /// Whether a write changed an object of the model the store started
    /// from. Rolling back restores those objects and this flag.
    prepared_written: bool,
    /// Rows of recently resolved identities. Lookup-only: a hit is rechecked
    /// against the row's identity and the live index, so a stale or
    /// colliding entry only costs the ordinary lookup.
    recent_objects: RecentObjects,
}

/// Direct-mapped by the identity's length and trailing bytes, where
/// generated identities differ.
struct RecentObjects(Box<[std::sync::atomic::AtomicU32]>);
impl RecentObjects {
    const SLOTS: usize = 1024;
    fn new() -> Self {
        Self(
            (0..Self::SLOTS)
                .map(|_| std::sync::atomic::AtomicU32::new(u32::MAX))
                .collect(),
        )
    }
    fn slot(&self, id: &str) -> &std::sync::atomic::AtomicU32 {
        let bytes = id.as_bytes();
        let mut tail = [0u8; 8];
        let taken = bytes.len().min(8);
        tail[..taken].copy_from_slice(&bytes[bytes.len() - taken..]);
        let mixed =
            (u64::from_le_bytes(tail) ^ bytes.len() as u64).wrapping_mul(0x9e37_79b9_7f4a_7c15);
        &self.0[(mixed >> 54) as usize % Self::SLOTS]
    }
}

impl FrozenStore {
    #[cfg(feature = "components")]
    pub(in crate::core) fn borrow_mounted<'a>(
        &'a self,
        id: &str,
        context: crate::core::components::ReadContext<'a>,
    ) -> StorageResult<Option<FrozenObject<'a>>> {
        self.object(id)
            .map(|row| FrozenObject::new(self, row).map(|object| object.with_mount(context)))
            .transpose()
    }

    #[cfg(feature = "components")]
    pub(crate) fn component_slot_state(
        &self,
        id: &str,
        feature: super::FeatureId,
    ) -> Option<(bool, bool, bool)> {
        let row = self.object(id)?;
        let data = &self.builder.data;
        let mm = self.metamodel();
        let slot = data.slot(mm, row, feature)?;
        let unassigned = matches!(slot, Slot::Default | Slot::Absent);
        let empty = data.is_empty_slot(mm, feature, slot);
        let optional_unassigned = matches!(slot, Slot::Default)
            && mm.feature_info(feature).is_some_and(|info| {
                info.kind == FeatureKind::Attribute && !info.many && info.lower == 0
            });
        Some((unassigned, empty, optional_unassigned))
    }

    #[cfg(feature = "components")]
    pub(crate) fn share_schema(
        &mut self,
        context: &crate::core::components::SharedSchema,
    ) -> StorageResult<()> {
        if self.metamodel().digest().map_err(|e| e.to_string())? != context.digest {
            return Err("Component uses an incompatible metamodel".into());
        }
        self.builder.metamodel = context.metamodel.clone();
        self.descriptors = context.descriptors.clone();
        Ok(())
    }

    #[cfg(feature = "components")]
    pub(crate) fn inspect_mounted(
        &self,
        id: &str,
        context: crate::core::components::ReadContext<'_>,
        f: &mut dyn FnMut(storage::ObjectView<'_>) -> StorageResult<()>,
    ) -> StorageResult<bool> {
        let Some(row) = self.object(id) else {
            return Ok(false);
        };
        let view = FrozenObject::new(self, row)?.with_mount(context);
        f(storage::ObjectView::frozen(view))?;
        Ok(true)
    }

    /// Freeze a schema once and create an empty builder-backed store with
    /// the default value indexes for checked model editing.
    pub fn for_schema(schema: &crate::definitions::schema::Schema) -> Result<Self, FrozenError> {
        Ok(Self::with_index_policy(
            Arc::new(Metamodel::freeze(schema)?),
            IndexPolicy::default(),
        ))
    }

    /// A parser construction store. Its wire snapshots retain explicit
    /// assignments of defaults for later linking and default completion.
    pub fn new(metamodel: Arc<Metamodel>) -> Self {
        let mut store =
            Self::with_index_policy(metamodel, IndexPolicy::SelectedValues(BTreeSet::new()));
        store.archive_defaults = true;
        store
    }

    /// Construct with selected feature indexes. By default the frozen parser
    /// store indexes only class and resource membership.
    pub fn with_index_policy(metamodel: Arc<Metamodel>, policy: IndexPolicy) -> Self {
        Self {
            builder: ModelBuilder::new(metamodel),
            revision: String::new(),
            records: BTreeMap::new(),
            validated_resource_uris: ResourceUris::default(),
            checkpoints: CheckpointIssuer::default(),
            frames: Vec::new(),
            indexes: Indexes::new(policy),
            append_lists: FxHashMap::default(),
            descriptors: Arc::default(),
            incarnations: Vec::new(),
            archive_defaults: false,
            identity_proxies: Default::default(),
            prefix: None,
            prepared_written: false,
            recent_objects: RecentObjects::new(),
        }
    }
    /// A store holding a validated model, ready to parse more resources into
    /// (a host's prepared standard library). Its objects keep their indices;
    /// `into_builder`, `link` and the completions then treat them as already
    /// linked and completed: only proxies added later are resolved, and
    /// implicit elements, defaults and language validation (`Model::check`)
    /// cover only objects added later. Opposites and structural validation
    /// still hold for the whole model, so a new reference into the model
    /// gets its opposite end.
    ///
    /// While no write changes an object of `model`, `into_builder` keeps
    /// those objects in place, and the phases after it do the work the new
    /// objects need, not the whole model's again: a prepared object is
    /// visited only when a new reference gives it an opposite end. The
    /// result is the same as the whole-model work of
    /// `into_builder_repacked`.
    pub fn from_model(model: super::Model) -> Self {
        let super::Model {
            metamodel,
            data,
            facts,
            ..
        } = model;
        let records = data
            .resources
            .iter()
            .zip(&data.resource_records)
            .filter_map(|(uri, record)| {
                record
                    .as_ref()
                    .map(|record| (data.strings.resolve(*uri).to_owned(), record.clone()))
            })
            .collect();
        let prepared = data.headers.len();
        // A validated model has one object per identity and no unresolved
        // reference; anything else is handed over by repacking.
        let prefix = (data.by_id.len() == prepared && data.proxies.is_empty()).then(|| Prefix {
            slots: data.slots.len(),
            lists: data.lists.len(),
            json: data.json.len(),
            resources: data.resources.len(),
            facts,
        });
        let mut builder = ModelBuilder::new(metamodel);
        builder.data = data;
        builder.prepared = prepared;
        let mut store = Self::new(builder.metamodel.clone());
        store.builder = builder;
        store.records = records;
        store.prefix = prefix;
        // One scope serves every object of the model: a handle is checked
        // against the scope stored at its own row.
        store.incarnations.resize(prepared, Arc::new(()));
        store.indexes.defer();
        store
    }
    pub fn metamodel(&self) -> &Arc<Metamodel> {
        self.builder.metamodel()
    }

    /// Package assembly has copied already validated payloads into this arena.
    /// Later language completion/checking covers only project objects.
    pub(crate) fn mark_prepared(&mut self) {
        self.builder.prepared = self.builder.data.headers.len();
        // The prepared objects are no longer one validated model's.
        self.prefix = None;
    }

    /// Scoped descriptors for this store's immutable schema. Forks share the
    /// scope. A separately opened store issues its own descriptors.
    pub fn schema_descriptor(&self) -> StorageResult<&SchemaDescriptor> {
        self.descriptors
            .get_or_init(|| SchemaDescriptor::new(self.metamodel().schema()))
            .as_ref()
            .map_err(Clone::clone)
    }

    pub(crate) fn fork_arena(&self) -> StorageResult<Self> {
        if self.is_transaction_active() {
            return Err("Cannot fork an active storage transaction".into());
        }
        Ok(Self {
            builder: self.builder.clone(),
            revision: self.revision.clone(),
            records: self.records.clone(),
            validated_resource_uris: self.validated_resource_uris.clone(),
            checkpoints: CheckpointIssuer::default(),
            frames: Vec::new(),
            indexes: self.indexes.clone(),
            append_lists: self.append_lists.clone(),
            descriptors: self.descriptors.clone(),
            incarnations: self.incarnations.clone(),
            archive_defaults: self.archive_defaults,
            identity_proxies: self.identity_proxies.clone(),
            prefix: self.prefix.clone(),
            prepared_written: self.prepared_written,
            recent_objects: RecentObjects::new(),
        })
    }

    /// Load the materialized storage wire into a store with a known schema.
    /// The arena checks storage shapes during import. Graph validation remains
    /// the responsibility of the checked model facade or the linker.
    pub fn from_materialized(
        schema: &crate::definitions::schema::Schema,
        model: &crate::core::model::Model,
    ) -> StorageResult<Self> {
        let mut store = Self::for_schema(schema).map_err(|error| error.to_string())?;
        storage::atomic(&mut store, |store| {
            store.set_revision(&model.schema_revision)?;
            for element in &model.elements {
                if element.id.is_empty() || store.object(&element.id).is_some() {
                    return Err("Duplicate or empty model identity".into());
                }
                store.put_with(element.clone(), ReferenceEncoding::Identity)?;
            }
            Ok(())
        })?;
        // The supplied registry is authoritative, including empty resources.
        store.records = model.resources.clone();
        store.validated_resource_uris.clear();
        Ok(store)
    }

    /// Consume the mutable store without losing wire metadata or row order.
    pub fn into_model(self) -> StorageResult<crate::core::model::Model> {
        if self.is_transaction_active() {
            return Err("Cannot consume an active storage transaction".into());
        }
        storage::export(&self)
    }
    /// Finish construction: resolve identity references, keep only proxies a
    /// slot still names, and install the resource records.
    ///
    /// A store that started from a validated model (`from_model`) whose
    /// objects no write changed keeps them in place and rebuilds only the
    /// objects after them, so the work is proportional to what was added.
    /// The builder, and the model linked and validated from it, hold the
    /// same objects, values, containers and resource records as
    /// `into_builder_repacked` gives.
    pub fn into_builder(mut self) -> Result<ModelBuilder, FrozenError> {
        if !self.frames.is_empty() {
            return Err(FrozenError::Unsupported("finishing inside a transaction"));
        }
        let Some(prefix) = self
            .prefix
            .take()
            .filter(|prefix| !self.prepared_written && self.tail_follows(prefix))
        else {
            return self.into_builder_repacked();
        };
        let Self {
            mut builder,
            records,
            identity_proxies,
            ..
        } = self;
        rebuild_tail(&mut builder, &prefix, &identity_proxies)?;
        let data = &mut builder.data;
        data.resource_records
            .iter_mut()
            .for_each(|slot| *slot = None);
        for (uri, record) in records {
            super::builder::intern_resource_record(data, &uri, record)?;
        }
        // Later validation skips the prepared objects only if they are
        // valid with nothing delegated; established once per model.
        let prepared = builder.prepared;
        prefix.facts.strict.get_or_init(|| {
            super::validate::rows_valid(&builder.metamodel, &builder.data, prepared)
        });
        builder.layer = Some(Layer {
            slots: prefix.slots,
            lists: prefix.lists,
            touched: Default::default(),
            completed: Default::default(),
            facts: prefix.facts,
        });
        Ok(builder)
    }

    /// Whether the arena still extends the model the store started from,
    /// and keeping the resource table gives the resources a repack would:
    /// every resource has a live object or a record. A resource of that
    /// model has one of the two unless its record was removed.
    fn tail_follows(&self, prefix: &Prefix) -> bool {
        let data = &self.builder.data;
        if data.headers.len() < self.builder.prepared
            || data.slots.len() < prefix.slots
            || data.lists.len() < prefix.lists
            || data.json.len() < prefix.json
            || data.resources.len() < prefix.resources
        {
            return false;
        }
        let mut used = vec![false; data.resources.len()];
        for (index, header) in data.headers.iter().enumerate().skip(self.builder.prepared) {
            if is_live(data, ObjId(index as u32))
                && let Some(used) = used.get_mut(header.resource as usize)
            {
                *used = true;
            }
        }
        data.resources
            .iter()
            .zip(&data.resource_records)
            .zip(used)
            .enumerate()
            .all(|(index, ((uri, record), used))| {
                let recorded = || self.records.contains_key(data.strings.resolve(*uri));
                if index < prefix.resources {
                    record.is_none() || recorded()
                } else {
                    used || recorded()
                }
            })
    }

    /// As `into_builder`, always by repacking every live object into a new
    /// arena, and with the whole model, not only the objects after the
    /// prepared ones, covered by the later opposite completion and
    /// structural validation. It is the reference `into_builder` must agree
    /// with, and what it falls back to.
    pub fn into_builder_repacked(self) -> Result<ModelBuilder, FrozenError> {
        if !self.frames.is_empty() {
            return Err(FrozenError::Unsupported("finishing inside a transaction"));
        }
        let Self {
            builder,
            records,
            identity_proxies,
            indexes,
            ..
        } = self;
        drop(indexes);
        let mut packed = pack_arena(&builder, &identity_proxies, PackMode::Link)?.builder;
        drop(builder);
        for (uri, record) in records {
            super::builder::intern_resource_record(&mut packed.data, &uri, record)?;
        }
        Ok(packed)
    }

    fn object(&self, id: &str) -> Option<ObjId> {
        use std::sync::atomic::Ordering::Relaxed;
        let data = &self.builder.data;
        let slot = self.recent_objects.slot(id);
        let recent = ObjId(slot.load(Relaxed));
        if let Some(header) = data.header(recent)
            && data.strings.resolve(header.id) == id
            && data.by_id.get(&header.id) == Some(&recent)
        {
            return Some(recent);
        }
        let found = self.builder.lookup(id)?;
        slot.store(found.0, Relaxed);
        Some(found)
    }

    fn object_view(&self, id: &str) -> StorageResult<Option<FrozenObject<'_>>> {
        self.object(id)
            .map(|row| FrozenObject::new(self, row))
            .transpose()
    }

    fn create_row(
        &mut self,
        id: &str,
        class: super::ClassId,
        resource: &str,
        start: usize,
        end: usize,
    ) -> StorageResult<ObjId> {
        let row = self
            .builder
            .create_at(id, class, resource, start, end)
            .map_err(|error| error.to_string())?;
        self.incarnations.push(Arc::new(()));
        Ok(row)
    }

    /// Resolve a live identity to a handle for this object incarnation.
    pub fn handle(&self, id: &str) -> Option<ObjectHandle> {
        let row = self.object(id)?;
        Some(ObjectHandle {
            scope: self.incarnations.get(row.index())?.clone(),
            row: row.0.into(),
        })
    }

    /// Inspect the same live incarnation, rejecting stale and foreign handles.
    pub fn inspect_handle(
        &self,
        handle: &ObjectHandle,
        f: &mut dyn FnMut(storage::ObjectView<'_>) -> StorageResult<()>,
    ) -> StorageResult<()> {
        self.inspect_row(self.handle_row(handle)?, f)
    }

    fn handle_row(&self, handle: &ObjectHandle) -> StorageResult<ObjId> {
        let row = ObjId(u32::try_from(handle.row).map_err(|_| "Invalid object handle")?);
        let data = &self.builder.data;
        if !is_live(data, row) {
            return Err("Stale object handle".into());
        }
        if self
            .incarnations
            .get(row.index())
            .is_none_or(|scope| !Arc::ptr_eq(scope, &handle.scope))
        {
            return Err("Stale or foreign object handle".into());
        }
        Ok(row)
    }

    pub(crate) fn handle_target(&self, handle: &ObjectHandle) -> StorageResult<(&str, &str)> {
        let view = FrozenObject::new(self, self.handle_row(handle)?)?;
        Ok((view.id(), view.class()))
    }

    fn inspect_row(
        &self,
        row: ObjId,
        f: &mut dyn FnMut(storage::ObjectView<'_>) -> StorageResult<()>,
    ) -> StorageResult<()> {
        let view = FrozenObject::new(self, row)?;
        let assignment = if self.archive_defaults {
            view.archive_assignment()?
        } else {
            None
        };
        f(storage::ObjectView::frozen(
            view.with_assignment(assignment.as_ref()),
        ))
    }
    fn active(&self) -> StorageResult<()> {
        if self.frames.is_empty() {
            return Err("Storage write requires a transaction".into());
        }
        Ok(())
    }
    /// Snapshot an object the first time the current frame changes it.
    fn touch(&mut self, obj: ObjId) -> StorageResult<()> {
        if obj.index() < self.builder.prepared {
            self.prepared_written = true;
        }
        let data = &self.builder.data;
        let Some(frame) = self.frames.last_mut() else {
            return Err("Storage write requires a transaction".into());
        };
        if obj.index() >= frame.headers || frame.saved.contains_key(&obj) {
            return Ok(());
        }
        let header = data
            .headers
            .get(obj.index())
            .ok_or("Missing object")?
            .clone();
        let count = self
            .builder
            .metamodel
            .class_info(header.class)
            .map_or(0, |c| c.stored.len());
        let base = header.slots as usize;
        let slots = data
            .slots
            .get(base..base + count)
            .ok_or("Missing object slots")?
            .to_vec();
        let carried = data.carried.get(&obj).cloned();
        frame.saved.insert(
            obj,
            Saved {
                header,
                slots,
                carried,
                implicit: data.implicit.contains(&obj),
            },
        );
        Ok(())
    }
    fn element(&self, obj: ObjId) -> StorageResult<Element> {
        let mm = &self.builder.metamodel;
        let data = &self.builder.data;
        let header = data.headers.get(obj.index()).ok_or("Missing object")?;
        let class = mm.class_info(header.class).ok_or("Unknown class")?;
        let mut values = BTreeMap::new();
        for (index, feature) in class.stored.iter().enumerate() {
            let value = match data.slots.get(header.slots as usize + index) {
                None | Some(Slot::Absent) => continue,
                Some(Slot::Default) => mm
                    .definition(*feature)
                    .map_or(Value::Null, |d| d.default.clone()),
                Some(slot) => self.decode(*slot),
            };
            values.insert(mm.feature_name(*feature).to_owned(), value);
        }
        let carried = data.carried.get(&obj);
        let mut assignment_state = carried.and_then(|c| c.assignment_state.clone());
        if assignment_state.is_none() && self.archive_defaults && !class.has_unsettable {
            assignment_state =
                super::interop::derived_assignment(mm, data, header.class, header.slots as usize)?;
        }
        Ok(Element {
            implicit: data.implicit.contains(&obj),
            initialization: carried.and_then(|c| c.initialization.clone()),
            assignment_state,
            id: data.strings.resolve(header.id).to_owned(),
            class: mm.class_key(header.class).to_owned(),
            type_identity: class.identity.to_string(),
            resource: data
                .resources
                .get(header.resource as usize)
                .map_or("", |s| data.strings.resolve(*s))
                .to_owned(),
            start: header.start,
            end: header.end,
            values,
        })
    }
    fn decode(&self, slot: Slot) -> Value {
        let data = &self.builder.data;
        match slot {
            Slot::Proxy(p) => match data.proxies.get(p as usize) {
                Some(proxy) if self.identity_proxies.contains(&p) => {
                    Value::String(data.strings.resolve(proxy.spelling).to_owned())
                }
                Some(proxy) => Value::String(format!("?{}", data.strings.resolve(proxy.spelling))),
                None => Value::Null,
            },
            Slot::Many(list) => Value::Array(
                data.lists
                    .get(list as usize)
                    .map_or(&[][..], Vec::as_slice)
                    .iter()
                    .map(|item| self.decode(*item))
                    .collect(),
            ),
            slot => super::interop::decode(data, slot),
        }
    }
    fn reference(
        &mut self,
        obj: ObjId,
        feature: super::FeatureId,
        value: &Value,
        encoding: ReferenceEncoding,
    ) -> StorageResult<Slot> {
        let Value::String(text) = value else {
            return Err("Reference value must be an identity".into());
        };
        let data = &mut self.builder.data;
        let wire_spelling = match encoding {
            ReferenceEncoding::Wire => text.strip_prefix('?'),
            ReferenceEncoding::Identity => None,
        };
        let (spelling, identity) = match wire_spelling {
            Some(spelling) => (spelling, false),
            None => {
                if let Some(target) = data.strings.get(text).and_then(|s| data.by_id.get(&s)) {
                    return Ok(Slot::Ref(*target));
                }
                (text.as_str(), true)
            }
        };
        proxy_slot(
            data,
            obj,
            feature,
            spelling,
            identity.then_some(&mut self.identity_proxies),
        )
        .map_err(|error| error.to_string())
    }
    /// Replace every stored value of `obj` from `element`. A slot that was
    /// never assigned stays unassigned (`Slot::Default`) while its value equals
    /// the schema default, unless the element's assignment state lists it as
    /// set: element values alone cannot tell an
    /// assignment of the default from no assignment.
    fn write(
        &mut self,
        obj: ObjId,
        element: &Element,
        encoding: ReferenceEncoding,
    ) -> StorageResult<()> {
        let mm = self.builder.metamodel.clone();
        let header = self
            .builder
            .data
            .header(obj)
            .ok_or("Missing object")?
            .clone();
        let class = mm.class_info(header.class).ok_or("Unknown class")?;
        let mut slots = vec![Slot::Absent; class.stored.len()];
        let base = header.slots as usize;
        for (name, value) in &element.values {
            let feature = mm
                .feature(header.class, name)
                .ok_or_else(|| format!("Unknown feature {}.{name}", element.class))?;
            let feature = mm.write_feature(header.class, feature);
            let slot = mm
                .slot(header.class, feature)
                .ok_or("Derived values cannot be stored")?;
            let target = slots.get_mut(slot).ok_or("Missing object slots")?;
            // A redefined attribute and its redefining one share this slot
            // (an export lists both, `declaredShortName: "1"` beside
            // `reqId: null`); an absent value never replaces a written one.
            if value.is_null() && *target != Slot::Absent {
                continue;
            }
            let info = mm.feature_info(feature).ok_or("Unknown feature")?;
            let unassigned = self.builder.data.slots.get(base + slot) == Some(&Slot::Default);
            if unassigned
                && !element
                    .assignment_state
                    .as_ref()
                    .is_some_and(|state| state.explicitly_set(name))
                && mm.definition(feature).is_some_and(|d| d.default == *value)
            {
                *target = Slot::Default;
                continue;
            }
            *target = self.encode_slot(obj, feature, info.kind, value, encoding)?;
        }
        let data = &mut self.builder.data;
        data.slots
            .get_mut(base..base + slots.len())
            .ok_or("Missing object slots")?
            .copy_from_slice(&slots);
        let resource = super::builder::intern_resource_uri(data, &element.resource)
            .map_err(|e| e.to_string())?;
        if let Some(h) = data.headers.get_mut(obj.index()) {
            h.resource = resource;
            h.start = element.start;
            h.end = element.end;
        }
        if element.initialization.is_some() || element.assignment_state.is_some() {
            data.carried.insert(
                obj,
                Box::new(Carried {
                    initialization: element.initialization.clone(),
                    assignment_state: element.assignment_state.clone(),
                }),
            );
        } else {
            data.carried.remove(&obj);
        }
        if element.implicit {
            data.implicit.insert(obj);
        } else {
            data.implicit.remove(&obj);
        }
        Ok(())
    }
    fn encode_slot(
        &mut self,
        obj: ObjId,
        feature: super::FeatureId,
        kind: FeatureKind,
        value: &Value,
        encoding: ReferenceEncoding,
    ) -> StorageResult<Slot> {
        Ok(if kind == FeatureKind::Attribute {
            encode_value(&mut self.builder.data, value.clone()).map_err(|e| e.to_string())?
        } else {
            match value {
                Value::Null => Slot::Null,
                Value::Array(items) => {
                    let items = items
                        .iter()
                        .map(|item| self.reference(obj, feature, item, encoding))
                        .collect::<StorageResult<Vec<_>>>()?;
                    new_list(&mut self.builder.data, items).map_err(|e| e.to_string())?
                }
                item => self.reference(obj, feature, item, encoding)?,
            }
        })
    }
    fn has_pending_atom(&self, slot: Slot) -> bool {
        let pending =
            |atom| matches!(atom, Slot::Proxy(index) if !self.identity_proxies.contains(&index));
        match slot {
            Slot::Many(list) => self
                .builder
                .data
                .lists
                .get(list as usize)
                .is_some_and(|items| items.iter().copied().any(pending)),
            atom => pending(atom),
        }
    }

    /// An explicit assignment of one slot, without materializing or
    /// rewriting the object; it records the assignment even when the value
    /// equals the schema default. Effects match the element-level `put`: an
    /// unchanged value changes nothing else; otherwise the resource's content
    /// revision moves, and containment targets that enter or leave the slot
    /// leave or join the resource's roots.
    fn assign_value(
        &mut self,
        id: &str,
        field: &str,
        value: Option<Value>,
        encoding: ReferenceEncoding,
    ) -> StorageResult<()> {
        self.active()?;
        let obj = self.object(id).ok_or("Missing element")?;
        let mm = self.builder.metamodel.clone();
        let header = self
            .builder
            .data
            .header(obj)
            .ok_or("Missing object")?
            .clone();
        let feature = mm
            .feature(header.class, field)
            .ok_or_else(|| format!("Unknown feature {}.{field}", mm.class_key(header.class)))?;
        let slot = mm
            .slot(header.class, feature)
            .ok_or("Derived values cannot be stored")?;
        let info = mm.feature_info(feature).ok_or("Unknown feature")?;
        let (kind, containment) = (info.kind, info.kind == FeatureKind::Containment);
        let index = header.slots as usize + slot;
        let current = *self
            .builder
            .data
            .slots
            .get(index)
            .ok_or("Missing object slots")?;
        let before = self.slot_value(feature, current);
        let changed = before != value
            || (encoding == ReferenceEncoding::Identity && self.has_pending_atom(current));
        if !changed && current != Slot::Default {
            return Ok(());
        }
        let resource = if changed {
            let data = &self.builder.data;
            let uri = data
                .resources
                .get(header.resource as usize)
                .ok_or("Missing object resource")?;
            Some(data.strings.resolve(*uri).to_owned())
        } else {
            None
        };
        self.touch(obj)?;
        // Stage arena allocations and metadata before publishing the slot.
        // The outer frame already owns the object's rollback snapshot.
        let next = storage::atomic(self, |store| {
            let next = match &value {
                None => Slot::Absent,
                Some(value) => store.encode_slot(obj, feature, kind, value, encoding)?,
            };
            if let Some(resource) = &resource {
                crate::core::resources::field_delta_inner(
                    store,
                    resource,
                    containment,
                    before.as_ref(),
                    value.as_ref(),
                )?;
            }
            Ok(next)
        })?;
        *self
            .builder
            .data
            .slots
            .get_mut(index)
            .ok_or("Missing object slots")? = next;
        if !changed {
            // An explicit assignment of the unassigned default value.
            return Ok(());
        }
        if let Some(name) = mm
            .class_info(header.class)
            .and_then(|class| class.stored.get(slot))
            .map(|&feature| mm.feature_name(feature))
            .filter(|name| self.indexes.indexes(name))
        {
            if let Some(before) = &before {
                self.indexes.update_value(obj, name, before, false);
            }
            if let Some(after) = &value {
                self.indexes.update_value(obj, name, after, true);
            }
        }
        Ok(())
    }
    /// A stored slot's value as the element-level API reports it.
    fn slot_value(&self, feature: super::FeatureId, slot: Slot) -> Option<Value> {
        match slot {
            Slot::Absent => None,
            Slot::Default => Some(
                self.builder
                    .metamodel
                    .definition(feature)
                    .map_or(Value::Null, |d| d.default.clone()),
            ),
            slot => Some(self.decode(slot)),
        }
    }
    fn index_row(&mut self, row: ObjId, add: bool) {
        let data = &self.builder.data;
        let Some(header) = data
            .header(row)
            .filter(|h| data.by_id.get(&h.id) == Some(&row))
        else {
            return;
        };
        let Some(&resource) = data.resources.get(header.resource as usize) else {
            return;
        };
        if !self.indexes.update_row(row, header.class, resource, add) || !self.indexes.has_values()
        {
            return;
        }
        let mm = &self.builder.metamodel;
        if let Some(class) = mm.class_info(header.class) {
            for (offset, &feature) in class.stored.iter().enumerate() {
                let name = mm.feature_name(feature);
                if self.indexes.indexes(name) {
                    let slot = data.slots.get(header.slots as usize + offset).copied();
                    if let Some(value) = slot.and_then(|slot| self.slot_value(feature, slot)) {
                        self.indexes.update_value(row, name, &value, add);
                    }
                }
            }
        }
    }

    /// The postings, indexing the rows of a model the store started from
    /// (`from_model`) on first use: class and resource membership only,
    /// since such a store indexes no values.
    fn postings(&self) -> &index::Postings {
        self.indexes.postings(|postings| {
            let data = &self.builder.data;
            for row in self.live_rows(None) {
                if let Some((header, resource)) = data
                    .header(row)
                    .and_then(|h| Some((h, *data.resources.get(h.resource as usize)?)))
                {
                    postings.update_row(row, header.class, resource, true);
                }
            }
        })
    }

    fn live_rows(&self, after: Option<u64>) -> impl Iterator<Item = ObjId> + '_ {
        let start = after.map_or(0, |row| row.saturating_add(1));
        let data = &self.builder.data;
        let dense = data.by_id.len() == data.headers.len();
        (start..data.headers.len() as u64)
            .map(|row| ObjId(row as u32))
            .filter(move |&row| dense || is_live(data, row))
    }

    fn identity(&self, row: ObjId) -> StorageResult<&str> {
        let data = &self.builder.data;
        let header = data.header(row).ok_or("Missing object")?;
        Ok(data.strings.resolve(header.id))
    }

    fn posting(
        &self,
        query: Query<'_>,
        observation: &InvocationContext<'_>,
    ) -> StorageResult<Option<&BTreeSet<ObjId>>> {
        Ok(match query {
            Query::Class(class) => self
                .builder
                .metamodel
                .class(class)
                .and_then(|id| self.postings().classes.get(&id)),
            Query::Resource(uri) => self
                .builder
                .data
                .strings
                .get(uri)
                .and_then(|id| self.postings().resources.get(&id)),
            Query::Value(..) => self
                .postings()
                .values
                .get(&storage::query_key_observed(query, observation)?),
        })
    }

    fn collect_candidates(
        &self,
        mut candidates: impl Iterator<Item = ObjId>,
        after: Option<u64>,
        limit: usize,
        exact: bool,
        observation: &InvocationContext<'_>,
    ) -> StorageResult<QueryCandidates> {
        let mut rows = Vec::new();
        for row in candidates.by_ref().take(limit) {
            storage::push_identity_observed(
                &mut rows,
                row.0.into(),
                self.identity(row)?,
                observation,
            )?;
        }
        Ok(QueryCandidates {
            after: rows.last().map(|(row, _)| *row).or(after),
            rows,
            exhausted: candidates.next().is_none(),
            exact,
        })
    }
}

impl ModelRead for FrozenStore {
    fn export_snapshot_observed(
        &self,
        max_objects: usize,
        observation: &InvocationContext<'_>,
    ) -> StorageResult<crate::core::model::Model> {
        use crate::definitions::retained_storage::reserve_serialized;

        let data = &self.builder.data;
        let count = data.by_id.len();
        storage::export_snapshot_rows(
            &self.revision,
            &self.records,
            count,
            max_objects,
            observation,
            |visit| {
                observation.work(data.headers.len().saturating_sub(count))?;
                for row in self.live_rows(None) {
                    let view = FrozenObject::new(self, row)?;
                    if let Some(class) = view.class_info() {
                        // Both field iterations visit absent arena slots too.
                        observation.work(class.stored.len().saturating_mul(2))?;
                        if self.archive_defaults
                            && view.assignment_state().is_none()
                            && !class.has_unsettable
                        {
                            // Archive inspection may derive assignment metadata
                            // from default-valued fields. Pay for its temporary
                            // names and decoded values before inspection.
                            for (key, value) in storage::ObjectView::frozen(view).fields() {
                                observation.work(1)?;
                                observation.retain_storage(128)?;
                                reserve_serialized(key, observation)?;
                                reserve_serialized(&value, observation)?;
                            }
                        }
                    }
                    self.inspect_row(row, visit)?;
                }
                Ok(())
            },
        )
    }

    fn is_pending_reference(&self, id: &str, field: &str, index: usize, wire: &str) -> bool {
        let data = &self.builder.data;
        let mm = &self.builder.metamodel;
        let Some(spelling) = wire.strip_prefix('?') else {
            return false;
        };
        let proxy = self.object(id).and_then(|obj| {
            let header = data.header(obj)?;
            let feature = mm.feature(header.class, field)?;
            let slot = data.slot(mm, obj, feature)?;
            let atom = match slot {
                Slot::Many(list) => *data.lists.get(list as usize)?.get(index)?,
                atom if index == 0 => atom,
                _ => return None,
            };
            match atom {
                Slot::Proxy(p) if !self.identity_proxies.contains(&p) => {
                    data.proxies.get(p as usize)
                }
                _ => None,
            }
        });
        proxy.is_some_and(|proxy| data.strings.resolve(proxy.spelling) == spelling)
    }

    /// One slot, without materializing the object.
    fn value(&self, id: &str, field: &str) -> StorageResult<Option<Value>> {
        let Some(obj) = self.object(id) else {
            return Ok(None);
        };
        let mm = &self.builder.metamodel;
        let header = self.builder.data.header(obj).ok_or("Missing object")?;
        let Some(feature) = mm.feature(header.class, field) else {
            return Ok(None);
        };
        let Some(slot) = mm.slot(header.class, feature) else {
            return Ok(None);
        };
        let current = *self
            .builder
            .data
            .slots
            .get(header.slots as usize + slot)
            .ok_or("Missing object slots")?;
        Ok(self.slot_value(feature, current))
    }
    fn value_observed(
        &self,
        id: &str,
        field: &str,
        observation: &InvocationContext<'_>,
    ) -> StorageResult<Option<Value>> {
        observation.checkpoint()?;
        self.object_view(id)?
            .and_then(|view| view.value(field))
            .map(|value| value.to_json_observed(observation))
            .transpose()
    }
    fn inspect_stored_value_observed(
        &self,
        id: &str,
        field: &str,
        many: bool,
        observation: &InvocationContext<'_>,
        consume: &mut dyn FnMut(storage::ValueView<'_>) -> StorageResult<()>,
    ) -> StorageResult<bool> {
        observation.checkpoint()?;
        let object = self.object_view(id)?;
        let value = object.and_then(|view| view.value(field));
        if many {
            object.ok_or("Missing element")?;
            let value = value
                .filter(|value| value.collection().is_some())
                .ok_or("Multiple feature requires a list")?;
            consume(value)?;
            Ok(true)
        } else if let Some(value) = value {
            consume(value)?;
            Ok(true)
        } else {
            Ok(false)
        }
    }
    fn value_len(&self, id: &str, field: &str) -> StorageResult<Option<usize>> {
        Ok(self
            .object_view(id)?
            .and_then(|view| view.value(field))
            .and_then(|value| value.collection())
            .map(|items| items.len()))
    }
    fn collection_page(
        &self,
        id: &str,
        field: &str,
        offset: usize,
        limit: usize,
    ) -> StorageResult<Vec<Value>> {
        self.collection_page_observed(id, field, offset, limit, &InvocationContext::default())
    }
    fn collection_page_observed(
        &self,
        id: &str,
        field: &str,
        offset: usize,
        limit: usize,
        observation: &InvocationContext<'_>,
    ) -> StorageResult<Vec<Value>> {
        observation.checkpoint()?;
        let collection = self
            .object_view(id)?
            .ok_or("Missing element")?
            .value(field)
            .and_then(|value| value.collection())
            .ok_or("Multiple feature requires a list")?;
        storage::project_values_observed(collection.iter().skip(offset).take(limit), observation)
    }
    fn resource_records(&self) -> StorageResult<BTreeMap<String, ResourceRecord>> {
        Ok(self.records.clone())
    }
    fn resource_record(&self, uri: &str) -> StorageResult<Option<ResourceRecord>> {
        Ok(self.records.get(uri).cloned())
    }
    fn identity_reserved(&self, id: &str) -> StorageResult<bool> {
        Ok(self.records.values().any(|record| {
            record
                .unloaded
                .as_ref()
                .is_some_and(|contents| contents.identities.contains(id))
        }))
    }
    fn revision(&self) -> StorageResult<String> {
        Ok(self.revision.clone())
    }
    fn inspect_revision(&self, f: &mut dyn FnMut(&str) -> StorageResult<()>) -> StorageResult<()> {
        f(&self.revision)
    }
    fn inspect_class(
        &self,
        id: &str,
        f: &mut dyn FnMut(&str) -> StorageResult<()>,
    ) -> StorageResult<bool> {
        let Some(row) = self.object(id) else {
            return Ok(false);
        };
        let header = self.builder.data.header(row).ok_or("Missing object")?;
        f(self.builder.metamodel.class_key(header.class))?;
        Ok(true)
    }
    fn inspect(
        &self,
        id: &str,
        f: &mut dyn FnMut(storage::ObjectView<'_>) -> StorageResult<()>,
    ) -> StorageResult<bool> {
        let Some(row) = self.object(id) else {
            return Ok(false);
        };
        self.inspect_row(row, f)?;
        Ok(true)
    }
    fn accepts_structure(&self, facts: &dyn storage::StructureFacts) -> bool {
        self.accepts_structure_rows(facts)
    }
    fn may_accept_structure(&self) -> bool {
        self.may_accept_structure_rows()
    }
    fn get(&self, id: &str) -> StorageResult<Option<Element>> {
        self.object(id).map(|obj| self.element(obj)).transpose()
    }
    fn class(&self, id: &str) -> StorageResult<Option<String>> {
        Ok(self.object(id).and_then(|obj| {
            let header = self.builder.data.header(obj)?;
            Some(self.builder.metamodel.class_key(header.class).to_owned())
        }))
    }
    fn page(&self, after: Option<u64>, limit: usize) -> StorageResult<Vec<Row>> {
        self.live_rows(after)
            .take(limit)
            .map(|row| Ok((row.0.into(), self.element(row)?)))
            .collect()
    }
    fn id_page(&self, after: Option<u64>, limit: usize) -> StorageResult<Vec<(u64, String)>> {
        self.id_page_observed(after, limit, &InvocationContext::default())
    }
    fn id_page_observed(
        &self,
        after: Option<u64>,
        limit: usize,
        observation: &InvocationContext<'_>,
    ) -> StorageResult<Vec<(u64, String)>> {
        observation.checkpoint()?;
        let mut rows = Vec::new();
        for row in self.live_rows(after).take(limit) {
            storage::push_identity_observed(
                &mut rows,
                row.0.into(),
                self.identity(row)?,
                observation,
            )?;
        }
        Ok(rows)
    }
    fn query_candidates(
        &self,
        query: Query<'_>,
        after: Option<u64>,
        limit: usize,
    ) -> StorageResult<QueryCandidates> {
        self.query_candidates_observed(query, after, limit, &InvocationContext::default())
    }
    fn query_candidates_observed(
        &self,
        query: Query<'_>,
        after: Option<u64>,
        limit: usize,
        observation: &InvocationContext<'_>,
    ) -> StorageResult<QueryCandidates> {
        observation.checkpoint()?;
        if limit == 0 {
            return Err("Query candidate limit must be positive".into());
        }
        if matches!(query, Query::Value(field, _) if !self.indexes.indexes(field)) {
            return self.collect_candidates(
                self.live_rows(after),
                after,
                limit,
                false,
                observation,
            );
        }
        let start = after.map_or(Unbounded, |row| {
            Excluded(ObjId(u32::try_from(row).unwrap_or(u32::MAX)))
        });
        let candidates = self
            .posting(query, observation)?
            .into_iter()
            .flat_map(|rows| rows.range((start, Unbounded)).copied());
        self.collect_candidates(candidates, after, limit, true, observation)
    }
    fn query(&self, query: Query<'_>) -> StorageResult<Vec<String>> {
        if !matches!(query, Query::Value(field, _) if !self.indexes.indexes(field)) {
            return self
                .posting(query, &InvocationContext::default())?
                .into_iter()
                .flatten()
                .map(|&row| {
                    let id = self.identity(row)?;
                    // Callers usually look the returned identities up next.
                    self.recent_objects
                        .slot(id)
                        .store(row.0, std::sync::atomic::Ordering::Relaxed);
                    Ok(id.to_owned())
                })
                .collect();
        }
        let mut found = Vec::new();
        for row in self.live_rows(None) {
            let id = self.identity(row)?;
            if self.matches(id, query)? {
                found.push(id.to_owned());
            }
        }
        Ok(found)
    }
    fn matches(&self, id: &str, query: Query<'_>) -> StorageResult<bool> {
        self.matches_observed(id, query, &InvocationContext::default())
    }
    #[allow(
        clippy::cmp_owned,
        reason = "Membership compares canonical JSON encodings, not JSON string values"
    )]
    fn matches_observed(
        &self,
        id: &str,
        query: Query<'_>,
        observation: &InvocationContext<'_>,
    ) -> StorageResult<bool> {
        observation.checkpoint()?;
        let Some(row) = self.object(id) else {
            return Ok(false);
        };
        if let Query::Value(field, _) = query
            && !self.indexes.indexes(field)
        {
            let (_, key) = storage::query_key_observed(query, observation)?;
            return Ok(self
                .value(id, field)?
                .is_some_and(|v| value_members(&v).any(|item| item.to_string() == key)));
        }
        Ok(self
            .posting(query, observation)?
            .is_some_and(|posting| posting.contains(&row)))
    }
}

impl ModelStore for FrozenStore {
    fn supports_incremental_validation(&self) -> bool {
        true
    }

    fn is_transaction_active(&self) -> bool {
        !self.frames.is_empty()
    }
    fn begin(&mut self) -> StorageResult<Checkpoint> {
        let token = self.checkpoints.issue()?;
        let data = &self.builder.data;
        self.frames.push(Frame {
            token: token.clone(),
            headers: data.headers.len(),
            slots: data.slots.len(),
            lists: data.lists.len(),
            json: data.json.len(),
            proxies: data.proxies.len(),
            strings: data.strings.len(),
            resources: data.resources.len(),
            saved: FxHashMap::default(),
            list_lengths: FxHashMap::default(),
            list_items: Vec::new(),
            records: FxHashMap::default(),
            touches: ResourceTouches::default(),
            revision: None,
            release_unloaded_values: false,
            prepared_written: self.prepared_written,
        });
        Ok(token)
    }
    fn commit(&mut self, checkpoint: &Checkpoint) -> StorageResult<()> {
        if self.frames.last().map(|f| &f.token) != Some(checkpoint) {
            return Err("Commit does not match the innermost checkpoint".into());
        }
        let packed = if self.frames.len() == 1
            && self
                .frames
                .last()
                .is_some_and(|frame| frame.release_unloaded_values)
        {
            Some(
                pack_arena(&self.builder, &self.identity_proxies, PackMode::Residency)
                    .map_err(|error| error.to_string())?,
            )
        } else {
            None
        };
        let Some(frame) = self.frames.pop() else {
            return Err("Missing checkpoint".into());
        };
        // The parent keeps its earliest snapshot of each object.
        if let Some(parent) = self.frames.last_mut() {
            for change in frame.list_items {
                if (change.0.0 as usize) < parent.lists {
                    parent.list_items.push(change);
                }
            }
            for (list, length) in frame.list_lengths {
                if (list as usize) < parent.lists {
                    parent.list_lengths.entry(list).or_insert(length);
                }
            }
            for (obj, saved) in frame.saved {
                if obj.index() < parent.headers {
                    parent.saved.entry(obj).or_insert(saved);
                }
            }
            resource_journal::merge(
                &self.builder.data.strings,
                parent,
                frame.records,
                frame.touches,
            );
            if parent.revision.is_none() {
                parent.revision = frame.revision;
            }
            parent.release_unloaded_values |= frame.release_unloaded_values;
        }
        if let Some(packed) = packed {
            self.builder = packed.builder;
            self.identity_proxies = packed.identity_proxies;
            self.append_lists.clear();
            // The arena was rebuilt: no object is where its model put it.
            self.prefix = None;
            self.indexes = self.indexes.empty();
            for row in 0..self.builder.data.headers.len() {
                self.index_row(ObjId(row as u32), true);
            }
        }
        Ok(())
    }
    fn rollback(&mut self, checkpoint: &Checkpoint) -> StorageResult<()> {
        if self.frames.last().map(|f| &f.token) != Some(checkpoint) {
            return Err("Rollback does not match the innermost checkpoint".into());
        }
        let Some(frame) = self.frames.pop() else {
            return Err("Missing checkpoint".into());
        };
        // Symbols must be resolved before the checkpoint's string suffix is
        // removed. Full record replacements promote compact undo beforehand.
        for (uri, undo) in frame.touches.into_iter() {
            let uri = self.builder.data.strings.resolve(uri);
            let record = self
                .records
                .get_mut(uri)
                .ok_or("Missing touched resource")?;
            undo.restore(record);
        }
        for row in frame.saved.keys().copied() {
            self.index_row(row, false);
        }
        for row in frame.headers..self.builder.data.headers.len() {
            self.index_row(ObjId(row as u32), false);
        }
        // Remove current postings before restoring list contents, then rebuild
        // restored rows below. Membership journals must precede string rollback.
        for ((list, index), value) in frame.list_items.into_iter().rev() {
            *self
                .builder
                .data
                .lists
                .get_mut(list as usize)
                .and_then(|items| items.get_mut(index))
                .ok_or("Missing journaled reference item")? = value;
        }
        for (&list, &length) in &frame.list_lengths {
            if let Some(items) = self.builder.data.lists.get_mut(list as usize) {
                items.truncate(length);
            }
            if let Some(owned) = self.append_lists.get_mut(&list) {
                owned.truncate(length);
            }
        }
        self.append_lists
            .retain(|&list, owned| (list as usize) < frame.lists && owned.owner < frame.slots);
        let data = &mut self.builder.data;
        for (index, header) in data.headers.drain(frame.headers..).enumerate() {
            let obj = ObjId((frame.headers + index) as u32);
            if data.by_id.get(&header.id) == Some(&obj) {
                data.by_id.remove(&header.id);
            }
        }
        data.slots.truncate(frame.slots);
        self.incarnations.truncate(frame.headers);
        data.lists.truncate(frame.lists);
        data.json.truncate(frame.json);
        data.proxies.truncate(frame.proxies);
        data.resources.truncate(frame.resources);
        data.resource_records.truncate(frame.resources);
        data.strings.truncate(frame.strings);
        self.identity_proxies
            .retain(|p| (*p as usize) < frame.proxies);
        data.carried.retain(|obj, _| obj.index() < frame.headers);
        data.implicit.retain(|obj| obj.index() < frame.headers);
        for (obj, saved) in frame.saved {
            let data = &mut self.builder.data;
            let base = saved.header.slots as usize;
            if let Some(target) = data.slots.get_mut(base..base + saved.slots.len()) {
                target.copy_from_slice(&saved.slots);
            }
            if let Some(header) = data.headers.get_mut(obj.index()) {
                data.by_id.insert(saved.header.id, obj);
                *header = saved.header;
            }
            match saved.carried {
                Some(carried) => {
                    data.carried.insert(obj, carried);
                }
                None => {
                    data.carried.remove(&obj);
                }
            }
            if saved.implicit {
                data.implicit.insert(obj);
            } else {
                data.implicit.remove(&obj);
            }
            self.index_row(obj, true);
        }
        for (uri, record) in frame.records {
            if let Some(record) = record {
                self.records.insert(uri, record);
            } else {
                self.records.remove(&uri);
                self.validated_resource_uris.remove(&uri);
            }
        }
        if let Some(revision) = frame.revision {
            self.revision = revision;
        }
        self.prepared_written = frame.prepared_written;
        Ok(())
    }
    fn set_revision(&mut self, revision: &str) -> StorageResult<()> {
        self.active()?;
        if self.revision == revision {
            return Ok(());
        }
        if let Some(frame) = self.frames.last_mut() {
            frame.revision.get_or_insert_with(|| self.revision.clone());
        }
        self.revision = revision.to_owned();
        Ok(())
    }
    fn set_resource_record(&mut self, record: ResourceRecord) -> StorageResult<()> {
        self.active()?;
        if !self.validated_resource_uris.contains(&record.uri) {
            crate::core::resources::validate_resource_uri(&record.uri)?;
            self.validated_resource_uris.insert(record.uri.clone());
        }
        self.remember_resource(&record.uri);
        self.records.insert(record.uri.clone(), record);
        Ok(())
    }
    fn touch_resource_record(&mut self, uri: &str) -> StorageResult<()> {
        self.touch_record(uri)
    }
    /// The shared root change on the registered record itself: the same
    /// membership test, revision check, admission order, journal entry and
    /// resulting record as reading it, editing the copy and replacing it.
    fn change_resource_root(&mut self, uri: &str, id: &str, root: bool) -> StorageResult<bool> {
        let Some(record) = self.records.get(uri) else {
            return Ok(false);
        };
        if record.roots.iter().any(|r| r == id) == root {
            return Ok(true);
        }
        let revision = record.next_content_revision()?;
        self.active()?;
        if !self.validated_resource_uris.contains(uri) {
            crate::core::resources::validate_resource_uri(uri)?;
            self.validated_resource_uris.insert(uri.to_owned());
        }
        self.remember_resource(uri);
        let record = self.records.get_mut(uri).ok_or("Missing resource record")?;
        if root {
            Arc::make_mut(&mut record.roots).push(id.into());
        } else {
            Arc::make_mut(&mut record.roots).retain(|r| r != id);
        }
        record.content_revision = revision;
        record.source = None;
        record.diagnostics.clear();
        Ok(true)
    }
    fn remove_resource_record(&mut self, uri: &str) -> StorageResult<()> {
        self.active()?;
        if !self.query(Query::Resource(uri))?.is_empty() {
            return Err("Cannot remove a resource containing objects".into());
        }
        self.remember_resource(uri);
        self.records.remove(uri);
        self.validated_resource_uris.remove(uri);
        Ok(())
    }
    fn put(&mut self, element: Element) -> StorageResult<()> {
        self.active()?;
        let encoding = if self.archive_defaults {
            ReferenceEncoding::Wire
        } else {
            ReferenceEncoding::Identity
        };
        storage::atomic(self, |store| store.put_with(element, encoding))
    }
    /// The frozen metamodel validated the schema's instantiations once, at
    /// freeze, so creation does not revalidate the whole schema per object.
    /// Every slot starts unassigned (its schema default).
    fn create_element(
        &mut self,
        schema: &crate::definitions::schema::Schema,
        id: &str,
        class: &str,
        resource: &str,
        start: usize,
        end: usize,
    ) -> StorageResult<()> {
        self.active()?;
        let mm = self.builder.metamodel.clone();
        if schema.revision != mm.revision() {
            return Err("Schema differs from the frozen metamodel".into());
        }
        if id.is_empty() || self.object(id).is_some() {
            return Err("Element identity must be unique and nonempty".into());
        }
        let class_id = mm.class(class).ok_or("Unknown class")?;
        let info = mm.class_info(class_id).ok_or("Unknown class")?;
        if info.abstract_type {
            return Err(format!("Cannot instantiate abstract class {class}"));
        }
        storage::atomic(self, |store| {
            store.configure_schema(schema)?;
            if info.has_containment_defaults {
                crate::core::resources::add_member_with_defaults(
                    store,
                    resource,
                    id,
                    info.stored
                        .iter()
                        .filter_map(|&feature| mm.definition(feature)),
                )?;
            } else {
                crate::core::resources::add_member(store, resource, id)?;
            }
            let obj = store.create_row(id, class_id, resource, start, end)?;
            if info.has_unsettable {
                store.builder.data.carried.insert(
                    obj,
                    Box::new(Carried {
                        assignment_state: Some(crate::core::model::AssignmentState::new(Vec::<
                            String,
                        >::new(
                        ))?),
                        initialization: None,
                    }),
                );
            }
            store.index_row(obj, true);
            Ok(())
        })
    }
    /// The span lives in the object header; nothing else changes, so only the
    /// resource's content revision moves (as with the element-level `put`).
    fn set_span(&mut self, id: &str, start: usize, end: usize) -> StorageResult<()> {
        self.active()?;
        let obj = self.object(id).ok_or("Missing element")?;
        let header = self.builder.data.header(obj).ok_or("Missing object")?;
        if header.start == start && header.end == end {
            return Ok(());
        }
        let resource = self
            .builder
            .data
            .resources
            .get(header.resource as usize)
            .map(|uri| self.builder.data.strings.resolve(*uri).to_owned())
            .ok_or("Missing object resource")?;
        self.touch(obj)?;
        crate::core::resources::touch_resource(self, &resource)?;
        let header = self
            .builder
            .data
            .headers
            .get_mut(obj.index())
            .ok_or("Missing object")?;
        header.start = start;
        header.end = end;
        Ok(())
    }
    fn set_assignment_state(
        &mut self,
        id: &str,
        state: Option<crate::core::model::AssignmentState>,
    ) -> StorageResult<()> {
        self.active()?;
        let obj = self.object(id).ok_or("Missing element")?;
        let data = &self.builder.data;
        let previous = data
            .carried
            .get(&obj)
            .and_then(|carried| carried.assignment_state.as_ref());
        if previous == state.as_ref() {
            return Ok(());
        }
        let header = data.header(obj).ok_or("Missing object")?;
        let uri = data
            .resources
            .get(header.resource as usize)
            .ok_or("Missing object resource")?;
        let uri = data.strings.resolve(*uri).to_owned();
        self.touch(obj)?;
        crate::core::resources::touch_resource(self, &uri)?;
        let carried = self.builder.data.carried.entry(obj).or_default();
        carried.assignment_state = state;
        if carried.assignment_state.is_none() && carried.initialization.is_none() {
            self.builder.data.carried.remove(&obj);
        }
        Ok(())
    }
    fn set_value(&mut self, id: &str, field: &str, value: Option<Value>) -> StorageResult<()> {
        self.assign_value(id, field, value, ReferenceEncoding::Wire)
    }

    fn append_value(&mut self, id: &str, field: &str, value: Value) -> StorageResult<()> {
        self.append_item(id, field, value)
    }

    fn set_resolved_value(
        &mut self,
        id: &str,
        field: &str,
        value: Option<Value>,
    ) -> StorageResult<()> {
        self.assign_value(id, field, value, ReferenceEncoding::Identity)
    }

    fn resolve_reference(
        &mut self,
        id: &str,
        field: &str,
        index: Option<usize>,
        target: &str,
    ) -> StorageResult<()> {
        self.resolve_item(id, field, index, target)
    }

    fn remove(&mut self, id: &str) -> StorageResult<()> {
        self.active()?;
        let Some(obj) = self.object(id) else {
            return Ok(());
        };
        let old = self.element(obj)?;
        let identity = self.builder.data.header(obj).ok_or("Missing object")?.id;
        let metamodel = self.builder.metamodel.clone();
        self.touch(obj)?;
        // Match the raw store contract. Checked deletion detaches incoming
        // and opposite edges through the shared structural edit path.
        crate::core::resources::element_delta(self, Some(metamodel.schema()), Some(&old), None)?;
        self.index_row(obj, false);
        self.builder.data.by_id.remove(&identity);
        Ok(())
    }

    fn release_unloaded_values(&mut self) -> StorageResult<()> {
        let frame = self
            .frames
            .last_mut()
            .ok_or("Storage write requires a transaction")?;
        frame.release_unloaded_values = true;
        Ok(())
    }
}

fn is_live(data: &ModelData, obj: ObjId) -> bool {
    data.header(obj)
        .is_some_and(|header| data.by_id.get(&header.id) == Some(&obj))
}

/// Make the objects after the prepared ones dense and resolve their
/// identity references, leaving the prepared objects where they are: what
/// `pack_arena` does for a link handoff, for a store whose prepared objects
/// (those of the validated model `prefix` describes) no write changed.
/// Those objects are all live, refer only to each other and hold no proxy,
/// so repacking them would reproduce them.
fn rebuild_tail(
    builder: &mut ModelBuilder,
    prefix: &Prefix,
    identity_proxies: &IdentityProxies,
) -> Result<(), FrozenError> {
    let prepared = builder.prepared;
    let metamodel = builder.metamodel.clone();
    let data = &mut builder.data;
    let old_headers = data.headers.split_off(prepared);
    let old_slots = data.slots.split_off(prefix.slots);
    let old_lists = data.lists.split_off(prefix.lists);
    let old_json = data.json.split_off(prefix.json);
    let old_proxies = std::mem::take(&mut data.proxies);
    // Give each live object its dense row; `by_id` then answers every
    // identity with the row it will have.
    let mut live = Vec::with_capacity(old_headers.len());
    for (offset, header) in old_headers.iter().enumerate() {
        let old = ObjId((prepared + offset) as u32);
        let carried = data.carried.remove(&old);
        let implicit = data.implicit.remove(&old);
        if data.by_id.get(&header.id) != Some(&old) {
            continue;
        }
        let next = ObjId(
            u32::try_from(prepared + live.len()).map_err(|_| FrozenError::Capacity("objects"))?,
        );
        data.by_id.insert(header.id, next);
        if let Some(carried) = carried {
            data.carried.insert(next, carried);
        }
        if implicit {
            data.implicit.insert(next);
        }
        live.push((header, next));
    }
    let tail = Tail {
        prepared,
        prefix,
        headers: &old_headers,
        lists: &old_lists,
        json: &old_json,
        proxies: &old_proxies,
        identity_proxies,
    };
    for (header, next) in live {
        let info = metamodel
            .class_info(header.class)
            .ok_or(FrozenError::UnknownClass)?;
        let base = (header.slots as usize)
            .checked_sub(prefix.slots)
            .ok_or(FrozenError::UnknownObject)?;
        let values = old_slots
            .get(base..base + info.stored.len())
            .ok_or(FrozenError::UnknownObject)?;
        let slots = u32::try_from(data.slots.len()).map_err(|_| FrozenError::Capacity("slots"))?;
        data.headers.push(Header {
            slots,
            container: u32::MAX,
            containing: u32::MAX,
            ..header.clone()
        });
        for &slot in values {
            let slot = match slot {
                Slot::Many(list) => {
                    let items = tail
                        .list(data, list)?
                        .into_iter()
                        .map(|item| tail.atom(data, next, item))
                        .collect::<Result<Vec<_>, _>>()?;
                    new_list(data, items)?
                }
                slot => tail.atom(data, next, slot)?,
            };
            data.slots.push(slot);
        }
    }
    Ok(())
}

/// The arena suffix `rebuild_tail` replaces.
struct Tail<'a> {
    prepared: usize,
    prefix: &'a Prefix,
    headers: &'a [Header],
    lists: &'a [Vec<Slot>],
    json: &'a [Value],
    proxies: &'a [Proxy],
    identity_proxies: &'a IdentityProxies,
}

impl Tail<'_> {
    fn list(&self, data: &ModelData, list: u32) -> Result<Vec<Slot>, FrozenError> {
        let list = list as usize;
        match list.checked_sub(self.prefix.lists) {
            Some(index) => self.lists.get(index),
            None => data.lists.get(list),
        }
        .cloned()
        .ok_or(FrozenError::UnknownObject)
    }

    /// The row a live identity has, as a reference.
    fn identity(
        &self,
        data: &ModelData,
        identity: super::intern::Sym,
    ) -> Result<Slot, FrozenError> {
        data.by_id
            .get(&identity)
            .map(|row| Slot::Ref(*row))
            .ok_or_else(|| FrozenError::Identity(data.strings.resolve(identity).to_owned()))
    }

    fn atom(&self, data: &mut ModelData, owner: ObjId, slot: Slot) -> Result<Slot, FrozenError> {
        match slot {
            // Every prepared object is live at its own row.
            Slot::Ref(row) if row.index() < self.prepared => Ok(slot),
            Slot::Ref(row) => {
                let header = self
                    .headers
                    .get(row.index() - self.prepared)
                    .ok_or(FrozenError::UnknownObject)?;
                self.identity(data, header.id)
            }
            Slot::Proxy(index) => {
                let proxy = self
                    .proxies
                    .get(index as usize)
                    .ok_or(FrozenError::UnknownObject)?;
                if self.identity_proxies.contains(&index) {
                    return self.identity(data, proxy.spelling);
                }
                let index = u32::try_from(data.proxies.len())
                    .map_err(|_| FrozenError::Capacity("proxies"))?;
                data.proxies.push(Proxy {
                    owner,
                    feature: proxy.feature,
                    spelling: proxy.spelling,
                });
                Ok(Slot::Proxy(index))
            }
            Slot::Json(index) => {
                let index = index as usize;
                let value = match index.checked_sub(self.prefix.json) {
                    Some(index) => self.json.get(index),
                    None => data.json.get(index),
                }
                .cloned()
                .ok_or(FrozenError::UnknownObject)?;
                super::builder::json_slot(data, value)
            }
            Slot::Many(_) => Err(FrozenError::Unsupported("nested arena list")),
            slot => Ok(slot),
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum PackMode {
    Link,
    Residency,
}

struct PackedArena {
    builder: ModelBuilder,
    identity_proxies: IdentityProxies,
}

struct PackContext<'a> {
    old: &'a ModelData,
    identity_proxies: &'a IdentityProxies,
    mode: PackMode,
}

/// Repack live construction payloads in row order. Link handoff makes the
/// rows dense and resolves identities. Residency preserves cursors and can
/// retain identities whose objects are currently unavailable.
fn pack_arena(
    source: &ModelBuilder,
    identity_proxies: &IdentityProxies,
    mode: PackMode,
) -> Result<PackedArena, FrozenError> {
    let old = &source.data;
    let context = PackContext {
        old,
        identity_proxies,
        mode,
    };
    let capacity = match mode {
        PackMode::Link => old.by_id.len(),
        PackMode::Residency => old.headers.len(),
    };
    let mut packed = PackedArena {
        builder: ModelBuilder::with_capacity(source.metamodel.clone(), capacity),
        identity_proxies: IdentityProxies::default(),
    };
    let mut order = Vec::with_capacity(old.by_id.len());
    for (index, header) in old.headers.iter().enumerate() {
        let row = ObjId(index as u32);
        if !is_live(old, row) {
            if mode == PackMode::Residency {
                // Dead headers preserve their positions and hold no payload
                // identity. Live references to them are re-encoded below.
                packed.builder.data.headers.push(Header {
                    class: header.class,
                    id: packed.builder.data.strings.intern("")?,
                    resource: 0,
                    slots: 0,
                    container: u32::MAX,
                    containing: u32::MAX,
                    start: 0,
                    end: 0,
                });
            }
            continue;
        }
        let resource = old
            .resources
            .get(header.resource as usize)
            .ok_or(FrozenError::UnknownObject)?;
        let next = packed.builder.create_at(
            old.strings.resolve(header.id),
            header.class,
            old.strings.resolve(*resource),
            header.start,
            header.end,
        )?;
        order.push((row, next));
        if mode == PackMode::Link && index < source.prepared {
            packed.builder.prepared += 1;
        }
    }
    if mode == PackMode::Residency {
        packed.builder.prepared = source.prepared;
    }
    for (row, next) in order {
        let header = old.header(row).ok_or(FrozenError::UnknownObject)?;
        let info = source
            .metamodel
            .class_info(header.class)
            .ok_or(FrozenError::UnknownClass)?;
        let base = header.slots as usize;
        let values = old
            .slots
            .get(base..base + info.stored.len())
            .ok_or(FrozenError::UnknownObject)?;
        let destination = packed
            .builder
            .data
            .headers
            .get_mut(next.index())
            .ok_or(FrozenError::UnknownObject)?;
        if mode == PackMode::Residency {
            destination.container = header.container;
            destination.containing = header.containing;
        }
        let destination = destination.slots as usize;
        for (offset, (&slot, &feature)) in values.iter().zip(&info.stored).enumerate() {
            let slot = match slot {
                Slot::Many(list) => {
                    let items = old
                        .lists
                        .get(list as usize)
                        .ok_or(FrozenError::UnknownObject)?;
                    let items = items
                        .iter()
                        .copied()
                        .map(|item| context.atom(&mut packed, next, feature, item))
                        .collect::<Result<Vec<_>, _>>()?;
                    new_list(&mut packed.builder.data, items)?
                }
                slot => context.atom(&mut packed, next, feature, slot)?,
            };
            *packed
                .builder
                .data
                .slots
                .get_mut(destination + offset)
                .ok_or(FrozenError::UnknownObject)? = slot;
        }
        if let Some(carried) = old.carried.get(&row) {
            packed.builder.data.carried.insert(next, carried.clone());
        }
        if old.implicit.contains(&row) {
            packed.builder.data.implicit.insert(next);
        }
    }
    Ok(packed)
}

impl PackContext<'_> {
    fn atom(
        &self,
        packed: &mut PackedArena,
        owner: ObjId,
        feature: super::FeatureId,
        slot: Slot,
    ) -> Result<Slot, FrozenError> {
        let old = self.old;
        match slot {
            Slot::Ref(row) => {
                let header = old.header(row).ok_or(FrozenError::UnknownObject)?;
                self.identity(packed, owner, feature, old.strings.resolve(header.id))
            }
            Slot::Proxy(index) => {
                let proxy = old
                    .proxies
                    .get(index as usize)
                    .ok_or(FrozenError::UnknownObject)?;
                let spelling = old.strings.resolve(proxy.spelling);
                if self.identity_proxies.contains(&index) {
                    self.identity(packed, owner, proxy.feature, spelling)
                } else {
                    proxy_slot(
                        &mut packed.builder.data,
                        owner,
                        proxy.feature,
                        spelling,
                        None,
                    )
                }
            }
            Slot::Str(symbol) => Ok(Slot::Str(
                packed
                    .builder
                    .data
                    .strings
                    .intern(old.strings.resolve(symbol))?,
            )),
            Slot::Json(index) => {
                let value = old
                    .json
                    .get(index as usize)
                    .ok_or(FrozenError::UnknownObject)?;
                super::builder::json_slot(&mut packed.builder.data, value.clone())
            }
            Slot::Many(_) => Err(FrozenError::Unsupported("nested arena list")),
            slot => Ok(slot),
        }
    }

    fn identity(
        &self,
        packed: &mut PackedArena,
        owner: ObjId,
        feature: super::FeatureId,
        identity: &str,
    ) -> Result<Slot, FrozenError> {
        let data = &packed.builder.data;
        if let Some(row) = data
            .strings
            .get(identity)
            .and_then(|id| data.by_id.get(&id))
        {
            return Ok(Slot::Ref(*row));
        }
        if self.mode == PackMode::Residency {
            return proxy_slot(
                &mut packed.builder.data,
                owner,
                feature,
                identity,
                Some(&mut packed.identity_proxies),
            );
        }
        Err(FrozenError::Identity(identity.to_owned()))
    }
}

fn proxy_slot(
    data: &mut ModelData,
    owner: ObjId,
    feature: super::FeatureId,
    spelling: &str,
    identities: Option<&mut IdentityProxies>,
) -> Result<Slot, FrozenError> {
    let spelling = data.strings.intern(spelling)?;
    let index = u32::try_from(data.proxies.len()).map_err(|_| FrozenError::Capacity("proxies"))?;
    data.proxies.push(Proxy {
        owner,
        feature,
        spelling,
    });
    if let Some(identities) = identities {
        identities.insert(index);
    }
    Ok(Slot::Proxy(index))
}

impl FrozenStore {
    fn put_with(&mut self, element: Element, encoding: ReferenceEncoding) -> StorageResult<()> {
        self.active()?;
        if element.id.is_empty() {
            return Err("Empty model identity".into());
        }
        let mm = self.builder.metamodel.clone();
        let class = mm.class(&element.class).ok_or("Unknown class")?;
        let info = mm.class_info(class).ok_or("Unknown class")?;
        if *info.identity != *element.type_identity {
            return Err("Model type identity does not match schema".into());
        }
        let old = self.get(&element.id)?;
        if old.as_ref() == Some(&element) {
            return Ok(());
        }
        let obj = match self.object(&element.id) {
            Some(obj) => {
                if let Some(before) = old.as_ref().filter(|before| before.class != element.class) {
                    if info.abstract_type {
                        return Err(format!(
                            "Cannot instantiate abstract class {}",
                            element.class
                        ));
                    }
                    return crate::core::storage::atomic(self, |store| {
                        store.touch(obj)?;
                        store.index_row(obj, false);
                        let data = &mut store.builder.data;
                        let base = u32::try_from(data.slots.len()).map_err(|_| "Slot capacity")?;
                        data.slots
                            .extend(std::iter::repeat_n(Slot::Default, info.stored.len()));
                        let header = data.headers.get_mut(obj.index()).ok_or("Missing object")?;
                        header.class = class;
                        header.slots = base;
                        store.write(obj, &element, encoding)?;
                        store.index_row(obj, true);
                        crate::core::resources::element_delta(
                            store,
                            Some(mm.schema()),
                            Some(before),
                            Some(&element),
                        )?;
                        Ok(())
                    });
                }
                self.touch(obj)?;
                obj
            }
            None => self.create_row(
                &element.id,
                class,
                &element.resource,
                element.start,
                element.end,
            )?,
        };
        self.index_row(obj, false);
        let written = self.write(obj, &element, encoding);
        self.index_row(obj, true);
        written?;
        let schema = mm.schema();
        crate::core::resources::element_delta(self, Some(schema), old.as_ref(), Some(&element))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn resource_fixture() -> StorageResult<crate::definitions::schema::Schema> {
        crate::compile_rcore(
            r#"rcore 1; package records version "1";
               metamodel "urn:records" prefix r { class Node {} }"#,
        )
        .map(|package| package.schema().clone())
        .map_err(|error| error.to_string())
    }

    #[test]
    fn imported_resource_records_require_exact_uri_admission() -> StorageResult<()> {
        let schema = resource_fixture()?;
        let valid = ResourceRecord::new("urn:valid")?;
        let mut invalid = valid.clone();
        invalid.uri = "urn:valid#fragment".into();
        let mut model = crate::core::model::Model {
            schema_revision: schema.revision.clone(),
            ..Default::default()
        };
        model.resources.insert(valid.uri.clone(), valid.clone());
        model.resources.insert(invalid.uri.clone(), invalid.clone());
        let mut store = FrozenStore::from_materialized(&schema, &model)?;
        let before = storage::export(&store)?;
        assert!(store.validated_resource_uris.is_empty());
        assert!(
            storage::atomic(&mut store, |store| {
                store.set_resource_record(invalid.clone())
            })
            .is_err()
        );
        assert_eq!(storage::export(&store)?, before);
        assert!(store.validated_resource_uris.is_empty());
        storage::atomic(&mut store, |store| store.set_resource_record(valid.clone()))?;
        assert!(store.validated_resource_uris.contains("urn:valid"));
        // A cached prefix does not admit another exact identity.
        assert!(
            storage::atomic(&mut store, |store| { store.set_resource_record(invalid) }).is_err()
        );
        assert_eq!(storage::export(&store)?, before);
        Ok(())
    }

    #[test]
    fn resource_uri_admission_preserves_nested_record_rollback() -> StorageResult<()> {
        let schema = resource_fixture()?;
        let mut store = FrozenStore::for_schema(&schema).map_err(|e| e.to_string())?;
        let mut initial = ResourceRecord::new("urn:existing")?;
        initial.content_revision = 17;
        initial.diagnostics.push("retained diagnostic".into());
        initial.source = Some(crate::core::resources::SourceCorrespondence {
            uri: "file:///source.sml".into(),
            revision: "original".into(),
        });
        storage::atomic(&mut store, |store| {
            store.set_resource_record(initial.clone())
        })?;
        let before = storage::export(&store)?;
        let outer = store.begin()?;
        for _ in 0..8 {
            crate::core::resources::touch_resource(&mut store, &initial.uri)?;
        }
        let updated = store
            .resource_record(&initial.uri)?
            .ok_or("Missing record")?;
        assert_eq!(updated.content_revision, 25);
        assert!(updated.source.is_none() && updated.diagnostics.is_empty());
        let inner = store.begin()?;
        store.remove_resource_record(&initial.uri)?;
        store.set_resource_record(ResourceRecord::new("urn:temporary")?)?;
        store.commit(&inner)?;
        store.rollback(&outer)?;
        assert_eq!(storage::export(&store)?, before);
        assert!(!store.validated_resource_uris.contains("urn:temporary"));
        // Rollback can conservatively drop a proof. The next update still
        // validates and preserves the restored metadata and overflow behavior.
        storage::atomic(&mut store, |store| {
            let mut record = initial.clone();
            record.content_revision = u64::MAX;
            store.set_resource_record(record)
        })?;
        let before = storage::export(&store)?;
        assert!(
            storage::atomic(&mut store, |store| {
                crate::core::resources::touch_resource(store, &initial.uri)
            })
            .is_err()
        );
        assert_eq!(storage::export(&store)?, before);
        Ok(())
    }

    #[test]
    fn resource_uri_proofs_do_not_accumulate_rolled_back_identities() -> StorageResult<()> {
        let schema = resource_fixture()?;
        let mut store = FrozenStore::for_schema(&schema).map_err(|e| e.to_string())?;
        for i in 0..32 {
            let checkpoint = store.begin()?;
            store.set_resource_record(ResourceRecord::new(format!("urn:temporary:{i}"))?)?;
            store.rollback(&checkpoint)?;
        }
        assert!(store.validated_resource_uris.is_empty());
        storage::atomic(&mut store, |store| {
            store.set_resource_record(ResourceRecord::new("urn:retained")?)
        })?;
        let mut fork = store.fork_arena()?;
        storage::atomic(&mut fork, |store| {
            store.remove_resource_record("urn:retained")
        })?;
        assert!(fork.validated_resource_uris.is_empty());
        assert!(store.validated_resource_uris.contains("urn:retained"));
        assert!(store.resource_record("urn:retained")?.is_some());
        Ok(())
    }

    #[test]
    fn archive_views_borrow_values_and_preserve_explicit_defaults() -> StorageResult<()> {
        let package = crate::compile_rcore(
            r#"rcore 1; package archive version "1";
               metamodel "urn:archive" prefix a {
                   class Node { attr label: String [0,1] default "seed"; }
               }"#,
        )
        .map_err(|error| error.to_string())?;
        let schema = package.schema();
        let class = schema
            .classes
            .keys()
            .next()
            .ok_or("Missing fixture class")?;
        let mut store = FrozenStore::new(Arc::new(
            Metamodel::freeze(schema).map_err(|error| error.to_string())?,
        ));
        storage::atomic(&mut store, |store| {
            store.create_element(schema, "node", class, "fixture", 0, 4)?;
            store.set_value("node", "label", Some(json!("seed")))
        })?;
        let expected = store.get("node")?.ok_or("Missing fixture row")?;
        let handle = store.handle("node").ok_or("Missing fixture handle")?;
        store.inspect_handle(&handle, &mut |view| {
            assert_eq!(view.to_element(), expected);
            assert!(
                view.assignment_state()
                    .is_some_and(|state| state.explicitly_set("label"))
            );
            let ty = view.type_id().ok_or("Missing archive type")?;
            let label = store
                .schema_descriptor()?
                .type_descriptor(&ty)?
                .feature("label")
                .ok_or("Missing archive label feature")?
                .id();
            assert_eq!(
                view.value_by_feature(&label)?
                    .and_then(|value| value.as_str()),
                Some("seed")
            );
            Ok(())
        })
    }

    #[test]
    fn inspect_projects_arena_values_and_reference_states() -> StorageResult<()> {
        let package = crate::compile_rcore(
            r#"rcore 1; package views version "1";
               metamodel "urn:views" prefix v {
                   class Node {
                       attr absent: String [0,1];
                       attr label: String [0,1];
                       ref links: Node [0,*];
                   }
               }"#,
        )
        .map_err(|error| error.to_string())?;
        let schema = package.schema();
        let class = schema
            .classes
            .keys()
            .next()
            .ok_or("Missing fixture class")?;
        let mut store = FrozenStore::for_schema(schema).map_err(|error| error.to_string())?;
        storage::atomic(&mut store, |store| {
            store.create_element(schema, "owner", class, "fixture", 1, 8)?;
            store.create_element(schema, "?target", class, "fixture", 9, 12)?;
            store.set_value("owner", "absent", None)?;
            store.set_value("owner", "label", Some(json!("arena text")))?;
            store.set_value("owner", "links", Some(json!(["?missing"])))
        })?;
        let pending = store.get("owner")?.ok_or("Missing fixture row")?;
        assert!(store.inspect("owner", &mut |view| {
            assert_eq!(view.to_element(), pending);
            let links = view.value("links").ok_or("Missing links")?;
            let list = links.collection().ok_or("Missing link collection")?;
            let link = list.get(0).ok_or("Missing pending reference")?;
            assert!(matches!(
                link.reference(),
                Some(storage::ReferenceView::Unresolved {
                    spelling: "missing"
                })
            ));
            assert_eq!(
                serde_json::to_value(links).map_err(|error| error.to_string())?,
                json!(["?missing"])
            );
            Ok(())
        })?);

        storage::atomic(&mut store, |store| {
            store.set_resolved_value("owner", "links", Some(json!(["?target"])))
        })?;
        let resolved = store.get("owner")?.ok_or("Missing fixture row")?;
        assert!(store.inspect("owner", &mut |view| {
            assert_eq!(view.to_element(), resolved);
            assert!(view.value("absent").is_none());
            assert_eq!(
                view.value("label").and_then(|label| label.as_str()),
                Some("arena text")
            );
            let links = view
                .value("links")
                .and_then(|links| links.collection())
                .ok_or("Missing links")?;
            let link = links.get(0).ok_or("Missing resolved reference")?;
            assert_eq!(link.as_str(), Some("?target"));
            assert!(matches!(
                link.reference(),
                Some(storage::ReferenceView::Resolved(_))
            ));
            Ok(())
        })?);
        Ok(())
    }

    #[test]
    fn synchronous_audit_preserves_staged_scalar_validation() -> StorageResult<()> {
        let package = crate::compile_rcore(
            r#"rcore 1; package scalars version "1";
               metamodel "urn:scalars" prefix s {
                   class Node { attr number: Int32 [1,1]; }
               }"#,
        )
        .map_err(|error| error.to_string())?;
        let schema = package.schema();
        let class = schema
            .classes
            .keys()
            .next()
            .ok_or("Missing fixture class")?;
        let mut store = FrozenStore::for_schema(schema).map_err(|error| error.to_string())?;
        storage::atomic(&mut store, |store| {
            store.create_element(schema, "node", class, "fixture", 0, 0)
        })?;
        for (value, valid) in [
            (json!(123), true),
            (json!("123"), false),
            (json!(i64::from(i32::MAX) + 1), false),
            (Value::Null, false),
        ] {
            storage::atomic(&mut store, |store| {
                store.set_value("node", "number", Some(value))
            })?;
            let staged = (|| {
                let mut audit = storage::StructuralPreparation::ownership();
                while !audit.advance(&store, schema, None, None)? {}
                audit.finish().map(|_| ())
            })();
            assert_eq!(staged.is_ok(), valid);
            assert_eq!(storage::validate(&store, schema), staged);
        }
        Ok(())
    }

    #[test]
    fn one_pass_acceptance_agrees_with_the_staged_audit() -> StorageResult<()> {
        let package = crate::compile_rcore(
            r#"rcore 1; package trees version "1";
               metamodel "urn:trees" prefix t {
                   class Node {
                       attr name: String [1,1];
                       ref children: Node [0,*] containment;
                       ref peer: Node [0,1];
                       ref peers: Node [0,2];
                   }
               }"#,
        )
        .map_err(|error| error.to_string())?;
        let schema = package.schema();
        let class = schema
            .classes
            .keys()
            .next()
            .ok_or("Missing fixture class")?;
        let mut store = FrozenStore::for_schema(schema).map_err(|error| error.to_string())?;
        storage::atomic(&mut store, |store| {
            for id in ["a", "b", "c"] {
                store.create_element(schema, id, class, "fixture", 0, 0)?;
                store.set_value(id, "name", Some(json!(id)))?;
            }
            store.append_value("a", "children", json!("b"))?;
            store.append_value("a", "children", json!("c"))?;
            store.set_value("b", "peer", Some(json!("c")))?;
            store.set_value("c", "peers", Some(json!(["a", "b"])))
        })?;
        let staged = |store: &FrozenStore| {
            let mut audit = storage::StructuralPreparation::ownership();
            while !audit.advance(store, schema, None, None)? {}
            audit.finish().map(|_| ())
        };
        // The valid graph is in the forms the store accepts in one pass.
        assert!(store.may_accept_structure());
        assert_eq!(staged(&store), Ok(()));
        assert_eq!(storage::validate(&store, schema), Ok(()));
        type Mutation = fn(&mut FrozenStore) -> StorageResult<()>;
        let mutations: [(Mutation, bool); 9] = [
            (|s| s.set_value("b", "name", None), false),
            (|s| s.set_value("b", "name", Some(json!(5))), false),
            (|s| s.set_value("b", "peer", Some(json!("missing"))), false),
            (
                |s| s.set_value("c", "peers", Some(json!(["b", "b"]))),
                false,
            ),
            (
                |s| s.set_value("c", "peers", Some(json!(["a", "b", "c"]))),
                false,
            ),
            (|s| s.append_value("b", "children", json!("c")), false),
            (|s| s.append_value("b", "children", json!("a")), false),
            (|s| s.set_value("a", "children", Some(json!(["b"]))), true),
            (|s| s.set_value("b", "peer", None), true),
        ];
        for (mutate, valid) in mutations {
            let checkpoint = store.begin()?;
            mutate(&mut store)?;
            let expected = staged(&store);
            assert_eq!(expected.is_ok(), valid, "{expected:?}");
            // Same outcome and, when rejected, the staged audit's own error.
            assert_eq!(storage::validate(&store, schema), expected);
            store.rollback(&checkpoint)?;
        }
        assert_eq!(storage::validate(&store, schema), Ok(()));
        Ok(())
    }

    #[test]
    fn unloading_reclaims_payloads_only_on_commit_and_preserves_live_rows() -> StorageResult<()> {
        let package = crate::compile_rcore(
            r#"rcore 1; package residency version "1";
               metamodel "urn:residency" prefix r {
                   class Node { attr label: String [0,1]; ref link: Node [0,1]; }
               }"#,
        )
        .map_err(|error| error.to_string())?;
        let schema = package.schema();
        let class = schema
            .classes
            .keys()
            .next()
            .ok_or("Missing fixture class")?;
        let mut store = FrozenStore::for_schema(schema).map_err(|error| error.to_string())?;
        storage::atomic(&mut store, |store| {
            store.create_element(schema, "unloaded", class, "first", 0, 0)?;
            store.create_element(schema, "resident", class, "second", 0, 0)?;
            store.set_value("unloaded", "label", Some(json!("retired payload")))?;
            store.set_value("resident", "link", Some(json!("unloaded")))
        })?;
        let initial = storage::export(&store)?;
        let resident = store.object("resident").ok_or("Missing fixture object")?;
        let checkpoint = store.begin()?;
        store.remove("unloaded")?;
        store.release_unloaded_values()?;
        assert!(store.builder.data.strings.get("retired payload").is_some());
        store.rollback(&checkpoint)?;
        assert_eq!(storage::export(&store)?, initial);

        storage::atomic(&mut store, |store| {
            storage::atomic(store, |store| {
                store.remove("unloaded")?;
                store.release_unloaded_values()
            })?;
            assert!(store.builder.data.strings.get("retired payload").is_some());
            Ok(())
        })?;
        assert!(store.builder.data.strings.get("retired payload").is_none());
        assert_eq!(store.object("resident"), Some(resident));
        assert_eq!(store.object("unloaded"), None);
        assert_eq!(store.value("resident", "link")?, Some(json!("unloaded")));
        assert_eq!(
            store.query(Query::Value("link", &json!("unloaded")))?,
            ["resident"]
        );
        Ok(())
    }
}
