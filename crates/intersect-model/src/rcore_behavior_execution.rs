//! Handwritten pure scalar control-flow evaluator over the definitions verifier.
//! Arithmetic lives in rcore_behavior_scalar; this module owns no source/type inference.
use crate::rcore_behavior_scalar::{ArithmeticErrorKind, NumericLimits};
use intersect_model::definitions::{
    bundle::SourceSpan,
    rcore_behavior::{
        ir::{Value, ValueLimits, VerifiedProgram},
        modules::SymbolId,
    },
};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExecutionLimits {
    pub max_steps: usize,
    /// Cumulative bytes of arguments, registers and values copied across edges.
    pub max_value_bytes: usize,
    pub values: ValueLimits,
    pub numeric: NumericLimits,
}
impl Default for ExecutionLimits {
    fn default() -> Self {
        Self {
            max_steps: 100_000,
            max_value_bytes: 4_194_304,
            values: ValueLimits::default(),
            numeric: NumericLimits::default(),
        }
    }
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub enum ExecutionErrorKind {
    Arguments,
    ResourceExhausted,
    Arithmetic,
    InvalidProgram,
}
#[derive(Clone, Debug, Serialize)]
pub struct ExecutionError {
    pub kind: ExecutionErrorKind,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub arithmetic_kind: Option<ArithmeticErrorKind>,
    pub symbol: SymbolId,
    pub span: Option<Box<SourceSpan>>,
}
impl std::fmt::Display for ExecutionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}
impl std::error::Error for ExecutionError {}
#[derive(Clone, Debug, Serialize)]
pub struct Evaluation {
    pub value: Value,
    pub steps: usize,
    pub numeric_work: u64,
    pub value_bytes: usize,
}

/// Execute a verified v1 image through the common function frame machine with
/// its exact legacy accounting, lazy constants and error policy.
pub fn evaluate(
    verified: &VerifiedProgram,
    arguments: &[Value],
    limits: &ExecutionLimits,
) -> Result<Evaluation, ExecutionError> {
    crate::rcore_behavior_functions::evaluate_scalar(verified, arguments, limits)
}
