#[path = "../tests/support/checked_normalization_consumers.rs"]
mod controls;
fn main() -> Result<(), String> {
    assert_eq!(controls::verify()?, 3);
    println!("Three public checked normalization consumer groups passed; full S09 remains open.");
    Ok(())
}
#[unsafe(no_mangle)]
pub extern "C" fn verify_checked_normalization_consumers() -> u32 {
    controls::verify().unwrap_or(0)
}
