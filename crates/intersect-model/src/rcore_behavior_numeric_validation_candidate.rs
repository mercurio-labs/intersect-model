//! Private prototype: original validation priority, shared retained JSON measure.
//! No default operation path uses this until complete ownership/admission proof.
use crate::rcore_behavior_scalar::{NumericBudget, NumericLimits};
use intersect_model::definitions::rcore_behavior::ir::Value;
use intersect_model::definitions::{
    rcore_behavior::ir::scalar_validation::{ScalarSource, ScalarValidation},
    rcore_behavior::types::Scalar,
    retained_json::Admission,
    retained_json_frontier::MeasureJob,
};
use serde_json::json;

use crate::rcore_behavior_scalar::validation::NumericValidation;

// The actual default input owner is now measured by numeric::Numeric::layouts.
// This independent result probe remains a direct owning-cell consumer.
struct ResultOwner {
    kind: Scalar,
    payload: serde_json::Value,
}

self_cell::self_cell! {
    struct ResultCell {
        owner: ResultOwner,
        #[covariant]
        dependent: NumericValidation,
    }
}

pub(super) fn observe_layouts() -> serde_json::Value {
    use std::mem::size_of;
    json!({
        "numeric_validation":size_of::<NumericValidation>(),
        "scalar_validation":size_of::<ScalarValidation>(),
        "measure":size_of::<MeasureJob>(),
        "result_owner":size_of::<ResultOwner>(),
        "result_owner_and_dependent":size_of::<(ResultOwner,NumericValidation)>(),
        "result_cell_header":size_of::<ResultCell>(),
        "result_cell_total":size_of::<ResultCell>()+size_of::<(ResultOwner,NumericValidation)>(),
        "scalar_source":size_of::<ScalarSource>(),
        "scope":"actual compact structs and self-cell header; not default Session admission",
    })
}

pub(super) fn observe_validation(
    value: &Value,
    limits: &NumericLimits,
    admission: &mut impl Admission,
) -> serde_json::Value {
    let mut original = NumericBudget::new(limits.clone());
    let expected = original.validate(value);
    let mut budget = NumericBudget::new(limits.clone());
    let mut cursor = NumericValidation::new(value, limits);
    let mut polls = 0;
    let result = loop {
        polls += 1;
        match cursor.advance_with_admission(&mut budget, admission) {
            Ok(None) => {}
            result => break result,
        }
    };
    let after = cursor.advance_with_admission(&mut budget, admission);
    json!({"result":result,"after":after,"polls":polls,"work":budget.work_used(),"original":{"result":expected,"work":original.work_used()}})
}

pub(super) fn observe_owned(
    value: Value,
    limits: &NumericLimits,
    admission: &mut impl Admission,
) -> serde_json::Value {
    let intersect_model::definitions::rcore_behavior::ir::ValueType::Scalar(kind) = value.ty else {
        return json!({"unsupported":"result kernels must produce actual Scalar"});
    };
    let owner = ResultOwner {
        kind,
        payload: value.payload,
    };
    let mut cell = ResultCell::new(owner, |owner| {
        NumericValidation::from_source(ScalarSource::scalar(owner.kind, &owner.payload), limits)
    });
    let mut budget = NumericBudget::new(limits.clone());
    let mut polls = 0;
    let result = loop {
        polls += 1;
        let result = cell
            .with_dependent_mut(|_, cursor| cursor.advance_with_admission(&mut budget, admission));
        match result {
            Ok(None) => {}
            result => break result,
        }
    };
    let after =
        cell.with_dependent_mut(|_, cursor| cursor.advance_with_admission(&mut budget, admission));
    let owner = cell.into_owner();
    json!({"result":result,"after":after,"polls":polls,"work":budget.work_used(),"value":Value {ty:intersect_model::definitions::rcore_behavior::ir::ValueType::Scalar(owner.kind),payload:owner.payload}})
}

pub(super) fn observe_controlled(
    cases: &serde_json::Value,
) -> std::result::Result<serde_json::Value, String> {
    fn invoke(
        case: &serde_json::Value,
        stop: Option<usize>,
    ) -> std::result::Result<serde_json::Value, String> {
        let left: Value =
            serde_json::from_value(case["left"].clone()).map_err(|e| e.to_string())?;
        let right = if case["mode"] == "binary" {
            Some(
                serde_json::from_value::<Value>(case["right"].clone())
                    .map_err(|e| e.to_string())?,
            )
        } else {
            None
        };
        let count = std::cell::Cell::new(0usize);
        let cancelled = || {
            let old = count.get();
            count.set(old + 1);
            stop.is_some_and(|limit| old >= limit)
        };
        let mut budget = NumericBudget::default();
        let result = match right {
            Some(right) => crate::rcore_behavior_scalar::binary_controlled(
                serde_json::from_value(case["op"].clone()).map_err(|e| e.to_string())?,
                &left,
                &right,
                &mut budget,
                Some(&cancelled),
            ),
            None => crate::rcore_behavior_scalar::unary_controlled(
                serde_json::from_value(case["op"].clone()).map_err(|e| e.to_string())?,
                &left,
                &mut budget,
                Some(&cancelled),
            ),
        };
        Ok(
            json!({"stop":stop,"callback_invocations":count.get(),"result":result,"work":budget.work_used(),"value_bytes":budget.value_bytes_used()}),
        )
    }
    let mut rows = Vec::new();
    for case in cases
        .as_array()
        .ok_or("Controlled cases must be an array")?
    {
        let normal = invoke(case, None)?;
        let count = normal["callback_invocations"]
            .as_u64()
            .ok_or("Missing callback count")?;
        let mut prefixes = Vec::new();
        for stop in 0..=usize::try_from(count).map_err(|e| e.to_string())? + 1 {
            prefixes.push(invoke(case, Some(stop))?);
        }
        rows.push(json!({"case":case,"normal":normal,"cancellation_prefixes":prefixes}));
    }
    Ok(json!(rows))
}
