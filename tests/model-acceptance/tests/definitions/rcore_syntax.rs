use intersect_model::definitions::{
    bundle::{DefinitionBundle, DefinitionSource, SourceKind},
    rcore_behavior::{
        ast::{Declaration, Expr, Literal},
        function_lowering::compile_function,
        ir_v2::{EntrySelection, Limits},
        modules::SymbolId,
        parse,
    },
};

fn source(uri: &str, text: &str) -> DefinitionSource {
    DefinitionSource::new(uri, SourceKind::RcoreBehavior, text)
}

#[test]
fn rcore_blocks_retain_original_utf8_bytes_spans_and_shared_ast() {
    let text =
        "rcore 1; package demo version \"1\"; fn main()->String { let label=\"café😀\"; label }";
    let entry = source("memory:demo.rcore", text);
    let document = parse(entry.clone());
    assert!(
        document.diagnostics.is_empty(),
        "{:?}",
        document.diagnostics
    );
    assert_eq!(document.source, entry);
    assert_eq!(document.syntax_version.as_ref().unwrap().value, "1");
    let Declaration::Function { body, .. } = document.declarations[0].value else {
        panic!("Expected shared function declaration")
    };
    assert!(matches!(
        document.expressions[body.0].value,
        Expr::Block { .. }
    ));
    let literal = document
        .expressions
        .iter()
        .find(|e| matches!(&e.value, Expr::Literal(Literal::String(s)) if s == "café😀"))
        .unwrap();
    assert_eq!(&text[literal.span.start..literal.span.end], "\"café😀\"");
    assert_eq!(literal.span.source, entry.id);
    let decoded =
        serde_json::from_str::<intersect_model::definitions::rcore_behavior::ast::Document>(
            &serde_json::to_string(&document).unwrap(),
        )
        .unwrap();
    assert_eq!(decoded, document);
}

#[test]
fn rcore_accepts_imports_generics_optional_block_separator_and_expression_bodies() {
    let document = parse(source(
        "memory:syntax.rcore",
        "rcore 1; package demo version \"1\"; import model \"urn:test\" as m; import package helper version \"2\" as h; pub fn keep<T>(value:T)->T { value }; fn main()->Int8=42;",
    ));
    assert!(
        document.diagnostics.is_empty(),
        "{:?}",
        document.diagnostics
    );
    assert_eq!(document.declarations.len(), 4);
    assert!(matches!(
        document.declarations[0].value,
        Declaration::ModelImport { .. }
    ));
    assert!(matches!(
        document.declarations[1].value,
        Declaration::ModuleImport { .. }
    ));
}

#[test]
fn mixed_packages_compile_through_the_same_binder_checker_and_ir() {
    for (helper_text, caller_text) in [
        (
            "rcore 1; package helper version \"1\"; pub fn keep<T>(x:T)->T=x;",
            "rcore 1; package caller version \"1\"; import package helper version \"1\" as h; fn main()->Int8 { h.keep::<Int8>(42) }",
        ),
        (
            "rcore 1; package helper version \"1\"; pub fn keep<T>(x:T)->T { x }",
            "rcore 1; package caller version \"1\"; import package helper version \"1\" as h; fn main()->Int8=h.keep::<Int8>(42);",
        ),
    ] {
        let helper = source("memory:helper", helper_text);
        let mut entry = source("memory:caller", caller_text);
        entry.dependencies.push(helper.id.clone());
        let bundle = DefinitionBundle {
            version: 1,
            sources: vec![helper.clone(), entry.clone()],
            namespaces: vec![],
        };
        let image = compile_function(
            &bundle,
            &entry.id,
            &EntrySelection {
                symbol: SymbolId {
                    source: entry.id.clone(),
                    declaration: 1,
                },
                types: vec![],
                arguments: vec![],
            },
            &Limits::default(),
        )
        .unwrap();
        assert_eq!(image.program().entry_source, entry.id);
        assert!(
            image
                .program()
                .functions
                .iter()
                .any(|f| f.span.source == helper.id)
        );
        for f in &image.program().functions {
            assert!(bundle.sources.iter().any(|s| s.id == f.span.source));
        }
    }
}

#[test]
fn quoted_keywords_and_current_block_grammar_are_valid() {
    let document = parse(source(
        "memory:keywords.rcore",
        "rcore 1; package `package`.`ecore`.`rcore` version \"1\"; fn `package`()->Int8=1; fn `ecore`()->Int8=`package`(); fn `rcore`()->Int8= `ecore`();",
    ));
    assert!(
        document.diagnostics.is_empty(),
        "{:?}",
        document.diagnostics
    );
    assert_eq!(document.declarations.len(), 3);
    let block = parse(source(
        "memory:block.rcore",
        "rcore 1; package demo version \"1\"; fn main()->Int8 {42}",
    ));
    assert!(block.diagnostics.is_empty(), "{:?}", block.diagnostics);
}

#[test]
fn unsupported_versions_structural_forms_and_malformed_blocks_have_real_locations() {
    for text in [
        "rcore 2; package demo version \"1\"; fn main()->Int8 {42}",
        "rcore 1; package demo version \"1\"; class Type {}",
        "rcore 1; package demo version \"1\"; fn main()->Int8 { let x=1; }",
        "rcore 1; package demo version \"1\"; fn package()->Int8 {42}",
    ] {
        let entry = source("memory:invalid.rcore", text);
        let document = parse(entry.clone());
        assert!(!document.diagnostics.is_empty(), "Accepted {text}");
        for diagnostic in &document.diagnostics {
            assert_eq!(diagnostic.span.source, entry.id);
            assert!(
                diagnostic.span.start <= diagnostic.span.end && diagnostic.span.end <= text.len()
            );
        }
    }
}

#[test]
fn rcore_nesting_is_bounded_and_type_errors_preserve_source_identity() {
    let text = format!(
        "rcore 1; package demo version \"1\"; fn main()->Int8 {{{}42{}}}",
        "(".repeat(90),
        ")".repeat(90)
    );
    let document = parse(source("memory:deep.rcore", &text));
    assert!(
        document
            .diagnostics
            .iter()
            .any(|d| d.message.contains("nesting"))
    );
    let entry = source(
        "memory:typed.rcore",
        "rcore 1; package demo version \"1\"; fn main()->Int8 { let label=\"café😀\"; true }",
    );
    let bundle = DefinitionBundle {
        version: 1,
        sources: vec![entry.clone()],
        namespaces: vec![],
    };
    let errors = compile_function(
        &bundle,
        &entry.id,
        &EntrySelection {
            symbol: SymbolId {
                source: entry.id.clone(),
                declaration: 0,
            },
            types: vec![],
            arguments: vec![],
        },
        &Limits::default(),
    )
    .unwrap_err();
    assert!(!errors.is_empty());
    for error in errors {
        assert_eq!(error.span.source, entry.id);
        assert!(
            entry.text.is_char_boundary(error.span.start)
                && entry.text.is_char_boundary(error.span.end)
        );
    }
}
