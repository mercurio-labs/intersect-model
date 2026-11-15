//! Handwritten logical resource state over the canonical model graph. No I/O.
use super::{
    model::Element,
    project_usages::PackageIdentity,
    storage::{self, ModelRead, ModelStore, StorageResult},
};
use crate::definitions::schema::{Feature, Schema};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ResourceLoadState {
    Loaded,
    Unloaded,
    Failed,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceCorrespondence {
    pub uri: String,
    pub revision: String,
}

/// Logical resource metadata. Root IDs refer to existing canonical elements.
/// The shared root list makes revision-only edits independent of root count.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResourceRecord {
    pub uri: String,
    pub roots: Arc<Vec<String>>,
    #[serde(with = "revision_wire")]
    pub content_revision: u64,
    pub load_state: ResourceLoadState,
    pub diagnostics: Vec<String>,
    pub source: Option<SourceCorrespondence>,
    /// Model-package origin. None denotes the editable project's resource.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub package: Option<PackageIdentity>,
    /// Identity reservations only: no object values or executable authority.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unloaded: Option<UnloadedContents>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UnloadedContents {
    pub identities: BTreeSet<String>,
    pub roots: Arc<Vec<String>>,
    pub content_digest: String,
}
/// Decimal strings survive JSON hosts without narrowing u64 revisions to f64.
mod revision_wire {
    use serde::{Deserialize, Deserializer, Serializer};
    pub fn serialize<S: Serializer>(value: &u64, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&value.to_string())
    }
    pub fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<u64, D::Error> {
        let text = String::deserialize(deserializer)?;
        let revision = text.parse::<u64>().map_err(serde::de::Error::custom)?;
        if revision.to_string() != text {
            return Err(serde::de::Error::custom(
                "Resource revision requires a canonical unsigned decimal string",
            ));
        }
        Ok(revision)
    }
}

impl ResourceRecord {
    pub fn new(uri: impl Into<String>) -> StorageResult<Self> {
        let uri = uri.into();
        validate_resource_uri(&uri)?;
        Ok(Self {
            uri,
            roots: Arc::new(Vec::new()),
            content_revision: 0,
            load_state: ResourceLoadState::Loaded,
            diagnostics: Vec::new(),
            source: None,
            package: None,
            unloaded: None,
        })
    }
    pub fn touch(&mut self) -> StorageResult<()> {
        self.content_revision = self.next_content_revision()?;
        self.source = None;
        self.diagnostics.clear();
        Ok(())
    }
    pub(crate) fn next_content_revision(&self) -> StorageResult<u64> {
        self.content_revision
            .checked_add(1)
            .ok_or_else(|| "Resource content revision exhausted".into())
    }
}

/// The package origin of an existing resource (or None for a project resource
/// or an unknown URI). This grants no visibility or immutability authority.
pub fn resource_package(
    store: &(impl ModelRead + ?Sized),
    uri: &str,
) -> StorageResult<Option<PackageIdentity>> {
    Ok(store
        .resource_record(uri)?
        .and_then(|record| record.package))
}

/// Resource URIs belonging to exactly this package identity, in URI order.
pub fn resources_of_package(
    store: &(impl ModelRead + ?Sized),
    identity: &PackageIdentity,
) -> StorageResult<Vec<String>> {
    Ok(store
        .resource_records()?
        .into_iter()
        .filter_map(|(uri, record)| (record.package.as_ref() == Some(identity)).then_some(uri))
        .collect())
}

fn validate_reference_syntax(uri: &str) -> StorageResult<()> {
    if uri.is_empty()
        || uri.contains(['#', '?', '\\'])
        || uri.chars().any(|c| c.is_whitespace() || c.is_control())
    {
        return Err(format!("Unsupported logical resource URI: {uri}"));
    }
    let parsed = iri_string::types::IriReferenceStr::new(uri)
        .map_err(|error| format!("Invalid resource URI: {error}"))?;
    // The resolver recognizes encoded dot segments as navigation even when
    // general percent normalization is disabled. Reject those spellings rather
    // than silently changing a logical identity or bypassing the root boundary.
    if parsed.path_str().split('/').any(|segment| {
        let dots = segment.replace("%2e", ".").replace("%2E", ".");
        segment.contains('%') && matches!(dots.as_str(), "." | "..")
    }) {
        return Err("Percent-encoded dot path segments are unsupported".into());
    }
    Ok(())
}

pub fn validate_resource_uri(uri: &str) -> StorageResult<()> {
    validate_reference_syntax(uri)?;
    let parsed = iri_string::types::IriReferenceStr::new(uri)
        .map_err(|error| format!("Invalid resource URI: {error}"))?;
    let path = parsed.path_str();
    if (parsed.scheme_str().is_none() && parsed.authority_str().is_some())
        || path.contains("//")
        || (path.len() > 1 && path.ends_with('/'))
        || path.split('/').any(|part| matches!(part, "." | ".."))
    {
        return Err("Logical resource URIs require canonical path segments".into());
    }
    Ok(())
}

fn validate_source(record: &ResourceRecord) -> StorageResult<()> {
    if record
        .source
        .as_ref()
        .is_some_and(|source| source.uri.trim().is_empty() || source.revision.trim().is_empty())
    {
        return Err("Source correspondence requires nonempty URI and revision".into());
    }
    if let Some(unloaded) = &record.unloaded
        && (record.load_state != ResourceLoadState::Unloaded
            || !record.roots.is_empty()
            || unloaded.content_digest.len() != 64
            || !unloaded
                .content_digest
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
            || unloaded.identities.iter().any(|id| id.is_empty())
            || unloaded.roots.iter().collect::<BTreeSet<_>>().len() != unloaded.roots.len()
            || unloaded
                .roots
                .iter()
                .any(|id| !unloaded.identities.contains(id)))
    {
        return Err("Invalid unloaded resource identity reservations".into());
    }
    Ok(())
}

/// Reject traversal beyond the logical root before RFC resolution can clamp it.
fn reject_above_base(path: &str) -> StorageResult<()> {
    let mut depth = 0usize;
    for segment in path.split('/') {
        match segment {
            "" | "." => {}
            ".." => {
                depth = depth
                    .checked_sub(1)
                    .ok_or("Reference escapes its resource base")?;
            }
            _ => {
                depth = depth.checked_add(1).ok_or("Resource path depth overflow")?;
            }
        }
    }
    Ok(())
}

/// Bounded RFC reference resolution with no I/O or identity normalization.
/// Relative logical IDs retain their relative identity; paths escaping their
/// logical root and noncanonical/unrepresentable results reject explicitly.
pub fn resolve_resource_uri(base: &str, reference: &str) -> StorageResult<String> {
    use iri_string::types::{IriAbsoluteStr, IriReferenceStr};
    use std::borrow::Cow;

    validate_resource_uri(base)?;
    if reference.is_empty() {
        return Ok(base.into());
    }
    validate_reference_syntax(reference)?;
    let base_parts = IriReferenceStr::new(base)
        .map_err(|error| format!("Invalid resource base URI: {error}"))?;
    let reference_parts = IriReferenceStr::new(reference)
        .map_err(|error| format!("Invalid resource reference URI: {error}"))?;
    if base_parts.scheme_str().is_none()
        && reference_parts.scheme_str().is_none()
        && reference_parts.authority_str().is_some()
    {
        return Err("Network-path references require a resource base with a scheme".into());
    }

    let base_path = base_parts.path_str();
    let reference_path = reference_parts.path_str();
    let merged_path = if reference_parts.scheme_str().is_some()
        || reference_parts.authority_str().is_some()
        || reference_path.starts_with('/')
    {
        Cow::Borrowed(reference_path)
    } else if base_parts.authority_str().is_some() && base_path.is_empty() {
        Cow::Owned(format!("/{reference_path}"))
    } else {
        let directory = base_path
            .rsplit_once('/')
            .map_or("", |(directory, _)| &base_path[..directory.len() + 1]);
        Cow::Owned(format!("{directory}{reference_path}"))
    };
    reject_above_base(&merged_path)?;

    // iri-string requires an absolute base. A private scheme and optional
    // leading slash adapt relative logical IDs without changing their identity.
    const SYNTHETIC_PREFIX: &str = "intersect-relative:";
    let absolute_base = if base_parts.scheme_str().is_some() {
        Cow::Borrowed(base)
    } else if base_path.starts_with('/') {
        Cow::Owned(format!("{SYNTHETIC_PREFIX}{base}"))
    } else {
        Cow::Owned(format!("{SYNTHETIC_PREFIX}/{base}"))
    };
    let absolute_base = IriAbsoluteStr::new(&absolute_base)
        .map_err(|error| format!("Invalid resource resolution base: {error}"))?;
    let resolved = reference_parts.resolve_against(absolute_base);
    resolved
        .ensure_rfc3986_normalizable()
        .map_err(|error| format!("Unsupported resource resolution: {error}"))?;
    let resolved = resolved.to_string();
    let result = if base_parts.scheme_str().is_none() && reference_parts.scheme_str().is_none() {
        let relative = resolved
            .strip_prefix(SYNTHETIC_PREFIX)
            .ok_or("Relative resource resolution changed its scheme")?;
        if base_path.starts_with('/') || reference_path.starts_with('/') {
            relative.to_owned()
        } else {
            let relative = relative
                .strip_prefix('/')
                .ok_or("Relative resource resolution lost its path root")?;
            let parsed = IriReferenceStr::new(relative)
                .map_err(|error| format!("Invalid resolved relative resource: {error}"))?;
            if parsed.scheme_str().is_some() || parsed.authority_str().is_some() {
                return Err("Resolved relative resource identity would be ambiguous".into());
            }
            relative.to_owned()
        }
    } else {
        resolved
    };
    validate_resource_uri(&result)?;
    Ok(result)
}

pub fn create_resource(store: &mut (impl ModelStore + ?Sized), uri: &str) -> StorageResult<()> {
    storage::atomic(store, |store| {
        if store.resource_record(uri)?.is_some() {
            return Err(format!("Duplicate resource URI: {uri}"));
        }
        store.set_resource_record(ResourceRecord::new(uri)?)
    })
}

pub fn reorder_roots(
    store: &mut (impl ModelStore + ?Sized),
    uri: &str,
    roots: &[String],
) -> StorageResult<()> {
    storage::atomic(store, |store| {
        let mut record = store
            .resource_record(uri)?
            .ok_or_else(|| format!("Unknown resource {uri}"))?;
        let expected: BTreeSet<_> = record.roots.iter().collect();
        let supplied: BTreeSet<_> = roots.iter().collect();
        if expected != supplied || roots.len() != expected.len() {
            return Err("Root ordering must contain every root exactly once".into());
        }
        if record.roots.as_slice() != roots {
            record.roots = Arc::new(roots.to_vec());
            record.touch()?;
            store.set_resource_record(record)?;
        }
        Ok(())
    })
}

pub(crate) fn touch_resource(
    store: &mut (impl ModelStore + ?Sized),
    uri: &str,
) -> StorageResult<()> {
    store.touch_resource_record(uri)
}

pub(crate) fn touch_resource_compat(
    store: &mut (impl ModelStore + ?Sized),
    uri: &str,
) -> StorageResult<()> {
    let mut record = match store.resource_record(uri)? {
        Some(record) => record,
        None => ResourceRecord::new(uri)?,
    };
    if record.load_state != ResourceLoadState::Loaded {
        return Err(format!("Resource is not loaded: {uri}"));
    }
    record.touch()?;
    store.set_resource_record(record)
}

pub(crate) fn change_root(
    store: &mut (impl ModelStore + ?Sized),
    uri: &str,
    id: &str,
    root: bool,
) -> StorageResult<()> {
    if store.change_resource_root(uri, id, root)? {
        return Ok(());
    }
    let mut record = match store.resource_record(uri)? {
        Some(record) => record,
        None => ResourceRecord::new(uri)?,
    };
    let present = record.roots.iter().any(|r| r == id);
    if present != root {
        if root {
            Arc::make_mut(&mut record.roots).push(id.into());
        } else {
            Arc::make_mut(&mut record.roots).retain(|r| r != id);
        }
        record.touch()?;
        store.set_resource_record(record)?;
    }
    Ok(())
}

fn reference_ids(value: Option<&Value>) -> BTreeSet<&str> {
    match value {
        Some(Value::String(id)) => BTreeSet::from([id.as_str()]),
        Some(Value::Array(values)) => values.iter().filter_map(Value::as_str).collect(),
        _ => BTreeSet::new(),
    }
}

/// Adapter hook before a field mutation. Scalar edits never traverse the graph.
pub(super) fn field_delta_inner(
    store: &mut (impl ModelStore + ?Sized),
    uri: &str,
    containment: bool,
    before: Option<&Value>,
    after: Option<&Value>,
) -> StorageResult<()> {
    touch_resource(store, uri)?;
    if containment {
        let old = reference_ids(before);
        let new = reference_ids(after);
        for id in old.difference(&new) {
            let mut resource = None;
            store.inspect(id, &mut |object| {
                resource = Some(object.resource().to_owned());
                Ok(())
            })?;
            if let Some(resource) = resource {
                change_root(store, &resource, id, true)?;
            }
        }
        for id in new.difference(&old) {
            let mut resource = None;
            store.inspect(id, &mut |object| {
                resource = Some(object.resource().to_owned());
                Ok(())
            })?;
            if let Some(resource) = resource {
                change_root(store, &resource, id, false)?;
            }
        }
    }
    Ok(())
}

/// Adapter hook before insertion, replacement or removal. Checked edits supply
/// valid ownership; raw persistence can temporarily retain invalid draft graphs.
fn element_delta_inner(
    store: &mut (impl ModelStore + ?Sized),
    schema: Option<&Schema>,
    before: Option<&Element>,
    after: Option<&Element>,
) -> StorageResult<()> {
    if let Some(after) = after
        && before.is_none_or(|before| before.id != after.id)
    {
        check_identity_available(store, &after.id)?;
    }
    if let Some(before) = before {
        touch_resource(store, &before.resource)?;
        if after.is_none_or(|after| after.resource != before.resource) {
            change_root(store, &before.resource, &before.id, false)?;
        }
    }
    if let Some(after) = after
        && before.is_none_or(|before| before.resource != after.resource)
    {
        touch_resource(store, &after.resource)?;
        change_root(store, &after.resource, &after.id, true)?;
    }
    if let Some(schema) = schema {
        let mut old = BTreeSet::new();
        let mut new = BTreeSet::new();
        for (element, targets) in [(before, &mut old), (after, &mut new)] {
            if let Some(element) = element {
                for (name, feature) in schema.features(&element.class)? {
                    if feature.containment && !feature.derived {
                        targets.extend(reference_ids(element.values.get(&name)));
                    }
                }
            }
        }
        for id in old.difference(&new) {
            if let Some(target) = store.get(id)? {
                change_root(store, &target.resource, id, true)?;
            }
        }
        for id in new.difference(&old) {
            if let Some(target) = store.get(id)? {
                change_root(store, &target.resource, id, false)?;
            }
        }
    }
    Ok(())
}

/// Explicit producer initialization, preserving element order as initial root order.
/// This does not load anything or invent empty resources.
pub fn records_for_elements(
    schema: &Schema,
    elements: &[Element],
) -> StorageResult<BTreeMap<String, ResourceRecord>> {
    let mut records = BTreeMap::new();
    let mut contained = BTreeSet::new();
    let mut fields = RecordContainmentFields::new(schema);
    for element in elements {
        if !records.contains_key(&element.resource) {
            records.insert(
                element.resource.clone(),
                ResourceRecord::new(&element.resource)?,
            );
        }
        for name in fields.get(&element.class)?.iter() {
            contained.extend(reference_ids(element.values.get(name)));
        }
    }
    for element in elements {
        if !contained.contains(element.id.as_str()) {
            Arc::make_mut(
                &mut records
                    .get_mut(&element.resource)
                    .ok_or("Missing resource")?
                    .roots,
            )
            .push(element.id.clone());
        }
    }
    Ok(records)
}

/// Canonical containment selection for one immutable-schema resource projection.
/// Only metadata is reused: each object still contributes its own references.
struct RecordContainmentFields<'a> {
    schema: &'a Schema,
    classes: BTreeMap<String, Vec<String>>,
}
impl<'a> RecordContainmentFields<'a> {
    const CAPACITY: usize = 256;

    fn new(schema: &'a Schema) -> Self {
        Self {
            schema,
            classes: BTreeMap::new(),
        }
    }

    fn select(schema: &Schema, class: &str) -> StorageResult<Vec<String>> {
        Ok(schema
            .features(class)?
            .into_iter()
            .filter_map(|(name, field)| (field.containment && !field.derived).then_some(name))
            .collect())
    }

    fn get(&mut self, class: &str) -> StorageResult<std::borrow::Cow<'_, [String]>> {
        if !self.classes.contains_key(class) && self.classes.len() < Self::CAPACITY {
            let fields = Self::select(self.schema, class)?;
            self.classes.insert(class.into(), fields);
        }
        match self.classes.get(class) {
            Some(fields) => Ok(std::borrow::Cow::Borrowed(fields)),
            None => Ok(std::borrow::Cow::Owned(Self::select(self.schema, class)?)),
        }
    }
}

/// Audit logical boundaries independently of full structural/provider checks.
pub fn validate_records(store: &(impl ModelRead + ?Sized), schema: &Schema) -> StorageResult<()> {
    let records = store.resource_records()?;
    validate_reservations(store, &records)?;
    let mut elements = Vec::new();
    storage::visit(store, |element| {
        elements.push(element);
        Ok(())
    })?;
    let expected = records_for_elements(schema, &elements)?;
    for (uri, record) in &records {
        validate_resource_uri(uri)?;
        validate_source(record)?;
        if record.uri != *uri {
            return Err("Resource registry key does not match logical URI".into());
        }
        let roots: BTreeSet<_> = record.roots.iter().collect();
        if roots.len() != record.roots.len() {
            return Err("Duplicate resource root".into());
        }
        let actual = expected
            .get(uri)
            .map(|record| record.roots.iter().collect::<BTreeSet<_>>())
            .unwrap_or_default();
        if roots != actual {
            return Err(format!(
                "Resource roots disagree with membership/containment: {uri}"
            ));
        }
        if record.load_state != ResourceLoadState::Loaded
            && elements.iter().any(|e| e.resource == *uri)
        {
            return Err("Unavailable resource contains loaded elements".into());
        }
    }
    for uri in expected.keys() {
        if !records.contains_key(uri) {
            return Err(format!("Missing resource record: {uri}"));
        }
    }
    Ok(())
}

fn validate_reservations(
    store: &(impl ModelRead + ?Sized),
    records: &BTreeMap<String, ResourceRecord>,
) -> StorageResult<()> {
    let mut reserved = BTreeSet::new();
    for record in records.values() {
        if let Some(unloaded) = &record.unloaded {
            for id in &unloaded.identities {
                if !reserved.insert(id) || store.class(id)?.is_some() {
                    return Err(format!(
                        "Unloaded resource identity is already occupied: {id}"
                    ));
                }
            }
        }
    }
    Ok(())
}

/// Fast root creation used by compact stores without materializing an Element.
pub(crate) fn add_member(
    store: &mut (impl ModelStore + ?Sized),
    uri: &str,
    id: &str,
) -> StorageResult<()> {
    storage::atomic(store, |store| {
        check_identity_available(store, id)?;
        touch_resource(store, uri)?;
        change_root(store, uri, id, true)
    })
}

pub(super) fn has_populated_containment_default(field: &Feature) -> bool {
    field.containment
        && !field.derived
        && !field.default.is_null()
        && !field.default.as_array().is_some_and(Vec::is_empty)
}

pub(super) fn add_member_with_defaults<'a>(
    store: &mut (impl ModelStore + ?Sized),
    uri: &str,
    id: &str,
    fields: impl Iterator<Item = &'a Feature>,
) -> StorageResult<()> {
    add_member(store, uri, id)?;
    for field in fields.filter(|field| has_populated_containment_default(field)) {
        field_delta(store, uri, true, None, Some(&field.default))?;
    }
    Ok(())
}

fn check_identity_available(store: &(impl ModelRead + ?Sized), id: &str) -> StorageResult<()> {
    if store.identity_reserved(id)? {
        return Err(format!(
            "Identity is reserved by an unloaded resource: {id}"
        ));
    }
    Ok(())
}
pub(crate) fn field_delta(
    store: &mut (impl ModelStore + ?Sized),
    uri: &str,
    containment: bool,
    before: Option<&Value>,
    after: Option<&Value>,
) -> StorageResult<()> {
    storage::atomic(store, |store| {
        field_delta_inner(store, uri, containment, before, after)
    })
}
pub fn element_delta(
    store: &mut (impl ModelStore + ?Sized),
    schema: Option<&Schema>,
    before: Option<&Element>,
    after: Option<&Element>,
) -> StorageResult<()> {
    storage::atomic(store, |store| {
        element_delta_inner(store, schema, before, after)
    })
}

/// Streaming root audit piggybacking on the structural validator's existing passes.
pub(crate) struct ResourceAudit {
    records: BTreeMap<String, ResourceRecord>,
    declared: BTreeMap<String, (Arc<str>, bool)>,
    undeclared: BTreeSet<String>,
}
impl ResourceAudit {
    pub(crate) fn new(store: &(impl ModelRead + ?Sized)) -> StorageResult<Self> {
        let records = store.resource_records()?;
        validate_reservations(store, &records)?;
        let mut declared = BTreeMap::new();
        for (uri, record) in &records {
            validate_resource_uri(uri)?;
            validate_source(record)?;
            if record.unloaded.is_some() {
                return Err(format!(
                    "Resource content unavailable: {uri}; restore resident contents before model validation"
                ));
            }
            if record.uri != *uri {
                return Err("Resource registry key does not match logical URI".into());
            }
            if record.load_state != ResourceLoadState::Loaded && !record.roots.is_empty() {
                return Err("Unavailable resource has loaded roots".into());
            }
            let uri: Arc<str> = Arc::from(uri.as_str());
            for root in record.roots.iter() {
                if declared
                    .insert(root.clone(), (uri.clone(), false))
                    .is_some()
                {
                    return Err("Duplicate resource root".into());
                }
            }
        }
        Ok(Self {
            records,
            declared,
            undeclared: BTreeSet::new(),
        })
    }
    pub(crate) fn element(&mut self, element: storage::ObjectView<'_>) -> StorageResult<()> {
        let record = self
            .records
            .get(element.resource())
            .ok_or_else(|| format!("Missing resource record: {}", element.resource()))?;
        if record.load_state != ResourceLoadState::Loaded {
            return Err("Unavailable resource contains loaded elements".into());
        }
        if let Some((uri, present)) = self.declared.get_mut(element.id()) {
            if uri.as_ref() != element.resource() {
                return Err("Root belongs to another resource".into());
            }
            *present = true;
        } else {
            self.undeclared.insert(element.id().into());
        }
        Ok(())
    }
    /// Whether `element` would accept a member of this resource.
    pub(crate) fn loaded(&self, uri: &str) -> bool {
        self.records
            .get(uri)
            .is_some_and(|record| record.load_state == ResourceLoadState::Loaded)
    }
    /// Declared roots as (identity, resource URI).
    pub(crate) fn declared_roots(&self) -> impl Iterator<Item = (&str, &str)> {
        self.declared
            .iter()
            .map(|(id, (uri, _))| (id.as_str(), uri.as_ref()))
    }
    pub(crate) fn contained(&mut self, id: &str) {
        if let Some((_, present)) = self.declared.get_mut(id) {
            *present = false;
        } else {
            self.undeclared.remove(id);
        }
    }
    pub(crate) fn finish(self) -> StorageResult<()> {
        if !self.undeclared.is_empty() || self.declared.values().any(|(_, present)| !present) {
            return Err("Resource roots disagree with membership/containment".into());
        }
        Ok(())
    }
}

/// A resource projection borrowing its canonical store. Members and object
/// inspection use the existing store indexes and borrowed object views.
pub struct ResourceView<'a, S: ModelRead + ?Sized> {
    store: &'a S,
    record: ResourceRecord,
}
impl<S: ModelRead + ?Sized> ResourceView<'_, S> {
    pub fn record(&self) -> &ResourceRecord {
        &self.record
    }
    pub fn roots(&self) -> &[String] {
        &self.record.roots
    }
    pub fn members(&self) -> StorageResult<Vec<String>> {
        self.store.query(storage::Query::Resource(&self.record.uri))
    }
    /// Inspect only members of this resource; foreign/missing IDs return false.
    pub fn inspect(
        &self,
        id: &str,
        f: &mut dyn FnMut(storage::ObjectView<'_>) -> StorageResult<()>,
    ) -> StorageResult<bool> {
        let mut member = false;
        self.store.inspect(id, &mut |object| {
            if object.resource() == self.record.uri {
                member = true;
                f(object)?;
            }
            Ok(())
        })?;
        Ok(member)
    }
}

pub fn resource_view<'a, S: ModelRead + ?Sized>(
    store: &'a S,
    uri: &str,
) -> StorageResult<Option<ResourceView<'a, S>>> {
    Ok(store
        .resource_record(uri)?
        .map(|record| ResourceView { store, record }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        core::api::{BasicModel, ModelEdit, TransactionalModel},
        definitions::dynamic::{ScalarType, SchemaBuilder},
    };
    use serde_json::json;

    fn model() -> BasicModel {
        let mut builder = SchemaBuilder::new("resource-tree", "urn:resource-tree");
        let node = builder.class("Node").unwrap();
        node.attribute("name", ScalarType::String)
            .optional()
            .unwrap();
        node.reference("children", &node)
            .many()
            .containment()
            .unwrap();
        BasicModel::new(builder.build().unwrap().as_schema().clone()).unwrap()
    }

    // The pre-reuse projection is retained only as an independent test oracle.
    fn uncached_records(
        schema: &Schema,
        elements: &[Element],
    ) -> StorageResult<BTreeMap<String, ResourceRecord>> {
        let mut records = BTreeMap::new();
        let mut contained = BTreeSet::new();
        for element in elements {
            if !records.contains_key(&element.resource) {
                records.insert(
                    element.resource.clone(),
                    ResourceRecord::new(&element.resource)?,
                );
            }
            for (name, feature) in schema.features(&element.class)? {
                if feature.containment && !feature.derived {
                    contained.extend(reference_ids(element.values.get(&name)));
                }
            }
        }
        for element in elements {
            if !contained.contains(element.id.as_str()) {
                Arc::make_mut(&mut records.get_mut(&element.resource).unwrap().roots)
                    .push(element.id.clone());
            }
        }
        Ok(records)
    }

    #[test]
    fn record_metadata_reuse_preserves_inheritance_roots_order_and_raw_reference_shapes() {
        let mut builder = SchemaBuilder::new("record-fields", "urn:record-fields");
        let node = builder.class("Node").unwrap();
        node.reference("children", &node)
            .many()
            .containment()
            .unwrap();
        let child = builder.class("Child").unwrap();
        child.extends(&node).unwrap();
        let mut schema = builder.build().unwrap().as_schema().clone();
        let mut computed = schema.classes["Node"].features["children"].clone();
        computed.name = "computed".into();
        computed.derived = true;
        let class = schema.classes.get_mut("Child").unwrap();
        class.features.insert("computed".into(), computed);
        class.feature_order.push("computed".into());
        let mut elements: Vec<_> = [
            ("tail", "Node", "urn:b"),
            ("owner", "Child", "urn:a"),
            ("first", "Node", "urn:a"),
            ("child", "Child", "urn:b"),
        ]
        .into_iter()
        .map(|(id, class, uri)| Element::new(&schema, id, class, uri, 0, 0).unwrap())
        .collect();
        elements[1]
            .values
            .insert("children".into(), json!(["child", 17, "child", "missing"]));
        elements[1]
            .values
            .insert("computed".into(), json!(["first"]));
        elements[2].values.insert("children".into(), json!("tail"));
        elements[3]
            .values
            .insert("children".into(), json!(["child"]));
        let actual = records_for_elements(&schema, &elements).unwrap();
        assert_eq!(actual, uncached_records(&schema, &elements).unwrap());
        assert_eq!(actual["urn:a"].roots.as_ref(), &["owner", "first"]);
        assert!(actual["urn:b"].roots.is_empty());
        assert!(records_for_elements(&schema, &[]).unwrap().is_empty());
        // Neither a previous call nor its schema revision authorizes reuse in a new call.
        schema
            .classes
            .get_mut("Node")
            .unwrap()
            .features
            .get_mut("children")
            .unwrap()
            .derived = true;
        let changed = records_for_elements(&schema, &elements).unwrap();
        assert_eq!(changed, uncached_records(&schema, &elements).unwrap());
        assert_eq!(changed["urn:b"].roots.as_ref(), &["tail", "child"]);
        elements[0].class = "Unknown".into();
        assert_eq!(
            records_for_elements(&schema, &elements),
            uncached_records(&schema, &elements)
        );
        elements[0].resource = "bad uri".into();
        assert_eq!(
            records_for_elements(&schema, &elements),
            uncached_records(&schema, &elements)
        );
    }

    #[test]
    fn resource_containment_metadata_is_bounded_with_canonical_fallback() {
        let mut builder = SchemaBuilder::new("many-record-fields", "urn:many-record-fields");
        let target = builder.class("Target").unwrap();
        for i in 0..RecordContainmentFields::CAPACITY + 2 {
            builder
                .class(&format!("C{i}"))
                .unwrap()
                .reference("child", &target)
                .containment()
                .unwrap();
        }
        let schema = builder.build().unwrap();
        let mut fields = RecordContainmentFields::new(schema.as_schema());
        assert!(fields.get("Missing").is_err());
        assert!(fields.classes.is_empty());
        for i in 0..RecordContainmentFields::CAPACITY + 2 {
            assert_eq!(fields.get(&format!("C{i}")).unwrap().as_ref(), &["child"]);
        }
        assert_eq!(fields.classes.len(), RecordContainmentFields::CAPACITY);
        assert!(
            !fields
                .classes
                .contains_key(&format!("C{}", RecordContainmentFields::CAPACITY))
        );
        assert!(matches!(
            fields.get("C0").unwrap(),
            std::borrow::Cow::Borrowed(_)
        ));
        assert!(matches!(
            fields.get("C257").unwrap(),
            std::borrow::Cow::Owned(_)
        ));
        assert!(fields.get("Missing").is_err());
    }

    #[test]
    fn empty_resources_roots_order_and_scalar_revision_share_roots() {
        let mut model = model();
        model.create_resource("urn:empty").unwrap();
        assert!(
            model
                .resource_record("urn:empty")
                .unwrap()
                .unwrap()
                .roots
                .is_empty()
        );
        assert!(model.create_resource("urn:empty").is_err());
        model.create("a", "Node", "urn:tree").unwrap();
        model.create("b", "Node", "urn:tree").unwrap();
        model
            .reorder_roots("urn:tree", &["b".into(), "a".into()])
            .unwrap();
        let before = model.resource_record("urn:tree").unwrap().unwrap();
        model.set("a", "name", json!("changed")).unwrap();
        let after = model.resource_record("urn:tree").unwrap().unwrap();
        assert_eq!(after.roots.as_ref(), &["b", "a"]);
        assert!(Arc::ptr_eq(&before.roots, &after.roots));
        assert!(after.content_revision > before.content_revision);
        assert!(
            model
                .reorder_roots("urn:tree", &["a".into(), "a".into()])
                .is_err()
        );
        validate_records(&model, model.schema()).unwrap();
        let serialized = serde_json::to_string(&storage::export(&model).unwrap()).unwrap();
        assert!(serialized.contains("urn:empty"));
    }

    #[test]
    fn cross_resource_containment_changes_roots_without_moving_identity() {
        let mut model = model();
        model.create("a", "Node", "urn:first").unwrap();
        model.create("b", "Node", "urn:second").unwrap();
        model.set("a", "children", json!(["b"])).unwrap();
        assert!(
            model
                .resource_record("urn:second")
                .unwrap()
                .unwrap()
                .roots
                .is_empty()
        );
        assert_eq!(model.get("b").unwrap().unwrap().resource, "urn:second");
        model.set("a", "children", json!([])).unwrap();
        assert_eq!(
            model
                .resource_record("urn:second")
                .unwrap()
                .unwrap()
                .roots
                .as_ref(),
            &["b"]
        );
        validate_records(&model, model.schema()).unwrap();
    }

    #[test]
    fn failed_operations_and_outer_rollback_restore_exact_resource_state() {
        let mut model = model();
        model.create("a", "Node", "urn:tree").unwrap();
        let before = storage::export(&model).unwrap();
        assert!(model.set("a", "children", json!(["missing"])).is_err());
        assert_eq!(storage::export(&model).unwrap(), before);
        let mut tx = TransactionalModel::new(model);
        let result: Result<(), String> = tx.transaction(|session| {
            session.create_resource("urn:temporary")?;
            session.create("b", "Node", "urn:tree")?;
            session.set("a", "children", json!(["b"]))?;
            Err("deliberate rejection".into())
        });
        assert!(result.is_err());
        assert_eq!(storage::export(&tx).unwrap(), before);
    }

    #[test]
    fn raw_nested_rollback_and_content_overflow_preserve_records() {
        let mut model = model();
        model.create("a", "Node", "urn:tree").unwrap();
        let mut store = model.into_store();
        let before = storage::export(&store).unwrap();
        let outer = store.begin().unwrap();
        let inner = store.begin().unwrap();
        touch_resource(&mut store, "urn:tree").unwrap();
        store.commit(&inner).unwrap();
        store.rollback(&outer).unwrap();
        assert_eq!(storage::export(&store).unwrap(), before);
        storage::atomic(&mut store, |store| {
            let mut record = store.resource_record("urn:tree")?.ok_or("missing")?;
            record.content_revision = u64::MAX;
            store.set_resource_record(record)
        })
        .unwrap();
        let before = storage::export(&store).unwrap();
        assert!(
            storage::atomic(&mut store, |store| store.set_value(
                "a",
                "name",
                Some(json!("rejected"))
            ))
            .is_err()
        );
        assert_eq!(storage::export(&store).unwrap(), before);
    }

    #[test]
    fn logical_uri_resolution_is_lexical_and_bounded() {
        assert_eq!(
            resolve_resource_uri("https://example.test/model/a.xmi", "../b.xmi").unwrap(),
            "https://example.test/b.xmi"
        );
        assert!(resolve_resource_uri("a.xmi", "../b.xmi").is_err());
        assert!(ResourceRecord::new("a.xmi#fragment").is_err());
        assert!(ResourceRecord::new("a/../b.xmi").is_err());
        assert!(ResourceRecord::new("a//b.xmi").is_err());
        assert!(ResourceRecord::new("C:\\models\\a.xmi").is_err());
    }

    #[test]
    fn materialization_keeps_canonical_records_and_edits_invalidate_correspondence() {
        let mut model = model();
        model.create("a", "Node", "urn:tree").unwrap();
        let schema = model.schema().clone();
        let mut materialized = storage::export(&model).unwrap();
        let record = materialized.resources.get_mut("urn:tree").unwrap();
        record.source = Some(SourceCorrespondence {
            uri: "source.tree".into(),
            revision: "source-revision".into(),
        });
        record.diagnostics.push("previous source diagnostic".into());
        let store =
            crate::core::frozen::FrozenStore::from_materialized(&schema, &materialized).unwrap();
        assert_eq!(storage::export(&store).unwrap(), materialized);
        let mut model = BasicModel::from_store(schema.clone(), store).unwrap();
        model.set("a", "name", json!("new name")).unwrap();
        let record = model.resource_record("urn:tree").unwrap().unwrap();
        assert!(record.source.is_none());
        assert!(record.diagnostics.is_empty());
        materialized.resources.clear();
        let store =
            crate::core::frozen::FrozenStore::from_materialized(&schema, &materialized).unwrap();
        assert!(BasicModel::from_store(schema, store).is_err());
        assert!(
            serde_json::from_value::<crate::core::model::Model>(
                json!({"schema_revision":"s","elements":[]})
            )
            .is_err()
        );
    }

    #[test]
    fn revisions_round_trip_above_json_integer_precision_without_narrowing() {
        let mut record = ResourceRecord::new("urn:large-revision").unwrap();
        record.content_revision = u64::MAX;
        let wire = serde_json::to_value(&record).unwrap();
        assert_eq!(wire["content_revision"], json!(u64::MAX.to_string()));
        assert_eq!(
            serde_json::from_value::<ResourceRecord>(wire.clone()).unwrap(),
            record
        );
        for invalid in [
            json!("01"),
            json!("+1"),
            json!("18446744073709551616"),
            json!(42),
        ] {
            let mut malformed = wire.clone();
            malformed["content_revision"] = invalid;
            assert!(serde_json::from_value::<ResourceRecord>(malformed).is_err());
        }
    }

    #[test]
    fn caught_raw_append_failure_rolls_back_default_materialization_and_revisions() {
        let mut model = model();
        model.create("a", "Node", "urn:tree").unwrap();
        model.create("b", "Node", "urn:tree").unwrap();
        let mut store = model.into_store();
        storage::atomic(&mut store, |store| {
            let mut record = store.resource_record("urn:tree")?.ok_or("missing")?;
            record.content_revision = u64::MAX - 1;
            store.set_resource_record(record)
        })
        .unwrap();
        let before = storage::export(&store).unwrap();
        let outer = store.begin().unwrap();
        assert!(store.append_value("a", "children", json!("b")).is_err());
        assert_eq!(storage::export(&store).unwrap(), before);
        // A caught write failure leaves the caller's transaction usable.
        store.commit(&outer).unwrap();
        assert_eq!(storage::export(&store).unwrap(), before);
    }
    #[test]
    fn structural_audit_checks_declared_and_undeclared_roots() -> StorageResult<()> {
        let mut model = model();
        let uri = "urn:tree";
        model.create("parent", "Node", uri)?;
        model.create("child", "Node", uri)?;
        model.set("parent", "children", json!(["child"]))?;
        model.validate()?;
        let schema = model.schema().clone();
        let mut store = model.into_store();
        for roots in [vec![], vec!["parent", "child"], vec!["parent", "missing"]] {
            storage::atomic(&mut store, |store| {
                let mut record = store
                    .resource_record(uri)?
                    .ok_or("Missing fixture resource")?;
                record.roots = Arc::new(roots.into_iter().map(str::to_owned).collect());
                store.set_resource_record(record)
            })?;
            assert_eq!(
                storage::validate(&store, &schema),
                Err("Resource roots disagree with membership/containment".into())
            );
        }
        Ok(())
    }

    #[test]
    fn resource_view_uses_ordered_roots_and_existing_membership_projections() {
        let mut model = model();
        model.create("parent", "Node", "urn:tree").unwrap();
        model.create("child", "Node", "urn:tree").unwrap();
        model.create("foreign", "Node", "urn:other").unwrap();
        model.set("parent", "children", json!(["child"])).unwrap();
        let view = resource_view(&model, "urn:tree").unwrap().unwrap();
        assert_eq!(view.record().uri, "urn:tree");
        assert_eq!(view.roots(), &["parent"]);
        assert_eq!(view.members().unwrap(), vec!["parent", "child"]);
        let mut observed = String::new();
        assert!(
            view.inspect("child", &mut |object| {
                observed = object.id().into();
                Ok(())
            })
            .unwrap()
        );
        assert_eq!(observed, "child");
        assert!(
            !view
                .inspect("foreign", &mut |_| panic!("foreign object exposed"))
                .unwrap()
        );
        assert!(resource_view(&model, "urn:missing").unwrap().is_none());
    }

    #[test]
    fn component_resolution_preserves_bounded_logical_identities() {
        for (base, reference, expected) in [
            ("models/a.xmi", "folder/x:y", "models/folder/x:y"),
            ("memory:a", "b", "memory:b"),
            ("memory:folder/a", "b", "memory:folder/b"),
            ("https://example", "child", "https://example/child"),
            ("https://example/", "child", "https://example/child"),
            ("models/a.xmi", "https://other", "https://other"),
            ("https://example/a", "https://other", "https://other"),
            ("https://example", "https://other", "https://other"),
            ("models/a.xmi", "../b.xmi", "b.xmi"),
            (
                "https://example/models/a.xmi",
                "../b.xmi",
                "https://example/b.xmi",
            ),
            ("models/a.xmi", "/b.xmi", "/b.xmi"),
            ("/models/a.xmi", "b.xmi", "/models/b.xmi"),
            ("/models/a.xmi", "../b.xmi", "/b.xmi"),
            ("https://example/a", "//other/b.xmi", "https://other/b.xmi"),
            ("https://example/a", "//other", "https://other"),
            ("file:///models/a.xmi", "../b.xmi", "file:///b.xmi"),
            ("models/a.xmi", "folder/./b.xmi", "models/folder/b.xmi"),
            ("models/a.xmi", "folder/../b.xmi", "models/b.xmi"),
            ("models/a.xmi", "", "models/a.xmi"),
            (
                "models/a.xmi",
                "intersect-relative:/actual",
                "intersect-relative:/actual",
            ),
            ("HTTP://EXAMPLE/a", "b", "HTTP://EXAMPLE/b"),
            ("model/a", "folder/%2efoo/b", "model/folder/%2efoo/b"),
            ("model/a", "folder/%61/%2F/b", "model/folder/%61/%2F/b"),
        ] {
            assert_eq!(
                resolve_resource_uri(base, reference).unwrap(),
                expected,
                "base={base} reference={reference}"
            );
        }
    }

    #[test]
    fn above_base_and_unsupported_reference_forms_reject() {
        for (base, reference) in [
            ("models/a.xmi", "../../b.xmi"),
            ("a.xmi", "../b.xmi"),
            ("https://example/a", "../b.xmi"),
            ("https://example/models/a", "../../b.xmi"),
            ("models/a.xmi", "/../b.xmi"),
            ("models/a.xmi", "https://other/../b.xmi"),
            ("models/a.xmi", "//other/b.xmi"),
            ("models/a.xmi", "folder//b.xmi"),
            ("models/a.xmi", "../x:y"),
            ("a.xmi", "./b:c"),
            ("models/a.xmi", "b.xmi#fragment"),
            ("models/a.xmi", "b.xmi?query"),
            ("models/a.xmi", "b\\c.xmi"),
            ("models/a.xmi", "folder/"),
            ("models/a.xmi", "%2e/b.xmi"),
            ("models/a.xmi", "%2e%2e/b.xmi"),
            ("https://example/a", "%2E%2E/b.xmi"),
            ("models/a.xmi", ".%2e/b.xmi"),
            ("models/a.xmi", "%2E./b.xmi"),
        ] {
            assert!(
                resolve_resource_uri(base, reference).is_err(),
                "base={base} reference={reference}"
            );
        }
    }

    #[test]
    fn canonical_registry_validation_uses_actual_path_components() {
        for uri in [
            "https://example//child",
            "dir/x://y",
            "//other/a",
            "models/./a",
            "models/a/../b",
            "models/a/",
            "file:////a",
            "models/%2e/a",
            "models/%2E%2e/a",
        ] {
            assert!(validate_resource_uri(uri).is_err(), "accepted {uri}");
        }
        for uri in [
            "https://example",
            "https://example/",
            "https://example/a",
            "file:///a",
            "memory:a",
            "models/a",
            "models/folder/x:y",
            "/a",
        ] {
            validate_resource_uri(uri).unwrap();
        }
    }
}
