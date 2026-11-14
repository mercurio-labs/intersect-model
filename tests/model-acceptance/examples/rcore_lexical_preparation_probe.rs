//! Verification-only shared native/WASM lexical and actual consumer controls.
use intersect_model::definitions::{
    bundle::{DefinitionBundle, DefinitionSource, SourceKind},
    conversion::{
        ConversionRegistry, InvocationContext, InvocationObserver, ProviderId, ReferenceContext,
    },
    ecore::{ErrorKind, ObservedImportError, ParseLimits},
    generic_types::{RuntimeType, Type},
    rcore_behavior::{self, LexicalPreparation, TokenKind},
    resources::{DeclarationId, ResourceSet, ResourceSetPreparation},
    schema::SharedRuntimeSourceArrayTarget,
};
use intersect_model::model_reference::{ReferenceSchema, ReferenceSchemaFactory};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::{cell::Cell, rc::Rc};
fn check(value: bool, message: &str) -> Result<(), String> {
    if value { Ok(()) } else { Err(message.into()) }
}
#[derive(Default)]
struct Meter {
    calls: Cell<usize>,
    work: Cell<usize>,
    bytes: Cell<usize>,
    stop: Option<usize>,
    max_work: Option<usize>,
    max_bytes: Option<usize>,
}
impl InvocationObserver for Meter {
    fn before_invoke(&self, _: &ProviderId) -> Result<(), String> {
        Err("Unexpected provider".into())
    }
    fn checkpoint(&self) -> Result<(), String> {
        let current = self.calls.get();
        self.calls.set(current + 1);
        if self.stop == Some(current) {
            Err("lexical interrupted".into())
        } else {
            Ok(())
        }
    }
    fn work(&self, amount: usize) -> Result<(), String> {
        self.checkpoint()?;
        let next = self.work.get() + amount;
        if self.max_work.is_some_and(|limit| next > limit) {
            return Err("lexical work denied".into());
        }
        self.work.set(next);
        Ok(())
    }
    fn retain_storage(&self, amount: usize) -> Result<(), String> {
        self.checkpoint()?;
        let next = self.bytes.get() + amount;
        if self.max_bytes.is_some_and(|limit| next > limit) {
            return Err("lexical storage denied".into());
        }
        self.bytes.set(next);
        Ok(())
    }
}
fn compatibility() -> Result<(), String> {
    #[derive(Deserialize)]
    struct Capture {
        cases: Vec<Case>,
    }
    #[derive(Deserialize)]
    struct Case {
        source: DefinitionSource,
        expected_sha256: String,
    }
    let bytes = include_bytes!(
        "../../../crates/intersect-model/assets/fixtures/rcore-lexical-baseline.json"
    );
    check(
        format!("{:x}", Sha256::digest(bytes))
            == "17751e34dfb020492a4bb05ccc1fc1b53d6f49ca69c50998f1e781acfd888167",
        "Frozen lexical capture changed",
    )?;
    let capture: Capture = serde_json::from_slice(bytes).map_err(|e| e.to_string())?;
    check(
        capture.cases.len() == 1840,
        "Missing lexical compatibility cases",
    )?;
    for (index, case) in capture.cases.into_iter().enumerate() {
        let current = rcore_behavior::lex(case.source);
        let encoded = serde_json::to_vec(&current).map_err(|e| e.to_string())?;
        if format!("{:x}", Sha256::digest(encoded)) != case.expected_sha256 {
            return Err(format!(
                "Complete lexical compatibility case {index} changed: {current:?}"
            ));
        }
    }
    Ok(())
}
fn literals() -> Result<(), String> {
    let mut source = DefinitionSource::new(
        "memory:literal",
        SourceKind::RcoreBehavior,
        "abc 12.5e-2 `a``b` \"\\uD83D\\uDE80\\n\" // skip\r\n+ @ /* unterminated",
    );
    let first = DefinitionSource::new("dependency:revision", SourceKind::Xtext, "first").id;
    let second = DefinitionSource::new("dependency:revision", SourceKind::Xtext, "second").id;
    source.dependencies = vec![second.clone(), first, second];
    let moved = source.clone();
    let text_pointer = moved.text.as_ptr();
    let dependency_pointer = moved.dependencies.as_ptr();
    let output = rcore_behavior::lex(moved);
    check(
        output.source == source
            && output.source.text.as_ptr() == text_pointer
            && output.source.dependencies.as_ptr() == dependency_pointer,
        "Ordinary lexing changed complete source ownership or dependency order/revisions",
    )?;
    let expected = [
        (TokenKind::Word("abc".into()), 0, 3),
        (TokenKind::Number("12.5e-2".into()), 4, 11),
        (TokenKind::QuotedName("a`b".into()), 12, 18),
        (TokenKind::String("🚀\n".into()), 19, 35),
        (TokenKind::Symbol("+".into()), 45, 46),
        (TokenKind::Error, 47, 48),
        (TokenKind::Error, 49, 64),
        (TokenKind::End, 64, 64),
    ];
    check(
        source.text.len() == 64 && output.tokens.len() == expected.len(),
        "Literal lexical extent changed",
    )?;
    for (token, (kind, start, end)) in output.tokens.iter().zip(expected) {
        check(
            token.kind == kind
                && token.span.start == start
                && token.span.end == end
                && token.span.source == source.id,
            "Literal token/value/span/identity changed",
        )?;
    }
    check(
        output.diagnostics.len() == 2
            && output.diagnostics[0].message == "Unexpected character '@'"
            && output.diagnostics[0].span == output.tokens[5].span
            && output.diagnostics[1].message == "Unterminated block comment"
            && output.diagnostics[1].span == output.tokens[6].span,
        "Literal lexical diagnostics changed",
    )?;
    for (text, message, end) in [
        ("\"\\x", "Unterminated string", 3),
        ("\"\\x\"", "Invalid string escape or control character", 4),
        ("1e+ x", "Missing exponent digits", 3),
        ("``", "Empty quoted name", 2),
        ("`a\nnext", "Unterminated quoted name", 2),
    ] {
        let current = rcore_behavior::lex(DefinitionSource::new(
            "memory:negative",
            SourceKind::Rcore,
            text,
        ));
        check(
            current
                .diagnostics
                .first()
                .is_some_and(|d| d.message == message && d.span.start == 0 && d.span.end == end),
            "Literal negative/recovery priority changed",
        )?;
    }
    for (uri, forged, kind, message) in [
        (
            "",
            true,
            SourceKind::Ecore,
            "Definition source URI must be nonempty and contain no controls",
        ),
        (
            "memory:bad",
            true,
            SourceKind::Ecore,
            "Source content digest mismatch: memory:bad",
        ),
        (
            "memory:kind",
            false,
            SourceKind::Ecore,
            "Expected a Rcore source",
        ),
    ] {
        let mut source = DefinitionSource::new(uri, kind, "word");
        if forged {
            source.id.sha256.clear();
        }
        let output = rcore_behavior::lex(source.clone());
        check(
            output.diagnostics.len() == 1
                && output.diagnostics[0].message == message
                && output.diagnostics[0].span.source == source.id
                && output.diagnostics[0].span.start == 0
                && output.diagnostics[0].span.end == 0
                && output.tokens.len() == 2
                && output.tokens[0].kind == TokenKind::Error
                && output.tokens[1].kind == TokenKind::End,
            "Source URI/hash/kind priority changed",
        )?;
    }
    Ok(())
}
fn json_boundaries() -> Result<(), String> {
    for offset in 0..=257 {
        for escape in [
            r#"\uD83D\uDE80"#,
            r#"\uD800"#,
            r#"\uDC00"#,
            r#"\uD800\u0041"#,
            r#"\uZZZZ"#,
            r#"\x"#,
            r#"\\"#,
            r#"\""#,
            r#"\n\t\b\f\r\/"#,
        ] {
            let raw = format!("\"{}{}{}\"", "a".repeat(offset), escape, "é🚀".repeat(70));
            let expected = serde_json::from_str::<String>(&raw);
            let output = rcore_behavior::lex(DefinitionSource::new(
                "memory:json",
                SourceKind::Rcore,
                raw.clone(),
            ));
            check(
                output.tokens.len() == 2
                    && output.tokens[0].span.start == 0
                    && output.tokens[0].span.end == raw.len()
                    && output.tokens[1].kind == TokenKind::End,
                "JSON chunk boundaries changed token extent",
            )?;
            match expected {
                Ok(value) => check(
                    output.tokens[0].kind == TokenKind::String(value)
                        && output.diagnostics.is_empty(),
                    "JSON chunk decoding changed a value",
                )?,
                Err(_) => check(
                    output.tokens[0].kind == TokenKind::Error
                        && output.diagnostics.len() == 1
                        && output.diagnostics[0].message
                            == "Invalid string escape or control character",
                    "JSON chunk decoding changed rejection",
                )?,
            }
        }
    }
    Ok(())
}
fn interruptions() -> Result<(), String> {
    for text in [
        format!("/*{}*/word", "λ🚀".repeat(150)),
        format!("\"{}\\uD83D\\uDE80\"", "λ🚀".repeat(150)),
        format!("`{}`", "λ``🚀".repeat(150)),
        "identifier".repeat(100),
        format!("{}e-{}", "1".repeat(500), "2".repeat(500)),
    ] {
        let source = DefinitionSource::new("memory:interrupt", SourceKind::Rcore, text);
        let expected = rcore_behavior::lex(source.clone());
        let original = source.text.as_ptr();
        let mut steps = 0;
        let mut baseline = LexicalPreparation::new(&source);
        while !baseline.advance(&InvocationContext::default())? {
            steps += 1;
        }
        steps += 1;
        let result = baseline.finish()?;
        check(
            result.tokens() == expected.tokens
                && result.diagnostics() == expected.diagnostics
                && steps > source.text.len() / 256,
            "Long token did not retain exact lexical progress",
        )?;
        for stop in 0..=steps {
            let mut pending = LexicalPreparation::new(&source);
            check(
                pending.finish().is_err(),
                "Incomplete lexical output was published",
            )?;
            pending = LexicalPreparation::new(&source);
            for _ in 0..stop {
                pending.advance(&InvocationContext::default())?;
            }
            let observer = Rc::new(Meter {
                stop: Some(0),
                ..Default::default()
            });
            let weak = Rc::downgrade(&observer);
            check(
                pending.advance(&InvocationContext::new(observer))
                    == Err("lexical interrupted".into())
                    && weak.upgrade().is_none()
                    && pending.advance(&InvocationContext::default())
                        == Err("lexical interrupted".into())
                    && matches!(pending.finish(), Err(message) if message == "lexical interrupted"),
                "Lexical cancellation was not terminal/fresh",
            )?;
        }
        for quantum in [1, 3, 256] {
            let mut pending = LexicalPreparation::new(&source);
            let mut done = false;
            while !done {
                for _ in 0..quantum {
                    let observer = Rc::new(Meter::default());
                    let weak = Rc::downgrade(&observer);
                    done = pending.advance(&InvocationContext::new(observer))?;
                    check(
                        weak.upgrade().is_none(),
                        "Lexical cursor retained an observer",
                    )?;
                    if done {
                        break;
                    }
                }
            }
            let result = pending.finish()?;
            check(
                result.tokens() == expected.tokens
                    && result.diagnostics() == expected.diagnostics
                    && result.source().text.as_ptr() == original,
                "Lexical resume grouping or source ownership changed",
            )?;
        }
    }
    Ok(())
}
const NS: &str = "urn:lexical";
fn fixture() -> DefinitionBundle {
    DefinitionBundle {
        version: 1,
        namespaces: vec![],
        sources: vec![DefinitionSource::new(
            "memory:lexical.rcore",
            SourceKind::Rcore,
            format!(
                "rcore 1; package lexical version \"1\"; /*{}*/ metamodel \"urn:lexical\" prefix lx {{ class Item {{}} }}",
                "λ🚀".repeat(150)
            ),
        )],
    }
}
fn run_resource(
    bundle: &DefinitionBundle,
    limits: ParseLimits,
    retained: bool,
    meter: Rc<Meter>,
) -> Result<ResourceSet, ObservedImportError> {
    let operation = InvocationContext::new(meter);
    let mut pending = if retained {
        ResourceSetPreparation::begin_with_retained_bundle(bundle, limits, &operation)
    } else {
        ResourceSetPreparation::begin(bundle, limits, &operation)
    }?;
    while !pending.advance(&operation)? {}
    pending.finish()
}
fn resources() -> Result<(), String> {
    let bundle = fixture();
    let old = Rc::new(Meter::default());
    let meter = Rc::new(Meter::default());
    let expected = run_resource(&bundle, ParseLimits::default(), false, old.clone())
        .map_err(|e| e.to_string())?;
    let current = run_resource(&bundle, ParseLimits::default(), true, meter.clone())
        .map_err(|e| e.to_string())?;
    check(
        format!("{current:?}") == format!("{expected:?}")
            && current.document(&bundle.sources[0].id)?.source == bundle.sources[0]
            && (meter.work.get(), meter.bytes.get()) == (old.work.get(), old.bytes.get()),
        "Retained Rcore documents or logical receipts changed",
    )?;
    for (work, bytes, success) in [
        (meter.work.get(), meter.bytes.get(), true),
        (meter.work.get() - 1, meter.bytes.get(), false),
        (meter.work.get(), meter.bytes.get() - 1, false),
    ] {
        check(
            run_resource(
                &bundle,
                ParseLimits::default(),
                true,
                Rc::new(Meter {
                    max_work: Some(work),
                    max_bytes: Some(bytes),
                    ..Default::default()
                }),
            )
            .is_ok()
                == success,
            "Adjacent Rcore resource limits changed",
        )?;
    }
    for stop in 0..meter.calls.get() {
        check(
            matches!(run_resource(&bundle, ParseLimits::default(), true, Rc::new(Meter {
            stop: Some(stop), ..Default::default()
        })), Err(ObservedImportError::Observation(message)) if message == "lexical interrupted"),
            "Rcore resource missed an observer interruption",
        )?;
    }
    for invalid in [false, true] {
        let mut bad = bundle.clone();
        if invalid {
            bad.sources[0] = DefinitionSource::new(
                "memory:bad.rcore",
                SourceKind::Rcore,
                "rcore 1;package bad version \"1\";metamodel \"\\uD800\" prefix b { class Item {} }",
            );
        }
        let limits = if invalid {
            ParseLimits::default()
        } else {
            ParseLimits {
                source_bytes: bad.sources[0].text.len() - 1,
            }
        };
        let old = run_resource(&bad, limits, false, Rc::new(Meter::default())).unwrap_err();
        let new = run_resource(&bad, limits, true, Rc::new(Meter::default())).unwrap_err();
        check(
            old == new,
            "Rcore resource diagnostic kind/span/priority changed",
        )?;
        let ObservedImportError::Import(error) = new else {
            return Err("Lost Rcore import diagnostic".into());
        };
        check(
            error.span.source == bad.sources[0].id
                && if invalid {
                    error.kind == ErrorKind::InvalidDefinition
                        && error.message == "Invalid string escape or control character"
                        && error.span.start == 42
                        && error.span.end == 50
                } else {
                    error.kind == ErrorKind::ResourceLimit
                        && error.message == "Rcore source size budget exceeded"
                        && error.span.start == 0
                        && error.span.end == bad.sources[0].text.len()
                },
            "Literal Rcore resource rejection changed",
        )?;
    }
    Ok(())
}
fn factory() -> Result<(), String> {
    let bundle = fixture();
    let entry = bundle.sources[0].id.clone();
    let types = vec![RuntimeType::new(Type::Classifier {
        declaration: DeclarationId {
            source: entry.clone(),
            path: vec![0],
        },
        arguments: vec![],
        raw: false,
    })?];
    let expected = intersect_model::definitions::compiler::materialize_runtime_bundle(
        &bundle, &entry, NS, &types, None,
    )
    .map_err(|e| e.to_string())?;
    check(
        expected.classes.len() == 1,
        "Authored Item schema did not compile",
    )?;
    let registry = ConversionRegistry::default();
    for prefix in [None, Some(0), Some(16), Some(64), Some(256), Some(512)] {
        let input = bundle.clone();
        let pointer = input.sources[0].text.as_ptr();
        let mut context = ReferenceContext {
            bundle: input,
            entry: entry.clone(),
            namespace: NS.into(),
            conversions: vec![],
            materialized_types: types.clone(),
            applied_conversions: vec![],
            constructors: vec![],
            model: serde_json::json!({}),
        };
        let operation = InvocationContext::default();
        let mut pending =
            SharedRuntimeSourceArrayTarget::begin_compilation(&mut context, &registry, &operation)?
                .ok_or("Actual factory did not retain compilation")?;
        if let Some(prefix) = prefix {
            for _ in 0..prefix {
                check(
                    !SharedRuntimeSourceArrayTarget::advance_compilation(
                        &mut pending,
                        &registry,
                        &operation,
                    )?,
                    "Unexpected early schema publication",
                )?;
            }
            let cancelled = InvocationContext::new(Rc::new(Meter {
                stop: Some(0),
                ..Default::default()
            }));
            check(
                SharedRuntimeSourceArrayTarget::advance_compilation(
                    &mut pending,
                    &registry,
                    &cancelled,
                )
                .is_err()
                    && SharedRuntimeSourceArrayTarget::advance_compilation(
                        &mut pending,
                        &registry,
                        &operation,
                    )
                    .is_err(),
                "Actual lexical factory cancellation was not terminal",
            )?;
        } else {
            while !SharedRuntimeSourceArrayTarget::advance_compilation(
                &mut pending,
                &registry,
                &operation,
            )? {}
        }
        let result =
            SharedRuntimeSourceArrayTarget::finish_compilation(pending, &mut context, &operation);
        check(
            context.bundle == bundle && context.bundle.sources[0].text.as_ptr() == pointer,
            "Actual factory lost exact source allocations",
        )?;
        if prefix.is_some() {
            check(
                result.is_err(),
                "Cancelled lexical factory published a schema",
            )?;
        } else {
            let ReferenceSchema::Checked(schema) = result? else {
                return Err("Actual factory lost checked completion".into());
            };
            check(
                serde_json::to_value(schema.schema()).map_err(|e| e.to_string())?
                    == serde_json::to_value(&expected).map_err(|e| e.to_string())?,
                "Actual lexical factory changed the complete schema",
            )?;
        }
    }
    Ok(())
}
fn verify() -> Result<u32, String> {
    compatibility()?;
    literals()?;
    json_boundaries()?;
    interruptions()?;
    resources()?;
    factory()?;
    Ok(6)
}
#[unsafe(no_mangle)]
pub extern "C" fn verify_rcore_lexical_preparation() -> u32 {
    verify().unwrap_or(0)
}
fn main() -> Result<(), String> {
    println!("{} retained Rcore lexical groups passed", verify()?);
    Ok(())
}
