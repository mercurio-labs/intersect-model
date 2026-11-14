//! Advanced host and language integration contracts.
//!
//! Ordinary applications should start with [`crate::dynamic`] or
//! [`crate::RcorePackage`]. This module owns the shared accounting constant and
//! callback contract; the implementation modules are private.
//!
//! Existing specialized contracts retain their owning paths:
//!
//! | Contract | Owner | Host obligation |
//! | --- | --- | --- |
//! | Storage and borrowed views | [`crate::storage`], [`crate::frozen`] | Preserve identities, revisions and checked mutation semantics |
//! | Source preparation and declarations | [`crate::definitions`] | Validate retained sources; deserialization is not admission |
//! | Conversions and initialization | [`crate::conversion`], [`crate::initialization`] | Bind providers to exact declarations and budgets |
//! | Semantic providers | [`crate::semantic_bindings`], [`crate::scope_runtime`] | Use the caller's accounting and failure/rollback boundaries |
//! | Source functions and reads | [`crate::source_model_functions`], [`crate::semantic_reads`] | Retain source authority and borrowed-reader lifetimes |
//!
//! These are provisional extension APIs, not additional application facades.
//! The root [`crate::logical_size!`] macro remains available to generated code
//! and downstream runtimes because exported Rust macros have a crate-root path.

pub use crate::logical_size::LOGICAL_WORD;
pub use crate::source_read_callbacks::SourceReadCallbacks;
