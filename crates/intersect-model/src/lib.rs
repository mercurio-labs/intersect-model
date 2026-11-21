//! Shared model APIs and handwritten Rcore execution, independent of Xtext.
//! Internal structural and definition layers retain their dependency direction.
//!
//! # Application APIs
//! Start with [`dynamic::DynamicModel`], [`definitions::dynamic::SchemaBuilder`],
//! or [`compile_rcore`] and [`RcorePackage`]. [`api`] provides reflective editing;
//! [`resource_set`] and [`interchange`] provide resource and persistence services.
//!
//! # Prepared dependency components
//! With the `components` feature, `components::ComponentImage` owns an admitted,
//! immutable dependency. `components::MountedModel` combines it with writable
//! project data under one compatible schema. Read references through the mounted
//! model and use checked edits for project data. Hosts acquire dependency
//! artifacts, and language packages own text parsing and linking.
//!
//! # Advanced extension contracts
//! [`extensions`] describes provider, storage and execution integration. These
//! contracts require explicit source identity, validation and work accounting;
//! public transport data is not a checked executable model.
#[cfg(feature = "components")]
pub use intersect_model::core::components;
pub use intersect_model::core::{DuplicatePackage, PackageRegistry};
pub use intersect_model::core::{
    api, dynamic, frozen, interchange, model, package_load, project_usages, resource_set,
    resources, storage, transaction,
};
extern crate self as intersect_model;
#[cfg(feature = "codegen")]
pub mod codegen;
pub mod core;
pub mod definitions;
pub mod extensions;
mod logical_size;
#[cfg(all(feature = "sqlite", not(target_arch = "wasm32")))]
pub mod sqlite;
#[cfg(feature = "xmi")]
pub mod xmi;
pub use intersect_model::definitions::ecore_migration as migration;

/// Installed model package version, independent of language package releases.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Deterministic identity of this crate's compiler: a SHA-256 (64 lowercase hex
/// digits) over its version, enabled features, the workspace `Cargo.toml`, its
/// manifest, build script, every `src/` file, every file it embeds or mounts as
/// a module from outside `src/`, and the workspace `Cargo.lock`, computed by
/// the host-only intersect-build-support package. Equal identities mean the
/// same compiler inputs, so saved compiler output keyed by it may be reused; it
/// is a cache key for the host's own output, not a signature.
pub const SOURCE_IDENTITY: &str = env!("INTERSECT_MODEL_SOURCE_IDENTITY");

pub mod class_conversion;
pub mod collection_conversion;
pub mod conversion;
pub mod diagnostic_chain_conversion;
pub mod enumerator_conversion;
pub mod feature_map_conversion;
pub mod frozen_scopes;
pub mod initialization;
pub mod invocation_error_conversion;
pub mod model_reference;
pub mod model_rules;
pub mod navigation;
pub mod python;
pub mod query;
pub mod rcore_behavior_execution;
pub mod rcore_behavior_functions;
pub mod rcore_behavior_scalar;
// Fast execution is an internal optimization behind the checked scope runtime.
mod rcore_fast;
#[cfg(test)]
mod rcore_getter_tests;
pub mod rcore_getters;
pub mod rcore_operations;
pub mod resource_conversion;
pub mod scope_runtime;
pub mod semantic_bindings;
mod sequence_conversion;
pub mod setting;
pub mod source_classified_reads;
mod source_getters;
pub mod source_model_functions;
pub mod source_model_reads;
mod source_read_callbacks;
pub mod source_scalar_functions;
pub mod source_scalar_reads;
pub mod tree_iterator_conversion;

pub mod rcore;
pub use rcore::{
    RcoreError, RcoreModel, RcorePackage, compile_rcore, compile_rcore_at, compile_rcore_bundle,
    compile_rcore_bundle_selected, compile_rcore_bundle_selected_with_initializers,
    compile_rcore_bundle_with_initializers,
};
pub mod semantic_reads;
