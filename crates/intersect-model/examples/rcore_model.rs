//! Handwritten Rcore-only consumer; no generated Ecore or Xtext input.
use intersect_model::{
    api::ModelEdit,
    compile_rcore_at,
    rcore::{ExecutionLimits, RcorePackage},
};
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let source = include_str!("../assets/fixtures/rcore/api/organization.rcore");
    let mut package = compile_rcore_at("memory:organization.rcore", source)?;
    package.bind_operation("Node", "reachable", "reachable")?;
    let mut model = package.new_model()?;
    for id in ["A", "B", "C"] {
        model.create(id, "Node", "memory:objects")?;
    }
    model.set("A", "links", serde_json::json!(["B"]))?;
    model.set("B", "links", serde_json::json!(["C"]))?;
    model.validate()?;
    let before = package.invoke(&model, "A", "reachable", &[], &ExecutionLimits::default())?;
    let saved = package.save(&model)?;
    let (package, model) = RcorePackage::reload(&saved)?;
    let after = package.invoke(&model, "A", "reachable", &[], &ExecutionLimits::default())?;
    if before.value != after.value {
        return Err("Reload changed behavior".into());
    }
    println!("{}", serde_json::to_string_pretty(&after.value)?);
    Ok(())
}
