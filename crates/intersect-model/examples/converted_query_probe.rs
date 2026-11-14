//! Handwritten native/WASM probe over the same exact nominal-query controls.
#[path = "../tests/query_conversions.rs"]
mod controls;
#[unsafe(no_mangle)]
pub extern "C" fn verify_converted_query_columns() -> i32 {
    controls::explicit_nominal_columns_share_typed_dynamic_prepared_and_collection_execution();
    controls::selected_provider_destination_and_payload_are_checked_before_results();
    controls::nominal_projection_never_invents_comparison_or_ordering_semantics();
    controls::derived_nominal_results_are_validated_against_the_selected_destination();
    4
}
fn main() {
    assert_eq!(verify_converted_query_columns(), 4);
}
