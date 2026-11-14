#[path = "../../../fixtures/draft-null-defaults/controls.rs"]
mod controls;
#[test]
fn optional_null_same_factory_binding_codec_budget_rollback_recovery() -> Result<(), String> {
    println!("{}", controls::verify()?);
    Ok(())
}
