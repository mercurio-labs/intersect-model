//! Handwritten storage contracts and shared structural operations.
//! Adapters own I/O; core never opens files or selects a clock/scheduler.
use crate::core::model::{Element, Model};
pub use intersect_model::definitions::conversion::InvocationContext;
mod assignment;
mod checkpoint;
mod collection;
mod json_preflight;
pub use json_preflight::{JsonDecodeBudget, decode_json_observed};
mod relocation;
mod reparent;
pub use reparent::{ContainmentDestination, ReparentRequest};
mod deletion;
pub use deletion::{DeleteRequest, DeleteSelection, IncomingReferences, StructuralContext};
mod indexing;
pub use indexing::IndexedModelPreparation;
mod draft_construction;
mod initialization;
mod snapshot_initialization;
pub use draft_construction::{
    DraftClassContract, DraftConstructionContext, DraftConstructionReceipt,
    DraftConstructionSource, DraftConstructionStore, DraftDisposition, DraftMemberDisposition,
    DraftPhysicalDefaults,
};
#[cfg(feature = "xmi")]
pub(crate) use initialization::FreshReadonlySnapshot;
pub use initialization::{
    ClassInitializationSelector, ClassInitializer, Construction, InitialValue,
    InitializationBinding, InitializationContext, SnapshotInitializationSelector,
};
pub(crate) use snapshot_initialization::ReadonlyDecodeProof;
#[cfg(feature = "xmi")]
pub(crate) use snapshot_initialization::VerifiedReadonlyInitialization;
mod setting;
pub use assignment::{
    is_explicitly_set, is_set, is_set_with_derived, is_set_with_derived_and_conversions, unset,
    unset_with_conversions,
};
use assignment::{update_assignment, validate_assignment_state};
pub use collection::{CollectionEdit, edit_collection, edit_collection_with_conversions};
pub use setting::{ComputedEdit, ComputedSettings, SettingContext, SettingEdit, SettingValues};
mod feature_shape;
use feature_shape::FeatureShape;
mod construction_shapes;
mod structural_preparation;
pub use structural_preparation::{DraftReferencePolicy, StructuralPreparation};
mod structure_acceptance;
pub use structure_acceptance::{FeatureRule, FeatureRuleKind, StructureFacts};
mod pending_source;
pub use pending_source::PendingSourceInputs;
mod ownership;
mod pending_containment;
pub use ownership::{
    OwnershipIndex, validate_ownership_with_conversions,
    validate_ownership_with_conversions_observed,
};
pub use pending_containment::{ContainmentNavigation, PendingContainmentIndex};
mod projection;
mod target_membership;
pub(crate) use projection::{SemanticFeaturePreparation, SemanticFeatureSelection};
pub use projection::{StoredFeaturePreparation, StoredFeatureProjection, StoredFeatureSelection};
pub(crate) use target_membership::{TargetMembership, advance_target_membership};
mod descriptors;
mod immutable_structural_audit;
mod structural_values;
pub use immutable_structural_audit::{ImmutableStructuralAudit, with_immutable_structural_audit};
mod validation;
pub use structural_values::{
    BorrowedStructuralValues, StructuralValueContext, StructuralValueRequest,
    StructuralValueSelector, validate_native_with_structural_values,
    validate_with_structural_values,
};
mod views;
pub use checkpoint::{Checkpoint, CheckpointIssuer};
pub use descriptors::{FeatureDescriptor, FeatureId, SchemaDescriptor, TypeDescriptor, TypeId};
use intersect_model::definitions::{
    conversion::ConversionRegistry,
    schema::{FeatureLookup, Schema},
};
use serde_json::Value;
use std::{
    collections::{BTreeMap, BTreeSet},
    hash::{Hash, Hasher},
    sync::Arc,
};
pub(crate) use validation::DraftValidationPlan;
pub use views::{CollectionView, ObjectView, ReferenceView, ValueView};

/// Scoped, nonserializable object incarnation. Deleting or rolling back an
/// insertion invalidates its handles. Reloading an identity creates a new one.
#[derive(Clone, Debug)]
pub struct ObjectHandle {
    pub(super) scope: Arc<()>,
    pub(super) row: u64,
}
impl PartialEq for ObjectHandle {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.scope, &other.scope) && self.row == other.row
    }
}
impl Eq for ObjectHandle {}
impl Hash for ObjectHandle {
    fn hash<H: Hasher>(&self, state: &mut H) {
        Arc::as_ptr(&self.scope).hash(state);
        self.row.hash(state);
    }
}

/// Secondary indexes affect lookup cost, never query results. Class and resource
/// membership are always indexed. The policy is fixed for a store's lifetime.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum IndexPolicy {
    #[default]
    AllValues,
    SelectedValues(BTreeSet<String>),
}

impl IndexPolicy {
    pub fn selected(fields: impl IntoIterator<Item = impl Into<String>>) -> Self {
        Self::SelectedValues(fields.into_iter().map(Into::into).collect())
    }

    pub(super) fn contains(&self, field: &str) -> bool {
        match self {
            Self::AllValues => true,
            Self::SelectedValues(fields) => fields.contains(field),
        }
    }
}

/// Stable insertion-order cursor, independent of element identity.
pub type Row = (u64, Element);
pub type StorageResult<T> = Result<T, String>;
#[derive(Clone, Copy)]
pub enum Query<'a> {
    Class(&'a str),
    Resource(&'a str),
    Value(&'a str, &'a Value),
}
/// A bounded batch of storage candidates, not a typed inverse-query result.
/// A batch can be empty while the scan still has candidates to visit.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct QueryCandidates {
    pub rows: Vec<(u64, String)>,
    /// Last consumed row. Continue exclusively after this cursor.
    pub after: Option<u64>,
    pub exhausted: bool,
    /// False means candidates still require storage-value membership checks.
    /// Even exact index hits require schema/type qualification by the caller.
    pub exact: bool,
}
/// Reads must observe the current transaction. Pages are ordered by cursor;
/// query results are distinct IDs in insertion order. Values index each array
/// member, or a non-null scalar, using its canonical serde_json representation.
pub trait ModelRead {
    fn resource_view(
        &self,
        uri: &str,
    ) -> StorageResult<Option<crate::core::resources::ResourceView<'_, Self>>>
    where
        Self: Sized,
    {
        crate::core::resources::resource_view(self, uri)
    }
    fn resource_records(
        &self,
    ) -> StorageResult<BTreeMap<String, crate::core::resources::ResourceRecord>> {
        Err("Explicit resource records are not supported by this storage adapter".into())
    }
    fn resource_record(
        &self,
        uri: &str,
    ) -> StorageResult<Option<crate::core::resources::ResourceRecord>> {
        Ok(self.resource_records()?.remove(uri))
    }
    /// Whether an unloaded resource reserves the identity. Same answer as
    /// searching `resource_records`; stores may answer without copying them.
    fn identity_reserved(&self, id: &str) -> StorageResult<bool> {
        Ok(self.resource_records()?.values().any(|record| {
            record
                .unloaded
                .as_ref()
                .is_some_and(|contents| contents.identities.contains(id))
        }))
    }
    fn revision(&self) -> StorageResult<String>;
    /// Borrow the revision during the callback. The compatibility default keeps
    /// revision()'s owned-result boundary; shipped adapters borrow their value.
    fn inspect_revision(&self, f: &mut dyn FnMut(&str) -> StorageResult<()>) -> StorageResult<()> {
        f(&self.revision()?)
    }
    /// Export canonical rows and resource records under a snapshot limit and
    /// the caller's observation budget. Adapters must account for allocations
    /// before materializing owned data.
    fn export_snapshot_observed(
        &self,
        _max_objects: usize,
        _observation: &InvocationContext<'_>,
    ) -> StorageResult<Model> {
        Err("Observed snapshots are not supported by this storage adapter".into())
    }
    fn get(&self, id: &str) -> StorageResult<Option<Element>>;
    fn class(&self, id: &str) -> StorageResult<Option<String>>;
    /// Borrow just the class name during the callback. Shipped adapters avoid
    /// an owned class result or complete element projection. The compatibility
    /// default retains class()'s allocation boundary; no bounded-allocation
    /// capability is inferred for external adapters that use this default.
    fn inspect_class(
        &self,
        id: &str,
        f: &mut dyn FnMut(&str) -> StorageResult<()>,
    ) -> StorageResult<bool> {
        if let Some(class) = self.class(id)? {
            f(&class)?;
            Ok(true)
        } else {
            Ok(false)
        }
    }
    fn page(&self, after: Option<u64>, limit: usize) -> StorageResult<Vec<Row>>;
    fn query(&self, query: Query<'_>) -> StorageResult<Vec<String>>;
    /// Consume at most `limit` candidates in a stable read view, with at most
    /// one lookahead per ordered input stream (at most two streams). A zero limit
    /// rejects. Nonterminal batches must advance `after`, even with no rows.
    /// Implementations must not collect an unbounded query before paging it.
    /// The caller must not mutate the view between continuation steps.
    fn query_candidates(
        &self,
        _query: Query<'_>,
        _after: Option<u64>,
        _limit: usize,
    ) -> StorageResult<QueryCandidates> {
        Err("Bounded query candidates are not supported by this storage adapter".into())
    }
    /// Observe shipped adapter allocations before copying query keys/identities.
    /// The compatibility default keeps the adapter's existing allocation boundary.
    fn query_candidates_observed(
        &self,
        query: Query<'_>,
        after: Option<u64>,
        limit: usize,
        observation: &InvocationContext<'_>,
    ) -> StorageResult<QueryCandidates> {
        observation.checkpoint()?;
        self.query_candidates(query, after, limit)
    }
    /// Test one membership without materializing all matching IDs. Adapters may
    /// inspect an unindexed field directly without retaining an index for it.
    fn matches(&self, id: &str, query: Query<'_>) -> StorageResult<bool>;
    fn matches_observed(
        &self,
        id: &str,
        query: Query<'_>,
        observation: &InvocationContext<'_>,
    ) -> StorageResult<bool> {
        observation.checkpoint()?;
        self.matches(id, query)
    }
    /// Bounded identity traversal without materializing object properties.
    fn id_page(&self, after: Option<u64>, limit: usize) -> StorageResult<Vec<(u64, String)>> {
        Ok(self
            .page(after, limit)?
            .into_iter()
            .map(|(row, e)| (row, e.id))
            .collect())
    }
    /// The default observes entry only; it does not infer preflight for arbitrary
    /// compatibility adapters. Shipped adapters reserve identities before copies.
    fn id_page_observed(
        &self,
        after: Option<u64>,
        limit: usize,
        observation: &InvocationContext<'_>,
    ) -> StorageResult<Vec<(u64, String)>> {
        observation.checkpoint()?;
        self.id_page(after, limit)
    }
    fn value(&self, id: &str, field: &str) -> StorageResult<Option<Value>> {
        Ok(self.get(id)?.and_then(|mut e| e.values.remove(field)))
    }
    /// Whether this store can establish, from its own representation, that
    /// the staged structural audit prepared `facts` for would pass. False
    /// (the default) only means the audit decides; true must be exact.
    fn accepts_structure(&self, _facts: &dyn StructureFacts) -> bool {
        false
    }
    /// Cheap screen before an audit prepares facts for `accepts_structure`:
    /// false when this store would answer false whatever the facts are.
    fn may_accept_structure(&self) -> bool {
        false
    }
    /// Test whether a wire-projected atom represents stored lexical state.
    /// Adapters without typed evidence must leave this false.
    fn is_pending_reference(&self, _id: &str, _field: &str, _index: usize, _wire: &str) -> bool {
        false
    }
    /// Compatibility adapters only observe entry; owned decoding remains their boundary.
    fn value_observed(
        &self,
        id: &str,
        field: &str,
        observation: &InvocationContext<'_>,
    ) -> StorageResult<Option<Value>> {
        observation.checkpoint()?;
        self.value(id, field)
    }
    /// Tentative raw value loan; no semantic/model authority is granted.
    /// The callback must run exactly once when true is returned. Compatibility
    /// adapters retain their existing value/collection overrides and owned
    /// decoding; canonical memory can borrow without building JSON first.
    fn inspect_stored_value_observed(
        &self,
        id: &str,
        field: &str,
        many: bool,
        observation: &InvocationContext<'_>,
        consume: &mut dyn FnMut(ValueView<'_>) -> StorageResult<()>,
    ) -> StorageResult<bool> {
        if many {
            let value = Value::Array(self.collection_page_observed(
                id,
                field,
                0,
                usize::MAX,
                observation,
            )?);
            consume(ValueView::json(&value))?;
            Ok(true)
        } else if let Some(value) = self.value_observed(id, field, observation)? {
            consume(ValueView::json(&value))?;
            Ok(true)
        } else {
            Ok(false)
        }
    }
    /// Compatibility defaults do not establish preflighted collection allocation.
    fn collection_page_observed(
        &self,
        id: &str,
        field: &str,
        offset: usize,
        limit: usize,
        observation: &InvocationContext<'_>,
    ) -> StorageResult<Vec<Value>> {
        observation.checkpoint()?;
        self.collection_page(id, field, offset, limit)
    }
    fn value_len(&self, id: &str, field: &str) -> StorageResult<Option<usize>> {
        Ok(self
            .value(id, field)?
            .and_then(|v| v.as_array().map(Vec::len)))
    }
    fn collection_page(
        &self,
        id: &str,
        field: &str,
        offset: usize,
        limit: usize,
    ) -> StorageResult<Vec<Value>> {
        let value = self.value(id, field)?.ok_or("Missing collection feature")?;
        Ok(value
            .as_array()
            .ok_or("Multiple feature requires a list")?
            .iter()
            .skip(offset)
            .take(limit)
            .cloned()
            .collect())
    }
    /// Borrow object data for the duration of a read context. Callers must
    /// explicitly copy data to retain it. The callback bounds the borrow's
    /// lifetime, not its memory usage: this default materializes a complete
    /// Element, including its collections, and supplies a compatibility view
    /// without schema descriptors or distinct reference states. FrozenStore
    /// overrides this to borrow arena records; SQLite currently uses this
    /// default. Prefer value/collection projections for bounded persistent reads.
    fn inspect(
        &self,
        id: &str,
        f: &mut dyn FnMut(ObjectView<'_>) -> StorageResult<()>,
    ) -> StorageResult<bool> {
        if let Some(element) = self.get(id)? {
            f(ObjectView::from_element(&element))?;
            Ok(true)
        } else {
            Ok(false)
        }
    }
}
/// Raw persistence, not a semantic API. Writes require an active transaction.
/// Checkpoints are single-use, adapter-local, LIFO tokens. Adapters must make
/// each write atomic and preserve outer rollback after an inner commit.
/// A failed commit must leave its checkpoint available for rollback. Callers
/// must close any manually opened child frames before completing their parent.
pub trait ModelStore: ModelRead {
    /// Opt in to bounded validation while owned by a checked model facade.
    ///
    /// Return true only when model data cannot change through another handle,
    /// connection or interior mutation: all writes must require this exclusive
    /// mutable store reference. Creation must only insert the requested object,
    /// setters must only change their requested feature, and rebinding the same
    /// schema must not change existing model contents. Transaction rollback must
    /// restore those contents. The facade validates the initial store in full.
    /// Stores with external writers or unqualified implementations retain full
    /// per-operation validation by leaving this false.
    fn supports_incremental_validation(&self) -> bool {
        false
    }

    /// Whether this adapter currently has an open write transaction.
    fn is_transaction_active(&self) -> bool;
    fn begin(&mut self) -> StorageResult<Checkpoint>;
    fn commit(&mut self, checkpoint: &Checkpoint) -> StorageResult<()>;
    fn rollback(&mut self, checkpoint: &Checkpoint) -> StorageResult<()>;
    fn set_revision(&mut self, revision: &str) -> StorageResult<()>;
    fn set_resource_record(
        &mut self,
        _record: crate::core::resources::ResourceRecord,
    ) -> StorageResult<()> {
        Err("Explicit resource records are not supported by this storage adapter".into())
    }
    fn remove_resource_record(&mut self, _uri: &str) -> StorageResult<()> {
        Err("Explicit resource records are not supported by this storage adapter".into())
    }
    /// Advance a loaded resource's content revision and clear stale source
    /// correspondence and diagnostics. Missing records are created. Adapters
    /// may specialize storage while preserving the shared resource effects.
    fn touch_resource_record(&mut self, uri: &str) -> StorageResult<()> {
        crate::core::resources::touch_resource_compat(self, uri)
    }
    /// Add or remove one root of a registered resource in place, with the
    /// shared root-change effects. False (the default) leaves the change to
    /// the shared read, edit and replace of the record.
    fn change_resource_root(&mut self, _uri: &str, _id: &str, _root: bool) -> StorageResult<bool> {
        Ok(false)
    }
    fn put(&mut self, element: Element) -> StorageResult<()>;
    fn remove(&mut self, id: &str) -> StorageResult<()>;
    /// Arrange to release payloads retired by unload when the outer
    /// checkpoint commits. Stores that reclaim values on removal need no hook.
    fn release_unloaded_values(&mut self) -> StorageResult<()> {
        Ok(())
    }
    fn configure_schema(&mut self, schema: &Schema) -> StorageResult<()> {
        let revision = self.revision()?;
        if !revision.is_empty() && revision != schema.revision {
            return Err("Model belongs to a different schema revision".into());
        }
        self.set_revision(&schema.revision)
    }
    fn create_element(
        &mut self,
        schema: &Schema,
        id: &str,
        class: &str,
        resource: &str,
        start: usize,
        end: usize,
    ) -> StorageResult<()> {
        if !self.is_transaction_active() {
            return Err("Storage write requires a transaction".into());
        }
        let element = Element::new(schema, id, class, resource, start, end)?;
        initialization::insert_created_element(self, schema, element)
    }
    /// Replace retained construction evidence inside the current checkpoint.
    /// Raw persistence does not establish that a deserialized receipt is trusted.
    fn set_initialization(
        &mut self,
        id: &str,
        receipt: Option<intersect_model::definitions::initialization::InitializationReceipt>,
    ) -> StorageResult<()> {
        if !self.is_transaction_active() {
            return Err("Storage write requires a transaction".into());
        }
        let mut element = self.get(id)?.ok_or("Missing element")?;
        element.initialization = receipt;
        self.put(element)
    }
    /// Replace raw assignment evidence independently of stored values. This is
    /// not semantic eSet/eUnset; callers must validate it against their schema.
    fn set_assignment_state(
        &mut self,
        id: &str,
        state: Option<crate::core::model::AssignmentState>,
    ) -> StorageResult<()> {
        if !self.is_transaction_active() {
            return Err("Storage write requires a transaction".into());
        }
        let mut element = self.get(id)?.ok_or("Missing element")?;
        element.assignment_state = state;
        self.put(element)
    }
    fn set_value(&mut self, id: &str, field: &str, value: Option<Value>) -> StorageResult<()> {
        let mut element = self.get(id)?.ok_or("Missing element")?;
        if let Some(value) = value {
            element.values.insert(field.into(), value);
        } else {
            element.values.remove(field);
        }
        self.put(element)
    }
    /// Write a checked value. Reference strings here name target identities.
    fn set_resolved_value(
        &mut self,
        id: &str,
        field: &str,
        value: Option<Value>,
    ) -> StorageResult<()> {
        self.set_value(id, field, value)
    }
    fn append_value(&mut self, id: &str, field: &str, value: Value) -> StorageResult<()> {
        let mut current = self
            .value(id, field)?
            .unwrap_or_else(|| Value::Array(vec![]));
        current
            .as_array_mut()
            .ok_or("Multiple feature requires a list")?
            .push(value);
        self.set_value(id, field, Some(current))
    }
    fn truncate_values(&mut self, id: &str, field: &str, len: usize) -> StorageResult<()> {
        let mut current = self.value(id, field)?.ok_or("Missing feature")?;
        let list = current
            .as_array_mut()
            .ok_or("Multiple feature requires a list")?;
        if len > list.len() {
            return Err("Cannot extend a collection by truncating it".into());
        }
        list.truncate(len);
        self.set_value(id, field, Some(current))
    }
    fn set_span(&mut self, id: &str, start: usize, end: usize) -> StorageResult<()> {
        let mut element = self.get(id)?.ok_or("Missing element")?;
        element.start = start;
        element.end = end;
        self.put(element)
    }
    /// Store lexical reference state. Canonical memory storage keeps a distinct
    /// unresolved variant and requires a schema reference feature. This default
    /// uses the lossy compatibility spelling `?name`, with no field-type guard;
    /// it cannot distinguish that marker from a literal target identity. SQLite
    /// format 2 inherits this limitation. Use canonical memory storage when
    /// distinct unresolved state is required, including through validation.
    fn set_unresolved_reference(
        &mut self,
        id: &str,
        field: &str,
        spelling: &str,
        append: bool,
    ) -> StorageResult<()> {
        let value = Value::String(format!("?{spelling}"));
        if append {
            self.append_value(id, field, value)
        } else {
            self.set_value(id, field, Some(value))
        }
    }
    /// Write a target identity. Canonical memory storage checks the schema
    /// reference feature; this compatibility default only checks the value's
    /// shape/index. It does not establish target existence or conformance;
    /// shared structural validation is a separate operation.
    fn resolve_reference(
        &mut self,
        id: &str,
        field: &str,
        index: Option<usize>,
        target: &str,
    ) -> StorageResult<()> {
        resolve_reference_compat(self, id, field, index, target)
    }
}

/// The existing wire-value compatibility operation, shared by adapters that
/// cannot update one physical reference cell without changing its semantics.
pub(crate) fn resolve_reference_compat(
    store: &mut (impl ModelStore + ?Sized),
    id: &str,
    field: &str,
    index: Option<usize>,
    target: &str,
) -> StorageResult<()> {
    let value = Value::String(target.into());
    if let Some(index) = index {
        let mut current = store.value(id, field)?.ok_or("Missing reference feature")?;
        *current
            .as_array_mut()
            .and_then(|a| a.get_mut(index))
            .ok_or("Missing reference item")? = value;
        store.set_value(id, field, Some(current))
    } else {
        store.set_value(id, field, Some(value))
    }
}
/// Roll back on returned errors and on unwinding. Drop is best effort; explicit
/// rollback failures are reported together with the original failure.
pub fn atomic<S: ModelStore + ?Sized, T>(
    store: &mut S,
    f: impl FnOnce(&mut S) -> StorageResult<T>,
) -> StorageResult<T> {
    struct Guard<'a, S: ModelStore + ?Sized> {
        store: &'a mut S,
        token: Checkpoint,
        active: bool,
    }
    impl<S: ModelStore + ?Sized> Drop for Guard<'_, S> {
        fn drop(&mut self) {
            if self.active {
                let _ = self.store.rollback(&self.token);
            }
        }
    }
    let token = store.begin()?;
    let mut guard = Guard {
        store,
        token,
        active: true,
    };
    let result = f(guard.store).and_then(|value| {
        guard.store.commit(&guard.token)?;
        Ok(value)
    });
    match result {
        Ok(value) => {
            guard.active = false;
            Ok(value)
        }
        Err(error) => {
            let rollback = guard.store.rollback(&guard.token);
            guard.active = false;
            match rollback {
                Ok(()) => Err(error),
                Err(other) => Err(format!("{error}; rollback failed: {other}")),
            }
        }
    }
}
pub fn visit(
    store: &(impl ModelRead + ?Sized),
    f: impl FnMut(Element) -> StorageResult<()>,
) -> StorageResult<()> {
    visit_observed(store, &InvocationContext::default(), f)
}
fn visit_observed(
    store: &(impl ModelRead + ?Sized),
    observation: &InvocationContext<'_>,
    mut f: impl FnMut(Element) -> StorageResult<()>,
) -> StorageResult<()> {
    let mut after = None;
    loop {
        observation.work(1)?;
        let page = store.page(after, 128)?;
        if page.is_empty() {
            return Ok(());
        }
        for (cursor, element) in page {
            observation.work(1)?;
            after = Some(cursor);
            f(element)?;
        }
    }
}
pub fn export(store: &(impl ModelRead + ?Sized)) -> StorageResult<Model> {
    let mut model = Model {
        schema_revision: store.revision()?,
        resources: store.resource_records()?,
        elements: vec![],
    };
    visit(store, |e| {
        model.elements.push(e);
        Ok(())
    })?;
    Ok(model)
}
pub fn bind_schema(store: &mut (impl ModelStore + ?Sized), schema: &Schema) -> StorageResult<()> {
    schema.validate_instantiations()?;
    bind_runtime_validated_schema(store, schema)
}
// Only callers that completed the SAME immutable runtime validator may enter.
// Revision/configuration are live storage operations and always remain fresh.
fn bind_runtime_validated_schema(
    store: &mut (impl ModelStore + ?Sized),
    schema: &Schema,
) -> StorageResult<()> {
    let revision = store.revision()?;
    if !revision.is_empty() && revision != schema.revision {
        return Err("Model belongs to a different schema revision".into());
    }
    store.configure_schema(schema)
}
pub fn insert(
    store: &mut (impl ModelStore + ?Sized),
    schema: &Schema,
    element: Element,
) -> StorageResult<()> {
    if !store.is_transaction_active() {
        return Err("Storage write requires a transaction".into());
    }
    if element.id.is_empty() || store.get(&element.id)?.is_some() {
        return Err("Element identity must be unique and nonempty".into());
    }
    if element.type_identity != schema.identity(&element.class) {
        return Err("Model type identity does not match schema".into());
    }
    atomic(store, |store| {
        bind_schema(store, schema)?;
        store.put(element)
    })
}
pub fn edit(
    store: &mut (impl ModelStore + ?Sized),
    schema: &Schema,
    edits: &[(String, String, Value)],
) -> StorageResult<()> {
    edit_context(store, schema, edits, None)
}
pub fn edit_with_conversions(
    store: &mut (impl ModelStore + ?Sized),
    schema: &Schema,
    edits: &[(String, String, Value)],
    conversions: &ConversionRegistry<'_>,
) -> StorageResult<()> {
    edit_context(store, schema, edits, Some(conversions))
}
fn edit_context(
    store: &mut (impl ModelStore + ?Sized),
    schema: &Schema,
    edits: &[(String, String, Value)],
    conversions: Option<&ConversionRegistry<'_>>,
) -> StorageResult<()> {
    atomic(store, |store| {
        bind_schema(store, schema)?;
        for (id, name, value) in edits {
            set_inner(store, schema, id, name, value.clone(), conversions)?;
        }
        validate_context(store, schema, conversions, None)
    })
}
pub fn set_with_opposite(
    store: &mut (impl ModelStore + ?Sized),
    schema: &Schema,
    id: &str,
    name: &str,
    value: Value,
) -> StorageResult<()> {
    set_with_opposite_context(store, schema, id, name, value, None)
}
/// The private checked facade has validated the immutable schema and owns
/// rollback for the whole batch. Check the live store binding on every edit.
pub(super) fn set_batch_member(
    store: &mut impl ModelStore,
    index: &dyn FeatureLookup,
    id: &str,
    name: &str,
    value: Value,
) -> StorageResult<()> {
    bind_runtime_validated_schema(store, index.schema())?;
    set_inner_indexed(store, index, id, name, value, None)
}
/// SAME atomic opposite setter with immutable schema validation supplied by
/// this exact prepared owner. Every edit still binds the current store revision
/// and configuration, then executes the ordinary checked setter body. No source,
/// constructor, reference or graph-validity authority is transferred.
/// A copied/equal schema does not acquire this prepared-owner proof.
pub fn set_with_opposite_prepared(
    store: &mut (impl ModelStore + ?Sized),
    schema: &Schema,
    metadata: &crate::definitions::preparation::PreparedPhysicalMetadata<'_>,
    id: &str,
    name: &str,
    value: Value,
) -> StorageResult<()> {
    atomic(store, |store| {
        if !metadata.matches_schema(schema) {
            return Err("Prepared opposite setter requires the exact schema owner".into());
        }
        // PreparedPhysicalMetadata completed InstantiationValidation's SAME
        // stages; its reference_views flag changes immutable retention only.
        bind_runtime_validated_schema(store, schema)?;
        set_inner(store, schema, id, name, value, None)
    })
}
pub fn set_with_opposite_and_conversions(
    store: &mut (impl ModelStore + ?Sized),
    schema: &Schema,
    id: &str,
    name: &str,
    value: Value,
    conversions: &ConversionRegistry<'_>,
) -> StorageResult<()> {
    set_with_opposite_context(store, schema, id, name, value, Some(conversions))
}
fn set_with_opposite_context(
    store: &mut (impl ModelStore + ?Sized),
    schema: &Schema,
    id: &str,
    name: &str,
    value: Value,
    conversions: Option<&ConversionRegistry<'_>>,
) -> StorageResult<()> {
    atomic(store, |store| {
        bind_schema(store, schema)?;
        set_inner(store, schema, id, name, value, conversions)
    })
}

/// Common index keys for native adapters. Prefixes keep metadata separate from
/// feature names, including names that happen to be 'class' or 'resource'.
pub fn index_keys(e: &Element) -> Vec<(String, String)> {
    let mut keys: Vec<_> = unsorted_index_keys(e).flatten().collect();
    keys.sort();
    keys.dedup();
    keys
}
fn unsorted_index_keys(e: &Element) -> impl Iterator<Item = Option<(String, String)>> + '_ {
    let mut cursor = indexing::KeyCursor::default();
    std::iter::from_fn(move || cursor.next(e))
}
pub fn query_key(q: Query<'_>) -> (String, String) {
    match q {
        Query::Class(v) => ("class".into(), v.into()),
        Query::Resource(v) => ("resource".into(), v.into()),
        Query::Value(k, v) => (format!("value:{k}"), v.to_string()),
    }
}
/// Guard the two logical string slots and exact encoded bytes before key copies.
pub fn query_key_observed(
    query: Query<'_>,
    observation: &InvocationContext<'_>,
) -> StorageResult<(String, String)> {
    observation.checkpoint()?;
    if observation.has_observer() {
        let (name, value) = match query {
            Query::Class(value) => (5, value.len()),
            Query::Resource(value) => (8, value.len()),
            Query::Value(field, value) => {
                let bytes = query_value_size(value, observation)?;
                (
                    field
                        .len()
                        .checked_add(6)
                        .ok_or("Query key storage overflow")?,
                    bytes,
                )
            }
        };
        let bytes = 128usize
            .checked_add(name)
            .and_then(|n| n.checked_add(value))
            .ok_or("Query key storage overflow")?;
        observation.retain_storage(bytes)?;
    }
    Ok(query_key(query))
}
pub(super) fn query_value_size(
    value: &Value,
    observation: &InvocationContext<'_>,
) -> StorageResult<usize> {
    intersect_model::definitions::metadata_limits::measure_with_work(
        value,
        65_536,
        64,
        usize::MAX,
        &|units| observation.work(units),
    )?;
    intersect_model::definitions::metadata_limits::serialized_size_with_work(
        value,
        usize::MAX,
        &|units| observation.work(units),
    )
}
/// One logical row-vector slot (32), string slot (64), and borrowed UTF-8 bytes.
pub fn push_identity_observed(
    rows: &mut Vec<(u64, String)>,
    row: u64,
    id: &str,
    observation: &InvocationContext<'_>,
) -> StorageResult<()> {
    observation.work(1)?;
    if observation.has_observer() {
        observation.retain_storage(
            96usize
                .checked_add(id.len())
                .ok_or("Identity page storage overflow")?,
        )?;
    }
    rows.try_reserve(1)
        .map_err(|_| "Identity page allocation failed")?;
    rows.push((row, id.to_owned()));
    Ok(())
}
/// Guard each vector slot and borrowed value before allocating a page copy.
pub(crate) fn project_values_observed<'a>(
    values: impl Iterator<Item = ValueView<'a>>,
    observation: &InvocationContext<'_>,
) -> StorageResult<Vec<Value>> {
    let mut page = Vec::new();
    for value in values {
        observation.work(1)?;
        if observation.has_observer() {
            observation.retain_storage(32)?;
        }
        page.try_reserve(1)
            .map_err(|_| "Value page allocation failed")?;
        page.push(value.to_json_observed(observation)?);
    }
    Ok(page)
}
/// Compare one read-view revision without requesting an owned result.
pub(crate) fn revision_matches(
    store: &(impl ModelRead + ?Sized),
    expected: &str,
) -> StorageResult<bool> {
    let mut matching = false;
    store.inspect_revision(&mut |revision| {
        matching = revision == expected;
        Ok(())
    })?;
    Ok(matching)
}
use crate::core::model::value_slice as values;
/// Traverse bounded identity pages and inspect each object within its read guard.
/// Memory-backed reads borrow canonical records; no owned Element is required.
pub fn visit_views(
    store: &(impl ModelRead + ?Sized),
    f: impl FnMut(ObjectView<'_>) -> StorageResult<()>,
) -> StorageResult<()> {
    visit_views_observed(store, &InvocationContext::default(), f)
}
fn visit_views_observed(
    store: &(impl ModelRead + ?Sized),
    observation: &InvocationContext<'_>,
    mut f: impl FnMut(ObjectView<'_>) -> StorageResult<()>,
) -> StorageResult<()> {
    let mut after = None;
    loop {
        observation.work(1)?;
        let page = store.id_page(after, 128)?;
        if page.is_empty() {
            return Ok(());
        }
        for (cursor, id) in page {
            observation.work(1)?;
            if !store.inspect(&id, &mut f)? {
                return Err("Missing paged element".into());
            }
            after = Some(cursor);
        }
    }
}
/// Validate the complete supported structural profile, including lower bounds.
pub fn validate(
    store: &(impl ModelRead + ?Sized),
    schema: &intersect_model::definitions::schema::SchemaRepresentation<
        impl intersect_model::definitions::schema::RuntimeAlternative,
    >,
) -> Result<(), String> {
    validate_context(store, schema, None, None)
}
pub fn validate_with_conversions(
    store: &(impl ModelRead + ?Sized),
    schema: &intersect_model::definitions::schema::SchemaRepresentation<
        impl intersect_model::definitions::schema::RuntimeAlternative,
    >,
    conversions: &ConversionRegistry<'_>,
) -> Result<(), String> {
    validate_context(store, schema, Some(conversions), None)
}
/// Validate with operation-scoped work/cancellation observation. Storage adapter
/// calls and individual schema/codec operations remain synchronous atomic units.
pub fn validate_with_conversions_observed(
    store: &(impl ModelRead + ?Sized),
    schema: &intersect_model::definitions::schema::SchemaRepresentation<
        impl intersect_model::definitions::schema::RuntimeAlternative,
    >,
    conversions: &ConversionRegistry<'_>,
    observation: &InvocationContext<'_>,
) -> Result<(), String> {
    validate_context(store, schema, Some(conversions), Some(observation))
}
/// Draft validation relaxes only lower multiplicities.
pub fn validate_draft(
    store: &(impl ModelRead + ?Sized),
    schema: &intersect_model::definitions::schema::SchemaRepresentation<
        impl intersect_model::definitions::schema::RuntimeAlternative,
    >,
) -> Result<(), String> {
    validate_structure(store, schema, None, None, false, false).map(|_| ())
}
/// Validate an incomplete model using the explicitly selected converters.
pub(crate) fn validate_draft_with_conversions(
    store: &(impl ModelRead + ?Sized),
    schema: &intersect_model::definitions::schema::SchemaRepresentation<
        impl intersect_model::definitions::schema::RuntimeAlternative,
    >,
    conversions: &ConversionRegistry<'_>,
) -> Result<(), String> {
    validate_structure(store, schema, Some(conversions), None, false, false).map(|_| ())
}
fn validate_context(
    store: &(impl ModelRead + ?Sized),
    schema: &intersect_model::definitions::schema::SchemaRepresentation<
        impl intersect_model::definitions::schema::RuntimeAlternative,
    >,
    conversions: Option<&ConversionRegistry<'_>>,
    scoped: Option<&InvocationContext<'_>>,
) -> Result<(), String> {
    validate_structure(store, schema, conversions, scoped, true, false).map(|_| ())
}

/// Shared shape, multiplicity and uniqueness validation for stored/provider values.
fn validate_feature_shape(
    class: &str,
    field: &intersect_model::definitions::schema::Feature,
    value: Option<ValueView<'_>>,
    require_lower_bounds: bool,
    mut charge: impl FnMut() -> StorageResult<()>,
) -> StorageResult<()> {
    let mut pending = FeatureShape::new(class, field, value, require_lower_bounds)?;
    while !pending.advance(value, &mut charge)? {}
    Ok(())
}

fn validate_structure(
    store: &(impl ModelRead + ?Sized),
    schema: &intersect_model::definitions::schema::SchemaRepresentation<
        impl intersect_model::definitions::schema::RuntimeAlternative,
    >,
    conversions: Option<&ConversionRegistry<'_>>,
    scoped: Option<&InvocationContext<'_>>,
    require_lower_bounds: bool,
    retain_ownership: bool,
) -> Result<OwnershipIndex, String> {
    StructuralPreparation::new(require_lower_bounds, retain_ownership).run(
        store,
        schema,
        conversions,
        scoped,
    )
}

// Internal inverse maintenance, not a reflective setter. The caller owns the
// checkpoint and writes the new reciprocal endpoint after this disconnection.
fn displace_inverse(
    store: &mut (impl ModelStore + ?Sized),
    schema: &Schema,
    owner: &str,
    name: &str,
    target: &str,
) -> StorageResult<()> {
    let class = store
        .class(owner)?
        .ok_or("Missing previous opposite owner")?;
    let fields = schema.features(&class)?;
    let field = fields
        .get(name)
        .ok_or("Missing previous opposite feature")?;
    if !field.reference || field.derived {
        return Err("Displacement requires a stored reciprocal reference".into());
    }
    store.inspect(owner, &mut |object| {
        validate_assignment_state(schema, object, &fields)
    })?;
    let mut value = store
        .value(owner, name)?
        .ok_or("Missing previous opposite value")?;
    if field.upper == 1 {
        if value.as_str() != Some(target) {
            return Err("Inconsistent previous reciprocal opposite".into());
        }
        store.set_resolved_value(owner, name, Some(Value::Null))?;
        if field.unsettable {
            update_assignment(store, owner, name, false)?;
        }
    } else {
        let list = value
            .as_array_mut()
            .ok_or("Invalid previous opposite list")?;
        let before = list.len();
        list.retain(|value| value.as_str() != Some(target));
        if list.len() == before {
            return Err("Inconsistent previous reciprocal opposite".into());
        }
        // Inverse removal preserves Many assignment history and survivor order.
        store.set_resolved_value(owner, name, Some(value))?;
    }
    Ok(())
}

fn set_inner(
    store: &mut (impl ModelStore + ?Sized),
    schema: &Schema,
    id: &str,
    name: &str,
    value: Value,
    conversions: Option<&ConversionRegistry<'_>>,
) -> Result<(), String> {
    write_inner(
        store,
        schema,
        None,
        id,
        name,
        value,
        conversions,
        WriteMode::Replace,
    )
}
/// The checked setter, with feature maps answered by `index`, a lookup over
/// the SAME schema (equal answers, possibly cached).
fn set_inner_indexed(
    store: &mut (impl ModelStore + ?Sized),
    index: &dyn FeatureLookup,
    id: &str,
    name: &str,
    value: Value,
    conversions: Option<&ConversionRegistry<'_>>,
) -> Result<(), String> {
    write_inner(
        store,
        index.schema(),
        Some(index),
        id,
        name,
        value,
        conversions,
        WriteMode::Replace,
    )
}
enum WriteMode {
    Replace,
    Collection,
    Initialize,
}
/// Feature maps of `schema`, from `index` (a lookup over the same schema) when given.
fn feature_map(
    schema: &Schema,
    index: Option<&dyn FeatureLookup>,
    class: &str,
) -> Result<intersect_model::definitions::schema::FeatureMap, String> {
    match index {
        Some(index) => index.feature_map(class),
        None => schema.prepared_features(class).map(std::rc::Rc::new),
    }
}
#[allow(clippy::too_many_arguments)] // The checked writer and its optional same-schema index.
fn write_inner(
    store: &mut (impl ModelStore + ?Sized),
    schema: &Schema,
    index: Option<&dyn FeatureLookup>,
    id: &str,
    name: &str,
    value: Value,
    conversions: Option<&ConversionRegistry<'_>>,
    mode: WriteMode,
) -> Result<(), String> {
    if index.is_some_and(|index| !std::ptr::eq(index.schema(), schema)) {
        return Err("Feature index belongs to another schema".into());
    }
    let class = store.class(id)?.ok_or("Missing element")?;
    let fields = feature_map(schema, index, &class)?;
    let field = fields.get(name).ok_or("Unknown feature")?;
    if field.derived {
        return Err("Cannot write a derived feature".into());
    }
    if !field.changeable && matches!(mode, WriteMode::Replace) {
        return Err(format!(
            "Cannot write a non-changeable feature: {class}.{name}"
        ));
    }
    if field.unsettable {
        store.inspect(id, &mut |object| {
            validate_assignment_state(schema, object, fields.fields())
        })?;
    }
    if !field.reference
        && (field.datatype.is_some() || !schema.feature_domains(&class, name).is_empty())
    {
        for atom in values(&value, field.upper)? {
            match index {
                // The member selection is the index's, of this same schema.
                Some(_) => schema.validate_runtime_attribute_prepared(
                    &class,
                    field,
                    atom,
                    conversions,
                    &*fields,
                )?,
                None => schema.validate_runtime_attribute(&class, field, atom, conversions)?,
            }
        }
    }
    let old = store.value(id, name)?.unwrap_or(Value::Null);
    if let Some(opposite) = &field.opposite {
        let (_, back) = opposite.split_once('/').ok_or("Invalid opposite")?;
        let retained = values(&value, field.upper)?;
        for (value, add) in [(&old, false), (&value, true)] {
            for target in values(value, field.upper)? {
                // Preserve reciprocal order and history for retained memberships.
                // Add still reconciles a missing inverse during text linking.
                if !add && retained.contains(target) {
                    continue;
                }
                let target = target.as_str().ok_or("Invalid reference")?;
                write_opposite(store, schema, index, id, back, target, add)?;
            }
        }
    }
    let assigned = !value.is_null() || !schema.is_container_feature(field)?;
    store.set_resolved_value(id, name, Some(value))?;
    if field.unsettable {
        update_assignment(store, id, name, assigned)?;
    }
    Ok(())
}
/// Connect (`add`) or disconnect `id` at `target`'s inverse feature `back`,
/// displacing a previous scalar inverse owner. Shared by the checked setter
/// and the checked append.
fn write_opposite(
    store: &mut (impl ModelStore + ?Sized),
    schema: &Schema,
    index: Option<&dyn FeatureLookup>,
    id: &str,
    back: &str,
    target: &str,
    add: bool,
) -> Result<(), String> {
    let class = store.class(target)?.ok_or("Missing opposite target")?;
    let feature = match index {
        Some(index) => index.effective_feature(&class, back)?.feature,
        None => schema.effective_feature(&class, back)?.feature,
    };
    if feature.unsettable {
        let back_fields = feature_map(schema, index, &class)?;
        store.inspect(target, &mut |object| {
            validate_assignment_state(schema, object, back_fields.fields())
        })?;
    }
    if feature.upper == 1 {
        let previous = store.value(target, back)?.unwrap_or(Value::Null);
        if add && !previous.is_null() && previous.as_str() != Some(id) {
            let previous = previous.as_str().ok_or("Invalid occupied opposite")?;
            let reciprocal = feature
                .opposite
                .as_deref()
                .and_then(|value| value.split_once('/'))
                .map(|(_, name)| name)
                .ok_or("Missing reciprocal opposite declaration")?;
            displace_inverse(store, schema, previous, reciprocal, target)?;
        }
        if add || previous.as_str() == Some(id) {
            store.set_resolved_value(
                target,
                back,
                Some(if add {
                    Value::String(id.into())
                } else {
                    Value::Null
                }),
            )?;
            if feature.unsettable {
                update_assignment(store, target, back, add)?;
            }
        }
    } else {
        let mut value = store
            .value(target, back)?
            .unwrap_or_else(|| Value::Array(vec![]));
        let list = value.as_array_mut().ok_or("Invalid opposite list")?;
        if add {
            if !list.iter().any(|v| v.as_str() == Some(id)) {
                list.push(Value::String(id.into()));
            }
        } else {
            list.retain(|v| v.as_str() != Some(id));
        }
        store.set_resolved_value(target, back, Some(value))?;
        // Removing from a Many inverse preserves assignment history.
        if feature.unsettable && add {
            update_assignment(store, target, back, true)?;
        }
    }
    Ok(())
}
/// Append one reference to a stored Many reference feature and connect its
/// opposite, with the prepared owner's schema validation. Equivalent to the
/// checked setter writing the extended list: retained members' inverses
/// already point at `id`, so only the appended target's inverse changes.
pub fn append_with_opposite_prepared(
    store: &mut (impl ModelStore + ?Sized),
    index: &dyn FeatureLookup,
    metadata: &crate::definitions::preparation::PreparedPhysicalMetadata<'_>,
    id: &str,
    name: &str,
    value: Value,
) -> StorageResult<()> {
    let schema = index.schema();
    atomic(store, |store| {
        if !metadata.matches_schema(schema) {
            return Err("Prepared opposite append requires the exact schema owner".into());
        }
        bind_runtime_validated_schema(store, schema)?;
        let class = store.class(id)?.ok_or("Missing element")?;
        let field = index.effective_feature(&class, name)?.feature;
        if field.derived {
            return Err("Cannot write a derived feature".into());
        }
        if !field.reference || field.upper == 1 || field.unsettable {
            return Err("Opposite append requires a Many reference that is not unsettable".into());
        }
        let target = value.as_str().ok_or("Invalid reference")?.to_owned();
        if let Some(opposite) = &field.opposite {
            let (_, back) = opposite.split_once('/').ok_or("Invalid opposite")?;
            write_opposite(store, schema, Some(index), id, back, &target, true)?;
        }
        store.append_value(id, name, value)
    })
}
/// Indexed immutable view, sharing read semantics for borrowed and owned models.
/// Indexes retain identities and positions, never a duplicate object graph.
pub struct IndexedModel<M> {
    model: M,
    ids: BTreeMap<String, usize>,
    indexes: BTreeMap<(String, String), Vec<usize>>,
}
/// Compatibility view that borrows the existing model without copying it.
pub type ModelView<'a> = IndexedModel<&'a Model>;
/// Owned immutable snapshot with retained read indexes.
pub type OwnedModelView = IndexedModel<Model>;
impl<M: std::borrow::Borrow<Model>> IndexedModel<M> {
    pub fn model(&self) -> &Model {
        self.model.borrow()
    }
    /// Resolve an identity to this immutable view's local row. The position is
    /// not a portable object identity or authority for another model context.
    pub fn row_for_id(&self, id: &str) -> Option<usize> {
        self.ids.get(id).copied()
    }
    pub fn new(model: M) -> StorageResult<Self> {
        Self::new_observed(model, &InvocationContext::default())
    }
    /// Build an immutable index using the same keys as all storage adapters.
    pub fn new_observed(model: M, observation: &InvocationContext<'_>) -> StorageResult<Self> {
        let mut preparation = Self::begin_observed(model, observation)?;
        while !preparation.advance(observation)? {}
        preparation.finish(observation)
    }
    /// Start a resumable immutable index build without copying the model graph.
    pub fn begin_observed(
        model: M,
        observation: &InvocationContext<'_>,
    ) -> StorageResult<IndexedModelPreparation<M>> {
        IndexedModelPreparation::new(model, observation)
    }
}
impl<M: std::borrow::Borrow<Model>> ModelRead for IndexedModel<M> {
    fn resource_records(
        &self,
    ) -> StorageResult<BTreeMap<String, crate::core::resources::ResourceRecord>> {
        Ok(self.model.borrow().resources.clone())
    }
    fn resource_record(
        &self,
        uri: &str,
    ) -> StorageResult<Option<crate::core::resources::ResourceRecord>> {
        Ok(self.model.borrow().resources.get(uri).cloned())
    }
    fn inspect(
        &self,
        id: &str,
        f: &mut dyn FnMut(ObjectView<'_>) -> StorageResult<()>,
    ) -> StorageResult<bool> {
        if let Some(index) = self.ids.get(id) {
            f(ObjectView::from_element(
                &self.model.borrow().elements[*index],
            ))?;
            Ok(true)
        } else {
            Ok(false)
        }
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
        let start = after.map_or(0, |v| v.saturating_add(1));
        let mut rows = Vec::new();
        for (i, element) in self
            .model
            .borrow()
            .elements
            .iter()
            .enumerate()
            .skip(usize::try_from(start).unwrap_or(usize::MAX))
            .take(limit)
        {
            push_identity_observed(&mut rows, i as u64, &element.id, observation)?;
        }
        Ok(rows)
    }
    fn value(&self, id: &str, field: &str) -> StorageResult<Option<Value>> {
        self.value_observed(id, field, &InvocationContext::default())
    }
    fn value_observed(
        &self,
        id: &str,
        field: &str,
        observation: &InvocationContext<'_>,
    ) -> StorageResult<Option<Value>> {
        observation.checkpoint()?;
        self.ids
            .get(id)
            .and_then(|i| self.model.borrow().elements[*i].values.get(field))
            .map(|value| ValueView::json(value).to_json_observed(observation))
            .transpose()
    }
    fn value_len(&self, id: &str, field: &str) -> StorageResult<Option<usize>> {
        Ok(self
            .ids
            .get(id)
            .and_then(|i| self.model.borrow().elements[*i].values.get(field))
            .and_then(Value::as_array)
            .map(Vec::len))
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
        let values = self
            .ids
            .get(id)
            .and_then(|i| self.model.borrow().elements[*i].values.get(field))
            .and_then(Value::as_array)
            .ok_or("Multiple feature requires a stored list")?;
        project_values_observed(
            values
                .get(offset..)
                .unwrap_or(&[])
                .iter()
                .take(limit)
                .map(ValueView::json),
            observation,
        )
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
        let key = query_key_observed(query, observation)?;
        let candidates = self.indexes.get(&key).map(Vec::as_slice).unwrap_or(&[]);
        let start = after.map_or(0, |cursor| {
            candidates.partition_point(|row| (*row as u64) <= cursor)
        });
        let tail = &candidates[start..];
        let mut rows = Vec::new();
        for i in tail.iter().take(limit) {
            push_identity_observed(
                &mut rows,
                *i as u64,
                &self.model.borrow().elements[*i].id,
                observation,
            )?;
        }
        Ok(QueryCandidates {
            after: rows.last().map(|(row, _)| *row).or(after),
            exhausted: tail.len() <= limit,
            rows,
            exact: true,
        })
    }
    fn matches(&self, id: &str, query: Query<'_>) -> StorageResult<bool> {
        self.matches_observed(id, query, &InvocationContext::default())
    }
    fn matches_observed(
        &self,
        id: &str,
        query: Query<'_>,
        observation: &InvocationContext<'_>,
    ) -> StorageResult<bool> {
        observation.checkpoint()?;
        let Some(row) = self.ids.get(id) else {
            return Ok(false);
        };
        let key = query_key_observed(query, observation)?;
        Ok(self
            .indexes
            .get(&key)
            .is_some_and(|rows| rows.binary_search(row).is_ok()))
    }

    fn class(&self, id: &str) -> StorageResult<Option<String>> {
        Ok(self
            .ids
            .get(id)
            .map(|i| self.model.borrow().elements[*i].class.clone()))
    }
    fn inspect_class(
        &self,
        id: &str,
        f: &mut dyn FnMut(&str) -> StorageResult<()>,
    ) -> StorageResult<bool> {
        if let Some(row) = self.ids.get(id) {
            f(&self.model.borrow().elements[*row].class)?;
            Ok(true)
        } else {
            Ok(false)
        }
    }
    fn revision(&self) -> StorageResult<String> {
        Ok(self.model.borrow().schema_revision.clone())
    }
    fn inspect_revision(&self, f: &mut dyn FnMut(&str) -> StorageResult<()>) -> StorageResult<()> {
        f(&self.model.borrow().schema_revision)
    }
    fn get(&self, id: &str) -> StorageResult<Option<Element>> {
        Ok(self
            .ids
            .get(id)
            .map(|i| self.model.borrow().elements[*i].clone()))
    }
    fn page(&self, after: Option<u64>, limit: usize) -> StorageResult<Vec<Row>> {
        let start = after.map_or(0, |v| v.saturating_add(1));
        Ok(self
            .model
            .borrow()
            .elements
            .iter()
            .enumerate()
            .skip(usize::try_from(start).unwrap_or(usize::MAX))
            .take(limit)
            .map(|(i, e)| (i as u64, e.clone()))
            .collect())
    }
    fn query(&self, query: Query<'_>) -> StorageResult<Vec<String>> {
        let key = query_key(query);
        Ok(self
            .indexes
            .get(&key)
            .into_iter()
            .flatten()
            .map(|i| self.model.borrow().elements[*i].id.clone())
            .collect())
    }
}

// Explicit completed-snapshot checked query sibling; default adapters stay literal.

#[path = "storage/retained_model_wire.rs"]
mod retained_model_wire;
pub(crate) use retained_model_wire::export_snapshot_rows;
pub use retained_model_wire::{PhysicalPersistenceLimits, RetainedModelWire};

mod inverse_nominal_witness;
pub(crate) use inverse_nominal_witness::checked_inverse_nominal_witness;
