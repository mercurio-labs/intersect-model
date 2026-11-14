//! Shared fixture compilation. Native and WASM consumers rebuild exact sources.
use intersect_model::definitions::{
    bundle::DefinitionBundle,
    generic_types::RuntimeType,
    semantic_bindings::{Contract, compile_sources},
};
pub fn contracts() -> Result<(DefinitionBundle, Vec<Contract>, RuntimeType), String> {
    compile(include_str!("model.rcore"), include_str!("behavior.rcore"))
}

pub fn compile(
    structure: &str,
    behavior: &str,
) -> Result<(DefinitionBundle, Vec<Contract>, RuntimeType), String> {
    let sources = compile_sources(structure, behavior)?;
    let receiver = sources
        .model_classes
        .get("Meter")
        .ok_or("Missing example Meter class")?
        .clone();
    Ok((sources.bundle, sources.contracts, receiver))
}
