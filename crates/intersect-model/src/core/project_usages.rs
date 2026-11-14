//! Handwritten model-package contracts and deterministic usage resolution.
use crate::{core::fingerprint::digest, definitions::bundle::SourceId};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    fmt,
};

/// A project address and an optional source-specific version constraint.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Usage {
    pub resource_uri: String,
    pub version_constraint: Option<String>,
}

/// Exact identity of model content in its dependency and build context.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PackageIdentity {
    pub content_digest: String,
    pub dependency_lock_digest: String,
    pub engine: String,
    pub schema: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PackageManifest {
    pub uri: String,
    pub version: String,
    pub provenance: SourceId,
    pub identity: PackageIdentity,
    pub usages: Vec<Usage>,
    /// The exact dependency snapshot used when this package was built.
    pub dependency_lock: DependencyLock,
}

/// An already available, consistent catalog snapshot. Return all candidates
/// satisfying the request. Constraint interpretation belongs to the catalog.
pub trait PackageCatalog {
    fn candidates(&self, uri: &str, constraint: Option<&str>) -> Vec<PackageManifest>;

    /// Validate source-specific constraint syntax before candidate lookup.
    fn validate_usage(&self, _usage: &Usage) -> Result<(), String> {
        Ok(())
    }
}

/// Requests from the implicit root to the package, including the final usage.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct UsagePath(pub Vec<Usage>);

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LockEntry {
    pub identity: PackageIdentity,
    pub direct: bool,
    pub requested_by: Vec<UsagePath>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
enum LockFormat {
    #[serde(rename = "intersect.dependency-lock.v1")]
    V1,
}

/// A resolved snapshot keyed by logical package URI.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DependencyLock {
    format: LockFormat,
    root_usages: Vec<Usage>,
    entries: BTreeMap<String, LockEntry>,
}

impl DependencyLock {
    pub fn empty() -> Self {
        Self {
            format: LockFormat::V1,
            root_usages: Vec::new(),
            entries: BTreeMap::new(),
        }
    }

    pub fn root_usages(&self) -> &[Usage] {
        &self.root_usages
    }

    pub fn entries(&self) -> &BTreeMap<String, LockEntry> {
        &self.entries
    }

    /// Lowercase SHA-256 of the compact JSON encoding of a resolver-produced lock.
    pub fn digest(&self) -> Result<String, String> {
        digest(self)
    }
}

/// Bounds all root paths and all recorded per-package paths checked in a run.
#[derive(Clone, Copy, Debug)]
pub struct ResolutionLimits {
    pub max_paths: usize,
}

impl Default for ResolutionLimits {
    fn default() -> Self {
        Self { max_paths: 10_000 }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ResolutionError {
    Conflict {
        uri: String,
        paths: Vec<UsagePath>,
    },
    Unsatisfied {
        usage: Usage,
        path: UsagePath,
    },
    Ambiguous {
        usage: Usage,
        candidates: Vec<PackageIdentity>,
    },
    LockMismatch {
        package: String,
        uri: String,
        recorded: Option<Box<PackageIdentity>>,
        path: UsagePath,
        reason: String,
    },
    Constraint {
        usage: Usage,
        reason: String,
    },
    Cycle {
        members: Vec<String>,
    },
    PathLimitExceeded {
        limit: usize,
    },
    Invariant {
        reason: String,
    },
}

impl fmt::Display for ResolutionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Conflict { uri, paths } => {
                write!(f, "Incompatible packages for {uri}: {paths:?}")
            }
            Self::Unsatisfied { usage, path } => {
                write!(f, "Unsatisfied usage {usage:?} on path {path:?}")
            }
            Self::Ambiguous { usage, candidates } => write!(
                f,
                "Ambiguous usage {usage:?} with {} candidate identities. Add a constraint to select one",
                candidates.len()
            ),
            Self::LockMismatch {
                package,
                uri,
                recorded,
                path,
                reason,
            } => write!(
                f,
                "Dependency lock mismatch for {package}, dependency {uri}, recorded {recorded:?}, path {path:?}: {reason}"
            ),
            Self::Constraint { usage, reason } => {
                write!(f, "Invalid constraint for {usage:?}: {reason}")
            }
            Self::Cycle { members } => write!(f, "Unsupported package cycle: {members:?}"),
            Self::PathLimitExceeded { limit } => {
                write!(f, "Dependency path limit {limit} exceeded")
            }
            Self::Invariant { reason } => {
                write!(f, "Dependency resolver invariant failed: {reason}")
            }
        }
    }
}

impl std::error::Error for ResolutionError {}

fn invariant(reason: impl Into<String>) -> ResolutionError {
    ResolutionError::Invariant {
        reason: reason.into(),
    }
}

pub struct UsageResolver;

impl UsageResolver {
    pub fn resolve(
        roots: &[Usage],
        catalog: &dyn PackageCatalog,
    ) -> Result<DependencyLock, ResolutionError> {
        Self::resolve_with_limits(roots, catalog, ResolutionLimits::default())
    }

    pub fn resolve_with_limits(
        roots: &[Usage],
        catalog: &dyn PackageCatalog,
        limits: ResolutionLimits,
    ) -> Result<DependencyLock, ResolutionError> {
        let roots = canonical_usages(roots.to_vec());
        let (requests, selected) = discover(&roots, catalog, limits)?;
        let (paths, cycle) = request_paths(&roots, &requests, &selected, limits)?;
        for (uri, paths) in &paths {
            let group = requests
                .get(uri)
                .ok_or_else(|| invariant(format!("Missing requests for {uri}")))?;
            let recorded: BTreeSet<_> = paths
                .iter()
                .filter_map(|observation| observation.request.recorded.as_ref())
                .collect();
            if recorded.len() > 1 {
                return Err(ResolutionError::Conflict {
                    uri: uri.clone(),
                    paths: paths.iter().map(|p| p.path.clone()).collect(),
                });
            }
            for observation in paths {
                let request = &observation.request;
                let demand = group
                    .demands
                    .get(request)
                    .ok_or_else(|| invariant(format!("Missing demand for {uri}")))?;
                if let Some(reason) = &demand.mismatch {
                    return Err(request.mismatch(observation.path.clone(), reason.clone()));
                }
                if demand.identities.is_empty() {
                    return Err(ResolutionError::Unsatisfied {
                        usage: request.usage.clone(),
                        path: observation.path.clone(),
                    });
                }
            }
            let demands = paths
                .iter()
                .map(|observation| {
                    group
                        .demands
                        .get(&observation.request)
                        .ok_or_else(|| invariant(format!("Missing demand for {uri}")))
                })
                .collect::<Result<Vec<_>, _>>()?;
            let identities = matching_identities(demands);
            if identities.is_empty() {
                return Err(ResolutionError::Conflict {
                    uri: uri.clone(),
                    paths: paths.iter().map(|p| p.path.clone()).collect(),
                });
            }
            if identities.len() > 1 {
                let first = paths
                    .first()
                    .ok_or_else(|| invariant("Empty requesting paths"))?;
                return Err(ResolutionError::Ambiguous {
                    usage: first.request.usage.clone(),
                    candidates: identities,
                });
            }
        }
        if let Some(members) = cycle {
            return Err(ResolutionError::Cycle { members });
        }
        validate_recorded_locks(&paths, &selected, limits)?;
        let entries = paths
            .into_iter()
            .map(|(uri, observations)| {
                let manifest = selected
                    .get(&uri)
                    .ok_or_else(|| invariant(format!("Missing selection for {uri}")))?;
                let requested_by: Vec<_> = observations.into_iter().map(|o| o.path).collect();
                let direct = requested_by.iter().any(|p| p.0.len() == 1);
                Ok((
                    uri,
                    LockEntry {
                        identity: manifest.identity.clone(),
                        direct,
                        requested_by,
                    },
                ))
            })
            .collect::<Result<_, ResolutionError>>()?;
        Ok(DependencyLock {
            format: LockFormat::V1,
            root_usages: roots,
            entries,
        })
    }
}

fn canonical_usages(mut usages: Vec<Usage>) -> Vec<Usage> {
    usages.sort();
    usages.dedup();
    usages
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct Request {
    usage: Usage,
    package: Option<String>,
    recorded: Option<PackageIdentity>,
}

impl Request {
    fn root(usage: Usage) -> Self {
        Self {
            usage,
            package: None,
            recorded: None,
        }
    }

    fn dependency(manifest: &PackageManifest, usage: Usage) -> Self {
        let recorded = manifest
            .dependency_lock
            .entries
            .get(&usage.resource_uri)
            .filter(|entry| entry.direct)
            .map(|entry| entry.identity.clone());
        Self {
            usage,
            package: Some(manifest.uri.clone()),
            recorded,
        }
    }

    fn mismatch(&self, path: UsagePath, reason: String) -> ResolutionError {
        ResolutionError::LockMismatch {
            package: self
                .package
                .clone()
                .unwrap_or_else(|| self.usage.resource_uri.clone()),
            uri: self.usage.resource_uri.clone(),
            recorded: self.recorded.clone().map(Box::new),
            path,
            reason,
        }
    }
}

struct Demand {
    identities: BTreeSet<PackageIdentity>,
    mismatch: Option<String>,
}

#[derive(Default)]
struct Requests {
    demands: BTreeMap<Request, Demand>,
    manifests: BTreeMap<PackageIdentity, PackageManifest>,
}

impl Requests {
    fn matching(&self) -> Vec<PackageIdentity> {
        matching_identities(self.demands.values())
    }

    fn add(
        &mut self,
        request: Request,
        catalog: &dyn PackageCatalog,
    ) -> Result<(), ResolutionError> {
        if self.demands.contains_key(&request) {
            return Ok(());
        }
        catalog
            .validate_usage(&request.usage)
            .map_err(|reason| ResolutionError::Constraint {
                usage: request.usage.clone(),
                reason,
            })?;
        let mut identities = BTreeSet::new();
        for mut manifest in catalog.candidates(
            &request.usage.resource_uri,
            request.usage.version_constraint.as_deref(),
        ) {
            if manifest.uri != request.usage.resource_uri {
                continue;
            }
            manifest.usages = canonical_usages(manifest.usages);
            if let Some(recorded) = &request.recorded
                && &manifest.identity != recorded
            {
                continue;
            }
            identities.insert(manifest.identity.clone());
            if let Some(previous) = self.manifests.get(&manifest.identity)
                && (previous.usages != manifest.usages
                    || previous.dependency_lock != manifest.dependency_lock)
            {
                return Err(invariant(format!(
                    "Catalog gave inconsistent manifests for {}",
                    manifest.uri
                )));
            }
            self.manifests.insert(manifest.identity.clone(), manifest);
        }
        let mismatch = if request.package.is_none() {
            None
        } else if let Some(recorded) = &request.recorded {
            if identities.is_empty() {
                let present = catalog
                    .candidates(&request.usage.resource_uri, None)
                    .iter()
                    .any(|m| m.uri == request.usage.resource_uri && &m.identity == recorded);
                Some(
                    if present {
                        "Recorded identity violates the declared constraint"
                    } else {
                        "Recorded identity is absent from the catalog"
                    }
                    .to_owned(),
                )
            } else {
                None
            }
        } else {
            identities.clear();
            Some("Package lock has no direct identity for the declared usage".to_owned())
        };
        self.demands.insert(
            request,
            Demand {
                identities,
                mismatch,
            },
        );
        Ok(())
    }
}

fn matching_identities<'a>(demands: impl IntoIterator<Item = &'a Demand>) -> Vec<PackageIdentity> {
    let mut demands = demands.into_iter();
    let Some(first) = demands.next() else {
        return Vec::new();
    };
    let mut matching = first.identities.clone();
    for demand in demands {
        matching.retain(|identity| demand.identities.contains(identity));
    }
    matching.into_iter().collect()
}

type Selected = BTreeMap<String, PackageManifest>;
type Groups = BTreeMap<String, Requests>;

fn discover(
    roots: &[Usage],
    catalog: &dyn PackageCatalog,
    limits: ResolutionLimits,
) -> Result<(Groups, Selected), ResolutionError> {
    let mut requests: Groups = BTreeMap::new();
    let mut selected = Selected::new();
    let mut pending: Vec<_> = roots.iter().cloned().map(Request::root).collect();
    while !pending.is_empty() {
        let mut changed = BTreeSet::new();
        for request in pending.drain(..) {
            let uri = request.usage.resource_uri.clone();
            requests
                .entry(uri.clone())
                .or_default()
                .add(request, catalog)?;
            changed.insert(uri);
        }
        for uri in changed {
            if selected.contains_key(&uri) {
                continue;
            }
            let group = requests
                .get(&uri)
                .ok_or_else(|| invariant(format!("Missing request group for {uri}")))?;
            let matching = group.matching();
            if let [identity] = matching.as_slice() {
                if selected.len() >= limits.max_paths {
                    return Err(ResolutionError::PathLimitExceeded {
                        limit: limits.max_paths,
                    });
                }
                let manifest = group
                    .manifests
                    .get(identity)
                    .ok_or_else(|| invariant(format!("Missing manifest for {uri}")))?
                    .clone();
                pending.extend(
                    manifest
                        .usages
                        .iter()
                        .cloned()
                        .map(|u| Request::dependency(&manifest, u)),
                );
                selected.insert(uri, manifest);
            }
        }
    }
    Ok((requests, selected))
}

struct Observation {
    request: Request,
    path: UsagePath,
}

type RequestPaths = BTreeMap<String, Vec<Observation>>;

fn charge_paths(
    total: &mut usize,
    amount: usize,
    limits: ResolutionLimits,
) -> Result<(), ResolutionError> {
    *total = total
        .checked_add(amount)
        .filter(|n| *n <= limits.max_paths)
        .ok_or(ResolutionError::PathLimitExceeded {
            limit: limits.max_paths,
        })?;
    Ok(())
}

fn request_paths(
    roots: &[Usage],
    requests: &Groups,
    selected: &Selected,
    limits: ResolutionLimits,
) -> Result<(RequestPaths, Option<Vec<String>>), ResolutionError> {
    let mut total = 0;
    charge_paths(&mut total, roots.len(), limits)?;
    let mut pending: Vec<_> = roots
        .iter()
        .rev()
        .map(|u| Observation {
            request: Request::root(u.clone()),
            path: UsagePath(vec![u.clone()]),
        })
        .collect();
    let mut paths: RequestPaths = BTreeMap::new();
    let mut cycle = None;
    while let Some(observation) = pending.pop() {
        let uri = &observation.request.usage.resource_uri;
        let group = requests
            .get(uri)
            .ok_or_else(|| invariant(format!("Missing request group for {uri}")))?;
        let demand = group
            .demands
            .get(&observation.request)
            .ok_or_else(|| invariant(format!("Missing demand for {uri}")))?;
        if let Some(manifest) = selected.get(uri).filter(|manifest| {
            demand.identities.contains(&manifest.identity)
                && group
                    .demands
                    .values()
                    .all(|demand| demand.identities.contains(&manifest.identity))
        }) {
            let (_, parents) = observation
                .path
                .0
                .split_last()
                .ok_or_else(|| invariant("Empty requesting path"))?;
            if let Some(start) = parents.iter().position(|u| &u.resource_uri == uri) {
                let mut members: Vec<_> = parents
                    .iter()
                    .skip(start)
                    .map(|u| u.resource_uri.clone())
                    .collect();
                members.sort();
                if cycle.as_ref().is_none_or(|previous| &members < previous) {
                    cycle = Some(members);
                }
            } else {
                charge_paths(&mut total, manifest.usages.len(), limits)?;
                for child in manifest.usages.iter().rev() {
                    let mut path = observation.path.clone();
                    path.0.push(child.clone());
                    pending.push(Observation {
                        request: Request::dependency(manifest, child.clone()),
                        path,
                    });
                }
            }
        }
        paths.entry(uri.clone()).or_default().push(observation);
    }
    for paths in paths.values_mut() {
        paths.sort_by(|a, b| a.path.cmp(&b.path));
    }
    Ok((paths, cycle))
}

fn first_path<'a>(paths: &'a RequestPaths, uri: &str) -> Result<&'a UsagePath, ResolutionError> {
    paths
        .get(uri)
        .and_then(|p| p.first())
        .map(|o| &o.path)
        .ok_or_else(|| invariant(format!("Selected package {uri} has no requesting path")))
}

fn lock_mismatch(
    manifest: &PackageManifest,
    path: UsagePath,
    uri: &str,
    recorded: Option<PackageIdentity>,
    reason: &str,
) -> ResolutionError {
    ResolutionError::LockMismatch {
        package: manifest.uri.clone(),
        uri: uri.to_owned(),
        recorded: recorded.map(Box::new),
        path,
        reason: reason.to_owned(),
    }
}

/// Verify projections in one pass over the shared walk. For each package,
/// only paths below its first root occurrence contribute to its own lock.
fn validate_recorded_locks(
    paths: &RequestPaths,
    selected: &Selected,
    limits: ResolutionLimits,
) -> Result<(), ResolutionError> {
    let mut total = 0;
    for observations in paths.values() {
        charge_paths(&mut total, observations.len(), limits)?;
    }
    for (uri, manifest) in selected {
        let path = first_path(paths, uri)?;
        let lock = &manifest.dependency_lock;
        let actual_digest = lock.digest().map_err(|reason| {
            invariant(format!("Cannot digest recorded lock for {uri}: {reason}"))
        })?;
        if actual_digest != manifest.identity.dependency_lock_digest
            || lock.root_usages != manifest.usages
        {
            return Err(lock_mismatch(
                manifest,
                path.clone(),
                uri,
                Some(manifest.identity.clone()),
                "Recorded lock roots or digest disagree with the manifest",
            ));
        }
        for entry in lock.entries.values() {
            charge_paths(&mut total, entry.requested_by.len(), limits)?;
            if entry.requested_by.is_empty()
                || entry
                    .requested_by
                    .windows(2)
                    .any(|pair| pair.first() >= pair.get(1))
                || entry.direct != entry.requested_by.iter().any(|p| p.0.len() == 1)
            {
                return Err(lock_mismatch(
                    manifest,
                    path.clone(),
                    uri,
                    Some(entry.identity.clone()),
                    "Recorded lock paths or direct flag are not canonical",
                ));
            }
        }
    }
    let mut counts: BTreeMap<String, usize> = BTreeMap::new();
    for (uri, observations) in paths {
        let target = selected
            .get(uri)
            .ok_or_else(|| invariant(format!("Missing selected dependency {uri}")))?;
        for observation in observations {
            let path = &observation.path;
            for length in 1..path.0.len() {
                let prefix = path
                    .0
                    .get(..length)
                    .ok_or_else(|| invariant("Invalid path prefix"))?;
                let ancestor = prefix
                    .last()
                    .ok_or_else(|| invariant("Empty ancestor path"))?;
                if first_path(paths, &ancestor.resource_uri)?.0 != prefix {
                    continue;
                }
                let manifest = selected.get(&ancestor.resource_uri).ok_or_else(|| {
                    invariant(format!("Missing ancestor {}", ancestor.resource_uri))
                })?;
                let suffix = UsagePath(
                    path.0
                        .get(length..)
                        .ok_or_else(|| invariant("Invalid path suffix"))?
                        .to_vec(),
                );
                let entry = manifest.dependency_lock.entries.get(uri);
                if !entry.is_some_and(|e| {
                    e.identity == target.identity && e.requested_by.binary_search(&suffix).is_ok()
                }) {
                    return Err(lock_mismatch(
                        manifest,
                        path.clone(),
                        uri,
                        entry.map(|e| e.identity.clone()),
                        "Recorded lock disagrees with the resolved dependency paths",
                    ));
                }
                *counts.entry(ancestor.resource_uri.clone()).or_default() += 1;
            }
        }
    }
    for (uri, manifest) in selected {
        let declared = manifest
            .dependency_lock
            .entries
            .values()
            .try_fold(0usize, |n, e| n.checked_add(e.requested_by.len()))
            .ok_or_else(|| invariant("Recorded path count overflow"))?;
        if counts.get(uri).copied().unwrap_or(0) != declared {
            return Err(lock_mismatch(
                manifest,
                first_path(paths, uri)?.clone(),
                uri,
                Some(manifest.identity.clone()),
                "Recorded lock contains paths outside the resolved dependency closure",
            ));
        }
    }
    Ok(())
}
