#[path = "../tests/support/legacy_agreement_host_consumers.rs"]
mod controls;
fn main() -> Result<(), String> {
    assert_eq!(controls::verify()?, 5);
    println!("Five public legacy agreement host groups passed; full S09 remains open.");
    Ok(())
}
#[unsafe(no_mangle)]
pub extern "C" fn verify_legacy_agreement_host_consumers() -> u32 {
    controls::verify().unwrap_or(0)
}
