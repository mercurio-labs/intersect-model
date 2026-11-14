//! Local current-store reciprocal witness, not a whole-model ownership proof.
use super::*;
use crate::definitions::schema::{
    EffectiveFeaturePreparation, MemberIdentity, ReferenceConformancePreparation, Schema,
};

#[allow(clippy::too_many_arguments)]
pub(crate) fn checked_inverse_nominal_witness(
    store: &dyn ModelRead,
    schema: &Schema,
    receiver: &str,
    actual: &str,
    reference: &str,
    expected: &Value,
    observation: &InvocationContext<'_>,
) -> StorageResult<()> {
    observation.work(1)?;
    if store.revision()? != schema.revision {
        return Err("Inverse nominal store/schema mismatch".into());
    }
    let selected = schema.effective_feature(actual, reference)?;
    let inverse = selected.feature;
    let inverse_owner = selected.owner;
    if !matches!(selected.identity, MemberIdentity::Source { .. })
        || !inverse.reference
        || inverse.derived
        || inverse.containment
        || inverse.upper != 1
        || !schema.is_container_feature(inverse)?
    {
        return Err("Inverse nominal requires a source-defined stored inverse container".into());
    }
    let opposite = inverse
        .opposite
        .as_deref()
        .ok_or("Missing inverse nominal opposite")?;
    let (opposite_owner, opposite_name) = opposite
        .split_once('/')
        .ok_or("Invalid inverse nominal opposite")?;
    let expected_owner = if expected.is_null() {
        None
    } else {
        Some(
            expected
                .as_str()
                .filter(|s| !s.is_empty())
                .ok_or("Malformed inverse nominal owner")?,
        )
    };
    let check_receiver = || -> StorageResult<()> {
        let mut found = false;
        store.inspect(receiver, &mut |row| {
            observation.work(1)?;
            if found
                || row.id() != receiver
                || row.class() != actual
                || row.type_identity() != schema.identity(actual)
            {
                return Err("Inverse nominal receiver identity mismatch".into());
            }
            validate_feature_shape(actual, inverse, row.value(reference), true, || {
                observation.work(1)
            })?;
            let value = row
                .value(reference)
                .map(|v| v.to_json())
                .unwrap_or(Value::Null);
            if &value != expected {
                return Err("Inverse nominal receiver value changed".into());
            }
            found = true;
            Ok(())
        })?;
        if !found {
            return Err("Missing inverse nominal receiver".into());
        }
        Ok(())
    };
    check_receiver()?;
    // Reuse the authoritative duplicate-parent rule, scoped to this one child.
    // Nothing escapes as OwnershipIndex/audited graph or serialized authority.
    let mut ownership = OwnershipIndex::new(false);
    let mut reciprocal = false;
    // Query candidates are exact raw value occurrences, then qualified below.
    // Source field names are deduplicated, so one edge is never counted twice.
    let mut names = BTreeSet::new();
    for class in schema.classes.values() {
        observation.work(1)?;
        for (name, feature) in &class.features {
            observation.work(1)?;
            if feature.containment && !names.contains(name.as_str()) {
                observation.retain_storage(64)?;
                names.insert(name.as_str());
            }
        }
    }
    crate::definitions::retained_storage::reserve_serialized(&receiver, observation)?;
    let needle = Value::String(receiver.to_owned());
    for name in names {
        let mut cursor = None;
        loop {
            observation.work(1)?;
            let batch = store.query_candidates_observed(
                Query::Value(name, &needle),
                cursor,
                16,
                observation,
            )?;
            let mut position_before = cursor;
            for (position, id) in batch.rows {
                observation.work(1)?;
                if position_before.is_some_and(|old| position <= old) {
                    return Err("Inverse nominal candidate cursor did not advance".into());
                }
                position_before = Some(position);
                let mut visited = false;
                store.inspect(&id, &mut |row| {
                    observation.work(1)?;
                    if visited
                        || row.id() != id
                        || row.type_identity() != schema.identity(row.class())
                        || !schema.classes.contains_key(row.class())
                    {
                        return Err("Inverse nominal containment owner identity mismatch".into());
                    }
                    visited = true;
                    let mut fields = EffectiveFeaturePreparation::new(row.class());
                    while !fields.advance(schema)? {
                        observation.work(1)?;
                    }
                    for field in fields.finish()? {
                        observation.work(1)?;
                        let selected = field.resolve(schema)?;
                        let feature = selected.feature;
                        if !feature.containment || feature.name != name {
                            continue;
                        }
                        let value = row.value(&feature.name);
                        let mut occurrences = 0usize;
                        if let Some(list) = value.and_then(|v| v.collection()) {
                            for offset in 0..list.len() {
                                observation.work(1)?;
                                if list
                                    .get(offset)
                                    .is_some_and(|v| v.as_str() == Some(receiver))
                                {
                                    occurrences += 1;
                                }
                            }
                        } else if value.is_some_and(|v| v.as_str() == Some(receiver)) {
                            occurrences = 1;
                        }
                        if occurrences == 0 {
                            continue;
                        }
                        if feature.derived
                            || !feature.reference
                            || !matches!(selected.identity, MemberIdentity::Source { .. })
                        {
                            return Err(
                                "Inverse nominal ownership requires stored source containment"
                                    .into(),
                            );
                        }
                        validate_feature_shape(row.class(), feature, value, true, || {
                            observation.work(1)
                        })?;
                        let mut conformance = ReferenceConformancePreparation::new_observed(
                            row.class(),
                            &feature.name,
                            actual,
                            observation,
                        )
                        .map_err(|e| e.to_string())?;
                        while !conformance
                            .advance_observed(schema, observation)
                            .map_err(|e| e.to_string())?
                        {}
                        if !conformance.finish()? {
                            return Err(
                                "Inverse nominal containment target does not conform".into()
                            );
                        }
                        for _ in 0..occurrences {
                            ownership.insert(receiver, row.id(), observation)?;
                        }
                        let back = feature.opposite.as_deref().and_then(|s| s.split_once('/'));
                        if Some(row.id()) != expected_owner
                            || selected.owner != opposite_owner
                            || feature.name != opposite_name
                            || back != Some((inverse_owner, reference))
                            || receiver == row.id()
                        {
                            return Err(
                                "Inverse nominal containment is not the reciprocal source pair"
                                    .into(),
                            );
                        }
                        reciprocal = true;
                    }
                    Ok(())
                })?;
                if !visited {
                    return Err("Missing inverse nominal containment owner".into());
                }
            }
            if batch.exhausted {
                break;
            }
            let next = batch
                .after
                .ok_or("Missing inverse nominal candidate cursor")?;
            if cursor.is_some_and(|old| next <= old) {
                return Err("Inverse nominal candidate cursor did not advance".into());
            }
            cursor = Some(next);
        }
    }
    if ownership.parent(receiver) != expected_owner || reciprocal != expected_owner.is_some() {
        return Err("Inverse nominal missing reciprocal containment owner".into());
    }
    check_receiver()?;
    observation.work(1)?;
    if store.revision()? != schema.revision {
        return Err("Inverse nominal revision changed during witness".into());
    }
    Ok(())
}
