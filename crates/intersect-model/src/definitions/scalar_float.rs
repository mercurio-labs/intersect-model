//! Native IEEE value transport and Ecore floating-point lexical conversion.
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum Wire {
    Float32 { bits: String },
    Float64 { bits: String },
}

fn shape(single: bool) -> (u32, i64, u64, u64) {
    if single {
        (24, 127, 0x7f800000, 0x7fc00000)
    } else {
        (53, 1023, 0x7ff0000000000000, 0x7ff8000000000000)
    }
}

pub(super) fn parse(text: &str, single: bool) -> Result<Value, String> {
    let text = text.trim_matches(|ch: char| ch <= '\u{20}');
    let negative = text.starts_with('-');
    let unsigned = text.strip_prefix(['+', '-']).unwrap_or(text);
    let (_, _, infinity, nan) = shape(single);
    let sign = if negative {
        1_u64 << if single { 31 } else { 63 }
    } else {
        0
    };
    let bits = match unsigned {
        "NaN" => nan,
        "Infinity" => sign | infinity,
        _ => {
            let number = unsigned
                .strip_suffix(['f', 'F', 'd', 'D'])
                .unwrap_or(unsigned);
            if let Some(hex) = number
                .strip_prefix("0x")
                .or_else(|| number.strip_prefix("0X"))
            {
                sign | hex_bits(hex, single)?
            } else {
                decimal_syntax(number)?;
                // Parse at the destination precision; f64 -> f32 double rounding
                // would give the wrong answer for valid inputs near a float tie.
                let magnitude = if single {
                    u64::from(
                        number
                            .parse::<f32>()
                            .map_err(|_| "Invalid float decimal")?
                            .to_bits(),
                    )
                } else {
                    number
                        .parse::<f64>()
                        .map_err(|_| "Invalid double decimal")?
                        .to_bits()
                };
                sign | magnitude
            }
        }
    };
    let wire = if single {
        Wire::Float32 {
            bits: format!("{bits:08x}"),
        }
    } else {
        Wire::Float64 {
            bits: format!("{bits:016x}"),
        }
    };
    serde_json::to_value(wire).map_err(|error| error.to_string())
}

fn decimal_syntax(text: &str) -> Result<(), String> {
    let bytes = text.as_bytes();
    let mut index = 0;
    let mut digits = 0;
    while bytes.get(index).is_some_and(u8::is_ascii_digit) {
        index += 1;
        digits += 1;
    }
    if bytes.get(index) == Some(&b'.') {
        index += 1;
        while bytes.get(index).is_some_and(u8::is_ascii_digit) {
            index += 1;
            digits += 1;
        }
    }
    if digits == 0 {
        return Err("Float requires decimal digits".into());
    }
    if matches!(bytes.get(index), Some(b'e' | b'E')) {
        index += 1;
        if matches!(bytes.get(index), Some(b'+' | b'-')) {
            index += 1;
        }
        let start = index;
        while bytes.get(index).is_some_and(u8::is_ascii_digit) {
            index += 1;
        }
        if index == start {
            return Err("Float exponent requires decimal digits".into());
        }
    }
    if index != bytes.len() {
        return Err("Invalid floating-point decimal syntax".into());
    }
    Ok(())
}

fn exponent(text: &str) -> Result<i64, String> {
    let negative = text.starts_with('-');
    let digits = text.strip_prefix(['+', '-']).unwrap_or(text);
    if digits.is_empty() || !digits.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err("Invalid hexadecimal float exponent".into());
    }
    let mut value = 0_i64;
    for digit in digits.bytes() {
        value = value
            .saturating_mul(10)
            .saturating_add(i64::from(digit - b'0'));
    }
    Ok(if negative { -value } else { value })
}

fn hex_bits(text: &str, single: bool) -> Result<u64, String> {
    let (mantissa, power) = text
        .split_once(['p', 'P'])
        .ok_or("Hexadecimal float requires a binary exponent")?;
    let power = exponent(power)?;
    let mut bits = Vec::new();
    let mut dot = false;
    let mut fractional_digits = 0_i64;
    for byte in mantissa.bytes() {
        if byte == b'.' && !dot {
            dot = true;
            continue;
        }
        let digit = match byte {
            b'0'..=b'9' => byte - b'0',
            b'a'..=b'f' => byte - b'a' + 10,
            b'A'..=b'F' => byte - b'A' + 10,
            _ => return Err("Invalid hexadecimal float significand".into()),
        };
        if dot {
            fractional_digits = fractional_digits.saturating_add(1);
        }
        bits.extend((0..4).rev().map(|shift| (digit >> shift) & 1));
    }
    if bits.is_empty() {
        return Err("Hexadecimal float requires digits".into());
    }
    let Some(first) = bits.iter().position(|bit| *bit != 0) else {
        return Ok(0);
    };
    let bits = &bits[first..];
    let length = i64::try_from(bits.len()).map_err(|_| "Float significand too long")?;
    let exponent = power.saturating_sub(fractional_digits.saturating_mul(4));
    let mut top = exponent.saturating_add(length - 1);
    let (precision, bias, infinity, _) = shape(single);
    let minimum = 1 - bias;
    let smallest = minimum - i64::from(precision - 1);
    if top > bias {
        return Ok(infinity);
    }
    if top < smallest - 1 {
        return Ok(0);
    }
    let normal = top >= minimum;
    let unit = if normal {
        top - i64::from(precision - 1)
    } else {
        smallest
    };
    let kept = length.saturating_add(exponent).saturating_sub(unit);
    let kept = usize::try_from(kept).map_err(|_| "Invalid float rounding position")?;
    let mut significand = 0_u64;
    for index in 0..kept {
        significand = (significand << 1) | u64::from(bits.get(index).copied().unwrap_or(0));
    }
    let guard = bits.get(kept).copied().unwrap_or(0) != 0;
    let sticky = bits
        .get(kept + 1..)
        .is_some_and(|tail| tail.iter().any(|bit| *bit != 0));
    if guard && (sticky || significand & 1 != 0) {
        significand += 1;
    }
    if !normal {
        return Ok(significand);
    }
    if significand == 1_u64 << precision {
        significand >>= 1;
        top += 1;
    }
    if top > bias {
        return Ok(infinity);
    }
    let biased = u64::try_from(top + bias).map_err(|_| "Invalid float exponent")?;
    Ok((biased << (precision - 1)) | (significand - (1_u64 << (precision - 1))))
}

/// Validate the exact IEEE transport, including all NaN payloads. Ecore formatting
/// applies its additional canonical-NaN policy after this common wire check.
pub(super) fn exact_bits(value: &Value, single: bool) -> Result<u64, String> {
    let wire: Wire = serde_json::from_value(value.clone())
        .map_err(|error| format!("Invalid float wire value: {error}"))?;
    let text = match (wire, single) {
        (Wire::Float32 { bits }, true) | (Wire::Float64 { bits }, false) => bits,
        _ => return Err("Float wire precision mismatch".into()),
    };
    if text.len() != if single { 8 } else { 16 }
        || !text
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err("Float bits require fixed-width lowercase hexadecimal".into());
    }
    u64::from_str_radix(&text, 16).map_err(|_| "Invalid float bits".into())
}

pub(super) fn format(value: &Value, single: bool) -> Result<String, String> {
    let bits = exact_bits(value, single)?;
    let (precision, bias, infinity, nan) = shape(single);
    let negative = bits >> if single { 31 } else { 63 } != 0;
    let sign = if negative { "-" } else { "" };
    let fraction = bits & ((1_u64 << (precision - 1)) - 1);
    let exponent = bits & infinity;
    if exponent == infinity {
        if fraction == 0 {
            return Ok(format!("{sign}Infinity"));
        }
        return if bits == nan {
            Ok("NaN".into())
        } else {
            Err("Noncanonical NaN bits".into())
        };
    }
    if exponent == 0 && fraction == 0 {
        return Ok(format!("{sign}0x0.0p0"));
    }
    let digits = if single { 6 } else { 13 };
    let aligned = fraction << (digits * 4 - (precision - 1));
    let hex = format!("{aligned:0width$x}", width = digits as usize);
    let hex = hex.trim_end_matches('0');
    let hex = if hex.is_empty() { "0" } else { hex };
    let (leading, power) = if exponent == 0 {
        (0, 1 - bias)
    } else {
        (1, (exponent >> (precision - 1)) as i64 - bias)
    };
    Ok(format!("{sign}0x{leading}.{hex}p{power}"))
}
