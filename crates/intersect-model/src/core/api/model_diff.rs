//! Handwritten deterministic comparison of immutable model snapshots. Not a patch.
use super::{ModelSnapshot, SnapshotError, SnapshotIdentity, snapshots::identity};
use crate::{
    core::{model::Element, resources::ResourceRecord, storage},
    definitions::schema::{MemberIdentity, Schema},
};
use serde::Serialize;
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};

/// Missing storage and explicit JSON null are different observations.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub enum StoredValue {
    Missing,
    Present(Value),
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct FeatureState {
    pub value: StoredValue,
    /// For unsettable features: None means legacy/unknown assignment evidence.
    pub explicitly_set: Option<bool>,
}
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct FeatureChange {
    pub name: String,
    pub identity: MemberIdentity,
    pub before: FeatureState,
    pub after: FeatureState,
}

/// Full records preserve metadata, construction evidence and explicit assignment
/// state. Feature changes additionally provide stable schema member identities.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ObjectChange {
    Created {
        after: Box<Element>,
    },
    Deleted {
        before: Box<Element>,
    },
    Updated {
        before: Box<Element>,
        after: Box<Element>,
        features: Vec<FeatureChange>,
    },
}
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct ResourceChange {
    pub uri: String,
    pub before: Option<ResourceRecord>,
    pub after: Option<ResourceRecord>,
}

/// Immutable, deterministic net differences under the exact same schema.
/// Object/resource changes are lexically ordered by durable identity. Stored
/// lists retain their order. Resource metadata and exported row order are explicit.
/// Capturing/comparing never validates domain rules or mutates either input.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct ModelDiff {
    format: &'static str,
    before: SnapshotIdentity,
    after: SnapshotIdentity,
    objects: Vec<ObjectChange>,
    resources: Vec<ResourceChange>,
    object_order_before: Vec<String>,
    object_order_after: Vec<String>,
}
impl ModelDiff {
    pub fn between(before: &ModelSnapshot, after: &ModelSnapshot) -> Result<Self, SnapshotError> {
        let old = storage::export(before)?;
        let new = storage::export(after)?;
        let before_id = identity(before.schema(), &old)?;
        let after_id = identity(after.schema(), &new)?;
        if before_id.schema != after_id.schema {
            return Err(SnapshotError::SchemaMismatch);
        }
        let a: BTreeMap<_, _> = old.elements.iter().map(|e| (&e.id, e)).collect();
        let b: BTreeMap<_, _> = new.elements.iter().map(|e| (&e.id, e)).collect();
        let mut objects = Vec::new();
        for id in a.keys().chain(b.keys()).copied().collect::<BTreeSet<_>>() {
            match (a.get(id), b.get(id)) {
                (None, Some(e)) => objects.push(ObjectChange::Created {
                    after: Box::new((*e).clone()),
                }),
                (Some(e), None) => objects.push(ObjectChange::Deleted {
                    before: Box::new((*e).clone()),
                }),
                (Some(a), Some(b)) if a != b => objects.push(ObjectChange::Updated {
                    before: Box::new((*a).clone()),
                    after: Box::new((*b).clone()),
                    features: features(before.schema(), a, b)?,
                }),
                _ => {}
            }
        }
        let mut resources = Vec::new();
        for uri in old
            .resources
            .keys()
            .chain(new.resources.keys())
            .collect::<BTreeSet<_>>()
        {
            let a = old.resources.get(uri);
            let b = new.resources.get(uri);
            if a != b {
                resources.push(ResourceChange {
                    uri: uri.clone(),
                    before: a.cloned(),
                    after: b.cloned(),
                });
            }
        }
        Ok(Self {
            format: "intersect-model-diff-v1",
            before: before_id,
            after: after_id,
            objects,
            resources,
            object_order_before: old.elements.iter().map(|e| e.id.clone()).collect(),
            object_order_after: new.elements.iter().map(|e| e.id.clone()).collect(),
        })
    }
    pub fn before(&self) -> &SnapshotIdentity {
        &self.before
    }
    pub fn after(&self) -> &SnapshotIdentity {
        &self.after
    }
    pub fn objects(&self) -> &[ObjectChange] {
        &self.objects
    }
    pub fn resources(&self) -> &[ResourceChange] {
        &self.resources
    }
    pub fn object_order_before(&self) -> &[String] {
        &self.object_order_before
    }
    pub fn object_order_after(&self) -> &[String] {
        &self.object_order_after
    }
    pub fn is_empty(&self) -> bool {
        self.objects.is_empty()
            && self.resources.is_empty()
            && self.object_order_before == self.object_order_after
    }
}

fn state(e: &Element, name: &str, unsettable: bool) -> FeatureState {
    FeatureState {
        value: e
            .values
            .get(name)
            .cloned()
            .map_or(StoredValue::Missing, StoredValue::Present),
        explicitly_set: if unsettable {
            e.assignment_state.as_ref().map(|s| s.explicitly_set(name))
        } else {
            None
        },
    }
}
fn features(
    schema: &Schema,
    a: &Element,
    b: &Element,
) -> Result<Vec<FeatureChange>, SnapshotError> {
    // A type change is retained in the full records; feature correspondence across
    // different types needs an explicit migration contract.
    if a.class != b.class || a.type_identity != b.type_identity {
        return Ok(Vec::new());
    }
    let mut changes = Vec::new();
    for field in schema.effective_features(&a.class)? {
        let name = &field.feature.name;
        if field.feature.derived {
            continue;
        }
        let before = state(a, name, field.feature.unsettable);
        let after = state(b, name, field.feature.unsettable);
        if before != after {
            changes.push(FeatureChange {
                name: name.clone(),
                identity: field.identity,
                before,
                after,
            });
        }
    }
    changes.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(changes)
}
