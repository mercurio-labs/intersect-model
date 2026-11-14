//! Frozen metamodel, mutable arena storage and validated immutable models.
//!
//! Phases are types: `Metamodel::freeze` validates static schema facts once;
//! `ModelBuilder` appends objects with O(1) shape checks; `link` resolves all
//! proxies through a pluggable [`Resolver`] and completes opposites; and
//! `LinkedModel::validate` runs one O(N + E) structural pass. The resulting
//! [`Model`] is immutable and its reads perform no checks.
//!
//! Supported profile: classes, inheritance, stored attributes (validated by the
//! canonical schema datatype validators), references, containment, opposites,
//! multiplicities and uniqueness. [`ArenaStore`] implements mutable storage and
//! nested transactions over the builder; it can also start from a validated model.
//! The validated [`Model`] itself remains immutable. Default, implicit-object and
//! validation providers are supplied explicitly through the builder interfaces.
//! Import and execution paths refuse state outside their supported contracts.
#[path = "frozen/builder.rs"]
mod builder;
#[path = "frozen/intern.rs"]
mod intern;
#[path = "frozen/interop.rs"]
mod interop;
#[path = "frozen/metamodel.rs"]
mod metamodel;
#[path = "frozen/model.rs"]
mod model;
#[path = "frozen/snapshot.rs"]
mod snapshot;
#[path = "frozen/store.rs"]
mod store;
#[path = "frozen/validate.rs"]
mod validate;

pub use builder::{
    ById, ByName, DefaultItem, Defaults, INTERNAL_VALIDATION, ImplicitObject, ImplicitValue,
    Implicits, Issue, LinkError, LinkValue, LinkView, LinkedModel, ModelBuilder, ObjectIssue,
    Resolver, Severity, Validators,
};
pub use intern::Sym;
pub use interop::ImportError;
pub use metamodel::{ClassId, ClassInfo, FeatureId, FeatureInfo, FeatureKind, Metamodel, Target};
pub use model::{Inverse, Items, Model, Obj, ObjId, ValueRef};
pub use snapshot::{
    SNAPSHOT_FORMAT, SNAPSHOT_HEADER_BYTES, SNAPSHOT_MAGIC, SnapshotError, SnapshotHeader,
    SnapshotTrust, snapshot_header,
};
pub use store::FrozenStore;
/// Preferred name for the mutable arena-backed store.
///
/// This is the same type as [`FrozenStore`], retained for existing consumers.
/// It implements [`crate::storage::ModelRead`] and [`crate::storage::ModelStore`];
/// its nested transactions journal edits and restore stable object identities on
/// rollback. Converting into a builder allows linking and validation to produce
/// an immutable [`Model`]. An arena store is not itself a validated snapshot.
pub use store::FrozenStore as ArenaStore;
pub(super) use store::{FrozenAtom, FrozenItems, FrozenObject, FrozenValue};

/// Refusal of one builder or freeze operation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FrozenError {
    Schema(String),
    Capacity(&'static str),
    UnknownClass,
    UnknownFeature,
    UnknownObject,
    Abstract(String),
    Identity(String),
    TypeIdentity(String),
    NotAFeature { class: String, feature: String },
    Kind(String),
    Derived(String),
    Ambiguous(String),
    Revision,
    Unsupported(&'static str),
    Scope(String),
}
impl std::fmt::Display for FrozenError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Schema(e) => write!(f, "schema: {e}"),
            Self::Capacity(what) => write!(f, "capacity exceeded: {what}"),
            Self::UnknownClass => f.write_str("unknown class"),
            Self::UnknownFeature => f.write_str("unknown feature"),
            Self::UnknownObject => f.write_str("unknown object"),
            Self::Abstract(c) => write!(f, "cannot instantiate abstract class {c}"),
            Self::Identity(id) => write!(f, "identity must be unique and nonempty: '{id}'"),
            Self::TypeIdentity(id) => write!(f, "type identity of {id} does not match schema"),
            Self::NotAFeature { class, feature } => {
                write!(f, "{feature} is not a stored feature of {class}")
            }
            Self::Kind(feature) => write!(f, "wrong value kind for {feature}"),
            Self::Derived(feature) => write!(f, "derived values cannot be stored: {feature}"),
            Self::Ambiguous(name) => write!(f, "ambiguous reference {name}"),
            Self::Revision => f.write_str("model belongs to a different schema revision"),
            Self::Unsupported(what) => write!(f, "unsupported in frozen engine: {what}"),
            Self::Scope(e) => write!(f, "scope: {e}"),
        }
    }
}
impl std::error::Error for FrozenError {}

/// One structural contract violation found by linking or validation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Violation {
    UnknownClass(String),
    Abstract {
        object: String,
        class: String,
    },
    Multiplicity {
        object: String,
        feature: String,
        count: usize,
        lower: u32,
        upper: Option<u32>,
    },
    Kind {
        object: String,
        feature: String,
    },
    Attribute {
        object: String,
        feature: String,
        error: String,
    },
    TargetType {
        object: String,
        feature: String,
        target: String,
    },
    Duplicate {
        object: String,
        feature: String,
    },
    Unresolved {
        object: String,
        feature: String,
        spelling: String,
    },
    ReferenceCycle {
        object: String,
        feature: String,
        spelling: String,
    },
    MultipleContainers(String),
    ContainmentCycle(String),
    Opposite {
        object: String,
        feature: String,
    },
    Resolver(String),
}
impl std::fmt::Display for Violation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnknownClass(o) => write!(f, "{o}: unknown class"),
            Self::Abstract { object, class } => write!(f, "{object}: abstract instance of {class}"),
            Self::Multiplicity {
                object,
                feature,
                count,
                lower,
                upper,
            } => {
                let upper = upper.map_or("*".to_owned(), |u| u.to_string());
                write!(
                    f,
                    "{object}.{feature}: {count} values outside [{lower},{upper}]"
                )
            }
            Self::Kind { object, feature } => write!(f, "{object}.{feature}: wrong value kind"),
            Self::Attribute {
                object,
                feature,
                error,
            } => write!(f, "{object}.{feature}: {error}"),
            Self::TargetType {
                object,
                feature,
                target,
            } => {
                write!(f, "{object}.{feature}: target {target} does not conform")
            }
            Self::Duplicate { object, feature } => {
                write!(f, "{object}.{feature}: duplicate in unique feature")
            }
            Self::Unresolved {
                object,
                feature,
                spelling,
            } => {
                write!(f, "{object}.{feature}: unresolved reference '{spelling}'")
            }
            Self::ReferenceCycle {
                object,
                feature,
                spelling,
            } => write!(
                f,
                "{object}.{feature}: reference '{spelling}' depends on itself"
            ),
            Self::MultipleContainers(o) => write!(f, "{o}: more than one container"),
            Self::ContainmentCycle(o) => write!(f, "{o}: containment cycle"),
            Self::Opposite { object, feature } => {
                write!(f, "{object}.{feature}: missing or conflicting opposite")
            }
            Self::Resolver(e) => write!(f, "resolver: {e}"),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Diagnostics(pub Vec<Violation>);
impl std::fmt::Display for Diagnostics {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        for (i, v) in self.0.iter().enumerate() {
            if i > 0 {
                f.write_str("; ")?;
            }
            v.fmt(f)?;
        }
        Ok(())
    }
}
impl std::error::Error for Diagnostics {}
