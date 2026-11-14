//! Handwritten operation facade over the shared callable runtime.
use crate::rcore_behavior_functions::{
    self, Evaluation, ExecutionContext, ExecutionError, ExecutionLimits,
};
use intersect_model::definitions::{
    rcore_behavior::ir_v2::ValueWire, rcore_operations::VerifiedOperation,
};

/// Receiver is the first checked argument; the remaining arguments follow the
/// exact declared parameter order. Model admission, budgets and failures belong
/// to the shared evaluator, not a second operation runtime.
pub fn invoke(
    operation: &VerifiedOperation,
    arguments: &[ValueWire],
    context: &ExecutionContext<'_>,
    limits: &ExecutionLimits,
) -> Result<Evaluation, ExecutionError> {
    rcore_behavior_functions::evaluate(operation.executable(), arguments, context, limits)
}
