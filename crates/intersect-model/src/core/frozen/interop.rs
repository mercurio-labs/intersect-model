//! Compatibility boundary with the materialized `core::model::Model`. Import
//! and export are lossless for the supported profile; unsupported element
//! state is refused explicitly rather than dropped.
use super::{
    Diagnostics, FrozenError, Violation,
    builder::{ModelBuilder, encode_value, intern_resource_record, new_list},
    metamodel::ClassId,
    metamodel::{FeatureKind, Metamodel},
    model::{Carried, Model, ModelData, Slot},
};
use crate::core::{model, resources::ResourceRecord};
use serde_json::Value;
use std::sync::Arc;

#[derive(Debug)]
pub enum ImportError {
    Refused(FrozenError),
    Invalid(Diagnostics),
}
impl std::fmt::Display for ImportError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Refused(e) => e.fmt(f),
            Self::Invalid(d) => d.fmt(f),
        }
    }
}
impl std::error::Error for ImportError {}
impl From<FrozenError> for ImportError {
    fn from(error: FrozenError) -> Self {
        Self::Refused(error)
    }
}

impl ModelBuilder {
    /// Load a materialized model without structural validation, so callers can
    /// add references and link before validating. References must name
    /// objects in the same model.
    pub fn import(metamodel: Arc<Metamodel>, source: &model::Model) -> Result<Self, ImportError> {
        if !source.schema_revision.is_empty() && source.schema_revision != metamodel.revision() {
            return Err(FrozenError::Revision.into());
        }
        let mut builder = ModelBuilder::with_capacity(metamodel.clone(), source.elements.len());
        for (uri, record) in &source.resources {
            intern_resource_record(&mut builder.data, uri, record.clone())?;
        }
        let mut objects = Vec::with_capacity(source.elements.len());
        for element in &source.elements {
            let class = metamodel
                .class(&element.class)
                .ok_or(FrozenError::UnknownClass)?;
            let info = metamodel
                .class_info(class)
                .ok_or(FrozenError::UnknownClass)?;
            if *info.identity != *element.type_identity {
                return Err(FrozenError::TypeIdentity(element.id.clone()).into());
            }
            let obj = builder.create_at(
                &element.id,
                class,
                &element.resource,
                element.start,
                element.end,
            )?;
            let header = builder.data.header(obj).ok_or(FrozenError::UnknownObject)?;
            let base = header.slots as usize;
            for slot in builder
                .data
                .slots
                .get_mut(base..base + info.stored.len())
                .into_iter()
                .flatten()
            {
                *slot = Slot::Absent;
            }
            // `to_model` exports the implicit mark; keep it on the way back.
            if element.implicit {
                builder.data.implicit.insert(obj);
            }
            if element.initialization.is_some() || element.assignment_state.is_some() {
                builder.data.carried.insert(
                    obj,
                    Box::new(Carried {
                        initialization: element.initialization.clone(),
                        assignment_state: element.assignment_state.clone(),
                    }),
                );
            }
            objects.push((obj, class));
        }
        let mut problems = Vec::new();
        for (element, (obj, class)) in source.elements.iter().zip(objects) {
            for (name, value) in &element.values {
                let Some(feature) = metamodel.feature(class, name) else {
                    return Err(FrozenError::NotAFeature {
                        class: element.class.clone(),
                        feature: name.clone(),
                    }
                    .into());
                };
                let info = metamodel
                    .feature_info(feature)
                    .ok_or(FrozenError::UnknownFeature)?;
                if info.derived {
                    return Err(FrozenError::Derived(name.clone()).into());
                }
                let data = &mut builder.data;
                let slot = if info.kind == FeatureKind::Attribute {
                    encode_value(data, value.clone())?
                } else {
                    match value {
                        Value::Null => Slot::Null,
                        Value::Array(items) => {
                            let items = items
                                .iter()
                                .map(|item| reference(data, item, &mut problems, &element.id, name))
                                .collect::<Result<_, _>>()?;
                            new_list(data, items)?
                        }
                        item => reference(data, item, &mut problems, &element.id, name)?,
                    }
                };
                *builder.data.slot_mut(&metamodel, obj, feature)? = slot;
            }
        }
        if !problems.is_empty() {
            return Err(ImportError::Invalid(Diagnostics(problems)));
        }
        Ok(builder)
    }
}

impl Model {
    /// Attach package provenance without materializing object values or
    /// changing their order, indices, evidence, or prepared status.
    pub fn with_package_identity(
        mut self,
        identity: &crate::core::project_usages::PackageIdentity,
    ) -> Result<Self, FrozenError> {
        let records = self.resource_records()?;
        for (uri, slot) in self
            .data
            .resources
            .iter()
            .zip(&mut self.data.resource_records)
        {
            let uri = self.data.strings.resolve(*uri);
            let mut record = records
                .get(uri)
                .cloned()
                .ok_or(FrozenError::Unsupported("missing resource"))?;
            if record
                .package
                .as_ref()
                .is_some_and(|package| package != identity)
            {
                return Err(FrozenError::Schema(format!(
                    "Resource {uri} already belongs to another package"
                )));
            }
            record.package = Some(identity.clone());
            *slot = Some(record);
        }
        Ok(self)
    }

    /// Resource metadata in URI order, including empty resources and roots
    /// established by the validated model's containment graph.
    pub fn resource_records(
        &self,
    ) -> Result<std::collections::BTreeMap<String, ResourceRecord>, FrozenError> {
        let mut roots = vec![Vec::new(); self.data.resources.len()];
        for object in self.objects() {
            if object.container().is_none() {
                let header = self
                    .data
                    .headers
                    .get(object.id().index())
                    .ok_or(FrozenError::UnknownObject)?;
                roots
                    .get_mut(header.resource as usize)
                    .ok_or(FrozenError::Unsupported("missing resource"))?
                    .push(object.identity().to_owned());
            }
        }
        self.data
            .resources
            .iter()
            .zip(&self.data.resource_records)
            .zip(roots)
            .map(|((uri, record), roots)| {
                let uri = self.data.strings.resolve(*uri);
                let mut record = record
                    .clone()
                    .map_or_else(|| ResourceRecord::new(uri).map_err(FrozenError::Schema), Ok)?;
                record.roots = Arc::new(roots);
                Ok((uri.to_owned(), record))
            })
            .collect()
    }

    /// Import and structurally validate a materialized model.
    pub fn from_model(
        metamodel: Arc<Metamodel>,
        source: &model::Model,
    ) -> Result<Self, ImportError> {
        let builder = ModelBuilder::import(metamodel, source)?;
        // Materialized opposites are already explicit; validate, don't complete.
        let containers = super::validate::validate(&builder.metamodel, &builder.data, None)
            .map_err(ImportError::Invalid)?;
        let ModelBuilder {
            metamodel,
            mut data,
            ..
        } = builder;
        data.apply_containers(containers);
        Ok(Model {
            metamodel,
            data,
            prepared: 0,
            facts: super::model::PreparedFacts::validated(Some(true)),
            base: None,
        })
    }

    /// Materialize for existing consumers (XMI, ResourceSet, oracle checks).
    pub fn to_model(&self) -> Result<model::Model, FrozenError> {
        let mm = &self.metamodel;
        let data = &self.data;
        let mut elements = Vec::with_capacity(data.headers.len());
        for obj in self.objects() {
            let header = &data.headers[obj.id().index()];
            let class = mm
                .class_info(header.class)
                .ok_or(FrozenError::UnknownClass)?;
            let mut values = std::collections::BTreeMap::new();
            for (index, feature) in class.stored.iter().enumerate() {
                let own = data.slots.get(header.slots as usize + index);
                // Redefined features are exported through their redefining one.
                if mm.redefined_by(header.class, *feature).is_some()
                    && own.is_none_or(|slot| data.is_empty_slot(mm, *feature, *slot))
                {
                    continue;
                }
                let value = match own {
                    None | Some(Slot::Absent) => continue,
                    Some(Slot::Default) => mm
                        .definition(*feature)
                        .map_or(Value::Null, |d| d.default.clone()),
                    Some(slot) => decode(data, *slot),
                };
                values.insert(mm.feature_name(*feature).to_owned(), value);
            }
            elements.push(model::Element {
                implicit: data.implicit.contains(&obj.id()),
                initialization: data
                    .carried
                    .get(&obj.id())
                    .and_then(|c| c.initialization.clone()),
                assignment_state: match data
                    .carried
                    .get(&obj.id())
                    .and_then(|c| c.assignment_state.clone())
                {
                    Some(state) => Some(state),
                    None => derived_assignment(mm, data, header.class, header.slots as usize)
                        .map_err(FrozenError::Schema)?,
                },
                id: obj.identity().to_owned(),
                class: obj.class_key().to_owned(),
                type_identity: class.identity.to_string(),
                resource: obj.resource().to_owned(),
                start: header.start,
                end: header.end,
                values,
            });
        }
        let resources = self.resource_records()?;
        Ok(model::Model {
            schema_revision: mm.revision().to_owned(),
            resources,
            elements,
        })
    }
}

fn reference(
    data: &mut ModelData,
    item: &Value,
    problems: &mut Vec<Violation>,
    object: &str,
    feature: &str,
) -> Result<Slot, FrozenError> {
    let Value::String(id) = item else {
        problems.push(Violation::Kind {
            object: object.to_owned(),
            feature: feature.to_owned(),
        });
        return Ok(Slot::Null);
    };
    match data.strings.get(id).and_then(|sym| data.by_id.get(&sym)) {
        Some(target) => Ok(Slot::Ref(*target)),
        None => {
            problems.push(Violation::Unresolved {
                object: object.to_owned(),
                feature: feature.to_owned(),
                spelling: id.clone(),
            });
            Ok(Slot::Null)
        }
    }
}

/// The assignment state an export of the object with these slots must carry
/// so that importing it again keeps every assignment. Element values cannot
/// tell an assigned schema default from an unassigned feature (both export
/// the default), so an object with an assigned single attribute equal to a
/// non-null default exports the complete set of its assigned features; any
/// other object needs none (`None`: its values say it all). It matters for
/// required attributes too: defaults complete only unassigned slots, so
/// `ref`'s assigned `isComposite = false` must stay assigned for a
/// `bind default` not to replace it. An assigned null needs no marker.
pub(super) fn derived_assignment(
    mm: &Metamodel,
    data: &ModelData,
    class: ClassId,
    base: usize,
) -> Result<Option<model::AssignmentState>, String> {
    let Some(info) = mm.class_info(class) else {
        return Ok(None);
    };
    let mut set = Vec::new();
    let mut needed = false;
    for (index, feature) in info.stored.iter().enumerate() {
        let slot = match data.slots.get(base + index) {
            None | Some(Slot::Absent | Slot::Default) => continue,
            Some(slot) => *slot,
        };
        let name = mm.feature_name(*feature);
        if let (Some(feature_info), Some(definition)) =
            (mm.feature_info(*feature), mm.definition(*feature))
            && feature_info.kind == FeatureKind::Attribute
            && !feature_info.many
            && !definition.default.is_null()
            && decode(data, slot) == definition.default
        {
            needed = true;
        }
        set.push(name.to_owned());
    }
    if needed {
        model::AssignmentState::new(set).map(Some)
    } else {
        Ok(None)
    }
}

/// Materialize one stored slot. References become target identities.
pub(super) fn decode(data: &ModelData, slot: Slot) -> Value {
    match slot {
        Slot::Default | Slot::Absent | Slot::Null | Slot::Proxy(_) => Value::Null,
        Slot::Bool(v) => Value::Bool(v),
        Slot::Int(v) => Value::from(v),
        Slot::Str(s) => Value::String(data.strings.resolve(s).to_owned()),
        Slot::Ref(o) => data.headers.get(o.index()).map_or(Value::Null, |h| {
            Value::String(data.strings.resolve(h.id).to_owned())
        }),
        Slot::Many(list) => Value::Array(
            data.lists
                .get(list as usize)
                .map_or(&[][..], Vec::as_slice)
                .iter()
                .map(|item| decode(data, *item))
                .collect(),
        ),
        Slot::Json(j) => data.json.get(j as usize).cloned().unwrap_or(Value::Null),
    }
}
