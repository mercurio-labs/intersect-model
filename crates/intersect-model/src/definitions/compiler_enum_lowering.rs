//! Shared literal lowering and index construction; field helpers stay synchronous.
use super::*;
use crate::definitions::{
    ecore::ParserObservation,
    enums::{Enumeration, EnumerationConstruction, Literal},
};
enum Phase {
    Header,
    Children(usize),
    Construct,
    Index,
    Payload,
    Complete,
}
pub(super) struct EnumLoweringPreparation<'a> {
    node: &'a Xml,
    phase: Phase,
    literals: Vec<Literal>,
    construction: Option<EnumerationConstruction>,
    output: Option<Enumeration>,
    failed: bool,
}
impl<'a> EnumLoweringPreparation<'a> {
    pub(super) fn begin(node: &'a Xml) -> Self {
        Self {
            node,
            phase: Phase::Header,
            literals: vec![],
            construction: None,
            output: None,
            failed: false,
        }
    }
    pub(super) fn advance(
        &mut self,
        observation: Option<&ParserObservation<'_, '_>>,
    ) -> Result<bool, ImportError> {
        if self.failed {
            return Err(self.node.error("Enum lowering has failed"));
        }
        let result = self.advance_inner(observation);
        if result.is_err() {
            self.failed = true;
        }
        result
    }
    fn advance_inner(
        &mut self,
        observation: Option<&ParserObservation<'_, '_>>,
    ) -> Result<bool, ImportError> {
        let node = self.node;
        match self.phase {
            Phase::Header => {
                if node.name != "eClassifiers"
                    || node.attrs.get("xsi:type").map(String::as_str) != Some("ecore:EEnum")
                {
                    return Err(node.error("Expected an EEnum classifier declaration"));
                }
                attrs(
                    node,
                    &[
                        "xsi:type",
                        "name",
                        "serializable",
                        "instanceClassName",
                        "instanceTypeName",
                    ],
                )?;
                self.phase = Phase::Children(0);
            }
            Phase::Children(index) => {
                if let Some(child) = node.children.get(index) {
                    if let Some(literal) = lower_literal(node, child, observation)? {
                        self.literals.push(literal);
                    }
                    self.phase = Phase::Children(index + 1);
                } else {
                    self.phase = Phase::Construct;
                }
            }
            Phase::Construct => {
                if let Some(observation) = observation {
                    let source = &node.opening.source;
                    observation.serialized(&node.declaration, source)?;
                    if let Some(name) = node.attrs.get("name") {
                        observation.work(name.len().div_ceil(256).max(1), source)?;
                        observation.string(name.len(), source)?;
                    }
                }
                let declaration = node.declaration.clone();
                let name = required(node, "name")?;
                let serializable = boolean(node, "serializable", true)?;
                self.construction = Some(EnumerationConstruction::begin(
                    declaration,
                    name,
                    serializable,
                    std::mem::take(&mut self.literals),
                ));
                self.phase = Phase::Index;
            }
            Phase::Index => {
                let construction = self
                    .construction
                    .as_mut()
                    .ok_or_else(|| node.error("Missing enum construction"))?;
                let mut admit = |name: Option<&str>| {
                    if let Some(observation) = observation {
                        let source = &node.opening.source;
                        let admission = (|| {
                            observation.work(1, source)?;
                            if let Some(name) = name {
                                observation.work(name.len().div_ceil(256).max(1), source)?;
                                observation.string(name.len(), source)?;
                            }
                            observation.reserve(128, source)
                        })();
                        admission.map_err(|error| error.to_string())?;
                    }
                    Ok(())
                };
                if construction
                    .advance(&mut admit)
                    .map_err(|message| node.error(message))?
                {
                    self.output = Some(
                        self.construction
                            .take()
                            .ok_or_else(|| node.error("Missing complete enum construction"))?
                            .finish()
                            .map_err(|message| node.error(message))?,
                    );
                    self.phase = Phase::Payload;
                }
            }
            Phase::Payload => {
                if let Some(observation) = observation {
                    let source = &node.opening.source;
                    observation.work(node.source_payload.len().div_ceil(256).max(1), source)?;
                    observation.string(node.source_payload.len(), source)?;
                }
                self.output
                    .as_mut()
                    .ok_or_else(|| node.error("Missing enum before source retention"))?
                    .retain_source(node.source_payload.clone());
                self.phase = Phase::Complete;
            }
            Phase::Complete => return Ok(true),
        }
        Ok(matches!(self.phase, Phase::Complete))
    }
    pub(super) fn finish(self) -> Result<Enumeration, ImportError> {
        if self.failed {
            return Err(self.node.error("Enum lowering has failed"));
        }
        if !matches!(self.phase, Phase::Complete) {
            return Err(self.node.error("Enum lowering is incomplete"));
        }
        self.output
            .ok_or_else(|| self.node.error("Missing completed enumeration"))
    }
}
fn lower_literal(
    node: &Xml,
    child: &Xml,
    observation: Option<&ParserObservation<'_, '_>>,
) -> Result<Option<Literal>, ImportError> {
    if let Some(observation) = observation {
        observation.work(1, &node.opening.source)?;
    }
    if child.name == "eAnnotations" {
        return Ok(None);
    }
    if child.name != "eLiterals" {
        return Err(
            child.unsupported("Extended enum signature requires later generic/metadata support")
        );
    }
    // Read the original literal without copying its annotation subtree.
    let literal = child;
    if let Some(kind) = literal.attrs.get("xsi:type")
        && kind != "ecore:EEnumLiteral"
    {
        return Err(literal.at(
            "xsi:type",
            ErrorKind::InvalidDefinition,
            "Expected EEnumLiteral",
        ));
    }
    attrs(literal, &["xmi:id", "xsi:type", "name", "value", "literal"])?;
    if literal
        .children
        .iter()
        .any(|child| child.name != "eAnnotations")
    {
        return Err(literal.unsupported("Unsupported enum literal payload"));
    }
    let value = literal
        .attrs
        .get("value")
        .map_or(Ok(0), |value| {
            let parsed = crate::definitions::scalars::Builtin::EInt.parse(value)?;
            parsed
                .as_i64()
                .and_then(|value| i32::try_from(value).ok())
                .ok_or_else(|| "Expected an EInt scalar value".to_owned())
        })
        .map_err(|_| {
            literal.at(
                "value",
                ErrorKind::InvalidDefinition,
                "Enum numeric value must be an EInt",
            )
        })?;
    if let Some(observation) = observation {
        let source = &node.opening.source;
        observation.serialized(&literal.declaration, source)?;
        for name in ["name", "literal"] {
            if let Some(value) = literal.attrs.get(name) {
                observation.work(value.len().div_ceil(256).max(1), source)?;
                observation.string(value.len(), source)?;
            }
        }
        observation.reserve(32, source)?;
    }
    Ok(Some(Literal {
        declaration: literal.declaration.clone(),
        name: required(literal, "name")?,
        value,
        literal: literal.attrs.get("literal").cloned(),
    }))
}
