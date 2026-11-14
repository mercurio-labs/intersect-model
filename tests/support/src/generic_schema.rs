//! Shared native qualification fixture.
use crate::FixtureRequired;
// Shared test-only native schema builder; not a generic compiler.
use intersect_model::definitions::{
    bundle::{DefinitionBundle, DefinitionSource, SourceKind},
    generic_resolution::Resolver,
    generic_types::ClosedType,
    resources::DeclarationId,
    schema::Schema,
};
use serde_json::json;
pub fn schema() -> Schema {
    let text = r#"rcore 1; package generic_runtime version "1";
metamodel "urn:g" prefix g {
    class Value {}
    class Other {}
    class Box<T> {}
    class Use {
        ref value: Box<Value> [0,1];
        ref other: Box<Other> [0,1];
        op pick(input: Box<Value> [0,1]) -> void;
        op pick(input: Box<Other> [0,1]) -> void;
    }
}"#;
    let source = DefinitionSource::new("memory:generic-runtime.rcore", SourceKind::Rcore, text);
    let resolver = Resolver::new(&DefinitionBundle {
        version: 1,
        sources: vec![source.clone()],
        namespaces: vec![],
    })
    .required();
    let mut instances = serde_json::Map::new();
    let mut declarations = serde_json::Map::new();
    let mut classes = serde_json::Map::new();
    for (position, key) in [(0, "box_value"), (1, "box_other")] {
        let report = resolver
            .application_bounds(&DeclarationId {
                source: source.id.clone(),
                path: vec![3, position, 0],
            })
            .required();
        assert!(report.valid);
        let ty = ClosedType::new(report.selected.expression).required();
        instances.insert(key.into(), serde_json::to_value(ty).required());
        declarations.insert(
            key.into(),
            json!({"source":source.id,"path":[2],"namespace":"urn:g","name":"Box"}),
        );
        classes.insert(
            key.into(),
            json!({"name":"Box","abstract_type":false,"parents":[],"features":{}}),
        );
    }
    classes.insert("Use".into(),json!({"name":"Use","abstract_type":false,"parents":[],"feature_order":["value"],"feature_declarations":{"value":{"source":source.id,"path":[3,0]}},"features":{
        "value":{"name":"value","target":"box_value","reference":true,"containment":false,"opposite":null,"lower":0,"upper":1,"unique":true,"ordered":true,"derived":false,"default":null}
    }}));
    let operations: Vec<_> = [(2, "box_value"), (3, "box_other")].into_iter().map(|(position, target)| json!({
        "name":"pick", "declaration":{"source":source.id,"path":[3,position]},
        "parameters":[{"name":"input","declaration":{"source":source.id,"path":[3,position,0]},
            "value":{"target":{"kind":"class","name":target},"lower":0,"upper":1,"ordered":true,"unique":true}}],
        "result":{"target":null,"lower":0,"upper":1,"ordered":true,"unique":true},
        "source_payload":"", "generic":false
    })).collect();
    classes.get_mut("Use").required()["operations"] = json!(operations);
    declarations.insert(
        "Use".into(),
        json!({"source":source.id,"path":[3],"namespace":"urn:g","name":"Use"}),
    );
    // Build the schema explicitly to test native identity validation independently
    // from the compiler materialization controls.
    serde_json::from_value(json!({"revision":source.id.sha256,"uri":"urn:g","name":"g","classes":classes,"declarations":declarations,"instantiations":instances})).required()
}
