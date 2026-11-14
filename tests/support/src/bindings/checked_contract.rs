//! Explicit host declaration of source-authored contract predicates.
use intersect_model::definitions::{
    bundle::DefinitionBundle,
    generic_types::RuntimeType,
    semantic_bindings::{Contract, compile_sources},
};
pub fn contracts() -> Result<(DefinitionBundle, Vec<Contract>, RuntimeType), String> {
    use intersect_model::definitions::rcore_behavior::ir_v2::EntrySelection;
    let sources = compile_sources(
        include_str!("model.rcore"),
        include_str!("checked-behavior.rcore"),
    )?;
    let select = |index: usize| EntrySelection {
        symbol: sources.contracts[index].descriptor().symbol.clone(),
        types: vec![],
        arguments: vec![],
    };
    let entry = &sources.contracts[0].descriptor().symbol.source;
    let twice = sources.contracts[0].clone().with_conditions(
        &sources.bundle,
        entry,
        &[select(1)],
        &[select(2)],
    )?;
    let doubled =
        sources.contracts[5]
            .clone()
            .with_conditions(&sources.bundle, entry, &[], &[select(6)])?;
    let receiver = sources
        .model_classes
        .get("Meter")
        .ok_or("Missing checked Meter")?
        .clone();
    Ok((sources.bundle, vec![twice, doubled], receiver))
}
