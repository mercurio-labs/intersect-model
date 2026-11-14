//! Handwritten typed container algorithms shared by native/WASM consumers.
use intersect_model::semantic_bindings::{Call, Result};
pub fn numbers(call: &Call<'_>, values: Vec<i64>) -> Result<Vec<i64>> {
    call.budget.charge(values.len() as u64)?;
    Ok(values)
}
pub fn optional(call: &Call<'_>, value: Option<i32>) -> Result<Option<i32>> {
    call.budget.charge(1)?;
    Ok(value)
}
