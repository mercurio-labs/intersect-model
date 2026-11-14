//! Handwritten shared Ecore scalar codecs. No host/Java numeric conversions.
use serde::{Deserialize, Serialize};
use serde_json::Value;
#[path = "scalar_binary.rs"]
mod binary;
pub(crate) use binary::ByteValidation;
#[path = "scalar_date.rs"]
pub mod date;
#[path = "scalar_decimal.rs"]
mod decimal;
#[path = "scalar_float.rs"]
mod floating;
#[path = "scalar_unicode.rs"]
mod unicode;

fn boolean_case_equal(text: &str, expected: &str) -> bool {
    text.chars()
        .map(|character| {
            unicode::BOOLEAN_CASE_EQUIVALENTS
                .iter()
                .find(|(source, _)| *source == character)
                .map(|(_, target)| *target)
                .unwrap_or_else(|| character.to_ascii_lowercase())
        })
        .eq(expected.chars())
}

fn decimal_digit(character: char) -> Option<char> {
    let code = u32::from(character);
    unicode::DECIMAL_ZEROES.iter().find_map(|zero| {
        code.checked_sub(*zero)
            .filter(|digit| *digit < 10)
            .and_then(|digit| char::from_u32(u32::from(b'0') + digit))
    })
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Builtin {
    EDate,
    EByteArray,
    EChar,
    ECharacterObject,
    EBigDecimal,
    EBigInteger,
    EString,
    EBoolean,
    EByte,
    EShort,
    EInt,
    ELong,
    EBooleanObject,
    EByteObject,
    EShortObject,
    EIntegerObject,
    ELongObject,
    EFloat,
    EDouble,
    EFloatObject,
    EDoubleObject,
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum ExactInteger {
    Int64 { value: String },
}

/// Canonical, lossless transport for an already typed Rust signed integer.
/// Untrusted wire values must still pass `Builtin::ELong` validation.
pub fn int64_wire(value: i64) -> Value {
    serde_json::json!({"kind": "int64", "value": value.to_string()})
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum ArbitraryInteger {
    Bigint { value: String },
}

fn canonical_integer(text: &str) -> Result<String, String> {
    let digits = text.strip_prefix(['+', '-']).unwrap_or(text);
    if digits.is_empty() {
        return Err("Integer requires decimal digits".into());
    }
    let normalized: String = digits
        .chars()
        .map(decimal_digit)
        .collect::<Option<_>>()
        .ok_or("Integer requires Unicode 13 BMP decimal digits")?;
    let significant = normalized.trim_start_matches('0');
    if significant.is_empty() {
        return Ok("0".into());
    }
    Ok(if text.starts_with('-') {
        format!("-{significant}")
    } else {
        significant.into()
    })
}

impl Builtin {
    pub fn named(name: &str) -> Option<Self> {
        Some(match name {
            "EDate" => Self::EDate,
            "EByteArray" => Self::EByteArray,
            "EChar" => Self::EChar,
            "ECharacterObject" => Self::ECharacterObject,
            "EBigDecimal" => Self::EBigDecimal,
            "EBigInteger" => Self::EBigInteger,
            "EString" => Self::EString,
            "EBoolean" => Self::EBoolean,
            "EByte" => Self::EByte,
            "EShort" => Self::EShort,
            "EInt" => Self::EInt,
            "ELong" => Self::ELong,
            "EBooleanObject" => Self::EBooleanObject,
            "EByteObject" => Self::EByteObject,
            "EShortObject" => Self::EShortObject,
            "EIntegerObject" => Self::EIntegerObject,
            "ELongObject" => Self::ELongObject,
            "EFloat" => Self::EFloat,
            "EDouble" => Self::EDouble,
            "EFloatObject" => Self::EFloatObject,
            "EDoubleObject" => Self::EDoubleObject,
            _ => return None,
        })
    }
    pub fn nullable(self) -> bool {
        matches!(
            self,
            Self::EString
                | Self::EDate
                | Self::EByteArray
                | Self::ECharacterObject
                | Self::EBigInteger
                | Self::EBigDecimal
                | Self::EBooleanObject
                | Self::EByteObject
                | Self::EShortObject
                | Self::EIntegerObject
                | Self::ELongObject
                | Self::EFloatObject
                | Self::EDoubleObject
        )
    }
    fn integer_bounds(self) -> Option<(i64, i64)> {
        match self {
            Self::EByte | Self::EByteObject => Some((i8::MIN.into(), i8::MAX.into())),
            Self::EShort | Self::EShortObject => Some((i16::MIN.into(), i16::MAX.into())),
            Self::EInt | Self::EIntegerObject => Some((i32::MIN.into(), i32::MAX.into())),
            Self::ELong | Self::ELongObject => Some((i64::MIN, i64::MAX)),
            _ => None,
        }
    }
    pub fn default_value(self) -> Value {
        if self.nullable() {
            return Value::Null;
        }
        match self {
            Self::EBoolean => Value::Bool(false),
            Self::EChar => serde_json::json!({"kind":"char16", "unit":0}),
            Self::ELong => int64_wire(0),
            Self::EFloat => serde_json::json!({"kind": "float32", "bits": "00000000"}),
            Self::EDouble => serde_json::json!({"kind": "float64", "bits": "0000000000000000"}),
            _ => Value::from(0),
        }
    }
    pub fn parse(self, text: &str) -> Result<Value, String> {
        match self {
            Self::EString => return Ok(Value::String(text.into())),
            Self::EDate => return date::parse(text),
            Self::EByteArray => return binary::parse_bytes(text),
            Self::EChar | Self::ECharacterObject => return binary::parse_char(text),
            Self::EBigDecimal => return decimal::parse(text),
            Self::EBigInteger => {
                return serde_json::to_value(ArbitraryInteger::Bigint {
                    value: canonical_integer(text)?,
                })
                .map_err(|error| error.to_string());
            }
            Self::EFloat | Self::EFloatObject => return floating::parse(text, true),
            Self::EDouble | Self::EDoubleObject => return floating::parse(text, false),
            Self::EBoolean | Self::EBooleanObject => {
                return if boolean_case_equal(text, "true") {
                    Ok(Value::Bool(true))
                } else if boolean_case_equal(text, "false") {
                    Ok(Value::Bool(false))
                } else {
                    Err(format!("Invalid {self:?} lexical value {text:?}"))
                };
            }
            _ => {}
        }
        let (minimum, maximum) = self.integer_bounds().ok_or("Unsupported integer domain")?;
        let digits = text.strip_prefix(['+', '-']).unwrap_or(text);
        let normalized: Option<String> = digits.chars().map(decimal_digit).collect();
        if digits.is_empty() || normalized.is_none() {
            return Err(format!(
                "Invalid {self:?} decimal lexical value {text:?}; Unicode 13 BMP decimal digits required"
            ));
        }
        let normalized = normalized.ok_or("Invalid decimal digits")?;
        let signed = if text.starts_with('-') {
            format!("-{normalized}")
        } else {
            normalized
        };
        let value = signed
            .parse::<i64>()
            .map_err(|_| format!("{self:?} integer overflow: {text:?}"))?;
        if !(minimum..=maximum).contains(&value) {
            return Err(format!("{self:?} integer out of range: {text:?}"));
        }
        if matches!(self, Self::ELong | Self::ELongObject) {
            Ok(int64_wire(value))
        } else {
            Ok(Value::from(value))
        }
    }
    /// Format present values canonically. None is nullable absence, not empty text.
    pub fn format(self, value: &Value) -> Result<Option<String>, String> {
        if value.is_null() {
            return if self.nullable() {
                Ok(None)
            } else {
                Err(format!("Null is not a {self:?} value"))
            };
        }
        let invalid = || format!("Invalid {self:?} wire value");
        match self {
            Self::EDate => return date::format(value).map(Some),
            Self::EByteArray => return binary::format_bytes(value).map(Some),
            Self::EChar | Self::ECharacterObject => return binary::format_char(value).map(Some),
            Self::EBigDecimal => return decimal::format(value).map(Some),
            Self::EBigInteger => {
                let ArbitraryInteger::Bigint { value } =
                    serde_json::from_value(value.clone()).map_err(|_| invalid())?;
                if canonical_integer(&value)? != value {
                    return Err("Noncanonical bigint wire decimal".into());
                }
                return Ok(Some(value));
            }
            Self::EFloat | Self::EFloatObject => return floating::format(value, true).map(Some),
            Self::EDouble | Self::EDoubleObject => return floating::format(value, false).map(Some),
            Self::EString => {
                return string_wire(value).map(|value| Some(value.to_owned()));
            }
            Self::EBoolean | Self::EBooleanObject => {
                return value
                    .as_bool()
                    .map(|value| Some(value.to_string()))
                    .ok_or_else(invalid);
            }
            Self::ELong | Self::ELongObject => {
                let ExactInteger::Int64 { value } =
                    serde_json::from_value(value.clone()).map_err(|_| invalid())?;
                let integer = value.parse::<i64>().map_err(|_| invalid())?;
                if value != integer.to_string() {
                    return Err("Noncanonical int64 wire decimal".into());
                }
                return Ok(Some(value));
            }
            _ => {}
        }
        let (minimum, maximum) = self.integer_bounds().ok_or_else(invalid)?;
        let value = value.as_i64().ok_or_else(invalid)?;
        if !(minimum..=maximum).contains(&value) {
            return Err(invalid());
        }
        Ok(Some(value.to_string()))
    }
    pub fn validate(self, value: &Value) -> Result<(), String> {
        // EString validation only inspects the wire kind. Keep formatting's
        // ownership copy for callers that actually request lexical output.
        // Null still uses the shared nullable/primitive policy in format.
        if self == Self::EString && !value.is_null() {
            return string_wire(value).map(|_| ());
        }
        if self == Self::EByteArray && !value.is_null() {
            return binary::validate_bytes(value);
        }
        self.format(value).map(|_| ())
    }
    /// Exact IEEE bit transport for consumers whose input domain preserves NaN
    /// payloads. This does not change Ecore's canonical storage/formatting policy.
    pub fn validate_ieee_bits(self, value: &Value) -> Result<(), String> {
        match self {
            Self::EFloat | Self::EFloatObject => floating::exact_bits(value, true).map(|_| ()),
            Self::EDouble | Self::EDoubleObject => floating::exact_bits(value, false).map(|_| ()),
            _ => Err("Exact IEEE bits require a floating-point type".into()),
        }
    }
}

// Shared wire predicate for formatting and validation; borrowing does not
// allocate or normalize an already decoded EString payload.
fn string_wire(value: &Value) -> Result<&str, String> {
    value
        .as_str()
        .ok_or_else(|| format!("Invalid {:?} wire value", Builtin::EString))
}
