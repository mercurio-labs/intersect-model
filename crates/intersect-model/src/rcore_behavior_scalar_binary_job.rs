//! Handwritten retained binary kernel over freshly validated scalar operands.
//! Each advance borrows its current budget/control lease; neither is retained.
use super::{
    ArithmeticErrorKind, BinaryOp, NumericBudget, NumericOperation, Result, Scalar, Value,
    binary_numeric_kernel, boolean, error, scalar,
};
use crate::definitions::retained_text_equal::Equality;

enum Phase<'a> {
    Admit,
    Text(Equality<'a>),
    // Keep the existing numeric/equality controller layout; only concatenation
    // allocates this bounded sidecar and its separately size-checked result.
    Concat(Box<Concatenation<'a>>),
    Done,
}

struct Concatenation<'a> {
    left: &'a str,
    right: &'a str,
    output: String,
    offset: usize,
}
impl<'a> Concatenation<'a> {
    fn new(left: &'a str, right: &'a str, budget: &NumericOperation<'_>) -> Result<Self> {
        let size = left
            .len()
            .checked_add(right.len())
            .ok_or_else(|| super::resource("String result size overflow"))?;
        if size > budget.limits.max_value_bytes
            || size
                > budget
                    .limits
                    .max_total_value_bytes
                    .saturating_sub(budget.value_bytes_used())
        {
            return Err(super::resource("String result byte limit exceeded"));
        }
        let mut output = String::new();
        output
            .try_reserve_exact(size)
            .map_err(|_| super::resource("String result allocation failed"))?;
        Ok(Self {
            left,
            right,
            output,
            offset: 0,
        })
    }
    fn advance(&mut self, budget: &mut NumericOperation<'_>) -> Result<bool> {
        budget.checkpoint()?;
        let (text, offset) = if self.offset < self.left.len() {
            (self.left, self.offset)
        } else {
            (self.right, self.offset - self.left.len())
        };
        let mut end = offset.saturating_add(1024).min(text.len());
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        let fragment = text
            .get(offset..end)
            .ok_or_else(|| super::resource("String concatenation cursor is invalid"))?;
        budget.work(1 + fragment.len().div_ceil(64))?;
        self.output.push_str(fragment);
        self.offset += fragment.len();
        Ok(self.offset == self.left.len() + self.right.len())
    }
}

pub(crate) struct BinaryJob<'a> {
    op: BinaryOp,
    left: &'a Value,
    right: &'a Value,
    phase: Phase<'a>,
}

impl<'a> BinaryJob<'a> {
    /// Callers must first complete the original left and right validation fees.
    pub(crate) fn new(op: BinaryOp, left: &'a Value, right: &'a Value) -> Self {
        Self {
            op,
            left,
            right,
            phase: Phase::Admit,
        }
    }

    #[cfg(feature = "allocation-profile")]
    pub(crate) fn progress(&self) -> Option<(&'static str, usize, usize)> {
        match &self.phase {
            Phase::Text(cursor) => {
                let (offset, total) = cursor.progress();
                Some(("compare-text", offset, total))
            }
            Phase::Concat(cursor) => Some((
                "concat-text",
                cursor.offset,
                cursor.left.len() + cursor.right.len(),
            )),
            Phase::Admit | Phase::Done => None,
        }
    }

    pub(crate) fn advance_prevalidated(
        &mut self,
        budget: &mut NumericBudget,
        cancelled: Option<&dyn Fn() -> bool>,
    ) -> Result<Option<Value>> {
        self.advance(&mut NumericOperation {
            budget,
            cancelled,
            finish_result: false,
        })
    }

    pub(super) fn advance(&mut self, budget: &mut NumericOperation<'_>) -> Result<Option<Value>> {
        match std::mem::replace(&mut self.phase, Phase::Done) {
            Phase::Admit => {
                // Exact original kernel debit and error precedence.
                budget.charge(1)?;
                if self.left.ty != self.right.ty {
                    return Err(error(
                        ArithmeticErrorKind::TypeMismatch,
                        "Scalar operands require exact matching types",
                    ));
                }
                let kind = scalar(self.left)?;
                if kind.numeric() {
                    return binary_numeric_kernel(self.op, self.left, self.right, kind, budget)
                        .map(Some);
                }
                if kind == Scalar::String && self.op == BinaryOp::Add {
                    let left = self.left.payload.as_str().ok_or_else(|| {
                        error(
                            ArithmeticErrorKind::InvalidValue,
                            "Malformed left String operand",
                        )
                    })?;
                    let right = self.right.payload.as_str().ok_or_else(|| {
                        error(
                            ArithmeticErrorKind::InvalidValue,
                            "Malformed right String operand",
                        )
                    })?;
                    self.phase = Phase::Concat(Box::new(Concatenation::new(left, right, budget)?));
                    return Ok(None);
                }
                if !matches!(self.op, BinaryOp::Equal | BinaryOp::NotEqual) {
                    return Err(error(
                        ArithmeticErrorKind::TypeMismatch,
                        "Non-numeric scalar supports equality only",
                    ));
                }
                if matches!(kind, Scalar::String | Scalar::Bytes) {
                    let text = |value: &'a Value| {
                        let payload = if kind == Scalar::Bytes {
                            value.payload.get("hex")
                        } else {
                            Some(&value.payload)
                        };
                        payload.and_then(serde_json::Value::as_str).ok_or_else(|| {
                            error(
                                ArithmeticErrorKind::InvalidValue,
                                "Validated text scalar lost its canonical payload",
                            )
                        })
                    };
                    // Canonical byte tags/shape and String kind were already
                    // checked by the one shared fresh validation engine.
                    return self
                        .compare(Equality::new(text(self.left)?, text(self.right)?), budget);
                }
                // The remaining admitted nonnumeric domains have bounded
                // canonical payloads: Bool, Char16, DateInstant and tagged Unit.
                // Null or malformed Unit retains its earlier admission error.
                budget
                    .finish(boolean(
                        (self.left.payload == self.right.payload) == (self.op == BinaryOp::Equal),
                    ))
                    .map(Some)
            }
            Phase::Text(cursor) => {
                budget.checkpoint()?;
                self.compare(cursor, budget)
            }
            Phase::Concat(mut cursor) => {
                if cursor.advance(budget)? {
                    budget
                        .finish(super::value(
                            Scalar::String,
                            serde_json::Value::String(cursor.output),
                        ))
                        .map(Some)
                } else {
                    self.phase = Phase::Concat(cursor);
                    Ok(None)
                }
            }
            Phase::Done => Err(error(
                ArithmeticErrorKind::InvalidValue,
                "Binary kernel already finished",
            )),
        }
    }

    fn compare(
        &mut self,
        mut cursor: Equality<'a>,
        budget: &mut NumericOperation<'_>,
    ) -> Result<Option<Value>> {
        match cursor.advance() {
            Some(equal) => budget
                .finish(boolean(equal == (self.op == BinaryOp::Equal)))
                .map(Some),
            None => {
                self.phase = Phase::Text(cursor);
                Ok(None)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn string_add_yields_on_utf8_boundaries_and_observes_cancellation() -> Result<()> {
        let left = super::super::value(Scalar::String, json!("é😀".repeat(1000)));
        let right = super::super::value(Scalar::String, json!("終".repeat(1000)));
        let mut budget = NumericBudget::default();
        let mut job = BinaryJob::new(BinaryOp::Add, &left, &right);
        assert!(job.advance_prevalidated(&mut budget, None)?.is_none());
        assert!(job.advance_prevalidated(&mut budget, None)?.is_none());
        let cancelled = job.advance_prevalidated(&mut budget, Some(&|| true));
        assert!(matches!(cancelled, Err(ref e) if e.kind == ArithmeticErrorKind::Cancelled));
        assert!(job.advance_prevalidated(&mut budget, None).is_err());

        let mut job = BinaryJob::new(BinaryOp::Add, &left, &right);
        let mut steps = 0;
        loop {
            steps += 1;
            if let Some(result) = job.advance_prevalidated(&mut budget, None)? {
                assert_eq!(
                    result.payload,
                    json!(format!("{}{}", "é😀".repeat(1000), "終".repeat(1000)))
                );
                break;
            }
            assert!(steps < 100);
        }
        assert!(steps > 4);
        Ok(())
    }

    #[test]
    fn string_add_checks_output_capacity_before_creating_the_result() -> Result<()> {
        let left = super::super::value(Scalar::String, json!("a".repeat(80)));
        let right = super::super::value(Scalar::String, json!("b".repeat(80)));
        let mut budget = NumericBudget::new(super::super::NumericLimits {
            max_value_bytes: 100,
            ..Default::default()
        });
        let mut job = BinaryJob::new(BinaryOp::Add, &left, &right);
        let result = job.advance_prevalidated(&mut budget, None);
        assert!(matches!(result, Err(ref e) if e.kind == ArithmeticErrorKind::ResourceLimit));
        assert_eq!(budget.value_bytes_used(), 0);
        Ok(())
    }

    #[test]
    fn retained_text_kernel_has_one_debit_and_current_cancellation_lease() -> Result<()> {
        let left = super::super::value(Scalar::String, json!("x".repeat(240000)));
        let right = super::super::value(Scalar::String, json!("x".repeat(240000)));
        let mut budget = NumericBudget::default();
        let mut job = BinaryJob::new(BinaryOp::Equal, &left, &right);
        assert!(
            job.advance_prevalidated(&mut budget, Some(&|| false))?
                .is_none()
        );
        assert_eq!(budget.work_used(), 1);
        assert_eq!(budget.value_bytes_used(), 0);
        let result = job.advance_prevalidated(&mut budget, Some(&|| true));
        assert!(matches!(result, Err(ref e) if e.kind == ArithmeticErrorKind::Cancelled));
        assert_eq!(budget.work_used(), 1);
        assert_eq!(budget.value_bytes_used(), 0);
        Ok(())
    }

    #[test]
    fn both_text_operators_drain_without_extra_numeric_fees() -> Result<()> {
        for (kind, payload) in [
            (Scalar::String, json!("é😀".repeat(20000))),
            (
                Scalar::Bytes,
                json!({"kind":"bytes","hex":"AB".repeat(120000)}),
            ),
        ] {
            let left = super::super::value(kind, payload.clone());
            let right = super::super::value(kind, payload);
            for op in [BinaryOp::Equal, BinaryOp::NotEqual] {
                let mut budget = NumericBudget::default();
                let mut job = BinaryJob::new(op, &left, &right);
                let mut steps = 0;
                loop {
                    steps += 1;
                    // These fixed <=240000-byte fixtures must finish within four
                    // ordinary worker quanta while preserving the one kernel debit.
                    assert!(steps <= 1024);
                    if let Some(value) = job.advance_prevalidated(&mut budget, None)? {
                        assert_eq!(value.payload, json!(op == BinaryOp::Equal));
                        break;
                    }
                }
                assert!(steps > 1);
                if kind == Scalar::Bytes {
                    // Real worker first/middle/late coverage needs comparison
                    // work to survive more than three ordinary quantum256 polls.
                    assert!(steps > 3 * 256);
                }
                assert_eq!(budget.work_used(), 1);
                assert_eq!(budget.value_bytes_used(), 0);
            }
        }
        Ok(())
    }
}
