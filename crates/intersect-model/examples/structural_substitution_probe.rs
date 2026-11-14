#[path = "../tests/support/structural_substitution_consumers.rs"]
mod controls;
fn main() -> Result<(), String> {
    assert_eq!(controls::verify()?, 3);
    println!("Three public structural substitution consumer groups passed; full S09 remains open.");
    Ok(())
}
#[unsafe(no_mangle)]
pub extern "C" fn verify_structural_substitution_consumers() -> u32 {
    controls::verify().unwrap_or(0)
}
