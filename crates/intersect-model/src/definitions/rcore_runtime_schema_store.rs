//! Runtime-schema preparation results shared between model-type contexts.
//!
//! A context's schema, closure and generic-check caches are pure functions of
//! its fixed bundle, parsed source resources and source limits. Contexts built
//! for the same immutable sources (every check, lowering and verification of a
//! binding build) otherwise re-materialize the same metamodel closures, which
//! dominated binding compile time. A store lets them reuse completed results.
//!
//! What each context records stays exact: importing an entry adds precisely
//! what a cold preparation of that key would have added to the context (its
//! schema, closure and the generic checks of the closure's members), so reports
//! derived from a context do not depend on what other contexts prepared.
//!
//! Contexts on different threads typically request the same keys at the same
//! time. A context preparing a key cold claims it; another thread's context
//! that holds no claim of its own may then await that preparation instead of
//! repeating it. Claimers never wait, so waiting cannot deadlock, and a claim
//! released without a result (failure or cancellation) lets a waiter prepare
//! the key itself. Waiting only changes who computes a result, never the result.
use super::CheckedSchema;
use crate::definitions::bundle::{DefinitionBundle, NamespaceBinding, SourceId};
use crate::definitions::generic_resolution::RuntimeMembers;
use crate::definitions::generic_types::Type as ModelType;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::thread::ThreadId;

/// Upper bound of one awaiting step, so each step of a preparation that waits
/// for another thread stays bounded and its caller keeps control.
const AWAIT_STEP: std::time::Duration = std::time::Duration::from_millis(5);

/// Shared, thread-safe store of completed runtime-schema preparations.
///
/// Cloning shares the store. The first context attached to a store binds it to
/// that context's sources and limits; contexts with other sources or limits, or
/// with a conversion registry, are not attached and prepare everything
/// themselves. Entries are only added after a preparation completes, never
/// removed, and never replaced, so a store only grows by checked results.
#[derive(Clone, Default)]
pub struct RuntimeSchemaStore {
    shared: Arc<Shared>,
}

#[derive(Default)]
struct Shared {
    state: Mutex<StoreState>,
    published: Condvar,
}

#[derive(Default)]
struct StoreState {
    identity: Option<StoreIdentity>,
    entries: BTreeMap<Vec<ModelType>, Arc<StoreEntry>>,
    /// Keys being prepared cold, by the thread preparing them.
    claims: BTreeMap<Vec<ModelType>, ThreadId>,
}

/// What a context should do about a key it has not prepared itself.
pub(super) enum Lookup {
    /// Another context completed it: import the entry.
    Ready(Arc<StoreEntry>),
    /// Prepare it cold, holding this claim (if any) until publication.
    Prepare(Option<Claim>),
    /// Another thread is preparing it: await its result.
    Busy,
}

/// A thread's claim to prepare one key. Publication or drop releases it and
/// wakes contexts awaiting the key.
pub(super) struct Claim {
    store: RuntimeSchemaStore,
    key: Vec<ModelType>,
    owner: ThreadId,
}
impl Drop for Claim {
    fn drop(&mut self) {
        if let Some(mut state) = self.store.lock()
            && state.claims.get(&self.key) == Some(&self.owner)
        {
            state.claims.remove(&self.key);
        }
        self.store.shared.published.notify_all();
    }
}

/// The inputs a context's preparations depend on. Source identities carry
/// their content digests, so equal identities mean equal parsed resources.
#[derive(PartialEq, Eq)]
struct StoreIdentity {
    version: u32,
    sources: Vec<SourceId>,
    namespaces: Vec<NamespaceBinding>,
    limits: crate::definitions::ecore::ParseLimits,
}
impl StoreIdentity {
    fn of(bundle: &DefinitionBundle, limits: crate::definitions::ecore::ParseLimits) -> Self {
        Self {
            version: bundle.version,
            sources: bundle
                .sources
                .iter()
                .map(|source| source.id.clone())
                .collect(),
            namespaces: bundle.namespaces.clone(),
            limits,
        }
    }
}

/// One completed preparation: the checked schema of a key, the exact closure
/// it was materialized from, and the generic checks of that closure's members.
pub(super) struct StoreEntry {
    pub(super) schema: Arc<CheckedSchema>,
    pub(super) closure: Vec<ModelType>,
    pub(super) generic_checks: Vec<(ModelType, RuntimeMembers)>,
}

impl RuntimeSchemaStore {
    pub fn new() -> Self {
        Self::default()
    }

    fn lock(&self) -> Option<MutexGuard<'_, StoreState>> {
        self.shared.state.lock().ok()
    }

    /// Number of completed preparations held (diagnostics and tests).
    pub fn len(&self) -> usize {
        self.lock().map(|state| state.entries.len()).unwrap_or(0)
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Whether a context over `bundle` and `limits` may use this store, binding
    /// an unbound store to them.
    pub(super) fn admits(
        &self,
        bundle: &DefinitionBundle,
        limits: crate::definitions::ecore::ParseLimits,
    ) -> bool {
        let identity = StoreIdentity::of(bundle, limits);
        let Some(mut state) = self.lock() else {
            return false;
        };
        match &state.identity {
            Some(bound) => *bound == identity,
            None => {
                state.identity = Some(identity);
                true
            }
        }
    }

    /// Decide how a context that has not prepared `key` obtains it. Only a
    /// context that `may_wait` (no observer metering or pausing its steps, so it
    /// drives each preparation to completion or failure) claims keys, and only
    /// such a context whose thread holds no claim awaits another thread's.
    pub(super) fn lookup(&self, key: &[ModelType], may_wait: bool) -> Lookup {
        match self.lock() {
            Some(state) => self.decide(state, key, may_wait),
            None => Lookup::Prepare(None),
        }
    }

    /// One bounded awaiting step for `key`: wait up to `AWAIT_STEP` for its
    /// publication or release, then decide again.
    pub(super) fn await_step(&self, key: &[ModelType]) -> Lookup {
        let Some(state) = self.lock() else {
            return Lookup::Prepare(None);
        };
        let state = if state.entries.contains_key(key) || !state.claims.contains_key(key) {
            state
        } else {
            match self.shared.published.wait_timeout(state, AWAIT_STEP) {
                Ok((state, _)) => state,
                Err(_) => return Lookup::Prepare(None),
            }
        };
        self.decide(state, key, true)
    }

    fn decide(
        &self,
        mut state: MutexGuard<'_, StoreState>,
        key: &[ModelType],
        may_wait: bool,
    ) -> Lookup {
        if let Some(entry) = state.entries.get(key) {
            return Lookup::Ready(entry.clone());
        }
        if !may_wait {
            return Lookup::Prepare(None);
        }
        let thread = std::thread::current().id();
        match state.claims.get(key) {
            None => {
                state.claims.insert(key.to_vec(), thread);
                Lookup::Prepare(Some(Claim {
                    store: self.clone(),
                    key: key.to_vec(),
                    owner: thread,
                }))
            }
            Some(owner)
                if *owner != thread && !state.claims.values().any(|owner| *owner == thread) =>
            {
                Lookup::Busy
            }
            Some(_) => Lookup::Prepare(None),
        }
    }

    /// Record a completed preparation of `key`, keeping any earlier entry:
    /// equal inputs give equal results, so the first one stands.
    pub(super) fn publish(
        &self,
        key: &[ModelType],
        schema: &Arc<CheckedSchema>,
        closure: &[ModelType],
        generic_checks: &BTreeMap<ModelType, RuntimeMembers>,
    ) {
        let Some(mut state) = self.lock() else {
            return;
        };
        // Once an entry exists nobody needs to wait for or claim the key.
        state.claims.remove(key);
        self.shared.published.notify_all();
        if state.entries.contains_key(key) {
            return;
        }
        let members: BTreeSet<&ModelType> = closure.iter().collect();
        let entry = StoreEntry {
            schema: schema.clone(),
            closure: closure.to_vec(),
            generic_checks: generic_checks
                .iter()
                .filter(|(ty, _)| members.contains(ty))
                .map(|(ty, report)| (ty.clone(), report.clone()))
                .collect(),
        };
        state.entries.insert(key.to_vec(), Arc::new(entry));
    }
}

impl std::fmt::Debug for RuntimeSchemaStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RuntimeSchemaStore")
            .field("entries", &self.len())
            .finish()
    }
}
