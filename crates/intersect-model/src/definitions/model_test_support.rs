//! Test fixtures assembled through the public checked model services.
//! Retained text sources remain ordinary bundle data and are never interpreted.
use super::{
    bundle::SourceId,
    declarations::DeclarationCollection,
    ecore::ParseLimits,
    ecore_migration::BundleMigrationReport,
    model_definition::{
        CompiledModelDefinition, CompiledStructure, ModelPreparationMode, OwnedModelDefinition,
        PhysicalInputCapture, StructureContext, StructureSelection,
    },
    preparation::{CheckedModelOrigin, PreparedModelDefinition},
};

pub(crate) fn compile<'a>(
    selection: StructureSelection<'a>,
    behavior: &SourceId,
    capture: PhysicalInputCapture,
    limits: ParseLimits,
    eobject_context: bool,
) -> Result<CompiledModelDefinition<'a>, String> {
    let mut structure = CompiledStructure::prepare(
        selection,
        DeclarationCollection::Classifiers,
        capture,
        None,
        limits,
    )
    .map_err(|error| error.to_string())?;
    if eobject_context {
        let mut context = structure.reference_context();
        context.enable_eobject_references()?;
        context.commit();
    }
    structure
        .check_behavior(behavior, None, limits)
        .and_then(|checked| checked.finish(limits, false, ModelPreparationMode::Inspection, &[]))
        .map_err(|errors| format!("{errors:?}"))
}

pub(crate) fn prepare<'a>(
    selection: StructureSelection<'a>,
    behavior: &SourceId,
    origin: &'a BundleMigrationReport,
    limits: ParseLimits,
) -> Result<PreparedModelDefinition<'a>, String> {
    let checked_origin = CheckedModelOrigin::check(selection.bundle, origin, limits)?;
    compile(
        selection,
        behavior,
        PhysicalInputCapture::None,
        limits,
        false,
    )?
    .into_prepared_definition()
    .with_origin(checked_origin)
}

pub(crate) fn prepare_owned<'a>(
    owner: &'a OwnedModelDefinition,
    origin: &'a BundleMigrationReport,
    limits: ParseLimits,
) -> Result<PreparedModelDefinition<'a>, String> {
    owner
        .prepared_definition()
        .with_origin(CheckedModelOrigin::check(owner.sources(), origin, limits)?)
}
