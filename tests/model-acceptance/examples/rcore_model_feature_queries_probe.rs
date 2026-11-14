#[path = "../shared/rcore_model_feature_queries.rs"]
mod controls;
fn main() -> Result<(), String> {
    let groups = controls::verify()?;
    println!("{groups} full-schema model-feature consumer groups passed");
    Ok(())
}
#[cfg_attr(not(test), unsafe(no_mangle))]
pub extern "C" fn verify_rcore_model_feature_queries() -> u32 {
    controls::verify().map_or(0, |groups| groups as u32)
}
#[cfg(test)]
#[test]
fn complete_model_features_reference_closure_and_inherited_identities_are_preserved() {
    assert_eq!(controls::verify().unwrap(), 3);
}
