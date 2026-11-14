//! Definition identities, native model compilation, migration and semantic contracts.
//!
//! Preserving imported artifacts does not make their constructs executable.

use std::error::Error;
use std::fmt;

pub mod dynamic;

pub mod bundle;
pub mod compiler;
mod continuation;
pub mod conversion;
pub mod datatypes;
pub mod declarations;
mod declared_operations;
pub mod ecore;
pub mod ecore_document;
mod ecore_identifier;
pub mod ecore_migration;
pub mod enums;
pub mod features;
pub mod generic_resolution;
pub mod generic_types;
pub mod generics;
pub mod initialization;
mod json_digest;
pub(crate) mod json_exact;
pub mod library_part;
pub mod metadata_limits;
pub mod model_definition;
#[cfg(test)]
pub(crate) mod model_test_support;
// Private candidate: default-session adoption requires owner/admission review.
mod operation_selection;
mod operation_templates;
pub mod operations;
pub mod pack;
pub mod preparation;
pub mod rcore;
pub mod rcore_behavior;
mod rcore_behavior_rules;
pub mod rcore_getter;
pub(crate) mod rcore_names;
pub mod rcore_operations;
pub mod rcore_scope;
pub mod resources;
// Allocation probes need the admission contract; ordinary consumers do not.
#[cfg(feature = "allocation-profile")]
pub mod retained_json;
#[cfg(not(feature = "allocation-profile"))]
pub(crate) mod retained_json;
pub(crate) mod retained_json_frontier;
pub mod retained_storage;
pub(crate) mod retained_text_equal;
pub mod rule_conversion;
pub mod scalars;
pub mod schema;
pub mod semantic_bindings;
pub mod setting;

/// Identity of one exact language-package revision.
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct PackageId {
    language: String,
    revision: String,
}

impl PackageId {
    pub fn new(
        language: impl Into<String>,
        revision: impl Into<String>,
    ) -> Result<Self, InvalidPackageId> {
        let language = language.into();
        let revision = revision.into();
        if language.trim().is_empty() {
            return Err(InvalidPackageId::EmptyLanguage);
        }
        if revision.trim().is_empty() {
            return Err(InvalidPackageId::EmptyRevision);
        }
        Ok(Self { language, revision })
    }

    pub fn language(&self) -> &str {
        &self.language
    }

    pub fn revision(&self) -> &str {
        &self.revision
    }
}

impl fmt::Display for PackageId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}@{}", self.language, self.revision)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum InvalidPackageId {
    EmptyLanguage,
    EmptyRevision,
}

impl fmt::Display for InvalidPackageId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptyLanguage => f.write_str("a package needs a language identity"),
            Self::EmptyRevision => f.write_str("a package needs an explicit revision"),
        }
    }
}

impl Error for InvalidPackageId {}

#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ArtifactKind {
    /// Authored Rcore structure with optional behavior; never generated XML.
    Rcore,
    Ecore,
    Xtext,
    RcoreBehavior,
}

/// Digest metadata. Verification of artifact contents is future importer work.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ContentDigest {
    pub algorithm: String,
    pub value: String,
}

/// Source metadata; recording an artifact does not import or implement it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SourceArtifact {
    pub kind: ArtifactKind,
    pub uri: String,
    pub revision: String,
    pub digest: Option<ContentDigest>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DefinitionManifest {
    pub package: PackageId,
    pub artifacts: Vec<SourceArtifact>,
}

impl DefinitionManifest {
    pub fn new(package: PackageId) -> Self {
        Self {
            package,
            artifacts: Vec::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn package_identity_preserves_language_and_revision() {
        let original = PackageId::new("example.library", "revision-a").unwrap();
        let updated = PackageId::new("example.library", "revision-b").unwrap();
        let other = PackageId::new("example.inventory", "revision-a").unwrap();
        assert_ne!(original, updated);
        assert_ne!(original, other);
    }

    #[test]
    fn package_identity_requires_both_components() {
        assert_eq!(
            PackageId::new(" ", "revision-a"),
            Err(InvalidPackageId::EmptyLanguage)
        );
        assert_eq!(
            PackageId::new("example.library", ""),
            Err(InvalidPackageId::EmptyRevision)
        );
    }
}

#[cfg(test)]
pub(crate) mod migration_test_fixture;

#[cfg(test)]
#[path = "runtime_schema_test_inputs.rs"]
pub(crate) mod runtime_schema_test_inputs;

#[cfg(test)]
mod generics_decoder_new_contract_capture;
