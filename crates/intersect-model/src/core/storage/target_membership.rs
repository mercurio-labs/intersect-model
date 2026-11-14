//! One target-selection progression for owning and borrowed operation domains.
//! All state borrows are transient; no model/header or serialized authority escapes.
use super::{Element, InvocationContext, StorageResult};
use crate::definitions::schema::{
    ClassKeyPreparation, ConformancePreparation, PreparationError, RuntimeAlternative,
    SchemaRepresentation,
};
use std::collections::BTreeSet;

/// Borrowed fields of the existing membership cursor. No resident holder grows.
pub(crate) struct TargetMembership<'a> {
    pub key: &'a mut Option<Box<ClassKeyPreparation>>,
    pub conformance: &'a mut Option<Box<ConformancePreparation>>,
    pub next: &'a mut usize,
    pub done: &'a mut bool,
    pub target_key: &'a mut String,
    pub allowed: &'a mut BTreeSet<usize>,
}

pub(crate) fn advance_target_membership<A: RuntimeAlternative>(
    state: TargetMembership<'_>,
    schema: &SchemaRepresentation<A>,
    elements: &[Element],
    advance_key: impl FnOnce(&mut ClassKeyPreparation, &InvocationContext<'_>) -> StorageResult<bool>,
    before_publication: impl FnOnce(&InvocationContext<'_>) -> StorageResult<()>,
    observation: &InvocationContext<'_>,
) -> StorageResult<bool> {
    if *state.done {
        observation.checkpoint()?;
        return Ok(true);
    }
    observation.work(1)?;
    if let Some(key) = state.key {
        if advance_key(key, observation)? {
            *state.target_key = state
                .key
                .take()
                .ok_or("Missing runtime key preparation")?
                .finish()?;
        }
        return Ok(false);
    }
    if let Some(conformance) = state.conformance {
        // Canonical schema rejection filters this object; observer failures
        // escape and poison the caller's preparation, exactly as before.
        match conformance.advance_observed(schema, observation) {
            Ok(false) => return Ok(false),
            Ok(true) => {
                if matches!(
                    state
                        .conformance
                        .take()
                        .ok_or("Missing membership conformance")?
                        .finish(),
                    Ok(true)
                ) {
                    before_publication(observation)?;
                    state.allowed.insert(*state.next);
                }
            }
            Err(PreparationError::Schema(_)) => *state.conformance = None,
            Err(PreparationError::Observation(error)) => return Err(error),
        }
        *state.next += 1;
        *state.done = *state.next == elements.len();
        return Ok(*state.done);
    }
    let Some(element) = elements.get(*state.next) else {
        *state.done = true;
        return Ok(true);
    };
    *state.conformance = Some(Box::new(
        ConformancePreparation::new_observed(&element.class, state.target_key, observation)
            .map_err(|error| error.to_string())?,
    ));
    Ok(false)
}
