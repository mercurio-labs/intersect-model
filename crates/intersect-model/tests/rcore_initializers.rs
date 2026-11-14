//! Direct constant initialization uses the shared atomic source-bound service.
#[path = "../../../fixtures/rcore-model-rule-service/initializers.rs"]
mod controls;
#[test]
fn direct_constant_initializers_are_atomic_source_bound_and_rebuilt() -> Result<(), String> {
    controls::verify()
}
