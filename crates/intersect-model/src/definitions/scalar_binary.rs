//! Handwritten Ecore byte-array and UTF-16 code-unit conversion.
use super::Builtin;
use serde::{Deserialize, Serialize, de};
use serde_json::Value;

#[derive(Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum Wire<Hex = String> {
    Bytes { hex: Hex },
    Char16 { unit: u16 },
}

pub(super) fn parse_bytes(text: &str) -> Result<Value, String> {
    if !text.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err("Invalid hexadecimal byte array".into());
    }
    let mut hex = text.to_ascii_uppercase();
    if !hex.len().is_multiple_of(2) {
        hex.insert(hex.len() - 1, '0');
    }
    serde_json::to_value(Wire::Bytes { hex }).map_err(|error| error.to_string())
}

// The official Wire enum owns the schema. This primitive visitor has exactly
// String's expected type while retaining the immutable decoded JSON text.
struct TextView<'a>(&'a str);
impl<'de> Deserialize<'de> for TextView<'de> {
    fn deserialize<D: de::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct Visitor;
        impl<'de> de::Visitor<'de> for Visitor {
            type Value = TextView<'de>;
            fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str("a string")
            }
            fn visit_borrowed_str<E: de::Error>(self, value: &'de str) -> Result<Self::Value, E> {
                Ok(TextView(value))
            }
        }
        deserializer.deserialize_str(Visitor)
    }
}

/// One shared canonical-byte predicate. The source is the actual immutable
/// decoded wire, never a serialized certificate or a normalized replacement.
pub(crate) struct ByteValidation<'a> {
    hex: &'a str,
    offset: usize,
}
impl<'a> ByteValidation<'a> {
    pub(crate) fn new(value: &'a Value) -> Result<Self, String> {
        let parsed = Wire::<TextView<'a>>::deserialize(value)
            .map_err(|error| format!("Invalid byte wire: {error}"))?;
        match parsed {
            Wire::Bytes { hex: TextView(hex) } if hex.len() % 2 == 0 => Ok(Self { hex, offset: 0 }),
            _ => Err("Expected canonical uppercase even-length byte hex".into()),
        }
    }
    #[cfg(feature = "allocation-profile")]
    pub(crate) fn progress(&self) -> (usize, usize) {
        (self.offset, self.hex.len())
    }
    pub(crate) fn advance(&mut self) -> Result<bool, String> {
        let end = crate::definitions::bundle::input_chunk_end(self.hex, self.offset);
        let chunk = self
            .hex
            .get(self.offset..end)
            .ok_or("Invalid byte validation boundary")?;
        if !chunk
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'A'..=b'F').contains(&byte))
        {
            return Err("Expected canonical uppercase even-length byte hex".into());
        }
        self.offset = end;
        Ok(end == self.hex.len())
    }
    pub(crate) fn finish(self) -> Result<&'a str, String> {
        if self.offset != self.hex.len() {
            return Err("Byte validation is not complete".into());
        }
        Ok(self.hex)
    }
}
pub(super) fn validate_bytes(value: &Value) -> Result<(), String> {
    let mut cursor = ByteValidation::new(value)?;
    while !cursor.advance()? {}
    cursor.finish().map(|_| ())
}
pub(super) fn format_bytes(value: &Value) -> Result<String, String> {
    let mut cursor = ByteValidation::new(value)?;
    while !cursor.advance()? {}
    cursor.finish().map(str::to_owned)
}

pub(super) fn parse_char(text: &str) -> Result<Value, String> {
    let unit = match Builtin::EInt.parse(text) {
        Ok(value) => {
            let integer = value
                .as_i64()
                .ok_or("Expected an EInt character conversion")?;
            u16::try_from(integer.rem_euclid(65536)).map_err(|_| "Invalid character unit")?
        }
        Err(_) => text
            .encode_utf16()
            .next()
            .ok_or("Character lexical value cannot be empty")?,
    };
    serde_json::to_value(Wire::<String>::Char16 { unit }).map_err(|error| error.to_string())
}

pub(super) fn format_char(value: &Value) -> Result<String, String> {
    match serde_json::from_value::<Wire>(value.clone())
        .map_err(|error| format!("Invalid character wire: {error}"))?
    {
        Wire::Char16 { unit } => Ok(unit.to_string()),
        _ => Err("Expected a UTF-16 character wire value".into()),
    }
}
