#[path = "../../../fixtures/semantic-read-policy/controls.rs"]
mod controls;

#[test]
fn semantic_values_and_shared_queries() -> Result<(), String> {
    controls::delegated_value_length_projection_navigation_and_count_share_policy()
}
#[test]
fn unsupported_preflight() -> Result<(), String> {
    controls::unsupported_fields_dependencies_and_complete_projections_reject_before_store()
}
#[test]
fn subtype_anchor_policy() -> Result<(), String> {
    controls::unsafe_equality_anchor_is_disabled_for_computed_subtype_and_unsupported_fails_early()
}
#[test]
fn source_receiver_budget_recovery() -> Result<(), String> {
    controls::source_actual_identity_budget_failure_and_recovery_keep_raw_values()
}
#[test]
fn independent_index_policies() -> Result<(), String> {
    controls::index_on_and_off_preserve_whole_semantic_results_and_callback_work()
}

#[test]
fn charged_identity_zero_exact_one_short_and_recovery() -> Result<(), String> {
    controls::zero_budget_precedes_callbacks_and_exact_one_short_work_recovers()
}

#[path = "semantic_read_policy/nominal.rs"]
mod nominal;
