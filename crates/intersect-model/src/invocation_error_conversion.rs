//! Handwritten portable invocation-error descriptors; never constructs Java exceptions.
use intersect_model::definitions::conversion::{
    ConversionContract, ConversionProvider, ProviderId,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TargetError {
    pub domain: ProviderId,
    pub code: String,
    pub message: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InvocationError {
    pub message: Option<String>,
    pub target: Option<TargetError>,
}
impl InvocationError {
    /// The wrapped target, with its domain/version intact; absence is meaningful.
    pub fn cause(&self) -> Option<&TargetError> {
        self.target.as_ref()
    }
    /// Stable JSON-escaped diagnostics preserve null versus empty and avoid ambiguous delimiters.
    pub fn diagnostic(&self) -> Result<String, String> {
        self.check()?;
        let payload = serde_json::to_string(self).map_err(|error| error.to_string())?;
        Ok(format!("InvocationTargetException {payload}"))
    }
    pub(crate) fn check(&self) -> Result<(), String> {
        if let Some(target) = &self.target
            && (target.domain.name.trim().is_empty()
                || target.domain.version.trim().is_empty()
                || target.code.trim().is_empty())
        {
            return Err("Invocation target requires a nonblank domain, version and code".into());
        }
        Ok(())
    }
}

pub struct InvocationErrorConverter {
    contract: ConversionContract,
}
impl InvocationErrorConverter {
    pub fn new(contract: ConversionContract) -> Result<Self, String> {
        if contract.serializable {
            return Err(
                "Invocation error provider requires a non-serializable datatype contract".into(),
            );
        }
        if !contract.declarations.is_empty() {
            return Err("Invocation error provider does not consume declaration scope".into());
        }
        Ok(Self { contract })
    }
    pub fn descriptor(&self, value: &Value) -> Result<InvocationError, String> {
        let descriptor: InvocationError = serde_json::from_value(value.clone())
            .map_err(|error| format!("Invalid invocation error descriptor: {error}"))?;
        // Serde accepts absent Option fields. The wire requires explicit nulls so
        // snapshots cannot silently rewrite missing information on a round trip.
        if serde_json::to_value(&descriptor).map_err(|error| error.to_string())? != *value {
            return Err(
                "Invocation error descriptor requires explicit message and target fields".into(),
            );
        }
        descriptor.check()?;
        Ok(descriptor)
    }
}
impl ConversionProvider for InvocationErrorConverter {
    fn contract(&self) -> &ConversionContract {
        &self.contract
    }
    fn parse(&self, _text: &str) -> Result<Value, String> {
        Err("EInvocationTargetException is not lexically serializable".into())
    }
    fn format(&self, _value: &Value) -> Result<String, String> {
        Err("EInvocationTargetException is not lexically serializable".into())
    }
    fn validate(&self, value: &Value) -> Result<(), String> {
        self.descriptor(value).map(|_| ())
    }
    fn default_value(&self) -> Result<Option<Value>, String> {
        Ok(None)
    }
}
