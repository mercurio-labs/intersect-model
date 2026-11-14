//! Direct Rcore selects native constructors over the existing shared service.
#[path = "../../../fixtures/rcore-model-rule-service/provider_initializers.rs"]
mod controls;

#[test]
fn explicit_selection_preserves_disabled_routes() -> Result<(), String> {
    controls::explicit_selection_preserves_disabled_routes()
}

#[test]
fn live_relationship_plan_defaults_and_checked_edits() -> Result<(), String> {
    controls::live_relationship_plan_defaults_and_checked_edits()
}

#[test]
fn invalid_plans_and_authored_values_roll_back_whole_graph() -> Result<(), String> {
    controls::invalid_plans_and_authored_values_roll_back_whole_graph()
}

#[test]
fn exact_short_zero_and_preconsumed_use_original_ledger() -> Result<(), String> {
    controls::exact_short_zero_and_preconsumed_use_original_ledger()
}

#[test]
fn reload_requires_explicit_selection_and_revalidates_sources() -> Result<(), String> {
    controls::reload_requires_explicit_selection_and_revalidates_sources()
}

#[test]
fn exact_runtime_type_and_foreign_contract_boundaries() -> Result<(), String> {
    controls::exact_runtime_type_and_foreign_contract_boundaries()
}
