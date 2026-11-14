//! Pure S06 callable execution. Runtime handles are private to one evaluation.
//! The frame machine shares S05 scalar arithmetic and authoritative nominal validators.
use crate::rcore_behavior_scalar::{ArithmeticErrorKind, NumericBudget};
use intersect_model::definitions::{
    bundle::{DefinitionBundle, SourceSpan},
    conversion::{ConversionRegistry, ProviderId, ReferenceContext},
    rcore_behavior::ir_v2::{FunctionIdentity, ValueWire, VerifiedExecutable},
};
use serde::{Deserialize, Serialize};

/// Verification-only description of the actual retained numeric owner phase.
#[cfg(feature = "allocation-profile")]
pub type RetainedNumericPhase = (
    &'static str,
    Option<(&'static str, usize, usize)>,
    usize,
    usize,
);

#[path = "rcore_behavior_functions_algebra.rs"]
mod algebra;
#[path = "rcore_behavior_functions_closure.rs"]
mod closure;
#[path = "rcore_behavior_functions_collections.rs"]
mod collections;
#[path = "rcore_behavior_functions_context.rs"]
pub(crate) mod context;
#[path = "rcore_behavior_functions_conversion.rs"]
mod conversion;
#[path = "rcore_behavior_functions_decoding.rs"]
mod decoding;
#[path = "rcore_behavior_functions_encoding.rs"]
mod encoding;
#[path = "rcore_behavior_functions_equivalence.rs"]
mod equivalence;
#[path = "rcore_behavior_functions_grouping.rs"]
mod grouping;
#[path = "rcore_behavior_functions_inputs.rs"]
pub(crate) mod inputs;
#[path = "rcore_behavior_functions_machine.rs"]
mod machine;
#[path = "rcore_behavior_functions_membership.rs"]
mod membership;
#[path = "rcore_behavior_functions_navigation.rs"]
mod navigation;
#[path = "rcore_behavior_functions_nominal.rs"]
mod nominal;
#[path = "rcore_behavior_functions_numeric.rs"]
mod numeric;
#[path = "rcore_behavior_functions_observation.rs"]
mod observation;
pub use observation::{ObservationLimits, ReadKind, ReadReport, SemanticRead, ValidationAdmission};
#[path = "rcore_behavior_functions_ownership.rs"]
mod ownership;
#[path = "rcore_behavior_functions_payload.rs"]
mod payload;
#[path = "rcore_behavior_functions_preflight.rs"]
mod preflight;
#[path = "rcore_behavior_functions_providers.rs"]
mod providers;
#[path = "rcore_behavior_functions_scalar.rs"]
mod scalar;
#[path = "rcore_behavior_functions_sorting.rs"]
mod sorting;
#[path = "rcore_behavior_functions_values.rs"]
mod values;
pub(crate) use machine::BorrowedEvaluationSession;
pub use machine::{EvaluationSession, SessionProgress};
pub(crate) use scalar::evaluate_scalar;

pub struct ExecutionContext<'a> {
    pub bundle: &'a DefinitionBundle,
    pub conversions: Option<&'a ConversionRegistry<'a>>,
    pub references: &'a [ReferenceContext],
}
impl<'a> ExecutionContext<'a> {
    pub fn new(bundle: &'a DefinitionBundle) -> Self {
        Self {
            bundle,
            conversions: None,
            references: &[],
        }
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExecutionLimits {
    #[serde(default)]
    pub observation: ObservationLimits,
    pub scalar: crate::rcore_behavior_execution::ExecutionLimits,
    pub max_frames: usize,
    pub max_closures: usize,
    /// Cumulative capture slots created and transferred into call frames.
    pub max_captures: usize,
    pub max_value_nodes: usize,
    pub max_value_depth: usize,
    pub max_collection_comparisons: usize,
    pub max_trace_frames: usize,
    /// Encoded bytes of retained leaf span, trace frames and selected/consumed provider items.
    pub max_trace_bytes: usize,
    pub max_model_contexts: usize,
    pub max_reference_domains: usize,
    pub max_context_bytes: usize,
    /// Elements admitted across distinct immutable model contexts.
    pub max_model_elements: usize,
    /// Candidate objects inspected across all selected target domains.
    pub max_membership_checks: usize,
    /// Unique encoded selected and consumed provider identities retained per evaluation.
    pub max_provider_evidence_bytes: usize,
    /// Actual semantic provider callbacks, including nested validation.
    pub max_provider_invocations: usize,
    /// Aggregate dispatch and explicitly metered provider traversal units.
    #[serde(default = "default_provider_work")]
    pub max_provider_work: usize,
    /// Cumulative logical context-copy and decoded-model storage reservations.
    /// This is separate from encoded context bytes and runtime value transfers.
    #[serde(default = "default_model_storage")]
    pub max_model_storage_bytes: usize,
}
fn default_model_storage() -> usize {
    16_777_216
}
fn default_provider_work() -> usize {
    1048576
}
impl Default for ExecutionLimits {
    fn default() -> Self {
        Self {
            observation: ObservationLimits::default(),
            scalar: Default::default(),
            max_frames: 256,
            max_closures: 65536,
            max_captures: 65536,
            max_value_nodes: 65536,
            max_value_depth: 64,
            max_collection_comparisons: 1048576,
            max_trace_frames: 256,
            max_trace_bytes: 65536,
            max_model_contexts: 8,
            max_reference_domains: 128,
            max_context_bytes: 16777216,
            max_model_elements: 100000,
            max_membership_checks: 1048576,
            max_provider_evidence_bytes: 65536,
            max_provider_invocations: 1048576,
            max_provider_work: default_provider_work(),
            max_model_storage_bytes: default_model_storage(),
        }
    }
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub enum ExecutionErrorKind {
    Arguments,
    ResourceExhausted,
    Cancelled,
    Arithmetic,
    InvalidProgram,
    Unsupported,
    Nominal,
    MissingDependency,
}
#[derive(Clone, Debug, Serialize)]
pub struct TraceFrame {
    pub function: FunctionIdentity,
    pub definition: SourceSpan,
    pub call_site: Option<SourceSpan>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub enum DependencyCause {
    ExecutableContext(intersect_model::definitions::rcore_behavior::ir_v2::ContextDependency),
    ModelContextRevision,
    Provider(intersect_model::definitions::conversion::ProviderDependencyKind),
}
#[derive(Clone, Debug, Serialize)]
pub struct ExecutionError {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub observation: Option<Box<ReadReport>>,
    pub kind: ExecutionErrorKind,
    pub message: Box<String>,
    pub arithmetic_kind: Option<ArithmeticErrorKind>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dependency: Option<Box<DependencyCause>>,
    pub span: Option<Box<SourceSpan>>,
    pub trace: Vec<TraceFrame>,
    pub trace_truncated: bool,
    pub selected_providers: Vec<ProviderId>,
    pub consumed_providers: Vec<ProviderId>,
    pub provider_evidence_truncated: bool,
    pub consumed: Box<Consumption>,
}
impl std::fmt::Display for ExecutionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}
impl std::error::Error for ExecutionError {}
#[derive(Clone, Debug, Default, Serialize)]
pub struct Consumption {
    pub steps: usize,
    pub numeric_work: u64,
    pub value_bytes: usize,
    pub value_nodes: usize,
    pub closures: usize,
    pub captures: usize,
    pub collection_comparisons: usize,
    pub model_contexts: usize,
    pub reference_domains: usize,
    pub context_bytes: usize,
    pub model_elements: usize,
    pub membership_checks: usize,
    pub provider_invocations: usize,
    pub provider_work: usize,
    pub provider_evidence_bytes: usize,
    pub model_storage_bytes: usize,
}
#[derive(Clone, Debug, Serialize)]
pub struct Evaluation {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub observation: Option<ReadReport>,
    pub value: ValueWire,
    pub selected_providers: Vec<ProviderId>,
    pub consumed_providers: Vec<ProviderId>,
    pub consumed: Consumption,
}
/// Host-owned cancellation authority. Implementations must be cheap and must
/// not mutate source/model state. The evaluator supplies no clock or scheduler.
/// Cancellation is observed before a session work unit, never retroactively.
pub trait ExecutionControl {
    fn cancellation_requested(&self) -> bool;
}

type Result<T> = std::result::Result<T, ExecutionError>;

pub fn evaluate(
    image: &VerifiedExecutable,
    arguments: &[ValueWire],
    context: &ExecutionContext<'_>,
    limits: &ExecutionLimits,
) -> Result<Evaluation> {
    machine::evaluate(image, arguments, context, limits)
}

#[cfg(feature = "allocation-profile")]
#[path = "rcore_behavior_numeric_validation_candidate.rs"]
mod numeric_validation_candidate;
#[cfg(feature = "allocation-profile")]
pub fn numeric_validation_candidate_layouts() -> serde_json::Value {
    let mut layouts = numeric_validation_candidate::observe_layouts();
    layouts["machine_frame"] = serde_json::json!(std::mem::size_of::<machine::Frame>());
    layouts["default_numeric"] = numeric::Numeric::layouts();
    layouts["execution_state"] = serde_json::json!(std::mem::size_of::<machine::ExecutionState>());
    layouts["arena_node"] = serde_json::json!(std::mem::size_of::<values::Node>());
    layouts
}
#[cfg(feature = "allocation-profile")]
pub fn numeric_validation_candidate_observe(
    value: &intersect_model::definitions::rcore_behavior::ir::Value,
    limits: &crate::rcore_behavior_scalar::NumericLimits,
    admission: &mut impl intersect_model::definitions::retained_json::Admission,
) -> serde_json::Value {
    numeric_validation_candidate::observe_validation(value, limits, admission)
}

#[cfg(feature = "allocation-profile")]
pub fn numeric_validation_candidate_owned(
    value: intersect_model::definitions::rcore_behavior::ir::Value,
    limits: &crate::rcore_behavior_scalar::NumericLimits,
    admission: &mut impl intersect_model::definitions::retained_json::Admission,
) -> serde_json::Value {
    numeric_validation_candidate::observe_owned(value, limits, admission)
}

#[cfg(feature = "allocation-profile")]
pub fn numeric_control_candidate_observe(
    cases: &serde_json::Value,
) -> std::result::Result<serde_json::Value, String> {
    numeric_validation_candidate::observe_controlled(cases)
}
