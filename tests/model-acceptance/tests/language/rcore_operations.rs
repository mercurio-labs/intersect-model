use intersect_model::core::model::Model;
use intersect_model::definitions::{
    bundle::{DefinitionBundle, DefinitionSource, SourceKind},
    compiler::materialize_runtime_bundle,
    conversion::{ConversionRegistry, ReferenceContext},
    generic_types::{RuntimeType, Type},
    rcore::resolve_structure,
    rcore_behavior::{
        ir_v2::{DataWire, EntrySelection, Limits, ValueWire},
        modules::SymbolId,
    },
    rcore_operations::{OperationSelection, compile_operation},
};

use intersect_model::{
    model_reference::ReferenceDomain,
    rcore_behavior_functions::{ExecutionContext, ExecutionErrorKind, ExecutionLimits},
    rcore_operations::invoke,
};

#[test]
fn typed_source_operation_invokes_shared_runtime_and_preserves_rejection_and_recovery() {
    let structure = DefinitionSource::new(
        "memory:organization.rcore",
        SourceKind::Rcore,
        include_str!("../../../../fixtures/rcore/day04/organization.rcore"),
    );
    let model_source = structure;
    let model_bundle = DefinitionBundle {
        version: 1,
        sources: vec![model_source.clone()],
        namespaces: vec![],
    };
    let (_, operations) = resolve_structure(&model_bundle, &model_source.id).unwrap();
    let receiver = RuntimeType::new(Type::Classifier {
        declaration: operations[0].owner.clone(),
        arguments: vec![],
        raw: false,
    })
    .unwrap();

    let schema = materialize_runtime_bundle(
        &model_bundle,
        &model_source.id,
        "urn:intersect:rcore:organization",
        std::slice::from_ref(&receiver),
        None,
    )
    .unwrap();
    let mut model = Model::default();
    for id in ["A", "B", "C"] {
        model
            .create(
                &schema,
                id,
                schema.class_key_for_type(&receiver).unwrap(),
                "memory:model",
                0,
                0,
            )
            .unwrap();
    }
    model
        .edit(
            &schema,
            &[
                ("A".into(), "links".into(), serde_json::json!(["B"])),
                ("B".into(), "links".into(), serde_json::json!(["C"])),
            ],
        )
        .unwrap();
    model.validate(&schema).unwrap();
    let reference = ReferenceContext {
        bundle: model_bundle,
        entry: model_source.id.clone(),
        namespace: "urn:intersect:rcore:organization".into(),
        conversions: vec![],
        materialized_types: vec![receiver.clone()],
        applied_conversions: vec![],
        constructors: vec![],
        model: serde_json::to_value(&model).unwrap(),
    };
    let mut entry = DefinitionSource::new(
        "memory:behavior.rcore",
        SourceKind::RcoreBehavior,
        include_str!("../../../../fixtures/rcore/day04/organization-behavior.rcore"),
    );
    entry.dependencies.push(model_source.id.clone());
    let bundle = DefinitionBundle {
        version: 1,
        sources: vec![model_source, entry.clone()],
        namespaces: vec![],
    };
    let selected = OperationSelection {
        receiver: receiver.clone(),
        declaration: operations[0].declaration.clone(),
        materialized_types: vec![receiver.clone()],
        implementation: EntrySelection {
            symbol: SymbolId {
                source: entry.id.clone(),
                declaration: 1,
            },
            types: vec![],
            arguments: vec![],
        },
    };
    let operation = compile_operation(&bundle, &entry.id, &selected, &Limits::default()).unwrap();
    let program = operation.executable().program();
    let ty = program.functions[program.entry.0 as usize].parameters[0];
    let registry = ConversionRegistry::default();
    let payload = ReferenceDomain::new_runtime(receiver, reference.clone(), &registry)
        .unwrap()
        .bind("A")
        .unwrap();
    let argument = ValueWire {
        ty,
        data: DataWire::Model(payload),
    };
    let references = vec![reference];
    let context = ExecutionContext {
        bundle: &bundle,
        conversions: None,
        references: &references,
    };
    let evaluate = || {
        invoke(
            &operation,
            std::slice::from_ref(&argument),
            &context,
            &ExecutionLimits::default(),
        )
        .unwrap()
    };
    let before = serde_json::to_value(&model).unwrap();
    let result = evaluate();
    let json = serde_json::to_value(result.value).unwrap();
    let ids = json["data"]["Collection"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v["data"]["Model"]["element"].as_str().unwrap())
        .collect::<Vec<_>>();
    assert_eq!(ids, vec!["A", "B", "C"]);
    assert_eq!(serde_json::to_value(&model).unwrap(), before);
    let denied = invoke(
        &operation,
        std::slice::from_ref(&argument),
        &context,
        &ExecutionLimits {
            max_provider_work: 0,
            ..ExecutionLimits::default()
        },
    )
    .unwrap_err();
    assert_eq!(denied.kind, ExecutionErrorKind::ResourceExhausted);
    let absent = ExecutionContext {
        bundle: &bundle,
        conversions: None,
        references: &[],
    };
    assert!(
        invoke(
            &operation,
            std::slice::from_ref(&argument),
            &absent,
            &ExecutionLimits::default()
        )
        .is_err()
    );
    assert_eq!(serde_json::to_value(evaluate().value).unwrap(), json);
}
