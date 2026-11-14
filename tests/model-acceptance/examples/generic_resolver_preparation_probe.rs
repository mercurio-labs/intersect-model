#[path = "../shared/generic_resolver_preparation.rs"]
mod controls;
fn main() -> Result<(), String> {
    controls::verify()?;
    println!("Five retained resolver/input/member groups passed; full compiler/S09 remains open.");
    Ok(())
}
#[cfg_attr(not(test), unsafe(no_mangle))]
pub extern "C" fn verify_generic_resolver_preparation() -> u32 {
    controls::verify().unwrap_or(0)
}
#[cfg(test)]
#[test]
#[cfg_attr(not(feature = "milestone-tests"), ignore = "milestone")]
fn complete_retained_resolver_input_controls() -> Result<(), String> {
    controls::verify()?;
    Ok(())
}
