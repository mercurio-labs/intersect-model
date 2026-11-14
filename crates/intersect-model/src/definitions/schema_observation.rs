//! Observation failures must not become a false schema predicate.
use crate::definitions::conversion::InvocationContext;

#[derive(Debug, PartialEq, Eq)]
pub enum PreparationError {
    Schema(String),
    Observation(String),
}
impl std::fmt::Display for PreparationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Schema(message) | Self::Observation(message) => f.write_str(message),
        }
    }
}
impl std::error::Error for PreparationError {}
impl From<String> for PreparationError {
    fn from(value: String) -> Self {
        Self::Schema(value)
    }
}
impl From<&str> for PreparationError {
    fn from(value: &str) -> Self {
        Self::Schema(value.into())
    }
}
pub(crate) fn reserve(
    observation: &InvocationContext<'_>,
    slots: usize,
    names: &[&str],
) -> Result<(), PreparationError> {
    let bytes = names
        .iter()
        .try_fold(slots, |sum, name| {
            sum.checked_add(64)?.checked_add(name.len())
        })
        .ok_or_else(|| {
            PreparationError::Observation("Schema preparation storage overflow".into())
        })?;
    observation
        .checkpoint()
        .map_err(PreparationError::Observation)?;
    observation
        .retain_storage(bytes)
        .map_err(PreparationError::Observation)
}
pub(super) fn allocation<T>(
    values: &mut Vec<T>,
    additional: usize,
) -> Result<(), PreparationError> {
    values
        .try_reserve(additional)
        .map_err(|_| PreparationError::Observation("Schema preparation allocation failed".into()))
}
pub(crate) fn reserve_value<T: ?Sized + serde::Serialize>(
    value: &T,
    observation: &InvocationContext<'_>,
) -> Result<(), PreparationError> {
    // The unobserved compatibility path has no metadata/storage policy.
    if observation.has_observer() {
        crate::definitions::retained_storage::reserve_serialized(value, observation)
            .map_err(PreparationError::Observation)?;
    }
    Ok(())
}
