//! Handwritten package-payload assembly and lock admission. No I/O or language policy.
use crate::core::{
    frozen::{FrozenStore, Metamodel, Model as FrozenModel},
    model::Model,
    project_usages::{DependencyLock, PackageIdentity},
    resource_set::ResourceContext,
    resources::{ResourceLoadState, ResourceRecord},
    storage::{self, ModelRead, ModelStore},
};
use std::{
    collections::{BTreeMap, BTreeSet},
    fmt,
    sync::Arc,
};

/// One prepared, immutable package payload. A single payload satisfies all
/// lock entries with this exact identity; core does not interpret URI aliases.
pub struct PackagePayload<'a> {
    pub identity: &'a PackageIdentity,
    pub model: &'a FrozenModel,
}

/// One construction store and the exact context under which it was assembled.
/// The language still owns project parsing, linking, completion and validation.
pub struct LockedStore {
    pub store: FrozenStore,
    pub context: ResourceContext,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PackageLoadError {
    MissingPayload {
        uris: Vec<String>,
        identity: Box<PackageIdentity>,
    },
    IdentityMismatch {
        resource: String,
        expected: Box<PackageIdentity>,
        actual: Option<Box<PackageIdentity>>,
    },
    TaggedNotInLock {
        resource: String,
        identity: Box<PackageIdentity>,
    },
    Invalid(String),
}
impl fmt::Display for PackageLoadError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingPayload { uris, identity } => {
                write!(f, "Missing package payload for {uris:?}: {identity:?}")
            }
            Self::IdentityMismatch {
                resource,
                expected,
                actual,
            } => write!(
                f,
                "Package identity mismatch for {resource}: expected {expected:?}, found {actual:?}"
            ),
            Self::TaggedNotInLock { resource, identity } => write!(
                f,
                "Resource {resource} belongs to a package absent from the lock: {identity:?}"
            ),
            Self::Invalid(reason) => f.write_str(reason),
        }
    }
}
impl std::error::Error for PackageLoadError {}
impl From<String> for PackageLoadError {
    fn from(reason: String) -> Self {
        Self::Invalid(reason)
    }
}

fn check_records(
    lock: &DependencyLock,
    records: &BTreeMap<String, ResourceRecord>,
) -> Result<(), PackageLoadError> {
    let identities: BTreeSet<_> = lock
        .entries()
        .values()
        .map(|entry| &entry.identity)
        .collect();
    for (uri, record) in records {
        if record.uri != *uri {
            return Err(PackageLoadError::Invalid(format!(
                "Resource record URI differs from its map key: {uri}"
            )));
        }
        if let Some(identity) = &record.package
            && !identities.contains(identity)
        {
            return Err(PackageLoadError::TaggedNotInLock {
                resource: uri.clone(),
                identity: Box::new(identity.clone()),
            });
        }
    }
    Ok(())
}

/// Recheck provenance after a language has added project objects. This checks
/// identity membership, not visibility, content authenticity or model semantics.
pub fn validate_lock_resource_records(
    lock: &DependencyLock,
    records: &BTreeMap<String, ResourceRecord>,
) -> Result<(), PackageLoadError> {
    check_records(lock, records)
}

/// The same provenance check through an existing shared store reader.
fn validate_lock_resources(
    lock: &DependencyLock,
    store: &(impl ModelRead + ?Sized),
) -> Result<(), PackageLoadError> {
    validate_lock_resource_records(lock, &store.resource_records()?)
}

fn append(store: &mut FrozenStore, incoming: &Model) -> Result<(), PackageLoadError> {
    if incoming.schema_revision != store.metamodel().revision() {
        return Err(PackageLoadError::Invalid(
            "Package/project schema revision differs from the load context".into(),
        ));
    }
    for uri in incoming.resources.keys() {
        if store.resource_record(uri)?.is_some() {
            return Err(PackageLoadError::Invalid(format!(
                "Duplicate resource payload: {uri}"
            )));
        }
    }
    storage::atomic(store, |store| {
        for element in &incoming.elements {
            let mut exists = false;
            store.inspect(&element.id, &mut |_| {
                exists = true;
                Ok(())
            })?;
            if exists {
                return Err(format!(
                    "Duplicate package/project object identity: {}",
                    element.id
                ));
            }
            if !incoming.resources.contains_key(&element.resource) {
                return Err(format!(
                    "Object {} has no incoming resource record",
                    element.id
                ));
            }
            store.put(element.clone())?;
        }
        for record in incoming.resources.values() {
            store.set_resource_record(record.clone())?;
        }
        Ok(())
    })?;
    Ok(())
}

/// Assemble every package required by the lock and the project's materialized
/// resources in one arena. No selective loading: every distinct locked identity
/// needs exactly one payload. Payloads are ordered by identity, independently of
/// input order. The first prepared arena retains its object indices; subsequent
/// payloads use the same store import operations. Project resources must be
/// untagged. Languages may pass an empty project, then parse sources into the
/// returned store and call `validate_lock_resources` before publication.
pub fn load_against_lock(
    metamodel: Arc<Metamodel>,
    lock: &DependencyLock,
    payloads: &[PackagePayload<'_>],
    project: &Model,
) -> Result<LockedStore, PackageLoadError> {
    let context = ResourceContext::from_lock(metamodel.schema(), lock)?;
    let mut required: BTreeMap<&PackageIdentity, Vec<String>> = BTreeMap::new();
    for (uri, entry) in lock.entries() {
        required
            .entry(&entry.identity)
            .or_default()
            .push(uri.clone());
    }
    let mut available = BTreeMap::new();
    for payload in payloads {
        if available.insert(payload.identity, payload.model).is_some() {
            return Err(PackageLoadError::Invalid(
                "Duplicate package payload identity".into(),
            ));
        }
    }
    let metamodel_digest = metamodel.digest().map_err(|e| e.to_string())?;
    for (&identity, &model) in &available {
        if model.metamodel().digest().map_err(|e| e.to_string())? != metamodel_digest {
            return Err(PackageLoadError::Invalid(
                "Package payload uses another metamodel".into(),
            ));
        }
        let records = model.resource_records().map_err(|e| e.to_string())?;
        for (uri, record) in &records {
            if record.load_state != ResourceLoadState::Loaded {
                return Err(PackageLoadError::Invalid(format!(
                    "Package resource is not loaded: {uri}"
                )));
            }
            if record.package.as_ref() != Some(identity) {
                return Err(PackageLoadError::IdentityMismatch {
                    resource: uri.clone(),
                    expected: Box::new(identity.clone()),
                    actual: record.package.clone().map(Box::new),
                });
            }
        }
        check_records(lock, &records)?;
        if !required.contains_key(identity) {
            return Err(PackageLoadError::Invalid(
                "Package payload identity is absent from the lock".into(),
            ));
        }
    }
    for (identity, uris) in required {
        if !available.contains_key(identity) {
            return Err(PackageLoadError::MissingPayload {
                uris,
                identity: Box::new(identity.clone()),
            });
        }
    }
    check_records(lock, &project.resources)?;
    if project
        .resources
        .values()
        .any(|record| record.package.is_some())
    {
        return Err(PackageLoadError::Invalid(
            "Editable project resources must not carry a package identity".into(),
        ));
    }
    let mut models = available.into_values();
    let mut store = match models.next() {
        Some(model) => FrozenStore::from_model(model.clone()),
        None => FrozenStore::new(metamodel),
    };
    for model in models {
        append(&mut store, &model.to_model().map_err(|e| e.to_string())?)?;
    }
    store.mark_prepared();
    let revision = store.metamodel().revision().to_owned();
    storage::atomic(&mut store, |store| store.set_revision(&revision))?;
    append(&mut store, project)?;
    validate_lock_resources(lock, &store)?;
    Ok(LockedStore { store, context })
}
