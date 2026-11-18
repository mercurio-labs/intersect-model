//! Language-neutral manifest registry and schema-driven Intersect Model services.
//!
//! Registration records metadata only. It does not import definitions, construct
//! models, link references, execute semantic providers, or establish support.

use std::collections::BTreeMap;
use std::collections::btree_map::Entry;
use std::error::Error;
use std::fmt;

use intersect_model::definitions::{DefinitionManifest, PackageId};

pub mod api;
#[cfg(feature = "components")]
pub mod components;
pub mod dynamic;
pub mod frozen;
pub mod interchange;
pub mod model;
pub mod package_load;
pub mod project_usages;
pub mod query;
pub mod resource_cache;
pub mod resource_set;
pub mod resources;
pub mod storage;
pub mod text_document;
pub mod transaction;

#[derive(Debug, Default)]
pub struct PackageRegistry {
    manifests: BTreeMap<PackageId, DefinitionManifest>,
}

impl PackageRegistry {
    /// Register metadata without replacing an existing exact revision.
    pub fn register(&mut self, manifest: DefinitionManifest) -> Result<(), DuplicatePackage> {
        match self.manifests.entry(manifest.package.clone()) {
            Entry::Vacant(entry) => {
                entry.insert(manifest);
                Ok(())
            }
            Entry::Occupied(entry) => Err(DuplicatePackage(entry.key().clone())),
        }
    }

    pub fn get(&self, package: &PackageId) -> Option<&DefinitionManifest> {
        self.manifests.get(package)
    }

    pub fn len(&self) -> usize {
        self.manifests.len()
    }

    pub fn is_empty(&self) -> bool {
        self.manifests.is_empty()
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DuplicatePackage(pub PackageId);

impl fmt::Display for DuplicatePackage {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "package '{}' is already registered", self.0)
    }
}

impl Error for DuplicatePackage {}

#[cfg(test)]
mod tests {
    use super::*;
    use intersect_model::definitions::{ArtifactKind, SourceArtifact};

    #[test]
    fn revisions_can_coexist_without_replacing_each_other() {
        let first_id = PackageId::new("example.library", "revision-a").unwrap();
        let second_id = PackageId::new("example.library", "revision-b").unwrap();
        let mut registry = PackageRegistry::default();
        registry
            .register(DefinitionManifest::new(first_id.clone()))
            .unwrap();
        registry
            .register(DefinitionManifest::new(second_id.clone()))
            .unwrap();
        assert_eq!(registry.len(), 2);
        assert_eq!(registry.get(&first_id).unwrap().package, first_id);
        assert_eq!(registry.get(&second_id).unwrap().package, second_id);
    }

    #[test]
    fn duplicate_registration_preserves_original_provenance() {
        let package = PackageId::new("example.library", "revision-a").unwrap();
        let mut original = DefinitionManifest::new(package.clone());
        original.artifacts.push(SourceArtifact {
            kind: ArtifactKind::Ecore,
            uri: "urn:example:library:metamodel".into(),
            revision: "source-commit-a".into(),
            digest: None,
        });
        let mut registry = PackageRegistry::default();
        registry.register(original.clone()).unwrap();

        assert_eq!(
            registry.register(DefinitionManifest::new(package.clone())),
            Err(DuplicatePackage(package.clone()))
        );
        assert_eq!(registry.get(&package), Some(&original));
    }
}

mod fingerprint;
