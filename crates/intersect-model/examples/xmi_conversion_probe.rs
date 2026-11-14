//! Native/WASM probe over the exact same checked lexical-provider XMI controls.
#[path = "../tests/xmi_conversions.rs"]
mod controls;
#[unsafe(no_mangle)]
pub extern "C" fn verify_xmi_conversion_contexts() -> i32 {
    controls::source_bound_rgb_single_collection_default_null_and_builtin_enum_round_trip();
    controls::missing_wrong_version_stale_source_and_untrusted_payloads_reject();
    controls::malformed_lexical_nonserializable_and_namespace_reject_without_publication();
    controls::selected_resource_persistence_and_retained_reference_context_share_providers();
    controls::lossy_provider_format_is_rejected_by_shared_registry_round_trip_check();
    controls::ordinary_context_free_codec_bytes_remain_equal_with_explicit_empty_adapters();
    controls::codecs_without_provider_adoption_do_not_silently_discard_context();
    7
}
fn main() {
    assert_eq!(verify_xmi_conversion_contexts(), 7);
}
