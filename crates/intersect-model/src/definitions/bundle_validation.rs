//! One bundle validator for synchronous callers and retained resource admission.
//! Index entries borrow immutable inputs; callbacks belong to individual advances.
use super::{
    BUNDLE_VERSION, DefinitionBundle, DefinitionSource, NamespaceBinding, SourceId, SourceKind,
    SourceValidation, ValidationStep, validate_identity_chunk,
};
use std::collections::BTreeSet;

pub(super) struct SourceLookup<'bundle, 'query> {
    bundle: &'bundle DefinitionBundle,
    id: &'query SourceId,
    index: usize,
}
impl<'bundle, 'query> SourceLookup<'bundle, 'query> {
    pub(super) fn new(bundle: &'bundle DefinitionBundle, id: &'query SourceId) -> Self {
        Self {
            bundle,
            id,
            index: 0,
        }
    }
    pub(super) fn advance(
        &mut self,
        before: &dyn Fn(ValidationStep<'_>) -> Result<(), String>,
    ) -> Result<Option<&'bundle DefinitionSource>, String> {
        let source = self.bundle.sources.get(self.index).ok_or_else(|| {
            format!(
                "Missing exact definition source {}@{}",
                self.id.uri, self.id.sha256
            )
        })?;
        before(ValidationStep::Lookup(&source.id))?;
        self.index += 1;
        Ok((source.id == *self.id).then_some(source))
    }
}

enum Phase {
    Version,
    Sources,
    SourceInput,
    Identity,
    Dependencies,
    Namespaces,
    NamespaceText,
    NamespaceIdentity,
    NamespaceLookup,
    Release,
    Complete,
}

pub(crate) struct BundleValidation<'bundle> {
    bundle: &'bundle DefinitionBundle,
    phase: Phase,
    source_index: usize,
    dependency_index: usize,
    namespace_index: usize,
    namespace_offset: usize,
    namespace_non_whitespace: bool,
    source_input: Option<SourceValidation<'bundle>>,
    lookup: Option<SourceLookup<'bundle, 'bundle>>,
    identities: BTreeSet<&'bundle SourceId>,
    dependencies: BTreeSet<&'bundle SourceId>,
    namespaces: BTreeSet<&'bundle String>,
    failure: Option<String>,
}
impl<'bundle> BundleValidation<'bundle> {
    pub(crate) fn new(bundle: &'bundle DefinitionBundle) -> Self {
        Self {
            bundle,
            phase: Phase::Version,
            source_index: 0,
            dependency_index: 0,
            namespace_index: 0,
            namespace_offset: 0,
            namespace_non_whitespace: false,
            source_input: None,
            lookup: None,
            identities: BTreeSet::new(),
            dependencies: BTreeSet::new(),
            namespaces: BTreeSet::new(),
            failure: None,
        }
    }
    pub(crate) fn advance(
        &mut self,
        before: &dyn Fn(ValidationStep<'_>) -> Result<(), String>,
    ) -> Result<bool, String> {
        if let Some(error) = &self.failure {
            return Err(error.clone());
        }
        let result = self.advance_inner(before);
        if let Err(error) = &result {
            self.failure = Some(error.clone());
        }
        result
    }
    fn source(&self) -> Result<&'bundle DefinitionSource, String> {
        self.bundle
            .sources
            .get(self.source_index)
            .ok_or_else(|| "Missing bundle source selection".into())
    }
    fn namespace(&self) -> Result<&'bundle NamespaceBinding, String> {
        self.bundle
            .namespaces
            .get(self.namespace_index)
            .ok_or_else(|| "Missing bundle namespace selection".into())
    }
    fn advance_inner(
        &mut self,
        before: &dyn Fn(ValidationStep<'_>) -> Result<(), String>,
    ) -> Result<bool, String> {
        match self.phase {
            Phase::Version => {
                if self.bundle.version != BUNDLE_VERSION {
                    return Err(format!(
                        "Unsupported definition bundle version {}",
                        self.bundle.version
                    ));
                }
                self.phase = Phase::Sources;
            }
            Phase::Sources => {
                if let Some(source) = self.bundle.sources.get(self.source_index) {
                    before(ValidationStep::Source(source))?;
                    self.source_input = Some(SourceValidation::new(source));
                    self.phase = Phase::SourceInput;
                } else {
                    self.source_index = 0;
                    self.phase = Phase::Dependencies;
                }
            }
            Phase::SourceInput => {
                if self
                    .source_input
                    .as_mut()
                    .ok_or("Missing bundle source validation")?
                    .advance()?
                {
                    self.source_input = None;
                    self.phase = Phase::Identity;
                }
            }
            Phase::Identity => {
                let source = self.source()?;
                before(ValidationStep::Identity(&source.id))?;
                if !self.identities.insert(&source.id) {
                    return Err(format!("Duplicate source identity: {}", source.id.uri));
                }
                self.source_index += 1;
                self.phase = Phase::Sources;
            }
            Phase::Dependencies => {
                if let Some(source) = self.bundle.sources.get(self.source_index) {
                    if let Some(dependency) = source.dependencies.get(self.dependency_index) {
                        before(ValidationStep::Dependency(dependency))?;
                        if !self.identities.contains(dependency) {
                            return Err(format!(
                                "Missing exact source dependency {}@{} required by {}",
                                dependency.uri, dependency.sha256, source.id.uri,
                            ));
                        }
                        if !self.dependencies.insert(dependency) {
                            return Err(format!("Duplicate dependency in {}", source.id.uri));
                        }
                        self.dependency_index += 1;
                    } else if self.dependencies.pop_first().is_none() {
                        self.source_index += 1;
                        self.dependency_index = 0;
                    }
                } else {
                    self.phase = Phase::Namespaces;
                }
            }
            Phase::Namespaces => {
                if let Some(binding) = self.bundle.namespaces.get(self.namespace_index) {
                    before(ValidationStep::Namespace(binding))?;
                    self.namespace_offset = 0;
                    self.namespace_non_whitespace = false;
                    self.phase = Phase::NamespaceText;
                } else {
                    self.phase = Phase::Release;
                }
            }
            Phase::NamespaceText => {
                let binding = self.namespace()?;
                self.namespace_offset = validate_identity_chunk(
                    &binding.namespace,
                    self.namespace_offset,
                    &mut self.namespace_non_whitespace,
                )
                .map_err(|()| {
                    "Namespace identity must be nonempty and contain no controls".to_owned()
                })?;
                if self.namespace_offset == binding.namespace.len() {
                    self.phase = Phase::NamespaceIdentity;
                }
            }
            Phase::NamespaceIdentity => {
                let binding = self.namespace()?;
                if !self.namespaces.insert(&binding.namespace) {
                    return Err(format!(
                        "Ambiguous namespace binding: {}",
                        binding.namespace
                    ));
                }
                self.lookup = Some(SourceLookup::new(self.bundle, &binding.source));
                self.phase = Phase::NamespaceLookup;
            }
            Phase::NamespaceLookup => {
                if let Some(source) = self
                    .lookup
                    .as_mut()
                    .ok_or("Missing namespace source lookup")?
                    .advance(before)?
                {
                    let binding = self.namespace()?;
                    if !matches!(source.kind, SourceKind::Ecore | SourceKind::Rcore) {
                        return Err(format!(
                            "Namespace {} must bind an Ecore source",
                            binding.namespace
                        ));
                    }
                    self.lookup = None;
                    self.namespace_index += 1;
                    self.phase = Phase::Namespaces;
                }
            }
            Phase::Release => {
                // Successful release is retained too. Early-drop/error cleanup
                // and string comparison within one index operation remain separate.
                if self.identities.pop_first().is_some()
                    || self.dependencies.pop_first().is_some()
                    || self.namespaces.pop_first().is_some()
                {
                    return Ok(false);
                }
                self.phase = Phase::Complete;
            }
            Phase::Complete => return Ok(true),
        }
        Ok(matches!(self.phase, Phase::Complete))
    }
}

#[cfg(test)]
#[path = "bundle_validation_tests.rs"]
mod tests;
