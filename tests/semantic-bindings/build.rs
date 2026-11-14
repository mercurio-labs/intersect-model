//! Build host; generated adapters are compiled on both native and WASM targets.
use intersect_model_test_support::checked_contract;
use intersect_model_test_support::contract;
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let (_, contracts, _) = contract::contracts()?;
    let output = std::path::PathBuf::from(std::env::var("OUT_DIR")?);
    for (index, name) in ["twice", "doubled"].into_iter().enumerate() {
        let generated = intersect_model::codegen::semantic_bindings::generate(
            &contracts[index],
            &format!("crate::native::{name}"),
            &format!("register_{name}"),
            "bindings-example-rust-v1",
        )?;
        std::fs::write(output.join(format!("{name}.rs")), generated)?;
    }
    let (_, checked, _) = checked_contract::contracts()?;
    use intersect_model::codegen::semantic_bindings::{Export, generate_package};
    let exports = checked
        .iter()
        .zip(["twice", "doubled"])
        .map(|(contract, name)| Export {
            contract,
            rust_path: if name == "twice" {
                "crate::native::twice"
            } else {
                "crate::native::doubled"
            },
            registration: if name == "twice" {
                "register_checked_twice"
            } else {
                "register_checked_doubled"
            },
            implementation_revision: "checked-native-v1",
        })
        .collect::<Vec<_>>();
    std::fs::write(
        output.join("checked.rs"),
        generate_package(&exports, "register_checked_package")?,
    )?;
    let (_, typed, _) = contract::compile(
        include_str!("../../fixtures/rcore/bindings/model.rcore"),
        include_str!("../../fixtures/rcore/bindings/typed-behavior.rcore"),
    )?;
    for (contract, name) in typed.iter().zip(["numbers", "optional"]) {
        let generated = intersect_model::codegen::semantic_bindings::generate(
            contract,
            &format!("crate::typed_native::{name}"),
            &format!("register_{name}"),
            "typed-v1",
        )?;
        std::fs::write(output.join(format!("{name}.rs")), generated)?;
    }
    println!("cargo:rerun-if-changed=../../fixtures/rcore/bindings");
    Ok(())
}
