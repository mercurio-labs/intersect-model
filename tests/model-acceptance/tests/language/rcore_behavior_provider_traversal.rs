//! Source-callable cancellation through installed traversal providers.
use intersect_model::definitions::{
    bundle::{DefinitionBundle, DefinitionSource, SourceKind},
    compiler::import_datatype_bundle,
    conversion::{ConversionContract, ConversionProvider, ConversionRegistry, ProviderId},
    rcore_behavior::{
        function_lowering::compile_function_with_conversions,
        ir_v2::{DataWire, EntrySelection, Limits, ValueWire},
        modules::SymbolId,
    },
    resources::DeclarationId,
};

use intersect_model::{
    diagnostic_chain_conversion::DiagnosticChainConverter,
    rcore_behavior_functions::{
        EvaluationSession, ExecutionContext, ExecutionControl, ExecutionErrorKind, ExecutionLimits,
        SessionProgress, evaluate,
    },
    resource_conversion::{ResourceConverter, ResourceKind},
    tree_iterator_conversion::TreeIteratorConverter,
};
use serde_json::json;
use std::{cell::Cell, num::NonZeroUsize};
struct Control {
    polls: Cell<usize>,
    stop: usize,
}
impl ExecutionControl for Control {
    fn cancellation_requested(&self) -> bool {
        self.polls.set(self.polls.get() + 1);
        self.polls.get() == self.stop
    }
}
fn check(kind: &str) {
    let (class, provider_name) = match kind {
        "diagnostic" => (
            "org.eclipse.emf.common.util.DiagnosticChain",
            "ecore.diagnostic-chain",
        ),
        "tree" => (
            "org.eclipse.emf.common.util.TreeIterator",
            "ecore.definition-tree-iterator",
        ),
        "resources" => (
            "org.eclipse.emf.ecore.resource.ResourceSet",
            "ecore.definition-resource-set",
        ),
        _ => panic!("unknown fixture"),
    };
    let children = (0..8)
        .map(|n| format!("<eClassifiers x:type='e:EClass' name='C{n}'/>"))
        .collect::<String>();
    let model = intersect_model_acceptance::migration_fixture::migrate_source_fixture(DefinitionSource::new(
        "memory:traversal.ecore",
        SourceKind::Ecore,
        format!(
            "<e:EPackage xmlns:e='http://www.eclipse.org/emf/2002/Ecore' xmlns:x='http://www.w3.org/2001/XMLSchema-instance' name='m' nsURI='urn:traversal'><eClassifiers x:type='e:EDataType' name='Value' serializable='false' instanceClassName='{class}'/>{children}</e:EPackage>"
        ),
    )).0;
    let mut sources = vec![model.clone()];
    for n in 0..7 {
        sources.push(intersect_model_acceptance::migration_fixture::migrate_source_fixture(DefinitionSource::new(format!("memory:member{n}.ecore"), SourceKind::Ecore,
            format!("<e:EPackage xmlns:e='http://www.eclipse.org/emf/2002/Ecore' name='m{n}' nsURI='urn:member{n}'/>"))).0);
    }
    let mut roots = sources
        .iter()
        .map(|s| DeclarationId {
            source: s.id.clone(),
            path: vec![],
        })
        .collect::<Vec<_>>();
    roots.sort();
    let mut source = DefinitionSource::new(
        "memory:traversal-behavior.rcore",
        SourceKind::RcoreBehavior,
        r#"rcore 1;package traversal version "1";import model "urn:traversal" as m;fn run(x:m.Value)->m.Value={let keep=|n:Int8|x;keep(0)};"#,
    );
    source.dependencies = sources.iter().map(|s| s.id.clone()).collect();
    sources.push(source.clone());
    let bundle = DefinitionBundle {
        version: 1,
        sources,
        namespaces: vec![],
    };
    let declaration =
        intersect_model_acceptance::migration_fixture::named_declaration_fixture(&model, "Value");
    let contract = ConversionContract {
        datatype: declaration.clone(),
        provider: ProviderId {
            name: provider_name.into(),
            version: "1".into(),
        },
        serializable: false,
        members: vec![],
        declarations: match kind {
            "tree" => vec![DeclarationId {
                source: model.id.clone(),
                path: vec![],
            }],
            "resources" => roots.clone(),
            _ => vec![],
        },
    };
    let provider: Box<dyn ConversionProvider> = match kind {
        "diagnostic" => Box::new(DiagnosticChainConverter::new(contract.clone()).unwrap()),
        "tree" => Box::new(TreeIteratorConverter::new(contract.clone(), &bundle).unwrap()),
        "resources" => {
            Box::new(ResourceConverter::new(contract.clone(), ResourceKind::Set, &bundle).unwrap())
        }
        _ => unreachable!(),
    };
    let leaf = json!({"severity":"info","source":null,"code":1,"message":null,
        "data":[{"kind":"scalar","datatype":"EInt","value":7},{"kind":"scalar","datatype":"EInt","value":8}],"children":[]});
    let payload = match kind {
        "diagnostic" => {
            let mut parent = leaf.clone();
            parent["children"] = json!(vec![leaf; 3]);
            json!({"diagnostics":vec![parent; 4]})
        }
        "tree" => json!({"roots":contract.declarations,"position":6,"pruned":[2,3,4]}),
        "resources" => {
            json!({"resources":roots.iter().map(|r|r.source.clone()).collect::<Vec<_>>()})
        }
        _ => unreachable!(),
    };
    let datatype = import_datatype_bundle(&bundle, &declaration).unwrap();
    let mut registry = ConversionRegistry::default();
    registry.register(&datatype, provider.as_ref()).unwrap();
    let image = compile_function_with_conversions(
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
        &registry,
        &Limits::default(),
    )
    .unwrap();
    let wire = serde_json::to_value(registry.bind(&declaration, payload).unwrap()).unwrap();
    let arg = ValueWire {
        ty: image.program().functions[image.program().entry.0 as usize].parameters[0],
        data: DataWire::Datatype(wire.clone()),
    };
    let context = ExecutionContext {
        bundle: &bundle,
        conversions: Some(&registry),
        references: &[],
    };
    let limits = ExecutionLimits::default();
    let run = |argument: &ValueWire, stop, quantum| {
        let control = Control {
            polls: Cell::new(0),
            stop,
        };
        let mut session =
            EvaluationSession::new(&image, std::slice::from_ref(argument), &context, &limits)
                .unwrap();
        loop {
            match session.resume_with_control(NonZeroUsize::new(quantum).unwrap(), &control) {
                SessionProgress::Pending => {}
                SessionProgress::Complete(result) => {
                    assert!(matches!(
                        session.resume(NonZeroUsize::MIN),
                        SessionProgress::Finished
                    ));
                    break (*result, control.polls.get());
                }
                SessionProgress::Finished => panic!("missing terminal outcome"),
            }
        }
    };
    let (baseline, polls) = run(&arg, usize::MAX, 1);
    let baseline = baseline.unwrap();
    assert_eq!(baseline.value, arg);
    assert_eq!(
        serde_json::to_value(&baseline).unwrap(),
        serde_json::to_value(
            evaluate(&image, std::slice::from_ref(&arg), &context, &limits).unwrap()
        )
        .unwrap()
    );
    let needed = baseline.consumed.provider_work;
    assert!(needed > baseline.consumed.provider_invocations);
    for ceiling in [0, needed - 1, needed, needed + 1] {
        let bounded = ExecutionLimits {
            max_provider_work: ceiling,
            ..limits.clone()
        };
        let mut expected = None;
        for quantum in [1, 7] {
            let mut session =
                EvaluationSession::new(&image, std::slice::from_ref(&arg), &context, &bounded)
                    .unwrap();
            let result = loop {
                match session.resume(NonZeroUsize::new(quantum).unwrap()) {
                    SessionProgress::Pending => {}
                    SessionProgress::Complete(result) => break *result,
                    SessionProgress::Finished => panic!("missing bounded result"),
                }
            };
            assert!(matches!(
                session.resume(NonZeroUsize::MIN),
                SessionProgress::Finished
            ));
            let wire = if ceiling >= needed {
                let value = serde_json::to_value(result.unwrap()).unwrap();
                assert_eq!(value, serde_json::to_value(&baseline).unwrap());
                value
            } else {
                let error = result.unwrap_err();
                assert_eq!(error.kind, ExecutionErrorKind::ResourceExhausted);
                assert_eq!(error.consumed.provider_work, ceiling);
                assert!(error.dependency.is_none());
                if ceiling == 0 {
                    assert_eq!(error.consumed.provider_invocations, 0);
                    assert!(error.consumed_providers.is_empty());
                }
                serde_json::to_value(error).unwrap()
            };
            if let Some(expected) = &expected {
                assert_eq!(&wire, expected);
            } else {
                expected = Some(wire);
            }
        }
        assert_eq!(
            serde_json::to_value(
                evaluate(&image, std::slice::from_ref(&arg), &context, &limits).unwrap()
            )
            .unwrap(),
            serde_json::to_value(&baseline).unwrap()
        );
    }
    let mut inside = 0;
    for stop in 1..=polls {
        let mut expected = None;
        for quantum in [1, 7] {
            let (result, seen) = run(&arg, stop, quantum);
            let error = result.unwrap_err();
            assert_eq!(seen, stop);
            assert_eq!(error.kind, ExecutionErrorKind::Cancelled);
            assert!(error.dependency.is_none());
            assert!(error.consumed.provider_invocations <= baseline.consumed.provider_invocations);
            if error.message.contains("provider work") && error.consumed.provider_invocations == 1 {
                inside += 1;
                assert_eq!(error.consumed_providers, vec![contract.provider.clone()]);
            }
            let wire = serde_json::to_value(error).unwrap();
            if let Some(expected) = &expected {
                assert_eq!(&wire, expected);
            } else {
                expected = Some(wire);
            }
        }
    }
    assert!(
        inside >= 16,
        "must cancel during traversal, not only at dispatch: {kind}"
    );
    let mut bad = wire;
    match kind {
        "diagnostic" => bad["value"]["diagnostics"][0]["data"][0]["value"] = json!("wrong"),
        "tree" => bad["value"]["position"] = json!(u64::MAX),
        "resources" => {
            let first = bad["value"]["resources"][0].clone();
            bad["value"]["resources"]
                .as_array_mut()
                .unwrap()
                .push(first);
        }
        _ => unreachable!(),
    }
    let bad = ValueWire {
        ty: arg.ty,
        data: DataWire::Datatype(bad),
    };
    let (error, failure_polls) = run(&bad, usize::MAX, 1);
    let error = error.unwrap_err();
    assert_eq!(error.kind, ExecutionErrorKind::Nominal);
    assert_eq!(
        serde_json::to_value(&error).unwrap(),
        serde_json::to_value(run(&bad, failure_polls + 1, 7).0.unwrap_err()).unwrap()
    );
    assert_eq!(
        serde_json::to_value(run(&arg, usize::MAX, 7).0.unwrap()).unwrap(),
        serde_json::to_value(baseline).unwrap()
    );
}
#[test]
#[cfg_attr(not(feature = "milestone-tests"), ignore = "milestone")]
fn diagnostic_nodes_and_data_cancel_cooperatively() {
    check("diagnostic");
}
#[test]
fn tree_replay_and_child_expansion_cancel_cooperatively() {
    check("tree");
}
#[test]
fn resource_membership_validation_cancels_cooperatively() {
    check("resources");
}
