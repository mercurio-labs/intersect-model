#[path = "support/structural_substitution_consumers.rs"]
mod controls;
#[test]
fn public_structural_substitution_consumers() -> Result<(), String> {
    assert_eq!(controls::verify()?, 3);
    Ok(())
}
