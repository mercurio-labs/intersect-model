#[path = "support/legacy_agreement_host_consumers.rs"]
mod controls;
#[test]
fn public_legacy_agreement_host_consumers() -> Result<(), String> {
    assert_eq!(controls::verify()?, 5);
    Ok(())
}
