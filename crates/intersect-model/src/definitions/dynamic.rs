//! Handwritten runtime schema authoring. Handles retain their schema context;
//! finalization freezes definitions without invalidating those handles.
use crate::definitions::schema::{Class, Feature, Schema};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex, OnceLock},
};

pub type DefinitionResult<T> = Result<T, String>;

#[derive(Debug)]
struct Context {
    draft: Mutex<Option<Schema>>,
    frozen: OnceLock<Arc<Schema>>,
}
impl Context {
    fn edit<T>(&self, f: impl FnOnce(&mut Schema) -> DefinitionResult<T>) -> DefinitionResult<T> {
        let mut guard = self
            .draft
            .lock()
            .map_err(|_| "Schema builder lock poisoned")?;
        if self.frozen.get().is_some() {
            return Err("Schema is frozen".into());
        }
        f(guard.as_mut().ok_or("Schema is frozen")?)
    }
    fn read<T>(&self, f: impl FnOnce(&Schema) -> DefinitionResult<T>) -> DefinitionResult<T> {
        if let Some(schema) = self.frozen.get() {
            return f(schema);
        }
        let guard = self
            .draft
            .lock()
            .map_err(|_| "Schema builder lock poisoned")?;
        if let Some(schema) = self.frozen.get() {
            return f(schema);
        }
        f(guard.as_ref().ok_or("Schema is frozen")?)
    }
}

/// Builder and handles own their context; no process-global registry is used.
pub struct SchemaBuilder {
    context: Arc<Context>,
}
impl SchemaBuilder {
    pub fn new(name: impl Into<String>, uri: impl Into<String>) -> Self {
        Self {
            context: Arc::new(Context {
                draft: Mutex::new(Some(Schema {
                    runtime_feature_domains: BTreeMap::new(),
                    reference_top: None,
                    raw_reference_bounds: BTreeMap::new(),
                    runtime_warnings: Vec::new(),
                    declared_operation_selections: None,
                    instantiations: BTreeMap::new(),
                    generic_signatures: None,
                    conversion_contracts: Vec::new(),
                    applied_conversion_contracts: Vec::new(),
                    constructor_contracts: Vec::new(),
                    enumerations: BTreeMap::new(),
                    datatypes: BTreeMap::new(),
                    declarations: BTreeMap::new(),
                    revision: String::new(),
                    name: name.into(),
                    uri: uri.into(),
                    classes: BTreeMap::new(),
                })),
                frozen: OnceLock::new(),
            }),
        }
    }
    /// Declare a class. Use `DynamicSchema::class` for lookup after finalization.
    pub fn class(&mut self, name: &str) -> DefinitionResult<EClass> {
        self.context.edit(|schema| {
            if name.trim().is_empty() || name.contains('/') {
                return Err("Invalid class name".into());
            }
            if schema.classes.contains_key(name) {
                return Err(format!("Duplicate class {name}"));
            }
            schema.classes.insert(
                name.into(),
                Class {
                    interface_type: false,
                    parents: Vec::new(),
                    feature_order: Vec::new(),
                    feature_declarations: BTreeMap::new(),
                    operations: Vec::new(),
                    name: name.into(),
                    abstract_type: false,
                    features: BTreeMap::new(),
                },
            );
            Ok(())
        })?;
        Ok(EClass {
            context: self.context.clone(),
            name: name.into(),
        })
    }
    pub fn opposites(&mut self, a: &EFeature, b: &EFeature) -> DefinitionResult<()> {
        if !Arc::ptr_eq(&self.context, &a.class.context)
            || !Arc::ptr_eq(&self.context, &b.class.context)
        {
            return Err("Opposites belong to a different schema context".into());
        }
        self.context.edit(|schema| {
            let left = declared(schema, a)?;
            let right = declared(schema, b)?;
            if !left.reference || !right.reference || a == b {
                return Err("Opposites require two distinct references".into());
            }
            let a_id = format!("{}/{}", a.class.name, a.name);
            let b_id = format!("{}/{}", b.class.name, b.name);
            if left.opposite.as_ref().is_some_and(|s| s != &b_id)
                || right.opposite.as_ref().is_some_and(|s| s != &a_id)
            {
                return Err("Opposite already declared".into());
            }
            declared_mut(schema, a)?.opposite = Some(b_id);
            declared_mut(schema, b)?.opposite = Some(a_id);
            Ok(())
        })
    }
    /// Validation failure leaves the builder editable. Success freezes all handles.
    pub fn build(&mut self) -> DefinitionResult<DynamicSchema> {
        let source = self.context.edit(|schema| {
            schema.validate()?;
            schema.revision = programmatic_revision(schema)?;
            let source = Arc::new(schema.clone());
            self.context
                .frozen
                .set(source.clone())
                .map_err(|_| "Schema is frozen")?;
            Ok(source)
        })?;
        *self
            .context
            .draft
            .lock()
            .map_err(|_| "Schema builder lock poisoned")? = None;
        Ok(DynamicSchema {
            context: self.context.clone(),
            source,
        })
    }
}

fn programmatic_revision(schema: &Schema) -> DefinitionResult<String> {
    let bytes = serde_json::to_vec(&(1u32, &schema.uri, &schema.name, &schema.classes))
        .map_err(|e| e.to_string())?;
    Ok(format!("dynamic-v1:{:x}", Sha256::digest(bytes)))
}

/// Immutable validated schema. Clones share the handle context.
#[derive(Clone, Debug)]
pub struct DynamicSchema {
    context: Arc<Context>,
    source: Arc<Schema>,
}
impl DynamicSchema {
    /// Accept an imported schema without changing its revision or provenance.
    pub fn from_schema(schema: Schema) -> DefinitionResult<Self> {
        schema.validate()?;
        if schema.revision.trim().is_empty() {
            return Err("Schema revision is empty".into());
        }
        if schema.revision.starts_with("dynamic-v1:")
            && schema.revision != programmatic_revision(&schema)?
        {
            return Err("Programmatic schema revision does not match definitions".into());
        }
        let source = Arc::new(schema);
        let frozen = OnceLock::new();
        frozen
            .set(source.clone())
            .map_err(|_| "Schema already frozen")?;
        Ok(Self {
            context: Arc::new(Context {
                draft: Mutex::new(None),
                frozen,
            }),
            source,
        })
    }
    pub fn as_schema(&self) -> &Schema {
        &self.source
    }
    pub fn class(&self, name: &str) -> DefinitionResult<EClass> {
        if !self.source.classes.contains_key(name) {
            return Err(format!("Unknown class {name}"));
        }
        Ok(EClass {
            context: self.context.clone(),
            name: name.into(),
        })
    }
    pub fn check_class(&self, class: &EClass) -> DefinitionResult<()> {
        if !Arc::ptr_eq(&self.context, &class.context) {
            return Err("Class belongs to a different schema context".into());
        }
        Ok(())
    }
    pub(crate) fn check_feature(
        &self,
        actual_class: &str,
        feature: &EFeature,
    ) -> DefinitionResult<&Feature> {
        self.check_class(&feature.class)?;
        if !self.source.conforms(actual_class, feature.class.name()) {
            return Err("Feature does not belong to object's class or ancestors".into());
        }
        declared(&self.source, feature)
    }
}

#[derive(Clone, Debug)]
pub struct EClass {
    context: Arc<Context>,
    name: String,
}
impl PartialEq for EClass {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.context, &other.context) && self.name == other.name
    }
}
impl Eq for EClass {}
impl EClass {
    pub fn name(&self) -> &str {
        &self.name
    }
    pub fn abstract_type(&self) -> DefinitionResult<()> {
        self.context.edit(|s| {
            s.classes
                .get_mut(&self.name)
                .ok_or("Unknown class")?
                .abstract_type = true;
            Ok(())
        })
    }
    pub fn extends(&self, parent: &EClass) -> DefinitionResult<()> {
        if !Arc::ptr_eq(&self.context, &parent.context) {
            return Err("Parent belongs to a different schema context".into());
        }
        self.context.edit(|s| {
            let class = s.classes.get_mut(&self.name).ok_or("Unknown class")?;
            if !class.parents.is_empty() {
                return Err("Single inheritance only; parent already assigned".into());
            }
            class.parents = vec![parent.name.clone()];
            Ok(())
        })
    }
    /// Lookup returns the declaring ancestor's feature handle.
    pub fn feature(&self, name: &str) -> DefinitionResult<EFeature> {
        self.context.read(|schema| {
            schema.features(&self.name)?;
            let mut owner = self.name.as_str();
            loop {
                let class = schema.classes.get(owner).ok_or("Unknown class")?;
                if class.features.contains_key(name) {
                    return Ok(EFeature {
                        class: EClass {
                            context: self.context.clone(),
                            name: owner.into(),
                        },
                        name: name.into(),
                    });
                }
                owner = class
                    .parents
                    .first()
                    .map(String::as_str)
                    .ok_or_else(|| format!("Unknown feature {}.{name}", self.name))?;
            }
        })
    }
    pub fn attribute(&self, name: &str, scalar: ScalarType) -> FeatureBuilder {
        FeatureBuilder::new(self, name, scalar.name(), false, None)
    }
    pub fn reference(&self, name: &str, target: &EClass) -> FeatureBuilder {
        FeatureBuilder::new(self, name, &target.name, true, Some(target.clone()))
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EFeature {
    class: EClass,
    name: String,
}
impl EFeature {
    pub fn name(&self) -> &str {
        &self.name
    }
    pub fn declaring_class(&self) -> &EClass {
        &self.class
    }
}
fn declared<'a>(schema: &'a Schema, handle: &EFeature) -> DefinitionResult<&'a Feature> {
    schema
        .classes
        .get(&handle.class.name)
        .and_then(|c| c.features.get(&handle.name))
        .ok_or_else(|| "Unknown feature".into())
}
fn declared_mut<'a>(
    schema: &'a mut Schema,
    handle: &EFeature,
) -> DefinitionResult<&'a mut Feature> {
    schema
        .classes
        .get_mut(&handle.class.name)
        .and_then(|c| c.features.get_mut(&handle.name))
        .ok_or_else(|| "Unknown feature".into())
}

#[derive(Clone, Copy, Debug)]
pub enum ScalarType {
    String,
    Integer,
    Boolean,
}
impl ScalarType {
    fn name(self) -> &'static str {
        match self {
            Self::String => "EString",
            Self::Integer => "EInt",
            Self::Boolean => "EBoolean",
        }
    }
}

/// Configuration is committed only by a fallible terminal method. Use `build`
/// after combining options, or `required`, `optional`, `default`, `containment`.
#[must_use = "Finish the feature declaration with a fallible terminal method"]
pub struct FeatureBuilder {
    class: EClass,
    feature: Feature,
    target: Option<EClass>,
}
impl FeatureBuilder {
    fn new(
        class: &EClass,
        name: &str,
        target: &str,
        reference: bool,
        target_class: Option<EClass>,
    ) -> Self {
        Self {
            class: class.clone(),
            target: target_class,
            feature: Feature {
                datatype: None,
                name: name.into(),
                target: target.into(),
                reference,
                containment: false,
                opposite: None,
                lower: 0,
                upper: 1,
                unique: true,
                ordered: true,
                derived: false,
                unsettable: false,
                changeable: true,
                transient: false,
                volatile: false,
                default: if reference {
                    Value::Null
                } else {
                    match target {
                        "EInt" => Value::from(0),
                        "EBoolean" => Value::from(false),
                        _ => Value::Null,
                    }
                },
            },
        }
    }
    pub fn many(self) -> Self {
        self.bounds(0, -1)
    }
    pub fn bounds(mut self, lower: usize, upper: i32) -> Self {
        self.feature.lower = lower;
        self.feature.upper = upper;
        self.feature.default = if upper != 1 {
            Value::Array(vec![])
        } else if self.feature.reference {
            Value::Null
        } else {
            match self.feature.target.as_str() {
                "EInt" => Value::from(0),
                "EBoolean" => Value::from(false),
                _ => Value::Null,
            }
        };
        self
    }
    /// Retains the Ecore flag while using dynamic storage for non-derived features.
    pub fn volatile(mut self, volatile: bool) -> Self {
        self.feature.volatile = volatile;
        self
    }
    pub fn transient(mut self, transient: bool) -> Self {
        self.feature.transient = transient;
        self
    }
    pub fn changeable(mut self, changeable: bool) -> Self {
        self.feature.changeable = changeable;
        self
    }
    pub fn unsettable(mut self, unsettable: bool) -> Self {
        self.feature.unsettable = unsettable;
        self
    }
    pub fn ordered(mut self, ordered: bool) -> Self {
        self.feature.ordered = ordered;
        self
    }
    pub fn unique(mut self, unique: bool) -> Self {
        self.feature.unique = unique;
        self
    }
    pub fn required(mut self) -> DefinitionResult<EFeature> {
        self.feature.lower = 1;
        self.build()
    }
    pub fn optional(mut self) -> DefinitionResult<EFeature> {
        self.feature.lower = 0;
        self.build()
    }
    pub fn default(mut self, value: impl Into<Value>) -> DefinitionResult<EFeature> {
        self.feature.default = value.into();
        self.build()
    }
    pub fn containment(mut self) -> DefinitionResult<EFeature> {
        self.feature.containment = true;
        self.build()
    }
    pub fn build(self) -> DefinitionResult<EFeature> {
        if let Some(target) = &self.target
            && !Arc::ptr_eq(&self.class.context, &target.context)
        {
            return Err("Reference target belongs to a different schema context".into());
        }
        self.class.context.edit(|schema| {
            if self.feature.name.trim().is_empty() || self.feature.name.contains('/') {
                return Err("Invalid feature name".into());
            }
            let class = schema
                .classes
                .get_mut(&self.class.name)
                .ok_or("Unknown class")?;
            if class.features.contains_key(&self.feature.name) {
                return Err("Duplicate feature".into());
            }
            class
                .features
                .insert(self.feature.name.clone(), self.feature.clone());
            Ok(())
        })?;
        Ok(EFeature {
            class: self.class,
            name: self.feature.name,
        })
    }
}
