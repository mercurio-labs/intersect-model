//! Actual full-schema model-feature consumers, shared by native and zero-import WASM.
use intersect_model::definitions::{
    bundle::{DefinitionBundle, DefinitionSource, SourceKind},
    generic_types::Type,
    rcore_behavior::{
        bindings::{self, Target},
        checking,
        model_types::ModelTypes,
    },
    resources::DeclarationId,
};
fn require(value: bool, message: &str) -> Result<(), String> {
    if value { Ok(()) } else { Err(message.into()) }
}
fn inputs(case: usize, missing: bool) -> (DefinitionBundle, usize, usize) {
    let (classes, index, owner) = match case {
        0 => (
            "class Node {attr label:String [0,1] derived;ref next:Node [0,*];}",
            0,
            0,
        ),
        1 => (
            "class Root {attr label:String [0,1] derived;}class Left extends Root {}class Right extends Root {}class Leaf extends Left, Right {}",
            3,
            0,
        ),
        _ => (
            "class Value {}class Other {}class Box<T> {}class Use {attr label:String [0,1] derived;ref value:Box<Value> [0,1];ref other:Box<Other> [0,1];}",
            3,
            3,
        ),
    };
    let receiver = if case == 0 {
        "Node"
    } else if case == 1 {
        "Leaf"
    } else {
        "Use"
    };
    let name = if missing { "missing" } else { "label" };
    let source = DefinitionSource::new(
        "memory:model-feature-consumer.rcore",
        SourceKind::Rcore,
        format!(
            "rcore 1;package p version \"1\";metamodel \"urn:model-feature-consumer\" prefix p {{{classes}}}behavior {{import model \"urn:model-feature-consumer\" as m;derive m.{receiver}.{name}(self)->String?=none;}}"
        ),
    );
    (
        DefinitionBundle {
            version: 1,
            sources: vec![source],
            namespaces: vec![],
        },
        index,
        owner,
    )
}
pub fn verify() -> Result<usize, String> {
    for case in 0..3 {
        let (bundle, index, owner) = inputs(case, false);
        let original = serde_json::to_vec(&bundle).map_err(|e| e.to_string())?;
        let entry = &bundle.sources[0].id;
        let expression = Type::Classifier {
            declaration: DeclarationId {
                source: entry.clone(),
                path: vec![index],
            },
            arguments: vec![],
            raw: false,
        };
        let mut models = ModelTypes::new(&bundle)?;
        let projection = models.feature_projection(&expression, "label")?;
        let expected = DeclarationId {
            source: entry.clone(),
            path: vec![owner, 0],
        };
        require(
            projection.declaration == expected,
            "Full-schema projection lost declaration identity",
        )?;
        require(
            projection.receiver.expression() == &expression,
            "Projection changed receiver",
        )?;
        require(
            !projection.schema_revision.is_empty(),
            "Missing materialized schema revision",
        )?;
        if case == 2 {
            let checks = models.generic_checks();
            require(checks.iter().filter(|report| matches!(report.receiver.expression(),Type::Classifier{arguments,..} if !arguments.is_empty())).count()==2,"Full referenced generic closure was skipped")?;
        }
        let bound = bindings::bind(&bundle, entry).map_err(|e| format!("{e:?}"))?;
        let targets = bound
            .bindings
            .iter()
            .flat_map(|m| m.providers.values())
            .filter_map(|target| match target {
                Target::Feature(id) => Some(id),
                _ => None,
            })
            .collect::<Vec<_>>();
        require(
            targets == vec![&expected],
            "Binder changed exact full-schema feature identity",
        )?;
        checking::check(&bundle, entry).map_err(|e| format!("{e:?}"))?;
        require(
            serde_json::to_vec(&bundle).map_err(|e| e.to_string())? == original,
            "Compilation changed source inputs",
        )?;
        let (bad, _, _) = inputs(case, true);
        let errors = bindings::bind(&bad, &bad.sources[0].id)
            .err()
            .ok_or("Unknown model feature was accepted")?;
        require(
            errors.iter().any(|e| e.message.contains("Unknown feature")),
            "Unknown feature diagnostic changed",
        )?;
    }
    Ok(3)
}
