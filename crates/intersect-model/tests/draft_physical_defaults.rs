#[path = "../../../fixtures/draft-physical-defaults/controls.rs"]
mod controls;

#[test]
fn source_dispositions_actual_revision_budget_rollback_recovery() -> Result<(), String> {
    let report = controls::verify()?;
    println!("{}", report);
    Ok(())
}
