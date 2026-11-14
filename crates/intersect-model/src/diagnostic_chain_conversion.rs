//! Handwritten portable diagnostic accumulation. No host objects or ambient resolution.
use crate::invocation_error_conversion::InvocationError;
use intersect_model::definitions::{
    conversion::{ConversionContract, ConversionProvider, InvocationContext},
    scalars::Builtin,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    #[default]
    Ok,
    Info,
    Warning,
    Error,
    Cancel,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum DiagnosticData {
    Scalar {
        datatype: String,
        value: Value,
    },
    /// Inert identity of possibly invalid model evidence; never an implicit resource load.
    Reference {
        resource: String,
        revision: String,
        fragment: String,
    },
    InvocationError {
        value: InvocationError,
    },
}
impl DiagnosticData {
    fn validate(&self) -> Result<(), String> {
        match self {
            Self::Scalar { datatype, value } => Builtin::named(datatype)
                .ok_or("Unknown diagnostic scalar datatype")?
                .validate(value),
            Self::Reference {
                resource,
                revision,
                fragment,
            } => {
                if [resource, revision, fragment]
                    .iter()
                    .any(|part| part.trim().is_empty())
                {
                    Err("Diagnostic reference requires resource, revision and fragment".into())
                } else {
                    Ok(())
                }
            }
            Self::InvocationError { value } => value.check(),
        }
    }
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Diagnostic {
    pub severity: Severity,
    pub source: Option<String>,
    pub code: i32,
    pub message: Option<String>,
    pub data: Vec<DiagnosticData>,
    pub children: Vec<Diagnostic>,
}
impl Diagnostic {
    pub fn validate(&self) -> Result<(), String> {
        self.validate_observed(&InvocationContext::default())
    }
    fn validate_observed(&self, observation: &InvocationContext<'_>) -> Result<(), String> {
        observation.work(1)?;
        let mut pending = vec![self];
        while let Some(node) = pending.pop() {
            observation.work(1)?;
            for data in &node.data {
                observation.work(1)?;
                data.validate()?;
            }
            for child in &node.children {
                observation.work(1)?;
                pending.push(child);
            }
        }
        Ok(())
    }
    pub fn from_wire(value: &Value) -> Result<Self, String> {
        let diagnostic: Self = serde_json::from_value(value.clone())
            .map_err(|e| format!("Invalid diagnostic: {e}"))?;
        if serde_json::to_value(&diagnostic).map_err(|e| e.to_string())? != *value {
            return Err(
                "Diagnostic wire requires all fields explicitly, including nullable fields".into(),
            );
        }
        diagnostic.validate()?;
        Ok(diagnostic)
    }
}
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DiagnosticChain {
    diagnostics: Vec<Diagnostic>,
}
impl DiagnosticChain {
    pub fn from_wire(value: &Value) -> Result<Self, String> {
        Self::from_wire_observed(value, &InvocationContext::default())
    }
    fn from_wire_observed(
        value: &Value,
        observation: &InvocationContext<'_>,
    ) -> Result<Self, String> {
        observation.work(1)?;
        let chain: Self = serde_json::from_value(value.clone())
            .map_err(|e| format!("Invalid diagnostic chain: {e}"))?;
        if serde_json::to_value(&chain).map_err(|e| e.to_string())? != *value {
            return Err("Diagnostic chain wire requires all fields explicitly".into());
        }
        chain.validate_observed(observation)?;
        Ok(chain)
    }
    pub fn diagnostics(&self) -> &[Diagnostic] {
        &self.diagnostics
    }
    pub fn severity(&self) -> Severity {
        self.diagnostics
            .iter()
            .map(|d| d.severity)
            .max()
            .unwrap_or_default()
    }
    pub fn validate(&self) -> Result<(), String> {
        self.validate_observed(&InvocationContext::default())
    }
    fn validate_observed(&self, observation: &InvocationContext<'_>) -> Result<(), String> {
        observation.work(1)?;
        for diagnostic in &self.diagnostics {
            diagnostic.validate_observed(observation)?;
        }
        Ok(())
    }
    pub fn add(&mut self, diagnostic: Diagnostic) -> Result<(), String> {
        self.validate()?;
        diagnostic.validate()?;
        self.diagnostics.push(diagnostic);
        Ok(())
    }
    pub fn add_all(&mut self, diagnostic: Diagnostic) -> Result<(), String> {
        self.validate()?;
        diagnostic.validate()?;
        self.diagnostics.extend(diagnostic.children);
        Ok(())
    }
    pub fn merge(&mut self, diagnostic: Diagnostic) -> Result<(), String> {
        if diagnostic.children.is_empty() {
            self.add(diagnostic)
        } else {
            self.add_all(diagnostic)
        }
    }
}
pub struct DiagnosticChainConverter {
    contract: ConversionContract,
}
impl DiagnosticChainConverter {
    pub fn new(contract: ConversionContract) -> Result<Self, String> {
        if contract.serializable {
            return Err("Diagnostic chain requires a non-serializable datatype contract".into());
        }
        if !contract.declarations.is_empty() {
            return Err("Diagnostic chain does not consume declaration scope".into());
        }
        Ok(Self { contract })
    }
}
impl ConversionProvider for DiagnosticChainConverter {
    fn contract(&self) -> &ConversionContract {
        &self.contract
    }
    fn parse(&self, _text: &str) -> Result<Value, String> {
        Err("EDiagnosticChain is not lexically serializable".into())
    }
    fn format(&self, _value: &Value) -> Result<String, String> {
        Err("EDiagnosticChain is not lexically serializable".into())
    }
    fn validate(&self, value: &Value) -> Result<(), String> {
        self.validate_observed(value, &InvocationContext::default())
    }
    fn validate_observed(
        &self,
        value: &Value,
        observation: &InvocationContext<'_>,
    ) -> Result<(), String> {
        DiagnosticChain::from_wire_observed(value, observation).map(|_| ())
    }
    fn default_value(&self) -> Result<Option<Value>, String> {
        Ok(None)
    }
}
