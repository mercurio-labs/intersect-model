#[path = "../shared/rcore_enum_queries.rs"]
mod controls;
fn main() -> Result<(), String> {
    let groups = controls::verify()?;
    println!("{groups} enum import/binding/typing consumer groups passed");
    Ok(())
}
#[cfg_attr(not(test), unsafe(no_mangle))]
pub extern "C" fn verify_rcore_enum_queries() -> u32 {
    controls::verify().map_or(0, |groups| groups as u32)
}
#[cfg(test)]
#[test]
fn enum_source_identities_aliases_binding_typing_and_rejections_are_preserved() {
    assert_eq!(controls::verify().unwrap(), 3);
}
