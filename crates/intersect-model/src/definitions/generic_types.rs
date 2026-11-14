//! Exact-identity generic expressions and bounded structural substitution.
//! These transformations do not establish bound satisfaction or executable types.
use crate::definitions::{
    bundle::SourceSpan,
    conversion::InvocationContext,
    ecore::{ErrorKind, ImportError},
    generic_resolution::{Binding, Resolver},
    generics::{GenericKind, GenericType},
    resources::DeclarationId,
    schema::{PreparationError, reserve_preparation, reserve_preparation_value},
};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RigidParameterId {
    pub source: crate::definitions::bundle::SourceId,
    pub declaration: usize,
    pub parameter: usize,
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Type {
    RigidParameter {
        identity: RigidParameterId,
    },
    Classifier {
        declaration: DeclarationId,
        arguments: Vec<Type>,
        raw: bool,
    },
    Parameter {
        declaration: DeclarationId,
    },
    Wildcard {
        upper: Option<Box<Type>>,
        lower: Option<Box<Type>>,
    },
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TypeUse {
    pub declaration: DeclarationId,
    pub span: SourceSpan,
    pub expression: Type,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ParameterBinding {
    pub parameter: DeclarationId,
    pub argument: Type,
}
/// Construct with new: deserialized maps cannot bypass duplicate/shape checks.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Substitution {
    bindings: BTreeMap<DeclarationId, Type>,
}
#[derive(Clone, Copy)]
enum Expansion {
    Transitive,
    Simultaneous,
    Copy,
}
const MAX_DEPTH: usize = 128;
const MAX_NODES: usize = 16_384;
#[path = "generic_substitution_preparation.rs"]
mod substitution_preparation;
pub(crate) use substitution_preparation::{ShapePreparation, SubstitutionPreparation};
pub(crate) fn shape_observed(
    ty: &Type,
    observation: &InvocationContext<'_>,
) -> Result<(), PreparationError> {
    let mut preparation = ShapePreparation::begin();
    while !preparation.advance(ty, observation)? {}
    preparation.finish()
}
impl Substitution {
    /// Structural validation only; callers must separately check formal ownership and bounds.
    pub fn new(entries: Vec<ParameterBinding>) -> Result<Self, String> {
        Self::new_observed(entries, &InvocationContext::default())
            .map_err(|error| error.to_string())
    }
    pub(crate) fn new_observed(
        entries: Vec<ParameterBinding>,
        observation: &InvocationContext<'_>,
    ) -> Result<Self, PreparationError> {
        observation.work(1).map_err(PreparationError::Observation)?;
        if entries.len() > MAX_NODES {
            return Err("Generic binding count budget exceeded".into());
        }
        let mut bindings = BTreeMap::new();
        for entry in entries {
            observation.work(1).map_err(PreparationError::Observation)?;
            shape_observed(&entry.argument, observation)?;
            if bindings.contains_key(&entry.parameter) {
                return Err("Duplicate generic substitution parameter".into());
            }
            reserve_preparation(observation, 128, &[])?;
            bindings.insert(entry.parameter, entry.argument);
        }
        Ok(Self { bindings })
    }
    pub fn entries(&self) -> Vec<ParameterBinding> {
        self.bindings
            .iter()
            .map(|(parameter, argument)| ParameterBinding {
                parameter: parameter.clone(),
                argument: argument.clone(),
            })
            .collect()
    }
    /// Follow mapped parameter chains; retain unmapped/identity parameters as open types.
    /// Never expand declaration bounds here: F-bounds are constraints, not substitutions.
    pub fn apply(&self, expression: &Type) -> Result<Type, String> {
        self.apply_observed(expression, &InvocationContext::default())
            .map_err(|error| error.to_string())
    }
    pub(crate) fn apply_observed(
        &self,
        expression: &Type,
        observation: &InvocationContext<'_>,
    ) -> Result<Type, PreparationError> {
        let mut preparation = SubstitutionPreparation::transitive();
        while !preparation.advance(expression, self, observation)? {}
        preparation.finish()
    }
    /// Borrow a complete unchanged expression; real replacements still use the
    /// existing checked expansion. This shares input data, never a semantic result.
    pub(crate) fn apply_view_observed<'input>(
        &self,
        expression: &'input Type,
        observation: &InvocationContext<'_>,
    ) -> Result<std::borrow::Cow<'input, Type>, PreparationError> {
        shape_observed(expression, observation)?;
        if self.bindings.is_empty() {
            // Observe the current input map; no parameter can be replaced by an
            // empty map. Shape validation above still traverses every node.
            observation.work(1).map_err(PreparationError::Observation)?;
            return Ok(std::borrow::Cow::Borrowed(expression));
        }
        if !self.touches_binding_observed(expression, observation)? {
            return Ok(std::borrow::Cow::Borrowed(expression));
        }
        self.apply_observed(expression, observation)
            .map(std::borrow::Cow::Owned)
    }
    fn touches_binding_observed(
        &self,
        expression: &Type,
        observation: &InvocationContext<'_>,
    ) -> Result<bool, PreparationError> {
        observation.work(1).map_err(PreparationError::Observation)?;
        match expression {
            Type::Parameter { declaration } => Ok(self.bindings.contains_key(declaration)),
            Type::RigidParameter { .. } => Ok(false),
            Type::Classifier { arguments, .. } => {
                for argument in arguments {
                    if self.touches_binding_observed(argument, observation)? {
                        return Ok(true);
                    }
                }
                Ok(false)
            }
            Type::Wildcard { upper, lower } => {
                for bound in [upper, lower].into_iter().flatten() {
                    if self.touches_binding_observed(bound, observation)? {
                        return Ok(true);
                    }
                }
                Ok(false)
            }
        }
    }
    /// Replace formal occurrences simultaneously for one inheritance edge.
    /// Actual arguments already belong to the receiver context and must not be
    /// reinterpreted as formals in the callee's environment.
    pub fn apply_simultaneous(&self, expression: &Type) -> Result<Type, String> {
        self.apply_simultaneous_observed(expression, &InvocationContext::default())
            .map_err(|error| error.to_string())
    }
    pub(crate) fn apply_simultaneous_observed(
        &self,
        expression: &Type,
        observation: &InvocationContext<'_>,
    ) -> Result<Type, PreparationError> {
        let mut preparation = SubstitutionPreparation::simultaneous();
        while !preparation.advance(expression, self, observation)? {}
        preparation.finish()
    }
}
#[cfg(test)]
fn normalize_observed(
    syntax: GenericType,
    bindings: &BTreeMap<&DeclarationId, &Binding>,
    observation: &crate::definitions::ecore::ParserObservation<'_, '_>,
) -> Result<Type, ImportError> {
    let mut preparation = normalization_preparation::NormalizationPreparation::begin(syntax);
    while !preparation.advance(bindings, observation)? {}
    Ok(preparation.finish()?.expression)
}

impl Resolver {
    /// Resolve retained source before normalization. A deserialized TypeUse is only data.
    pub fn type_expression(&self, id: &DeclarationId) -> Result<TypeUse, ImportError> {
        self.type_expression_context(
            id,
            &crate::definitions::ecore::ParserObservation::new(&InvocationContext::default()),
        )
    }
    /// Observe source checking, normalization and shape validation through their
    /// shared synchronous implementation. This does not provide host yielding.
    pub fn type_expression_observed(
        &self,
        id: &DeclarationId,
        context: &InvocationContext<'_>,
    ) -> Result<TypeUse, crate::definitions::ecore::ObservedImportError> {
        context
            .checkpoint()
            .map_err(crate::definitions::ecore::ObservedImportError::Observation)?;
        let observation = crate::definitions::ecore::ParserObservation::new(context);
        observation.finish(self.type_expression_context(id, &observation))
    }
    pub(crate) fn type_expression_context(
        &self,
        id: &DeclarationId,
        observation: &crate::definitions::ecore::ParserObservation<'_, '_>,
    ) -> Result<TypeUse, ImportError> {
        let mut preparation = TypeExpressionPreparation::begin();
        while !preparation.advance(self, id, observation)? {}
        preparation.finish(id)
    }
}

/// Exact retained-source selections, never caller-asserted type expressions.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceBinding {
    pub parameter: DeclarationId,
    pub argument: DeclarationId,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SubstitutedUse {
    pub original: TypeUse,
    pub arguments: Vec<TypeUse>,
    pub bindings: Vec<ParameterBinding>,
    pub expression: Type,
}
impl Resolver {
    /// Resolve each formal and argument from retained sources before substitution.
    /// This does not verify application arity, formal bounds or runtime conformance.
    pub fn substitute_type(
        &self,
        declaration: &DeclarationId,
        selections: &[SourceBinding],
    ) -> Result<SubstitutedUse, ImportError> {
        let original = self.type_expression(declaration)?;
        if selections.len() > MAX_NODES {
            return Err(ImportError::new(
                ErrorKind::ResourceLimit,
                &original.span,
                "Generic binding count budget exceeded",
            ));
        }
        let mut bindings = Vec::with_capacity(selections.len());
        let mut arguments = Vec::with_capacity(selections.len());
        for selection in selections {
            let formal = self.resolve_parameter(&selection.parameter)?;
            let argument = self.type_expression(&selection.argument)?;
            bindings.push(ParameterBinding {
                parameter: formal.syntax.declaration,
                argument: argument.expression.clone(),
            });
            arguments.push(argument);
        }
        let environment = Substitution::new(bindings).map_err(|message| {
            ImportError::new(ErrorKind::InvalidDefinition, &original.span, message)
        })?;
        let expression = environment.apply(&original.expression).map_err(|message| {
            ImportError::new(ErrorKind::InvalidDefinition, &original.span, message)
        })?;
        Ok(SubstitutedUse {
            original,
            arguments,
            bindings: environment.entries(),
            expression,
        })
    }
}

/// Stable identity for a structurally closed type. Not a bound-check certificate.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClosedType {
    expression: Type,
    identity: String,
}
impl ClosedType {
    pub fn new(expression: Type) -> Result<Self, String> {
        let runtime = RuntimeType::new(expression)?;
        if runtime.has_raw() {
            return Err("Runtime type identity requires closed classifier arguments; raw, parameter and wildcard types remain open".into());
        }
        Ok(Self {
            expression: match std::sync::Arc::try_unwrap(runtime.expression) {
                Ok(value) => value,
                Err(value) => value.as_ref().clone(),
            },
            identity: runtime.identity.to_string(),
        })
    }
    pub fn expression(&self) -> &Type {
        &self.expression
    }
    pub fn identity(&self) -> &str {
        &self.identity
    }
}
impl Serialize for ClosedType {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.expression().serialize(serializer)
    }
}
impl<'de> Deserialize<'de> for ClosedType {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Self::new(Type::deserialize(deserializer)?).map_err(serde::de::Error::custom)
    }
}

/// Concrete classifier identity, permitting explicit raw applications but never
/// free parameters or wildcards. Construction validates shape, not source/bounds.
/// Closed applications retain the existing identity encoding.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RuntimeType {
    expression: std::sync::Arc<Type>,
    identity: std::sync::Arc<str>,
}

// An operation-scoped table of complete immutable type inputs, not bounds,
// memberships, provider results or other semantic outcomes.
#[derive(Default, Debug)]
pub(crate) struct RuntimeInputCache {
    values: std::collections::BTreeMap<std::sync::Arc<str>, Vec<RuntimeType>>,
    entries: usize,
}
pub(crate) type RuntimeExpressionInputIndex =
    std::collections::BTreeMap<std::sync::Arc<Type>, RuntimeType>;
impl RuntimeType {
    pub(crate) fn retained_clone_observed(
        &self,
        observation: &InvocationContext<'_>,
    ) -> Result<Self, PreparationError> {
        reserve_preparation(observation, 32, &[])?;
        Ok(self.clone())
    }
    pub(crate) fn from_input_indexed_observed(
        expression: std::borrow::Cow<'_, Type>,
        cache: &std::cell::RefCell<RuntimeInputCache>,
        index: &std::cell::RefCell<RuntimeExpressionInputIndex>,
        observation: &InvocationContext<'_>,
    ) -> Result<Self, PreparationError> {
        // Validate every input freshly; the index stores complete immutable
        // expression/identity metadata, never source/bound/membership results.
        Self::validate_expression_observed(expression.as_ref(), observation)?;
        observation.work(1).map_err(PreparationError::Observation)?;
        {
            let entries = index.try_borrow().map_err(|_| {
                PreparationError::Schema("Runtime expression inputs are already borrowed".into())
            })?;
            if let Some(input) = entries.get(expression.as_ref())
                && input.expression() == expression.as_ref()
            {
                return input.retained_clone_observed(observation);
            }
        }
        let input = Self::from_input_observed(expression, cache, observation)?;
        let mut entries = index.try_borrow_mut().map_err(|_| {
            PreparationError::Schema("Runtime expression inputs are already borrowed".into())
        })?;
        if entries.len() < 4096 {
            reserve_preparation(observation, 128 + 32 + 32, &[])?;
            entries.insert(input.expression.clone(), input.clone());
        }
        Ok(input)
    }
    pub(crate) fn from_input_observed(
        expression: std::borrow::Cow<'_, Type>,
        cache: &std::cell::RefCell<RuntimeInputCache>,
        observation: &InvocationContext<'_>,
    ) -> Result<Self, PreparationError> {
        let identity = Self::canonical_identity_observed(expression.as_ref(), observation)?;
        let mut cache = cache.try_borrow_mut().map_err(|_| {
            PreparationError::Schema("Runtime input metadata is already borrowed".into())
        })?;
        if let Some(values) = cache.values.get(identity.as_ref()) {
            for value in values {
                observation.work(1).map_err(PreparationError::Observation)?;
                if value.expression() == expression.as_ref()
                    && value.identity() == identity.as_ref()
                {
                    return value.retained_clone_observed(observation);
                }
            }
        }
        let retained = cache.entries < 4096;
        if retained {
            reserve_preparation(observation, 128 + 32, &[])?;
        }
        if matches!(&expression, std::borrow::Cow::Borrowed(_)) {
            reserve_preparation_value(expression.as_ref(), observation)?;
        }
        reserve_preparation(observation, 32, &[])?;
        // Allocate a new vector before publishing a new table key.
        let missing = retained && !cache.values.contains_key(identity.as_ref());
        let mut fresh = Vec::new();
        if missing {
            fresh.try_reserve_exact(1).map_err(|_| {
                PreparationError::Observation("Runtime input allocation failed".into())
            })?;
        }
        if retained && !missing {
            cache
                .values
                .get_mut(identity.as_ref())
                .ok_or("Missing runtime input key")?
                .try_reserve_exact(1)
                .map_err(|_| {
                    PreparationError::Observation("Runtime input allocation failed".into())
                })?;
        }
        let value = Self {
            expression: std::sync::Arc::new(expression.into_owned()),
            identity,
        };
        if retained {
            if missing {
                fresh.push(value.clone());
                cache.values.insert(value.identity.clone(), fresh);
            } else {
                cache
                    .values
                    .get_mut(value.identity())
                    .ok_or("Missing runtime input key")?
                    .push(value.clone());
            }
            cache.entries += 1;
        }
        Ok(value)
    }

    pub fn new(expression: Type) -> Result<Self, String> {
        Self::new_observed(expression, &InvocationContext::default())
            .map_err(|error| error.to_string())
    }

    /// Construct the existing exact identity without an encoded JSON buffer.
    /// The caller owns admission for any input copy; serialization remains a
    /// synchronous interval with a checkpoint before every emitted hash chunk.
    pub(crate) fn new_observed(
        expression: Type,
        observation: &InvocationContext<'_>,
    ) -> Result<Self, PreparationError> {
        let identity = Self::canonical_identity_observed(&expression, observation)?;
        // The canonical factory already precharges the 128-byte runtime
        // metadata record, covering the immutable expression allocation.
        Ok(Self {
            expression: std::sync::Arc::new(expression),
            identity,
        })
    }
    // Complete canonical metadata only. Source membership and bounds are always
    // checked separately by their shared authorities.
    pub(crate) fn canonical_identity_observed(
        expression: &Type,
        observation: &InvocationContext<'_>,
    ) -> Result<std::sync::Arc<str>, PreparationError> {
        use sha2::{Digest, Sha256};
        struct Writer<'a, 'b> {
            digest: Sha256,
            observation: &'a InvocationContext<'b>,
            failure: Option<String>,
        }
        impl std::io::Write for Writer<'_, '_> {
            fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
                if let Err(message) = self.observation.work(bytes.len().div_ceil(256).max(1)) {
                    self.failure = Some(message);
                    return Err(std::io::Error::other(
                        "Runtime identity observation interrupted",
                    ));
                }
                self.digest.update(bytes);
                Ok(bytes.len())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        Self::validate_expression_observed(expression, observation)?;
        reserve_preparation(observation, 128, &[])?;
        observation.work(1).map_err(PreparationError::Observation)?;
        let mut writer = Writer {
            digest: Sha256::new(),
            observation,
            failure: None,
        };
        writer.digest.update(b"intersect.generic.type.v1\0");
        if let Err(error) = serde_json::to_writer(&mut writer, expression) {
            return Err(match writer.failure {
                Some(message) => PreparationError::Observation(message),
                None => PreparationError::Schema(error.to_string()),
            });
        }
        observation.work(1).map_err(PreparationError::Observation)?;
        // Existing v1 prefix20 + SHA hex64, plus the logical string slot64.
        reserve_preparation(observation, 64 + 20 + 64, &[])?;
        let digest = writer.digest.finalize();
        const PREFIX: &[u8] = b"intersect:generic:1:";
        const HEX: &[u8] = b"0123456789abcdef";
        let mut identity = [0_u8; PREFIX.len() + 64];
        identity[..PREFIX.len()].copy_from_slice(PREFIX);
        for (index, byte) in digest.iter().enumerate() {
            identity[PREFIX.len() + index * 2] = HEX[(byte >> 4) as usize];
            identity[PREFIX.len() + index * 2 + 1] = HEX[(byte & 15) as usize];
        }
        let identity = std::str::from_utf8(&identity)
            .map_err(|error| PreparationError::Schema(error.to_string()))?;
        Ok(std::sync::Arc::from(identity))
    }

    /// Borrowed structural/canonical validation shared with construction. This
    /// establishes neither source membership nor bounds, and retains no identity.
    pub(crate) fn validate_expression_observed(
        expression: &Type,
        observation: &InvocationContext<'_>,
    ) -> Result<(), PreparationError> {
        shape_observed(expression, observation)?;
        let mut pending = Vec::new();
        reserve_preparation(observation, 32, &[])?;
        pending.push(expression);
        while let Some(ty) = pending.pop() {
            observation.work(1).map_err(PreparationError::Observation)?;
            let Type::Classifier {
                declaration,
                arguments,
                ..
            } = ty
            else {
                return Err(
                    "Runtime classifier identity cannot contain parameter or wildcard types".into(),
                );
            };
            observation
                .work(declaration.source.uri.len().div_ceil(256).max(1))
                .map_err(PreparationError::Observation)?;
            let mut invalid =
                declaration.path.is_empty() || declaration.source.uri.trim().is_empty();
            if !invalid {
                for character in declaration.source.uri.chars() {
                    observation.work(1).map_err(PreparationError::Observation)?;
                    if character.is_control() {
                        invalid = true;
                        break;
                    }
                }
            }
            if !invalid {
                invalid = declaration.source.sha256.len() != 64;
                if !invalid {
                    for byte in declaration.source.sha256.bytes() {
                        observation.work(1).map_err(PreparationError::Observation)?;
                        if !(byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)) {
                            invalid = true;
                            break;
                        }
                    }
                }
            }
            if invalid {
                return Err(
                    "Runtime type identity requires canonical exact declaration identities".into(),
                );
            }
            for argument in arguments {
                observation.work(1).map_err(PreparationError::Observation)?;
                reserve_preparation(observation, 32, &[])?;
                pending.push(argument);
            }
        }
        Ok(())
    }

    pub fn expression(&self) -> &Type {
        &self.expression
    }
    pub fn identity(&self) -> &str {
        &self.identity
    }
    pub fn has_raw(&self) -> bool {
        self.has_raw_observed(&InvocationContext::default())
            .unwrap_or(false)
    }
    pub(crate) fn has_raw_observed(
        &self,
        observation: &InvocationContext<'_>,
    ) -> Result<bool, PreparationError> {
        Self::expression_has_raw_observed(&self.expression, observation)
    }
    pub(crate) fn expression_has_raw_observed(
        expression: &Type,
        observation: &InvocationContext<'_>,
    ) -> Result<bool, PreparationError> {
        let mut pending = Vec::new();
        reserve_preparation(observation, 32, &[])?;
        pending.push(expression);
        while let Some(Type::Classifier { raw, arguments, .. }) = pending.pop() {
            observation.work(1).map_err(PreparationError::Observation)?;
            if *raw {
                return Ok(true);
            }
            for argument in arguments {
                observation.work(1).map_err(PreparationError::Observation)?;
                reserve_preparation(observation, 32, &[])?;
                pending.push(argument);
            }
        }
        Ok(false)
    }

    /// Predicate counterpart of construction, without copying or hashing an
    /// expression that the caller only needs to classify. Observation failures
    /// remain errors rather than becoming a nonconcrete classification.
    pub(crate) fn expression_is_concrete_observed(
        expression: &Type,
        observation: &InvocationContext<'_>,
    ) -> Result<bool, PreparationError> {
        match Self::validate_expression_observed(expression, observation) {
            Ok(()) => Ok(true),
            Err(PreparationError::Schema(_)) => Ok(false),
            Err(error @ PreparationError::Observation(_)) => Err(error),
        }
    }
}
impl From<&ClosedType> for RuntimeType {
    fn from(ty: &ClosedType) -> Self {
        Self {
            expression: std::sync::Arc::new(ty.expression.clone()),
            identity: std::sync::Arc::from(ty.identity.as_str()),
        }
    }
}
impl From<ClosedType> for RuntimeType {
    fn from(ty: ClosedType) -> Self {
        Self {
            expression: std::sync::Arc::new(ty.expression),
            identity: std::sync::Arc::from(ty.identity),
        }
    }
}
impl Serialize for RuntimeType {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.expression.serialize(serializer)
    }
}
impl<'de> Deserialize<'de> for RuntimeType {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Self::new(Type::deserialize(deserializer)?).map_err(serde::de::Error::custom)
    }
}

impl AsRef<Type> for ClosedType {
    fn as_ref(&self) -> &Type {
        self.expression()
    }
}
impl AsRef<Type> for RuntimeType {
    fn as_ref(&self) -> &Type {
        self.expression()
    }
}

#[cfg(test)]
mod normalization_controls {
    use super::*;
    use crate::definitions::{
        bundle::SourceId,
        compiler::xml_projection_controls::assert_lowering_receipt,
        ecore::{ObservedImportError, ParserObservation},
        generic_resolution::TargetKind,
        generics::TypeReference,
    };

    #[test]
    fn observed_normalization_retains_parameter_arguments_and_boxed_wildcard_bounds() {
        let source = SourceId {
            uri: "memory:x".into(),
            sha256: "0".repeat(64),
        };
        let id = |path| DeclarationId {
            source: source.clone(),
            path,
        };
        let span = SourceSpan {
            source: source.clone(),
            start: 0,
            end: 1,
        };
        let syntax = |declaration, value| GenericType {
            declaration,
            span: span.clone(),
            source_payload: String::new(),
            value,
        };
        let reference = TypeReference {
            href: "#//unused".into(),
            kind_hint: None,
            span: span.clone(),
        };
        let parameter = syntax(
            id(vec![0, 0]),
            GenericKind::Parameter {
                reference: reference.clone(),
            },
        );
        let bound = syntax(
            id(vec![0, 1, 0]),
            GenericKind::Classifier {
                reference: reference.clone(),
                arguments: vec![],
            },
        );
        let wildcard = syntax(
            id(vec![0, 1]),
            GenericKind::Wildcard {
                upper: Some(Box::new(bound)),
                lower: None,
            },
        );
        let root = syntax(
            id(vec![0]),
            GenericKind::Classifier {
                reference,
                arguments: vec![parameter, wildcard],
            },
        );
        let rows = [
            Binding {
                usage: id(vec![0]),
                target: id(vec![1]),
                target_kind: TargetKind::Class,
                raw: false,
            },
            Binding {
                usage: id(vec![0, 0]),
                target: id(vec![1, 0]),
                target_kind: TargetKind::Parameter,
                raw: false,
            },
            Binding {
                usage: id(vec![0, 1, 0]),
                target: id(vec![1]),
                target_kind: TargetKind::Class,
                raw: false,
            },
        ];
        let bindings = rows.iter().map(|row| (&row.usage, row)).collect();
        let expected = Type::Classifier {
            declaration: id(vec![1]),
            raw: false,
            arguments: vec![
                Type::Parameter {
                    declaration: id(vec![1, 0]),
                },
                Type::Wildcard {
                    upper: Some(Box::new(Type::Classifier {
                        declaration: id(vec![1]),
                        arguments: vec![],
                        raw: false,
                    })),
                    lower: None,
                },
            ],
        };
        // Direct normalization only: synthetic checked rows do not certify
        // source resolution. Four records512, IDs4399, two slots64 and box128.
        // Four node visits + three lookups + copied ID work50 + two argument
        // edges + two wildcard slots =61; all copied targets keep exact paths.
        assert_lowering_receipt(
            &expected,
            5103,
            61,
            &[128, 1541, 1669, 3242, 3274, 3402, 3530, 4943, 5071, 5103],
            |probe| {
                let context = InvocationContext::new(probe.clone());
                let observation = ParserObservation::new(&context);
                observation.finish(normalize_observed(root.clone(), &bindings, &observation))
            },
        );
        let expected = ImportError::new(
            ErrorKind::InvalidDefinition,
            &span,
            "Missing checked generic binding",
        );
        assert_lowering_receipt(&expected, 128, 2, &[128], |probe| {
            let context = InvocationContext::new(probe.clone());
            let observation = ParserObservation::new(&context);
            match observation.finish(normalize_observed(
                root.clone(),
                &BTreeMap::new(),
                &observation,
            )) {
                Err(ObservedImportError::Import(error)) => Ok(error),
                Err(error) => Err(error),
                Ok(_) => panic!("missing normalization binding was accepted"),
            }
        });
    }
    fn runtime_input_fixture() -> Type {
        let source = SourceId {
            uri: "memory:runtime-input-α.ecore".into(),
            sha256: "a".repeat(64),
        };
        Type::Classifier {
            declaration: DeclarationId {
                source: source.clone(),
                path: vec![0],
            },
            arguments: vec![Type::Classifier {
                declaration: DeclarationId {
                    source,
                    path: vec![1],
                },
                arguments: vec![],
                raw: true,
            }],
            raw: false,
        }
    }
    #[test]
    fn complete_runtime_type_clones_share_immutable_values_and_preserve_canonical_dto() {
        use sha2::{Digest, Sha256};
        assert!(std::mem::size_of::<Type>() + 2 * std::mem::size_of::<usize>() <= 128);
        let expression = runtime_input_fixture();
        let value = RuntimeType::new(expression.clone()).unwrap();
        let mut digest = Sha256::new();
        digest.update(b"intersect.generic.type.v1\0");
        digest.update(serde_json::to_vec(&expression).unwrap());
        let expected = format!("intersect:generic:1:{:x}", digest.finalize());
        assert_eq!(value.identity(), expected);
        assert_eq!(value.expression(), &expression);
        assert_eq!(
            serde_json::to_value(&value).unwrap(),
            serde_json::to_value(&expression).unwrap()
        );
        assert!(value.has_raw());
        let copy = value.clone();
        assert_eq!(copy, value);
        assert!(std::sync::Arc::ptr_eq(&copy.expression, &value.expression));
        assert!(std::sync::Arc::ptr_eq(&copy.identity, &value.identity));
        let wire = serde_json::to_string(&value).unwrap();
        let replay: RuntimeType = serde_json::from_str(&wire).unwrap();
        assert_eq!(replay, value);
        assert!(!std::sync::Arc::ptr_eq(
            &replay.expression,
            &value.expression
        ));
        fn send_sync<T: Send + Sync>() {}
        send_sync::<RuntimeType>();
    }
    #[test]
    fn runtime_expression_index_preserves_complete_metadata_and_validates_every_read() {
        use crate::definitions::conversion::{InvocationObserver, ProviderId};
        use std::{
            cell::{Cell, RefCell},
            rc::Rc,
        };
        #[derive(Default)]
        struct Meter {
            work: Cell<usize>,
            bytes: Cell<usize>,
            cancelled: Cell<bool>,
        }
        impl InvocationObserver for Meter {
            fn before_invoke(&self, _: &ProviderId) -> Result<(), String> {
                Err("No provider expected".into())
            }
            fn work(&self, n: usize) -> Result<(), String> {
                if self.cancelled.get() {
                    return Err("Runtime input cancelled".into());
                }
                self.work.set(self.work.get() + n);
                Ok(())
            }
            fn retain_storage(&self, n: usize) -> Result<(), String> {
                if self.cancelled.get() {
                    return Err("Runtime input cancelled".into());
                }
                self.bytes.set(self.bytes.get() + n);
                Ok(())
            }
        }
        let expression = runtime_input_fixture();
        let cache = RefCell::new(RuntimeInputCache::default());
        let index = RefCell::new(RuntimeExpressionInputIndex::default());
        let context = InvocationContext::default();
        let first = RuntimeType::from_input_indexed_observed(
            std::borrow::Cow::Borrowed(&expression),
            &cache,
            &index,
            &context,
        )
        .unwrap();
        assert_eq!(first, RuntimeType::new(expression.clone()).unwrap());
        let meter = Rc::new(Meter::default());
        let operation = InvocationContext::new(meter.clone());
        let second = RuntimeType::from_input_indexed_observed(
            std::borrow::Cow::Borrowed(&expression),
            &cache,
            &index,
            &operation,
        )
        .unwrap();
        assert!(std::sync::Arc::ptr_eq(
            &first.expression,
            &second.expression
        ));
        assert!(std::sync::Arc::ptr_eq(&first.identity, &second.identity));
        assert_eq!(
            serde_json::to_value(&first).unwrap(),
            serde_json::to_value(&second).unwrap()
        );
        assert!(meter.work.get() > 0);
        let original = Rc::new(Meter::default());
        let third = RuntimeType::from_input_observed(
            std::borrow::Cow::Borrowed(&expression),
            &cache,
            &InvocationContext::new(original.clone()),
        )
        .unwrap();
        assert_eq!(third, first);
        assert!(meter.work.get() < original.work.get());
        assert!(meter.bytes.get() < original.bytes.get());
        meter.cancelled.set(true);
        assert!(
            RuntimeType::from_input_indexed_observed(
                std::borrow::Cow::Borrowed(&expression),
                &cache,
                &index,
                &operation
            )
            .is_err()
        );
        assert_eq!(index.borrow().len(), 1);
        assert!(
            RuntimeType::from_input_indexed_observed(
                std::borrow::Cow::Borrowed(&expression),
                &cache,
                &index,
                &context
            )
            .is_ok()
        );
        let mut invalid = expression.clone();
        let Type::Classifier { declaration, .. } = &mut invalid else {
            panic!("Classifier expected");
        };
        declaration.source.sha256 = "BAD".into();
        assert_eq!(
            RuntimeType::from_input_indexed_observed(
                std::borrow::Cow::Borrowed(&invalid),
                &cache,
                &index,
                &context
            )
            .unwrap_err(),
            RuntimeType::new_observed(invalid, &context).unwrap_err()
        );
        let mut other = expression.clone();
        let Type::Classifier { declaration, .. } = &mut other else {
            panic!("Classifier expected");
        };
        declaration.path.push(9);
        let other = RuntimeType::new(other).unwrap();
        index.borrow_mut().insert(first.expression.clone(), other);
        let repaired = RuntimeType::from_input_indexed_observed(
            std::borrow::Cow::Borrowed(&expression),
            &cache,
            &index,
            &context,
        )
        .unwrap();
        assert_eq!(repaired, first);
        assert_eq!(index.borrow().get(&expression).unwrap(), &first);
        let held = index.borrow_mut();
        assert!(
            RuntimeType::from_input_indexed_observed(
                std::borrow::Cow::Borrowed(&expression),
                &cache,
                &index,
                &context
            )
            .is_err()
        );
        drop(held);
        assert!(
            RuntimeType::from_input_indexed_observed(
                std::borrow::Cow::Borrowed(&expression),
                &cache,
                &index,
                &context
            )
            .is_ok()
        );
    }
    #[test]
    fn runtime_expression_index_capacity_keeps_complete_uncached_fallback() {
        use std::cell::RefCell;
        let expression = runtime_input_fixture();
        let cache = RefCell::new(RuntimeInputCache::default());
        let index = RefCell::new(RuntimeExpressionInputIndex::default());
        for number in 0..4096 {
            let mut distinct = expression.clone();
            let Type::Classifier { declaration, .. } = &mut distinct else {
                panic!("Classifier expected");
            };
            declaration.path.extend([8, number]);
            let value = RuntimeType::new(distinct).unwrap();
            index.borrow_mut().insert(value.expression.clone(), value);
        }
        let value = RuntimeType::from_input_indexed_observed(
            std::borrow::Cow::Borrowed(&expression),
            &cache,
            &index,
            &InvocationContext::default(),
        )
        .unwrap();
        assert_eq!(index.borrow().len(), 4096);
        assert!(!index.borrow().contains_key(&expression));
        assert_eq!(value, RuntimeType::new(expression.clone()).unwrap());
        assert_eq!(
            serde_json::to_value(&value).unwrap(),
            serde_json::to_value(&expression).unwrap()
        );
        let replay = RuntimeType::from_input_indexed_observed(
            std::borrow::Cow::Borrowed(&expression),
            &cache,
            &index,
            &InvocationContext::default(),
        )
        .unwrap();
        assert_eq!(replay, value);
        assert_eq!(index.borrow().len(), 4096);
    }
    #[test]
    fn runtime_expression_index_precharges_cold_insertion_and_each_interruption() {
        use crate::definitions::conversion::{InvocationObserver, ProviderId};
        use std::{
            cell::{Cell, RefCell},
            rc::Rc,
        };
        struct Meter {
            bytes: Cell<usize>,
            calls: Cell<usize>,
            limit: usize,
            stop: usize,
        }
        impl Meter {
            fn step(&self) -> Result<(), String> {
                let n = self.calls.get();
                self.calls.set(n + 1);
                if n == self.stop {
                    Err("Input cancelled".into())
                } else {
                    Ok(())
                }
            }
        }
        impl InvocationObserver for Meter {
            fn before_invoke(&self, _: &ProviderId) -> Result<(), String> {
                Err("No provider expected".into())
            }
            fn work(&self, _: usize) -> Result<(), String> {
                self.step()
            }
            fn retain_storage(&self, n: usize) -> Result<(), String> {
                self.step()?;
                let next = self.bytes.get() + n;
                if next > self.limit {
                    return Err("Input storage denied".into());
                }
                self.bytes.set(next);
                Ok(())
            }
        }
        let expression = runtime_input_fixture();
        let run = |limit, stop| {
            let cache = RefCell::new(RuntimeInputCache::default());
            let index = RefCell::new(RuntimeExpressionInputIndex::default());
            let meter = Rc::new(Meter {
                bytes: Cell::new(0),
                calls: Cell::new(0),
                limit,
                stop,
            });
            let value = RuntimeType::from_input_indexed_observed(
                std::borrow::Cow::Borrowed(&expression),
                &cache,
                &index,
                &InvocationContext::new(meter.clone()),
            );
            let len = index.borrow().len();
            (value, meter.bytes.get(), meter.calls.get(), len)
        };
        let (complete, bytes, calls, count) = run(usize::MAX, usize::MAX);
        assert!(complete.is_ok());
        assert_eq!(count, 1);
        assert!(run(bytes, usize::MAX).0.is_ok());
        let denied = run(bytes - 1, usize::MAX);
        assert!(denied.0.is_err());
        assert_eq!(denied.3, 0);
        for stop in 0..calls {
            let cancelled = run(usize::MAX, stop);
            assert!(cancelled.0.is_err());
            assert_eq!(cancelled.3, 0);
        }
        assert!(run(bytes, usize::MAX).0.is_ok());
    }
    #[test]
    fn complete_runtime_input_table_checks_cold_limits_every_interruption_and_owned_fallback() {
        use crate::definitions::conversion::{InvocationObserver, ProviderId};
        use std::{
            cell::{Cell, RefCell},
            rc::Rc,
        };
        struct Meter {
            bytes: Cell<usize>,
            calls: Cell<usize>,
            limit: usize,
            stop: usize,
        }
        impl Meter {
            fn step(&self) -> Result<(), String> {
                let n = self.calls.get();
                self.calls.set(n + 1);
                if n == self.stop {
                    Err("runtime input cancelled".into())
                } else {
                    Ok(())
                }
            }
        }
        impl InvocationObserver for Meter {
            fn before_invoke(&self, _: &ProviderId) -> Result<(), String> {
                Err("no providers".into())
            }
            fn work(&self, _: usize) -> Result<(), String> {
                self.step()
            }
            fn retain_storage(&self, bytes: usize) -> Result<(), String> {
                self.step()?;
                let n = self
                    .bytes
                    .get()
                    .checked_add(bytes)
                    .filter(|n| *n <= self.limit)
                    .ok_or("runtime input denied")?;
                self.bytes.set(n);
                Ok(())
            }
        }
        let expression = runtime_input_fixture();
        let expected = RuntimeType::new(expression.clone()).unwrap();
        let run = |limit, stop| {
            let cache = RefCell::new(RuntimeInputCache::default());
            let meter = Rc::new(Meter {
                bytes: Cell::new(0),
                calls: Cell::new(0),
                limit,
                stop,
            });
            let result = RuntimeType::from_input_observed(
                std::borrow::Cow::Borrowed(&expression),
                &cache,
                &InvocationContext::new(meter.clone()),
            );
            if let Ok(value) = &result {
                assert_eq!(value, &expected);
                assert_eq!(
                    serde_json::to_value(value).unwrap(),
                    serde_json::to_value(&expected).unwrap()
                );
            }
            let entries = cache.borrow().entries;
            (result, meter.bytes.get(), meter.calls.get(), entries)
        };
        let (complete, bytes, calls, count) = run(usize::MAX, usize::MAX);
        assert!(complete.is_ok());
        assert_eq!(count, 1);
        assert!(run(bytes, usize::MAX).0.is_ok());
        assert!(run(bytes - 1, usize::MAX).0.is_err());
        for stop in 0..calls {
            let denied = run(usize::MAX, stop);
            assert!(denied.0.is_err());
            assert_eq!(denied.3, 0);
            assert!(run(bytes, usize::MAX).0.is_ok());
        }
        let cache = RefCell::new(RuntimeInputCache::default());
        let context = InvocationContext::default();
        let first = RuntimeType::from_input_observed(
            std::borrow::Cow::Borrowed(&expression),
            &cache,
            &context,
        )
        .unwrap();
        let meter = Rc::new(Meter {
            bytes: Cell::new(0),
            calls: Cell::new(0),
            limit: usize::MAX,
            stop: usize::MAX,
        });
        let second = RuntimeType::from_input_observed(
            std::borrow::Cow::Borrowed(&expression),
            &cache,
            &InvocationContext::new(meter.clone()),
        )
        .unwrap();
        assert_eq!(second, first);
        assert!(std::sync::Arc::ptr_eq(
            &first.expression,
            &second.expression
        ));
        assert!(std::sync::Arc::ptr_eq(&first.identity, &second.identity));
        assert!(meter.calls.get() > 0);
        assert!(meter.bytes.get() < bytes);
        assert_eq!(cache.borrow().entries, 1);
        {
            let guard = cache.borrow();
            assert!(
                RuntimeType::from_input_observed(
                    std::borrow::Cow::Borrowed(&expression),
                    &cache,
                    &context
                )
                .is_err()
            );
            drop(guard);
        }
        let fallback = RefCell::new(RuntimeInputCache {
            entries: 4096,
            ..Default::default()
        });
        let fallback_value = RuntimeType::from_input_observed(
            std::borrow::Cow::Borrowed(&expression),
            &fallback,
            &context,
        )
        .unwrap();
        assert_eq!(fallback_value, expected);
        assert_eq!(fallback.borrow().entries, 4096);
        assert!(fallback.borrow().values.is_empty());
        assert!(!std::sync::Arc::ptr_eq(
            &fallback_value.expression,
            &first.expression
        ));
        let mut invalid = expression.clone();
        let Type::Classifier { declaration, .. } = &mut invalid else {
            panic!("classifier")
        };
        declaration.source.sha256 = "BAD".into();
        let fresh = RuntimeType::new_observed(invalid.clone(), &context).unwrap_err();
        assert_eq!(
            RuntimeType::from_input_observed(
                std::borrow::Cow::Borrowed(&invalid),
                &cache,
                &context
            )
            .unwrap_err(),
            fresh
        );
        let mut wrong = expression.clone();
        let Type::Classifier { declaration, .. } = &mut wrong else {
            panic!("classifier")
        };
        declaration.path.push(7);
        let wrong = RuntimeType::new(wrong).unwrap();
        cache
            .borrow_mut()
            .values
            .insert(first.identity.clone(), vec![wrong]);
        let repaired = RuntimeType::from_input_observed(
            std::borrow::Cow::Borrowed(&expression),
            &cache,
            &context,
        )
        .unwrap();
        assert_eq!(repaired, expected);
        assert!(!std::sync::Arc::ptr_eq(
            &repaired.expression,
            &first.expression
        ));
    }
}

#[cfg(test)]
mod substitution_input_view_controls {
    use super::*;
    use crate::definitions::{
        bundle::SourceId,
        conversion::{InvocationObserver, ProviderId},
    };
    use std::{cell::Cell, rc::Rc};
    #[derive(Default)]
    struct Meter {
        work: Cell<usize>,
        bytes: Cell<usize>,
        cancelled: Cell<bool>,
    }
    impl InvocationObserver for Meter {
        fn before_invoke(&self, _: &ProviderId) -> Result<(), String> {
            Err("No provider expected".into())
        }
        fn checkpoint(&self) -> Result<(), String> {
            if self.cancelled.get() {
                Err("View cancelled".into())
            } else {
                Ok(())
            }
        }
        fn work(&self, n: usize) -> Result<(), String> {
            self.checkpoint()?;
            self.work.set(self.work.get() + n);
            Ok(())
        }
        fn retain_storage(&self, n: usize) -> Result<(), String> {
            self.checkpoint()?;
            self.bytes.set(self.bytes.get() + n);
            Ok(())
        }
    }
    fn id(index: usize) -> DeclarationId {
        DeclarationId {
            source: SourceId {
                uri: "memory:unchanged-α.ecore".into(),
                sha256: "a".repeat(64),
            },
            path: vec![index],
        }
    }
    fn classifier(index: usize, args: Vec<Type>) -> Type {
        Type::Classifier {
            declaration: id(index),
            arguments: args,
            raw: false,
        }
    }
    #[test]
    fn empty_binding_view_preserves_shape_checks_and_skips_only_impossible_parameter_search() {
        let expression = classifier(
            0,
            vec![
                Type::Parameter { declaration: id(1) },
                Type::Wildcard {
                    upper: Some(Box::new(classifier(2, vec![]))),
                    lower: None,
                },
            ],
        );
        let empty = Substitution::new(vec![]).unwrap();
        let meter = Rc::new(Meter::default());
        let operation = InvocationContext::new(meter.clone());
        let view = empty.apply_view_observed(&expression, &operation).unwrap();
        assert!(std::ptr::eq(view.as_ref(), &expression));
        assert_eq!(
            serde_json::to_value(view.as_ref()).unwrap(),
            serde_json::to_value(&expression).unwrap()
        );
        let original = Rc::new(Meter::default());
        let original_operation = InvocationContext::new(original.clone());
        shape_observed(&expression, &original_operation).unwrap();
        assert!(
            !empty
                .touches_binding_observed(&expression, &original_operation)
                .unwrap()
        );
        assert_eq!(meter.bytes.get(), original.bytes.get());
        assert!(meter.work.get() < original.work.get());
        meter.cancelled.set(true);
        assert!(empty.apply_view_observed(&expression, &operation).is_err());
        assert!(
            empty
                .apply_view_observed(&expression, &InvocationContext::default())
                .is_ok()
        );
        let invalid = Type::Classifier {
            declaration: id(0),
            arguments: vec![classifier(2, vec![])],
            raw: true,
        };
        assert_eq!(
            empty
                .apply_view_observed(&invalid, &InvocationContext::default())
                .unwrap_err()
                .to_string(),
            empty.apply(&invalid).unwrap_err()
        );
    }
    #[test]
    fn unchanged_substitution_borrows_every_field_and_reduces_owned_copy_receipts() {
        let expression = classifier(
            0,
            vec![
                Type::Parameter { declaration: id(1) },
                Type::Wildcard {
                    upper: Some(Box::new(classifier(2, vec![]))),
                    lower: None,
                },
            ],
        );
        let unrelated = Substitution::new(vec![ParameterBinding {
            parameter: id(9),
            argument: classifier(3, vec![]),
        }])
        .unwrap();
        let meter = Rc::new(Meter::default());
        let operation = InvocationContext::new(meter.clone());
        let view = unrelated
            .apply_view_observed(&expression, &operation)
            .unwrap();
        let std::borrow::Cow::Borrowed(input) = view else {
            panic!("Unchanged input must be borrowed");
        };
        assert!(std::ptr::eq(input, &expression));
        assert_eq!(
            serde_json::to_value(input).unwrap(),
            serde_json::to_value(&expression).unwrap()
        );
        let owned = Rc::new(Meter::default());
        let result = unrelated
            .apply_observed(&expression, &InvocationContext::new(owned.clone()))
            .unwrap();
        assert_eq!(result, expression);
        assert!(meter.work.get() > 0);
        assert!(meter.bytes.get() < owned.bytes.get());
        meter.cancelled.set(true);
        assert!(
            unrelated
                .apply_view_observed(&expression, &operation)
                .is_err()
        );
        assert_eq!(
            unrelated
                .apply_view_observed(&expression, &InvocationContext::default())
                .unwrap()
                .as_ref(),
            &expression
        );
    }
    #[test]
    fn changed_transitive_and_cyclic_substitutions_keep_the_existing_expansion_contract() {
        let t = Type::Parameter { declaration: id(1) };
        let u = Type::Parameter { declaration: id(2) };
        let closed = classifier(3, vec![]);
        let expression = classifier(0, vec![t.clone()]);
        let substitution = Substitution::new(vec![
            ParameterBinding {
                parameter: id(1),
                argument: u.clone(),
            },
            ParameterBinding {
                parameter: id(2),
                argument: closed,
            },
        ])
        .unwrap();
        let view = substitution
            .apply_view_observed(&expression, &InvocationContext::default())
            .unwrap();
        assert!(matches!(view, std::borrow::Cow::Owned(_)));
        assert_eq!(view.as_ref(), &substitution.apply(&expression).unwrap());
        let cyclic = Substitution::new(vec![
            ParameterBinding {
                parameter: id(1),
                argument: u,
            },
            ParameterBinding {
                parameter: id(2),
                argument: t,
            },
        ])
        .unwrap();
        assert_eq!(
            cyclic
                .apply_view_observed(&expression, &InvocationContext::default())
                .unwrap_err()
                .to_string(),
            cyclic.apply(&expression).unwrap_err()
        );
        for invalid in [
            Type::Classifier {
                declaration: id(0),
                arguments: vec![classifier(3, vec![])],
                raw: true,
            },
            Type::Wildcard {
                upper: Some(Box::new(classifier(3, vec![]))),
                lower: Some(Box::new(classifier(4, vec![]))),
            },
        ] {
            let empty = Substitution::new(vec![]).unwrap();
            assert_eq!(
                empty
                    .apply_view_observed(&invalid, &InvocationContext::default())
                    .unwrap_err()
                    .to_string(),
                empty.apply(&invalid).unwrap_err()
            );
        }
    }
}

#[cfg(test)]
#[path = "generic_substitution_preparation_tests.rs"]
mod substitution_preparation_tests;

#[cfg(test)]
#[path = "generic_normalization_preparation_tests.rs"]
mod normalization_preparation_tests;

#[path = "generic_normalization_preparation.rs"]
mod normalization_preparation;
pub(crate) use normalization_preparation::TypeExpressionPreparation;
