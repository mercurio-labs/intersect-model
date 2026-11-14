//! One index-construction algorithm for direct, wire and imported enums.
use super::*;
use crate::definitions::schema::validate_declaration_order;
pub(crate) struct EnumerationConstruction {
    value: Enumeration,
    index: usize,
    started: bool,
    complete: bool,
    failure: Option<String>,
}
impl EnumerationConstruction {
    pub(crate) fn begin(
        declaration: DeclarationId,
        name: String,
        serializable: bool,
        literals: Vec<Literal>,
    ) -> Self {
        Self {
            value: Enumeration {
                declaration,
                name,
                serializable,
                literals,
                source_payload: None,
                by_name: BTreeMap::new(),
                by_literal: BTreeMap::new(),
                by_value: BTreeMap::new(),
            },
            index: 0,
            started: false,
            complete: false,
            failure: None,
        }
    }
    pub(crate) fn advance(
        &mut self,
        admit: &mut impl FnMut(Option<&str>) -> Result<(), String>,
    ) -> Result<bool, String> {
        if let Some(error) = &self.failure {
            return Err(error.clone());
        }
        let result = self.advance_inner(admit);
        if let Err(error) = &result {
            self.failure = Some(error.clone());
        }
        result
    }
    fn advance_inner(
        &mut self,
        admit: &mut impl FnMut(Option<&str>) -> Result<(), String>,
    ) -> Result<bool, String> {
        if self.complete {
            return Ok(true);
        }
        if !self.started {
            if self.value.name.is_empty() || self.value.declaration.path.is_empty() {
                return Err("Enum requires a name and classifier declaration identity".into());
            }
            self.started = true;
            return Ok(false);
        }
        let Some(literal) = self.value.literals.get(self.index) else {
            self.complete = true;
            return Ok(true);
        };
        let id = &literal.declaration;
        if id.source != self.value.declaration.source
            || id.path.len() != self.value.declaration.path.len() + 1
            || !id.path.starts_with(&self.value.declaration.path)
        {
            return Err(format!("Enum literal has wrong owner: {id:?}"));
        }
        if self.index > 0 {
            validate_declaration_order(&self.value.literals[self.index - 1].declaration, id)?;
        }
        if literal.name.is_empty() {
            return Err(format!("Enum literal requires a name: {id:?}"));
        }
        admit(Some(&literal.name))?;
        if let Some(prior) = self.value.by_name.insert(literal.name.clone(), self.index) {
            return Err(format!(
                "Duplicate enum literal name {}: {:?} and {id:?}",
                literal.name, self.value.literals[prior].declaration
            ));
        }
        admit(Some(literal.spelling()))?;
        if let Some(prior) = self
            .value
            .by_literal
            .insert(literal.spelling().to_owned(), self.index)
        {
            return Err(format!(
                "Duplicate enum literal spelling {:?}: {:?} and {id:?}",
                literal.spelling(),
                self.value.literals[prior].declaration
            ));
        }
        if !self.value.by_value.contains_key(&literal.value) {
            admit(None)?;
        }
        self.value
            .by_value
            .entry(literal.value)
            .or_insert(self.index);
        self.index += 1;
        Ok(false)
    }
    pub(crate) fn finish(self) -> Result<Enumeration, String> {
        if let Some(error) = self.failure {
            return Err(error);
        }
        if !self.complete {
            return Err("Enum construction is incomplete".into());
        }
        Ok(self.value)
    }
}
