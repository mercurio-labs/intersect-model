//! Handwritten portable lexical sequence codec; no Java collection semantics are implied.
use crate::collection_conversion::Member;
use intersect_model::definitions::conversion::{
    ConversionContract, ConversionProvider, InvocationContext,
};
use serde_json::Value;

pub(crate) struct SequenceConverter {
    contract: ConversionContract,
    member: Member,
}
impl SequenceConverter {
    pub(crate) fn new(contract: ConversionContract, member: Member) -> Result<Self, String> {
        if !contract.serializable
            || !contract.declarations.is_empty()
            || contract.members.as_slice() != [member.contract().clone()]
        {
            return Err("Sequence requires a serializable contract with one exact member".into());
        }
        member.require_lexical()?;
        Ok(Self { contract, member })
    }
}
impl ConversionProvider for SequenceConverter {
    fn contract(&self) -> &ConversionContract {
        &self.contract
    }
    fn parse(&self, text: &str) -> Result<Value, String> {
        self.parse_observed(text, &InvocationContext::default())
    }
    fn parse_observed(&self, text: &str, observation: &InvocationContext) -> Result<Value, String> {
        let lexical: Vec<Option<String>> = serde_json::from_str(text).map_err(|e| e.to_string())?;
        let items = lexical
            .into_iter()
            .map(|item| match item {
                Some(text) => self.member.parse_lexical_observed(&text, observation),
                None => {
                    self.member.validate_observed(&Value::Null, observation)?;
                    Ok(Value::Null)
                }
            })
            .collect::<Result<Vec<_>, String>>()?;
        let value = Value::Array(items);
        self.validate_observed(&value, observation)?;
        Ok(value)
    }
    fn format(&self, value: &Value) -> Result<String, String> {
        self.format_observed(value, &InvocationContext::default())
    }
    fn format_observed(
        &self,
        value: &Value,
        observation: &InvocationContext,
    ) -> Result<String, String> {
        self.validate_observed(value, observation)?;
        let items = value
            .as_array()
            .ok_or("Sequence payload must be an array")?;
        let lexical = items
            .iter()
            .map(|item| {
                if item.is_null() {
                    Ok(None)
                } else {
                    self.member
                        .format_lexical_observed(item, observation)
                        .map(Some)
                }
            })
            .collect::<Result<Vec<_>, String>>()?;
        serde_json::to_string(&lexical).map_err(|e| e.to_string())
    }
    fn validate(&self, value: &Value) -> Result<(), String> {
        self.validate_observed(value, &InvocationContext::default())
    }
    fn validate_observed(
        &self,
        value: &Value,
        observation: &InvocationContext,
    ) -> Result<(), String> {
        observation.work(1)?;
        self.member.require_lexical()?;
        for item in value
            .as_array()
            .ok_or("Sequence payload must be an array")?
        {
            self.member.validate_observed(item, observation)?;
        }
        Ok(())
    }
    fn default_value(&self) -> Result<Option<Value>, String> {
        self.default_value_observed(&InvocationContext::default())
    }
    fn default_value_observed(
        &self,
        observation: &InvocationContext,
    ) -> Result<Option<Value>, String> {
        let value = Value::Array(Vec::new());
        self.validate_observed(&value, observation)?;
        Ok(Some(value))
    }
}
