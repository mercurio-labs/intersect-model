use intersect_model::core::model::Model;
use intersect_model::definitions::{
    bundle::{DefinitionBundle, DefinitionSource, SourceKind},
    compiler::compile_structure_bundle_with_conversions,
    conversion::{ConversionContract, ProviderId, ReferenceContext},
    rcore_behavior::{
        function_lowering::compile_function_with_conversions,
        ir_v2::{DataWire, EntrySelection, Limits, TypeNode, ValueWire},
        modules::SymbolId,
    },
};

use intersect_model::{
    conversion::with_catalog,
    rcore_behavior_functions::{
        DependencyCause, ExecutionContext, ExecutionErrorKind, ExecutionLimits, evaluate,
    },
};
use serde_json::json;

#[test]
fn provider_backed_stored_navigation_preserves_payloads_flags_and_failure_recovery() {
    for ordered in [false, true] {
        for unique in [false, true] {
            let text = include_str!("../../../../fixtures/ecore-navigation/provider-stored.ecore")
                .replace(
                    "ordered=\"true\" unique=\"true\"",
                    &format!("ordered=\"{ordered}\" unique=\"{unique}\""),
                );
            let model_source =
                intersect_model_acceptance::migration_fixture::migrate_source_fixture(
                    DefinitionSource::new("memory:provider-stored.ecore", SourceKind::Ecore, text),
                )
                .0;
            let contract = ConversionContract {
                datatype: intersect_model_acceptance::migration_fixture::named_declaration_fixture(
                    &model_source,
                    "RGB",
                ),
                provider: ProviderId {
                    name: "example.rgb".into(),
                    version: "1".into(),
                },
                serializable: true,
                declarations: vec![],
                members: vec![],
            };
            for (body, many) in [
                (
                    "fn main(x:m.Node)->m.RGB ={let read=|unused:Int8|x.color;read(0)};",
                    false,
                ),
                (
                    "fn main(x:m.Node)->Sequence<m.RGB> =Sequence(x.colors);",
                    true,
                ),
            ] {
                let body = if many && !ordered {
                    body.replace("Sequence", "Bag")
                } else {
                    body.into()
                };
                let mut source = DefinitionSource::new(
                    "memory:provider-stored-behavior.rcore",
                    SourceKind::RcoreBehavior,
                    format!(
                        "rcore 1;package provider_navigation version \"1\";import model \"urn:intersect:provider-navigation\" as m;{body}"
                    ),
                );
                source.dependencies.push(model_source.id.clone());
                let model_bundle = DefinitionBundle {
                    version: 1,
                    sources: vec![model_source.clone()],
                    namespaces: vec![],
                };
                let bundle = DefinitionBundle {
                    version: 1,
                    sources: vec![model_source.clone(), source.clone()],
                    namespaces: vec![],
                };
                with_catalog(&bundle, std::slice::from_ref(&contract), |registry| {
                    let schema = compile_structure_bundle_with_conversions(&model_bundle, &model_source.id, "urn:intersect:provider-navigation", registry).unwrap();
                    let payload = json!({"kind":"converted", "datatype":contract.datatype, "provider":contract.provider, "value":{"red":18,"green":58,"blue":188}});
                    let other = json!({"kind":"converted", "datatype":contract.datatype, "provider":contract.provider, "value":{"red":0,"green":255,"blue":1}});
                    let colors = if unique { json!([payload, other]) } else { json!([payload, other, payload]) };
                    let mut model = Model::default();
                    model.create(&schema, "d", "Node", "r", 0, 0).unwrap();
                    model.elements[0].values.insert("color".into(), payload.clone());
                    model.elements[0].values.insert("colors".into(), colors.clone());
                    model.validate_with_conversions(&schema, registry).unwrap();
                    let reference = ReferenceContext { bundle: model_bundle.clone(), entry: model_source.id.clone(), namespace: "urn:intersect:provider-navigation".into(), conversions: vec![contract.clone()], materialized_types: vec![], applied_conversions: vec![], constructors: vec![], model: serde_json::to_value(model).unwrap() };
                    let image = compile_function_with_conversions(&bundle, &source.id, &EntrySelection { symbol: SymbolId { source: source.id.clone(), declaration: 1 }, types: vec![], arguments: vec![] }, registry, &Limits::default()).unwrap();
                    if many {
                        assert!(image.program().types.iter().any(|t| matches!(t, TypeNode::Many { ordered: o, unique: u, .. } if *o == ordered && *u == unique)));
                    }
                    let ty = image.program().functions[image.program().entry.0 as usize].parameters[0];
                    let argument = |reference: &ReferenceContext| ValueWire { ty, data: DataWire::Model(json!({"kind":"model_reference", "context":reference.digest().unwrap(), "element":"d"})) };
                    let run = |reference: &ReferenceContext, limits: &ExecutionLimits| {
                        let mut context = ExecutionContext::new(&bundle); context.conversions = Some(registry); context.references = std::slice::from_ref(reference);
                        evaluate(&image, &[argument(reference)], &context, limits)
                    };
                    let limits = ExecutionLimits::default();
                    let baseline = run(&reference, &limits).unwrap();
                    assert_eq!(baseline.consumed_providers, vec![contract.provider.clone()]);
                    if many {
                        let DataWire::Collection(items) = baseline.value.data else { panic!("collection") };
                        let actual: Vec<_> = items.into_iter().map(|item| { let DataWire::Datatype(value) = item.data else { panic!("converted datatype") }; value }).collect();
                        assert_eq!(json!(actual), colors);
                    } else { assert_eq!(baseline.value.data, DataWire::Datatype(payload.clone())); }
                    let before = reference.clone();
                    if many {
                        let mut empty = reference.clone();
                        empty.model["elements"][0]["values"]["colors"] = json!([]);
                        let before = empty.clone();
                        assert_eq!(run(&empty, &limits).unwrap().value.data, DataWire::Collection(vec![]));
                        assert_eq!(empty, before);
                    }
                    let exhausted = run(&reference, &ExecutionLimits { max_provider_invocations: 0, ..Default::default() }).unwrap_err();
                    assert_eq!(exhausted.kind, ExecutionErrorKind::ResourceExhausted);
                    assert_eq!(reference, before);
                    let unmeasured = run(&reference, &ExecutionLimits { max_provider_work: 0, ..Default::default() }).unwrap_err();
                    assert_eq!(unmeasured.kind, ExecutionErrorKind::ResourceExhausted);
                    assert_eq!(unmeasured.consumed.provider_invocations, 0);
                    assert!(unmeasured.consumed_providers.is_empty());
                    let work = run(&reference, &limits).unwrap().consumed.provider_work;
                    assert!(work > 0);
                    assert_eq!(run(&reference, &ExecutionLimits { max_provider_work: work - 1, ..Default::default() }).unwrap_err().kind, ExecutionErrorKind::ResourceExhausted);
                    assert!(run(&reference, &ExecutionLimits { max_provider_work: work, ..Default::default() }).is_ok());
                    for stale in [false, true] {
                        let mut invalid = reference.clone();
                        if stale { invalid.model["elements"][0]["values"]["color"]["provider"]["version"] = json!("stale"); }
                        else { invalid.model["elements"][0]["values"]["color"]["value"]["red"] = json!(256); }
                        let before = invalid.clone(); let error = run(&invalid, &limits).unwrap_err();
                        assert_eq!(error.kind, ExecutionErrorKind::Nominal, "{error:?}"); assert_eq!(invalid, before);
                    }
                    let mut missing = ExecutionContext::new(&bundle); missing.references = std::slice::from_ref(&reference);
                    let error = evaluate(&image, &[argument(&reference)], &missing, &limits).unwrap_err();
                    assert_eq!(error.kind, ExecutionErrorKind::MissingDependency);
                    assert!(matches!(error.dependency.as_deref(), Some(DependencyCause::ExecutableContext(_))));
                    assert!(run(&reference, &limits).is_ok());
                    Ok(())
                }).unwrap();
            }
        }
    }
}
