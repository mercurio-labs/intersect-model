use intersect_model::definitions::{
    bundle::{DefinitionSource, SourceKind},
    rcore_behavior::{
        TokenKind,
        ast::{Declaration, Expr, Literal, Type},
        lex, parse,
    },
};

fn source(text: &str) -> DefinitionSource {
    DefinitionSource::new(
        "file:///library-behavior.rcore",
        SourceKind::RcoreBehavior,
        text,
    )
}

fn document(body: &str) -> intersect_model::definitions::rcore_behavior::ast::Document {
    parse(source(&format!(
        "rcore 1; package test.example version \"1\"; {body}"
    )))
}

#[test]
fn old_source_headers_and_artifact_kind_are_rejected() {
    for text in [
        "semdef 2; module old version \"1\";",
        "rcore 1; module old version \"1\";",
        "rcore 1; package old version \"1\"; import ecore \"urn:old\" as m;",
    ] {
        assert!(!parse(source(text)).diagnostics.is_empty(), "{text}");
    }
    assert!(serde_json::from_str::<SourceKind>("\"semantic_definition\"").is_err());
}

#[test]
fn parser_builds_complete_library_declarations_and_exact_locations() {
    let text = include_str!("../../../../fixtures/library/library-v2-behavior.rcore");
    let result = parse(source(text));
    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    assert_eq!(result.declarations.len(), 5);
    assert!(matches!(
        result.declarations[0].value,
        Declaration::ModelImport { .. }
    ));
    assert!(matches!(
        result.declarations[1].value,
        Declaration::Function { public: true, .. }
    ));
    assert!(matches!(
        result.declarations[2].value,
        Declaration::Derive { .. }
    ));
    assert!(matches!(
        result.declarations[4].value,
        Declaration::Validate { .. }
    ));
    for declaration in &result.declarations {
        assert!(!text[declaration.span.start..declaration.span.end].is_empty());
        assert_eq!(declaration.span.source, result.source.id);
    }
    let loaded =
        serde_json::from_str::<intersect_model::definitions::rcore_behavior::ast::Document>(
            &serde_json::to_string(&result).unwrap(),
        )
        .unwrap();
    assert_eq!(loaded, result);
}

#[test]
fn parser_covers_control_collections_types_patterns_and_quoted_names() {
    let result = document(
        r#"
        import package shared.tools version "3" as tools;
        private fn `match`<T: lib.Element>(value: Result<T, String>?, callback: (Int, Bool) -> String, pair: (Int, String), record: {name: String, count: Int}) -> Sequence<Int> = {
            let label: String = "café😀";
            let values = [1, 2, 3,];
            let record = {name: label, pair: (1, true), unit: (), one: (1,)};
            let lambda = |x: Int, y: Int| x + y;
            let empty = || 42;
            if !false && true { values.map::<Int>(lambda)[0] } else {
                match value { Some(item) => item?.count ?? 0, none => 0, "x" => 1, -1 => 3, _ => 2, }
            }
        };
        fn explicit() -> Int = { let x = 1; +x * -2 / 3 % 4 };
    "#,
    );
    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    assert_eq!(result.declarations.len(), 3);
    assert!(
        result
            .expressions
            .iter()
            .any(|e| matches!(e.value, Expr::Match { .. }))
    );
    assert!(
        result
            .expressions
            .iter()
            .any(|e| matches!(e.value, Expr::Record(_)))
    );
    assert!(
        result
            .types
            .iter()
            .any(|t| matches!(t.value, Type::Function { .. }))
    );
    assert!(
        result
            .types
            .iter()
            .any(|t| matches!(t.value, Type::Optional(_)))
    );
}

#[test]
fn pratt_precedence_and_associativity_have_expected_tree_shape() {
    let result = document("fn calc() -> Int = 1 + 2 * 3 - 4; fn optional() -> Int = a ?? b ?? c;");
    assert!(result.diagnostics.is_empty());
    let Declaration::Function { body, .. } = result.declarations[0].value else {
        panic!("function")
    };
    let Expr::Binary {
        ref operator,
        left,
        right,
    } = result.expressions[body.0].value
    else {
        panic!("binary")
    };
    assert_eq!(operator.value, "-");
    assert_eq!(
        result.expressions[right.0].value,
        Expr::Literal(Literal::Number("4".into()))
    );
    let Expr::Binary {
        ref operator,
        right,
        ..
    } = result.expressions[left.0].value
    else {
        panic!("add")
    };
    assert_eq!(operator.value, "+");
    assert!(
        matches!(&result.expressions[right.0].value, Expr::Binary { operator, .. } if operator.value == "*")
    );
    let Declaration::Function { body, .. } = result.declarations[1].value else {
        panic!("function")
    };
    let Expr::Binary { right, .. } = result.expressions[body.0].value else {
        panic!("coalesce")
    };
    assert!(
        matches!(&result.expressions[right.0].value, Expr::Binary { operator, .. } if operator.value == "??")
    );
}

#[test]
fn parser_recovers_valid_declarations_and_reports_header_errors() {
    let result = document(
        "fn bad(x: ) -> Int = 1; fn good() -> Int = 2; fn broken() -> Int = { let a = ; fn later() -> Bool = true;",
    );
    assert_eq!(result.diagnostics.len(), 2, "{:?}", result.diagnostics);
    assert_eq!(result.declarations.len(), 4);
    assert!(
        matches!(&result.declarations[1].value, Declaration::Function { name, .. } if name.value == "good")
    );
    assert!(
        matches!(&result.declarations[3].value, Declaration::Function { name, .. } if name.value == "later")
    );
    let missing = parse(source(
        "package retained version \"1\"; fn okay() -> Int = 1;",
    ));
    assert_eq!(missing.module.unwrap().value.name[0].value, "retained");
    assert_eq!(missing.diagnostics.len(), 1);
    for text in [
        "rcore_behavior 3; package m version \"1\";",
        "rcore 1; package if version \"1\";",
        "rcore 1; package m version \"1\"; rcore 1;",
    ] {
        assert!(!parse(source(text)).diagnostics.is_empty());
    }
}

#[test]
fn nesting_limit_recovers_and_long_flat_expressions_do_not_recurse_on_drop() {
    let text = format!(
        "fn deep() -> Int = {}1{}; fn after() -> Int = 0;",
        "(".repeat(200),
        ")".repeat(200)
    );
    let result = document(&text);
    assert!(
        result
            .diagnostics
            .iter()
            .any(|d| d.message.contains("nesting"))
    );
    assert!(
        result.declarations.iter().any(
            |d| matches!(&d.value, Declaration::Function { name, .. } if name.value == "after")
        )
    );
    let flat = format!("fn long() -> Int = {}1;", "1 + ".repeat(10_000));
    let result = document(&flat);
    assert!(result.diagnostics.is_empty());
    assert_eq!(result.expressions.len(), 20_001);
}

#[test]
fn truncated_documents_and_malformed_arguments_terminate_with_valid_spans() {
    let text = include_str!("../../../../fixtures/library/library-v2-behavior.rcore");
    for end in text
        .char_indices()
        .map(|(i, _)| i)
        .chain(std::iter::once(text.len()))
    {
        let result = parse(source(&text[..end]));
        for diagnostic in result.diagnostics {
            assert!(diagnostic.span.start <= diagnostic.span.end);
            assert!(diagnostic.span.end <= end);
            assert!(text.is_char_boundary(diagnostic.span.start));
            assert!(text.is_char_boundary(diagnostic.span.end));
        }
    }
    for body in [
        "fn bad() -> List<> = [];",
        "fn bad() -> Int = call::<>();",
        "fn bad() -> Int = match a { -name => 0 };",
        "fn if() -> Int = 1;",
    ] {
        let result = document(&format!("{body} fn recovered() -> Int = 2;"));
        assert!(!result.diagnostics.is_empty(), "{body}");
        assert!(result.declarations.iter().any(
            |d| matches!(&d.value, Declaration::Function { name, .. } if name.value == "recovered")
        ));
    }
}

#[test]
fn every_binary_operator_and_depth_limit_boundary_are_explicit() {
    for operator in [
        "??", "||", "&&", "==", "!=", "<", ">", "<=", ">=", "+", "-", "*", "/", "%",
    ] {
        let parsed = document(&format!("fn f() -> Int = a {operator} b;"));
        assert!(parsed.diagnostics.is_empty(), "{operator}");
        assert!(parsed.expressions.iter().any(|e| matches!(&e.value, Expr::Binary { operator: actual, .. } if actual.value == operator)));
    }
    for (depth, valid) in [(63, true), (64, false)] {
        let parsed = document(&format!(
            "fn f() -> Int = {}1{};",
            "(".repeat(depth),
            ")".repeat(depth)
        ));
        assert_eq!(parsed.diagnostics.is_empty(), valid);
    }
}

#[test]
fn complete_library_source_tokenizes_without_losing_retained_text() {
    let text = include_str!("../../../../fixtures/library/library-v2-behavior.rcore");
    let result = lex(source(text));
    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    assert_eq!(result.source.text, text);
    assert!(
        result
            .tokens
            .iter()
            .any(|t| t.kind == TokenKind::Symbol("::".into()))
    );
}

#[test]
fn tokens_preserve_exact_numbers_unicode_names_and_original_byte_spans() {
    let text = "/* nested /* café */ comment */ rcore 1; `名``称` \"café\\uD83D\\uDE00\" -123456789012345678901234567890 1.25e-300 ?. ?? :: -> =>";
    let result = lex(source(text));
    assert!(result.diagnostics.is_empty());
    assert_eq!(result.tokens[0].kind, TokenKind::Word("rcore".into()));
    assert_eq!(result.tokens[3].kind, TokenKind::QuotedName("名`称".into()));
    assert_eq!(result.tokens[4].kind, TokenKind::String("café😀".into()));
    assert_eq!(result.tokens[5].kind, TokenKind::Symbol("-".into()));
    assert_eq!(
        result.tokens[6].kind,
        TokenKind::Number("123456789012345678901234567890".into())
    );
    assert_eq!(result.tokens[7].kind, TokenKind::Number("1.25e-300".into()));
    let token = &result.tokens[3];
    assert_eq!(&text[token.span.start..token.span.end], "`名``称`");
    for token in &result.tokens {
        assert_eq!(token.span.source, result.source.id);
        assert!(text.is_char_boundary(token.span.start));
        assert!(text.is_char_boundary(token.span.end));
    }
    assert_eq!(result.tokens.last().unwrap().span.start, text.len());
    assert_eq!(result.tokens.last().unwrap().kind, TokenKind::End);
}

#[test]
fn lexical_recovery_keeps_following_tokens_and_locates_independent_errors() {
    let text = "\"bad\\q\" @ 1e+ \"unterminated\nfn recovered `unfinished\nfn after `` /* open";
    let result = lex(source(text));
    assert_eq!(result.diagnostics.len(), 7);
    assert!(
        result
            .tokens
            .iter()
            .any(|t| t.kind == TokenKind::Word("recovered".into()))
    );
    assert!(
        result
            .tokens
            .iter()
            .any(|t| t.kind == TokenKind::Word("after".into()))
    );
    let slices: Vec<_> = result
        .diagnostics
        .iter()
        .map(|d| &text[d.span.start..d.span.end])
        .collect();
    assert_eq!(
        slices,
        [
            "\"bad\\q\"",
            "@",
            "1e+",
            "\"unterminated",
            "`unfinished",
            "``",
            "/* open"
        ]
    );
}

#[test]
fn source_kind_digest_and_empty_input_are_explicit() {
    let empty = lex(source(""));
    assert!(empty.diagnostics.is_empty());
    assert_eq!(empty.tokens.len(), 1);
    let mut bad = source("rcore 1;");
    bad.text.push(' ');
    assert!(lex(bad).diagnostics[0].message.contains("digest mismatch"));
    let wrong = DefinitionSource::new("file:///wrong.ecore", SourceKind::Ecore, "rcore 1;");
    assert!(
        lex(wrong).diagnostics[0]
            .message
            .contains("Expected a Rcore")
    );
}

#[test]
fn comments_are_iterative_and_every_short_input_terminates() {
    let deep = format!("{}{}fn", "/*".repeat(10_000), "*/".repeat(10_000));
    let result = lex(source(&deep));
    assert!(result.diagnostics.is_empty());
    assert_eq!(result.tokens[0].kind, TokenKind::Word("fn".into()));
    for a in ['"', '`', '/', '*', '\\', '\n', '😀', '1', 'e', '+'] {
        for b in ['"', '`', '/', '*', '\\', '\n', '😀', '1', 'e', '+'] {
            let input = format!("{a}{b}");
            let result = lex(source(&input));
            assert_eq!(result.tokens.last().unwrap().kind, TokenKind::End);
            assert_eq!(result.tokens.last().unwrap().span.end, input.len());
        }
    }
}
