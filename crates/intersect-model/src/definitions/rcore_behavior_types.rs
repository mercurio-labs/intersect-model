//! Handwritten S03 type domains. This is not an executable IR or a type checker.
use super::ast::Literal;
use crate::definitions::resources::DeclarationId;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum Scalar {
    Bool,
    String,
    Bytes,
    Char16,
    DateInstant,
    Unit,
    Int8,
    Int16,
    Int32,
    Int64,
    UInt8,
    UInt16,
    UInt32,
    UInt64,
    BigInt,
    Decimal,
    Float32,
    Float64,
}
impl Scalar {
    pub(crate) fn literal_key(self, literal: &Literal) -> Result<String, String> {
        self.check_literal(literal)?;
        match literal {
            Literal::Boolean(value) => Ok(value.to_string()),
            Literal::String(value) => Ok(value.clone()),
            Literal::Number(text) => match self {
                Self::Float32 => {
                    let value = text.parse::<f32>().map_err(|_| "Invalid Float32")?;
                    Ok(if value == 0.0 { 0 } else { value.to_bits() }.to_string())
                }
                Self::Float64 => {
                    let value = text.parse::<f64>().map_err(|_| "Invalid Float64")?;
                    Ok(if value == 0.0 { 0 } else { value.to_bits() }.to_string())
                }
                _ => Ok(exact_key(text)),
            },
            _ => Err("Not a scalar literal pattern".into()),
        }
    }
    pub fn named(name: &str) -> Option<Self> {
        Some(match name {
            "Bool" => Self::Bool,
            "String" => Self::String,
            "Bytes" => Self::Bytes,
            "Char16" => Self::Char16,
            "DateInstant" => Self::DateInstant,
            "Unit" => Self::Unit,
            "Int8" => Self::Int8,
            "Int16" => Self::Int16,
            "Int32" => Self::Int32,
            "Int" | "Int64" => Self::Int64,
            "UInt8" => Self::UInt8,
            "UInt16" => Self::UInt16,
            "UInt32" => Self::UInt32,
            "UInt64" => Self::UInt64,
            "BigInt" => Self::BigInt,
            "Decimal" => Self::Decimal,
            "Float32" => Self::Float32,
            "Float64" => Self::Float64,
            _ => return None,
        })
    }

    pub fn numeric(self) -> bool {
        !matches!(
            self,
            Self::Bool | Self::String | Self::Bytes | Self::Char16 | Self::DateInstant | Self::Unit
        )
    }

    /// Check an exact literal without allocating exponent-sized integers.
    /// A unary minus may be folded into the number spelling by the checker.
    pub fn check_literal(self, literal: &Literal) -> Result<(), String> {
        match (self, literal) {
            (Self::Bool, Literal::Boolean(_)) | (Self::String, Literal::String(_)) => Ok(()),
            (scalar, Literal::Number(text)) if scalar.numeric() => scalar.number(text),
            _ => Err(format!("Literal does not have scalar type {self:?}")),
        }
    }

    fn number(self, text: &str) -> Result<(), String> {
        let number = ExactNumber::parse(text)?;
        match self {
            Self::Decimal => Ok(()),
            Self::Float32 => {
                let value = text.parse::<f32>().map_err(|_| "Invalid Float32 literal")?;
                finite_float(value.is_finite(), value == 0.0, number.zero(), self)
            }
            Self::Float64 => {
                let value = text.parse::<f64>().map_err(|_| "Invalid Float64 literal")?;
                finite_float(value.is_finite(), value == 0.0, number.zero(), self)
            }
            _ => {
                if !number.zero() && number.zeros < 0 {
                    return Err(format!(
                        "Fractional literal is not an integer of type {self:?}"
                    ));
                }
                if self == Self::BigInt || number.zero() {
                    return Ok(());
                }
                let (positive, negative) = match self {
                    Self::Int8 => ("127", Some("128")),
                    Self::Int16 => ("32767", Some("32768")),
                    Self::Int32 => ("2147483647", Some("2147483648")),
                    Self::Int64 => ("9223372036854775807", Some("9223372036854775808")),
                    Self::UInt8 => ("255", None),
                    Self::UInt16 => ("65535", None),
                    Self::UInt32 => ("4294967295", None),
                    Self::UInt64 => ("18446744073709551615", None),
                    _ => return Err(format!("Type {self:?} is not numeric")),
                };
                let limit = if number.negative {
                    negative.ok_or_else(|| format!("Negative literal is outside {self:?}"))?
                } else {
                    positive
                };
                if number.exceeds(limit) {
                    Err(format!("Integer literal is outside {self:?}"))
                } else {
                    Ok(())
                }
            }
        }
    }
}

// Canonical decimal equality uses an unbounded exponent spelling. The saturated
// exponent used for range checking must never become a value identity.
fn exact_key(text: &str) -> String {
    let (mantissa, exponent) = text.split_once(['e', 'E']).unwrap_or((text, "0"));
    let negative = mantissa.starts_with('-');
    let mantissa = mantissa.trim_start_matches('-');
    let fraction = mantissa.split_once('.').map_or(0, |(_, tail)| tail.len());
    let digits: String = mantissa.chars().filter(|&c| c != '.').collect();
    let significant = digits.trim_matches('0');
    if significant.is_empty() {
        return "0".into();
    }
    let trailing = digits.len() - digits.trim_end_matches('0').len();
    let offset = trailing as i128 - fraction as i128;
    let exponent = signed_decimal_add(exponent, offset);
    format!(
        "{}{significant}e{exponent}",
        if negative { "-" } else { "" }
    )
}
fn signed_decimal_add(left: &str, right: i128) -> String {
    let left_negative = left.starts_with('-');
    let left = left.trim_start_matches(['-', '+']).trim_start_matches('0');
    let left = if left.is_empty() { "0" } else { left };
    let right_negative = right < 0;
    let right = right.unsigned_abs().to_string();
    let (negative, digits) = if left_negative == right_negative {
        (left_negative, decimal_magnitude(left, &right, false))
    } else if (left.len(), left) >= (right.len(), right.as_str()) {
        (left_negative, decimal_magnitude(left, &right, true))
    } else {
        (right_negative, decimal_magnitude(&right, left, true))
    };
    if negative && digits != "0" {
        format!("-{digits}")
    } else {
        digits
    }
}
fn decimal_magnitude(left: &str, right: &str, subtract: bool) -> String {
    let mut right = right.bytes().rev();
    let mut carry = 0i16;
    let mut out = Vec::new();
    for a in left.bytes().rev() {
        let b = i16::from(right.next().unwrap_or(b'0') - b'0');
        let n = i16::from(a - b'0') + if subtract { -b } else { b } + carry;
        carry = n.div_euclid(10);
        out.push((n.rem_euclid(10) as u8 + b'0') as char);
    }
    for b in right {
        let n = i16::from(b - b'0') + carry;
        carry = n / 10;
        out.push(((n % 10) as u8 + b'0') as char);
    }
    if carry > 0 {
        out.push((carry as u8 + b'0') as char);
    }
    while out.len() > 1 && out.last() == Some(&'0') {
        out.pop();
    }
    out.into_iter().rev().collect()
}

fn finite_float(finite: bool, zero: bool, exact_zero: bool, ty: Scalar) -> Result<(), String> {
    if !finite {
        Err(format!("Literal overflows {ty:?}"))
    } else if zero && !exact_zero {
        Err(format!("Nonzero literal underflows {ty:?}"))
    } else {
        Ok(())
    }
}

/// Normalized significant decimal digits and a symbolic count of trailing zeros.
/// Saturation is relative to source length, sufficient for integer/range checks;
/// this structure is never used to encode or evaluate the original number.
struct ExactNumber {
    negative: bool,
    digits: Vec<u8>,
    zeros: i128,
}
impl ExactNumber {
    fn parse(text: &str) -> Result<Self, String> {
        let bytes = text.as_bytes();
        let negative = bytes.first() == Some(&b'-');
        let mut pos = usize::from(negative);
        let start = pos;
        while bytes.get(pos).is_some_and(u8::is_ascii_digit) {
            pos += 1;
        }
        if pos == start {
            return Err("Invalid numeric literal: expected integer digits".into());
        }
        let mut digits = bytes[start..pos].to_vec();
        let mut fraction = 0;
        if bytes.get(pos) == Some(&b'.') {
            pos += 1;
            let start = pos;
            while bytes.get(pos).is_some_and(u8::is_ascii_digit) {
                pos += 1;
            }
            fraction = pos - start;
            if fraction == 0 {
                return Err("Invalid numeric literal: expected fraction digits".into());
            }
            digits.extend_from_slice(&bytes[start..pos]);
        }
        let mut exponent = 0i128;
        if matches!(bytes.get(pos), Some(b'e' | b'E')) {
            pos += 1;
            let minus = bytes.get(pos) == Some(&b'-');
            if matches!(bytes.get(pos), Some(b'+' | b'-')) {
                pos += 1;
            }
            let start = pos;
            // Larger than any fraction/significand in this input, with room for
            // all fixed-width and float exponent boundaries. No giant expansion.
            let cap = bytes.len() as i128 + 1024;
            while let Some(digit) = bytes.get(pos).filter(|b| b.is_ascii_digit()) {
                exponent = (exponent * 10 + i128::from(*digit - b'0')).min(cap);
                pos += 1;
            }
            if start == pos {
                return Err("Invalid numeric literal: expected exponent digits".into());
            }
            if minus {
                exponent = -exponent;
            }
        }
        if pos != bytes.len() {
            return Err("Invalid numeric literal: trailing characters".into());
        }
        let mut zeros = exponent - fraction as i128;
        while digits.last() == Some(&b'0') {
            digits.pop();
            zeros += 1;
        }
        let leading = digits.iter().take_while(|&&b| b == b'0').count();
        digits.drain(..leading);
        Ok(Self {
            negative,
            digits,
            zeros,
        })
    }
    fn zero(&self) -> bool {
        self.digits.is_empty()
    }
    fn exceeds(&self, limit: &str) -> bool {
        let length = self.digits.len() as i128 + self.zeros;
        if length != limit.len() as i128 {
            return length > limit.len() as i128;
        }
        self.digits
            .iter()
            .copied()
            .chain(std::iter::repeat(b'0'))
            .take(limit.len())
            .cmp(limit.bytes())
            .is_gt()
    }
}

/// References are local to a TypeTable, never a persisted executable identity.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub struct TypeRef(pub usize);

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub enum CollectionKind {
    Sequence,
    Set,
    OrderedSet,
    Bag,
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub enum Type {
    Collection {
        kind: CollectionKind,
        element: TypeRef,
    },
    Tuple(Vec<TypeRef>),
    Record(BTreeMap<String, TypeRef>),
    Parameter {
        symbol: super::modules::SymbolId,
        ordinal: usize,
    },
    Scalar(Scalar),
    Model(DeclarationId),
    /// Exact shared Ecore application expression; use requires source/bound validation.
    ModelApplication(crate::definitions::generic_types::Type),
    /// Source-checked symbolic application; never a RuntimeType certificate.
    SymbolicModelApplication {
        declaration: DeclarationId,
        arguments: Vec<TypeRef>,
    },
    /// Source-checked symbolic datatype; never a ClosedType/value-codec certificate.
    SymbolicDatatypeApplication {
        declaration: DeclarationId,
        arguments: Vec<TypeRef>,
    },
    Enumeration(DeclarationId),
    Datatype(DeclarationId),
    /// Exact closed Ecore datatype application; checked providers remain explicit.
    DatatypeApplication(crate::definitions::generic_types::Type),
    Option(TypeRef),
    Result {
        ok: TypeRef,
        error: TypeRef,
    },
    Function {
        parameters: Vec<TypeRef>,
        result: TypeRef,
    },
    /// Structural multiplicity only; collection algebra belongs to S04/S08.
    Many {
        element: TypeRef,
        lower: usize,
        upper: i32,
        ordered: bool,
        unique: bool,
    },
}

/// Canonical, flat, acyclic types. No deserialization bypasses checked insertion.
#[derive(Clone, Debug, Default, Serialize)]
pub struct TypeTable {
    types: Vec<Type>,
    #[serde(skip)]
    lookup: BTreeMap<Type, TypeRef>,
    #[serde(skip)]
    depths: Vec<usize>,
}
impl TypeTable {
    pub fn get(&self, ty: TypeRef) -> Result<&Type, String> {
        self.types
            .get(ty.0)
            .ok_or_else(|| "Unknown type reference".into())
    }
    pub fn intern(&mut self, ty: Type) -> Result<TypeRef, String> {
        let ty = match ty {
            Type::Tuple(items) if items.is_empty() => Type::Scalar(Scalar::Unit),
            ty => ty,
        };
        if let Some(id) = self.lookup.get(&ty) {
            return Ok(*id);
        }
        let children = match &ty {
            Type::Scalar(_)
            | Type::Model(_)
            | Type::Enumeration(_)
            | Type::Datatype(_)
            | Type::Parameter { .. } => vec![],
            Type::SymbolicModelApplication { arguments, .. }
            | Type::SymbolicDatatypeApplication { arguments, .. } => arguments.clone(),
            Type::Collection { element, .. } => vec![*element],
            Type::Tuple(items) => items.clone(),
            Type::Record(fields) => fields.values().copied().collect(),
            Type::DatatypeApplication(expression) => {
                crate::definitions::generic_types::ClosedType::new(expression.clone())?;
                vec![]
            }
            Type::ModelApplication(expression) => {
                crate::definitions::generic_types::RuntimeType::new(expression.clone())?;
                vec![]
            }
            Type::Option(inner) => vec![*inner],
            Type::Result { ok, error } => vec![*ok, *error],
            Type::Function { parameters, result } => {
                let mut children = parameters.clone();
                children.push(*result);
                children
            }
            Type::Many {
                element,
                lower,
                upper,
                ..
            } => {
                if *upper < -1
                    || *upper == 1
                    || *upper == 0
                    || (*upper >= 0 && *lower > *upper as usize)
                {
                    return Err("Invalid many-valued feature multiplicity".into());
                }
                vec![*element]
            }
        };
        let mut depth = 1;
        for child in children {
            let child_depth = self
                .depths
                .get(child.0)
                .ok_or("Unknown child type reference")?;
            depth = depth.max(child_depth + 1);
        }
        if depth > 64 {
            return Err("Type nesting exceeds 64 levels".into());
        }
        let id = TypeRef(self.types.len());
        self.lookup.insert(ty.clone(), id);
        self.types.push(ty);
        self.depths.push(depth);
        Ok(id)
    }
}
