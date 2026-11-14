#[path = "support/checked_normalization_consumers.rs"]
mod controls;
#[test]
fn public_checked_normalization_consumers() -> Result<(), String> {
    assert_eq!(controls::verify()?, 3);
    Ok(())
}
