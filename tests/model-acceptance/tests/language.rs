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

#[path = "language/applied_conversion.rs"]
pub mod language_applied_conversion;

#[path = "language/collection_conversion.rs"]
pub mod language_collection_conversion;

#[path = "language/collection_dispatch_budget.rs"]
pub mod language_collection_dispatch_budget;

#[path = "language/diagnostic_chain_conversion.rs"]
pub mod language_diagnostic_chain_conversion;

#[path = "language/feature_map_conversion.rs"]
pub mod language_feature_map_conversion;

#[path = "language/incoming_cursor.rs"]
pub mod language_incoming_cursor;

#[path = "language/invocation_error_conversion.rs"]
pub mod language_invocation_error_conversion;

#[path = "language/model_reference.rs"]
pub mod language_model_reference;

#[path = "language/model_reference_storage.rs"]
pub mod language_model_reference_storage;

#[path = "language/rcore_behavior_collection_algebra.rs"]
pub mod language_rcore_behavior_collection_algebra;

#[path = "language/rcore_behavior_collection_callbacks.rs"]
pub mod language_rcore_behavior_collection_callbacks;

#[path = "language/rcore_behavior_collection_grouping.rs"]
pub mod language_rcore_behavior_collection_grouping;

#[path = "language/rcore_behavior_collection_key_sorting.rs"]
pub mod language_rcore_behavior_collection_key_sorting;

#[path = "language/rcore_behavior_collection_measures.rs"]
pub mod language_rcore_behavior_collection_measures;

#[path = "language/rcore_behavior_collection_membership.rs"]
pub mod language_rcore_behavior_collection_membership;

#[path = "language/rcore_behavior_collection_selection.rs"]
pub mod language_rcore_behavior_collection_selection;

#[path = "language/rcore_behavior_collection_sorting.rs"]
pub mod language_rcore_behavior_collection_sorting;

#[path = "language/rcore_behavior_collection_values.rs"]
pub mod language_rcore_behavior_collection_values;

#[path = "language/rcore_behavior_comprehensions.rs"]
pub mod language_rcore_behavior_comprehensions;

#[path = "language/rcore_behavior_function_adversarial.rs"]
pub mod language_rcore_behavior_function_adversarial;

#[path = "language/rcore_behavior_function_boundaries.rs"]
pub mod language_rcore_behavior_function_boundaries;

#[path = "language/rcore_behavior_function_calls.rs"]
pub mod language_rcore_behavior_function_calls;

#[path = "language/rcore_behavior_function_domains.rs"]
pub mod language_rcore_behavior_function_domains;

#[path = "language/rcore_behavior_function_inputs.rs"]
pub mod language_rcore_behavior_function_inputs;

#[path = "language/rcore_behavior_function_overloads.rs"]
pub mod language_rcore_behavior_function_overloads;

#[path = "language/rcore_behavior_function_provenance.rs"]
pub mod language_rcore_behavior_function_provenance;

#[path = "language/rcore_behavior_functions.rs"]
pub mod language_rcore_behavior_functions;

#[path = "language/rcore_behavior_generic_handoff.rs"]
pub mod language_rcore_behavior_generic_handoff;

#[path = "language/rcore_behavior_inferred_lambdas.rs"]
pub mod language_rcore_behavior_inferred_lambdas;

#[path = "language/rcore_behavior_navigation.rs"]
pub mod language_rcore_behavior_navigation;

#[path = "language/rcore_behavior_provider_metadata.rs"]
pub mod language_rcore_behavior_provider_metadata;

#[path = "language/rcore_behavior_provider_navigation.rs"]
pub mod language_rcore_behavior_provider_navigation;

#[path = "language/rcore_behavior_provider_traversal.rs"]
pub mod language_rcore_behavior_provider_traversal;

#[path = "language/rcore_behavior_scalar.rs"]
pub mod language_rcore_behavior_scalar;

#[path = "language/rcore_behavior_scalar_compatibility.rs"]
pub mod language_rcore_behavior_scalar_compatibility;

#[path = "language/rcore_behavior_scalar_enums.rs"]
pub mod language_rcore_behavior_scalar_enums;

#[path = "language/rcore_behavior_structural_models.rs"]
pub mod language_rcore_behavior_structural_models;

#[path = "language/rcore_behavior_symbolic_alias_inference.rs"]
pub mod language_rcore_behavior_symbolic_alias_inference;

#[path = "language/rcore_behavior_symbolic_datatypes.rs"]
pub mod language_rcore_behavior_symbolic_datatypes;

#[path = "language/rcore_behavior_unresolved_domain.rs"]
pub mod language_rcore_behavior_unresolved_domain;

#[path = "language/rcore_observation.rs"]
pub mod language_rcore_observation;

#[path = "language/rcore_operations.rs"]
pub mod language_rcore_operations;

#[path = "language/reference_context_depth.rs"]
pub mod language_reference_context_depth;

#[path = "language/reference_policy_preparation.rs"]
pub mod language_reference_policy_preparation;

#[path = "language/resource_conversion.rs"]
pub mod language_resource_conversion;

#[path = "language/retained_reference_compilation.rs"]
pub mod language_retained_reference_compilation;

#[path = "language/runtime_lower_attributes.rs"]
pub mod language_runtime_lower_attributes;

#[path = "language/tree_iterator_conversion.rs"]
pub mod language_tree_iterator_conversion;
