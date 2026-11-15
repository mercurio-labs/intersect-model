//! Facts a store needs to accept a structurally valid model in one pass of
//! its own representation. Acceptance is an optimization of the staged
//! structural audit, never a second validator: a store may only answer true
//! when that audit would pass, and answers false for anything it does not
//! recognize, after which the audit runs unchanged and owns every diagnostic.

/// How one effective feature of a class is checked by the staged audit.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FeatureRuleKind {
    /// Builtin string attribute: any present string value is valid.
    Text,
    /// Stored reference without an opposite.
    Reference { containment: bool },
    /// Derived non-containment feature: it must hold no stored value.
    Derived,
}

/// Multiplicity and uniqueness of one effective feature.
#[derive(Clone, Copy, Debug)]
pub struct FeatureRule<'a> {
    pub name: &'a str,
    pub lower: usize,
    pub upper: i32,
    pub unique: bool,
    pub kind: FeatureRuleKind,
}

/// Schema and resource facts of one audit, prepared by its canonical stages.
pub trait StructureFacts {
    /// Every effective feature of a concrete class whose type identity
    /// matches, or None when the class needs the staged audit.
    fn class(&self, class: &str, identity: &str) -> Option<Vec<FeatureRule<'_>>>;
    /// Whether an object of class `target` conforms to reference `feature`
    /// of `class`. False also when conformance cannot be established.
    fn conforms(&self, class: &str, feature: &str, target: &str) -> bool;
    /// Whether the resource is registered and loaded.
    fn resource_loaded(&self, uri: &str) -> bool;
    /// Visit every declared resource root as (identity, resource URI) until
    /// the visitor returns false; returns whether every visit returned true.
    fn roots(&self, visit: &mut dyn FnMut(&str, &str) -> bool) -> bool;
}
