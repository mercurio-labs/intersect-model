//! Bounded scalar arithmetic over the definitions-owned checked wire representation.
use intersect_model::definitions::rcore_behavior::{
    ir::{BinaryOp, UnaryOp, Value, ValueType},
    types::Scalar,
};
use num_bigint::BigInt;
use num_traits::{ToPrimitive, Zero};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::cmp::Ordering;
#[path = "rcore_behavior_scalar_binary_job.rs"]
pub(crate) mod binary_job;
#[path = "rcore_behavior_scalar_decimal.rs"]
mod decimal;
#[path = "rcore_behavior_scalar_validation_job.rs"]
pub(crate) mod validation;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NumericLimits {
    pub max_digits: usize,
    pub max_intermediate_digits: usize,
    pub max_scale_alignment: usize,
    pub max_value_bytes: usize,
    pub max_total_value_bytes: usize,
    pub max_work: u64,
}
impl Default for NumericLimits {
    fn default() -> Self {
        Self {
            max_digits: 4096,
            max_intermediate_digits: 8192,
            max_scale_alignment: 4096,
            max_value_bytes: 262144,
            max_total_value_bytes: 1048576,
            max_work: 16777216,
        }
    }
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ArithmeticErrorKind {
    InvalidValue,
    TypeMismatch,
    Overflow,
    DivisionByZero,
    NonTerminatingDecimal,
    ScaleOverflow,
    ResourceLimit,
    Cancelled,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ArithmeticError {
    pub kind: ArithmeticErrorKind,
    pub message: String,
}
impl std::fmt::Display for ArithmeticError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}
impl std::error::Error for ArithmeticError {}
pub(super) fn error(kind: ArithmeticErrorKind, message: impl Into<String>) -> ArithmeticError {
    ArithmeticError {
        kind,
        message: message.into(),
    }
}
pub(super) fn resource(message: &str) -> ArithmeticError {
    error(ArithmeticErrorKind::ResourceLimit, message)
}
pub(super) type Result<T> = std::result::Result<T, ArithmeticError>;
#[derive(Clone, Debug)]
pub struct NumericBudget {
    pub limits: NumericLimits,
    used: u64,
    value_bytes: usize,
}
impl NumericBudget {
    pub fn new(limits: NumericLimits) -> Self {
        Self {
            limits,
            used: 0,
            value_bytes: 0,
        }
    }
    pub fn work_used(&self) -> u64 {
        self.used
    }
    pub fn value_bytes_used(&self) -> usize {
        self.value_bytes
    }
    pub fn charge(&mut self, work: u64) -> Result<()> {
        let next = self
            .used
            .checked_add(work)
            .ok_or_else(|| resource("Numeric work limit exceeded"))?;
        if next > self.limits.max_work {
            return Err(resource("Numeric work limit exceeded"));
        }
        self.used = next;
        Ok(())
    }
    pub(super) fn work(&mut self, work: usize) -> Result<()> {
        self.charge(u64::try_from(work).map_err(|_| resource("Numeric work estimate overflow"))?)
    }
    pub(super) fn intermediate(&self, digits: usize) -> Result<()> {
        if digits > self.limits.max_intermediate_digits {
            Err(resource("Numeric intermediate digit limit exceeded"))
        } else {
            Ok(())
        }
    }
    pub(super) fn validate(&mut self, value: &Value) -> Result<()> {
        self.validate_bytes(value).map(|_| ())
    }
    fn validate_bytes(&mut self, value: &Value) -> Result<usize> {
        let mut cursor = validation::NumericValidation::new(value, &self.limits);
        loop {
            if let Some(bytes) = cursor.advance(self)? {
                return Ok(bytes);
            }
        }
    }
    pub(super) fn finish(&mut self, value: Value) -> Result<Value> {
        let bytes = self.validate_bytes(&value)?;
        self.finish_measured(bytes)?;
        Ok(value)
    }
    // Only a fresh completed NumericValidation may supply this measurement.
    // No serialized/cached certificate enters this private authority boundary.
    pub(crate) fn finish_measured(&mut self, bytes: usize) -> Result<()> {
        let next = self
            .value_bytes
            .checked_add(bytes)
            .ok_or_else(|| resource("Numeric result byte limit exceeded"))?;
        if next > self.limits.max_total_value_bytes {
            return Err(resource("Numeric result byte limit exceeded"));
        }
        self.value_bytes = next;
        Ok(())
    }
}
// Per-operation borrowing keeps cancellation authority out of retained budgets.
struct NumericOperation<'a> {
    budget: &'a mut NumericBudget,
    cancelled: Option<&'a dyn Fn() -> bool>,
    // Internal phase selection, never a wire/profile/poll-compatibility flag.
    // The default synchronous facade runs validation+kernel+finish; the default
    // session runs those SAME phases across separately admitted continuations.
    finish_result: bool,
}
impl std::ops::Deref for NumericOperation<'_> {
    type Target = NumericBudget;
    fn deref(&self) -> &Self::Target {
        self.budget
    }
}
impl NumericOperation<'_> {
    fn checkpoint(&self) -> Result<()> {
        if self.cancelled.is_some_and(|poll| poll()) {
            Err(error(
                ArithmeticErrorKind::Cancelled,
                "Execution cancelled during numeric work",
            ))
        } else {
            Ok(())
        }
    }
    fn charge(&mut self, work: u64) -> Result<()> {
        self.checkpoint()?;
        self.budget.charge(work)
    }
    fn work(&mut self, work: usize) -> Result<()> {
        self.checkpoint()?;
        self.budget.work(work)
    }
    fn validate(&mut self, value: &Value) -> Result<()> {
        self.checkpoint()?;
        self.budget.validate(value)
    }
    fn finish(&mut self, value: Value) -> Result<Value> {
        self.checkpoint()?;
        if self.finish_result {
            self.budget.finish(value)
        } else {
            Ok(value)
        }
    }
}
impl Default for NumericBudget {
    fn default() -> Self {
        Self::new(NumericLimits::default())
    }
}
fn scalar(value: &Value) -> Result<Scalar> {
    match value.ty {
        ValueType::Scalar(s) => Ok(s),
        _ => Err(error(
            ArithmeticErrorKind::TypeMismatch,
            "Numeric scalar operation cannot consume an enum",
        )),
    }
}
pub(super) fn value(scalar: Scalar, payload: serde_json::Value) -> Value {
    Value {
        ty: ValueType::Scalar(scalar),
        payload,
    }
}
fn boolean(result: bool) -> Value {
    value(Scalar::Bool, json!(result))
}
pub(super) fn digits(value: &BigInt) -> usize {
    value.to_str_radix(10).trim_start_matches('-').len()
}
pub(super) fn product_work(a: usize, b: usize) -> Result<usize> {
    a.checked_mul(b)
        .ok_or_else(|| resource("Numeric work estimate overflow"))
}
pub(super) fn sum_size(a: usize, b: usize) -> Result<usize> {
    a.checked_add(b)
        .ok_or_else(|| resource("Numeric size estimate overflow"))
}
fn integer_text(text: &str, budget: &mut NumericOperation<'_>) -> Result<BigInt> {
    let count = text.trim_start_matches('-').len();
    if count > budget.limits.max_digits {
        return Err(resource("Numeric coefficient digit limit exceeded"));
    }
    budget.work(count)?;
    BigInt::parse_bytes(text.as_bytes(), 10).ok_or_else(|| {
        error(
            ArithmeticErrorKind::InvalidValue,
            "Invalid integer coefficient",
        )
    })
}
fn integer(value: &Value, budget: &mut NumericOperation<'_>) -> Result<BigInt> {
    let text = match scalar(value)? {
        Scalar::Int64 | Scalar::UInt64 | Scalar::BigInt => value
            .payload
            .get("value")
            .and_then(|v| v.as_str())
            .ok_or_else(|| error(ArithmeticErrorKind::InvalidValue, "Missing integer payload"))?
            .to_owned(),
        _ => value.payload.to_string(),
    };
    integer_text(&text, budget)
}
fn integer_result(s: Scalar, n: BigInt, budget: &mut NumericOperation<'_>) -> Result<Value> {
    let payload = match s {
        Scalar::BigInt => {
            if digits(&n) > budget.limits.max_digits {
                return Err(resource("Numeric result digit limit exceeded"));
            }
            json!({"kind":"bigint","value":n.to_str_radix(10)})
        }
        Scalar::Int8 | Scalar::Int16 | Scalar::Int32 | Scalar::Int64 => {
            let n = n.to_i64().ok_or_else(|| {
                error(ArithmeticErrorKind::Overflow, "Signed arithmetic overflow")
            })?;
            let (min, max) = match s {
                Scalar::Int8 => (i64::from(i8::MIN), i64::from(i8::MAX)),
                Scalar::Int16 => (i64::from(i16::MIN), i64::from(i16::MAX)),
                Scalar::Int32 => (i64::from(i32::MIN), i64::from(i32::MAX)),
                _ => (i64::MIN, i64::MAX),
            };
            if n < min || n > max {
                return Err(error(
                    ArithmeticErrorKind::Overflow,
                    "Signed arithmetic overflow",
                ));
            }
            if s == Scalar::Int64 {
                json!({"kind":"int64","value":n.to_string()})
            } else {
                json!(n)
            }
        }
        Scalar::UInt8 | Scalar::UInt16 | Scalar::UInt32 | Scalar::UInt64 => {
            let n = n.to_u64().ok_or_else(|| {
                error(
                    ArithmeticErrorKind::Overflow,
                    "Unsigned arithmetic overflow or underflow",
                )
            })?;
            let max = match s {
                Scalar::UInt8 => u64::from(u8::MAX),
                Scalar::UInt16 => u64::from(u16::MAX),
                Scalar::UInt32 => u64::from(u32::MAX),
                _ => u64::MAX,
            };
            if n > max {
                return Err(error(
                    ArithmeticErrorKind::Overflow,
                    "Unsigned arithmetic overflow",
                ));
            }
            if s == Scalar::UInt64 {
                json!({"kind":"uint64","value":n.to_string()})
            } else {
                json!(n)
            }
        }
        _ => {
            return Err(error(
                ArithmeticErrorKind::TypeMismatch,
                "Expected integer scalar",
            ));
        }
    };
    budget.finish(value(s, payload))
}
pub(super) fn compare(op: BinaryOp, ordering: Ordering) -> Result<bool> {
    Ok(match op {
        BinaryOp::Equal => ordering == Ordering::Equal,
        BinaryOp::NotEqual => ordering != Ordering::Equal,
        BinaryOp::Less => ordering == Ordering::Less,
        BinaryOp::LessEqual => ordering != Ordering::Greater,
        BinaryOp::Greater => ordering == Ordering::Greater,
        BinaryOp::GreaterEqual => ordering != Ordering::Less,
        _ => {
            return Err(error(
                ArithmeticErrorKind::TypeMismatch,
                "Expected comparison operator",
            ));
        }
    })
}
pub(super) fn comparison(op: BinaryOp) -> bool {
    matches!(
        op,
        BinaryOp::Equal
            | BinaryOp::NotEqual
            | BinaryOp::Less
            | BinaryOp::LessEqual
            | BinaryOp::Greater
            | BinaryOp::GreaterEqual
    )
}
fn float_bits(value: &Value) -> Result<u64> {
    u64::from_str_radix(
        value
            .payload
            .get("bits")
            .and_then(|v| v.as_str())
            .ok_or_else(|| error(ArithmeticErrorKind::InvalidValue, "Missing float bits"))?,
        16,
    )
    .map_err(|_| error(ArithmeticErrorKind::InvalidValue, "Invalid float bits"))
}
fn float_result(s: Scalar, n: f64) -> Value {
    let bits = if n.is_nan() {
        0x7ff8000000000000
    } else {
        n.to_bits()
    };
    value(s, json!({"kind":"float64","bits":format!("{bits:016x}")}))
}
fn single_result(n: f32) -> Value {
    let bits = if n.is_nan() { 0x7fc00000 } else { n.to_bits() };
    value(
        Scalar::Float32,
        json!({"kind":"float32","bits":format!("{bits:08x}")}),
    )
}
macro_rules! float_binary {
    ($op:expr,$a:expr,$b:expr,$emit:expr,$budget:expr) => {{
        let a = $a;
        let b = $b;
        let output = match $op {
            BinaryOp::Add => ($emit)(a + b),
            BinaryOp::Subtract => ($emit)(a - b),
            BinaryOp::Multiply => ($emit)(a * b),
            BinaryOp::Divide => ($emit)(a / b),
            BinaryOp::Remainder => ($emit)(a % b),
            BinaryOp::Equal => boolean(a == b),
            BinaryOp::NotEqual => boolean(a != b),
            BinaryOp::Less => boolean(a < b),
            BinaryOp::LessEqual => boolean(a <= b),
            BinaryOp::Greater => boolean(a > b),
            BinaryOp::GreaterEqual => boolean(a >= b),
        };
        $budget.finish(output)
    }};
}
pub fn unary(op: UnaryOp, input: &Value, budget: &mut NumericBudget) -> Result<Value> {
    unary_controlled(op, input, budget, None)
}
pub(crate) fn unary_controlled(
    op: UnaryOp,
    input: &Value,
    budget: &mut NumericBudget,
    cancelled: Option<&dyn Fn() -> bool>,
) -> Result<Value> {
    unary_inner(
        op,
        input,
        &mut NumericOperation {
            budget,
            cancelled,
            finish_result: true,
        },
    )
}
fn unary_inner(op: UnaryOp, input: &Value, budget: &mut NumericOperation<'_>) -> Result<Value> {
    budget.validate(input)?;
    unary_kernel(op, input, budget)
}
fn unary_kernel(op: UnaryOp, input: &Value, budget: &mut NumericOperation<'_>) -> Result<Value> {
    budget.charge(1)?;
    let s = scalar(input)?;
    if op == UnaryOp::Not {
        return if s == Scalar::Bool {
            budget.finish(boolean(!input.payload.as_bool().ok_or_else(|| {
                error(ArithmeticErrorKind::InvalidValue, "Invalid Boolean")
            })?))
        } else {
            Err(error(
                ArithmeticErrorKind::TypeMismatch,
                "Logical not requires Bool",
            ))
        };
    }
    if !s.numeric() {
        return Err(error(
            ArithmeticErrorKind::TypeMismatch,
            "Unary arithmetic requires a numeric scalar",
        ));
    }
    if s == Scalar::Float32 {
        let n = f32::from_bits(
            u32::try_from(float_bits(input)?)
                .map_err(|_| error(ArithmeticErrorKind::InvalidValue, "Invalid Float32 bits"))?,
        );
        return budget.finish(single_result(if op == UnaryOp::Negate { -n } else { n }));
    }
    if s == Scalar::Float64 {
        let n = f64::from_bits(float_bits(input)?);
        return budget.finish(float_result(s, if op == UnaryOp::Negate { -n } else { n }));
    }
    if s == Scalar::Decimal {
        return decimal::unary(op, input, budget);
    }
    let n = integer(input, budget)?;
    integer_result(s, if op == UnaryOp::Negate { -n } else { n }, budget)
}
pub fn binary(
    op: BinaryOp,
    left: &Value,
    right: &Value,
    budget: &mut NumericBudget,
) -> Result<Value> {
    binary_controlled(op, left, right, budget, None)
}
pub(crate) fn binary_controlled(
    op: BinaryOp,
    left: &Value,
    right: &Value,
    budget: &mut NumericBudget,
    cancelled: Option<&dyn Fn() -> bool>,
) -> Result<Value> {
    binary_inner(
        op,
        left,
        right,
        &mut NumericOperation {
            budget,
            cancelled,
            finish_result: true,
        },
    )
}
fn binary_inner(
    op: BinaryOp,
    left: &Value,
    right: &Value,
    budget: &mut NumericOperation<'_>,
) -> Result<Value> {
    budget.validate(left)?;
    budget.validate(right)?;
    binary_kernel(op, left, right, budget)
}
fn binary_kernel(
    op: BinaryOp,
    left: &Value,
    right: &Value,
    budget: &mut NumericOperation<'_>,
) -> Result<Value> {
    let mut job = binary_job::BinaryJob::new(op, left, right);
    loop {
        if let Some(value) = job.advance(budget)? {
            return Ok(value);
        }
    }
}
// The original numeric algorithms remain one shared tail, reached only after
// the same kernel admission/type checks. No second arithmetic implementation.
fn binary_numeric_kernel(
    op: BinaryOp,
    left: &Value,
    right: &Value,
    s: Scalar,
    budget: &mut NumericOperation<'_>,
) -> Result<Value> {
    if s == Scalar::Float32 {
        return float_binary!(
            op,
            f32::from_bits(
                u32::try_from(float_bits(left)?).map_err(|_| error(
                    ArithmeticErrorKind::InvalidValue,
                    "Invalid Float32 bits"
                ))?
            ),
            f32::from_bits(
                u32::try_from(float_bits(right)?).map_err(|_| error(
                    ArithmeticErrorKind::InvalidValue,
                    "Invalid Float32 bits"
                ))?
            ),
            single_result,
            budget
        );
    }
    if s == Scalar::Float64 {
        return float_binary!(
            op,
            f64::from_bits(float_bits(left)?),
            f64::from_bits(float_bits(right)?),
            |n| float_result(s, n),
            budget
        );
    }
    if s == Scalar::Decimal {
        return decimal::binary(op, left, right, budget);
    }
    let a = integer(left, budget)?;
    let b = integer(right, budget)?;
    let ad = digits(&a);
    let bd = digits(&b);
    if comparison(op) {
        budget.work(ad.max(bd))?;
        return budget.finish(boolean(compare(op, a.cmp(&b))?));
    }
    let estimate = if op == BinaryOp::Multiply {
        sum_size(ad, bd)?
    } else {
        sum_size(ad.max(bd), 1)?
    };
    budget.intermediate(estimate)?;
    budget.work(
        if matches!(
            op,
            BinaryOp::Multiply | BinaryOp::Divide | BinaryOp::Remainder
        ) {
            product_work(ad, bd)?
        } else {
            ad.max(bd)
        },
    )?;
    if matches!(op, BinaryOp::Divide | BinaryOp::Remainder) && b.is_zero() {
        return Err(error(
            ArithmeticErrorKind::DivisionByZero,
            "Integer division or remainder by zero",
        ));
    }
    let result = match op {
        BinaryOp::Add => a + b,
        BinaryOp::Subtract => a - b,
        BinaryOp::Multiply => a * b,
        BinaryOp::Divide => a / b,
        BinaryOp::Remainder => a % b,
        _ => {
            return Err(error(
                ArithmeticErrorKind::TypeMismatch,
                "Unsupported integer operator",
            ));
        }
    };
    integer_result(s, result, budget)
}

/// Total scalar order for collection sorting. Ordinary expression comparisons
/// retain their IEEE and numeric-only contracts.
pub(crate) fn sort_order_controlled(
    left: &Value,
    right: &Value,
    budget: &mut NumericBudget,
    cancelled: Option<&dyn Fn() -> bool>,
) -> Result<Ordering> {
    let mut operation = NumericOperation {
        budget,
        cancelled,
        finish_result: true,
    };
    operation.checkpoint()?;
    operation.validate(left)?;
    operation.validate(right)?;
    operation.charge(1)?;
    if left.ty != right.ty {
        return Err(error(
            ArithmeticErrorKind::TypeMismatch,
            "Sort keys require exact matching scalar types",
        ));
    }
    let kind = scalar(left)?;
    if matches!(kind, Scalar::Float32 | Scalar::Float64) {
        let nan = |value: &Value| -> Result<bool> {
            let bits = float_bits(value)?;
            Ok(if kind == Scalar::Float32 {
                f32::from_bits(u32::try_from(bits).map_err(|_| {
                    error(ArithmeticErrorKind::InvalidValue, "Invalid Float32 bits")
                })?)
                .is_nan()
            } else {
                f64::from_bits(bits).is_nan()
            })
        };
        let a = nan(left)?;
        let b = nan(right)?;
        if a || b {
            return Ok(a.cmp(&b));
        }
    }
    if kind.numeric() {
        // Reuse exact integer/Decimal and IEEE comparison authorities.
        for (op, order) in [
            (BinaryOp::Less, Ordering::Less),
            (BinaryOp::Greater, Ordering::Greater),
        ] {
            let result = binary_inner(op, left, right, &mut operation)?;
            if result.payload.as_bool() == Some(true) {
                return Ok(order);
            }
        }
        return Ok(Ordering::Equal);
    }
    match kind {
        Scalar::Bool => Ok(left.payload.as_bool().cmp(&right.payload.as_bool())),
        Scalar::Char16 => Ok(left.payload["unit"]
            .as_u64()
            .cmp(&right.payload["unit"].as_u64())),
        Scalar::DateInstant => {
            let decode = |value: &Value| {
                value.payload["milliseconds"]
                    .as_str()
                    .and_then(|v| v.parse::<i64>().ok())
                    .ok_or_else(|| error(ArithmeticErrorKind::InvalidValue, "Invalid date key"))
            };
            Ok(decode(left)?.cmp(&decode(right)?))
        }
        Scalar::Unit => Ok(Ordering::Equal),
        Scalar::String | Scalar::Bytes => {
            fn text(value: &Value, kind: Scalar) -> Result<&str> {
                let text = if kind == Scalar::String {
                    value.payload.as_str()
                } else {
                    value.payload["hex"].as_str()
                };
                text.ok_or_else(|| {
                    error(ArithmeticErrorKind::InvalidValue, "Invalid text/byte key")
                })
            }
            let a = text(left, kind)?;
            let b = text(right, kind)?;
            // Canonical uppercase hex has the same lexicographic order as bytes.
            // UTF-8 lexicographic order agrees with Unicode scalar-value order.
            for (a, b) in a.bytes().zip(b.bytes()) {
                operation.checkpoint()?;
                operation.charge(1)?;
                let order = a.cmp(&b);
                if order != Ordering::Equal {
                    return Ok(order);
                }
            }
            Ok(a.len().cmp(&b.len()))
        }
        _ => Err(error(
            ArithmeticErrorKind::TypeMismatch,
            "Unsupported scalar sort key",
        )),
    }
}

/// Language-owned phase adapter over the SAME arithmetic kernel. Inputs must
/// have completed fresh NumericValidation and its exact original fees.
pub(crate) fn unary_prevalidated(
    op: UnaryOp,
    input: &Value,
    budget: &mut NumericBudget,
    cancelled: Option<&dyn Fn() -> bool>,
) -> Result<Value> {
    unary_kernel(
        op,
        input,
        &mut NumericOperation {
            budget,
            cancelled,
            finish_result: false,
        },
    )
}

#[cfg(test)]
mod cancellation_tests {
    use super::*;
    use std::cell::Cell;
    fn decimal(coefficient: &str) -> Value {
        value(
            Scalar::Decimal,
            json!({"kind":"decimal","coefficient":coefficient,"scale":0}),
        )
    }
    #[test]
    fn decimal_factorization_checks_control_without_changing_charges() {
        let left = decimal("1");
        let right = decimal("340282366920938463463374607431768211456"); // 2^128
        let mut baseline = NumericBudget::default();
        let expected = binary(BinaryOp::Divide, &left, &right, &mut baseline).unwrap();
        let polls = Cell::new(0);
        let observe = || {
            polls.set(polls.get() + 1);
            false
        };
        let mut controlled = NumericBudget::default();
        assert_eq!(
            binary_controlled(
                BinaryOp::Divide,
                &left,
                &right,
                &mut controlled,
                Some(&observe)
            )
            .unwrap(),
            expected
        );
        assert_eq!(controlled.work_used(), baseline.work_used());
        assert_eq!(controlled.value_bytes_used(), baseline.value_bytes_used());
        assert!(polls.get() > 128, "factorization must poll inside its loop");
        for stop in 1..=polls.get() {
            let seen = Cell::new(0);
            let cancel = || {
                seen.set(seen.get() + 1);
                seen.get() == stop
            };
            let mut budget = NumericBudget::default();
            let error =
                binary_controlled(BinaryOp::Divide, &left, &right, &mut budget, Some(&cancel))
                    .unwrap_err();
            assert_eq!(error.kind, ArithmeticErrorKind::Cancelled);
            assert_eq!(seen.get(), stop);
            assert_eq!(
                budget.value_bytes_used(),
                0,
                "cancelled results are never published"
            );
            assert!(budget.work_used() <= baseline.work_used());
        }
        let limits = NumericLimits {
            max_work: baseline.work_used() - 1,
            ..NumericLimits::default()
        };
        let mut limited = NumericBudget::new(limits.clone());
        let expected_error = binary(BinaryOp::Divide, &left, &right, &mut limited).unwrap_err();
        let mut limited_controlled = NumericBudget::new(limits);
        assert_eq!(
            binary_controlled(
                BinaryOp::Divide,
                &left,
                &right,
                &mut limited_controlled,
                Some(&|| false)
            )
            .unwrap_err(),
            expected_error
        );
        assert_eq!(limited_controlled.work_used(), limited.work_used());
    }
    #[test]
    fn arithmetic_error_precedes_later_cancellation() {
        let polls = Cell::new(0);
        let observe = || {
            polls.set(polls.get() + 1);
            false
        };
        let error = binary_controlled(
            BinaryOp::Divide,
            &decimal("1"),
            &decimal("3"),
            &mut NumericBudget::default(),
            Some(&observe),
        )
        .unwrap_err();
        assert_eq!(error.kind, ArithmeticErrorKind::NonTerminatingDecimal);
        let stop = polls.get() + 1;
        polls.set(0);
        let cancel = || {
            polls.set(polls.get() + 1);
            polls.get() == stop
        };
        assert_eq!(
            binary_controlled(
                BinaryOp::Divide,
                &decimal("1"),
                &decimal("3"),
                &mut NumericBudget::default(),
                Some(&cancel)
            )
            .unwrap_err(),
            error
        );
        assert_eq!(polls.get(), stop - 1);
    }
}
