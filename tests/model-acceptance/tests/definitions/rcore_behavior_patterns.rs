use intersect_model::definitions::{
    bundle::{DefinitionBundle, DefinitionSource, SourceKind},
    rcore_behavior::checking::check,
};

fn check_body(body: &str) -> Result<(), String> {
    let source = DefinitionSource::new(
        "memory:patterns",
        SourceKind::RcoreBehavior,
        format!("rcore 1; package patterns version \"1\"; // 🙂\n{body}"),
    );
    let result = check(
        &DefinitionBundle {
            version: 1,
            sources: vec![source.clone()],
            namespaces: vec![],
        },
        &source.id,
    );
    result.map(|_| ()).map_err(|errors| {
        for error in &errors {
            assert_eq!(error.span.source, source.id);
            assert!(source.text.get(error.span.start..error.span.end).is_some());
            assert!(error.span.start < error.span.end);
        }
        errors
            .iter()
            .map(|error| error.message.as_str())
            .collect::<Vec<_>>()
            .join("; ")
    })
}
fn good(body: &str) {
    check_body(body).unwrap_or_else(|e| panic!("{body}: {e}"));
}
fn bad(body: &str, expected: &str) {
    let error = check_body(body).unwrap_err();
    assert!(error.contains(expected), "{body}: {error}");
}

#[test]
fn finite_and_nested_domains_require_complete_payload_coverage() {
    good("fn f(x: Bool?) -> Int = match x { Some(true) => 1, Some(false) => 2, none => 0 };");
    good(
        "fn f(x: Result<Bool?, String>) -> Int = match x { Ok(Some(true)) => 1, Ok(Some(false)) => 2, Ok(none) => 3, Err(_) => 0 };",
    );
    good(
        "fn f(x: Result<Int, String>) -> Int = match x { Ok(value) => value, Err(msg) => if msg == \"\" { 0 } else { 1 } };",
    );
    good("fn f(x: Unit) -> Int = match x { _ => 0 };");
    for body in [
        "fn f(x: Bool) -> Int = match x { true => 1 };",
        "fn f(x: Bool?) -> Int = match x { Some(true) => 1, none => 0 };",
        "fn f(x: Result<Bool?, String>) -> Int = match x { Ok(Some(_)) => 1, Err(_) => 0 };",
        "fn f(x: Int) -> Int = match x { 0 => 0, 1 => 1 };",
    ] {
        bad(body, "Non-exhaustive");
    }
}

#[test]
fn unreachable_arms_include_nested_payload_unions_and_catchalls() {
    for body in [
        "fn f(x: Bool) -> Int = match x { true => 1, false => 0, _ => 2 };",
        "fn f(x: Bool?) -> Int = match x { Some(true) => 1, Some(false) => 2, Some(_) => 3, none => 0 };",
        "fn f(x: Int) -> Int = match x { all => all, 0 => 1 };",
        "fn f(x: Int?) -> Int = match x { none => 0, None() => 1, Some(n) => n };",
        "fn f(x: String) -> Int = match x { \"a\" => 1, \"a\" => 2, _ => 0 };",
    ] {
        bad(body, "Unreachable");
    }
}

#[test]
fn pattern_payloads_arity_and_literal_domains_are_checked() {
    for (body, message) in [
        (
            "fn f(x: Int?) -> Int = match x { Some() => 1, _ => 0 };",
            "arity",
        ),
        (
            "fn f(x: Int?) -> Int = match x { Some(a, b) => 1, _ => 0 };",
            "arity",
        ),
        (
            "fn f(x: Int?) -> Int = match x { Ok(a) => 1, _ => 0 };",
            "incompatible scrutinee",
        ),
        (
            "fn f(x: Int) -> Int = match x { none => 1, _ => 0 };",
            "requires Option",
        ),
        (
            "fn f(x: Int?) -> Int = match x { Some(true) => 1, none => 0 };",
            "scalar type",
        ),
        (
            "fn f(x: Int8) -> Int = match x { 128 => 1, _ => 0 };",
            "outside Int8",
        ),
        (
            "fn f(x: Int?) -> Bool = match x { Some(n) => n, none => false };",
            "Type mismatch",
        ),
        (
            "fn f(x: Result<Int, String>) -> Int = match x { Ok(n) => n, Err(e) => e };",
            "Type mismatch",
        ),
        (
            "fn Some(x: Int) -> Int = x; fn f(x: Int?) -> Int = match x { Some(n) => 0, _ => 1 };",
            "builtin Option/Result",
        ),
    ] {
        bad(body, message);
    }
}

#[test]
fn equivalent_numeric_patterns_compare_exact_values_not_spellings() {
    for (ty, left, right) in [
        ("Int", "1", "1.00"),
        ("Int8", "-128", "-1.28e2"),
        ("UInt64", "18446744073709551615", "184467440737095516150e-1"),
        ("Decimal", "0.00100", "1e-3"),
        (
            "Decimal",
            "1e1000000000000000000000000000000",
            "10e999999999999999999999999999999",
        ),
        (
            "Decimal",
            "1e-1000000000000000000000000000000",
            "0.1e-999999999999999999999999999999",
        ),
        (
            "BigInt",
            "1e999999999999999999999999999999",
            "10e999999999999999999999999999998",
        ),
        ("Float32", "16777216", "16777217"),
        ("Float64", "0", "-0"),
    ] {
        bad(
            &format!("fn f(x: {ty}) -> Int = match x {{ {left} => 1, {right} => 2, _ => 0 }};"),
            "Unreachable",
        );
    }
    good(
        "fn f(x: Decimal) -> Int = match x { 1e999999999999999999999999999999 => 1, 1e999999999999999999999999999998 => 2, _ => 0 };",
    );
    good(
        "fn f(x: Decimal) -> Int = match x { 1e-999999999999999999999999999999 => 1, 1e-999999999999999999999999999998 => 2, _ => 0 };",
    );
}

#[test]
fn nested_patterns_and_arm_local_identity_do_not_leak_between_arms() {
    let ty = format!("{}Int{}", "Option<".repeat(12), ">".repeat(12));
    let pattern = format!("{}n{}", "Some(".repeat(12), ")".repeat(12));
    good(&format!(
        "fn f(x: {ty}) -> Int = match x {{ {pattern} => n, _ => 0 }};"
    ));
    good(
        "fn f(x: Result<Int, Bool>) -> Int = match x { Ok(n) => n, Err(n) => if n { 1 } else { 0 } };",
    );
    bad(
        "fn f(x: Int?) -> Int = match x { Some(n) => n, none => n };",
        "Unresolved value",
    );
}
