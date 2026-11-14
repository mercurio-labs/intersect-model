//! Handwritten host-side acquisition contracts for model packages.
use intersect_model::{
    definitions::bundle::SourceId,
    project_usages::{
        PackageCatalog, PackageIdentity, PackageManifest, ResolutionError, Usage, UsageResolver,
    },
};
use std::{error::Error, fmt};

/// Acquires one exact source package through a host-selected route.
pub trait PackageSource {
    type Payload;
    type Error: Error;

    fn acquire(&self) -> Result<SourcePackage<Self::Payload>, Self::Error>;
}

/// An acquired package with its authored usages and route-specific payload.
#[derive(Clone, Debug, PartialEq)]
pub struct SourcePackage<T> {
    pub uri: String,
    pub version: String,
    pub provenance: SourceId,
    /// Digest of the exact source content under this acquisition route.
    pub content_digest: String,
    pub usages: Vec<Usage>,
    pub payload: T,
}

impl<T> SourcePackage<T> {
    /// Build a manifest bound to the package's own resolved dependency lock.
    pub fn manifest(
        &self,
        engine: &str,
        schema: &str,
        dependencies: &dyn PackageCatalog,
    ) -> Result<PackageManifest, ManifestError> {
        if engine.trim().is_empty() || schema.trim().is_empty() {
            return Err(ManifestError::MissingBuildContext);
        }
        let lock = UsageResolver::resolve(&self.usages, dependencies)
            .map_err(ManifestError::Dependencies)?;
        Ok(PackageManifest {
            uri: self.uri.clone(),
            version: self.version.clone(),
            provenance: self.provenance.clone(),
            usages: self.usages.clone(),
            identity: PackageIdentity {
                content_digest: self.content_digest.clone(),
                dependency_lock_digest: lock.digest().map_err(ManifestError::LockDigest)?,
                engine: engine.to_owned(),
                schema: schema.to_owned(),
            },
            dependency_lock: lock,
        })
    }
}

#[derive(Debug)]
pub enum ManifestError {
    MissingBuildContext,
    Dependencies(ResolutionError),
    LockDigest(String),
}

impl fmt::Display for ManifestError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingBuildContext => {
                f.write_str("Package registration requires exact engine and schema identities")
            }
            Self::Dependencies(error) => write!(f, "Cannot lock package dependencies: {error}"),
            Self::LockDigest(error) => {
                write!(f, "Cannot fingerprint package dependency lock: {error}")
            }
        }
    }
}

impl Error for ManifestError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Dependencies(error) => Some(error),
            _ => None,
        }
    }
}
