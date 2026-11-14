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
    rcore_operations::{OperationSelection, VerifiedOperation, compile_operation},
};

use intersect_model::{
    model_reference::ReferenceDomain,
    rcore_behavior_functions::{
        EvaluationSession, ExecutionContext, ExecutionLimits, ObservationLimits, ReadKind,
        SessionProgress,
    },
    rcore_operations::invoke,
};
use std::num::NonZeroUsize;
struct Fixture {
    bundle: DefinitionBundle,
    operation: VerifiedOperation,
    reference: ReferenceContext,
    argument: ValueWire,
}
fn fixture(kerml: bool, body: Option<&str>) -> Fixture {
    let name = if kerml {
        "kerml-specialization"
    } else {
        "organization"
    };
    let structure = if kerml {
        include_str!("../../../../fixtures/rcore/day04/kerml-specialization.rcore")
    } else {
        include_str!("../../../../fixtures/rcore/day04/organization.rcore")
    };
    let model_source =
        DefinitionSource::new(format!("memory:{name}.rcore"), SourceKind::Rcore, structure);
    let model_bundle = DefinitionBundle {
        version: 1,
        sources: vec![model_source.clone()],
        namespaces: vec![],
    };
    let (classifiers, operations) = resolve_structure(&model_bundle, &model_source.id).unwrap();
    let ty = |i: usize| {
        RuntimeType::new(Type::Classifier {
            declaration: classifiers[i].declaration.clone(),
            arguments: vec![],
            raw: false,
        })
        .unwrap()
    };
    let receiver = ty(0);
    let types = if kerml {
        vec![receiver.clone(), ty(1)]
    } else {
        vec![receiver.clone()]
    };
    let namespace = if kerml {
        "urn:intersect:rcore:kerml-specialization-demo"
    } else {
        "urn:intersect:rcore:organization"
    };

    let schema =
        materialize_runtime_bundle(&model_bundle, &model_source.id, namespace, &types, None)
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
    if kerml {
        for (id, specific, general) in [("s0", "A", "B"), ("s1", "B", "C")] {
            model
                .create(
                    &schema,
                    id,
                    schema.class_key_for_type(&types[1]).unwrap(),
                    "memory:model",
                    0,
                    0,
                )
                .unwrap();
            model
                .edit(
                    &schema,
                    &[
                        (id.into(), "specific".into(), serde_json::json!(specific)),
                        (id.into(), "general".into(), serde_json::json!(general)),
                    ],
                )
                .unwrap();
        }
    } else {
        model
            .edit(
                &schema,
                &[
                    ("A".into(), "links".into(), serde_json::json!(["B"])),
                    ("B".into(), "links".into(), serde_json::json!(["C"])),
                ],
            )
            .unwrap();
    }
    model.validate(&schema).unwrap();
    let reference = ReferenceContext {
        bundle: model_bundle,
        entry: model_source.id.clone(),
        namespace: namespace.into(),
        conversions: vec![],
        materialized_types: types.clone(),
        applied_conversions: vec![],
        constructors: vec![],
        model: serde_json::to_value(model).unwrap(),
    };
    let text = body.unwrap_or(if kerml {
        include_str!("../../../../fixtures/rcore/day04/kerml-specialization-behavior.rcore")
    } else {
        include_str!("../../../../fixtures/rcore/day04/organization-behavior.rcore")
    });
    let mut entry = DefinitionSource::new(
        format!("memory:{name}-behavior.rcore"),
        SourceKind::RcoreBehavior,
        text,
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
        materialized_types: types,
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
    let parameter = program.functions[program.entry.0 as usize].parameters[0];
    let registry = ConversionRegistry::default();
    let payload = ReferenceDomain::new_runtime(receiver, reference.clone(), &registry)
        .unwrap()
        .bind("A")
        .unwrap();
    Fixture {
        bundle,
        operation,
        reference,
        argument: ValueWire {
            ty: parameter,
            data: DataWire::Model(payload),
        },
    }
}
fn limits() -> ExecutionLimits {
    ExecutionLimits {
        observation: ObservationLimits {
            enabled: true,
            ..Default::default()
        },
        ..Default::default()
    }
}
#[test]
fn actual_reads_are_separate_from_validation_and_include_empty_inverse_queries() {
    for kerml in [false, true] {
        let f = fixture(kerml, None);
        let references = [f.reference];
        let context = ExecutionContext {
            bundle: &f.bundle,
            conversions: None,
            references: &references,
        };
        let plain = invoke(
            &f.operation,
            std::slice::from_ref(&f.argument),
            &context,
            &ExecutionLimits::default(),
        )
        .unwrap();
        assert!(plain.observation.is_none());
        assert!(
            serde_json::to_value(&plain)
                .unwrap()
                .get("observation")
                .is_none()
        );
        let observed = invoke(
            &f.operation,
            std::slice::from_ref(&f.argument),
            &context,
            &limits(),
        )
        .unwrap();
        assert_eq!(observed.value, plain.value);
        assert_eq!(
            serde_json::to_value(&observed.consumed).unwrap(),
            serde_json::to_value(&plain.consumed).unwrap()
        );
        let report = observed.observation.unwrap();
        assert!(report.execution_complete && report.events_complete && !report.truncated);
        assert!(!report.cache_dependencies_complete);
        assert_eq!(report.validation_admissions.len(), 1);
        for read in &report.semantic_reads {
            assert_eq!(read.context, report.validation_admissions[0].context);
            assert_eq!(
                read.member.source,
                f.operation.binding().operation.declaration.source
            );
            assert_eq!(
                read.source.source,
                f.operation.binding().implementation.source
            );
        }
        if kerml {
            let starts = report
                .semantic_reads
                .iter()
                .filter(|r| matches!(r.kind, ReadKind::IncomingStart))
                .map(|r| r.element.as_str())
                .collect::<Vec<_>>();
            assert_eq!(starts, ["A", "B", "C"]);
            let matches = report
                .semantic_reads
                .iter()
                .filter(|r| matches!(r.kind, ReadKind::IncomingMatch))
                .map(|r| r.matched.as_deref().unwrap())
                .collect::<Vec<_>>();
            assert_eq!(matches, ["s0", "s1"]);
            assert!(
                report
                    .semantic_reads
                    .iter()
                    .any(|r| matches!(r.kind, ReadKind::IncomingEnd)
                        && r.element == "C"
                        && r.cursor == Some(0))
            );
            let generals = report
                .semantic_reads
                .iter()
                .filter(|r| matches!(r.kind, ReadKind::StoredSingular))
                .map(|r| (r.element.as_str(), r.value.clone().unwrap()))
                .collect::<Vec<_>>();
            assert_eq!(
                generals,
                [
                    ("s0", serde_json::json!("B")),
                    ("s1", serde_json::json!("C"))
                ]
            );
        } else {
            let pages = report
                .semantic_reads
                .iter()
                .map(|r| (r.element.as_str(), r.cursor, r.value.clone()))
                .collect::<Vec<_>>();
            assert_eq!(
                pages,
                [
                    ("A", Some(0), Some(serde_json::json!("B"))),
                    ("A", Some(1), None),
                    ("B", Some(0), Some(serde_json::json!("C"))),
                    ("B", Some(1), None),
                    ("C", Some(0), None)
                ]
            );
        }
    }
}
#[test]
fn trace_bounds_failure_and_quantum_do_not_change_results_or_fabricate_completion() {
    let f = fixture(true, None);
    let references = [f.reference];
    let context = ExecutionContext {
        bundle: &f.bundle,
        conversions: None,
        references: &references,
    };
    let args = std::slice::from_ref(&f.argument);
    let expected = invoke(&f.operation, args, &context, &limits()).unwrap();
    for observation in [
        ObservationLimits {
            enabled: true,
            max_events: 1,
            max_event_bytes: 65536,
        },
        ObservationLimits {
            enabled: true,
            max_events: 4096,
            max_event_bytes: 0,
        },
    ] {
        let bounded = invoke(
            &f.operation,
            args,
            &context,
            &ExecutionLimits {
                observation: observation.clone(),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(bounded.value, expected.value);
        let report = bounded.observation.unwrap();
        assert!(report.execution_complete && report.truncated && !report.events_complete);
        assert!(report.event_bytes <= observation.max_event_bytes);
        assert!(
            report.semantic_reads.len() + report.validation_admissions.len()
                <= observation.max_events
        );
    }
    for quantum in [1, 7] {
        let settings = limits();
        let mut session =
            EvaluationSession::new(f.operation.executable(), args, &context, &settings).unwrap();
        let outcome = loop {
            match session.resume(NonZeroUsize::new(quantum).unwrap()) {
                SessionProgress::Pending => (),
                SessionProgress::Complete(value) => break value.unwrap(),
                SessionProgress::Finished => panic!("missing outcome"),
            }
        };
        assert_eq!(
            serde_json::to_value(outcome).unwrap(),
            serde_json::to_value(&expected).unwrap()
        );
    }
    let mut denied = limits();
    denied.scalar.max_steps = expected.consumed.steps - 1;
    let error = invoke(&f.operation, args, &context, &denied).unwrap_err();
    let report = error.observation.unwrap();
    assert!(!report.execution_complete && !report.cache_dependencies_complete);
    assert!(!report.semantic_reads.is_empty());
    struct Stop(std::cell::Cell<usize>);
    impl intersect_model::rcore_behavior_functions::ExecutionControl for Stop {
        fn cancellation_requested(&self) -> bool {
            let n = self.0.get() + 1;
            self.0.set(n);
            n == 10
        }
    }
    let settings = limits();
    let mut session =
        EvaluationSession::new(f.operation.executable(), args, &context, &settings).unwrap();
    let stop = Stop(std::cell::Cell::new(0));
    let cancelled = loop {
        match session.resume_with_control(NonZeroUsize::MIN, &stop) {
            SessionProgress::Pending => (),
            SessionProgress::Complete(value) => break value.unwrap_err(),
            SessionProgress::Finished => panic!("missing cancellation"),
        }
    };
    assert_eq!(
        cancelled.kind,
        intersect_model::rcore_behavior_functions::ExecutionErrorKind::Cancelled
    );
    assert!(!cancelled.observation.unwrap().execution_complete);
    assert_eq!(
        invoke(&f.operation, args, &context, &limits())
            .unwrap()
            .value,
        expected.value
    );
}
#[test]
fn dead_source_reads_are_not_observed_and_expression_bodies_use_the_same_runtime() {
    let text = "rcore 1; package test version \"1\"; import model \"urn:intersect:rcore:organization\" as m; fn main(node:m.Node)->OrderedSet<m.Node> { if false { closure([node], |t| t.links) } else { OrderedSet([node]) } }";
    let f = fixture(false, Some(text));
    let references = [f.reference];
    let context = ExecutionContext {
        bundle: &f.bundle,
        conversions: None,
        references: &references,
    };
    let result = invoke(
        &f.operation,
        std::slice::from_ref(&f.argument),
        &context,
        &limits(),
    )
    .unwrap();
    let report = result.observation.unwrap();
    assert!(report.semantic_reads.is_empty());
    assert_eq!(report.validation_admissions.len(), 1);
    let expression_source = "rcore 1; package test version \"1\"; import model \"urn:intersect:rcore:organization\" as m; fn main(node:m.Node)->OrderedSet<m.Node> = closure([node], |t| t.links);";
    let expression_fixture = fixture(false, Some(expression_source));
    let references = [expression_fixture.reference];
    let context = ExecutionContext {
        bundle: &expression_fixture.bundle,
        conversions: None,
        references: &references,
    };
    let result = invoke(
        &expression_fixture.operation,
        std::slice::from_ref(&expression_fixture.argument),
        &context,
        &limits(),
    )
    .unwrap();
    assert_eq!(result.observation.unwrap().semantic_reads.len(), 5);
}
