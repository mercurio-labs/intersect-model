//! Verification-only native/WASM consumer of the actual retained compiler.
use intersect_model::definitions::{
    bundle::{DefinitionBundle, DefinitionSource, SourceId, SourceKind},
    compiler::{self, OwnedRuntimeSourceArrayPreparation},
    conversion::{InvocationContext, InvocationObserver, ProviderId},
    ecore::{ErrorKind, ObservedImportError},
    generic_types::{RuntimeType, Type},
    resources::DeclarationId,
};
use std::rc::Rc;
fn fixture() -> Result<DefinitionBundle, String> {
    let mut original: DefinitionBundle = serde_json::from_str(include_str!(
        "../../../fixtures/ecore-generics/materialization-source-preparation/bundle.json"
    ))
    .map_err(|error| error.to_string())?;
    let entry = &original.sources.first().ok_or("Missing original entry")?.id;
    let requested = RuntimeType::new(Type::Classifier {
        declaration: DeclarationId {
            source: entry.clone(),
            path: vec![0],
        },
        arguments: vec![],
        raw: false,
    })?;
    let rejected = compiler::materialize_runtime_bundle(
        &original,
        entry,
        "urn:loading:a",
        std::slice::from_ref(&requested),
        None,
    )
    .err()
    .ok_or("Raw executable Ecore was accepted")?;
    if rejected.kind != ErrorKind::Unsupported || rejected.span.source != *entry {
        return Err("Raw Ecore rejection lost its original source identity".into());
    }
    if compiler::OwnedRuntimeSourceArrayPreparation::begin_with_compiled_signature_inputs(
        original.clone(),
        entry.clone(),
        "urn:loading:a".into(),
        &[requested],
        None,
        &InvocationContext::default(),
    )
    .is_ok()
    {
        return Err("Retained compiler accepted raw executable Ecore".into());
    }
    // Retain the historical fixture unchanged. Its unselected Unknown node
    // cannot migrate; verify that explicit boundary instead of allowing raw
    // executable Ecore or silently ignoring the migration issue.
    let unsupported = original
        .sources
        .pop()
        .ok_or("Missing unselected oracle source")?;
    let report = intersect_model::migration::migrate_ecore(
        unsupported.clone(),
        "file:///loading/unsupported.rcore",
    )
    .map_err(|error| error.to_string())?;
    if report.original != unsupported || report.converted.is_some() {
        return Err("Unsupported original Ecore was silently migrated".into());
    }
    let targets = original
        .sources
        .iter()
        .map(|source| (source.id.clone(), format!("{}.rcore", source.id.uri)))
        .collect();
    let report = intersect_model::migration::migrate_ecore_bundle(original.clone(), &targets)
        .map_err(|error| error.to_string())?;
    if report.original != original || report.reports.len() != 2 {
        return Err("Original source provenance changed during explicit migration".into());
    }
    let mut bundle = report
        .converted
        .ok_or("Selected Ecore fixture did not migrate")?;
    bundle.sources.push(DefinitionSource::new(
        "file:///loading/unselected.rcore", SourceKind::Rcore,
        "rcore 1; package unused version \"1\"; metamodel \"urn:loading:a\" prefix unused { class Unconsumed {} }",
    ));
    Ok(bundle)
}
struct Cancel;
impl InvocationObserver for Cancel {
    fn before_invoke(&self, _: &ProviderId) -> Result<(), String> {
        Err("Unexpected provider".into())
    }
    fn checkpoint(&self) -> Result<(), String> {
        Err("materialization source probe cancelled".into())
    }
}
fn begin(
    bundle: DefinitionBundle,
    entry: SourceId,
    types: &[RuntimeType],
) -> Result<OwnedRuntimeSourceArrayPreparation, String> {
    compiler::OwnedRuntimeSourceArrayPreparation::begin_with_compiled_signature_inputs(
        bundle,
        entry,
        "urn:loading:a".into(),
        types,
        None,
        &InvocationContext::default(),
    )
    .map_err(|failure| match failure.materialization.result {
        Err(error) => error.to_string(),
        Ok(_) => "Unexpected failed source result".into(),
    })
}
fn verify() -> Result<u32, String> {
    let bundle = fixture()?;
    let entry = bundle.sources[0].id.clone();
    let types = bundle.sources[..2]
        .iter()
        .map(|source| {
            RuntimeType::new(Type::Classifier {
                declaration: DeclarationId {
                    source: source.id.clone(),
                    path: vec![0],
                },
                arguments: vec![],
                raw: false,
            })
        })
        .collect::<Result<Vec<_>, _>>()?;
    let context = InvocationContext::default();
    let legacy =
        compiler::materialize_runtime_bundle(&bundle, &entry, "urn:loading:a", &types, None)
            .map_err(|e| e.to_string())?;
    let mut preparation = begin(bundle.clone(), entry.clone(), &types)?;
    let mut advances = 0;
    loop {
        advances += 1;
        if preparation
            .advance(None, &context)
            .map_err(|e| e.to_string())?
        {
            break;
        }
    }
    let completed = preparation.finish();
    let schema = completed
        .materialization
        .result
        .map_err(|e| e.to_string())?;
    if advances <= 30
        || completed.materialization.bundle != bundle
        || completed.entry_source != entry
        || schema.classes.len() != 2
        || serde_json::to_value(schema).map_err(|e| e.to_string())?
            != serde_json::to_value(legacy).map_err(|e| e.to_string())?
    {
        return Err("Retained source schema/ownership changed".into());
    }
    // Every early XML/loading boundary; native controls exhaust all loading and
    // observer boundaries, while the whole-host proof covers the large fixture.
    for prefix in 0..=30 {
        let moved = bundle.clone();
        let original = moved.sources[0].text.as_ptr();
        let mut pending = begin(moved, entry.clone(), &types)?;
        for _ in 0..prefix {
            if pending.advance(None, &context).map_err(|e| e.to_string())? {
                return Err("Unexpected early publication".into());
            }
        }
        let expected =
            ObservedImportError::Observation("materialization source probe cancelled".into());
        if pending.advance(None, &InvocationContext::new(Rc::new(Cancel))) != Err(expected.clone())
            || pending.advance(None, &context) != Err(expected.clone())
        {
            return Err("Terminal loading cancellation changed".into());
        }
        let failed = pending.finish();
        if failed.materialization.bundle != bundle
            || failed.materialization.bundle.sources[0].text.as_ptr() != original
            || !matches!(failed.materialization.result,Err(actual) if actual==expected)
        {
            return Err("Exact cancellation recovery changed".into());
        }
    }
    let mut bad = bundle.clone();
    bad.sources[2] = DefinitionSource::new(
        "file:///loading/unselected.rcore",
        SourceKind::Rcore,
        "rcore 1; package broken version \"1\"; metamodel {",
    );
    let expected =
        compiler::materialize_runtime_bundle(&bad, &entry, "urn:loading:a", &types, None)
            .err()
            .ok_or("Malformed Rcore accepted")?;
    let mut pending = begin(bad.clone(), entry.clone(), &types)?;
    loop {
        match pending.advance(None, &context) {
            Ok(false) => {}
            Ok(true) => return Err("Malformed Rcore published".into()),
            Err(ObservedImportError::Import(actual)) if actual == expected => break,
            Err(error) => return Err(error.to_string()),
        }
    }
    if pending.finish().materialization.bundle != bad {
        return Err("Malformed source recovery changed".into());
    }
    let original = bundle.sources[0].text.as_ptr();
    let incomplete = begin(bundle.clone(), entry.clone(), &types)?.finish();
    if incomplete.materialization.result.is_ok() {
        return Err("Incomplete schema published".into());
    }
    let mut fresh = begin(bundle, entry, &types)?;
    while !fresh.advance(None, &context).map_err(|e| e.to_string())? {}
    let checked = fresh.finish_checked(&context);
    if checked.result.is_err() || checked.bundle.sources[0].text.as_ptr() != original {
        return Err("Fresh checked compiler recovery changed".into());
    }
    Ok(4)
}
#[unsafe(no_mangle)]
pub extern "C" fn verify_materialization_source_preparation() -> u32 {
    verify().unwrap_or(0)
}
fn main() -> Result<(), String> {
    println!("{} materialization source groups passed", verify()?);
    Ok(())
}
