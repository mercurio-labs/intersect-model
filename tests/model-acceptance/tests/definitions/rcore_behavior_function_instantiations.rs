//! Source-owned S06 prerequisite evidence; no execution certificate is accepted.
use intersect_model::definitions::{
    bundle::{DefinitionBundle, DefinitionSource, SourceKind},
    rcore_behavior::{
        ast::Expr,
        checking::{TypeReport, check},
        modules::SymbolId,
        types::{Scalar, Type},
    },
};
fn source(name: &str, body: &str) -> DefinitionSource {
    DefinitionSource::new(
        format!("memory:{name}-behavior.rcore"),
        SourceKind::RcoreBehavior,
        format!("rcore 1; package {name} version \"1\"; {body}"),
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
    let entry = source("main", body);
    check(&bundle(vec![entry.clone()]), &entry.id)
        .unwrap_or_else(|errors| panic!("{body}: {errors:?}"))
}
#[test]
fn explicit_actuals_retain_phantom_formals_in_source_order() {
    let report = run("fn keep<T,U>(x:T)->T=x; fn caller()->Int8=keep::<Int8,Bool>(1);");
    let evidence = report.modules[0]
        .function_instantiations
        .values()
        .next()
        .unwrap();
    assert_eq!(evidence.symbol.declaration, 0);
    assert_eq!(evidence.arguments.len(), 2);
    assert_eq!(
        report.signatures.types.get(evidence.arguments[0]).unwrap(),
        &Type::Scalar(Scalar::Int8)
    );
    assert_eq!(
        report.signatures.types.get(evidence.arguments[1]).unwrap(),
        &Type::Scalar(Scalar::Bool)
    );
    let Type::Function { parameters, result } =
        report.signatures.types.get(evidence.function_type).unwrap()
    else {
        panic!("Expected checked function type")
    };
    assert_eq!(parameters, &vec![evidence.arguments[0]]);
    assert_eq!(*result, evidence.arguments[0]);
}
#[test]
fn inferred_and_grouped_direct_calls_record_actual_callee_expressions() {
    let report = run("fn id<T>(x:T)->T=x; fn caller()->(Int,Int)=(id(1),(id)(2));");
    let module = &report.modules[0];
    let document = &report.signatures.bound.index.modules[0].document;
    assert_eq!(module.function_instantiations.len(), 2);
    assert!(
        module
            .function_instantiations
            .keys()
            .any(|id| matches!(document.expressions[*id].value, Expr::Group(_)))
    );
    for (id, evidence) in &module.function_instantiations {
        assert_eq!(evidence.symbol.declaration, 0);
        assert_eq!(module.expressions[id], evidence.function_type);
        assert_eq!(
            report.signatures.types.get(evidence.arguments[0]).unwrap(),
            &Type::Scalar(Scalar::Int64)
        );
    }
}
#[test]
fn contextual_imported_function_values_and_callbacks_keep_exact_symbols() {
    let helper = source("helper", "pub fn id<T>(x:T)->T=x;");
    let mut entry = source(
        "main",
        "import package helper version \"1\" as h; fn apply<T>(f:(T)->T,x:T)->T=f(x); fn caller()->Int8={let alias:(Int8)->Int8=(h.id); apply(h.id,alias(1))};",
    );
    entry.dependencies.push(helper.id.clone());
    let report = check(&bundle(vec![entry.clone(), helper.clone()]), &entry.id).unwrap();
    let module = report
        .modules
        .iter()
        .find(|m| m.source == entry.id)
        .unwrap();
    let imported: Vec<_> = module
        .function_instantiations
        .values()
        .filter(|i| i.symbol.source == helper.id)
        .collect();
    assert_eq!(imported.len(), 2);
    for evidence in imported {
        assert_eq!(
            evidence.symbol,
            SymbolId {
                source: helper.id.clone(),
                declaration: 0
            }
        );
        assert_eq!(
            report.signatures.types.get(evidence.arguments[0]).unwrap(),
            &Type::Scalar(Scalar::Int8)
        );
    }
    assert_eq!(module.function_instantiations.len(), 3); // apply plus two named id values; alias calls remain dynamic.
}
#[test]
fn generic_body_evidence_preserves_the_callers_rigid_parameter() {
    let report = run("fn id<T>(x:T)->T=x; fn caller<T>(x:T)->T=id(x);");
    let evidence = report.modules[0]
        .function_instantiations
        .values()
        .next()
        .unwrap();
    assert_eq!(evidence.symbol.declaration, 0);
    let caller = report
        .signatures
        .functions
        .iter()
        .find(|f| f.symbol.declaration == 1)
        .unwrap();
    assert_eq!(evidence.arguments, vec![caller.generics[0].ty]);
    assert_eq!(
        report.signatures.types.get(evidence.arguments[0]).unwrap(),
        &Type::Parameter {
            symbol: caller.symbol.clone(),
            ordinal: 0
        }
    );
}
#[test]
fn source_replay_revisions_and_invalid_calls_preserve_the_proof_boundary() {
    let helper = source("helper", "pub fn id<T>(x:T)->T=x;");
    let mut entry = source(
        "main",
        "import package helper version \"1\" as h; fn caller()->Int=h.id(1);",
    );
    entry.dependencies.push(helper.id.clone());
    let original = bundle(vec![entry.clone(), helper.clone()]);
    let report = check(&original, &entry.id).unwrap();
    let reversed = bundle(vec![helper.clone(), entry.clone()]);
    let replayed: DefinitionBundle =
        serde_json::from_value(serde_json::to_value(reversed).unwrap()).unwrap();
    assert_eq!(
        serde_json::to_value(&report).unwrap(),
        serde_json::to_value(check(&replayed, &entry.id).unwrap()).unwrap()
    );
    let revised = source("helper", "pub fn id<T>(x:T)->T=x; // new revision");
    entry.dependencies = vec![revised.id.clone()];
    let fresh = check(&bundle(vec![entry.clone(), revised.clone()]), &entry.id).unwrap();
    let module = fresh.modules.iter().find(|m| m.source == entry.id).unwrap();
    assert_eq!(
        module
            .function_instantiations
            .values()
            .next()
            .unwrap()
            .symbol
            .source,
        revised.id
    );
    assert_ne!(helper.id, revised.id);
    for body in [
        "fn id<T>(x:T)->T=x; fn bad()->Bool=id(1);",
        "fn id<T>(x:T)->T=x; fn bad()->Int={let f=id;1};",
        "fn keep<T,U>(x:T)->T=x; fn bad()->Int=keep(1);",
    ] {
        let invalid = source("invalid", body);
        assert!(check(&bundle(vec![invalid.clone()]), &invalid.id).is_err());
    }
    let ordinary = run("fn main(x:Int)->Int=x;");
    let json = serde_json::to_value(&ordinary).unwrap();
    assert!(json["modules"][0].get("function_instantiations").is_none());
}
