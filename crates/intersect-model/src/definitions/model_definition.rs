//! Handwritten model definition views and checked structural source preparation.
//! Definition views are data; checked structures prove structural source equality,
//! not grammar admission, installed providers, or domain validity.
#[path = "model_compilation.rs"]
mod model_compilation;
pub use model_compilation::{
    BoundModelOrigin, CheckedModelCompilation, CompiledModelDefinition, ModelDefinitionMetadata,
    ModelPreparationMode, OwnedModelDefinition, SavedModelDefinition, SavedModelOrigin,
};
#[path = "reference_context.rs"]
mod reference_context;
use crate::definitions::{
    bundle::{DefinitionBundle, SourceId},
    compiler,
    conversion::{ConversionRegistry, InvocationContext},
    declarations::{DeclarationCollection, DeclarationInventory, DeclarationSelection},
    ecore::{ImportError, ObservedImportError, ParseLimits, ParserObservation},
    generic_types::RuntimeType,
    schema::{Schema, Semantics},
};
pub use reference_context::{ReferenceContextTransaction, StructureContext};

#[derive(Clone, Copy)]
pub struct ModelDefinitionView<'a> {
    pub(crate) schema: &'a Schema,
    pub(crate) semantics: &'a Semantics,
}
impl<'a> ModelDefinitionView<'a> {
    /// Borrow metadata without installing providers or granting model execution.
    pub fn new(schema: &'a Schema, semantics: &'a Semantics) -> Self {
        Self { schema, semantics }
    }
    pub fn schema(self) -> &'a Schema {
        self.schema
    }
    pub fn semantics(self) -> &'a Semantics {
        self.semantics
    }
}

/// Borrowed retained model inputs for metadata/provider adapters.
/// Source selectors are data; this view grants no checked origin or execution authority.
#[derive(Clone, Copy)]
pub struct ModelPackageView<'a> {
    pub definition: ModelDefinitionView<'a>,
    pub version: u32,
    pub structure_text: Option<&'a str>,
    pub behavior_text: Option<&'a str>,
    pub structure: Option<&'a SourceId>,
    pub behavior: Option<&'a SourceId>,
    pub bundle: Option<&'a DefinitionBundle>,
}
impl<'a> From<&'a crate::definitions::schema::Package> for ModelDefinitionView<'a> {
    fn from(package: &'a crate::definitions::schema::Package) -> Self {
        package.model_definition()
    }
}
impl<'a> From<&'a crate::definitions::schema::Package> for ModelPackageView<'a> {
    fn from(package: &'a crate::definitions::schema::Package) -> Self {
        package.model_package()
    }
}

/// Reproducible modeling inputs, independent of any text syntax or inference recipe.
/// Deserialization retains selections only; checking and source-origin replay remain separate.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModelCompilation {
    pub bundle: DefinitionBundle,
    pub structure: SourceId,
    pub behavior: SourceId,
    pub namespace: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub materialized_types: Vec<RuntimeType>,
}
impl ModelCompilation {
    pub fn structure_selection(&self) -> StructureSelection<'_> {
        StructureSelection {
            bundle: &self.bundle,
            entry: &self.structure,
            namespace: &self.namespace,
            materialized_types: &self.materialized_types,
        }
    }
}

/// Exact source selection for structural compilation, independent of text syntax.
#[derive(Clone, Copy)]
pub struct StructureSelection<'a> {
    pub bundle: &'a DefinitionBundle,
    pub entry: &'a SourceId,
    pub namespace: &'a str,
    pub materialized_types: &'a [RuntimeType],
}

/// Fresh canonical structure with private ownership. No Deserialize or constructor
/// from a cached Schema can establish this source correspondence.
///
/// ```compile_fail
/// use intersect_model::definitions::model_definition::CheckedStructure;
/// fn restore(input: &str) -> Result<CheckedStructure<'_>, serde_json::Error> {
///     serde_json::from_str(input)
/// }
/// ```
/// ```compile_fail
/// use intersect_model::definitions::{model_definition::CheckedStructure, schema::Schema};
/// fn replace(checked: &mut CheckedStructure<'_>, schema: Schema) {
///     checked.schema = schema;
/// }
/// ```
pub struct CheckedStructure<'a> {
    selection: StructureSelection<'a>,
    schema: Schema,
}
impl<'a> CheckedStructure<'a> {
    pub fn prepare(selection: StructureSelection<'a>, limits: ParseLimits) -> Result<Self, String> {
        selection.bundle.validate()?;
        let schema = if selection.materialized_types.is_empty() {
            compiler::compile_structure_bundle_with_limits(
                selection.bundle,
                selection.entry,
                selection.namespace,
                limits,
            )
        } else {
            compiler::materialize_runtime_bundle_with_limits(
                selection.bundle,
                selection.entry,
                selection.namespace,
                selection.materialized_types,
                None,
                limits,
            )
        }
        .map_err(|error| error.to_string())?;
        Ok(Self { selection, schema })
    }
    pub fn schema(&self) -> &Schema {
        &self.schema
    }
    pub fn sources(&self) -> &'a DefinitionBundle {
        self.selection.bundle
    }
    /// Select the canonical EObject reference context. This is a checked modeling
    /// operation; selecting which syntax needs it belongs to the text frontend.
    pub fn enable_eobject_references(&mut self) -> Result<(), String> {
        self.schema.prepare_eobject_reference_context()
    }
    pub fn require_schema(&self, expected: &Schema) -> Result<(), String> {
        if self.schema != *expected {
            return Err("Cached getter schema differs from the retained source structure".into());
        }
        Ok(())
    }
}

/// Optional completed structural field data. Retaining this data installs no
/// providers and does not validate a model or admit any text syntax.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum PhysicalInputCapture {
    #[default]
    None,
    Retain,
}

/// Native structural compilation with private source correspondence and optional
/// inspection data. A caller can request data, but cannot assert completion or
/// replace the canonical schema. No grammar or behavior execution is admitted.
///
/// ```compile_fail
/// use intersect_model::definitions::model_definition::CompiledStructure;
/// fn load(input: &str) -> Result<CompiledStructure<'_>, serde_json::Error> {
///     serde_json::from_str(input)
/// }
/// ```
/// ```compile_fail
/// use intersect_model::definitions::{model_definition::CompiledStructure, schema::Schema};
/// fn replace(compiled: &mut CompiledStructure<'_>, schema: Schema) {
///     compiled.checked.schema = schema;
/// }
/// ```
/// ```compile_fail
/// use intersect_model::definitions::model_definition::CompiledStructure;
/// fn keep_receipt(compiled: CompiledStructure<'_>) {
///     let _schema = compiled.into_schema();
///     let _checked = compiled.into_checked();
/// }
/// ```
pub struct CompiledStructure<'a> {
    checked: CheckedStructure<'a>,
    declarations: DeclarationInventory,
    physical_inputs: Option<compiler::CompletedLoweredInputs>,
}
impl<'a> CompiledStructure<'a> {
    pub fn prepare(
        selection: StructureSelection<'a>,
        declarations: DeclarationCollection,
        capture: PhysicalInputCapture,
        conversions: Option<&ConversionRegistry<'_>>,
        limits: ParseLimits,
    ) -> Result<Self, ImportError> {
        compiler::require_canonical_sources(selection.bundle)?;
        let context = InvocationContext::default();
        Self::compile(
            selection,
            declarations,
            capture,
            conversions,
            &ParserObservation::new(&context),
            limits,
        )
    }

    /// Forward the supplied invocation through the shared structural pipeline.
    /// Existing unobserved allocation boundaries remain unqualified. Exhausted
    /// or cancelled observations are never replaced with a fresh grant here.
    pub fn prepare_observed(
        selection: StructureSelection<'a>,
        declarations: DeclarationCollection,
        capture: PhysicalInputCapture,
        conversions: Option<&ConversionRegistry<'_>>,
        context: &InvocationContext<'_>,
        limits: ParseLimits,
    ) -> Result<Self, ObservedImportError> {
        compiler::require_canonical_sources(selection.bundle)
            .map_err(ObservedImportError::Import)?;
        context
            .checkpoint()
            .map_err(ObservedImportError::Observation)?;
        let observation = ParserObservation::new(context);
        observation.finish(Self::compile(
            selection,
            declarations,
            capture,
            conversions,
            &observation,
            limits,
        ))
    }

    fn compile(
        selection: StructureSelection<'a>,
        collection: DeclarationCollection,
        capture: PhysicalInputCapture,
        conversions: Option<&ConversionRegistry<'_>>,
        observation: &ParserObservation<'_, '_>,
        limits: ParseLimits,
    ) -> Result<Self, ImportError> {
        let mut declarations = DeclarationInventory::default();
        let selection_data =
            (collection != DeclarationCollection::None).then_some(DeclarationSelection {
                inventory: &mut declarations,
                classifiers: collection == DeclarationCollection::Classifiers,
            });
        let (schema, physical_inputs) = compiler::compile_structure_selection(
            selection,
            conversions,
            selection_data,
            capture == PhysicalInputCapture::Retain,
            observation,
            limits,
        )?;
        Ok(Self {
            checked: CheckedStructure { selection, schema },
            declarations,
            physical_inputs,
        })
    }

    pub fn schema(&self) -> &Schema {
        self.checked.schema()
    }
    pub fn sources(&self) -> &'a DefinitionBundle {
        self.checked.sources()
    }
    pub fn declarations(&self) -> &DeclarationInventory {
        &self.declarations
    }
    /// Move inspection data into a consumer. Further calls return empty data;
    /// moving it does not change the checked schema or its source owner.
    pub fn take_declarations(&mut self) -> DeclarationInventory {
        std::mem::take(&mut self.declarations)
    }
    pub fn has_physical_inputs(&self) -> bool {
        self.physical_inputs.is_some()
    }
    pub fn enable_eobject_references(&mut self) -> Result<(), String> {
        self.checked.enable_eobject_references()
    }
    /// Keep source correspondence while discarding optional inspection caches.
    pub fn into_checked(self) -> CheckedStructure<'a> {
        self.checked
    }

    /// Consume source correspondence and caches, returning ordinary metadata.
    /// This avoids cloning for model construction; the returned mutable data
    /// cannot restore a checked source owner or any prepared execution grant.
    pub fn into_schema(self) -> Schema {
        self.into_compiler_parts().0
    }

    // Transitional private bridge for the remaining model-hosted text worker.
    // External callers cannot take arbitrary mutable schema/cache tuples and
    // use them to construct a completed model/prepared owner.
    pub(crate) fn into_compiler_parts(self) -> (Schema, Option<compiler::CompletedLoweredInputs>) {
        (self.checked.schema, self.physical_inputs)
    }
}
