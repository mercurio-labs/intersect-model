#[path = "../shared/rcore_behavior_compiler_observations.rs"]
mod observations;
fn main() -> Result<(), String> {
    observations::verify(include_str!(
        "../../../crates/intersect-model/assets/fixtures/rcore-behavior-compiler-baseline.json"
    ))?;
    println!("51 complete pre-change module/type/image cases match; 17 verified images");
    Ok(())
}

#[unsafe(no_mangle)]
pub extern "C" fn verify_rcore_behavior_compiler_observations() -> u32 {
    u32::from(
        observations::verify(include_str!(
            "../../../crates/intersect-model/assets/fixtures/rcore-behavior-compiler-baseline.json"
        ))
        .is_ok(),
    )
}
