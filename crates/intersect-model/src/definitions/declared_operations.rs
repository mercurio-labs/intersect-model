//! Structural validation of compiled Ecore selection contracts. Recompile retained
//! package inputs to verify source/provider semantics; deserialization is not proof.
use crate::definitions::{
    conversion::InvocationContext,
    generic_resolution::RawSignature,
    operations::{DeclaredOperationSet, Operation, ParameterType, Resolution, Signature},
    schema::{PreparationError, reserve_preparation, reserve_preparation_value},
};
use std::collections::{BTreeMap, BTreeSet};

#[cfg(test)]
pub(crate) fn erased_signature(
    operation: &Operation,
    parameters: &[RawSignature],
) -> Result<Signature, String> {
    erased_signature_observed(operation, parameters, &InvocationContext::default())
        .map_err(|error| error.to_string())
}

pub(crate) fn erased_signature_observed(
    operation: &Operation,
    parameters: &[RawSignature],
    observation: &InvocationContext<'_>,
) -> Result<Signature, PreparationError> {
    if parameters.len() != operation.parameters.len() {
        return Err("Declared operation parameter count differs from projection".into());
    }
    let mut erased = Vec::new();
    for (raw, parameter) in parameters.iter().zip(&operation.parameters) {
        observation.work(1).map_err(PreparationError::Observation)?;
        if raw.owner != parameter.declaration || raw.feature != "eType" || raw.raw_types.len() != 1
        {
            return Err("Invalid declared operation parameter provenance".into());
        }
        reserve_preparation_value(&raw.raw_types[0], observation)?;
        reserve_preparation(observation, 32, &[])?;
        erased.push(ParameterType::Classifier {
            declaration: raw.raw_types[0].clone(),
        });
    }
    reserve_preparation(observation, 0, &[&operation.name])?;
    Ok(Signature {
        name: operation.name.clone(),
        parameters: erased,
    })
}

impl<A: crate::definitions::schema::RuntimeAlternative>
    crate::definitions::schema::SchemaRepresentation<A>
{
    pub(crate) fn resolved_declared_operations<'a>(
        &'a self,
        kind: &'a str,
        selections: &'a BTreeMap<String, DeclaredOperationSet>,
    ) -> Result<Vec<Resolution<'a>>, String> {
        self.checked_declared_operations_observed(
            kind,
            selections,
            true,
            &InvocationContext::default(),
        )
        .map_err(|error| error.to_string())
    }

    /// Validate stored group inventory without choosing an ambiguous typed owner.
    pub fn validate_declared_operation_inventory(&self, kind: &str) -> Result<(), String> {
        self.validate_declared_operation_inventory_observed(kind, &InvocationContext::default())
            .map_err(|error| error.to_string())
    }
    pub(crate) fn validate_declared_operation_inventory_observed(
        &self,
        kind: &str,
        observation: &InvocationContext<'_>,
    ) -> Result<(), PreparationError> {
        let selections = self
            .declared_operation_selections
            .as_ref()
            .ok_or("Missing declared operation selections")?;
        self.checked_declared_operations_observed(kind, selections, false, observation)
            .map(|_| ())
    }

    pub(crate) fn checked_declared_operations_observed<'a>(
        &'a self,
        kind: &'a str,
        selections: &'a BTreeMap<String, DeclaredOperationSet>,
        unique: bool,
        observation: &InvocationContext<'_>,
    ) -> Result<Vec<Resolution<'a>>, PreparationError> {
        self.checked_declared_operations_context(kind, selections, unique, observation, None)
    }

    pub(crate) fn validate_declared_operation_inventory_prepared(
        &self,
        kind: &str,
        prepared: &crate::definitions::schema::ValidatedRuntimeTypes<'_, A>,
        observation: &InvocationContext<'_>,
    ) -> Result<(), PreparationError> {
        if !prepared.belongs_to(self) {
            return Err("Runtime type validation belongs to a different schema".into());
        }
        let selections = self
            .declared_operation_selections
            .as_ref()
            .ok_or("Missing declared operation selections")?;
        self.validate_selected_operation_inventory_prepared(kind, selections, prepared, observation)
    }
    /// Check externally staged contracts using the identical immutable type proof.
    /// This adds no publication authority and skips no compiler validation stage.
    pub(crate) fn validate_selected_operation_inventory_prepared(
        &self,
        kind: &str,
        selections: &BTreeMap<String, DeclaredOperationSet>,
        prepared: &crate::definitions::schema::ValidatedRuntimeTypes<'_, A>,
        observation: &InvocationContext<'_>,
    ) -> Result<(), PreparationError> {
        if !prepared.belongs_to(self) {
            return Err("Runtime type validation belongs to a different schema".into());
        }
        self.checked_declared_operations_context(
            kind,
            selections,
            false,
            observation,
            Some(prepared),
        )
        .map(|_| ())
    }

    fn checked_declared_operations_context<'a>(
        &'a self,
        kind: &'a str,
        selections: &'a BTreeMap<String, DeclaredOperationSet>,
        unique: bool,
        observation: &InvocationContext<'_>,
        prepared: Option<&crate::definitions::schema::ValidatedRuntimeTypes<'_, A>>,
    ) -> Result<Vec<Resolution<'a>>, PreparationError> {
        let class_key = |ty: &crate::definitions::generic_types::RuntimeType| match prepared {
            Some(prepared) => prepared.class_key_for_type_observed(ty, observation),
            None => self.class_key_for_type_observed(ty, observation),
        };
        if selections.len() != self.classes.len() {
            return Err("Declared operation receiver inventory differs from schema classes".into());
        }
        for (selection, class) in selections.keys().zip(self.classes.keys()) {
            observation.work(1).map_err(PreparationError::Observation)?;
            if selection != class {
                return Err(
                    "Declared operation receiver inventory differs from schema classes".into(),
                );
            }
        }
        let selection = selections
            .get(kind)
            .ok_or("Missing declared operation receiver")?;
        if class_key(&selection.receiver)? != kind {
            return Err("Declared operation receiver identity differs from schema class".into());
        }
        let inventory = self
            .generic_signatures
            .as_ref()
            .ok_or("Missing declared operation source inventory")?;
        let effective = self.effective_operations_observed(kind, observation)?;
        let mut by_id = BTreeMap::new();
        for entry in &effective {
            observation.work(1).map_err(PreparationError::Observation)?;
            reserve_preparation(observation, 128, &[])?;
            by_id.insert((entry.owner, &entry.operation.declaration), entry);
        }
        let mut seen = BTreeSet::new();
        let mut covered = BTreeSet::new();
        let mut signatures = BTreeSet::new();
        let mut result = Vec::new();
        for group in &selection.groups {
            observation.work(1).map_err(PreparationError::Observation)?;
            reserve_preparation(observation, 128, &[])?;
            if !signatures.insert(&group.signature) {
                return Err("Duplicate declared operation group signature".into());
            }
            let mut candidates = Vec::new();
            for retained in &group.operations {
                observation.work(1).map_err(PreparationError::Observation)?;
                let id = &retained.operation.declaration;
                let source_owner = retained
                    .projection
                    .as_ref()
                    .map(|projection| &projection.source.owner);
                reserve_preparation(observation, 128, &[])?;
                if !seen.insert((id, retained.owner.as_str(), source_owner)) {
                    return Err("Repeated declared operation projection".into());
                }
                let entry = by_id
                    .get(&(retained.owner.as_str(), id))
                    .ok_or("Unknown declared operation member")?;
                if !covered.contains(&(retained.owner.as_str(), id)) {
                    reserve_preparation(observation, 128, &[])?;
                    covered.insert((retained.owner.as_str(), id));
                }
                reserve_preparation_value(entry.operation, observation)?;
                let mut expected = entry.operation.clone();
                if let Some(template) = &retained.template {
                    crate::definitions::operation_templates::project_observed(
                        &mut expected,
                        template,
                        observation,
                    )?;
                }
                observation.work(1).map_err(PreparationError::Observation)?;
                if expected != retained.operation {
                    return Err("Declared operation projection differs from schema member".into());
                }
                if let Some(projection) = &retained.projection
                    && (projection.receiver != selection.receiver
                        || projection.source.declaration != *id
                        || class_key(&projection.storage_owner)? != retained.owner)
                {
                    return Err("Declared operation runtime projection identity mismatch".into());
                }
                self.validate_operation_template_observed(retained, observation)?;
                erased_signature_observed(
                    &retained.operation,
                    &retained.raw_parameters,
                    observation,
                )?;
                if retained.operation.name != group.signature.name
                    || retained.raw_parameters.len() != group.signature.parameters.len()
                {
                    return Err("Declared operation group has inconsistent name or arity".into());
                }
                for raw in &retained.raw_parameters {
                    observation.work(1).map_err(PreparationError::Observation)?;
                    let mut found = false;
                    for candidate in &inventory.raw_signatures {
                        observation.work(1).map_err(PreparationError::Observation)?;
                        if candidate == raw {
                            found = true;
                            break;
                        }
                    }
                    if !found {
                        return Err(
                            "Declared operation raw signature differs from source inventory".into(),
                        );
                    }
                }
                reserve_preparation(observation, 32, &[])?;
                candidates.push(retained);
            }
            let mut winners = Vec::new();
            for (index, candidate) in candidates.iter().enumerate() {
                observation.work(1).map_err(PreparationError::Observation)?;
                let ancestors = self.ancestry_observed(&candidate.owner, observation)?;
                let mut dominates = true;
                for other in &candidates {
                    observation.work(1).map_err(PreparationError::Observation)?;
                    let mut found = false;
                    for ancestor in &ancestors {
                        observation.work(1).map_err(PreparationError::Observation)?;
                        if *ancestor == other.owner
                            || self
                                .declarations
                                .get(*ancestor)
                                .zip(self.declarations.get(&other.owner))
                                .is_some_and(|(a, b)| a.source == b.source && a.path == b.path)
                        {
                            found = true;
                            break;
                        }
                    }
                    if !found {
                        dominates = false;
                        break;
                    }
                }
                if dominates {
                    reserve_preparation(observation, 32, &[])?;
                    winners.push(index);
                }
            }
            let winner = *winners
                .first()
                .ok_or("Ambiguous declared operation group")?;
            for index in &winners {
                observation.work(1).map_err(PreparationError::Observation)?;
                if candidates[*index].operation.declaration != group.selected {
                    return Err("Declared operation winner does not dominate its group".into());
                }
            }
            if winners.len() > 1 {
                let mut owners = Vec::new();
                for index in &winners {
                    observation.work(1).map_err(PreparationError::Observation)?;
                    let owner = &candidates[*index]
                        .projection
                        .as_ref()
                        .ok_or("Missing ambiguous owner projection")?
                        .source
                        .owner;
                    reserve_preparation_value(owner, observation)?;
                    reserve_preparation(observation, 32, &[])?;
                    owners.push(owner.clone());
                }
                if owners != group.selected_owners {
                    return Err("Declared operation selected owners differ from projections".into());
                }
                if unique {
                    return Err(
                        "Ambiguous runtime operation owner; select an exact source owner".into(),
                    );
                }
            } else if !group.selected_owners.is_empty() {
                return Err("Unexpected declared operation owner alternatives".into());
            }
            let retained = candidates[winner];
            if erased_signature_observed(
                &retained.operation,
                &retained.raw_parameters,
                observation,
            )? != group.signature
            {
                return Err("Declared operation signature differs from selected erasure".into());
            }
            reserve_preparation_value(&group.signature, observation)?;
            let mut overridden = Vec::new();
            for candidate in &candidates {
                observation.work(1).map_err(PreparationError::Observation)?;
                if candidate.operation.declaration != group.selected {
                    reserve_preparation_value(&candidate.operation.declaration, observation)?;
                    reserve_preparation(observation, 32, &[])?;
                    overridden.push(candidate.operation.declaration.clone());
                }
            }
            reserve_preparation(observation, 32, &[])?;
            result.push(Resolution {
                signature: group.signature.clone(),
                selected: crate::definitions::operations::EffectiveOperation {
                    owner: &retained.owner,
                    position: by_id
                        .get(&(retained.owner.as_str(), &retained.operation.declaration))
                        .ok_or("Missing selected operation position")?
                        .position,
                    operation: &retained.operation,
                },
                overridden,
            });
        }
        if covered.len() != effective.len() {
            return Err("Declared operation inventory omits effective members".into());
        }
        Ok(result)
    }
}
