//! Consolidated qualification suite.
#![allow(clippy::duplicate_mod)]

mod stores {
    #[allow(unused_imports)]
    pub use intersect_model_test_support::stores::*;
    #[test]
    fn frozen_store_starts_empty_with_the_requested_schema() -> Result<(), String> {
        intersect_model_test_support::stores::verify_frozen_store_starts_empty_with_the_requested_schema()
    }
}

#[path = "core/assignment_state.rs"]
pub mod core_assignment_state;

#[path = "core/checked_schema_preparation.rs"]
pub mod core_checked_schema_preparation;

#[path = "core/class_projection.rs"]
pub mod core_class_projection;

#[path = "core/collection_edits.rs"]
pub mod core_collection_edits;

#[path = "core/compact_storage.rs"]
pub mod core_compact_storage;

#[path = "core/deletion.rs"]
pub mod core_deletion;

#[path = "core/dynamic.rs"]
pub mod core_dynamic;

#[path = "core/generic_feature_matrix.rs"]
pub mod core_generic_feature_matrix;

#[path = "core/generic_identity.rs"]
pub mod core_generic_identity;

#[path = "core/generic_materialization.rs"]
pub mod core_generic_materialization;

#[path = "core/incremental_validation.rs"]
pub mod core_incremental_validation;

#[path = "core/index_preparation.rs"]
pub mod core_index_preparation;

#[path = "core/ownership_index.rs"]
pub mod core_ownership_index;

#[path = "core/query_exact_integers.rs"]
pub mod core_query_exact_integers;

#[path = "core/query_projection.rs"]
pub mod core_query_projection;

#[path = "core/raw_runtime.rs"]
pub mod core_raw_runtime;

#[path = "core/raw_top_boundaries.rs"]
pub mod core_raw_top_boundaries;

#[path = "core/raw_unbounded.rs"]
pub mod core_raw_unbounded;

#[path = "core/reparent.rs"]
pub mod core_reparent;

#[path = "core/required_containment.rs"]
pub mod core_required_containment;

#[path = "core/resource_schema.rs"]
pub mod core_resource_schema;

#[path = "core/runtime_feature_domains.rs"]
pub mod core_runtime_feature_domains;

#[path = "core/runtime_lower_bounds.rs"]
pub mod core_runtime_lower_bounds;

#[path = "core/runtime_operations.rs"]
pub mod core_runtime_operations;

#[path = "core/scalar_values.rs"]
pub mod core_scalar_values;

#[path = "core/shared_array_inputs.rs"]
pub mod core_shared_array_inputs;

#[path = "core/shared_array_view_inputs.rs"]
pub mod core_shared_array_view_inputs;

#[path = "core/shared_domain_source_inputs.rs"]
pub mod core_shared_domain_source_inputs;

#[path = "core/shared_schema_inputs.rs"]
pub mod core_shared_schema_inputs;

#[path = "core/shared_source_array_inputs.rs"]
pub mod core_shared_source_array_inputs;

#[path = "core/stored_projection.rs"]
pub mod core_stored_projection;

#[path = "core/structural_attribute_preparation.rs"]
pub mod core_structural_attribute_preparation;

#[path = "core/structural_cache_storage.rs"]
pub mod core_structural_cache_storage;

#[path = "core/unsettable.rs"]
pub mod core_unsettable;
