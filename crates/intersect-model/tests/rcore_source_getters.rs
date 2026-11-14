//! The same source getter lifecycle, budget and rejection controls run in WASM.
#[path = "../../../fixtures/rcore-getter-service/direct.rs"]
mod direct;

#[test]
fn source_getters_load_read_reload_and_reject_through_the_public_api() -> Result<(), String> {
    direct::verify()
}
