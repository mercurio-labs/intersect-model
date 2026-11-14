//! Verification-only native/WASM consumer of the shared retained XML parser.
use intersect_model::definitions::{
    bundle::{DefinitionSource, SourceKind},
    conversion::{InvocationContext, InvocationObserver, ProviderId},
    ecore::{Document, ObservedImportError, OwnedDocumentPreparation, ParseLimits, PayloadKind},
};
use std::rc::Rc;
struct Cancel;
impl InvocationObserver for Cancel {
    fn before_invoke(&self, _: &ProviderId) -> Result<(), String> {
        Err("Unexpected provider".into())
    }
    fn checkpoint(&self) -> Result<(), String> {
        Err("probe cancelled".into())
    }
}
fn verify() -> Result<u32, String> {
    let source = DefinitionSource::new(
        "memory:events.ecore",
        SourceKind::Ecore,
        include_str!("../../../fixtures/ecore-generics/document-events.ecore"),
    );
    let context = InvocationContext::default();
    let mut preparation =
        OwnedDocumentPreparation::begin(&source, ParseLimits::default(), &context)
            .map_err(|e| e.to_string())?;
    let mut events = 0;
    loop {
        events += 1;
        if preparation.advance(&context).map_err(|e| e.to_string())? {
            break;
        }
    }
    let document = preparation.finish().map_err(|e| e.to_string())?;
    let expected = Document::parse(source.clone()).map_err(|e| e.to_string())?;
    if events != 16
        || format!("{document:?}") != format!("{expected:?}")
        || document.source != source
    {
        return Err("Complete document or event order changed".into());
    }
    let child = document.root.children.first().ok_or("Missing classifier")?;
    let name = child
        .attributes
        .iter()
        .find(|a| a.lexical_name == "name")
        .ok_or("Missing name")?;
    let annotations = child.children.first().ok_or("Missing annotation")?;
    if name.value != "Nα"
        || &source.text[name.span.start..name.span.end] != "name=\"Nα\""
        || annotations
            .payloads
            .iter()
            .map(|p| p.kind)
            .collect::<Vec<_>>()
            != [
                PayloadKind::Text,
                PayloadKind::EntityReference,
                PayloadKind::CData,
                PayloadKind::ProcessingInstruction,
                PayloadKind::Comment,
            ]
    {
        return Err("Literal Unicode or opaque source payload changed".into());
    }
    for boundary in 0..=events {
        let mut preparation =
            OwnedDocumentPreparation::begin(&source, ParseLimits::default(), &context)
                .map_err(|e| e.to_string())?;
        for _ in 0..boundary {
            preparation.advance(&context).map_err(|e| e.to_string())?;
        }
        let expected = ObservedImportError::Observation("probe cancelled".into());
        if preparation.advance(&InvocationContext::new(Rc::new(Cancel))) != Err(expected.clone())
            || preparation.advance(&context) != Err(expected.clone())
            || !matches!(preparation.finish(),Err(actual) if actual==expected)
        {
            return Err("Cancellation or terminal recovery changed".into());
        }
    }
    let invalid = DefinitionSource::new(
        "memory:invalid.ecore",
        SourceKind::Ecore,
        "<!DOCTYPE a><a/>",
    );
    let expected = Document::parse(invalid.clone())
        .err()
        .ok_or("DTD unexpectedly accepted")?;
    let mut preparation =
        OwnedDocumentPreparation::begin(&invalid, ParseLimits::default(), &context)
            .map_err(|e| e.to_string())?;
    if preparation.advance(&context) != Err(ObservedImportError::Import(expected)) {
        return Err("DTD diagnostics changed".into());
    }
    verify_input()?;
    Ok(8)
}
fn verify_input() -> Result<(), String> {
    // Fixed, independently generated digest rather than agreement between the
    // constructor and validator alone. This runs on native and raw WASM.
    let mut vector = DefinitionSource::new("memory:vector", SourceKind::Ecore, "é𝄞");
    vector.id.sha256 = "35ff9281a42a4aa3ec7d0f1e84e40bfaba55a2e98101ac9d4b5fdeb3efaa4932".into();
    vector.validate()?;
    let source = DefinitionSource::new(
        format!("memory:{}", "α".repeat(150)),
        SourceKind::Ecore,
        format!("<a>{}</a>", "α".repeat(300)),
    );
    let context = InvocationContext::default();
    let mut preparation = OwnedDocumentPreparation::begin_with_retained_input(
        &source,
        ParseLimits::default(),
        &context,
    )
    .map_err(|e| e.to_string())?;
    let mut advances = 0;
    loop {
        advances += 1;
        if preparation.advance(&context).map_err(|e| e.to_string())? {
            break;
        }
    }
    let document = preparation.finish().map_err(|e| e.to_string())?;
    let expected = Document::parse(source.clone()).map_err(|e| e.to_string())?;
    if advances != 12
        || document.source != source
        || format!("{document:?}") != format!("{expected:?}")
    {
        return Err("Retained input chunks changed source/tree output".into());
    }
    for boundary in 0..=advances {
        let mut preparation = OwnedDocumentPreparation::begin_with_retained_input(
            &source,
            ParseLimits::default(),
            &context,
        )
        .map_err(|e| e.to_string())?;
        for _ in 0..boundary {
            preparation.advance(&context).map_err(|e| e.to_string())?;
        }
        let error = ObservedImportError::Observation("probe cancelled".into());
        if preparation.advance(&InvocationContext::new(Rc::new(Cancel))) != Err(error.clone())
            || preparation.advance(&context) != Err(error.clone())
            || !matches!(preparation.finish(), Err(actual) if actual == error)
        {
            return Err("Retained input cancellation was not terminal".into());
        }
    }
    let invalid = DefinitionSource::new(
        "memory:invalid-input.ecore",
        SourceKind::Ecore,
        format!("<a>{}\0</a>", "α".repeat(257)),
    );
    let expected = Document::parse(invalid.clone())
        .err()
        .ok_or("Invalid XML character accepted")?;
    if expected.span.start != 517
        || expected.span.end != 518
        || expected.message != "Invalid XML character"
    {
        return Err("Independent XML character span changed".into());
    }
    let mut preparation = OwnedDocumentPreparation::begin_with_retained_input(
        &invalid,
        ParseLimits::default(),
        &context,
    )
    .map_err(|e| e.to_string())?;
    let error = loop {
        match preparation.advance(&context) {
            Ok(false) => {}
            Ok(true) => return Err("Invalid input reached completion".into()),
            Err(error) => break error,
        }
    };
    if error != ObservedImportError::Import(expected)
        || !matches!(preparation.finish(), Err(actual) if actual == error)
    {
        return Err("Retained input diagnostics changed".into());
    }
    // Identity failure still precedes kind, size and malformed XML failures.
    let mut invalid = invalid;
    invalid.id.sha256 = "0".repeat(64);
    invalid.kind = SourceKind::RcoreBehavior;
    let mut preparation = OwnedDocumentPreparation::begin_with_retained_input(
        &invalid,
        ParseLimits { source_bytes: 1 },
        &context,
    )
    .map_err(|e| e.to_string())?;
    loop {
        match preparation.advance(&context) {
            Ok(false) => {}
            Ok(true) => return Err("Forged identity reached completion".into()),
            Err(ObservedImportError::Import(error))
                if error.message
                    == "Source content digest mismatch: memory:invalid-input.ecore"
                    && error.span.start == 0
                    && error.span.end == 0 =>
            {
                break;
            }
            Err(error) => return Err(format!("Input error priority changed: {error}")),
        }
    }
    Ok(())
}
#[unsafe(no_mangle)]
pub extern "C" fn verify_document_preparation() -> u32 {
    verify().unwrap_or(0)
}
fn main() -> Result<(), String> {
    let groups = verify()?;
    println!("{groups} retained XML verification groups passed");
    Ok(())
}
