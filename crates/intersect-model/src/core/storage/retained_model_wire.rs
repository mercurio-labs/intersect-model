//! Canonical Model wire data for physical persistence. No executable/source
//! receipt authority, ModelRead, language validation or Package is serialized.
use super::*;
use crate::definitions::{
    conversion::{InvocationObserver, ProviderId},
    metadata_limits::TraversalBudget,
};
use serde::Serialize;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{cell::Cell, rc::Rc};

/// Explicit operation limits, never a work grant or replacement caller.
/// Logical admissions use the existing portable 128-byte/node policy. Legacy
/// canonical storage/index allocation (S09) is not an allocator/RSS guarantee.
#[derive(Clone, Copy, Debug)]
pub struct PhysicalPersistenceLimits {
    pub objects: usize,
    pub logical_bytes: usize,
    pub wire_bytes: usize,
}
impl PhysicalPersistenceLimits {
    fn check(self) -> StorageResult<()> {
        if self.objects == 0
            || self.logical_bytes < 512
            || self.wire_bytes == 0
            || self.wire_bytes > self.logical_bytes
        {
            return Err("Invalid finite physical persistence limits".into());
        }
        Ok(())
    }
}

/// Untrusted serialized physical data. Loading binds the actual live native
/// definition again and audits its newly constructed store; this value is not
/// a receipt, reader, model owner, source token or mutation capability.
pub struct RetainedModelWire {
    wire: Value,
}
impl std::fmt::Debug for RetainedModelWire {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RetainedModelWire").finish_non_exhaustive()
    }
}
struct WireObservation<'a> {
    parent: &'a InvocationContext<'a>,
    used: Cell<usize>,
    limit: usize,
    failed: Cell<bool>,
}
impl InvocationObserver for WireObservation<'_> {
    fn before_invoke(&self, _: &ProviderId) -> StorageResult<()> {
        Err("Physical wire has no semantic provider authority".into())
    }
    fn failed_result(&self, _: &str) {
        self.failed.set(true);
    }
    fn checkpoint(&self) -> StorageResult<()> {
        if self.failed.get() {
            return Err("Physical wire invocation has failed".into());
        }
        let result = self.parent.checkpoint();
        if result.is_err() {
            self.failed.set(true);
        }
        result
    }
    fn work(&self, units: usize) -> StorageResult<()> {
        self.checkpoint()?;
        let result = self.parent.work(units).and_then(|()| self.checkpoint());
        if result.is_err() {
            self.failed.set(true);
        }
        result
    }
    fn retain_storage(&self, bytes: usize) -> StorageResult<()> {
        self.checkpoint()?;
        let result = (|| {
            let next = self
                .used
                .get()
                .checked_add(bytes)
                .filter(|n| *n <= self.limit)
                .ok_or("Physical wire logical storage limit exceeded")?;
            self.parent.retain_storage(bytes)?;
            self.checkpoint()?;
            self.used.set(next);
            Ok(())
        })();
        if result.is_err() {
            self.failed.set(true);
        }
        result
    }
}
fn with_wire<T>(
    limits: PhysicalPersistenceLimits,
    original: &InvocationContext<'_>,
    run: impl FnOnce(&InvocationContext<'_>) -> StorageResult<T>,
) -> StorageResult<T> {
    limits.check()?;
    if !original.has_observer() {
        return Err("Physical persistence requires an invocation observer".into());
    }
    original.work(1)?;
    let header =
        crate::logical_size!(WireObservation<'_>, 32) + 2 * crate::logical_size::LOGICAL_WORD;
    if header > limits.logical_bytes {
        return Err("Physical wire logical storage limit exceeded".into());
    }
    original.retain_storage(header)?;
    original.checkpoint()?;
    let observed = InvocationContext::new(Rc::new(WireObservation {
        parent: original,
        used: Cell::new(header),
        limit: limits.logical_bytes,
        failed: Cell::new(false),
    }));
    let value = run(&observed)?;
    observed.checkpoint()?;
    Ok(value)
}
fn copy_admission<T: ?Sized + Serialize>(
    value: &T,
    limits: PhysicalPersistenceLimits,
    observation: &InvocationContext<'_>,
) -> StorageResult<usize> {
    let mut traversal = TraversalBudget::new(limits.logical_bytes / 128, 128, limits.logical_bytes);
    traversal.inspect_with_work(value, &|n| observation.work(n))?;
    let bytes = traversal
        .visited_nodes()
        .checked_mul(128)
        .and_then(|n| n.checked_add(traversal.payload_bytes()))
        .ok_or("Physical wire copy admission overflow")?;
    observation.retain_storage(bytes)?;
    observation.checkpoint()?;
    Ok(bytes)
}
fn context_digest(
    definition: &impl crate::definitions::preparation::PreparedPersistenceContext,
    limits: PhysicalPersistenceLimits,
    observation: &InvocationContext<'_>,
) -> StorageResult<[u8; 32]> {
    // Hash the canonical Serde representation of the ACTUAL prepared context,
    // not an asserted URI or copied executable configuration from the wire.
    let model = definition.model_preparation();
    let context = (model.sources(), model.schema(), definition.context_data());
    let mut traversal = TraversalBudget::new(limits.logical_bytes, 128, limits.logical_bytes);
    traversal.inspect_with_work(&context, &|n| observation.work(n))?;
    struct HashWriter<'a> {
        hash: Sha256,
        bytes: usize,
        limit: usize,
        observation: &'a InvocationContext<'a>,
    }
    impl std::io::Write for HashWriter<'_> {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            let next = self
                .bytes
                .checked_add(bytes.len())
                .filter(|n| *n <= self.limit)
                .ok_or_else(|| {
                    std::io::Error::other("Physical native-context byte limit exceeded")
                })?;
            self.observation
                .work(1 + bytes.len().div_ceil(64))
                .map_err(std::io::Error::other)?;
            self.hash.update(bytes);
            self.bytes = next;
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    observation.work(1)?;
    observation.retain_storage(crate::logical_size!(HashWriter<'_>, 136))?;
    observation.checkpoint()?;
    let mut writer = HashWriter {
        hash: Sha256::new(),
        bytes: 0,
        limit: limits.logical_bytes,
        observation,
    };
    serde_json::to_writer(&mut writer, &context).map_err(|e| e.to_string())?;
    observation.work(1)?;
    observation.checkpoint()?;
    Ok(writer.hash.finalize().into())
}
// Borrowed wire DATA only. Do not serialize the checked association holder:
// its private admission/accounting state is not persistent authority.
#[derive(Serialize)]
struct AssociationRow<'a> {
    source: &'a crate::definitions::bundle::SourceId,
    resource_uri: &'a str,
}
struct AssociationRows<'a>(&'a [crate::core::text_document::DocumentResourceAssociation]);
impl Serialize for AssociationRows<'_> {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeSeq;
        let mut sequence = serializer.serialize_seq(Some(self.0.len()))?;
        for association in self.0 {
            sequence.serialize_element(&AssociationRow {
                source: association.source(),
                resource_uri: association.resource_uri(),
            })?;
        }
        sequence.end()
    }
}
#[derive(Serialize)]
struct Wire<'a> {
    version: u8,
    context_sha256: [u8; 32],
    closed_sources: &'a [crate::definitions::bundle::SourceId],
    resource_associations: AssociationRows<'a>,
    model: &'a crate::core::model::Model,
}
/// Preflight each borrowed row before materializing canonical snapshot data.
pub(crate) fn export_snapshot_rows(
    revision: &str,
    resources: &BTreeMap<String, crate::core::resources::ResourceRecord>,
    count: usize,
    max_objects: usize,
    observation: &InvocationContext<'_>,
    visit: impl FnOnce(&mut dyn FnMut(ObjectView<'_>) -> StorageResult<()>) -> StorageResult<()>,
) -> StorageResult<Model> {
    use crate::definitions::retained_storage::reserve_serialized;
    observation.work(1)?;
    if count > max_objects {
        return Err("Physical snapshot object limit exceeded".into());
    }
    reserve_serialized(revision, observation)?;
    reserve_serialized(resources, observation)?;
    let slots = count
        .checked_mul(crate::logical_size!(Element, 408))
        .and_then(|n| n.checked_add(crate::logical_size!(Model, 72)))
        .ok_or("Physical snapshot row capacity overflow")?;
    observation.retain_storage(slots)?;
    observation.checkpoint()?;
    let mut elements = Vec::new();
    elements
        .try_reserve_exact(count)
        .map_err(|_| "Physical snapshot row allocation failed")?;
    if elements.capacity() != count {
        return Err("Physical snapshot capacity differs from paid slots".into());
    }
    visit(&mut |view| {
        observation.work(1)?;
        if elements.len() == count {
            return Err("Physical snapshot row count changed".into());
        }
        for text in [
            view.id(),
            view.class(),
            view.type_identity(),
            view.resource(),
        ] {
            reserve_serialized(text, observation)?;
        }
        reserve_serialized(&view.initialization(), observation)?;
        reserve_serialized(&view.assignment_state(), observation)?;
        for (key, value) in view.fields() {
            observation.work(1)?;
            // Same existing logical per-map-entry tariff; no physical BTree
            // node-size/peak-memory guarantee is inferred from this amount.
            observation.retain_storage(128)?;
            reserve_serialized(key, observation)?;
            reserve_serialized(&value, observation)?;
        }
        observation.checkpoint()?;
        elements.push(view.to_element());
        Ok(())
    })?;
    if elements.len() != count {
        return Err("Physical snapshot row count changed".into());
    }
    observation.checkpoint()?;
    Ok(Model {
        schema_revision: revision.to_owned(),
        resources: resources.clone(),
        elements,
    })
}

impl RetainedModelWire {
    /// Raw wire data only; no trusted receipt or graph capability is exposed.
    pub fn json(&self) -> &Value {
        &self.wire
    }
    /// External bytes/JSON are only data. The actual native context is checked
    /// at restore, followed by the caller's canonical source/construction audit.
    pub fn from_json(
        wire: &Value,
        limits: PhysicalPersistenceLimits,
        original: &InvocationContext<'_>,
    ) -> StorageResult<Self> {
        with_wire(limits, original, |observation| {
            copy_admission(wire, limits, observation)?;
            crate::definitions::metadata_limits::serialized_size_with_work(
                wire,
                limits.wire_bytes,
                &|n| observation.work(n),
            )?;
            Ok(Self { wire: wire.clone() })
        })
    }
    /// Explicit snapshot of the actual store for persistence. This may run
    /// inside the retained transaction: only current row data is materialized,
    /// never undo state. It does not replace/seed the original live continuation.
    pub fn save(
        store: &dyn ModelRead,
        definition: &impl crate::definitions::preparation::PreparedPersistenceContext,
        closed: &[crate::definitions::bundle::SourceId],
        resources: &[crate::core::text_document::DocumentResourceAssociation],
        limits: PhysicalPersistenceLimits,
        original: &InvocationContext<'_>,
    ) -> StorageResult<Self> {
        with_wire(limits, original, |observation| {
            observation.work(1)?;
            let model = store.export_snapshot_observed(limits.objects, observation)?;
            let context_sha256 = context_digest(definition, limits, observation)?;
            let envelope = Wire {
                version: 1,
                context_sha256,
                closed_sources: closed,
                resource_associations: AssociationRows(resources),
                model: &model,
            };
            crate::definitions::metadata_limits::serialized_size_with_work(
                &envelope,
                limits.wire_bytes,
                &|n| observation.work(n),
            )?;
            copy_admission(&envelope, limits, observation)?;
            let wire = serde_json::to_value(&envelope).map_err(|e| e.to_string())?;
            observation.checkpoint()?;
            Ok(Self { wire })
        })
    }
    /// Decode canonical physical data with source receipts removed. Callers
    /// must bind the decoded data to their native store and audit it before
    /// using it for language queries.
    pub fn restore_model(
        &self,
        definition: &impl crate::definitions::preparation::PreparedPersistenceContext,
        closed: &[crate::definitions::bundle::SourceId],
        resources: &[crate::core::text_document::DocumentResourceAssociation],
        limits: PhysicalPersistenceLimits,
        original: &InvocationContext<'_>,
    ) -> StorageResult<Model> {
        with_wire(limits, original, |observation| {
            observation.work(1)?;
            let object = self
                .wire
                .as_object()
                .ok_or("Physical snapshot requires an object")?;
            if object.len() != 5 || object.get("version").and_then(Value::as_u64) != Some(1) {
                return Err("Unsupported physical snapshot wire version/shape".into());
            }
            let raw_closed = object
                .get("closed_sources")
                .ok_or("Missing physical snapshot source context")?;
            if !crate::definitions::json_exact::matches_with_work(&closed, raw_closed, &|n| {
                observation.work(n)
            })? {
                return Err("Physical snapshot closed source/resource identity mismatch".into());
            }
            let raw_resources = object
                .get("resource_associations")
                .ok_or("Missing physical snapshot resource context")?;
            if !crate::definitions::json_exact::matches_with_work(
                &AssociationRows(resources),
                raw_resources,
                &|n| observation.work(n),
            )? {
                return Err("Physical snapshot closed source/resource identity mismatch".into());
            }
            let digest = context_digest(definition, limits, observation)?;
            let raw_digest = object
                .get("context_sha256")
                .ok_or("Missing physical snapshot native context")?;
            if !crate::definitions::json_exact::matches_with_work(&digest, raw_digest, &|n| {
                observation.work(n)
            })? {
                return Err("Physical snapshot native definition context mismatch".into());
            }
            let raw_model = object
                .get("model")
                .ok_or("Missing canonical Model snapshot")?;
            let raw_rows = raw_model
                .as_object()
                .and_then(|m| m.get("elements"))
                .and_then(Value::as_array)
                .ok_or("Physical snapshot requires canonical Model rows")?;
            if raw_rows.len() > limits.objects {
                return Err("Physical snapshot object limit exceeded".into());
            }
            let mut preparation = crate::core::model::SnapshotPreparation::new();
            while !preparation
                .advance(raw_model, observation)
                .map_err(|e| e.to_string())?
            {}
            let mut model = preparation.finish().map_err(|e| e.to_string())?;
            observation.work(1)?;
            if model.elements.len() > limits.objects {
                return Err("Physical snapshot object limit exceeded".into());
            }
            // Never revive serialized current-text receipts. Preserve resource
            // IDs, roots, values, references, origins and numeric revisions.
            for record in model.resources.values_mut() {
                observation.work(1)?;
                record.source = None;
            }
            Ok(model)
        })
    }
}

#[cfg(test)]
#[path = "retained_model_wire_tests.rs"]
mod tests;
