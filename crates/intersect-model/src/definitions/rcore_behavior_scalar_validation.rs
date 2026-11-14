//! Shared scalar admission/canonical stages. Ordinary facades drain this engine.
//! Native codecs other than byte hex and digit/resource metadata retain explicit
//! atomic boundaries. The official wire schema is never replaced here.
use super::{
    IrError, IrErrorKind, Value, ValueLimits, ValueType,
    values::{SizeCursor, begin_scalar_tail},
};
use crate::definitions::{
    rcore_behavior::types::Scalar, retained_json::ResourceCursor, scalars::ByteValidation,
};

pub(crate) struct ValidationIssue {
    pub(crate) message: String,
    resource: bool,
}
impl ValidationIssue {
    fn new(message: String, resource: bool) -> Self {
        Self { message, resource }
    }
    pub(crate) fn into_ir(self) -> IrError {
        IrError {
            kind: if self.resource {
                IrErrorKind::ResourceExhausted
            } else {
                IrErrorKind::Invalid
            },
            message: self.message,
            span: None,
        }
    }
}
enum Stage<'a> {
    Size {
        cursor: SizeCursor<'a>,
        resource: bool,
        validate_after: bool,
    },
    Bytes(ByteValidation<'a>),
    Done,
    Failed,
}
/// Private immutable projection of the actual scalar header and payload.
/// It never supplies executable/schema authority and cannot invoke user Borrow.
#[derive(Clone, Copy)]
pub(crate) struct ScalarSource<'a> {
    pub(crate) kind: Option<Scalar>,
    pub(crate) payload: &'a serde_json::Value,
}
impl<'a> ScalarSource<'a> {
    pub(crate) fn borrowed(value: &'a Value) -> Self {
        Self {
            kind: match &value.ty {
                ValueType::Scalar(kind) => Some(*kind),
                ValueType::Enum(_) => None,
            },
            payload: &value.payload,
        }
    }
    pub(crate) fn scalar(kind: Scalar, payload: &'a serde_json::Value) -> Self {
        Self {
            kind: Some(kind),
            payload,
        }
    }
}

pub(crate) struct ScalarValidation<'a> {
    value: ScalarSource<'a>,
    stage: Stage<'a>,
}
impl<'a> ScalarValidation<'a> {
    pub(crate) const ROOT_BYTES: usize = std::mem::size_of::<Self>();
    pub(crate) fn scalar(value: &'a Value, limits: &ValueLimits) -> Self {
        Self {
            value: ScalarSource::borrowed(value),
            stage: Stage::Size {
                cursor: SizeCursor::new(&value.payload, limits.max_value_bytes),
                resource: false,
                validate_after: false,
            },
        }
    }
    pub(crate) fn resources(value: &'a Value, limits: &ValueLimits) -> Self {
        Self {
            value: ScalarSource::borrowed(value),
            stage: Stage::Size {
                cursor: SizeCursor::new(&value.payload, limits.max_value_bytes),
                resource: true,
                validate_after: false,
            },
        }
    }
    pub(crate) fn full(value: &'a Value, limits: &ValueLimits) -> Self {
        Self::full_source(ScalarSource::borrowed(value), limits)
    }
    pub(crate) fn full_source(value: ScalarSource<'a>, limits: &ValueLimits) -> Self {
        Self {
            value,
            stage: Stage::Size {
                cursor: SizeCursor::new(value.payload, limits.max_value_bytes),
                resource: true,
                validate_after: true,
            },
        }
    }
    pub(crate) fn from_completed(
        value: &'a Value,
        limits: &ValueLimits,
        cursor: ResourceCursor<'a>,
    ) -> Result<Self, String> {
        Ok(Self {
            value: ScalarSource::borrowed(value),
            stage: Stage::Size {
                cursor: SizeCursor::from_completed(cursor, &value.payload, limits.max_value_bytes)?,
                resource: true,
                validate_after: true,
            },
        })
    }
    #[cfg(feature = "allocation-profile")]
    pub(crate) fn progress(&self) -> Option<(&'static str, usize, usize)> {
        match &self.stage {
            Stage::Size {
                cursor, resource, ..
            } => {
                let (len, cap) = cursor.retained_slots();
                Some((
                    if *resource {
                        "resource-size"
                    } else {
                        "validation-size"
                    },
                    len,
                    cap,
                ))
            }
            Stage::Bytes(cursor) => {
                let (offset, len) = cursor.progress();
                Some(("hex", offset, len))
            }
            _ => None,
        }
    }
    pub(crate) fn advance(&mut self, limits: &ValueLimits) -> Result<bool, ValidationIssue> {
        match std::mem::replace(&mut self.stage, Stage::Failed) {
            Stage::Size {
                mut cursor,
                resource,
                validate_after,
            } => {
                if !cursor
                    .advance()
                    .map_err(|e| ValidationIssue::new(e, resource))?
                {
                    self.stage = Stage::Size {
                        cursor,
                        resource,
                        validate_after,
                    };
                    return Ok(false);
                }
                if resource {
                    resource_digits(self.value, limits)
                        .map_err(|e| ValidationIssue::new(e, true))?;
                    if validate_after {
                        let old = cursor
                            .into_completed()
                            .map_err(|e| ValidationIssue::new(e, false))?;
                        let next = SizeCursor::from_completed(
                            old,
                            self.value.payload,
                            limits.max_value_bytes,
                        )
                        .map_err(|e| ValidationIssue::new(e, false))?;
                        self.stage = Stage::Size {
                            cursor: next,
                            resource: false,
                            validate_after: false,
                        };
                        return Ok(false);
                    }
                    self.stage = Stage::Done;
                    return Ok(true);
                }
                // Original size scratch is gone before codec/header allocation.
                drop(cursor);
                match begin_scalar_tail(self.value, limits)
                    .map_err(|e| ValidationIssue::new(e, false))?
                {
                    Some(bytes) => {
                        self.stage = Stage::Bytes(bytes);
                        Ok(false)
                    }
                    None => {
                        self.stage = Stage::Done;
                        Ok(true)
                    }
                }
            }
            Stage::Bytes(mut cursor) => {
                if cursor
                    .advance()
                    .map_err(|e| ValidationIssue::new(e, false))?
                {
                    cursor
                        .finish()
                        .map_err(|e| ValidationIssue::new(e, false))?;
                    self.stage = Stage::Done;
                    Ok(true)
                } else {
                    self.stage = Stage::Bytes(cursor);
                    Ok(false)
                }
            }
            Stage::Done => {
                self.stage = Stage::Done;
                Ok(true)
            }
            Stage::Failed => Err(ValidationIssue::new(
                "Scalar validation already failed".into(),
                false,
            )),
        }
    }
}
fn resource_digits(value: ScalarSource<'_>, limits: &ValueLimits) -> Result<(), String> {
    if matches!(value.kind, Some(Scalar::BigInt | Scalar::Decimal)) {
        let name = if value.kind == Some(Scalar::BigInt) {
            "value"
        } else {
            "coefficient"
        };
        if value
            .payload
            .get(name)
            .and_then(serde_json::Value::as_str)
            .is_some_and(|text| text.trim_start_matches('-').len() > limits.max_digits)
        {
            return Err("Scalar digit limit exceeded".into());
        }
    }
    Ok(())
}

/// The SAME original scalar frontier policy, reused only after canonical
/// validation destroys its preceding resource/size frontier.
pub(crate) fn measurement_admission(
    max_bytes: usize,
) -> impl crate::definitions::retained_json::Admission {
    super::values::ScalarScratch::new(max_bytes)
}
