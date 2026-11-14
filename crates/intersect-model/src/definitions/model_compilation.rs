//! Handwritten checked model compilation and source-owned inspection results.
//! No model result admits grammar syntax, installs providers or validates values.
use super::{CompiledStructure, ModelDefinitionView, StructureSelection};
use crate::definitions::{
    bundle::{DefinitionBundle, SourceId},
    compiler,
    conversion::ConversionRegistry,
    ecore::ParseLimits,
    ecore_migration::BundleMigrationReport,
    generic_types::RuntimeType,
    preparation::PreparedModelDefinition,
    schema::{Diagnostic, Schema, Semantics},
    setting::ComputedSettingContract,
};
use std::sync::Arc;

/// A request for model declaration requirements, never an execution approval.
/// Inspection retains missing providers as requirements; supplied invalid or
/// duplicate declarations still fail through the same model checker.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ModelPreparationMode {
    Strict,
    Inspection,
}
impl ModelPreparationMode {
    pub(crate) fn allows_missing(self) -> bool {
        self == Self::Inspection
    }
}

/// A fresh structure and checked behavior source from that same immutable world.
/// No constructor from metadata, flags or a serialized receipt is available.
///
/// ```compile_fail
/// use intersect_model::definitions::model_definition::CheckedModelCompilation;
/// fn load(input: &str) -> Result<CheckedModelCompilation<'_>, serde_json::Error> {
///     serde_json::from_str(input)
/// }
/// ```
pub struct CheckedModelCompilation<'inputs> {
    pub(super) structure: CompiledStructure<'inputs>,
    behavior: &'inputs SourceId,
}
impl<'inputs> CompiledStructure<'inputs> {
    /// Check source behavior before frontend composition. Consuming this owner
    /// prevents a check from one structure being attached to another structure.
    /// The existing source checker and its independent limits remain authoritative.
    pub fn check_behavior(
        self,
        entry: &SourceId,
        conversions: Option<&ConversionRegistry<'_>>,
        limits: ParseLimits,
    ) -> Result<CheckedModelCompilation<'inputs>, Vec<Diagnostic>> {
        let bundle = self.sources();
        let source = bundle
            .source(entry)
            .map_err(|message| vec![Diagnostic::new(&entry.uri, 0, 0, message)])?;
        crate::definitions::rcore_behavior::checking::check_with_source_limits(
            bundle,
            entry,
            conversions,
            limits,
        )
        .map_err(|errors| {
            errors
                .into_iter()
                .map(|error| Diagnostic::from_span(error.span, error.message))
                .collect::<Vec<_>>()
        })?;
        Ok(CheckedModelCompilation {
            structure: self,
            behavior: &source.id,
        })
    }
}
impl<'inputs> CheckedModelCompilation<'inputs> {
    /// Transfer ordinary declaration data. It carries no model or grammar authority.
    pub fn take_declarations(&mut self) -> crate::definitions::declarations::DeclarationInventory {
        self.structure.take_declarations()
    }
    pub fn schema(&self) -> &Schema {
        self.structure.schema()
    }
    pub fn sources(&self) -> &'inputs DefinitionBundle {
        self.structure.sources()
    }
    pub fn behavior_source(&self) -> &'inputs SourceId {
        self.behavior
    }
    /// Lower and validate declarations against this exact checked structure.
    /// This consumes the builder; no mutable schema or optional cache escapes.
    pub fn finish(
        self,
        limits: ParseLimits,
        imported_models: bool,
        mode: ModelPreparationMode,
        settings: &[ComputedSettingContract],
    ) -> Result<CompiledModelDefinition<'inputs>, Vec<Diagnostic>> {
        let selection = self.structure.checked.selection;
        let source = selection
            .bundle
            .source(self.behavior)
            .map_err(|message| vec![Diagnostic::new(&self.behavior.uri, 0, 0, message)])?;
        let (mut schema, physical_inputs) = self.structure.into_compiler_parts();
        let semantics = compiler::compile_semantics_source_for_finish(
            &mut schema,
            source,
            limits,
            imported_models,
            mode,
            settings,
        )?;
        Ok(CompiledModelDefinition {
            selection,
            behavior: self.behavior,
            data: ModelCompilationData {
                schema,
                semantics,
                physical_inputs,
            },
        })
    }
}

struct ModelCompilationData {
    schema: Schema,
    semantics: Semantics,
    physical_inputs: Option<compiler::CompletedLoweredInputs>,
}

/// Completed model-source compilation. Inspection is not model/domain validity
/// or language admission; source getters and providers still require activation.
///
/// ```compile_fail
/// use intersect_model::definitions::model_definition::CompiledModelDefinition;
/// fn load(input: &str) -> Result<CompiledModelDefinition<'_>, serde_json::Error> {
///     serde_json::from_str(input)
/// }
/// ```
pub struct CompiledModelDefinition<'inputs> {
    selection: StructureSelection<'inputs>,
    behavior: &'inputs SourceId,
    data: ModelCompilationData,
}
impl<'inputs> CompiledModelDefinition<'inputs> {
    pub fn schema(&self) -> &Schema {
        &self.data.schema
    }
    pub fn semantics(&self) -> &Semantics {
        &self.data.semantics
    }
    pub fn sources(&self) -> &DefinitionBundle {
        self.selection.bundle
    }
    pub fn behavior_source(&self) -> &SourceId {
        self.behavior
    }
    /// Consume fresh model output into inspection metadata. Optional captured
    /// inputs are discarded because this value has no stable owning allocation.
    pub fn into_prepared_definition(self) -> PreparedModelDefinition<'inputs> {
        PreparedModelDefinition::from_compiled_result(self)
    }
    pub(crate) fn preparation_selection(&self) -> StructureSelection<'inputs> {
        self.selection
    }
    pub(crate) fn preparation_behavior(&self) -> &'inputs SourceId {
        self.behavior
    }
    pub fn has_physical_inputs(&self) -> bool {
        self.data.physical_inputs.is_some()
    }
    /// Retain the actual source allocation, without copying the source graph.
    /// Equal cloned bundles cannot substitute for the checked live allocation.
    pub fn into_owned(
        self,
        sources: Arc<DefinitionBundle>,
    ) -> Result<OwnedModelDefinition, String> {
        if !std::ptr::eq(self.selection.bundle, sources.as_ref()) {
            return Err("Owned model requires its actual checked source allocation".into());
        }
        Ok(OwnedModelDefinition {
            sources,
            entry: self.selection.entry.clone(),
            behavior: self.behavior.clone(),
            namespace: self.selection.namespace.to_owned(),
            materialized_types: self.selection.materialized_types.to_vec(),
            data: Box::new(self.data),
        })
    }
    /// Discard checked source correspondence and optional optimization data.
    /// The resulting data cannot reconstruct fresh checked output. Only the
    /// explicit native-host cache exception can restore this host's saved data.
    pub fn into_metadata(self) -> ModelDefinitionMetadata {
        let (schema, semantics, _) = self.into_metadata_parts();
        ModelDefinitionMetadata { schema, semantics }
    }
    // Metadata export consumes correspondence and drops optional prepared inputs.
    fn into_metadata_parts(self) -> (Schema, Semantics, Option<compiler::CompletedLoweredInputs>) {
        (
            self.data.schema,
            self.data.semantics,
            self.data.physical_inputs,
        )
    }
}

/// Ordinary mutable model metadata, with no completion or execution authority.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct ModelDefinitionMetadata {
    pub schema: Schema,
    pub semantics: Semantics,
}

/// Immutable model-source owner from fresh compilation or explicit native-host
/// cache restoration. The schema allocation and the shared exact
/// source world stay stable through host moves and borrowed metadata preparation.
///
/// ```compile_fail
/// use intersect_model::definitions::model_definition::OwnedModelDefinition;
/// fn load(input: &str) -> Result<OwnedModelDefinition, serde_json::Error> {
///     serde_json::from_str(input)
/// }
/// ```
/// ```compile_fail
/// use intersect_model::definitions::{model_definition::OwnedModelDefinition, schema::Schema};
/// fn replace(owner: &mut OwnedModelDefinition, schema: Schema) {
///     owner.data.schema = schema;
/// }
/// ```
pub struct OwnedModelDefinition {
    sources: Arc<DefinitionBundle>,
    entry: SourceId,
    behavior: SourceId,
    namespace: String,
    materialized_types: Vec<RuntimeType>,
    data: Box<ModelCompilationData>,
}
impl OwnedModelDefinition {
    /// Snapshot this host's compiler output as ordinary cache data. Optional
    /// completed inputs are deliberately absent; restore takes the cold path.
    pub fn save_host_cache(&self) -> SavedModelDefinition {
        SavedModelDefinition {
            sources: self.sources.as_ref().clone(),
            entry: self.entry.clone(),
            behavior: self.behavior.clone(),
            namespace: self.namespace.clone(),
            materialized_types: self.materialized_types.clone(),
            metadata: ModelDefinitionMetadata {
                schema: self.schema().clone(),
                semantics: self.semantics().clone(),
            },
        }
    }
    pub fn schema(&self) -> &Schema {
        &self.data.schema
    }
    pub fn semantics(&self) -> &Semantics {
        &self.data.semantics
    }
    pub fn sources(&self) -> &DefinitionBundle {
        &self.sources
    }
    pub fn behavior_source(&self) -> &SourceId {
        &self.behavior
    }
    pub fn structure_selection(&self) -> StructureSelection<'_> {
        StructureSelection {
            bundle: &self.sources,
            entry: &self.entry,
            namespace: &self.namespace,
            materialized_types: &self.materialized_types,
        }
    }
    pub fn model(&self) -> ModelDefinitionView<'_> {
        ModelDefinitionView::new(self.schema(), self.semantics())
    }
    pub fn has_physical_inputs(&self) -> bool {
        self.data.physical_inputs.is_some()
    }
    pub fn prepared_definition(&self) -> PreparedModelDefinition<'_> {
        PreparedModelDefinition::from_owned_model(self)
    }
    pub(crate) fn completed_physical_inputs(&self) -> Option<&compiler::CompletedLoweredInputs> {
        self.data.physical_inputs.as_ref()
    }
}

/// Ordinary saved native-host model data. This is not a fresh compilation
/// receipt; neither construction nor serialization supplies source authority.
/// The explicit host-cache loader has the same trust exception as the text pack
/// loader: only this host's compiler output, matched by identity and digests.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SavedModelDefinition {
    sources: DefinitionBundle,
    entry: SourceId,
    behavior: SourceId,
    namespace: String,
    materialized_types: Vec<RuntimeType>,
    metadata: ModelDefinitionMetadata,
}
impl SavedModelDefinition {
    pub(in crate::definitions) fn from_prepared(
        selection: StructureSelection<'_>,
        behavior: &SourceId,
        schema: &Schema,
        semantics: &Semantics,
    ) -> Self {
        Self {
            sources: selection.bundle.clone(),
            entry: selection.entry.clone(),
            behavior: behavior.clone(),
            namespace: selection.namespace.to_owned(),
            materialized_types: selection.materialized_types.to_vec(),
            metadata: ModelDefinitionMetadata {
                schema: schema.clone(),
                semantics: semantics.clone(),
            },
        }
    }
    /// Borrow this host's saved model after checking its source and migration
    /// envelope. The caller must match compiler identity and section digests;
    /// schema, behavior and migration derivation remain trusted saved data.
    /// Never pass model, network or browser inputs. Fresh compilation/replay
    /// remains the independent oracle, and optional physical inputs stay absent.
    pub fn prepare_host_cache<'a>(
        &'a self,
        origin: &'a BundleMigrationReport,
    ) -> Result<PreparedModelDefinition<'a>, String> {
        self.sources.validate()?;
        self.sources.source(&self.entry)?;
        self.sources.source(&self.behavior)?;
        origin.original.validate()?;
        if origin.version != 1 || origin.converted.as_ref() != Some(&self.sources) {
            return Err("Preparation requires the exact converted migration bundle".into());
        }
        validate_saved_report_envelope(&self.sources, origin)?;
        Ok(PreparedModelDefinition::from_parts(
            StructureSelection {
                bundle: &self.sources,
                entry: &self.entry,
                namespace: &self.namespace,
                materialized_types: &self.materialized_types,
            },
            &self.behavior,
            Some(origin),
            std::borrow::Cow::Borrowed(&self.metadata.schema),
            std::borrow::Cow::Borrowed(&self.metadata.semantics),
            None,
        ))
    }
    /// Restore only native compiler output previously saved by this host and
    /// matched on compiler identity and section digests. Never use model, network
    /// or browser data. Source identities are checked; schema/behavior semantics
    /// stay trusted saved data. Fresh source recompilation is the oracle. Optional
    /// structural optimization data is absent, preserving cold accounting.
    pub fn load_host_cache(self) -> Result<OwnedModelDefinition, String> {
        self.sources.validate()?;
        self.sources.source(&self.entry)?;
        self.sources.source(&self.behavior)?;
        Ok(OwnedModelDefinition {
            sources: Arc::new(self.sources),
            entry: self.entry,
            behavior: self.behavior,
            namespace: self.namespace,
            materialized_types: self.materialized_types,
            data: Box::new(ModelCompilationData {
                schema: self.metadata.schema,
                semantics: self.metadata.semantics,
                physical_inputs: None,
            }),
        })
    }
}

/// Once-checked migration origin bound to the actual retained model source world.
/// This conveys source correspondence only, never grammar or domain validity.
///
/// ```compile_fail
/// use intersect_model::definitions::model_definition::BoundModelOrigin;
/// fn load(input: &str) -> Result<BoundModelOrigin, serde_json::Error> {
///     serde_json::from_str(input)
/// }
/// ```
pub struct BoundModelOrigin {
    sources: Arc<DefinitionBundle>,
    origin: crate::definitions::ecore_migration::BundleMigrationReport,
}
impl BoundModelOrigin {
    pub fn replay(
        owner: &OwnedModelDefinition,
        origin: crate::definitions::ecore_migration::BundleMigrationReport,
        limits: ParseLimits,
    ) -> Result<Self, String> {
        crate::definitions::preparation::CheckedModelOrigin::check(
            owner.sources(),
            &origin,
            limits,
        )?;
        Ok(Self {
            sources: Arc::clone(&owner.sources),
            origin,
        })
    }
    pub fn from_completed(
        owner: &OwnedModelDefinition,
        completed: crate::definitions::ecore_migration::CompletedBundleMigration,
    ) -> Result<Self, String> {
        let origin = completed.into_report();
        if origin.version != 1 || origin.converted.as_ref() != Some(owner.sources()) {
            return Err("Preparation requires the exact converted migration bundle".into());
        }
        Ok(Self {
            sources: Arc::clone(&owner.sources),
            origin,
        })
    }
    pub fn save_host_cache(&self) -> SavedModelOrigin {
        SavedModelOrigin {
            origin: self.origin.clone(),
        }
    }
    pub fn report(&self) -> &crate::definitions::ecore_migration::BundleMigrationReport {
        &self.origin
    }
    pub fn prepare<'owner>(
        &'owner self,
        owner: &'owner OwnedModelDefinition,
    ) -> Result<PreparedModelDefinition<'owner>, String> {
        PreparedModelDefinition::from_bound_origin(owner, self)
    }
    pub(crate) fn belongs_to(&self, owner: &OwnedModelDefinition) -> bool {
        Arc::ptr_eq(&self.sources, &owner.sources)
    }
}

/// Saved origin data under the existing native-host cache trust exception.
/// It is not a native migration token and ordinary metadata never becomes one.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
#[serde(transparent)]
pub struct SavedModelOrigin {
    origin: crate::definitions::ecore_migration::BundleMigrationReport,
}
impl SavedModelOrigin {
    pub fn report(&self) -> &crate::definitions::ecore_migration::BundleMigrationReport {
        &self.origin
    }
    /// Discard the saved-data wrapper into ordinary mutable report data. This
    /// does not create native migration or bound-source authority.
    pub fn into_metadata(self) -> crate::definitions::ecore_migration::BundleMigrationReport {
        self.origin
    }
    /// Restore this host's own bound origin without replay, after matching its
    /// compiler identity and section digests. Exact envelope checks retain the
    /// original/converted mapping; spans, paths and draft derivation stay trusted
    /// saved data. Never pass model, network or browser inputs.
    pub fn load_host_cache(self, owner: &OwnedModelDefinition) -> Result<BoundModelOrigin, String> {
        self.origin.original.validate()?;
        if self.origin.version != 1 || self.origin.converted.as_ref() != Some(owner.sources()) {
            return Err("Preparation requires the exact converted migration bundle".into());
        }
        validate_saved_report_envelope(owner.sources(), &self.origin)?;
        Ok(BoundModelOrigin {
            sources: Arc::clone(&owner.sources),
            origin: self.origin,
        })
    }
}

fn validate_saved_report_envelope(
    converted: &DefinitionBundle,
    origin: &BundleMigrationReport,
) -> Result<(), String> {
    use intersect_model::definitions::bundle::SourceKind;
    use std::collections::{BTreeMap, BTreeSet};
    let original = &origin.original;
    if converted.version != 1 || original.sources.len() != converted.sources.len() {
        return Err("Saved converted bundle does not follow its original bundle".into());
    }
    let ids = original
        .sources
        .iter()
        .zip(&converted.sources)
        .map(|(a, b)| (&a.id, &b.id))
        .collect::<BTreeMap<_, _>>();
    let original_uris = original
        .sources
        .iter()
        .map(|source| source.id.uri.as_str())
        .collect::<BTreeSet<_>>();
    let mut reports = origin.reports.iter();
    let mut used = BTreeSet::new();
    for (source, target) in original.sources.iter().zip(&converted.sources) {
        let dependencies = source
            .dependencies
            .iter()
            .map(|id| ids.get(id).copied())
            .collect::<Option<Vec<_>>>()
            .ok_or("Saved original source has an unknown dependency")?;
        if target.dependencies.iter().collect::<Vec<_>>() != dependencies {
            return Err("Saved converted source dependencies differ from its original".into());
        }
        if source.kind != SourceKind::Ecore {
            if target.id != source.id || target.kind != source.kind || target.text != source.text {
                return Err("Saved converted bundle changes a pass-through source".into());
            }
            continue;
        }
        let report = reports
            .next()
            .ok_or("Saved migration is missing a resource report")?;
        if report.version != 1 {
            return Err("Saved migration resource report has an unsupported version".into());
        }
        if report.original != *source {
            return Err(
                "Saved migration report original differs from its bundle source at its position"
                    .into(),
            );
        }
        let reported = report
            .converted
            .as_ref()
            .ok_or("Saved migration has blocking or missing converted sources")?;
        if reported != target {
            return Err("Saved migration report converted source differs from the compiled bundle source at its position".into());
        }
        if target.kind != SourceKind::Rcore || target.text != report.draft {
            return Err("Saved migration converted source is not its Rcore draft".into());
        }
        if original_uris.contains(target.id.uri.as_str()) || !used.insert(&target.id.uri) {
            return Err("Saved migration requires distinct new output URIs".into());
        }
        if report.issues.iter().any(|issue| issue.blocks_conversion) {
            return Err("Saved completed migration carries a blocking issue".into());
        }
        if report.correspondence.iter().any(|row| {
            row.original.source != source.id
                || row.original_span.source != source.id
                || row.converted.source != target.id
                || row.converted_span.source != target.id
        }) {
            return Err(
                "Saved migration correspondence names another resource than its report".into(),
            );
        }
    }
    if reports.next().is_some() {
        return Err("Saved migration has a report for no Ecore original resource".into());
    }
    let namespaces_match = original.namespaces.len() == converted.namespaces.len()
        && original
            .namespaces
            .iter()
            .zip(&converted.namespaces)
            .all(|(a, b)| a.namespace == b.namespace && ids.get(&a.source) == Some(&&b.source));
    if !namespaces_match {
        return Err("Saved converted namespaces differ from their originals".into());
    }
    Ok(())
}
