//! Immutable schema-scoped descriptors. Numeric slots never cross the wire.
use super::StorageResult;
use intersect_model::definitions::schema::{Feature, Schema};
use std::{
    collections::BTreeMap,
    hash::{Hash, Hasher},
    sync::Arc,
};

#[derive(Clone, Debug)]
pub struct TypeId {
    pub(super) scope: Arc<()>,
    pub(super) index: usize,
}
#[derive(Clone, Debug)]
pub struct FeatureId {
    pub(super) owner: TypeId,
    pub(super) index: usize,
}
impl PartialEq for TypeId {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.scope, &other.scope) && self.index == other.index
    }
}
impl Eq for TypeId {}
impl Hash for TypeId {
    fn hash<H: Hasher>(&self, state: &mut H) {
        Arc::as_ptr(&self.scope).hash(state);
        self.index.hash(state);
    }
}
impl PartialEq for FeatureId {
    fn eq(&self, other: &Self) -> bool {
        self.owner == other.owner && self.index == other.index
    }
}
impl Eq for FeatureId {}
impl Hash for FeatureId {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.owner.hash(state);
        self.index.hash(state);
    }
}

#[derive(Debug)]
pub struct FeatureDescriptor {
    pub(super) id: FeatureId,
    pub(super) definition: Feature,
}
impl FeatureDescriptor {
    /// A class-local slot, including inherited features; this is not the identity
    /// of the declaring feature in the source metamodel.
    pub fn id(&self) -> FeatureId {
        self.id.clone()
    }
    pub fn definition(&self) -> &Feature {
        &self.definition
    }
    pub fn name(&self) -> &str {
        &self.definition.name
    }
}
#[derive(Debug)]
pub struct TypeDescriptor {
    pub(super) id: TypeId,
    pub(super) name: Arc<str>,
    pub(super) identity: Arc<str>,
    pub(super) features: BTreeMap<Arc<str>, FeatureDescriptor>,
}
impl TypeDescriptor {
    pub fn id(&self) -> TypeId {
        self.id.clone()
    }
    pub fn name(&self) -> &str {
        &self.name
    }
    pub fn identity(&self) -> &str {
        &self.identity
    }
    pub fn feature(&self, name: &str) -> Option<&FeatureDescriptor> {
        self.features.get(name)
    }
    pub fn features(&self) -> impl Iterator<Item = &FeatureDescriptor> {
        self.features.values()
    }
    pub fn feature_descriptor(&self, id: &FeatureId) -> StorageResult<&FeatureDescriptor> {
        if id.owner != self.id {
            return Err("Feature handle belongs to a different type or schema context".into());
        }
        self.features
            .values()
            .nth(id.index)
            .ok_or_else(|| "Unknown feature handle".into())
    }
}
#[derive(Debug)]
pub struct SchemaDescriptor {
    pub(super) source: Schema,
    scope: Arc<()>,
    pub(super) types: BTreeMap<Arc<str>, Arc<TypeDescriptor>>,
}
impl SchemaDescriptor {
    pub(in crate::core) fn new(schema: &Schema) -> StorageResult<Self> {
        let scope = Arc::new(());
        let mut types = BTreeMap::new();
        for (index, name) in schema.classes.keys().enumerate() {
            let id = TypeId {
                scope: scope.clone(),
                index,
            };
            let features = schema
                .features(name)?
                .into_iter()
                .enumerate()
                .map(|(index, (name, definition))| {
                    (
                        Arc::from(name),
                        FeatureDescriptor {
                            id: FeatureId {
                                owner: id.clone(),
                                index,
                            },
                            definition,
                        },
                    )
                })
                .collect();
            let descriptor = Arc::new(TypeDescriptor {
                id,
                name: Arc::from(name.as_str()),
                identity: Arc::from(schema.identity(name)),
                features,
            });
            types.insert(descriptor.name.clone(), descriptor);
        }
        Ok(Self {
            source: schema.clone(),
            scope,
            types,
        })
    }
    pub fn revision(&self) -> &str {
        &self.source.revision
    }
    pub fn type_id(&self, name: &str) -> Option<TypeId> {
        self.class(name).map(TypeDescriptor::id)
    }
    pub(in crate::core) fn class(&self, name: &str) -> Option<&TypeDescriptor> {
        self.types.get(name).map(AsRef::as_ref)
    }
    pub fn type_descriptor(&self, id: &TypeId) -> StorageResult<&TypeDescriptor> {
        if !Arc::ptr_eq(&id.scope, &self.scope) {
            return Err("Type handle belongs to a different schema context".into());
        }
        self.types
            .values()
            .nth(id.index)
            .map(AsRef::as_ref)
            .ok_or_else(|| "Unknown type handle".into())
    }
    pub fn types(&self) -> impl Iterator<Item = &TypeDescriptor> {
        self.types.values().map(AsRef::as_ref)
    }
}
