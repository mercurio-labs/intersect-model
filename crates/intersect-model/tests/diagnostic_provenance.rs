use intersect_model::definitions::{
    bundle::{DefinitionSource, SourceKind, SourceSpan},
    schema::{Diagnostic, DiagnosticOrigin},
};
use serde_json::json;

#[test]
fn native_origin_retains_exact_identity_without_changing_legacy_dto_or_equality() {
    let first = DefinitionSource::new("memory:shared.xtext", SourceKind::Xtext, "first");
    let second = DefinitionSource::new("memory:shared.xtext", SourceKind::Xtext, "second");
    let legacy = Diagnostic::new(&first.id.uri, 1, 3, "message");
    let located = legacy.clone().with_span(SourceSpan {
        source: second.id.clone(),
        start: 1,
        end: 3,
    });
    assert_eq!(located, legacy);
    assert_eq!(
        format!("{located:?}"),
        r#"Diagnostic { source: "memory:shared.xtext", start: 1, end: 3, message: "message" }"#
    );
    assert_eq!(format!("{located:?}"), format!("{legacy:?}"));
    assert_eq!(format!("{located:#?}"), format!("{legacy:#?}"));
    assert_eq!(
        serde_json::to_value(&located).unwrap(),
        serde_json::to_value(&legacy).unwrap()
    );
    assert_eq!(
        serde_json::to_value(&located).unwrap(),
        json!({"source":"memory:shared.xtext","start":1,"end":3,"message":"message"})
    );
    assert_eq!(
        located.origin.as_deref(),
        Some(&DiagnosticOrigin::Source {
            source: second.id.clone(),
            span: Some(SourceSpan {
                source: second.id.clone(),
                start: 1,
                end: 3
            })
        })
    );
    assert_ne!(first.id, second.id);
}

#[test]
fn serialized_diagnostic_metadata_never_becomes_native_provenance() {
    let source = DefinitionSource::new("memory:real.xtext", SourceKind::Xtext, "body");
    let mut value =
        serde_json::to_value(Diagnostic::at_source(&source.id, 0, 1, "message")).unwrap();
    value["origin"] =
        json!({"kind":"source","source":{"uri":"memory:forged","sha256":"0".repeat(64)}});
    let loaded: Diagnostic = serde_json::from_value(value).unwrap();
    assert_eq!(loaded.origin, None);
    assert_eq!(loaded, Diagnostic::new(&source.id.uri, 0, 1, "message"));
}

#[test]
fn contexts_and_generated_correspondence_remain_distinct_from_authored_source_ownership() {
    let source = DefinitionSource::new("memory:input.xtext", SourceKind::Xtext, "body");
    let generated = DefinitionSource::new("memory:generated.rcore", SourceKind::Rcore, "generated");
    let context = Diagnostic::new("bundle", 0, 0, "invalid world").with_context(&source.id);
    assert_eq!(
        context.origin.as_deref(),
        Some(&DiagnosticOrigin::Context {
            source: source.id.clone()
        })
    );
    let originals = vec![
        SourceSpan {
            source: source.id.clone(),
            start: 0,
            end: 1,
        },
        SourceSpan {
            source: source.id.clone(),
            start: 2,
            end: 4,
        },
    ];
    let error = Diagnostic::new("legacy", 0, 1, "generated error").with_generated(
        SourceSpan {
            source: generated.id,
            start: 0,
            end: 3,
        },
        originals.clone(),
    );
    let Some(DiagnosticOrigin::Generated {
        originals: retained,
        ..
    }) = error.origin.as_deref()
    else {
        panic!("generated provenance was lost")
    };
    assert_eq!(retained, &originals);
}
