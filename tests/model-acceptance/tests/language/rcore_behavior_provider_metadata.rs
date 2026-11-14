//! Unselected caller registry metadata is bounded before copy and policy comparison.
use intersect_model::definitions::{
    bundle::{DefinitionBundle, DefinitionSource, SourceKind},
    compiler::import_datatype_bundle,
    conversion::{
        ConversionContract, ConversionProvider, ConversionRegistry, MemberContract, MemberType,
        ProviderId, ReferenceContext,
    },
    rcore_behavior::{
        function_lowering::compile_function_with_conversions,
        ir_v2::{EntrySelection, Limits},
        modules::SymbolId,
    },
    resources::DeclarationId,
};
use intersect_model::rcore_behavior_functions::{
    ExecutionContext, ExecutionErrorKind, ExecutionLimits, evaluate,
};
use serde_json::Value;
use std::cell::Cell;

struct Probe {
    contract: ConversionContract,
    calls: Cell<usize>,
}
impl ConversionProvider for Probe {
    fn contract(&self) -> &ConversionContract {
        &self.contract
    }
    fn parse(&self, _: &str) -> Result<Value, String> {
        self.calls.set(self.calls.get() + 1);
        Ok(Value::Bool(true))
    }
    fn format(&self, _: &Value) -> Result<String, String> {
        self.calls.set(self.calls.get() + 1);
        Ok("true".into())
    }
    fn validate(&self, _: &Value) -> Result<(), String> {
        self.calls.set(self.calls.get() + 1);
        Ok(())
    }
    fn default_value(&self) -> Result<Option<Value>, String> {
        self.calls.set(self.calls.get() + 1);
        Ok(None)
    }
}
fn control(reference: bool) {
    let model = intersect_model_acceptance::migration_fixture::migrate_source_fixture(DefinitionSource::new(
        "memory:extra-provider.ecore",
        SourceKind::Ecore,
        r#"<e:EPackage xmlns:e="http://www.eclipse.org/emf/2002/Ecore" xmlns:x="http://www.w3.org/2001/XMLSchema-instance" name="extra" nsURI="urn:extra"><eClassifiers x:type="e:EDataType" name="Opaque" instanceClassName="example.Opaque"/></e:EPackage>"#,
    )).0;
    let mut source = DefinitionSource::new(
        "memory:extra-provider-behavior.rcore",
        SourceKind::RcoreBehavior,
        r#"rcore 1;package extra version "1";fn run()->Int8=7;"#,
    );
    source.dependencies.push(model.id.clone());
    let bundle = DefinitionBundle {
        version: 1,
        sources: vec![model.clone(), source.clone()],
        namespaces: vec![],
    };
    let empty = ConversionRegistry::default();
    let image = compile_function_with_conversions(
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
        &empty,
        &Limits::default(),
    )
    .unwrap();
    let baseline_context = ExecutionContext {
        bundle: &bundle,
        conversions: Some(&empty),
        references: &[],
    };
    let limits = ExecutionLimits::default();
    let baseline = evaluate(&image, &[], &baseline_context, &limits).unwrap();
    let declaration = DeclarationId {
        source: model.id.clone(),
        path: vec![0],
    };
    let datatype = import_datatype_bundle(&bundle, &declaration).unwrap();
    let plain = ConversionContract {
        datatype: declaration.clone(),
        provider: ProviderId {
            name: "extra.unselected".into(),
            version: "1".into(),
        },
        serializable: true,
        declarations: vec![],
        members: vec![],
    };
    let mut contract = plain.clone();
    if reference {
        let mut payload = Value::Null;
        for _ in 0..128 {
            payload = Value::Array(vec![payload]);
        }
        contract.members.push(MemberContract {
            nullable: false,
            value: MemberType::Reference {
                class: declaration.clone(),
                context: Box::new(ReferenceContext {
                    bundle: bundle.clone(),
                    entry: model.id.clone(),
                    namespace: "urn:extra".into(),
                    conversions: vec![],
                    materialized_types: vec![],
                    applied_conversions: vec![],
                    constructors: vec![],
                    model: payload,
                }),
            },
        });
    } else {
        for _ in 0..128 {
            let mut parent = plain.clone();
            parent.members.push(MemberContract {
                nullable: false,
                value: MemberType::Converted {
                    contract: Box::new(contract),
                },
            });
            contract = parent;
        }
    }
    let probe = Probe {
        contract,
        calls: Cell::new(0),
    };
    let mut extra = ConversionRegistry::default();
    extra.register(&datatype, &probe).unwrap();
    let error = evaluate(
        &image,
        &[],
        &ExecutionContext {
            bundle: &bundle,
            conversions: Some(&extra),
            references: &[],
        },
        &limits,
    )
    .unwrap_err();
    assert_eq!(
        error.kind,
        ExecutionErrorKind::ResourceExhausted,
        "{error:?}"
    );
    assert!(error.message.contains("depth"), "{error:?}");
    assert_eq!(error.consumed.steps, 0);
    assert_eq!(error.consumed.provider_invocations, 0);
    assert_eq!(error.consumed.context_bytes, 0);
    assert!(error.selected_providers.is_empty());
    assert!(error.consumed_providers.is_empty());
    assert_eq!(probe.calls.get(), 0);
    // A shallow extra contract reaches the distinct policy-mismatch check.
    let shallow = Probe {
        contract: plain,
        calls: Cell::new(0),
    };
    let mut shallow_registry = ConversionRegistry::default();
    shallow_registry.register(&datatype, &shallow).unwrap();
    let mismatch = evaluate(
        &image,
        &[],
        &ExecutionContext {
            bundle: &bundle,
            conversions: Some(&shallow_registry),
            references: &[],
        },
        &limits,
    )
    .unwrap_err();
    assert_eq!(mismatch.kind, ExecutionErrorKind::Arguments, "{mismatch:?}");
    assert_eq!(shallow.calls.get(), 0);
    let recovered = evaluate(&image, &[], &baseline_context, &limits).unwrap();
    assert_eq!(
        serde_json::to_value(recovered).unwrap(),
        serde_json::to_value(baseline).unwrap()
    );
}
#[test]
fn unused_nested_conversion_contract_is_rejected_before_registry_copy() {
    control(false);
}
#[test]
fn unused_reference_model_payload_is_rejected_before_registry_copy() {
    control(true);
}
