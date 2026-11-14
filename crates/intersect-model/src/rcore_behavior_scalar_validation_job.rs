//! Retained numeric admission over the one shared canonical scalar/JSON engine.
//! Ordinary numeric APIs drain this exact cursor. Its stages do not change
//! numeric work/value fees; scheduling is the calling session's responsibility.
use super::{ArithmeticErrorKind, NumericBudget, NumericLimits, Result, error, resource};
use crate::definitions::{
    metadata_limits::serialized_size,
    rcore_behavior::ir::{
        IrErrorKind, Value, ValueLimits, ValueType,
        scalar_validation::{ScalarSource, ScalarValidation, measurement_admission},
    },
    retained_json::Admission,
    retained_json_frontier::MeasureJob,
};
enum ValidationStage<'a> {
    Validate(ScalarValidation<'a>),
    Measure(MeasureJob<'a>),
    Done(usize),
    Failed,
}
pub(crate) struct NumericValidation<'a> {
    source: ScalarSource<'a>,
    stage: ValidationStage<'a>,
}
impl<'a> NumericValidation<'a> {
    #[cfg(feature = "allocation-profile")]
    pub(crate) fn progress(&self) -> Option<(&'static str, usize, usize)> {
        match &self.stage {
            ValidationStage::Validate(value) => value.progress(),
            ValidationStage::Measure(value) => {
                Some(("measure", value.encoded_bytes(), value.frontier_capacity()))
            }
            ValidationStage::Done(bytes) => Some(("done", *bytes, 0)),
            ValidationStage::Failed => None,
        }
    }

    pub(crate) fn advance(&mut self, budget: &mut NumericBudget) -> Result<Option<usize>> {
        let mut admission = measurement_admission(budget.limits.max_value_bytes);
        self.advance_with_admission(budget, &mut admission)
    }
    pub(crate) fn new(source: &'a Value, limits: &NumericLimits) -> Self {
        Self::from_source(ScalarSource::borrowed(source), limits)
    }
    pub(crate) fn from_source(source: ScalarSource<'a>, limits: &NumericLimits) -> Self {
        Self {
            source,
            stage: ValidationStage::Validate(ScalarValidation::full_source(
                source,
                &ValueLimits {
                    max_value_bytes: limits.max_value_bytes,
                    max_digits: limits.max_digits,
                },
            )),
        }
    }
    pub(crate) fn advance_with_admission(
        &mut self,
        budget: &mut NumericBudget,
        admission: &mut impl Admission,
    ) -> Result<Option<usize>> {
        match std::mem::replace(&mut self.stage, ValidationStage::Failed) {
            ValidationStage::Validate(mut validation) => {
                let complete = validation
                    .advance(&ValueLimits {
                        max_value_bytes: budget.limits.max_value_bytes,
                        max_digits: budget.limits.max_digits,
                    })
                    .map_err(|issue| {
                        let issue = issue.into_ir();
                        error(
                            if issue.kind == IrErrorKind::ResourceExhausted {
                                ArithmeticErrorKind::ResourceLimit
                            } else {
                                ArithmeticErrorKind::InvalidValue
                            },
                            issue.message,
                        )
                    })?;
                self.stage = if complete {
                    ValidationStage::Measure(MeasureJob::new(self.source.payload))
                } else {
                    ValidationStage::Validate(validation)
                };
                Ok(None)
            }
            ValidationStage::Measure(mut measurement) => {
                if !measurement
                    .advance(admission)
                    .map_err(|issue| error(ArithmeticErrorKind::InvalidValue, issue))?
                {
                    self.stage = ValidationStage::Measure(measurement);
                    return Ok(None);
                }
                // SAME official Value schema and original ValueType, borrowed.
                // Null substitutes only the payload for constant header measure.
                let header = Value {
                    ty: ValueType::Scalar(self.source.kind.ok_or_else(|| {
                        error(ArithmeticErrorKind::InvalidValue, "Expected scalar value")
                    })?),
                    payload: (),
                };
                let bytes = serialized_size(&header, usize::MAX)
                    .map_err(|issue| error(ArithmeticErrorKind::InvalidValue, issue))?
                    .checked_sub(4)
                    .and_then(|n| n.checked_add(measurement.encoded_bytes()))
                    .ok_or_else(|| resource("Numeric value byte size overflow"))?;
                if bytes > budget.limits.max_value_bytes {
                    return Err(resource("Numeric value byte limit exceeded"));
                }
                // Original numeric fee is charged once, after all original
                // validation and exact size checks. Continuations add no fee.
                budget.work(bytes)?;
                self.stage = ValidationStage::Done(bytes);
                Ok(Some(bytes))
            }
            ValidationStage::Done(bytes) => {
                self.stage = ValidationStage::Done(bytes);
                Ok(Some(bytes))
            }
            ValidationStage::Failed => Err(error(
                ArithmeticErrorKind::InvalidValue,
                "Numeric validation already failed",
            )),
        }
    }
}
