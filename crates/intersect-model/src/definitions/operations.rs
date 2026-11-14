//! Handwritten Ecore operation identity and selection. Selection is not invocation.
use crate::definitions::{
    conversion::InvocationContext,
    resources::DeclarationId,
    schema::{PreparationError, reserve_preparation, reserve_preparation_value},
};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Operation {
    pub name: String,
    pub declaration: DeclarationId,
    pub parameters: Vec<Parameter>,
    pub result: TypedElement,
    /// Exact declaration source retained for annotations, exceptions and generic work.
    pub source_payload: String,
    /// Source-expression metadata; method formals, if any, require explicit arguments.
    pub generic: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Parameter {
    pub name: String,
    pub declaration: DeclarationId,
    pub value: TypedElement,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TypedElement {
    /// Schema classifier key or explicitly supported Ecore builtin. None is void.
    pub target: Option<TypeTarget>,
    pub lower: usize,
    pub upper: i32,
    pub ordered: bool,
    pub unique: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "name", rename_all = "snake_case")]
pub enum TypeTarget {
    /// Exact receiver-substituted signature expression, independent of runtime codecs.
    Expression(crate::definitions::generic_types::Type),
    Builtin(String),
    Class(String),
    Enumeration(String),
    Datatype(String),
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ParameterType {
    /// Exact closed application identity, distinct from its generic declaration.
    Instantiation {
        identity: String,
    },
    Builtin {
        name: String,
    },
    Classifier {
        declaration: DeclarationId,
    },
    Programmatic {
        revision: String,
        namespace: String,
        name: String,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct Signature {
    pub name: String,
    pub parameters: Vec<ParameterType>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct EffectiveOperation<'a> {
    pub owner: &'a str,
    pub position: usize,
    pub operation: &'a Operation,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Resolution<'a> {
    pub signature: Signature,
    pub selected: EffectiveOperation<'a>,
    /// Includes every base declaration, in effective enumeration order.
    pub overridden: Vec<DeclarationId>,
}

/// A source-selected operation and the concrete schema projection it governs.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeclaredOperation {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub projection: Option<crate::definitions::generic_resolution::RuntimeMemberProjection>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub template: Option<crate::definitions::generic_resolution::SpecializedMember>,
    pub owner: String,
    pub operation: Operation,
    pub raw_parameters: Vec<crate::definitions::generic_resolution::RawSignature>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeclaredOperationGroup {
    pub signature: Signature,
    pub selected: DeclarationId,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub selected_owners: Vec<crate::definitions::generic_types::Type>,
    pub operations: Vec<DeclaredOperation>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeclaredOperationSet {
    pub receiver: crate::definitions::generic_types::RuntimeType,
    pub groups: Vec<DeclaredOperationGroup>,
}

impl<A: crate::definitions::schema::RuntimeAlternative>
    crate::definitions::schema::SchemaRepresentation<A>
{
    /// Enumerate declarations without discarding base operations or overloads.
    pub fn effective_operations<'a>(
        &'a self,
        kind: &'a str,
    ) -> Result<Vec<EffectiveOperation<'a>>, String> {
        self.effective_operations_observed(kind, &InvocationContext::default())
            .map_err(|error| error.to_string())
    }
    pub(crate) fn effective_operations_observed<'a>(
        &'a self,
        kind: &'a str,
        observation: &InvocationContext<'_>,
    ) -> Result<Vec<EffectiveOperation<'a>>, PreparationError> {
        let mut result = Vec::new();
        let mut ids = BTreeSet::new();
        for owner in self.ancestry_observed(kind, observation)? {
            observation.work(1).map_err(PreparationError::Observation)?;
            let class = self.classes.get(owner).ok_or("Missing operation owner")?;
            let mut previous = None;
            for operation in &class.operations {
                observation.work(1).map_err(PreparationError::Observation)?;
                reserve_preparation(observation, 128, &[])?;
                if !ids.insert((owner, &operation.declaration)) {
                    return Err(format!(
                        "Repeated operation declaration identity: {:?}",
                        operation.declaration
                    )
                    .into());
                }
                self.validate_member_declaration(owner, &operation.declaration)?;
                for id in class.feature_declarations.values() {
                    observation.work(1).map_err(PreparationError::Observation)?;
                    if id == &operation.declaration {
                        return Err(format!(
                            "Operation and feature share declaration identity: {:?}",
                            operation.declaration
                        )
                        .into());
                    }
                }
                if let Some(prior) = previous {
                    crate::definitions::schema::validate_declaration_order(
                        prior,
                        &operation.declaration,
                    )?;
                }
                previous = Some(&operation.declaration);
                reserve_preparation(observation, 32, &[])?;
                result.push(EffectiveOperation {
                    owner,
                    position: result.len(),
                    operation,
                });
            }
        }
        Ok(result)
    }

    pub fn operation_by_id<'a>(
        &'a self,
        kind: &'a str,
        id: &DeclarationId,
    ) -> Result<EffectiveOperation<'a>, String> {
        let effective = self.effective_operations(kind)?;
        let mut matching = effective
            .into_iter()
            .filter(|entry| &entry.operation.declaration == id);
        let selected = matching
            .next()
            .ok_or_else(|| format!("Unknown operation declaration in {kind}: {id:?}"))?;
        if matching.next().is_some() {
            return Err("Ambiguous runtime operation owner; select an exact source owner".into());
        }
        Ok(selected)
    }

    /// Native type identity does not infer Java instance-class equivalence.
    pub fn operation_parameter_type(&self, target: &TypeTarget) -> Result<ParameterType, String> {
        self.operation_parameter_type_observed(target, &InvocationContext::default())
            .map_err(|error| error.to_string())
    }
    fn operation_parameter_type_observed(
        &self,
        target: &TypeTarget,
        observation: &InvocationContext<'_>,
    ) -> Result<ParameterType, PreparationError> {
        let name = match target {
            TypeTarget::Expression(_) => {
                return Err("Open operation signature requires explicit arguments".into());
            }
            TypeTarget::Builtin(name) => {
                return match name.as_str() {
                    name if crate::definitions::scalars::Builtin::named(name).is_some() => {
                        reserve_preparation(observation, 0, &[name])?;
                        Ok(ParameterType::Builtin {
                            name: name.to_owned(),
                        })
                    }
                    _ => Err(format!(
                        "Unsupported operation datatype {name}; mappings require E05/E06"
                    )
                    .into()),
                };
            }
            TypeTarget::Class(name) => name,
            TypeTarget::Datatype(name) => {
                let datatype = self
                    .datatypes
                    .get(name)
                    .ok_or_else(|| format!("Unknown operation datatype {name}"))?;
                if let Some(application) = datatype.application() {
                    self.applied_datatype_contract(datatype)?;
                    reserve_preparation(observation, 0, &[application.identity()])?;
                    return Ok(ParameterType::Instantiation {
                        identity: application.identity().to_owned(),
                    });
                }
                if let Err(error) = datatype.codec() {
                    let mut contracts = self
                        .conversion_contracts
                        .iter()
                        .filter(|contract| &contract.datatype == datatype.declaration());
                    let contract = contracts.next().ok_or(error)?;
                    if contracts.next().is_some()
                        || contract.serializable != datatype.serializable()
                        || contract.provider.name.trim().is_empty()
                        || contract.provider.version.trim().is_empty()
                    {
                        return Err("Invalid operation datatype conversion contract".into());
                    }
                }
                reserve_preparation_value(datatype.declaration(), observation)?;
                return Ok(ParameterType::Classifier {
                    declaration: datatype.declaration().clone(),
                });
            }
            TypeTarget::Enumeration(name) => {
                let enumeration = self
                    .enumerations
                    .get(name)
                    .ok_or_else(|| format!("Unknown operation enum {name}"))?;
                reserve_preparation_value(enumeration.declaration(), observation)?;
                return Ok(ParameterType::Classifier {
                    declaration: enumeration.declaration().clone(),
                });
            }
        };
        if !self.classes.contains_key(name) {
            return Err(format!("Unknown operation class {name}").into());
        }
        if let Some(instantiation) = self.instantiations.get(name) {
            self.validate_instantiations()?;
            reserve_preparation(observation, 0, &[instantiation.identity()])?;
            return Ok(ParameterType::Instantiation {
                identity: instantiation.identity().to_owned(),
            });
        }
        if let Some(classifier) = self.declarations.get(name) {
            reserve_preparation_value(&classifier.source, observation)?;
            reserve_preparation_value(&classifier.path, observation)?;
            return Ok(ParameterType::Classifier {
                declaration: DeclarationId {
                    source: classifier.source.clone(),
                    path: classifier.path.clone(),
                },
            });
        }
        reserve_preparation(observation, 0, &[&self.revision, &self.uri, name])?;
        Ok(ParameterType::Programmatic {
            revision: self.revision.clone(),
            namespace: self.uri.clone(),
            name: name.clone(),
        })
    }

    pub fn operation_signature(&self, operation: &Operation) -> Result<Signature, String> {
        self.operation_signature_observed(operation, &InvocationContext::default())
            .map_err(|error| error.to_string())
    }
    pub(crate) fn operation_signature_observed(
        &self,
        operation: &Operation,
        observation: &InvocationContext<'_>,
    ) -> Result<Signature, PreparationError> {
        if operation.generic {
            return Err(format!(
                "Generic operation signature requires explicit method arguments: {:?}",
                operation.declaration
            )
            .into());
        }
        self.validate_operation_structure_observed(operation, false, observation)?;
        let mut parameters = Vec::new();
        for parameter in &operation.parameters {
            observation.work(1).map_err(PreparationError::Observation)?;
            let target = parameter
                .value
                .target
                .as_ref()
                .ok_or("Operation parameter requires a type")?;
            let ty = self.operation_parameter_type_observed(target, observation)?;
            reserve_preparation(observation, 32, &[])?;
            parameters.push(ty);
        }
        reserve_preparation(observation, 0, &[&operation.name])?;
        Ok(Signature {
            name: operation.name.clone(),
            parameters,
        })
    }

    pub(crate) fn validate_operation_structure_observed(
        &self,
        operation: &Operation,
        allow_open: bool,
        observation: &InvocationContext<'_>,
    ) -> Result<(), PreparationError> {
        if operation.name.is_empty() {
            return Err("Operation name is empty".into());
        }
        let mut names = BTreeSet::new();
        let mut declarations = BTreeSet::new();
        let mut previous = None;
        for parameter in &operation.parameters {
            observation.work(1).map_err(PreparationError::Observation)?;
            reserve_preparation(observation, 128, &[])?;
            if parameter.name.is_empty() || !names.insert(&parameter.name) {
                return Err(format!(
                    "Duplicate or empty operation parameter name: {:?}",
                    operation.declaration
                )
                .into());
            }
            let id = &parameter.declaration;
            reserve_preparation(observation, 128, &[])?;
            if id.source != operation.declaration.source
                || id.path.len() != operation.declaration.path.len() + 1
                || !id.path.starts_with(&operation.declaration.path)
                || !declarations.insert(id)
            {
                return Err(format!("Invalid operation parameter declaration: {id:?}").into());
            }
            if let Some(prior) = previous {
                crate::definitions::schema::validate_declaration_order(prior, id)?;
            }
            previous = Some(id);
            self.validate_operation_value_observed(&parameter.value, allow_open, observation)?;
            if parameter.value.target.is_none() {
                return Err("Operation parameter requires a type".into());
            }
        }
        self.validate_operation_value_observed(&operation.result, allow_open, observation)?;
        Ok(())
    }

    fn validate_operation_value_observed(
        &self,
        value: &TypedElement,
        allow_open: bool,
        observation: &InvocationContext<'_>,
    ) -> Result<(), PreparationError> {
        if value.upper < -1 || (value.upper >= 0 && value.lower > value.upper as usize) {
            return Err("Invalid operation multiplicity".into());
        }
        if let Some(target) = &value.target {
            if let TypeTarget::Expression(expression) = target {
                if !allow_open {
                    return Err("Open operation signature requires explicit arguments".into());
                }
                reserve_preparation_value(expression, observation)?;
                crate::definitions::generic_types::Substitution::new(vec![])?.apply(expression)?;
            } else {
                self.operation_parameter_type_observed(target, observation)?;
            }
        }
        Ok(())
    }

    /// Resolve declared Ecore groups when present, otherwise native concrete signatures.
    /// Ambiguity never depends on parent order.
    /// Return types are retained but are not an overload-discrimination predicate.
    pub fn resolved_operations<'a>(&'a self, kind: &'a str) -> Result<Vec<Resolution<'a>>, String> {
        if let Some(selections) = &self.declared_operation_selections {
            return self.resolved_declared_operations(kind, selections);
        }
        self.resolve_native_operation_groups(kind, &InvocationContext::default())
            .map_err(|error| error.to_string())
    }
    pub(crate) fn resolve_native_operation_groups<'a>(
        &'a self,
        kind: &'a str,
        observation: &InvocationContext<'_>,
    ) -> Result<Vec<Resolution<'a>>, PreparationError> {
        if self.declared_operation_selections.is_some() {
            return Err(
                "Ordinary operation resolution requires a schema without declared selection groups"
                    .into(),
            );
        }
        let mut groups: Vec<(Signature, Vec<EffectiveOperation<'a>>)> = Vec::new();
        let mut indices = BTreeMap::new();
        for entry in self.effective_operations_observed(kind, observation)? {
            observation.work(1).map_err(PreparationError::Observation)?;
            let signature = self.operation_signature_observed(entry.operation, observation)?;
            let next = groups.len();
            let index = match indices.get(&signature) {
                Some(index) => *index,
                None => {
                    reserve_preparation_value(&signature, observation)?;
                    reserve_preparation(observation, 128, &[])?;
                    indices.insert(signature.clone(), next);
                    reserve_preparation(observation, 32, &[])?;
                    groups.push((signature, Vec::new()));
                    next
                }
            };
            let (_, candidates) = &mut groups[index];
            for previous in candidates.iter() {
                observation.work(1).map_err(PreparationError::Observation)?;
                if previous.owner == entry.owner {
                    return Err(format!(
                        "Duplicate local operation signature: {:?} and {:?}",
                        previous.operation.declaration, entry.operation.declaration
                    )
                    .into());
                }
            }
            reserve_preparation(observation, 32, &[])?;
            candidates.push(entry);
        }
        let mut resolved = Vec::new();
        for (signature, candidates) in groups {
            observation.work(1).map_err(PreparationError::Observation)?;
            let winner = crate::definitions::operation_selection::unique_dominating_candidate(
                &candidates,
                |candidate, other| {
                    observation.work(1).map_err(PreparationError::Observation)?;
                    self.conforms_observed(candidate.owner, other.owner, observation)
                },
            )?;
            let selected = match winner {
                Some(index) => candidates[index].clone(),
                None => {
                    return Err(format!(
                        "Ambiguous inherited operation {kind}.{}: {:?}",
                        signature.name,
                        candidates
                            .iter()
                            .map(|c| &c.operation.declaration)
                            .collect::<Vec<_>>()
                    )
                    .into());
                }
            };
            let mut overridden = Vec::new();
            for entry in &candidates {
                observation.work(1).map_err(PreparationError::Observation)?;
                if entry.operation.declaration != selected.operation.declaration {
                    reserve_preparation_value(&entry.operation.declaration, observation)?;
                    reserve_preparation(observation, 32, &[])?;
                    overridden.push(entry.operation.declaration.clone());
                }
            }
            reserve_preparation(observation, 32, &[])?;
            resolved.push(Resolution {
                signature,
                selected,
                overridden,
            });
        }
        Ok(resolved)
    }

    pub fn resolve_operation<'a>(
        &'a self,
        kind: &'a str,
        signature: &Signature,
    ) -> Result<Resolution<'a>, String> {
        self.resolved_operations(kind)?
            .into_iter()
            .find(|entry| &entry.signature == signature)
            .ok_or_else(|| format!("Unknown operation signature in {kind}: {signature:?}"))
    }
}
