//! Model-only consumer: no Xtext compiler or language dependency required.
#[path = "../../../fixtures/rcore-model-rule-service/controls.rs"]
mod controls;
use intersect_model::{model_rules, storage::ModelView};

#[test]
fn prepared_rules_keep_literal_values_duplicate_diagnostics_subtypes_and_budget_order() {
    let (package, model) = controls::fixture();
    let before = serde_json::to_value(&model).unwrap();
    let view = ModelView::new(&model).unwrap();
    let (mut values, mut diagnostics) = controls::seed();
    let provider = controls::Recording::new(&package, false);
    let outcome =
        model_rules::apply_with_provider(&package, &view, &mut values, &mut diagnostics, &provider);
    assert_eq!(
        controls::snapshot(outcome, &values, &diagnostics, &provider),
        controls::expected(false)
    );
    assert_eq!(serde_json::to_value(&model).unwrap(), before);
}

#[test]
fn provider_failure_keeps_original_partial_prefix_then_same_model_recovers() {
    let (package, model) = controls::fixture();
    let before = serde_json::to_value(&model).unwrap();
    let view = ModelView::new(&model).unwrap();
    let (mut values, mut diagnostics) = controls::seed();
    let rejected = controls::Recording::new(&package, true);
    let outcome =
        model_rules::apply_with_provider(&package, &view, &mut values, &mut diagnostics, &rejected);
    assert_eq!(
        controls::snapshot(outcome, &values, &diagnostics, &rejected),
        controls::expected(true)
    );
    let recovered = controls::Recording::new(&package, false);
    let outcome = model_rules::apply_with_provider(
        &package,
        &view,
        &mut values,
        &mut diagnostics,
        &recovered,
    );
    assert_eq!(
        controls::snapshot(outcome, &values, &diagnostics, &recovered),
        controls::expected(false)
    );
    assert_eq!(serde_json::to_value(&model).unwrap(), before);
}
