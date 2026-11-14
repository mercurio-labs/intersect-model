//! Shared scalar payload validation and bounded source literal normalization.
use super::{Value, ValueLimits, ValueType};
use crate::definitions::{
    rcore_behavior::{ast::Literal, types::Scalar},
    scalars::Builtin,
};
use serde_json::json;

#[path = "rcore_behavior_scalar_size.rs"]
mod size;
pub(super) use size::{Scratch as ScalarScratch, SizeCursor};

pub(super) fn check_size(payload: &serde_json::Value, limits: &ValueLimits) -> Result<(), String> {
    let mut cursor = SizeCursor::new(payload, limits.max_value_bytes);
    while !cursor.advance()? {}
    Ok(())
}

pub fn validate_scalar_value(value: &Value, limits: &ValueLimits) -> Result<(), String> {
    let mut cursor = super::scalar_validation::ScalarValidation::scalar(value, limits);
    while !cursor.advance(limits).map_err(|e| e.message)? {}
    Ok(())
}

pub(super) fn begin_scalar_tail<'a>(
    value: super::scalar_validation::ScalarSource<'a>,
    limits: &ValueLimits,
) -> Result<Option<crate::definitions::scalars::ByteValidation<'a>>, String> {
    let Some(scalar) = value.kind else {
        return Err("Expected scalar value".into());
    };
    if value.payload.is_null() {
        return Err("Null is not a scalar value".into());
    }
    if matches!(scalar, Scalar::BigInt | Scalar::Decimal) {
        let name = if scalar == Scalar::BigInt {
            "value"
        } else {
            "coefficient"
        };
        let digits = value
            .payload
            .get(name)
            .and_then(serde_json::Value::as_str)
            .ok_or("Missing exact coefficient")?;
        if digits.trim_start_matches('-').len() > limits.max_digits {
            return Err("Scalar digit limit exceeded".into());
        }
    }
    let builtin = match scalar {
        Scalar::Bool => Builtin::EBoolean,
        Scalar::String => Builtin::EString,
        Scalar::Bytes => Builtin::EByteArray,
        Scalar::Char16 => Builtin::EChar,
        Scalar::DateInstant => Builtin::EDate,
        Scalar::Int8 => Builtin::EByte,
        Scalar::Int16 => Builtin::EShort,
        Scalar::Int32 => Builtin::EInt,
        Scalar::Int64 => Builtin::ELong,
        Scalar::BigInt => Builtin::EBigInteger,
        Scalar::Decimal => Builtin::EBigDecimal,
        Scalar::Float32 => Builtin::EFloat,
        Scalar::Float64 => Builtin::EDouble,
        Scalar::Unit => {
            return if value.payload == &json!({"kind":"unit"}) {
                Ok(None)
            } else {
                Err("Invalid Unit payload".into())
            };
        }
        Scalar::UInt8 | Scalar::UInt16 | Scalar::UInt32 => {
            let number = value
                .payload
                .as_u64()
                .ok_or("Unsigned scalar requires exact integer")?;
            let max = match scalar {
                Scalar::UInt8 => u64::from(u8::MAX),
                Scalar::UInt16 => u64::from(u16::MAX),
                _ => u64::from(u32::MAX),
            };
            return if number <= max {
                Ok(None)
            } else {
                Err("Unsigned scalar outside range".into())
            };
        }
        Scalar::UInt64 => {
            let object = value.payload.as_object().ok_or("Invalid UInt64 payload")?;
            if object.len() != 2 || object.get("kind") != Some(&json!("uint64")) {
                return Err("Invalid UInt64 payload".into());
            }
            let text = object
                .get("value")
                .and_then(serde_json::Value::as_str)
                .ok_or("Missing UInt64 decimal")?;
            let number = text.parse::<u64>().map_err(|_| "Invalid UInt64 decimal")?;
            return if number.to_string() == text {
                Ok(None)
            } else {
                Err("Noncanonical UInt64 decimal".into())
            };
        }
    };
    if scalar == Scalar::Bytes {
        return crate::definitions::scalars::ByteValidation::new(value.payload).map(Some);
    }
    if matches!(scalar, Scalar::Float32 | Scalar::Float64) {
        builtin.validate_ieee_bits(value.payload).map(|()| None)
    } else {
        builtin.validate(value.payload).map(|()| None)
    }
}

pub fn literal_value(
    scalar: Scalar,
    literal: &Literal,
    limits: &ValueLimits,
) -> Result<Value, String> {
    let payload = match (scalar, literal) {
        (Scalar::Bool, Literal::Boolean(value)) => json!(value),
        (Scalar::String, Literal::String(value)) => {
            if value.len() > limits.max_value_bytes {
                return Err("Literal byte limit exceeded".into());
            }
            json!(value)
        }
        (scalar, Literal::Number(text)) if scalar.numeric() => {
            if text.len() > limits.max_value_bytes {
                return Err("Literal byte limit exceeded".into());
            }
            scalar.check_literal(literal)?;
            match scalar {
                Scalar::Float32 => Builtin::EFloat.parse(text)?,
                Scalar::Float64 => Builtin::EDouble.parse(text)?,
                Scalar::Decimal => Builtin::EBigDecimal.parse(text)?,
                _ => {
                    // Zero has no magnitude growth; source checking deliberately
                    // admits arbitrarily spelled exponents without allocating it.
                    let mantissa = text.split(['e', 'E']).next().ok_or("Missing number")?;
                    let zero = mantissa
                        .bytes()
                        .filter(u8::is_ascii_digit)
                        .all(|digit| digit == b'0');
                    let decimal = if zero {
                        json!({"kind":"decimal","coefficient":"0","scale":0})
                    } else {
                        Builtin::EBigDecimal.parse(text)?
                    };
                    let coefficient = decimal
                        .get("coefficient")
                        .and_then(serde_json::Value::as_str)
                        .ok_or("Missing coefficient")?;
                    let scale = decimal
                        .get("scale")
                        .and_then(serde_json::Value::as_i64)
                        .ok_or("Missing scale")?;
                    let negative = coefficient.starts_with('-');
                    let mut digits = coefficient.trim_start_matches('-').to_owned();
                    if digits != "0" {
                        if scale > 0 {
                            let remove =
                                usize::try_from(scale).map_err(|_| "Integer scale overflow")?;
                            let keep = digits
                                .len()
                                .checked_sub(remove)
                                .ok_or("Fractional integer")?;
                            if !digits[keep..].bytes().all(|b| b == b'0') {
                                return Err("Fractional integer".into());
                            }
                            digits.truncate(keep);
                        } else {
                            let zeros =
                                usize::try_from(-scale).map_err(|_| "Integer scale overflow")?;
                            let size = digits
                                .len()
                                .checked_add(zeros)
                                .ok_or("Integer digit limit exceeded")?;
                            if size > limits.max_digits {
                                return Err("Integer digit limit exceeded".into());
                            }
                            digits.extend(std::iter::repeat_n('0', zeros));
                        }
                    }
                    if digits.len() > limits.max_digits {
                        return Err("Integer digit limit exceeded".into());
                    }
                    if negative && digits != "0" {
                        digits.insert(0, '-');
                    }
                    match scalar {
                        Scalar::Int64 => Builtin::ELong.parse(&digits)?,
                        Scalar::BigInt => Builtin::EBigInteger.parse(&digits)?,
                        Scalar::UInt64 => json!({"kind":"uint64","value":digits}),
                        Scalar::UInt8 | Scalar::UInt16 | Scalar::UInt32 => json!(
                            digits
                                .parse::<u64>()
                                .map_err(|_| "Invalid unsigned integer")?
                        ),
                        _ => json!(
                            digits
                                .parse::<i64>()
                                .map_err(|_| "Invalid signed integer")?
                        ),
                    }
                }
            }
        }
        _ => return Err("Unsupported scalar source literal".into()),
    };
    let value = Value {
        ty: ValueType::Scalar(scalar),
        payload,
    };
    validate_scalar_value(&value, limits)?;
    Ok(value)
}
