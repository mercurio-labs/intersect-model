//! Explicit conversion for original Ecore fixtures used by canonical consumers.
//! Compiled only in unit tests, never exposed as a production fallback.
use super::bundle::{DefinitionBundle, DefinitionSource};

pub(super) fn source(original: DefinitionSource) -> DefinitionSource {
    let uri = format!("{}.rcore", original.id.uri);
    let report = crate::migration::migrate_ecore(original, &uri).unwrap();
    assert!(
        report.issues.iter().all(|issue| !issue.blocks_conversion),
        "{:?}",
        report.issues
    );
    report.converted.unwrap()
}

pub(super) fn bundle(original: DefinitionBundle) -> DefinitionBundle {
    let targets = original
        .sources
        .iter()
        .map(|source| (source.id.clone(), format!("{}.rcore", source.id.uri)))
        .collect();
    let report = crate::migration::migrate_ecore_bundle(original, &targets).unwrap();
    assert!(
        report
            .reports
            .iter()
            .all(|report| report.issues.iter().all(|issue| !issue.blocks_conversion)),
        "{:?}",
        report.reports
    );
    report.converted.unwrap()
}

/// Preserve the original fixture's declaration addresses through migration.
pub(super) fn source_with_ids(
    original: DefinitionSource,
) -> (
    DefinitionSource,
    std::collections::BTreeMap<Vec<usize>, super::resources::DeclarationId>,
) {
    let uri = format!("{}.rcore", original.id.uri);
    let report = crate::migration::migrate_ecore(original, &uri).unwrap();
    assert!(
        report.issues.iter().all(|issue| !issue.blocks_conversion),
        "{:?}",
        report.issues
    );
    let ids = report
        .correspondence
        .into_iter()
        .map(|entry| (entry.original.path, entry.converted))
        .collect();
    (report.converted.unwrap(), ids)
}
