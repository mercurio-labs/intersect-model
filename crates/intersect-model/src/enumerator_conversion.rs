//! Language-owned EEnumerator adapter over the shared exact enum value contract.
use intersect_model::definitions::{
    conversion::{ConversionContract, ConversionProvider},
    enums::{EnumValue, Enumeration, Literal},
    resources::DeclarationId,
};
use serde_json::Value;
use std::collections::BTreeMap;

pub struct EnumeratorConverter {
    contract: ConversionContract,
    enumerations: BTreeMap<DeclarationId, Enumeration>,
}
impl EnumeratorConverter {
    pub fn new(
        mut contract: ConversionContract,
        enumerations: impl IntoIterator<Item = Enumeration>,
    ) -> Result<Self, String> {
        if contract.serializable {
            return Err(
                "EEnumerator provider requires a non-serializable datatype contract".into(),
            );
        }
        let mut selected = BTreeMap::new();
        for enumeration in enumerations {
            if selected
                .insert(enumeration.declaration().clone(), enumeration)
                .is_some()
            {
                return Err("Duplicate enum declaration in EEnumerator provider".into());
            }
        }
        let declarations: Vec<_> = selected.keys().cloned().collect();
        if !contract.declarations.is_empty() && contract.declarations != declarations {
            return Err("EEnumerator declaration scope differs from its exact enum table".into());
        }
        contract.declarations = declarations;
        Ok(Self {
            contract,
            enumerations: selected,
        })
    }

    /// Resolve a checked literal without reducing numeric aliases to one value.
    pub fn literal(&self, value: &EnumValue) -> Result<&Literal, String> {
        let EnumValue::Enum { enumeration, .. } = value;
        self.enumerations
            .get(enumeration)
            .ok_or("Unresolved enum declaration or revision in EEnumerator provider")?
            .validate_value(value)
    }
}
impl ConversionProvider for EnumeratorConverter {
    fn contract(&self) -> &ConversionContract {
        &self.contract
    }
    fn parse(&self, _text: &str) -> Result<Value, String> {
        Err("EEnumerator is not lexically serializable".into())
    }
    fn format(&self, _value: &Value) -> Result<String, String> {
        Err("EEnumerator is not lexically serializable".into())
    }
    fn validate(&self, value: &Value) -> Result<(), String> {
        let value: EnumValue =
            serde_json::from_value(value.clone()).map_err(|error| error.to_string())?;
        self.literal(&value).map(|_| ())
    }
    fn default_value(&self) -> Result<Option<Value>, String> {
        Ok(None)
    }
}
