//! Handwritten nominal EDataType bindings over the shared scalar codecs.
use crate::definitions::{resources::DeclarationId, scalars::Builtin};
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Exact mappings, independent of the EDataType's local classifier name.
pub(crate) fn native_binding(name: &str) -> Option<Builtin> {
    Some(match name {
        "java.math.BigInteger" => Builtin::EBigInteger,
        "byte[]" => Builtin::EByteArray,
        "char" => Builtin::EChar,
        "java.lang.Character" => Builtin::ECharacterObject,
        "java.util.Date" => Builtin::EDate,
        "java.math.BigDecimal" => Builtin::EBigDecimal,
        "java.lang.String" => Builtin::EString,
        "boolean" => Builtin::EBoolean,
        "byte" => Builtin::EByte,
        "short" => Builtin::EShort,
        "int" => Builtin::EInt,
        "long" => Builtin::ELong,
        "float" => Builtin::EFloat,
        "double" => Builtin::EDouble,
        "java.lang.Boolean" => Builtin::EBooleanObject,
        "java.lang.Byte" => Builtin::EByteObject,
        "java.lang.Short" => Builtin::EShortObject,
        "java.lang.Integer" => Builtin::EIntegerObject,
        "java.lang.Long" => Builtin::ELongObject,
        "java.lang.Float" => Builtin::EFloatObject,
        "java.lang.Double" => Builtin::EDoubleObject,
        _ => return None,
    })
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Binding {
    Scalar { codec: Builtin },
    Unsupported { reason: String },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum DatatypeValue<Payload = Value> {
    Datatype {
        datatype: DeclarationId,
        value: Payload,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "DatatypeWire")]
pub struct Datatype {
    declaration: DeclarationId,
    name: String,
    serializable: bool,
    instance_class_name: Option<String>,
    instance_type_name: Option<String>,
    binding: Binding,
    source_payload: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    parameters: Vec<crate::definitions::generics::TypeParameter>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    application: Option<crate::definitions::generic_types::ClosedType>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct DatatypeWire {
    declaration: DeclarationId,
    name: String,
    serializable: bool,
    instance_class_name: Option<String>,
    instance_type_name: Option<String>,
    binding: Binding,
    source_payload: Option<String>,
    #[serde(default)]
    parameters: Vec<crate::definitions::generics::TypeParameter>,
    #[serde(default)]
    application: Option<crate::definitions::generic_types::ClosedType>,
}
impl TryFrom<DatatypeWire> for Datatype {
    type Error = String;
    fn try_from(wire: DatatypeWire) -> Result<Self, String> {
        let mut result = Self::new(
            wire.declaration,
            wire.name,
            wire.serializable,
            wire.instance_class_name,
            wire.instance_type_name,
        )?;
        result.retain_parameters(wire.parameters)?;
        if let Some(application) = wire.application {
            result.retain_application(application)?;
        }
        if result.binding != wire.binding {
            return Err("Datatype binding does not match its declared instance metadata".into());
        }
        result.source_payload = wire.source_payload;
        Ok(result)
    }
}

impl Datatype {
    pub fn new(
        declaration: DeclarationId,
        name: String,
        serializable: bool,
        instance_class_name: Option<String>,
        instance_type_name: Option<String>,
    ) -> Result<Self, String> {
        Self::new_with_formatter(
            declaration,
            name,
            serializable,
            instance_class_name,
            instance_type_name,
            |arguments| Ok(arguments.to_string()),
        )
    }

    pub(crate) fn new_with_formatter(
        declaration: DeclarationId,
        name: String,
        serializable: bool,
        instance_class_name: Option<String>,
        instance_type_name: Option<String>,
        mut format: impl FnMut(std::fmt::Arguments<'_>) -> Result<String, String>,
    ) -> Result<Self, String> {
        if name.is_empty() || declaration.path.is_empty() {
            return Err("Datatype requires a name and classifier declaration identity".into());
        }
        let binding = match (
            instance_class_name.as_deref(),
            instance_type_name.as_deref(),
        ) {
            (Some(class), Some(ty)) if class != ty => Binding::Unsupported {
                reason: format(format_args!(
                    "Conflicting or extended instance metadata: {class:?}, {ty:?}"
                ))?,
            },
            (Some(name), _) | (_, Some(name)) => match native_binding(name) {
                Some(codec) => Binding::Scalar { codec },
                None => Binding::Unsupported {
                    reason: format(format_args!(
                        "No native scalar binding for {name:?}; an explicit provider is required"
                    ))?,
                },
            },
            (None, None) => Binding::Unsupported {
                reason: format(format_args!(
                    "No declared instance binding; an explicit provider is required"
                ))?,
            },
        };
        Ok(Self {
            declaration,
            name,
            serializable,
            instance_class_name,
            instance_type_name,
            binding,
            source_payload: None,
            parameters: vec![],
            application: None,
        })
    }
    pub fn application(&self) -> Option<&crate::definitions::generic_types::ClosedType> {
        self.application.as_ref()
    }
    pub fn specialize(
        mut self,
        checked: &crate::definitions::generic_resolution::CheckedDatatypeApplication,
    ) -> Result<Self, String> {
        if self.parameters
            != checked
                .parameters()
                .iter()
                .map(|p| p.syntax.clone())
                .collect::<Vec<_>>()
        {
            return Err("Datatype formals differ from checked application".into());
        }
        self.retain_application(checked.application().clone())?;
        Ok(self)
    }
    // Structural wire validation only; provider reconstruction rechecks source/bounds.
    fn retain_application(
        &mut self,
        application: crate::definitions::generic_types::ClosedType,
    ) -> Result<(), String> {
        let crate::definitions::generic_types::Type::Classifier {
            declaration,
            arguments,
            ..
        } = application.expression()
        else {
            return Err("Datatype application requires a closed classifier".into());
        };
        if declaration != &self.declaration
            || self.parameters.is_empty()
            || arguments.len() != self.parameters.len()
        {
            return Err("Datatype application identity or arity mismatch".into());
        }
        self.application = Some(application);
        Ok(())
    }
    /// Ordered source formals. Deserialization is not a source-resolution certificate.
    pub fn parameters(&self) -> &[crate::definitions::generics::TypeParameter] {
        &self.parameters
    }
    pub(crate) fn retain_parameters(
        &mut self,
        parameters: Vec<crate::definitions::generics::TypeParameter>,
    ) -> Result<(), String> {
        let mut names = std::collections::BTreeSet::new();
        let mut previous = None;
        for parameter in &parameters {
            let id = &parameter.declaration;
            if id.source != self.declaration.source
                || parameter.span.source != id.source
                || id.path.len() != self.declaration.path.len() + 1
                || !id.path.starts_with(&self.declaration.path)
                || previous.is_some_and(|prior| prior >= id)
                || parameter.name.is_empty()
                || !names.insert(&parameter.name)
            {
                return Err("Invalid datatype formal identity, order or name".into());
            }
            previous = Some(id);
        }
        if !parameters.is_empty() {
            self.binding = Binding::Unsupported {
                reason: "Generic datatype requires a checked application and specialized provider"
                    .into(),
            };
        }
        self.parameters = parameters;
        Ok(())
    }
    pub fn declaration(&self) -> &DeclarationId {
        &self.declaration
    }
    pub fn name(&self) -> &str {
        &self.name
    }
    pub fn serializable(&self) -> bool {
        self.serializable
    }
    pub fn instance_class_name(&self) -> Option<&str> {
        self.instance_class_name.as_deref()
    }
    pub fn instance_type_name(&self) -> Option<&str> {
        self.instance_type_name.as_deref()
    }
    pub fn binding(&self) -> &Binding {
        &self.binding
    }
    pub fn source_payload(&self) -> Option<&str> {
        self.source_payload.as_deref()
    }
    pub(crate) fn retain_source(&mut self, source: String) {
        self.source_payload = Some(source);
    }
    pub fn codec(&self) -> Result<Builtin, String> {
        match &self.binding {
            Binding::Scalar { codec } => Ok(*codec),
            Binding::Unsupported { reason } => {
                Err(format!("Unsupported datatype {}: {reason}", self.name))
            }
        }
    }
    /// Explicitly construct a nominal value after validating its scalar payload.
    pub fn bind_value(&self, value: Value) -> Result<DatatypeValue, String> {
        self.codec()?.validate(&value)?;
        if value.is_null() {
            return Err("Null is absence, not a present datatype value".into());
        }
        Ok(DatatypeValue::Datatype {
            datatype: self.declaration.clone(),
            value,
        })
    }
    pub fn default_value(&self) -> Result<Value, String> {
        let value = self.codec()?.default_value();
        if value.is_null() {
            return Ok(value);
        }
        serde_json::to_value(self.bind_value(value)?).map_err(|error| error.to_string())
    }
    pub fn validate_value<'a>(&self, value: &'a DatatypeValue) -> Result<&'a Value, String> {
        let DatatypeValue::Datatype { datatype, value } = value;
        self.validate_parts(datatype, value)
    }
    /// Check a borrowed canonical value through the same declaration and codec
    /// policy as the owned facade. Parsing a view is not membership authority.
    pub fn validate_borrowed_value<'a>(
        &self,
        value: &DatatypeValue<&'a Value>,
    ) -> Result<&'a Value, String> {
        let DatatypeValue::Datatype { datatype, value } = value;
        self.validate_parts(datatype, value)
    }
    fn validate_parts<'a>(
        &self,
        datatype: &DeclarationId,
        value: &'a Value,
    ) -> Result<&'a Value, String> {
        if datatype != &self.declaration {
            return Err("Datatype value belongs to another declaration or revision".into());
        }
        self.codec()?.validate(value)?;
        if value.is_null() {
            return Err("Null is absence, not a present datatype value".into());
        }
        Ok(value)
    }
    fn require_serializable(&self) -> Result<(), String> {
        if self.serializable {
            Ok(())
        } else {
            Err(format!("Datatype {} is not serializable", self.name))
        }
    }
    pub fn parse(&self, text: &str) -> Result<DatatypeValue, String> {
        self.require_serializable()?;
        self.bind_value(self.codec()?.parse(text)?)
    }
    pub fn format(&self, value: &DatatypeValue) -> Result<String, String> {
        self.require_serializable()?;
        self.codec()?
            .format(self.validate_value(value)?)?
            .ok_or_else(|| "Expected a present datatype value".into())
    }

    /// Shared JSON boundary for native datatype codecs and explicitly installed
    /// conversion providers. Neither names nor JSON tags confer provider authority.
    pub fn format_json(
        &self,
        value: &Value,
        conversions: Option<&crate::definitions::conversion::ConversionRegistry<'_>>,
    ) -> Result<String, String> {
        if self.codec().is_ok() {
            let value = serde_json::from_value(value.clone()).map_err(|e| e.to_string())?;
            self.format(&value)
        } else {
            conversions
                .ok_or("Explicit conversion registry required for datatype formatting")?
                .format_datatype(self, value)
        }
    }
}

/// Parse an immutable canonical native datatype view without cloning its JSON
/// payload. The same generic wire schema supplies owned and borrowed facades.
/// Declaration/codec validation remains mandatory and separate. This routine is
/// synchronous: serde header preparation and the work-hook walk are not yielding.
pub fn borrowed_value_with_work<'a>(
    payload: &'a Value,
    work: &dyn Fn(usize) -> Result<(), String>,
) -> Result<DatatypeValue<&'a Value>, String> {
    let parsed = DatatypeValue::<serde::de::IgnoredAny>::deserialize(payload)
        .map_err(|error| error.to_string())?;
    let DatatypeValue::Datatype { datatype, .. } = parsed;
    let value = payload
        .get("value")
        .ok_or("Checked datatype payload unavailable")?;
    let view = DatatypeValue::Datatype { datatype, value };
    if !crate::definitions::json_exact::matches_with_work(&view, payload, work)? {
        return Err("Nominal value requires exact canonical fields".into());
    }
    Ok(view)
}
