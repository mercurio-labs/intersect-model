use super::*;
use crate::definitions::{
    bundle::{DefinitionSource, SourceKind},
    rcore_behavior::{self, ast::Declaration},
};

const SOURCE: &str = r#"rcore 1;
package example.scopes version "1";
metamodel "urn:scope-test" prefix m {
  class Namespace {
    ref types: Type [0,*] containment;
    ref properties: Property [0,*] containment;
  }
  class Type { attr name: String [1,1]; }
  class Property {
    ref owner: Namespace [0,1];
    ref targetType: Type [0,1];
  }
}
behavior {
  import model "urn:scope-test" as m;
  bind scope m.Property.targetType = visible_types;
  fn visible_types(owner:m.Property, spelling:String)->Sequence<m.Type> = [];
}
"#;

fn bundle(text: &str) -> (DefinitionBundle, SourceId) {
    let source = DefinitionSource::new("memory:scope-test.rcore", SourceKind::Rcore, text);
    let entry = source.id.clone();
    (
        DefinitionBundle {
            version: 1,
            sources: vec![source],
            namespaces: vec![],
        },
        entry,
    )
}

fn compile(text: &str) -> Result<VerifiedScope, Vec<SyntaxDiagnostic>> {
    let (bundle, entry) = bundle(text);
    compile_scope(&bundle, &entry, 1, &Limits::default())
}

#[test]
fn scope_declaration_retains_original_paths_and_source_span() {
    let source = DefinitionSource::new("memory:scope-test.rcore", SourceKind::Rcore, SOURCE);
    let document = rcore_behavior::parse(source.clone());
    assert!(
        document.diagnostics.is_empty(),
        "{:?}",
        document.diagnostics
    );
    let selected = &document.declarations[1];
    let Declaration::ScopeBinding {
        target,
        implementation,
    } = &selected.value
    else {
        panic!("Expected a scope binding")
    };
    assert_eq!(
        target
            .iter()
            .map(|name| name.value.as_str())
            .collect::<Vec<_>>(),
        ["m", "Property", "targetType"]
    );
    assert_eq!(implementation[0].value, "visible_types");
    assert_eq!(selected.span.source, source.id);
    assert_eq!(
        &SOURCE[selected.span.start..selected.span.end],
        "bind scope m.Property.targetType = visible_types;"
    );
}

#[test]
fn scope_contract_is_bound_to_exact_reference_and_function() {
    let scope = compile(SOURCE).expect("source scope compiles");
    let (bundle, entry) = bundle(SOURCE);
    assert_eq!(scope.binding().implementation.source, entry);
    assert_eq!(scope.binding().implementation.declaration, 2);
    assert!(
        matches!(scope.member(), MemberIdentity::Source { declaration } if declaration.source == entry)
    );
    scope
        .executable()
        .check_context(&bundle, None)
        .expect("exact source context");
    assert_eq!(scope.span().source, entry);
    assert_ne!(scope.receiver(), scope.target());
}

#[test]
fn scope_contract_reuses_recursive_model_navigation_closure() {
    let source = SOURCE.replace(
        "Sequence<m.Type> = []",
        "Sequence<m.Type> = match owner.owner { Some(ns) => Sequence(filter(ns.types, |candidate| candidate.name == spelling)), None => [] }",
    );
    compile(&source)
        .expect("receiver references and their navigation targets share checked closure");
}

#[test]
fn selector_replay_checks_source_identity_and_explicit_ceiling() {
    let mut source = DefinitionSource::new("memory:scope-test.rcore", SourceKind::Rcore, SOURCE);
    let descriptors = source_scope_bindings(&source, Default::default()).unwrap();
    assert_eq!(
        descriptors,
        vec![ScopeBindingDescriptor {
            source: source.id.clone(),
            declaration: 1
        }]
    );
    assert!(
        source_scope_bindings(
            &source,
            crate::definitions::ecore::ParseLimits {
                source_bytes: SOURCE.len() - 1
            }
        )
        .is_err()
    );
    source.text.push_str("\n// changed without identity\n");
    assert!(source_scope_bindings(&source, Default::default()).is_err());
}

#[test]
fn scope_contract_rejects_wrong_signatures_and_nonreferences() {
    for source in [
        SOURCE.replace("owner:m.Property", "owner:m.Type"),
        SOURCE.replace("spelling:String", "spelling:Bool"),
        SOURCE.replace("Sequence<m.Type>", "Sequence<m.Property>"),
        SOURCE.replace("Sequence<m.Type> = []", "String = \"wrong\""),
        SOURCE.replace("bind scope m.Property.targetType", "bind scope m.Type.name"),
        SOURCE.replace(
            "bind scope m.Property.targetType",
            "bind scope m.Namespace.types",
        ),
        SOURCE.replace("fn visible_types(", "fn visible_types<T>("),
    ] {
        assert!(
            compile(&source).is_err(),
            "Unexpected admitted source: {source}"
        );
    }
}

#[test]
fn missing_duplicate_and_overloaded_scope_implementations_reject() {
    for source in [
        SOURCE.replace("= visible_types;", "= absent;"),
        SOURCE.replace("  bind scope", "  bind scope m.Property.targetType = visible_types;\n  bind scope"),
        SOURCE.replace("  fn visible_types", "  fn visible_types(owner:m.Property, spelling:Bool)->Sequence<m.Type> = [];\n  fn visible_types"),
    ] {
        assert!(compile(&source).is_err(), "Unexpected admitted source: {source}");
    }
}

#[test]
fn stale_sources_and_nonbinding_declaration_ordinals_reject() {
    let (mut bundle, entry) = bundle(SOURCE);
    assert!(compile_scope(&bundle, &entry, 2, &Limits::default()).is_err());
    bundle.sources[0].text.push_str("// changed source\n");
    assert!(compile_scope(&bundle, &entry, 1, &Limits::default()).is_err());
}

#[test]
fn scope_source_limits_are_explicit_and_do_not_raise_normal_entry_point_limits() {
    let text = format!("{SOURCE}\n/* {} */", "x".repeat(256_000));
    let (bundle, entry) = bundle(&text);
    let limits = Limits::default();
    assert!(compile_scope(&bundle, &entry, 1, &limits).is_err());
    assert!(
        compile_scope_with_source_limits(
            &bundle,
            &entry,
            1,
            &limits,
            crate::definitions::ecore::ParseLimits {
                source_bytes: SOURCE.len()
            },
        )
        .is_err()
    );
    compile_scope_with_source_limits(
        &bundle,
        &entry,
        1,
        &limits,
        crate::definitions::ecore::ParseLimits {
            source_bytes: text.len(),
        },
    )
    .expect("explicit finite source admission");
}

#[test]
fn malformed_scope_binding_recovers_at_next_declaration() {
    let text = SOURCE.replace("= visible_types;", "visible_types;");
    let source = DefinitionSource::new("memory:scope-test.rcore", SourceKind::Rcore, text);
    let document = rcore_behavior::parse(source);
    assert!(!document.diagnostics.is_empty());
    assert!(
        document
            .declarations
            .iter()
            .any(|declaration| matches!(declaration.value, Declaration::Function { .. }))
    );
}

#[test]
fn bind_and_scope_remain_contextual_identifiers() {
    let text = r#"rcore 1; package contextual version "1";
fn bind(scope:String)->String = scope;
"#;
    let source = DefinitionSource::new("memory:contextual.rcore", SourceKind::RcoreBehavior, text);
    let document = rcore_behavior::parse(source);
    assert!(
        document.diagnostics.is_empty(),
        "{:?}",
        document.diagnostics
    );
}

#[test]
fn imported_scope_functions_require_exact_dependencies_and_public_visibility() {
    let entry_text = SOURCE
        .replace(
            "bind scope m.Property.targetType = visible_types;",
            "import package example.helpers version \"1\" as h;\n  bind scope m.Property.targetType = h.visible_types;",
        )
        .replace(
            "  fn visible_types(owner:m.Property, spelling:String)->Sequence<m.Type> = [];\n",
            "",
        );
    for public in [true, false] {
        let mut entry =
            DefinitionSource::new("memory:scope-test.rcore", SourceKind::Rcore, &entry_text);
        let helper_text = format!(
            "rcore 1;package example.helpers version \"1\";import model \"urn:scope-test\" as m;\n{}fn visible_types(owner:m.Property, spelling:String)->Sequence<m.Type> = [];",
            if public { "pub " } else { "" },
        );
        let mut helper = DefinitionSource::new(
            "memory:scope-helpers.rcore",
            SourceKind::RcoreBehavior,
            helper_text,
        );
        helper.dependencies.push(entry.id.clone());
        entry.dependencies.push(helper.id.clone());
        let bundle = DefinitionBundle {
            version: 1,
            sources: vec![entry.clone(), helper.clone()],
            namespaces: vec![],
        };
        let result = compile_scope(&bundle, &entry.id, 2, &Limits::default());
        if public {
            let scope = result.expect("explicitly imported public implementation");
            assert_eq!(scope.binding().implementation.source, helper.id);
            let mut missing = bundle;
            missing.sources[0].dependencies.clear();
            assert!(compile_scope(&missing, &entry.id, 2, &Limits::default()).is_err());
        } else {
            assert!(
                result.is_err(),
                "Private imported implementation was admitted"
            );
        }
    }
}

#[test]
fn saved_bindings_reload_only_for_the_same_compiler_identity() {
    let (bundle, _) = bundle(SOURCE);
    let scope = compile(SOURCE).expect("scope compiles");
    let compiler = format!("test+{}", crate::SOURCE_IDENTITY);
    let text = save_bindings(std::slice::from_ref(&scope), &compiler).expect("bindings save");
    // Canonical JSON with the scopes nested, so a canonical JSON document
    // that holds it stores this exact text.
    let value: serde_json::Value = serde_json::from_str(&text).expect("saved text is JSON");
    assert!(
        value["scopes"].is_array(),
        "scopes are nested JSON, not text"
    );
    assert_eq!(
        crate::definitions::pack::canonical_json(&value).expect("canonical"),
        text.as_bytes()
    );
    let loaded = load_bindings(&text, &bundle, &compiler).expect("same compiler reloads");
    assert_eq!(loaded.len(), 1);
    assert_eq!(loaded[0].binding.declaration, scope.binding.declaration);
    assert_eq!(loaded[0].kind, scope.kind);
    let other = format!("test+{}", "0".repeat(64));
    assert_eq!(
        load_bindings(&text, &bundle, &other).err().as_deref(),
        Some("Saved bindings were made by another compiler")
    );
}

/// `save_bindings` text with its scopes edited and the digest recomputed, so
/// only the per-image checks of `load_bindings` can refuse it.
fn edited_bindings(text: &str, edit: impl FnOnce(&mut serde_json::Value)) -> String {
    use sha2::{Digest, Sha256};
    let mut envelope: serde_json::Value = serde_json::from_str(text).expect("envelope");
    let mut scopes = envelope["scopes"].take();
    assert!(scopes.is_array(), "scopes are nested JSON, not text");
    edit(&mut scopes);
    // The digest covers the scopes' text as written: the same value writes
    // the same text inside the envelope.
    let text = serde_json::to_string(&scopes).expect("scopes text");
    envelope["digest"] = format!("{:x}", Sha256::digest(text.as_bytes())).into();
    envelope["scopes"] = scopes;
    envelope.to_string()
}

/// The bundle context hash is computed once per `load_bindings` call, yet
/// every image is still checked against it: an image whose context names
/// another bundle is refused wherever it sits in the batch, and an image
/// saved under other (still sufficient) limits still loads. Whether those
/// limits key the shared hash is covered by
/// `saved_context_keys_hashes_on_limits_and_keeps_no_failure`.
#[test]
fn saved_bindings_refuse_a_mismatched_context_in_a_batch() {
    let (bundle, _) = bundle(SOURCE);
    let scopes = [
        compile(SOURCE).expect("scope compiles"),
        compile(SOURCE).expect("scope compiles"),
    ];
    let compiler = "test-compiler";
    let text = save_bindings(&scopes, compiler).expect("bindings save");
    let unchanged = edited_bindings(&text, |_| {});
    assert_eq!(
        load_bindings(&unchanged, &bundle, compiler)
            .expect("re-encoded bindings reload")
            .len(),
        2
    );
    let other = "0".repeat(64);
    for index in 0..2 {
        let tampered = edited_bindings(&text, |scopes| {
            scopes[index]["executable"]["context"] = other.clone().into();
        });
        assert_eq!(
            load_bindings(&tampered, &bundle, compiler).err().as_deref(),
            Some("Saved image was verified against another bundle"),
            "image {index}"
        );
    }
    // Other (still sufficient) limits on the second image still load. The
    // hash does not depend on the limits, so this cannot catch a cache that
    // ignores them; the test below can.
    let relimited = edited_bindings(&text, |scopes| {
        let limits = &mut scopes[1]["executable"]["limits"]["max_types"];
        *limits = (limits.as_u64().expect("max_types") + 1).into();
    });
    assert_eq!(
        load_bindings(&relimited, &bundle, compiler)
            .expect("other limits reload")
            .len(),
        2
    );
}

/// `SavedContext` keys the bundle context hash on the whole `Limits` and does
/// not keep a failed hash. The bundle carries a long comment, so a
/// `max_source_bytes` that covers the program's serialized size is still too
/// small for the bundle: the program preflight passes and only the context
/// hash (`ir::preflight_bundle`) refuses those limits.
#[test]
fn saved_context_keys_hashes_on_limits_and_keeps_no_failure() {
    const BUNDLE_PREFLIGHT: &str = "IR verification resource budget exceeded";
    let source = format!(
        "{SOURCE}// {}
",
        "x".repeat(65_536)
    );
    let (bundle, _) = bundle(&source);
    let scopes = [
        compile(&source).expect("scope compiles"),
        compile(&source).expect("scope compiles"),
    ];
    let compiler = "test-compiler";
    let text = save_bindings(&scopes, compiler).expect("bindings save");
    let mut images = Vec::new();
    let mut max_source_bytes = 0;
    let starved_text = edited_bindings(&text, |scopes| {
        let program = serde_json::to_string(&scopes[1]["executable"]["program"]).expect("program");
        max_source_bytes = program.len() + 1024;
        images.push(scopes[0]["executable"].clone());
        scopes[1]["executable"]["limits"]["scalar"]["max_source_bytes"] = max_source_bytes.into();
        images.push(scopes[1]["executable"].clone());
    });
    assert!(
        max_source_bytes < source.len(),
        "the bundle must need more bytes than the program"
    );
    let image = |index: usize| -> SavedExecutable {
        serde_json::from_value(images[index].clone()).expect("saved image")
    };
    fn message<T, E: std::fmt::Display>(result: Result<T, E>) -> Option<String> {
        result.err().map(|e| e.to_string())
    }
    let expected = Some(BUNDLE_PREFLIGHT.to_string());
    // Alone, the starved image fails in the context hash, not earlier.
    assert_eq!(
        message(VerifiedExecutable::load_trusted(image(1), &bundle)),
        expected
    );
    let mut context = SavedContext::new(&bundle);
    // A failed hash is computed again, with the same error, each time.
    for _ in 0..2 {
        assert_eq!(
            message(VerifiedExecutable::load_trusted_in(image(1), &mut context)),
            expected
        );
    }
    assert!(VerifiedExecutable::load_trusted_in(image(0), &mut context).is_ok());
    // After a successful hash under the default limits, the starved limits
    // still fail: a cache that ignored `Limits` would accept this image.
    assert_eq!(
        message(VerifiedExecutable::load_trusted_in(image(1), &mut context)),
        expected
    );
    assert!(VerifiedExecutable::load_trusted_in(image(0), &mut context).is_ok());
    // In a batch, the starved image after a good one is refused the same way.
    assert_eq!(
        load_bindings(&starved_text, &bundle, compiler).err(),
        expected
    );
}

/// `load_verified_bindings` skips only the saved digest: it loads what
/// `load_bindings` loads and keeps the compiler check.
#[test]
fn verified_bindings_skip_only_the_saved_digest() {
    let (bundle, _) = bundle(SOURCE);
    let scope = compile(SOURCE).expect("scope compiles");
    let compiler = format!("test+{}", crate::SOURCE_IDENTITY);
    let text = save_bindings(std::slice::from_ref(&scope), &compiler).expect("bindings save");
    let loaded = load_verified_bindings(&text, &bundle, &compiler).expect("verified text loads");
    assert_eq!(loaded.len(), 1);
    assert_eq!(loaded[0].binding.declaration, scope.binding.declaration);
    assert_eq!(loaded[0].kind, scope.kind);
    let other = format!("test+{}", "0".repeat(64));
    assert_eq!(
        load_verified_bindings(&text, &bundle, &other)
            .err()
            .as_deref(),
        Some("Saved bindings were made by another compiler")
    );
    let mut saved: serde_json::Value = serde_json::from_str(&text).expect("envelope");
    saved["digest"] = serde_json::json!("0".repeat(64));
    let forged = saved.to_string();
    assert_eq!(
        load_bindings(&forged, &bundle, &compiler).err().as_deref(),
        Some("Saved bindings are corrupt")
    );
    assert_eq!(
        load_verified_bindings(&forged, &bundle, &compiler)
            .expect("digest is the caller's")
            .len(),
        1
    );
}
