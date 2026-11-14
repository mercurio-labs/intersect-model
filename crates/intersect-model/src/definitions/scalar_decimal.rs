//! Handwritten exact decimal coefficient/scale transport and Java 17 lexical rules.
use super::{Builtin, canonical_integer, decimal_digit};
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum Wire {
    Decimal { coefficient: String, scale: i32 },
}

pub(super) fn parse(text: &str) -> Result<Value, String> {
    let negative = text.starts_with('-');
    let unsigned = text.strip_prefix(['+', '-']).unwrap_or(text);
    let (mantissa, exponent) = match unsigned.split_once(['e', 'E']) {
        Some((mantissa, exponent)) => (
            mantissa,
            Builtin::EInt
                .parse(exponent)?
                .as_i64()
                .ok_or("Invalid decimal exponent")?,
        ),
        None => (unsigned, 0),
    };
    let mut coefficient = String::new();
    let mut point = false;
    let mut fractional = 0_i64;
    for character in mantissa.chars() {
        if character == '.' && !point {
            point = true;
            continue;
        }
        coefficient.push(decimal_digit(character).ok_or("Invalid decimal significand")?);
        if point {
            fractional = fractional.checked_add(1).ok_or("Decimal scale overflow")?;
        }
    }
    if coefficient.is_empty() {
        return Err("Decimal requires significand digits".into());
    }
    if negative {
        coefficient.insert(0, '-');
    }
    let coefficient = canonical_integer(&coefficient)?;
    let scale = fractional
        .checked_sub(exponent)
        .and_then(|value| i32::try_from(value).ok())
        .ok_or("Decimal scale out of range")?;
    serde_json::to_value(Wire::Decimal { coefficient, scale }).map_err(|error| error.to_string())
}

pub(super) fn format(value: &Value) -> Result<String, String> {
    let Wire::Decimal { coefficient, scale } = serde_json::from_value(value.clone())
        .map_err(|error| format!("Invalid decimal wire: {error}"))?;
    if canonical_integer(&coefficient)? != coefficient {
        return Err("Noncanonical decimal coefficient".into());
    }
    let negative = coefficient.starts_with('-');
    let digits = coefficient.strip_prefix('-').unwrap_or(&coefficient);
    let precision = i64::try_from(digits.len()).map_err(|_| "Decimal coefficient too long")?;
    let adjusted = precision
        .checked_sub(1)
        .and_then(|value| value.checked_sub(i64::from(scale)))
        .ok_or("Decimal adjusted exponent overflow")?;
    let mut text = if negative {
        String::from("-")
    } else {
        String::new()
    };
    if scale >= 0 && adjusted >= -6 {
        let point = precision - i64::from(scale);
        if point <= 0 {
            text.push_str("0.");
            // The plain-format condition bounds this padding to five zeros.
            let padding = usize::try_from(-point).map_err(|_| "Invalid decimal padding")?;
            text.extend(std::iter::repeat_n('0', padding));
            text.push_str(digits);
        } else {
            let point = usize::try_from(point).map_err(|_| "Invalid decimal point")?;
            text.push_str(digits.get(..point).ok_or("Invalid decimal precision")?);
            if scale != 0 {
                text.push('.');
                text.push_str(digits.get(point..).ok_or("Invalid decimal fraction")?);
            }
        }
    } else {
        text.push_str(digits.get(..1).ok_or("Missing decimal coefficient")?);
        if digits.len() > 1 {
            text.push('.');
            text.push_str(digits.get(1..).ok_or("Invalid decimal coefficient")?);
        }
        if adjusted != 0 {
            text.push('E');
            if adjusted > 0 {
                text.push('+');
            }
            text.push_str(&adjusted.to_string());
        }
    }
    Ok(text)
}
