//! Handwritten, host-independent model format contracts. Hosts own all I/O.
use crate::core::{
    model::Model,
    resources::ResourceLoadState,
    storage::{self, ModelStore},
};
use intersect_model::definitions::{conversion::ConversionRegistry, schema::Schema};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Resource {
    pub uri: String,
    pub bytes: Vec<u8>,
}

/// A decoded physical snapshot plus optional opaque checked evidence. Existing
/// codecs construct plain snapshots; serializable model metadata cannot mint
/// proof. Consuming into Model explicitly discards any ephemeral decode proof.
#[derive(Debug)]
pub struct DecodedModel {
    pub(crate) model: Model,
    pub(crate) readonly: Option<storage::ReadonlyDecodeProof>,
}
impl DecodedModel {
    pub fn new(model: Model) -> Self {
        Self {
            model,
            readonly: None,
        }
    }
    pub fn model(&self) -> &Model {
        &self.model
    }
    pub fn into_model(self) -> Model {
        self.model
    }
    #[cfg(feature = "xmi")]
    pub(crate) fn checked(
        model: Model,
        rows: Vec<storage::VerifiedReadonlyInitialization>,
        schema: &Schema,
        context: &FormatContext<'_>,
    ) -> Result<Self, String> {
        let readonly = if rows.is_empty() {
            None
        } else {
            context
                .structural_values
                .ok_or("Missing checked decode operation authority")?
                .seal_readonly(&model, schema, rows)?
        };
        Ok(Self { model, readonly })
    }
}

/// Explicit serialization scope. Neither policy performs I/O or loads resources.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ReferenceClosure {
    /// Emit only selected resources; outgoing references use the supplied context.
    SelectedOnly,
    /// Include the transitive persistent-reference dependencies already loaded.
    IncludeDependencies,
}

/// Explicit executable adapters supplied by the language package. Format metadata
/// and serialized values never install providers. The registry remains borrowed.
#[derive(Clone, Copy, Default)]
pub struct FormatContext<'a> {
    pub conversions: Option<&'a ConversionRegistry<'a>>,
    /// Explicit operation authority, never installed from serialized metadata.
    pub structural_values: Option<storage::BorrowedStructuralValues<'a>>,
    /// Current source-owned constructor authority, used only for fresh readonly import.
    pub snapshot_initializers: Option<&'a dyn storage::SnapshotInitializationSelector>,
}
impl FormatContext<'_> {
    pub fn validate(&self, schema: &Schema, model: &Model) -> Result<(), String> {
        if let Some(values) = self.structural_values {
            return values.validate_model(model, schema, self.conversions);
        }
        match self.conversions {
            Some(conversions) => model.validate_with_conversions(schema, conversions),
            None => model.validate(schema),
        }
    }
    pub(crate) fn validate_store_observed(
        &self,
        schema: &Schema,
        store: &dyn storage::ModelRead,
        observation: &storage::InvocationContext<'_>,
    ) -> Result<(), String> {
        observation.checkpoint()?;
        if let Some(values) = self.structural_values {
            values.validate_persistent(store, schema, self.conversions)?;
        } else {
            match self.conversions {
                Some(registry) => storage::validate_with_conversions_observed(
                    store,
                    schema,
                    registry,
                    observation,
                )?,
                None => storage::validate(store, schema)?,
            }
        }
        observation.checkpoint()
    }
}

/// A resource-set codec. Implementations must report unsupported information;
/// they must not fetch references or invent language semantics. Explicit format
/// contexts may invoke the language package's checked lexical adapters.
pub trait ModelFormat {
    fn id(&self) -> &str;
    fn extensions(&self) -> &[&str];
    fn media_type(&self) -> &str;
    fn decode(&self, schema: &Schema, resources: &[Resource]) -> Result<Model, String>;
    fn encode(&self, schema: &Schema, model: &Model) -> Result<Vec<Resource>, String>;
    /// Decode against explicitly retained resources and return the combined model.
    /// The caller removes resources being replaced from `retained` first.
    fn decode_with_context(
        &self,
        schema: &Schema,
        resources: &[Resource],
        retained: &Model,
    ) -> Result<Model, String> {
        if !retained.elements.is_empty() || !retained.resources.is_empty() {
            return Err(format!(
                "Format {} does not support retained resources",
                self.id()
            ));
        }
        self.decode(schema, resources)
    }
    /// Serialize a selection using the complete supplied model for references.
    /// Returned resource URIs are the selected logical addresses; hosts map them
    /// to physical locations. Existing whole-model encoders do not imply support.
    fn encode_selected(
        &self,
        _schema: &Schema,
        _model: &Model,
        _selected: &[String],
    ) -> Result<Vec<Resource>, String> {
        Err(format!(
            "Format {} does not support resource selection",
            self.id()
        ))
    }
    /// Decode with explicit providers and optional retained resource context.
    /// Existing codecs reject supplied providers unless they implement this path.
    fn decode_with_format_context(
        &self,
        schema: &Schema,
        resources: &[Resource],
        retained: Option<&Model>,
        context: &FormatContext<'_>,
    ) -> Result<Model, String> {
        if context.structural_values.is_some() || context.snapshot_initializers.is_some() {
            return Err(format!(
                "Format {} does not support structural-value providers",
                self.id()
            ));
        }
        if context.conversions.is_some() {
            return Err(format!(
                "Format {} does not support conversion providers",
                self.id()
            ));
        }
        match retained {
            Some(retained) => self.decode_with_context(schema, resources, retained),
            None => self.decode(schema, resources),
        }
    }
    /// Default-compatible opaque evidence carrier. Existing codecs return a
    /// plain Model; only canonical fresh verification can create checked proof.
    fn decode_with_format_context_and_proof(
        &self,
        schema: &Schema,
        resources: &[Resource],
        retained: Option<&Model>,
        context: &FormatContext<'_>,
    ) -> Result<DecodedModel, String> {
        self.decode_with_format_context(schema, resources, retained, context)
            .map(DecodedModel::new)
    }
    /// Encode with explicit providers and optional logical resource selection.
    fn encode_with_format_context(
        &self,
        schema: &Schema,
        model: &Model,
        selected: Option<&[String]>,
        context: &FormatContext<'_>,
    ) -> Result<Vec<Resource>, String> {
        if context.structural_values.is_some() || context.snapshot_initializers.is_some() {
            return Err(format!(
                "Format {} does not support structural-value providers",
                self.id()
            ));
        }
        if context.conversions.is_some() {
            return Err(format!(
                "Format {} does not support conversion providers",
                self.id()
            ));
        }
        match selected {
            Some(selected) => self.encode_selected(schema, model, selected),
            None => self.encode(schema, model),
        }
    }
}

/// Explicit host registration; no global mutable registry or implicit fallback.
#[derive(Default)]
pub struct FormatRegistry {
    formats: BTreeMap<String, Box<dyn ModelFormat>>,
    extensions: BTreeMap<String, String>,
}
impl FormatRegistry {
    pub fn formats(&self) -> impl Iterator<Item = &dyn ModelFormat> {
        self.formats.values().map(Box::as_ref)
    }
    /// Register a domain-specific suffix for an existing codec (for example,
    /// `.library` using XMI). Explicit mappings never overwrite one another.
    pub fn register_extension(&mut self, extension: &str, id: &str) -> Result<(), String> {
        self.get(id)?;
        let extension = extension.to_ascii_lowercase();
        if extension.is_empty() || !extension.bytes().all(|b| b.is_ascii_alphanumeric()) {
            return Err(format!("Invalid format extension: {extension}"));
        }
        if self.extensions.contains_key(&extension) {
            return Err(format!("Already registered format extension: {extension}"));
        }
        self.extensions.insert(extension, id.into());
        Ok(())
    }
    pub fn register(&mut self, format: impl ModelFormat + 'static) -> Result<(), String> {
        let id = format.id().to_owned();
        if id.is_empty() || self.formats.contains_key(&id) {
            return Err(format!("Empty or already registered format ID: {id}"));
        }
        let mut extensions = std::collections::BTreeSet::new();
        for extension in format.extensions() {
            let extension = extension.to_ascii_lowercase();
            if extension.is_empty() || !extension.bytes().all(|b| b.is_ascii_alphanumeric()) {
                return Err(format!("Invalid format extension: {extension}"));
            }
            if !extensions.insert(extension.clone()) || self.extensions.contains_key(&extension) {
                return Err(format!("Already registered format extension: {extension}"));
            }
        }
        for extension in extensions {
            self.extensions.insert(extension, id.clone());
        }
        self.formats.insert(id, Box::new(format));
        Ok(())
    }
    pub fn get(&self, id: &str) -> Result<&dyn ModelFormat, String> {
        self.formats
            .get(id)
            .map(Box::as_ref)
            .ok_or_else(|| format!("Unknown model format: {id}"))
    }
    pub fn for_uri(&self, uri: &str) -> Result<&dyn ModelFormat, String> {
        let path = uri.split(['?', '#']).next().unwrap_or(uri);
        let filename = path.rsplit('/').next().unwrap_or(path);
        let extension = filename
            .rsplit_once('.')
            .map(|(_, e)| e.to_ascii_lowercase())
            .unwrap_or_default();
        let id = self
            .extensions
            .get(&extension)
            .ok_or_else(|| format!("No model format registered for {uri}"))?;
        self.get(id)
    }
    pub fn import(
        &self,
        id: &str,
        schema: &Schema,
        resources: &[Resource],
    ) -> Result<Model, String> {
        self.import_with_format_context(id, schema, resources, None, &FormatContext::default())
    }
    pub fn export(
        &self,
        id: &str,
        schema: &Schema,
        model: &Model,
    ) -> Result<Vec<Resource>, String> {
        self.export_with_format_context(id, schema, model, None, &FormatContext::default())
    }
    /// Decode a replacement against retained resources. Language validation and
    /// atomic publication belong to the resource-set transaction boundary.
    pub fn import_with_context(
        &self,
        id: &str,
        schema: &Schema,
        resources: &[Resource],
        retained: &Model,
    ) -> Result<Model, String> {
        self.import_with_format_context(
            id,
            schema,
            resources,
            Some(retained),
            &FormatContext::default(),
        )
    }
    /// Import using explicit lexical adapters. Retained resources are caller-owned;
    /// this does not publish a resource-set replacement or run domain validation.
    pub fn import_with_format_context(
        &self,
        id: &str,
        schema: &Schema,
        resources: &[Resource],
        retained: Option<&Model>,
        context: &FormatContext<'_>,
    ) -> Result<Model, String> {
        self.import_with_format_context_and_proof(id, schema, resources, retained, context)
            .map(DecodedModel::into_model)
    }
    pub(crate) fn import_with_format_context_and_proof(
        &self,
        id: &str,
        schema: &Schema,
        resources: &[Resource],
        retained: Option<&Model>,
        context: &FormatContext<'_>,
    ) -> Result<DecodedModel, String> {
        let decoded = self
            .get(id)?
            .decode_with_format_context_and_proof(schema, resources, retained, context)?;
        context.validate(schema, &decoded.model)?;
        Ok(decoded)
    }
    /// Serialize explicitly selected loaded resources from one consistent model
    /// value. Unavailable records may be archived by a whole-model codec, but
    /// selected serialization and save acknowledgments require loaded content.
    pub fn export_selected(
        &self,
        id: &str,
        schema: &Schema,
        model: &Model,
        selected: &[String],
        closure: ReferenceClosure,
    ) -> Result<Vec<Resource>, String> {
        self.export_with_format_context(
            id,
            schema,
            model,
            Some((selected, closure)),
            &FormatContext::default(),
        )
    }
    /// Export using the same checked provider context for admission and lexical
    /// conversion. Optional selection retains the existing closure policy.
    pub fn export_with_format_context(
        &self,
        id: &str,
        schema: &Schema,
        model: &Model,
        selection: Option<(&[String], ReferenceClosure)>,
        context: &FormatContext<'_>,
    ) -> Result<Vec<Resource>, String> {
        let format = self.get(id)?;
        context.validate(schema, model)?;
        let Some((selected, closure)) = selection else {
            return format.encode_with_format_context(schema, model, None, context);
        };
        let mut selection = BTreeSet::new();
        for uri in selected {
            let record = model
                .resources
                .get(uri)
                .ok_or_else(|| format!("Unknown selected resource: {uri}"))?;
            if record.load_state != ResourceLoadState::Loaded {
                return Err(format!("Selected resource is unavailable: {uri}"));
            }
            if !selection.insert(uri.clone()) {
                return Err(format!("Duplicate selected resource: {uri}"));
            }
        }
        if selection.is_empty() {
            return Ok(Vec::new());
        }
        if closure == ReferenceClosure::IncludeDependencies {
            let elements: BTreeMap<_, _> =
                model.elements.iter().map(|e| (e.id.as_str(), e)).collect();
            loop {
                let mut added = BTreeSet::new();
                for element in &model.elements {
                    if !selection.contains(&element.resource) {
                        continue;
                    }
                    for (name, feature) in schema.features(&element.class)? {
                        if !feature.reference || feature.derived || feature.transient {
                            continue;
                        }
                        for target in crate::core::model::values(
                            element
                                .values
                                .get(&name)
                                .unwrap_or(&serde_json::Value::Null),
                            feature.upper,
                        )? {
                            let target = target.as_str().ok_or("Invalid reference identity")?;
                            let target = elements
                                .get(target)
                                .ok_or("Unavailable serialization dependency")?;
                            if !selection.contains(&target.resource) {
                                added.insert(target.resource.clone());
                            }
                        }
                    }
                }
                if added.is_empty() {
                    break;
                }
                selection.extend(added);
            }
        }
        for uri in &selection {
            let record = model
                .resources
                .get(uri)
                .ok_or("Unavailable serialization dependency")?;
            if record.load_state != ResourceLoadState::Loaded {
                return Err(format!("Serialization dependency is unavailable: {uri}"));
            }
        }
        let resources = format.encode_with_format_context(
            schema,
            model,
            Some(&selection.iter().cloned().collect::<Vec<_>>()),
            context,
        )?;
        let emitted: BTreeSet<_> = resources
            .iter()
            .map(|resource| resource.uri.clone())
            .collect();
        if emitted != selection || emitted.len() != resources.len() {
            return Err("Selected codec output does not match logical resource selection".into());
        }
        Ok(resources)
    }
    /// Replace the complete stored model atomically. For additional language
    /// checks, call within an outer storage::atomic transaction.
    pub fn import_into(
        &self,
        id: &str,
        schema: &Schema,
        resources: &[Resource],
        store: &mut impl ModelStore,
    ) -> Result<(), String> {
        let model = self.import(id, schema, resources)?;
        storage::atomic(store, |store| {
            storage::bind_schema(store, schema)?;
            let previous = storage::export(store)?;
            for element in previous.elements {
                store.remove(&element.id)?;
            }
            for uri in previous.resources.keys() {
                store.remove_resource_record(uri)?;
            }
            for element in model.elements {
                storage::insert(store, schema, element)?;
            }
            for record in model.resources.into_values() {
                store.set_resource_record(record)?;
            }
            storage::validate(store, schema)
        })
    }
}
