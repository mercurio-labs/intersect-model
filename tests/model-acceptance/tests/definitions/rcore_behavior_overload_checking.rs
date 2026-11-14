//! Independent source acceptance for static overload sets and exact selection.
use intersect_model::definitions::{
    bundle::{DefinitionBundle, DefinitionSource, SourceKind},
    rcore_behavior::{
        checking::{TypeReport, check},
        types::{Scalar, Type},
    },
};
fn source(name: &str, body: &str) -> DefinitionSource {
    DefinitionSource::new(
        format!("memory:{name}-behavior.rcore"),
        SourceKind::RcoreBehavior,
        format!("rcore 1;package {name} version \"1\";{body}"),
    )
}
fn bundle(sources: Vec<DefinitionSource>) -> DefinitionBundle {
    DefinitionBundle {
        version: 1,
        sources,
        namespaces: vec![],
    }
}
fn run(body: &str) -> TypeReport {
    let s = source("overloads", body);
    check(&bundle(vec![s.clone()]), &s.id).unwrap_or_else(|e| panic!("{body}\n{e:?}"))
}
fn rejects(body: &str, word: &str) {
    let s = source("overloads", body);
    let e = check(&bundle(vec![s.clone()]), &s.id).unwrap_err();
    assert!(
        e.iter().any(|e| e.message.to_lowercase().contains(word)),
        "{body}\n{e:?}"
    );
    assert!(e.iter().all(|e| e.span.source == s.id));
}

#[test]
fn unique_scalar_arity_and_return_context_selections_retain_exact_evidence() {
    let r = run(
        "fn main()->Int8={let x:Int8=3;pick(x)+pick(x,true)+result()};fn pick(x:Bool)->Int8=9;fn pick(x:Int8)->Int8=x;fn pick(x:Int8,flag:Bool)->Int8=x;fn result()->Bool=true;fn result()->Int8=1;",
    );
    let selections: Vec<_> = r.modules[0].function_instantiations.values().collect();
    for wanted in [2, 3, 5] {
        let s = selections
            .iter()
            .find(|s| s.symbol.declaration == wanted)
            .unwrap();
        assert!(s.arguments.is_empty());
        assert!(matches!(
            r.signatures.types.get(s.function_type).unwrap(),
            Type::Function { .. }
        ));
    }
    assert!(
        !selections
            .iter()
            .any(|s| [1, 4].contains(&s.symbol.declaration))
    );
}

#[test]
fn genuine_overlap_and_uncontextualized_values_reject_without_priorities() {
    for body in [
        "fn main()->Int8=pick(1);fn pick(x:Int8)->Int8=1;fn pick(x:Int16)->Int8=2;",
        "fn main()->Int8={let x:Int8=1;pick(x)};fn pick<T>(x:T)->T=x;fn pick(x:Int8)->Int8=x;",
        "fn main()->Unit={let f=pick;()};fn pick(x:Int8)->Int8=x;fn pick(x:Bool)->Bool=x;",
        "fn main()->Unit={let x=pick();()};fn pick()->Int8=1;fn pick()->Bool=true;",
        "fn main()->Int8=pick::<Int8>(1);fn pick<T>(x:T)->T=x;fn pick<U:Int8>(x:U)->U=x;",
    ] {
        rejects(body, "ambig");
    }
    rejects(
        "fn main()->Int8=pick(\"bad\");fn pick(x:Int8)->Int8=x;fn pick(x:Bool)->Int8=1;",
        "applicable",
    );
}

#[test]
fn explicit_generic_filter_and_failed_candidate_inference_do_not_pollute_winner() {
    let r = run(
        "fn main()->Int8={let x:Int8=4;pick::<Int8,Bool>(x)+apply(identity,x)};fn pick<T,U>(x:T)->T=x;fn pick(x:Bool)->Bool=x;fn apply<T>(f:(T)->T,x:T)->T=f(x);fn identity(x:Bool)->Bool=x;fn identity<T:Int8>(x:T)->T=x;",
    );
    let m = &r.modules[0];
    let phantom = m
        .function_instantiations
        .values()
        .find(|e| e.symbol.declaration == 1)
        .unwrap();
    assert_eq!(phantom.arguments.len(), 2);
    assert_eq!(
        r.signatures.types.get(phantom.arguments[1]).unwrap(),
        &Type::Scalar(Scalar::Bool)
    );
    assert!(
        m.function_instantiations
            .values()
            .any(|e| e.symbol.declaration == 5)
    );
    assert!(
        !m.function_instantiations
            .values()
            .any(|e| e.symbol.declaration == 4)
    );
    rejects(
        "fn main()->Int8=pick::<Bool,Int8>(true);fn pick<T:Int8,U>(x:T)->T=x;fn pick(x:Bool)->Bool=x;",
        "applicable",
    );
}

#[test]
fn imported_public_set_contextual_grouping_and_shadowing_survive_source_replay() {
    let helper = source(
        "helper",
        "pub fn pick(x:Int8)->Int8=x;pub fn pick(x:Bool)->Bool=x;fn pick(x:Int16)->Int8=1;",
    );
    let mut entry = source(
        "entry",
        "import package helper version \"1\" as h;fn main()->Int8={let selected:(Int8)->Int8=((h.pick));let pick:(Int8)->Int8=|x:Int8|x;pick(selected(7))};",
    );
    entry.dependencies.push(helper.id.clone());
    let mut b = bundle(vec![entry.clone(), helper.clone()]);
    let a = check(&b, &entry.id).unwrap();
    let m = a.modules.iter().find(|m| m.source == entry.id).unwrap();
    let e = m
        .function_instantiations
        .values()
        .find(|e| e.symbol.source == helper.id)
        .unwrap();
    assert_eq!(e.symbol.declaration, 0);
    assert!(e.arguments.is_empty());
    b.sources.reverse();
    let rebuilt: DefinitionBundle =
        serde_json::from_value(serde_json::to_value(&b).unwrap()).unwrap();
    assert_eq!(
        serde_json::to_value(&a).unwrap(),
        serde_json::to_value(check(&rebuilt, &entry.id).unwrap()).unwrap()
    );
    let mut stale = rebuilt;
    stale
        .sources
        .iter_mut()
        .find(|s| s.id == helper.id)
        .unwrap()
        .text
        .push(' ');
    assert!(check(&stale, &entry.id).is_err());
}

#[test]
fn private_candidates_do_not_participate_and_private_only_names_reject() {
    let helper = source(
        "helper",
        "pub fn pick(x:Int8)->Int8=x;fn pick(x:Int16)->Int8=2;fn hidden(x:Int8)->Int8=x;",
    );
    let mut entry = source(
        "entry",
        "import package helper version \"1\" as h;fn main()->Int8=h.pick(1);",
    );
    entry.dependencies.push(helper.id.clone());
    assert!(check(&bundle(vec![entry.clone(), helper.clone()]), &entry.id).is_ok());
    let mut bad = source(
        "entry",
        "import package helper version \"1\" as h;fn main()->Int8=h.hidden(1);",
    );
    bad.dependencies.push(helper.id.clone());
    let errors = check(&bundle(vec![bad.clone(), helper]), &bad.id).unwrap_err();
    assert!(
        errors
            .iter()
            .any(|e| e.message.to_lowercase().contains("private")),
        "{errors:?}"
    );
}

#[test]
fn every_overload_body_and_alpha_equivalent_duplicate_signature_is_checked() {
    let invalid = source(
        "invalid",
        "fn main()->Int8=pick(1);fn pick(x:Int8)->Int8=x;fn pick(x:Bool)->Bool=1;",
    );
    let errors = check(&bundle(vec![invalid.clone()]), &invalid.id).unwrap_err();
    let body = invalid.text.rfind("=1;").unwrap() + 1;
    assert!(
        errors
            .iter()
            .any(|e| e.span.start <= body && e.span.end > body),
        "{errors:?}"
    );
    rejects(
        "fn main()->Int8=1;fn duplicate<T>(x:T)->T=x;fn duplicate<U>(x:U)->U=x;",
        "duplicate",
    );
    for body in [
        "fn main()->Int8=pick(1);fn pick(x:Int8)->Int8=1;fn pick(x:Int16)->Int8=2;",
        "fn main()->Int8=pick(1);fn pick(x:Int16)->Int8=2;fn pick(x:Int8)->Int8=1;",
    ] {
        rejects(body, "ambig");
    }
}

#[test]
fn nominal_overlaps_remain_ambiguous_while_contextual_variance_selects_uniquely() {
    let model = DefinitionSource::new(
        "memory:overload.ecore",
        SourceKind::Ecore,
        r##"<e:EPackage xmlns:e="http://www.eclipse.org/emf/2002/Ecore" xmlns:x="http://www.w3.org/2001/XMLSchema-instance" name="m" nsURI="urn:overload"><eClassifiers x:type="e:EClass" name="Base"/><eClassifiers x:type="e:EClass" name="Child" eSuperTypes="#//Base"/></e:EPackage>"##,
    );
    let (model, _) = intersect_model_acceptance::migration_fixture::migrate_source_fixture(model);
    for (body, valid) in [
        (
            "fn caller(x:m.Child)->Int8=pick(x);fn pick(x:m.Base)->Int8=1;fn pick(x:m.Child)->Int8=2;",
            false,
        ),
        (
            "fn caller(x:m.Child)->m.Base={let f:(m.Child)->m.Base=pick;f(x)};fn pick(x:m.Child)->m.Child=x;fn pick(x:Bool)->Bool=x;",
            true,
        ),
        (
            "fn caller(x:m.Base)->m.Child={let f:(m.Base)->m.Child=pick;f(x)};fn pick(x:m.Child)->m.Child=x;fn pick(x:Bool)->Bool=x;",
            false,
        ),
    ] {
        let mut s = source(
            "nominal",
            &format!("import model \"urn:overload\" as m;{body}"),
        );
        s.dependencies.push(model.id.clone());
        let result = check(&bundle(vec![s.clone(), model.clone()]), &s.id);
        if valid {
            assert!(result.is_ok(), "{result:?}")
        } else {
            assert!(result.is_err(), "{body}")
        }
    }
}

#[test]
fn overload_candidate_limit_is_deterministic_and_fresh_request_recovers() {
    fn body(count: usize) -> String {
        let mut text = String::from("fn main()->Int8=pick();");
        for arity in 0..count {
            let parameters = (0..arity)
                .map(|i| format!("x{i}:Int8"))
                .collect::<Vec<_>>()
                .join(",");
            text.push_str(&format!("fn pick({parameters})->Int8=1;"));
        }
        text
    }
    run(&body(256));
    let s = source("wide", &body(257));
    let b = bundle(vec![s.clone()]);
    let first = check(&b, &s.id).unwrap_err();
    assert!(
        first
            .iter()
            .any(|e| e.message == "Overload candidate limit exceeded (256)"),
        "{first:?}"
    );
    assert_eq!(
        serde_json::to_value(&first).unwrap(),
        serde_json::to_value(check(&b, &s.id).unwrap_err()).unwrap()
    );
    run(&body(256));
}

#[test]
fn aggregate_snapshot_budget_is_fatal_even_after_an_applicable_first_candidate() {
    fn body(count: usize) -> String {
        let mut text = String::from("fn main()->Int8=pick({v0:1});");
        for i in 0..count {
            text.push_str(&format!("fn pick(x:{{v{i}:Int8}})->Int8=x.v{i};"));
        }
        text
    }
    run(&body(2));
    let s = source("snapshots", &body(256));
    let b = bundle(vec![s.clone()]);
    let first = check(&b, &s.id).unwrap_err();
    assert!(
        first
            .iter()
            .any(|e| e.message == "Overload snapshot byte limit exceeded"),
        "{first:?}"
    );
    assert_eq!(
        serde_json::to_value(&first).unwrap(),
        serde_json::to_value(check(&b, &s.id).unwrap_err()).unwrap()
    );
    run(&body(2));
}
#[test]
fn literal_range_mismatch_eliminates_only_the_inapplicable_candidate() {
    let report = run("fn main()->Int8=pick(200);fn pick(x:Int8)->Int8=1;fn pick(x:Int16)->Int8=2;");
    let selections = &report.modules[0].function_instantiations;
    assert!(selections.values().any(|s| s.symbol.declaration == 2));
    assert!(!selections.values().any(|s| s.symbol.declaration == 1));
}

#[test]
fn unsupported_candidate_body_is_not_downgraded_to_no_applicable_overload() {
    for candidates in [
        "fn choose(f:(Bool)->Bool)->Bool=f(true);fn choose(f:(Sequence<Int8>)->Bool)->Bool=f([]);",
        "fn choose(f:(Sequence<Int8>)->Bool)->Bool=f([]);fn choose(f:(Bool)->Bool)->Bool=f(true);",
    ] {
        rejects(
            &format!("fn main()->Bool=choose(|x:Sequence<Int8>|x==x);{candidates}"),
            "unsupported",
        );
    }
}

#[test]
fn user_type_field_names_cannot_turn_candidate_mismatch_into_fatal_resource_error() {
    for field in ["budget", "unsupported", "exceeds"] {
        for reverse in [false, true] {
            let record = format!("fn pick(x:{{{field}:Int8}})->Int8=1;");
            let boolean = "fn pick(x:Bool)->Int8=2;";
            let candidates = if reverse {
                format!("{boolean}{record}")
            } else {
                format!("{record}{boolean}")
            };
            let report = run(&format!("fn main(x:Bool)->Int8=pick(x);{candidates}"));
            let selected = if reverse { 1 } else { 2 };
            assert!(
                report.modules[0]
                    .function_instantiations
                    .values()
                    .any(|s| s.symbol.declaration == selected)
            );
            assert!(
                !report.modules[0]
                    .function_instantiations
                    .values()
                    .any(|s| s.symbol.declaration == 3 - selected)
            );
        }
    }
}
