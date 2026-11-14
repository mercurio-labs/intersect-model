//! Handwritten source/identity envelope for the expanding definition compiler.
//! This preserves definitions; it does not implement their language constructs.
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

#[path = "bundle_source_validation.rs"]
mod source_validation;
pub(crate) use source_validation::{SourceValidation, input_chunk_end, validate_identity_chunk};
#[path = "bundle_validation.rs"]
mod validation;
pub(crate) use validation::BundleValidation;
use validation::SourceLookup;
#[path = "bundle_source_copy.rs"]
mod source_copy;
pub(crate) use source_copy::{AdmittedSourceCopy, SourceCopyPreparation};

pub const BUNDLE_VERSION: u32 = 1;
pub use crate::definitions::ArtifactKind as SourceKind;

/// URI is opaque: the definition layer never opens a file or a network address.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceId {
    pub uri: String,
    pub sha256: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DefinitionSource {
    pub id: SourceId,
    pub kind: SourceKind,
    pub text: String,
    pub dependencies: Vec<SourceId>,
}

impl SourceId {
    /// Content identity without assigning a language kind. Derived-definition
    /// contexts can bind their exact input manifest with the same identity code.
    pub fn for_text(uri: impl Into<String>, text: &str) -> Self {
        Self {
            uri: uri.into(),
            sha256: format!("{:x}", Sha256::digest(text.as_bytes())),
        }
    }
}

impl DefinitionSource {
    pub fn new(uri: impl Into<String>, kind: SourceKind, text: impl Into<String>) -> Self {
        let text = text.into();
        Self {
            id: SourceId::for_text(uri, &text),
            kind,
            text,
            dependencies: Vec::new(),
        }
    }

    pub fn validate(&self) -> Result<(), String> {
        let mut validation = SourceValidation::new(self);
        while !validation.advance()? {}
        Ok(())
    }
}

/// One explicit resolution context; a namespace never silently selects a revision.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NamespaceBinding {
    pub namespace: String,
    pub source: SourceId,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DefinitionBundle {
    pub version: u32,
    pub sources: Vec<DefinitionSource>,
    pub namespaces: Vec<NamespaceBinding>,
}

pub(crate) enum ValidationStep<'a> {
    Source(&'a DefinitionSource),
    Identity(&'a SourceId),
    Dependency(&'a SourceId),
    Namespace(&'a NamespaceBinding),
    Lookup(&'a SourceId),
}

/// A bundle that passed `DefinitionBundle::validate`, for as long as it stays
/// borrowed and therefore unchanged. Checks that would each validate the same
/// bundle again (and hash every source again) take this instead.
#[derive(Clone, Copy, Debug)]
pub struct ValidatedBundle<'a>(&'a DefinitionBundle);
impl<'a> ValidatedBundle<'a> {
    pub fn bundle(&self) -> &'a DefinitionBundle {
        self.0
    }
}

impl DefinitionBundle {
    /// Validate on load. Deserialization alone never makes a bundle executable.
    pub fn validate(&self) -> Result<(), String> {
        self.validate_with_hooks(&|_| Ok(()))
    }
    /// `validate`, keeping the result for the checks that follow on this
    /// same borrowed bundle.
    pub fn validated(&self) -> Result<ValidatedBundle<'_>, String> {
        self.validate()?;
        Ok(ValidatedBundle(self))
    }
    pub(crate) fn validate_with_hooks(
        &self,
        before: &dyn Fn(ValidationStep<'_>) -> Result<(), String>,
    ) -> Result<(), String> {
        let mut validation = BundleValidation::new(self);
        while !validation.advance(before)? {}
        Ok(())
    }

    fn find_source(&self, id: &SourceId) -> Result<&DefinitionSource, String> {
        self.find_source_with_hooks(id, &|_| Ok(()))
    }
    fn find_source_with_hooks(
        &self,
        id: &SourceId,
        before: &dyn Fn(ValidationStep<'_>) -> Result<(), String>,
    ) -> Result<&DefinitionSource, String> {
        let mut lookup = SourceLookup::new(self, id);
        loop {
            if let Some(source) = lookup.advance(before)? {
                return Ok(source);
            }
        }
    }

    pub fn source(&self, id: &SourceId) -> Result<&DefinitionSource, String> {
        self.validate()?;
        self.find_source(id)
    }

    pub fn resolve_namespace(&self, namespace: &str) -> Result<&DefinitionSource, String> {
        self.validate()?;
        let binding = self
            .namespaces
            .iter()
            .find(|b| b.namespace == namespace)
            .ok_or_else(|| format!("Unbound namespace: {namespace}"))?;
        self.find_source(&binding.source)
    }
}

/// Half-open byte span tied to one exact retained artifact, not its short name.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceSpan {
    pub source: SourceId,
    pub start: usize,
    pub end: usize,
}

impl SourceSpan {
    pub fn text<'a>(&self, bundle: &'a DefinitionBundle) -> Result<&'a str, String> {
        let source = bundle.source(&self.source)?;
        source.text.get(self.start..self.end).ok_or_else(|| {
            "Source span is reversed, out of bounds or not on UTF-8 boundaries".into()
        })
    }

    /// Convert absolute UTF-16 code-unit offsets, rejecting split surrogate pairs.
    pub fn from_utf16(source: &DefinitionSource, start: usize, end: usize) -> Result<Self, String> {
        source.validate()?;
        if start > end {
            return Err("Source span start exceeds end".into());
        }
        fn offset(text: &str, requested: usize) -> Result<usize, String> {
            let mut units = 0;
            for (byte, ch) in text.char_indices() {
                if units == requested {
                    return Ok(byte);
                }
                units += ch.len_utf16();
                if units > requested {
                    return Err("UTF-16 offset splits a surrogate pair".into());
                }
            }
            if units == requested {
                Ok(text.len())
            } else {
                Err("UTF-16 offset exceeds source length".into())
            }
        }
        Ok(Self {
            source: source.id.clone(),
            start: offset(&source.text, start)?,
            end: offset(&source.text, end)?,
        })
    }
}
