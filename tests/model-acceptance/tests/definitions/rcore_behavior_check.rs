use intersect_model::definitions::{
    bundle::{DefinitionBundle, DefinitionSource, SourceKind},
    rcore_behavior::{
        SyntaxDiagnostic,
        checking::{TypeReport, check},
    },
};
fn run(body: &str) -> Result<TypeReport, Vec<SyntaxDiagnostic>> {
    let source = DefinitionSource::new(
        "memory:body",
        SourceKind::RcoreBehavior,
        format!("rcore 1; package body version \"1\"; {body}"),
    );
    check(
        &DefinitionBundle {
            version: 1,
            sources: vec![source.clone()],
            namespaces: vec![],
        },
        &source.id,
    )
}
fn good(body: &str) {
    assert!(run(body).is_ok(), "{body}: {:?}", run(body).unwrap_err());
}
fn bad(body: &str, expected: &str) {
    let errors = run(body).unwrap_err();
    assert!(
        errors.iter().any(|e| e.message.contains(expected)),
        "{body}: {errors:?}"
    );
    assert!(errors.iter().all(|e| e.span.start < e.span.end));
}

#[test]
fn bodies_check_scalar_context_calls_operators_and_recursive_functions() {
    good("fn unit() -> Unit = (); fn bytes(x: Bytes) -> Bytes = x;");
    good("fn min() -> Int8 = -(128); fn grouped() -> Int8 = -((128));");
    good(
        "fn min() -> Int8 = -128; fn max() -> UInt64 = 18446744073709551615; fn f(x: Int8) -> Int8 = x + 1; fn g(x: Int8) -> Bool = 1 < x; fn even(x: Int) -> Bool = if x == 0 { true } else { odd(x - 1) }; fn odd(x: Int) -> Bool = if x == 0 { false } else { even(x - 1) };",
    );
    // String `+` is concatenation since 6aaa2047 (conjugated-port names in
    // Rcore); other arithmetic on String stays rejected below.
    good("fn f() -> String = \"a\" + \"b\";");
    for (body, message) in [
        ("fn f() -> Int = true;", "Type mismatch"),
        ("fn f() -> Int8 = 128;", "outside Int8"),
        ("fn f() -> Int8 = -129;", "outside Int8"),
        ("fn f() -> UInt8 = -1;", "Negative"),
        ("fn f() -> Int = 0.5;", "Fractional"),
        ("fn f() -> Bool = 1 && 2;", "Type mismatch"),
        ("fn f() -> String = \"a\" - \"b\";", "incompatible scalar"),
        ("fn f() -> String = \"a\" * \"b\";", "incompatible scalar"),
        ("fn f(x: Int8) -> Int = x;", "Type mismatch"),
        ("fn f(x: Int) -> Int = f();", "requires 1 arguments"),
        ("fn f(x: Int) -> Int = x(1);", "not a function"),
        ("fn f() -> Int = if 1 { 2 } else { 3 };", "Type mismatch"),
    ] {
        bad(body, message);
    }
}

#[test]
fn model_bodies_check_subtypes_safe_access_and_inherited_feature_multiplicity() {
    let model = DefinitionSource::new(
        "memory:model.ecore",
        SourceKind::Ecore,
        r##"<e:EPackage xmlns:e="http://www.eclipse.org/emf/2002/Ecore" xmlns:x="http://www.w3.org/2001/XMLSchema-instance" name="test" nsURI="urn:test"><eClassifiers x:type="e:EClass" name="Base"><eStructuralFeatures x:type="e:EAttribute" name="name" lowerBound="1" eType="e:EDataType http://www.eclipse.org/emf/2002/Ecore#//EString"/><eStructuralFeatures x:type="e:EReference" name="parent" eType="#//Base"/><eStructuralFeatures x:type="e:EReference" name="children" eType="#//Base" upperBound="-1"/></eClassifiers><eClassifiers x:type="e:EClass" name="Child" eSuperTypes="#//Base"/></e:EPackage>"##,
    );
    let (model, _) = intersect_model_acceptance::migration_fixture::migrate_source_fixture(model);
    for (body, expected_error) in [
        (
            "fn up(x: m.Child) -> m.Base = x; fn name(x: m.Child) -> String = x.name; fn safe(x: m.Child?) -> String? = x?.name; fn parent(x: m.Child?) -> m.Base? = x?.parent;",
            None,
        ),
        ("fn down(x: m.Base) -> m.Child = x;", Some("Type mismatch")),
        (
            "fn missing(x: m.Child) -> String = x.missing;",
            Some("Unknown feature"),
        ),
        (
            "fn unsafe(x: m.Child?) -> String = x.name;",
            Some("optional values need safe"),
        ),
        (
            "fn unsafe(x: m.Child) -> String? = x?.name;",
            Some("Safe access requires"),
        ),
        (
            "fn wrong(x: m.Child) -> m.Base = x.parent;",
            Some("Type mismatch"),
        ),
        (
            "fn wrong(x: m.Child) -> m.Base? = x.children;",
            Some("Type mismatch"),
        ),
    ] {
        let mut source = DefinitionSource::new(
            "memory:body",
            SourceKind::RcoreBehavior,
            format!("rcore 1; package body version \"1\"; import model \"urn:test\" as m; {body}"),
        );
        source.dependencies.push(model.id.clone());
        let result = check(
            &DefinitionBundle {
                version: 1,
                sources: vec![source.clone(), model.clone()],
                namespaces: vec![],
            },
            &source.id,
        );
        if let Some(expected) = expected_error {
            let errors = result.unwrap_err();
            assert!(
                errors.iter().any(|e| e.message.contains(expected)),
                "{errors:?}"
            );
        } else {
            let report = result.unwrap();
            assert_eq!(report.modules[0].members.len(), 3);
            assert!(
                report.modules[0]
                    .members
                    .values()
                    .all(|f| f.declaration.source == model.id)
            );
        }
    }
}

#[test]
fn imported_calls_use_exact_signatures_and_errors_keep_the_helper_source() {
    let helper = DefinitionSource::new(
        "memory:helper",
        SourceKind::RcoreBehavior,
        "rcore 1; package helper version \"1\"; pub fn identity(x: Int8) -> Int8 = x;",
    );
    let mut entry = DefinitionSource::new(
        "memory:entry",
        SourceKind::RcoreBehavior,
        "rcore 1; package entry version \"1\"; import package helper version \"1\" as h; fn run() -> Int8 = h.identity(127);",
    );
    entry.dependencies.push(helper.id.clone());
    let mut bundle = DefinitionBundle {
        version: 1,
        sources: vec![entry.clone(), helper.clone()],
        namespaces: vec![],
    };
    let first = check(&bundle, &entry.id).unwrap();
    bundle.sources.reverse();
    assert_eq!(
        serde_json::to_value(&first).unwrap(),
        serde_json::to_value(check(&bundle, &entry.id).unwrap()).unwrap()
    );
    let broken = DefinitionSource::new(
        "memory:helper",
        SourceKind::RcoreBehavior,
        helper.text.replace("= x;", "= true;"),
    );
    entry.dependencies = vec![broken.id.clone()];
    let errors = check(
        &DefinitionBundle {
            version: 1,
            sources: vec![entry.clone(), broken.clone()],
            namespaces: vec![],
        },
        &entry.id,
    )
    .unwrap_err();
    assert!(
        errors
            .iter()
            .any(|e| e.span.source == broken.id && e.message.contains("Type mismatch"))
    );
}

#[test]
fn sequential_lets_shadowing_lambda_capture_and_function_variance_are_checked() {
    bad(
        "fn f() -> Bool = { let x = Some(|n: Int| n); x == x };",
        "Equality for functions",
    );
    good(
        "fn f(x: Int) -> Int = { let x = x + 1; let f: (Int) -> Int = |y: Int| x + y; f(2) }; fn apply(f: (Int) -> Bool, x: Int) -> Bool = f(x);",
    );
    bad(
        "fn f(x: Int) -> Int = { let x: Bool = x; 0 };",
        "Type mismatch",
    );
    bad(
        "fn f() -> Int = { let f: (Int) -> Bool = |x: Int| x; 0 };",
        "Type mismatch",
    );
    bad(
        "fn f() -> Int = { let callback = |x: Int| x; callback(true) };",
        "Type mismatch",
    );
}

#[test]
fn inferred_compound_branches_and_unused_model_parameters_are_fully_checked() {
    let rcore = r#"rcore 1; package test version "1"; metamodel "urn:test" prefix test { class Root {} class Left extends Root {} class Right extends Root {} }"#;
    for (model_text, body, expected_error) in [
        (
            rcore.to_owned(),
            "fn f(b: Bool, l: m.Left, r: m.Right) -> m.Root? = { let chosen = if b { Some(l) } else { Some(r) }; chosen }; fn g(b: Bool, l: Result<m.Left, String>, r: Result<m.Right, String>) -> Result<m.Root, String> = { let chosen = match b { true => l, false => r }; chosen };",
            None,
        ),
        (
            rcore.replace("class Root {}", "class Root extends Left {}"),
            "fn f(unused: m.Left) -> Int = 0;",
            Some("Cyclic"),
        ),
    ] {
        let model = DefinitionSource::new("memory:model.rcore", SourceKind::Rcore, model_text);
        let mut source = DefinitionSource::new(
            "memory:body",
            SourceKind::RcoreBehavior,
            format!("rcore 1; package body version \"1\"; import model \"urn:test\" as m; {body}"),
        );
        source.dependencies.push(model.id.clone());
        let result = check(
            &DefinitionBundle {
                version: 1,
                sources: vec![source.clone(), model],
                namespaces: vec![],
            },
            &source.id,
        );
        if let Some(expected) = expected_error {
            assert!(
                result
                    .unwrap_err()
                    .iter()
                    .any(|e| e.message.contains(expected))
            );
        } else {
            result.unwrap();
        }
    }
}

#[test]
fn complete_scalar_operator_inventory_checks_inputs_and_results() {
    for scalar in [
        "Int8", "Int16", "Int32", "Int64", "UInt8", "UInt16", "UInt32", "UInt64", "BigInt",
        "Decimal", "Float32", "Float64",
    ] {
        for operator in ["+", "-", "*", "/", "%"] {
            good(&format!(
                "fn f(a: {scalar}, b: {scalar}) -> {scalar} = a {operator} b;"
            ));
        }
        for operator in ["<", "<=", ">", ">=", "==", "!="] {
            good(&format!(
                "fn f(a: {scalar}, b: {scalar}) -> Bool = a {operator} b;"
            ));
        }
        for operator in ["+", "-"] {
            good(&format!("fn f(a: {scalar}) -> {scalar} = {operator}a;"));
        }
    }
    for operator in ["&&", "||", "==", "!="] {
        good(&format!("fn f(a: Bool, b: Bool) -> Bool = a {operator} b;"));
    }
    good(
        "fn f(x: Bool) -> Bool = !x; fn text(a: String, b: String) -> Bool = a == b; fn bytes(a: Bytes, b: Bytes) -> Bool = a != b; fn unit(a: Unit) -> Bool = a == ();",
    );
    for body in [
        "fn f(a: Int, b: Int32) -> Int = a + b;",
        "fn f(a: Decimal, b: Float64) -> Bool = a < b;",
        "fn f(a: Bool) -> Bool = -a;",
        "fn f(a: Int) -> Bool = !a;",
        "fn f(a: String, b: String) -> Bool = a < b;",
    ] {
        assert!(run(body).is_err(), "{body}");
    }
}

#[test]
fn namespaces_and_static_feature_tokens_are_not_runtime_values() {
    let model = DefinitionSource::new(
        "memory:model.ecore",
        SourceKind::Ecore,
        include_str!("../../../../fixtures/library/library.ecore"),
    );
    let (model, _) = intersect_model_acceptance::migration_fixture::migrate_source_fixture(model);
    for value in ["lib", "lib.BookCopy", "lib.BookCopy.title"] {
        let mut source = DefinitionSource::new(
            "memory:bad",
            SourceKind::RcoreBehavior,
            format!(
                "rcore 1; package bad version \"1\"; import model \"urn:intersect:example:library:1\" as lib; fn f() -> String = {value};"
            ),
        );
        source.dependencies.push(model.id.clone());
        let result = check(
            &DefinitionBundle {
                version: 1,
                sources: vec![source.clone(), model.clone()],
                namespaces: vec![],
            },
            &source.id,
        );
        assert!(
            result
                .unwrap_err()
                .iter()
                .any(|error| error.message.contains("runtime value"))
        );
    }
}

#[test]
fn optional_and_result_constructors_preserve_absence_and_error_types() {
    good(
        "fn a() -> Int? = none; fn b() -> Int? = None; fn c() -> Int? = Some(1); fn d() -> Result<Int, String> = Ok(1); fn e() -> Result<Int, String> = Err(\"error\"); fn f(x: Int?) -> Int = x ?? 0; fn g() -> Int = { let x = Some(3); x ?? 0 }; fn h() -> Int? = None::<Int>(); fn i() -> Result<Int, String> = Ok::<Int, String>(1);",
    );
    for (body, message) in [
        ("fn f() -> Int = none;", "expected Option"),
        ("fn f() -> Int = Some(1);", "incompatible expected"),
        ("fn f() -> Int? = 1;", "Type mismatch"),
        ("fn f(x: Int?) -> Int = x;", "Type mismatch"),
        ("fn f(x: Result<Int, String>) -> Int = x;", "Type mismatch"),
        ("fn f() -> Int = { let x = Ok(1); 0 };", "expected type"),
        ("fn f() -> Int = { let x = none; 0 };", "expected Option"),
        ("fn f() -> Int? = Some();", "requires 1"),
        ("fn f() -> Int? = None(1);", "requires 0"),
        ("fn f() -> Result<Int, String> = Err(1);", "Type mismatch"),
        ("fn f(x: Int) -> Int = x ?? 0;", "Coalesce requires"),
    ] {
        bad(body, message);
    }
}

#[test]
fn long_flat_bodies_use_iterative_traversal_and_incomplete_rules_reject() {
    good(&format!(
        "fn f() -> Int = {};",
        vec!["1"; 10_000].join(" + ")
    ));
    good("fn f(x: Bool) -> Int = match x { true => 1, false => 0 };");
    good("fn f() -> Int = { let xs = [1, 2]; 0 };");
    bad(
        "fn f() -> Int = { let x = if true { 1 } else { false }; 0 };",
        "incompatible types",
    );
}
