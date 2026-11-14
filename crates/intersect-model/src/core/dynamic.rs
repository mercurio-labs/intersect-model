//! Handwritten handle-based facade over shared storage and structural services.
use crate::core::{
    api::{BasicModel, EditSession, ModelEdit, TransactionalModel},
    frozen::FrozenStore,
    interchange::{FormatRegistry, Resource},
    model::Model,
    storage::{self, ModelRead, ObjectHandle},
};
use intersect_model::definitions::{
    dynamic::{DynamicSchema, EClass, EFeature},
    schema::Schema,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;

pub type ModelResult<T> = Result<T, String>;

pub struct DynamicModel {
    schema: DynamicSchema,
    model: TransactionalModel,
}
impl DynamicModel {
    pub fn new(schema: DynamicSchema) -> ModelResult<Self> {
        let basic = BasicModel::new(schema.as_schema().clone())?;
        Self::from_basic(schema, basic)
    }

    /// Adopt checked ownership without rebuilding storage or invalidating handles.
    /// A dynamic model always starts with a fully valid structural graph.
    pub fn from_basic(schema: DynamicSchema, basic: BasicModel) -> ModelResult<Self> {
        if basic.schema() != schema.as_schema() {
            return Err("Dynamic and basic model schemas differ".into());
        }
        if schema
            .as_schema()
            .classes
            .values()
            .any(|c| c.features.values().any(|f| f.derived))
        {
            return Err(
                "DynamicModel does not execute derived features; use a language provider".into(),
            );
        }
        basic.validate()?;
        Ok(Self {
            schema,
            model: TransactionalModel::new(basic),
        })
    }

    /// Transfer checked ownership for basic edits or `VersionedModel::new`.
    /// Keep a clone of `schema()` if returning to this handle-based facade.
    pub fn into_basic(self) -> BasicModel {
        self.model.into_basic()
    }

    /// Model reads for queries and language-owned services.
    pub fn as_read(&self) -> &impl ModelRead {
        &self.model
    }
    pub fn schema(&self) -> &DynamicSchema {
        &self.schema
    }
    pub fn object(&self, id: &str) -> ModelResult<ObjectHandle> {
        self.model
            .reader()
            .handle(id)
            .ok_or_else(|| format!("Unknown object {id}"))
    }
    pub fn class(&self, object: &ObjectHandle) -> ModelResult<EClass> {
        let (_, class) = self.model.reader().handle_target(object)?;
        self.schema.class(class)
    }
    pub fn get(&self, object: &ObjectHandle, feature: &EFeature) -> ModelResult<DynamicValue<'_>> {
        get(self.model.reader(), &self.schema, object, feature)
    }
    pub fn transaction<T>(
        &mut self,
        edit: impl FnOnce(&mut DynamicTransaction<'_>) -> ModelResult<T>,
    ) -> ModelResult<T> {
        self.transaction_with(edit, |_| Ok(()))
    }

    /// Apply the shared transaction boundary and a per-call domain validator.
    pub fn transaction_with<T>(
        &mut self,
        edit: impl FnOnce(&mut DynamicTransaction<'_>) -> ModelResult<T>,
        validate_language: impl FnOnce(&EditSession<'_, FrozenStore>) -> ModelResult<()>,
    ) -> ModelResult<T> {
        let schema = &self.schema;
        self.model.transaction_with(
            |session| {
                edit(&mut DynamicTransaction {
                    edit: session.reborrow(),
                    schema,
                })
            },
            validate_language,
        )
    }
    /// Explicit compatibility materialization, including schema and resource identities.
    pub fn to_json(&self) -> ModelResult<String> {
        serde_json::to_string(&Snapshot {
            version: 1,
            schema: self.schema.as_schema().clone(),
            model: self.to_model()?,
        })
        .map_err(|e| e.to_string())
    }
    /// Explicit owned projection for codecs and other shared-core consumers.
    pub fn to_model(&self) -> ModelResult<Model> {
        storage::export(&self.model)
    }
    pub fn from_model(schema: DynamicSchema, source: Model) -> ModelResult<Self> {
        if source.schema_revision != schema.as_schema().revision {
            return Err("Model schema revision mismatch".into());
        }
        let store = FrozenStore::from_materialized(schema.as_schema(), &source)?;
        let basic = BasicModel::from_store(schema.as_schema().clone(), store)?;
        Self::from_basic(schema, basic)
    }
    pub fn export(&self, formats: &FormatRegistry, format: &str) -> ModelResult<Vec<Resource>> {
        formats.export(format, self.schema.as_schema(), &self.to_model()?)
    }
    pub fn import(
        schema: DynamicSchema,
        formats: &FormatRegistry,
        format: &str,
        resources: &[Resource],
    ) -> ModelResult<Self> {
        let model = formats.import(format, schema.as_schema(), resources)?;
        Self::from_model(schema, model)
    }
    /// Reload uses a fresh schema context and fresh object handles; discover new
    /// handles by name/identity. This never revives handles from the old model.
    pub fn from_json(json: &str) -> ModelResult<Self> {
        let snapshot: Snapshot = serde_json::from_str(json).map_err(|e| e.to_string())?;
        if snapshot.version != 1 {
            return Err("Unsupported dynamic model snapshot version".into());
        }
        if snapshot.model.schema_revision != snapshot.schema.revision {
            return Err("Snapshot schema revision mismatch".into());
        }
        Self::from_model(DynamicSchema::from_schema(snapshot.schema)?, snapshot.model)
    }
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Snapshot {
    version: u32,
    schema: Schema,
    model: Model,
}

/// A schema-checked containment endpoint in the current model context.
pub struct DynamicContainmentDestination<'a> {
    pub owner: &'a ObjectHandle,
    pub feature: &'a EFeature,
    pub position: Option<usize>,
}

pub struct DynamicTransaction<'a> {
    edit: EditSession<'a, FrozenStore>,
    schema: &'a DynamicSchema,
}
impl<'model> DynamicTransaction<'model> {
    /// Delete loaded objects or their subtrees through shared checked cleanup.
    pub fn delete_objects(
        &mut self,
        roots: &[ObjectHandle],
        selection: storage::DeleteSelection,
        incoming: storage::IncomingReferences,
        budget: &mut crate::core::query::QueryBudget,
    ) -> ModelResult<Vec<String>> {
        let ids = roots
            .iter()
            .map(|object| {
                budget.charge()?;
                let (id, _) = self.edit.reader().handle_target(object)?;
                Ok(id.to_owned())
            })
            .collect::<ModelResult<Vec<_>>>()?;
        let ids = ids.iter().map(String::as_str).collect::<Vec<_>>();
        self.edit.delete_objects(
            storage::DeleteRequest {
                roots: &ids,
                selection,
                incoming,
            },
            None,
            budget,
        )
    }

    /// Move or detach an object, checking local handles and feature ownership.
    pub fn reparent(
        &mut self,
        object: &ObjectHandle,
        destination: Option<DynamicContainmentDestination<'_>>,
        budget: &mut crate::core::query::QueryBudget,
    ) -> ModelResult<bool> {
        budget.charge()?;
        let (id, _) = self.edit.reader().handle_target(object)?;
        let id = id.to_owned();
        let owner = if let Some(destination) = &destination {
            let (id, class) = self.edit.reader().handle_target(destination.owner)?;
            self.schema.check_feature(class, destination.feature)?;
            Some(id.to_owned())
        } else {
            None
        };
        let destination = destination
            .as_ref()
            .zip(owner.as_deref())
            .map(|(target, owner)| storage::ContainmentDestination {
                owner,
                feature: target.feature.name(),
                position: target.position,
            });
        self.edit.reparent(
            storage::ReparentRequest {
                object: &id,
                destination,
            },
            None,
            budget,
        )
    }

    pub fn create_resource<'a>(
        &'a mut self,
        uri: &str,
    ) -> ModelResult<DynamicResource<'a, 'model>> {
        self.edit.create_resource(uri)?;
        self.resource(uri)
    }
    pub fn reorder_roots(&mut self, uri: &str, roots: &[String]) -> ModelResult<()> {
        self.edit.reorder_roots(uri, roots)
    }

    pub fn resource<'a>(&'a mut self, uri: &str) -> ModelResult<DynamicResource<'a, 'model>> {
        if uri.trim().is_empty() {
            return Err("Resource URI must be nonempty".into());
        }
        if self.edit.resource_record(uri)?.is_none() {
            return Err(format!("Unknown resource {uri}"));
        }
        Ok(DynamicResource {
            tx: self,
            uri: uri.into(),
        })
    }
    pub fn create(
        &mut self,
        class: &EClass,
        id: &str,
        resource: &str,
    ) -> ModelResult<ObjectHandle> {
        self.schema.check_class(class)?;
        if resource.trim().is_empty() {
            return Err("Resource URI must be nonempty".into());
        }
        self.edit.create(id, class.name(), resource)?;
        self.edit
            .reader()
            .handle(id)
            .ok_or_else(|| "Created object is unavailable".into())
    }
    pub fn get(&self, object: &ObjectHandle, feature: &EFeature) -> ModelResult<DynamicValue<'_>> {
        get(self.edit.reader(), self.schema, object, feature)
    }
    pub fn set(
        &mut self,
        object: &ObjectHandle,
        feature: &EFeature,
        value: impl Into<DynamicInput>,
    ) -> ModelResult<()> {
        let (id, class) = self.edit.reader().handle_target(object)?;
        let definition = self.schema.check_feature(class, feature)?;
        let id = id.to_owned();
        let value = value
            .into()
            .encode(self.edit.reader(), definition.reference)?;
        self.edit.set(&id, feature.name(), value)
    }
    pub fn clear(&mut self, object: &ObjectHandle, feature: &EFeature) -> ModelResult<()> {
        let (_, class) = self.edit.reader().handle_target(object)?;
        let definition = self.schema.check_feature(class, feature)?;
        self.set(
            object,
            feature,
            if definition.upper == 1 {
                Value::Null
            } else {
                Value::Array(vec![])
            },
        )
    }
    /// Append one value, using the same opposite mutation path as replacement.
    pub fn add(
        &mut self,
        object: &ObjectHandle,
        feature: &EFeature,
        value: impl Into<DynamicInput>,
    ) -> ModelResult<()> {
        let (id, class) = self.edit.reader().handle_target(object)?;
        let definition = self.schema.check_feature(class, feature)?;
        if definition.upper == 1 {
            return Err("add requires a many-valued feature".into());
        }
        let id = id.to_owned();
        let value = value
            .into()
            .encode(self.edit.reader(), definition.reference)?;
        if value.is_array() || value.is_null() {
            return Err("add requires one non-null value".into());
        }
        let mut list = self
            .edit
            .value(&id, feature.name())?
            .ok_or("Missing collection")?;
        list.as_array_mut()
            .ok_or("Feature is not a collection")?
            .push(value);
        self.edit.set(&id, feature.name(), list)
    }
}

/// A transaction view selecting the resource for subsequent creations.
pub struct DynamicResource<'a, 'model> {
    tx: &'a mut DynamicTransaction<'model>,
    uri: String,
}
impl DynamicResource<'_, '_> {
    pub fn create(&mut self, class: &EClass, id: &str) -> ModelResult<ObjectHandle> {
        self.tx.create(class, id, &self.uri)
    }
    pub fn set(
        &mut self,
        object: &ObjectHandle,
        feature: &EFeature,
        value: impl Into<DynamicInput>,
    ) -> ModelResult<()> {
        self.tx.set(object, feature, value)
    }
    pub fn add(
        &mut self,
        object: &ObjectHandle,
        feature: &EFeature,
        value: impl Into<DynamicInput>,
    ) -> ModelResult<()> {
        self.tx.add(object, feature, value)
    }
    pub fn clear(&mut self, object: &ObjectHandle, feature: &EFeature) -> ModelResult<()> {
        self.tx.clear(object, feature)
    }
    pub fn get(&self, object: &ObjectHandle, feature: &EFeature) -> ModelResult<DynamicValue<'_>> {
        self.tx.get(object, feature)
    }
}

/// Attribute data and references stay distinct at this API boundary. References
/// must be live handles in this model, never raw strings masquerading as IDs.
pub enum DynamicInput {
    Attribute(Value),
    Object(ObjectHandle),
    Objects(Vec<ObjectHandle>),
}
impl DynamicInput {
    fn encode(self, store: &FrozenStore, reference: bool) -> ModelResult<Value> {
        match self {
            Self::Attribute(value)
                if !reference || value.is_null() || value.as_array().is_some_and(Vec::is_empty) =>
            {
                Ok(value)
            }
            Self::Attribute(_) => Err("Reference values require object handles".into()),
            Self::Object(handle) if reference => {
                let (id, _) = store.handle_target(&handle)?;
                Ok(Value::String(id.into()))
            }
            Self::Objects(handles) if reference => handles
                .iter()
                .map(|handle| {
                    let (id, _) = store.handle_target(handle)?;
                    Ok(Value::String(id.into()))
                })
                .collect::<ModelResult<Vec<_>>>()
                .map(Value::Array),
            _ => Err("Attribute values cannot be object handles".into()),
        }
    }
}
impl From<Value> for DynamicInput {
    fn from(value: Value) -> Self {
        Self::Attribute(value)
    }
}
impl From<&str> for DynamicInput {
    fn from(value: &str) -> Self {
        Self::Attribute(value.into())
    }
}
impl From<String> for DynamicInput {
    fn from(value: String) -> Self {
        Self::Attribute(value.into())
    }
}
impl From<i32> for DynamicInput {
    fn from(value: i32) -> Self {
        Self::Attribute(value.into())
    }
}
impl From<bool> for DynamicInput {
    fn from(value: bool) -> Self {
        Self::Attribute(value.into())
    }
}
impl From<ObjectHandle> for DynamicInput {
    fn from(value: ObjectHandle) -> Self {
        Self::Object(value)
    }
}
impl From<&ObjectHandle> for DynamicInput {
    fn from(value: &ObjectHandle) -> Self {
        Self::Object(value.clone())
    }
}
impl From<Vec<ObjectHandle>> for DynamicInput {
    fn from(value: Vec<ObjectHandle>) -> Self {
        Self::Objects(value)
    }
}

/// A single stored value and the store that scopes its object handles.
pub struct DynamicValue<'a> {
    store: &'a FrozenStore,
    value: Value,
    reference: bool,
    many: bool,
}
impl DynamicValue<'_> {
    pub fn as_str(&self) -> ModelResult<&str> {
        if self.reference {
            return Err("Value is a reference, not a string attribute".into());
        }
        self.value
            .as_str()
            .ok_or_else(|| "Value is not a string".into())
    }
    pub fn as_i32(&self) -> ModelResult<i32> {
        self.value
            .as_i64()
            .and_then(|n| i32::try_from(n).ok())
            .ok_or_else(|| "Value is not an i32".into())
    }
    pub fn as_bool(&self) -> ModelResult<bool> {
        self.value
            .as_bool()
            .ok_or_else(|| "Value is not a boolean".into())
    }
    pub fn as_object(&self) -> ModelResult<Option<ObjectHandle>> {
        if !self.reference || self.many {
            return Err("Value is not a singular reference".into());
        }
        if self.value.is_null() {
            return Ok(None);
        }
        self.store
            .handle(self.value.as_str().ok_or("Invalid reference")?)
            .map(Some)
            .ok_or_else(|| "Reference target unavailable".into())
    }
    pub fn as_objects(&self) -> ModelResult<Vec<ObjectHandle>> {
        if !self.reference || !self.many {
            return Err("Value is not a reference collection".into());
        }
        self.value
            .as_array()
            .ok_or("Missing collection")?
            .iter()
            .map(|v| {
                self.store
                    .handle(v.as_str().ok_or("Invalid reference")?)
                    .ok_or_else(|| "Reference target unavailable".into())
            })
            .collect()
    }
    pub fn to_json(&self) -> Value {
        self.value.clone()
    }
}
fn get<'a>(
    store: &'a FrozenStore,
    schema: &DynamicSchema,
    object: &ObjectHandle,
    feature: &EFeature,
) -> ModelResult<DynamicValue<'a>> {
    let (id, class) = store.handle_target(object)?;
    let definition = schema.check_feature(class, feature)?;
    Ok(DynamicValue {
        store,
        value: store
            .value(id, feature.name())?
            .ok_or("Feature has no stored value")?,
        reference: definition.reference,
        many: definition.upper != 1,
    })
}
