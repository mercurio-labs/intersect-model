//! Handwritten source-identified declaration inventory, independent of text syntax.
//! Inspection data never grants model execution, providers, or language admission.
use crate::definitions::resources::DeclarationId;
use std::collections::BTreeMap;

/// Requested structural inspection data, independent of any language profile.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum DeclarationCollection {
    #[default]
    None,
    Packages,
    Classifiers,
}

/// Exact selected packages and optionally enumerated classifiers. Empty packages
/// retain their identities. A missing classifier map means it was not requested;
/// a present empty map means that package was inspected and has no classifiers.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct DeclarationInventory {
    pub(crate) packages: BTreeMap<String, DeclarationId>,
    pub(crate) classifiers: BTreeMap<String, BTreeMap<String, DeclarationId>>,
}
impl DeclarationInventory {
    pub fn packages(&self) -> &BTreeMap<String, DeclarationId> {
        &self.packages
    }

    pub fn classifiers(&self) -> &BTreeMap<String, BTreeMap<String, DeclarationId>> {
        &self.classifiers
    }
}

/// Caller-selected inventory collection within the existing structural traversal.
/// This carries no grammar policy and cannot grant a completed compiler lease.
pub(crate) struct DeclarationSelection<'a> {
    pub(crate) inventory: &'a mut DeclarationInventory,
    pub(crate) classifiers: bool,
}
