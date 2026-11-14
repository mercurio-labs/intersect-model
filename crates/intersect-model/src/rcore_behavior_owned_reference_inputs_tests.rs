use super::*;
use intersect_model::definitions::{
    bundle::{DefinitionSource, SourceKind},
    conversion::ReferenceContext,
    rcore_behavior::{
        function_lowering::compile_function,
        ir_v2::{EntrySelection, Limits},
        modules::SymbolId,
    },
};
fn inputs() -> (DefinitionBundle, VerifiedExecutable, Vec<ReferenceContext>) {
    let source = DefinitionSource::new(
        "memory:owned-inputs-behavior.rcore",
        SourceKind::RcoreBehavior,
        "rcore 1;package ownedInputs version \"1\";fn run()->Int8=42;",
    );
    let bundle = DefinitionBundle {
        version: 1,
        sources: vec![source.clone()],
        namespaces: vec![],
    };
    let image = compile_function(
        &bundle,
        &source.id,
        &EntrySelection {
            symbol: SymbolId {
                source: source.id.clone(),
                declaration: 0,
            },
            types: vec![],
            arguments: vec![],
        },
        &Limits::default(),
    )
    .unwrap();
    let references = (0..2)
        .map(|index| ReferenceContext {
            bundle: bundle.clone(),
            entry: source.id.clone(),
            namespace: format!("unused-{index}"),
            conversions: vec![],
            materialized_types: vec![],
            applied_conversions: vec![],
            constructors: vec![],
            model: serde_json::json!({"text":format!("{index}:{}","x".repeat(256))}),
        })
        .collect();
    (bundle, image, references)
}
fn owned(limits: ExecutionLimits) -> EvaluationSession<'static> {
    let (bundle, image, references) = inputs();
    EvaluationSession::new_owned(image, vec![], bundle, references, None, limits).unwrap()
}
fn prepared(session: &mut EvaluationSession<'_>) {
    for _ in 0..10000 {
        if matches!(session.phase(), Phase::TransferReferences) {
            return;
        }
        assert!(matches!(
            session.resume(NonZeroUsize::MIN),
            SessionProgress::Pending
        ));
    }
    panic!("context preparation did not reach transfer");
}
struct Cancel;
impl ExecutionControl for Cancel {
    fn cancellation_requested(&self) -> bool {
        true
    }
}
#[test]
fn prepared_owned_contexts_move_exact_allocations_and_preserve_original_indices() {
    let (bundle, image, references) = inputs();
    let addresses = references
        .iter()
        .map(|reference| reference.model["text"].as_str().unwrap().as_ptr() as usize)
        .collect::<Vec<_>>();
    let digests = references
        .iter()
        .map(|reference| reference.digest().unwrap())
        .collect::<Vec<_>>();
    let mut session = EvaluationSession::new_owned(
        image,
        vec![],
        bundle,
        references,
        None,
        ExecutionLimits::default(),
    )
    .unwrap();
    prepared(&mut session);
    assert_eq!(session.execution().reference_inputs.len(), 2);
    for index in [1, 0] {
        assert!(matches!(
            session.resume(NonZeroUsize::MIN),
            SessionProgress::Pending
        ));
        let pool = session.execution().owned_reference_inputs.as_ref().unwrap();
        let moved = &pool[&index];
        assert_eq!(
            moved.model["text"].as_str().unwrap().as_ptr() as usize,
            addresses[index]
        );
        assert_eq!(moved.digest().unwrap(), digests[index]);
        assert_eq!(
            session.execution().reference_inputs[&digests[index]].0,
            index
        );
        assert!(session.execution().domains.is_empty());
    }
    assert!(session.references().is_empty());
    loop {
        match session.resume(NonZeroUsize::MIN) {
            SessionProgress::Pending => {}
            SessionProgress::Complete(outcome) => {
                let intersect_model::definitions::rcore_behavior::ir_v2::DataWire::Scalar(value) =
                    outcome.unwrap().value.data
                else {
                    panic!("Scalar result expected");
                };
                assert_eq!(value.payload, serde_json::json!(42));
                break;
            }
            SessionProgress::Finished => panic!("missing outcome"),
        }
    }
}
#[test]
fn transfer_storage_denial_precedes_movement_and_exact_boundary_admits_one_record() {
    let mut baseline = owned(ExecutionLimits::default());
    prepared(&mut baseline);
    let mut consumed = Consumption::default();
    baseline
        .execution()
        .provider_receipts
        .counters(&mut consumed);
    let record = OWNED_REFERENCE_INPUT_BYTES;
    for slack in [record - 1, record] {
        let mut session = owned(ExecutionLimits {
            max_model_storage_bytes: consumed.model_storage_bytes + slack,
            ..ExecutionLimits::default()
        });
        prepared(&mut session);
        let progress = session.resume(NonZeroUsize::MIN);
        if slack < record {
            let SessionProgress::Complete(outcome) = progress else {
                panic!("storage denial expected")
            };
            assert_eq!(
                outcome.unwrap_err().kind,
                ExecutionErrorKind::ResourceExhausted
            );
            assert_eq!(session.references().len(), 2);
            assert!(session.execution().owned_reference_inputs.is_none());
        } else {
            assert!(matches!(progress, SessionProgress::Pending));
            assert_eq!(session.references().len(), 1);
            assert_eq!(
                session
                    .execution()
                    .owned_reference_inputs
                    .as_ref()
                    .unwrap()
                    .len(),
                1
            );
        }
        assert!(session.execution().domains.is_empty());
    }
}
#[test]
fn cancellation_at_each_owned_transfer_boundary_is_terminal_without_domain_publication() {
    for prefix in 0..=2 {
        let mut session = owned(ExecutionLimits::default());
        prepared(&mut session);
        for _ in 0..prefix {
            assert!(matches!(
                session.resume(NonZeroUsize::MIN),
                SessionProgress::Pending
            ));
        }
        let SessionProgress::Complete(outcome) =
            session.resume_with_control(NonZeroUsize::MAX, &Cancel)
        else {
            panic!("cancellation expected")
        };
        let error = outcome.unwrap_err();
        assert_eq!(error.kind, ExecutionErrorKind::Cancelled);
        assert_eq!(error.consumed.reference_domains, 0);
        assert!(session.execution().domains.is_empty());
        assert_eq!(session.references().len(), 2 - prefix);
        assert!(matches!(
            session.resume(NonZeroUsize::MAX),
            SessionProgress::Finished
        ));
    }
}

// Diagnostic only: these larger limits must never replace the frozen host gate.
#[test]
#[ignore = "Requires an explicitly supplied exact frozen request for native profiling"]
fn exact_reference_runtime_allocation_profile_is_diagnostic_only() {
    let path = std::env::var("INTERSECT_DIAGNOSTIC_REFERENCE_REQUEST").unwrap();
    let request: serde_json::Value = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
    let bundle: DefinitionBundle = serde_json::from_value(request["bundle"].clone()).unwrap();
    let entry = serde_json::from_value(request["entry"].clone()).unwrap();
    let selection = serde_json::from_value(request["selection"].clone()).unwrap();
    assert_eq!(
        bundle.sources[0].id.sha256,
        "f1e38abc4457ce56324d15be77e39b28f30d4d8fe4c8a856c5bc070802371557"
    );
    let image = compile_function(&bundle, &entry, &selection, &Limits::default()).unwrap();
    let arguments = serde_json::from_value(request["arguments"].clone()).unwrap();
    let references = serde_json::from_value(request["reference_contexts"].clone()).unwrap();
    let limits = ExecutionLimits {
        max_provider_work: 64 * 1024 * 1024,
        max_model_storage_bytes: 64 * 1024 * 1024,
        ..ExecutionLimits::default()
    };
    let mut session =
        EvaluationSession::new_owned(image, arguments, bundle, references, None, limits).unwrap();
    // Stop at actual admission publication; the frozen spin body is deliberately
    // expensive and is outside this allocation-phase diagnostic.
    let mut last_phase = None;
    for _ in 0..100000 {
        let phase = session
            .decoder()
            .as_ref()
            .and_then(|decoder| decoder.diagnostic_domain_phase());
        if phase != last_phase {
            let mut consumed = session.execution().consumed.clone();
            session
                .execution()
                .provider_receipts
                .counters(&mut consumed);
            println!(
                "DIAGNOSTIC_NOT_ACCEPTANCE phase={phase:?} consumed={}",
                serde_json::to_string(&consumed).unwrap()
            );
            last_phase = phase;
        }
        if session.execution().domains.len() == 1 {
            let mut consumed = session.execution().consumed.clone();
            session
                .execution()
                .provider_receipts
                .counters(&mut consumed);
            println!(
                "DIAGNOSTIC_NOT_ACCEPTANCE admission consumed={}",
                serde_json::to_string(&consumed).unwrap()
            );
            return;
        }
        match session.resume(NonZeroUsize::MIN) {
            SessionProgress::Pending => {}
            SessionProgress::Complete(outcome) => panic!(
                "Diagnostic ended before admission: {}",
                serde_json::to_string(&*outcome).unwrap()
            ),
            SessionProgress::Finished => panic!("Missing diagnostic outcome"),
        }
    }
    panic!("Diagnostic admission did not complete");
}

fn reference_inputs() -> (
    DefinitionBundle,
    VerifiedExecutable,
    ValueWire,
    ReferenceContext,
) {
    use intersect_model::definitions::{
        compiler::materialize_runtime_bundle,
        generic_types::{RuntimeType, Type},
        resources::DeclarationId,
    };
    let model_source = DefinitionSource::new(
        "memory:owned-reference.rcore",
        SourceKind::Rcore,
        r#"rcore 1; package owned version "1"; metamodel "urn:owned-reference" prefix owned { class Base { attr name: EString [0,1]; } class Child extends Base {} }"#,
    );
    let types = (0..2)
        .map(|index| {
            RuntimeType::new(Type::Classifier {
                declaration: DeclarationId {
                    source: model_source.id.clone(),
                    path: vec![index],
                },
                arguments: vec![],
                raw: false,
            })
            .unwrap()
        })
        .collect::<Vec<_>>();
    let model_bundle = DefinitionBundle {
        version: 1,
        sources: vec![model_source.clone()],
        namespaces: vec![],
    };
    let schema = materialize_runtime_bundle(
        &model_bundle,
        &model_source.id,
        "urn:owned-reference",
        &types,
        None,
    )
    .unwrap();
    let mut model = intersect_model::core::model::Model::default();
    model
        .create(
            &schema,
            "child",
            schema.class_key_for_type(&types[1]).unwrap(),
            "memory:owned-model",
            0,
            0,
        )
        .unwrap();
    let reference = ReferenceContext {
        bundle: model_bundle,
        entry: model_source.id.clone(),
        namespace: "urn:owned-reference".into(),
        conversions: vec![],
        materialized_types: types,
        applied_conversions: vec![],
        constructors: vec![],
        model: serde_json::to_value(model).unwrap(),
    };
    let mut source = DefinitionSource::new(
        "memory:owned-reference-behavior.rcore",
        SourceKind::RcoreBehavior,
        "rcore 1;package ownedReference version \"1\";import model \"urn:owned-reference\" as m;fn run(value:m.Base)->m.Base=value;",
    );
    source.dependencies.push(model_source.id.clone());
    let bundle = DefinitionBundle {
        version: 1,
        sources: vec![model_source, source.clone()],
        namespaces: vec![],
    };
    let image = compile_function(
        &bundle,
        &source.id,
        &EntrySelection {
            symbol: SymbolId {
                source: source.id.clone(),
                declaration: 1,
            },
            types: vec![],
            arguments: vec![],
        },
        &Limits::default(),
    )
    .unwrap();
    let argument = ValueWire {
        ty: image.program().functions[0].parameters[0],
        data: intersect_model::definitions::rcore_behavior::ir_v2::DataWire::Model(
            serde_json::json!({"kind":"model_reference","context":reference.digest().unwrap(),"element":"child"}),
        ),
    };
    (bundle, image, argument, reference)
}
fn finish_owned_reference(
    bundle: DefinitionBundle,
    image: VerifiedExecutable,
    argument: ValueWire,
    reference: ReferenceContext,
) -> Result<Evaluation> {
    let mut session = EvaluationSession::new_owned(
        image,
        vec![argument],
        bundle,
        vec![reference],
        None,
        ExecutionLimits::default(),
    )?;
    loop {
        match session.resume(NonZeroUsize::new(256).unwrap()) {
            SessionProgress::Pending => {}
            SessionProgress::Complete(outcome) => return *outcome,
            SessionProgress::Finished => panic!("missing owned outcome"),
        }
    }
}
#[test]
fn registered_owned_reference_replay_preserves_value_and_rejects_changed_source_and_model() {
    let (bundle, image, argument, reference) = reference_inputs();
    let references = vec![reference.clone()];
    let baseline = evaluate(
        &image,
        std::slice::from_ref(&argument),
        &ExecutionContext {
            bundle: &bundle,
            conversions: None,
            references: &references,
        },
        &ExecutionLimits::default(),
    )
    .unwrap();
    let replay: ReferenceContext =
        serde_json::from_value(serde_json::to_value(&reference).unwrap()).unwrap();
    let owned =
        finish_owned_reference(bundle.clone(), image.clone(), argument.clone(), replay).unwrap();
    assert_eq!(owned.value, baseline.value);
    assert_eq!(owned.consumed.reference_domains, 1);
    assert_eq!(owned.consumed.model_elements, 1);
    assert_eq!(
        serde_json::to_value(owned.observation).unwrap(),
        serde_json::to_value(baseline.observation).unwrap()
    );
    for changed_source in [true, false] {
        let mut changed = reference.clone();
        if changed_source {
            changed.bundle.sources[0].text.push(' ');
        } else {
            changed.model["elements"][0]["type_identity"] = serde_json::json!("forged");
        }
        let mut wire = argument.clone();
        let intersect_model::definitions::rcore_behavior::ir_v2::DataWire::Model(data) =
            &mut wire.data
        else {
            panic!("model reference expected")
        };
        data["context"] = serde_json::json!(changed.digest().unwrap());
        let error =
            finish_owned_reference(bundle.clone(), image.clone(), wire, changed).unwrap_err();
        assert!(matches!(
            error.kind,
            ExecutionErrorKind::Nominal | ExecutionErrorKind::MissingDependency
        ));
        assert_eq!(error.consumed.reference_domains, 0);
    }
    assert_eq!(
        finish_owned_reference(bundle, image, argument, reference)
            .unwrap()
            .consumed
            .reference_domains,
        1
    );
}
#[test]
fn registered_admission_checks_cancellation_inside_compiler_and_structural_batches() {
    struct After(std::cell::Cell<usize>);
    impl ExecutionControl for After {
        fn cancellation_requested(&self) -> bool {
            let remaining = self.0.get();
            self.0.set(remaining.saturating_sub(1));
            remaining == 0
        }
    }
    for phase in ["compiler", "validate_model"] {
        let (bundle, image, argument, reference) = reference_inputs();
        let mut session = EvaluationSession::new_owned(
            image,
            vec![argument],
            bundle,
            vec![reference],
            None,
            ExecutionLimits::default(),
        )
        .unwrap();
        let mut reached = false;
        for _ in 0..10000 {
            if session
                .decoder()
                .as_ref()
                .and_then(|decoder| decoder.diagnostic_domain_phase())
                == Some(phase)
            {
                reached = true;
                break;
            }
            assert!(matches!(
                session.resume(NonZeroUsize::MIN),
                SessionProgress::Pending
            ));
        }
        assert!(reached, "missing admission phase {phase}");
        let control = After(std::cell::Cell::new(8));
        let SessionProgress::Complete(outcome) =
            session.resume_with_control(NonZeroUsize::MIN, &control)
        else {
            panic!("batch did not observe inner cancellation in {phase}")
        };
        let error = outcome.unwrap_err();
        assert_eq!(error.kind, ExecutionErrorKind::Cancelled);
        assert_eq!(error.consumed.reference_domains, 0);
        assert!(session.execution().domains.is_empty());
        assert!(matches!(
            session.resume(NonZeroUsize::MAX),
            SessionProgress::Finished
        ));
    }
    let (bundle, image, argument, reference) = reference_inputs();
    assert_eq!(
        finish_owned_reference(bundle, image, argument, reference)
            .unwrap()
            .consumed
            .reference_domains,
        1
    );
}
