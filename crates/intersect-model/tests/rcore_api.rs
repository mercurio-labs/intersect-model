use intersect_model::{
    api::{BasicModel, ModelEdit},
    compile_rcore_at,
    core::{
        frozen::FrozenStore,
        storage::{self, ModelRead},
    },
    rcore::{DataWire, ExecutionErrorKind, ExecutionLimits, RcoreError, RcorePackage},
};
use serde_json::json;
const SOURCE: &str = include_str!("../assets/fixtures/rcore/api/organization.rcore");
#[test]
fn direct_api_constructs_executes_and_reloads() -> Result<(), Box<dyn std::error::Error>> {
    let mut package = compile_rcore_at("memory:organization.rcore", SOURCE)?;
    package.bind_operation("Node", "reachable", "reachable")?;
    package.bind_operation("Node", "label", "label")?;
    let mut model = package.new_model()?;
    for id in ["A", "B", "C"] {
        model.create(id, "Node", "memory:objects")?;
    }
    model.set("A", "links", json!(["B"]))?;
    model.set("B", "links", json!(["C"]))?;
    let baseline = package.save(&model)?;
    let saved: serde_json::Value = serde_json::from_slice(&baseline)?;
    assert_eq!(
        saved["model"],
        serde_json::to_value(storage::export(&*model)?)?
    );
    assert!(model.set("A", "links", json!(42)).is_err());
    assert_eq!(baseline, package.save(&model)?);
    let result = package.invoke(&model, "A", "reachable", &[], &ExecutionLimits::default())?;
    let DataWire::Collection(values) = result.value.data else {
        panic!("Expected reachable nodes")
    };
    let ids: Vec<_> = values
        .iter()
        .map(|v| match &v.data {
            DataWire::Model(v) => v["element"].as_str().unwrap(),
            _ => panic!("Expected model reference"),
        })
        .collect();
    assert_eq!(ids, ["A", "B", "C"]);
    let (reloaded, model) = RcorePackage::reload(&baseline)?;
    let frozen: &BasicModel<FrozenStore> = &model;
    let reader: &dyn ModelRead = frozen;
    assert_eq!(reader.value("A", "links")?, Some(json!(["B"])));
    assert_eq!(baseline, reloaded.save(&model)?);
    let result = reloaded.invoke(
        &model,
        "A",
        "reachable",
        &[],
        &ExecutionLimits {
            max_provider_work: 0,
            ..Default::default()
        },
    );
    assert!(
        matches!(result, Err(RcoreError::Execution(e)) if e.kind == ExecutionErrorKind::ResourceExhausted)
    );
    reloaded.invoke(&model, "A", "reachable", &[], &ExecutionLimits::default())?;
    Ok(())
}

#[test]
fn explicit_bindings_and_arguments_are_checked_atomically() -> Result<(), Box<dyn std::error::Error>>
{
    use intersect_model::rcore::{Scalar, ScalarType, ScalarValue};
    let mut package = compile_rcore_at("memory:organization.rcore", SOURCE)?;
    let mut model = package.new_model()?;
    model.create("A", "Node", "memory:objects")?;
    assert!(matches!(
        package.invoke(&model, "A", "reachable", &[], &ExecutionLimits::default()),
        Err(RcoreError::MissingImplementation { .. })
    ));
    assert!(
        package
            .bind_operation("Node", "reachable", "missing")
            .is_err()
    );
    assert!(
        package
            .bind_operation("Node", "reachable", "label")
            .is_err()
    );
    package.bind_operation("Node", "reachable", "reachable")?;
    assert!(
        package
            .bind_operation("Node", "reachable", "reachable")
            .is_err()
    );
    package.bind_operation("Node", "label", "label")?;
    let arg = DataWire::Scalar(ScalarValue {
        ty: ScalarType::Scalar(Scalar::String),
        payload: json!("hello"),
    });
    let result = package.invoke(&model, "A", "label", &[arg], &ExecutionLimits::default())?;
    assert!(
        matches!(result.value.data, DataWire::Scalar(ref value) if value.payload == json!("hello"))
    );
    assert!(
        package
            .invoke(&model, "A", "label", &[], &ExecutionLimits::default())
            .is_err()
    );
    assert!(
        package
            .invoke(
                &model,
                "A",
                "label",
                &[DataWire::None],
                &ExecutionLimits::default()
            )
            .is_err()
    );
    assert!(
        package
            .invoke(
                &model,
                "missing",
                "reachable",
                &[],
                &ExecutionLimits::default()
            )
            .is_err()
    );
    package.invoke(&model, "A", "reachable", &[], &ExecutionLimits::default())?;
    let revised = compile_rcore_at(
        "memory:organization.rcore",
        &SOURCE.replace("unnamed", "renamed"),
    )?;
    assert!(revised.save(&model).is_err());
    Ok(())
}
#[test]
fn reload_rebuilds_authority_and_rejects_corrupt_data() -> Result<(), Box<dyn std::error::Error>> {
    let mut package = compile_rcore_at("memory:organization.rcore", SOURCE)?;
    package.bind_operation("Node", "reachable", "reachable")?;
    let mut model = package.new_model()?;
    model.create("A", "Node", "memory:objects")?;
    let original: serde_json::Value = serde_json::from_slice(&package.save(&model)?)?;
    for mutation in 0..5 {
        let mut saved = original.clone();
        match mutation {
            0 => saved["version"] = json!(1),
            1 => saved["bundle"]["sources"][0]["text"] = json!("tampered source"),
            2 => saved["bindings"][0]["function"] = json!("label"),
            3 => saved["model"]["elements"][0]["class"] = json!("Missing"),
            _ => saved["executable"] = json!({"fake":true}),
        }
        assert!(
            RcorePackage::reload(&serde_json::to_vec(&saved)?).is_err(),
            "Mutation {mutation}"
        );
    }
    Ok(())
}
#[test]
fn dependency_sources_are_exact_explicit_and_io_free() -> Result<(), Box<dyn std::error::Error>> {
    use intersect_model::{
        compile_rcore_bundle,
        rcore::{DefinitionBundle, DefinitionSource, SourceKind},
    };
    let dependency = DefinitionSource::new(
        "memory:helpers.rcore",
        SourceKind::RcoreBehavior,
        "rcore 1; package helpers version \"1\"; pub fn echo(text:String)->String { text }",
    );
    let text = SOURCE
        .replace(
            "import model",
            "import package helpers version \"1\" as h;\n import model",
        )
        .replace("{ text }", "{ h.echo(text) }");
    let mut source = DefinitionSource::new("memory:organization.rcore", SourceKind::Rcore, text);
    source.dependencies.push(dependency.id.clone());
    let entry = source.id.clone();
    let bundle = DefinitionBundle {
        version: 1,
        sources: vec![source, dependency],
        namespaces: vec![],
    };
    let mut package = compile_rcore_bundle(bundle.clone(), &entry)?;
    package.bind_operation("Node", "label", "label")?;
    let mut model = package.new_model()?;
    model.create("A", "Node", "memory:objects")?;
    let arg = DataWire::Scalar(intersect_model::rcore::ScalarValue {
        ty: intersect_model::rcore::ScalarType::Scalar(intersect_model::rcore::Scalar::String),
        payload: json!("dependency"),
    });
    let result = package.invoke(&model, "A", "label", &[arg], &ExecutionLimits::default())?;
    assert!(matches!(result.value.data, DataWire::Scalar(v) if v.payload == json!("dependency")));
    let mut missing = bundle.clone();
    missing.sources.pop();
    assert!(compile_rcore_bundle(missing, &entry).is_err());
    let mut undeclared = bundle.clone();
    undeclared.sources[0].dependencies.clear();
    assert!(compile_rcore_bundle(undeclared, &entry).is_err());
    let mut changed = bundle;
    changed.sources[1].text.push(' ');
    assert!(compile_rcore_bundle(changed, &entry).is_err());
    Ok(())
}
#[test]
fn unsupported_behavior_adapters_fail_visibly() {
    let text = SOURCE.replace("behavior {", "behavior legacy {");
    assert!(compile_rcore_at("memory:legacy.rcore", &text).is_err());
    let text = SOURCE.replace(
        "fn reachable",
        "validate m.Node.named(node) { require true code \"name\" message \"name\"; }\n fn reachable",
    );
    let errors = compile_rcore_at("memory:unsupported.rcore", &text).unwrap_err();
    assert!(
        errors
            .to_string()
            .contains("Derived/validation and provider contract adapters are unsupported"),
        "{errors}"
    );
    assert!(compile_rcore_at("", SOURCE).is_err());
}
