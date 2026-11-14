//! One canonical resolver for completed prepared-language member DATA.
//! No model values, graph results, source leases or policy decisions are held.
use crate::definitions::schema::{EffectiveFeature, Feature, PreparedFeatureInputs, Schema};
use crate::definitions::{conversion::InvocationContext, preparation::PreparedPhysicalMetadata};
use std::sync::Arc;

fn find(
    metadata: &PreparedPhysicalMetadata<'_>,
    schema: &Schema,
    receiver: &str,
    name: &str,
    observation: &InvocationContext<'_>,
) -> Result<Option<(Arc<PreparedFeatureInputs>, usize)>, String> {
    // Exact owner/schema admission; full unfiltered compiler/runtime rows only.
    // Construction-filtered PendingSource inputs cannot enter this path.
    let Some(row) = metadata.frozen_feature_inputs(schema, receiver, observation)? else {
        return Ok(None);
    };
    observation.work(1 + receiver.len().max(row.receiver().len()).div_ceil(64))?;
    if row.receiver() != receiver {
        return Err("Frozen read field receiver changed".into());
    }
    Ok(row
        .named_position_observed(name, observation)?
        .map(|position| (row, position)))
}
fn declaration<'schema>(
    schema: &'schema Schema,
    row: &PreparedFeatureInputs,
    name: &str,
    observation: &InvocationContext<'_>,
) -> Result<Option<(&'schema str, &'schema Feature)>, String> {
    // Look up the selected feature under its declaring owner.
    row.admit_lookup_observed(name, 1, observation)?;
    let Some(owner) = row.owner(name) else {
        return Ok(None);
    };
    crate::definitions::schema::admit_feature_lookup_observed(schema, owner, name, observation)?;
    let (owner, class) = schema
        .classes
        .get_key_value(owner)
        .ok_or("Missing frozen read class")?;
    let field = class
        .features
        .get(name)
        .ok_or("Missing frozen read member")?;
    Ok(Some((owner.as_str(), field)))
}
pub(super) fn named<'schema>(
    metadata: &PreparedPhysicalMetadata<'_>,
    schema: &'schema Schema,
    receiver: &str,
    name: &str,
    observation: &InvocationContext<'_>,
) -> Result<Option<EffectiveFeature<'schema>>, String> {
    let Some((row, position)) = find(metadata, schema, receiver, name, observation)? else {
        return Ok(None);
    };
    let Some((_, Some(identity))) = row.member_row_observed(name, observation)? else {
        return Ok(None);
    };
    let Some((owner, feature)) = declaration(schema, &row, name, observation)? else {
        return Ok(None);
    };
    crate::definitions::retained_storage::reserve_serialized(identity, observation)?;
    observation.checkpoint()?;
    // Complete rows preserve the receiver's canonical ordinal, including all
    // inherited/derived entries, rather than the declaring owner's ordinal.
    Ok(Some(EffectiveFeature {
        owner,
        feature,
        identity: identity.clone(),
        position,
    }))
}
