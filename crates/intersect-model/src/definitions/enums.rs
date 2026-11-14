//! Handwritten enum identity and value codec shared by definition/model consumers.
use crate::definitions::resources::DeclarationId;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Literal {
    pub declaration: DeclarationId,
    pub name: String,
    pub value: i32,
    /// None inherits the name; Some("") is an explicit empty spelling.
    pub literal: Option<String>,
}
impl Literal {
    pub fn spelling(&self) -> &str {
        self.literal.as_deref().unwrap_or(&self.name)
    }
}

/// Wire values identify a particular alias, not merely its name or numeric value.
/// Deserialize does not authorize a value: consumers must call validate_value.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum EnumValue {
    Enum {
        enumeration: DeclarationId,
        literal: DeclarationId,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "EnumWire")]
pub struct Enumeration {
    declaration: DeclarationId,
    name: String,
    serializable: bool,
    literals: Vec<Literal>,
    /// Exact imported XML preserves annotations and Java binding metadata without
    /// executing them. Hand-constructed definitions have no source payload.
    source_payload: Option<String>,
    #[serde(skip)]
    by_name: BTreeMap<String, usize>,
    #[serde(skip)]
    by_literal: BTreeMap<String, usize>,
    #[serde(skip)]
    by_value: BTreeMap<i32, usize>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct EnumWire {
    declaration: DeclarationId,
    name: String,
    serializable: bool,
    literals: Vec<Literal>,
    source_payload: Option<String>,
}
impl TryFrom<EnumWire> for Enumeration {
    type Error = String;
    fn try_from(wire: EnumWire) -> Result<Self, String> {
        let mut result = Self::new(
            wire.declaration,
            wire.name,
            wire.serializable,
            wire.literals,
        )?;
        result.source_payload = wire.source_payload;
        Ok(result)
    }
}
impl Enumeration {
    pub fn new(
        declaration: DeclarationId,
        name: String,
        serializable: bool,
        literals: Vec<Literal>,
    ) -> Result<Self, String> {
        Self::new_with_admission(declaration, name, serializable, literals, |_| Ok(()))
    }

    // The importer supplies allocation admission; validation and index semantics
    // remain shared with direct construction and wire restoration.
    pub(crate) fn new_with_admission(
        declaration: DeclarationId,
        name: String,
        serializable: bool,
        literals: Vec<Literal>,
        mut admit: impl FnMut(Option<&str>) -> Result<(), String>,
    ) -> Result<Self, String> {
        let mut preparation =
            EnumerationConstruction::begin(declaration, name, serializable, literals);
        while !preparation.advance(&mut admit)? {}
        preparation.finish()
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
    pub fn literals(&self) -> &[Literal] {
        &self.literals
    }
    pub fn source_payload(&self) -> Option<&str> {
        self.source_payload.as_deref()
    }
    pub(crate) fn retain_source(&mut self, source: String) {
        self.source_payload = Some(source);
    }
    pub fn by_name(&self, name: &str) -> Option<&Literal> {
        self.by_name
            .get(name)
            .and_then(|index| self.literals.get(*index))
    }
    pub fn by_literal(&self, spelling: &str) -> Option<&Literal> {
        self.by_literal
            .get(spelling)
            .and_then(|index| self.literals.get(*index))
    }
    pub fn by_value(&self, value: i32) -> Option<&Literal> {
        self.by_value
            .get(&value)
            .and_then(|index| self.literals.get(*index))
    }
    pub fn by_id(&self, id: &DeclarationId) -> Option<&Literal> {
        self.literals
            .iter()
            .find(|literal| &literal.declaration == id)
    }
    fn value(&self, literal: &Literal) -> EnumValue {
        EnumValue::Enum {
            enumeration: self.declaration.clone(),
            literal: literal.declaration.clone(),
        }
    }
    pub fn value_by_id(&self, id: &DeclarationId) -> Result<EnumValue, String> {
        self.by_id(id)
            .map(|literal| self.value(literal))
            .ok_or_else(|| format!("Unknown literal declaration in {}: {id:?}", self.name))
    }
    pub fn default_value(&self) -> Option<EnumValue> {
        self.literals.first().map(|literal| self.value(literal))
    }
    pub fn parse(&self, spelling: &str) -> Result<EnumValue, String> {
        if !self.serializable {
            return Err(format!("Enum {} is not serializable", self.name));
        }
        self.by_literal(spelling)
            .map(|literal| self.value(literal))
            .ok_or_else(|| format!("Invalid literal {spelling:?} for enum {}", self.name))
    }
    pub fn validate_value(&self, value: &EnumValue) -> Result<&Literal, String> {
        let EnumValue::Enum {
            enumeration,
            literal,
        } = value;
        if enumeration != &self.declaration {
            return Err(format!(
                "Enum value belongs to another declaration or revision: {enumeration:?}"
            ));
        }
        self.by_id(literal)
            .ok_or_else(|| format!("Unknown enum value literal: {literal:?}"))
    }
    pub fn format(&self, value: &EnumValue) -> Result<&str, String> {
        if !self.serializable {
            return Err(format!("Enum {} is not serializable", self.name));
        }
        Ok(self.validate_value(value)?.spelling())
    }
}

#[path = "enum_construction.rs"]
mod construction;
pub(crate) use construction::EnumerationConstruction;
