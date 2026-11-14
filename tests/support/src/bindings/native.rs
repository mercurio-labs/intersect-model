//! Handwritten algorithms shared by static and dynamic registration examples.
use intersect_model::semantic_bindings::{Call, Error, ErrorKind, ModelHandle, Result};

pub fn twice(call: &Call<'_>, value: i64) -> Result<i64> {
    call.budget.charge(1)?;
    value
        .checked_mul(2)
        .ok_or_else(|| Error::new(ErrorKind::Semantic, "Int64 doubling overflow"))
}

pub fn doubled(call: &Call<'_>, meter: ModelHandle) -> Result<i64> {
    twice(call, meter.read_i64("value")?)
}
