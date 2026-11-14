//! S04 source-level structural typing controls; no runtime execution is asserted.
use intersect_model::definitions::{
    bundle::{DefinitionBundle, DefinitionSource, SourceKind},
    rcore_behavior::{
        SyntaxDiagnostic,
        checking::{TypeReport, check},
    },
};

fn run(body: &str) -> Result<TypeReport, Vec<SyntaxDiagnostic>> {
    let source = DefinitionSource::new(
        "memory:structural-behavior.rcore",
        SourceKind::RcoreBehavior,
        format!("rcore 1; package structural version \"1\"; {body}"),
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
    if let Err(errors) = run(body) {
        panic!("Expected source program to type-check: {body}\n{errors:?}");
    }
}
fn bad(body: &str) {
    let errors = run(body).expect_err("Invalid structural program must not type-check");
    assert!(!errors.is_empty(), "{body}");
    assert!(
        errors.iter().all(|error| error.span.start < error.span.end),
        "{body}: {errors:?}"
    );
    assert!(
        errors
            .iter()
            .all(|error| error.span.source.uri == "memory:structural-behavior.rcore"),
        "{body}: {errors:?}"
    );
}

#[test]
fn contextual_and_inferred_sequence_literals_preserve_nested_element_types() {
    good(
        "fn empty() -> Sequence<Int8> = []; fn limits() -> Sequence<Int8> = [-128, 127]; fn nested() -> Sequence<Sequence<Int8>> = [[], [1, 2]];",
    );
    good(
        "fn inferred() -> Sequence<Int> = { let values = [1, 2, 3]; values }; fn nested() -> Sequence<Sequence<Int>> = { let values = [[1], [2, 3]]; values };",
    );
    good(
        "fn composed() -> Sequence<(Int8, {label: String, flags: Sequence<Bool>})> = [(1, {flags: [true], label: \"a\"})];",
    );
    for body in [
        "fn overflow() -> Sequence<Int8> = [128];",
        "fn mixed() -> Sequence<Int> = [1, true];",
        "fn nested() -> Sequence<Sequence<Int>> = [[1], [false]];",
        "fn uninferred() -> Int = { let values = []; 1 };",
        "fn implicit_set() -> Set<Int> = [1, 2];",
        "fn implicit_ordered_set() -> OrderedSet<Int> = [1, 2];",
        "fn implicit_bag() -> Bag<Int> = [1, 2];",
    ] {
        bad(body);
    }
}

#[test]
fn explicit_collection_conversions_cover_the_complete_kind_matrix() {
    let kinds = ["Sequence", "OrderedSet", "Set", "Bag"];
    for source in kinds {
        for target in kinds {
            let body = format!(
                "fn convertValue(value: {source}<Int>) -> {target}<Int> = {target}::<Int>(value);"
            );
            let unordered_source = matches!(source, "Set" | "Bag");
            let ordered_target = matches!(target, "Sequence" | "OrderedSet");
            if unordered_source && ordered_target {
                bad(&body);
            } else {
                good(&body);
            }
            if source != target {
                bad(&format!(
                    "fn implicit(value: {source}<Int>) -> {target}<Int> = value;"
                ));
            }
        }
    }
    good(
        "fn deduplicate() -> OrderedSet<Int8> = OrderedSet::<Int8>([1, 1, 2]); fn lose_order() -> Bag<Int8> = Bag::<Int8>([1, 2]); fn nested() -> Set<Sequence<Int8>> = Set::<Sequence<Int8>>([[1], []]);",
    );
    // Conversion authorizes changes of collection kind, never element coercion.
    bad("fn scalar() -> Set<Int> = Set::<Int>(1);");
    bad("fn element(values: Sequence<Bool>) -> Set<Int> = Set::<Int>(values);");
    bad("fn overflow() -> Set<Int8> = Set::<Int8>([128]);");
    bad("fn missing() -> Set<Int> = Set::<Int>();");
    bad("fn extra() -> Set<Int> = Set::<Int>([1], [2]);");
}

#[test]
fn tuples_infer_or_consume_context_without_losing_arity_or_positions() {
    good(
        "fn contextual() -> (Int8, String) = (127, \"last\"); fn inferred() -> (Int, Bool) = { let pair = (1, true); pair }; fn one() -> (Int8,) = (1,); fn unit() -> Unit = ();",
    );
    good("fn nested() -> ((Int8, Bool), Sequence<String>) = ((-128, true), [\"x\"]);");
    for body in [
        "fn arity() -> (Int, Bool) = (1,);",
        "fn arity() -> (Int,) = (1, true);",
        "fn order() -> (Int, Bool) = (true, 1);",
        "fn overflow() -> (Int8, Bool) = (128, true);",
        "fn scalar() -> (Int,) = 1;",
        "fn unit() -> (Int,) = ();",
    ] {
        bad(body);
    }
}

#[test]
fn record_literals_and_access_preserve_exact_named_shape() {
    good("fn empty() -> {} = {}; fn inferred() -> {} = { let value = {}; value };");
    good(
        "fn reordered() -> {name: String, count: Int8} = {count: 127, name: \"a\"}; fn inferred() -> String = { let item = {count: 1, name: \"a\"}; item.name }; fn nested(x: {data: {value: Int8}}) -> Int8 = x.data.value;",
    );
    good("fn nested() -> {entries: Sequence<(Int8, Bool)>} = {entries: [(1, true)]};");
    for body in [
        "fn missing() -> {name: String, count: Int} = {name: \"a\"};",
        "fn extra() -> {name: String} = {name: \"a\", count: 1};",
        "fn wrong() -> {count: Int} = {count: true};",
        "fn duplicate() -> {count: Int} = {count: 1, count: 2};",
        "fn duplicate(x: {count: Int, count: Bool}) -> Int = 1;",
        "fn missing(x: {count: Int}) -> Int = x.absent;",
        "fn kind(x: {count: Int}) -> Bool = x.count;",
    ] {
        bad(body);
    }
}

#[test]
fn tuple_indexing_requires_an_in_range_constant_and_returns_the_exact_component() {
    good(
        "fn first(x: (Int8, Bool)) -> Int8 = x[0]; fn last(x: (Int8, Bool)) -> Bool = x[1]; fn singleton(x: (String,)) -> String = x[0]; fn inferred() -> Bool = { let pair = (1, true); pair[1] };",
    );
    for body in [
        "fn past(x: (Int, Bool)) -> Bool = x[2];",
        "fn negative(x: (Int, Bool)) -> Int = x[-1];",
        "fn dynamic(x: (Int, Bool), i: Int) -> Int = x[i];",
        "fn fractional(x: (Int, Bool)) -> Int = x[0.5];",
        "fn kind(x: (Int, Bool)) -> Int = x[true];",
        "fn wrong(x: (Int, Bool)) -> Bool = x[0];",
    ] {
        bad(body);
    }
}

#[test]
fn ordered_collection_indexing_returns_option_and_unordered_kinds_reject() {
    for kind in ["Sequence", "OrderedSet"] {
        good(&format!(
            "fn index(values: {kind}<Int8>, i: Int) -> Int8? = values[i];"
        ));
        bad(&format!(
            "fn unchecked(values: {kind}<Int8>, i: Int) -> Int8 = values[i];"
        ));
        bad(&format!(
            "fn wrong_index(values: {kind}<Int8>) -> Int8? = values[true];"
        ));
    }
    good("fn nested(values: Sequence<Sequence<Int8>>, i: Int) -> Sequence<Int8>? = values[i];");
    for kind in ["Set", "Bag"] {
        bad(&format!(
            "fn unordered(values: {kind}<Int8>) -> Int8? = values[0];"
        ));
    }
    bad("fn scalar(value: Int) -> Int? = value[0];");
}

#[test]
fn collection_intrinsics_require_builtin_binding_and_allow_ordinary_shadowing() {
    good("fn Set(x: Int) -> Bool = true; fn use_name() -> Bool = Set(1);");
    good("fn use_local(Sequence: (Int) -> Bool) -> Bool = Sequence(1);");
    bad("fn Set(x: Int) -> Bool = true; fn bad() -> Set<Int> = Set(1);");
}
