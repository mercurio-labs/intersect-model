//! Run the same handwritten implementation through a generated native adapter.
use intersect_model::{
    rcore_behavior_functions::{ExecutionContext, ExecutionLimits},
    semantic_bindings::{Budget, NativeValue, Registry},
};
use intersect_semantic_bindings_tests::{contract, generated};
fn main() -> Result<(), Box<dyn std::error::Error>> {
    intersect_semantic_bindings_tests::qualify()?;
    let (bundle, contracts, _) = contract::contracts()?;
    let contract = contracts.first().ok_or("Missing example contract")?;
    let mut registry = Registry::default();
    generated::register_twice(&mut registry, contract)?;
    let argument = 21_i64.into_wire(contract.descriptor().parameters[0])?;
    let result = registry.pin(contract)?.invoke(
        &[argument],
        &ExecutionContext {
            bundle: &bundle,
            conversions: None,
            references: &[],
        },
        &ExecutionLimits::default(),
        Budget::new(100_000),
    )?;
    println!("{}", serde_json::to_string_pretty(&result)?);
    Ok(())
}
