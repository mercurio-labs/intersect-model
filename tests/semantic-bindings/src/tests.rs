use super::*;
use intersect_model::definitions::{
    conversion::{ConversionRegistry, ReferenceContext},
    rcore_behavior::ir_v2::{DataWire, ValueWire},
};
use intersect_model::{
    rcore_behavior_functions::{ExecutionContext, ExecutionLimits},
    semantic_bindings::*,
};
use std::rc::Rc;

#[test]
fn external_contracts_need_no_reference_body_and_cannot_be_replayed_as_rcore() {
    use intersect_model::definitions::{
        rcore_behavior::{
            function_lowering::compile_function,
            ir_v2::{EntrySelection, Limits, verify},
        },
        semantic_bindings::compile_sources,
    };
    let sources = compile_sources(
        include_str!("../../../fixtures/rcore/bindings/external-project.rcore"),
        "",
    )
    .unwrap();
    let c = &sources.contracts[0];
    assert!(c.reference().is_none());
    assert!(c.admission_image().program().functions[0].blocks.is_empty());
    let mut registry = Registry::default();
    assert_eq!(
        registry.install_rcore(c).unwrap_err().kind,
        ErrorKind::MissingImplementation
    );
    assert!(registry.pin(c).is_err());
    manual(&mut registry, c).unwrap();
    let arg = 21_i64.into_wire(c.descriptor().parameters[0]).unwrap();
    assert_eq!(run(&registry, c, &sources.bundle, &[arg], &[]).unwrap(), 42);
    let selection = EntrySelection {
        symbol: c.descriptor().symbol.clone(),
        types: vec![],
        arguments: vec![],
    };
    assert!(
        compile_function(
            &sources.bundle,
            &selection.symbol.source,
            &selection,
            &Limits::default()
        )
        .is_err()
    );
    assert!(
        verify(
            c.admission_image().program().clone(),
            &sources.bundle,
            &Limits::default()
        )
        .is_err()
    );
    assert!(
        compile_sources(
            include_str!("../../../fixtures/rcore/bindings/model.rcore"),
            "rcore 1; package bad version \"1\"; extern fn wrong(value:Unknown)->Int64;"
        )
        .is_err()
    );
}

#[cfg(feature = "rhai")]
#[test]
fn external_rust_and_rhai_share_one_contract_without_an_rcore_body() {
    let sources = intersect_model::definitions::semantic_bindings::compile_sources(
        include_str!("../../../fixtures/rcore/bindings/external-project.rcore"),
        "",
    )
    .unwrap();
    let c = &sources.contracts[0];
    let mut registry = Registry::default();
    let arg = 21_i64.into_wire(c.descriptor().parameters[0]).unwrap();
    let source = include_str!("../../../fixtures/rcore/bindings/external.rhai");
    registry
        .install(
            c,
            Rc::new(rhai::RhaiProvider::compile(c, "memory:external.rhai", source).unwrap()),
        )
        .unwrap();
    assert_eq!(
        run(
            &registry,
            c,
            &sources.bundle,
            std::slice::from_ref(&arg),
            &[]
        )
        .unwrap(),
        42
    );
    registry
        .replace(
            c,
            Rc::new(NativeProvider::new(
                c,
                "native-external",
                |call: &Call<'_>, args: &[ValueWire]| call.result(i64::from_wire(&args[0])? * 2),
            )),
        )
        .unwrap();
    assert_eq!(run(&registry, c, &sources.bundle, &[arg], &[]).unwrap(), 42);
}

#[test]
fn source_revision_changes_invalidate_generated_adapters() {
    let (bundle, contracts, _) = contract::contracts().unwrap();
    let changed = include_str!("../../../fixtures/rcore/bindings/behavior.rcore")
        .replace("value + value", "value + value + value");
    let (_, newer, _) = contract::compile(
        include_str!("../../../fixtures/rcore/bindings/model.rcore"),
        &changed,
    )
    .unwrap();
    assert_ne!(contracts[0].id(), newer[0].id());
    let mut registry = Registry::default();
    assert_eq!(
        generated::register_twice(&mut registry, &newer[0])
            .unwrap_err()
            .kind,
        ErrorKind::Contract
    );
    assert!(registry.pin(&newer[0]).is_err());
    generated::register_twice(&mut registry, &contracts[0]).unwrap();
    let arg = 21_i64
        .into_wire(contracts[0].descriptor().parameters[0])
        .unwrap();
    assert_eq!(
        run(&registry, &contracts[0], &bundle, &[arg], &[]).unwrap(),
        42
    );
}
#[test]
fn nested_registry_dispatch_is_explicitly_unsupported_and_cancellation_is_sticky() {
    let (bundle, contracts, _) = contract::contracts().unwrap();
    let c = &contracts[0];
    let mut nested = Registry::default();
    nested.install_rcore(c).unwrap();
    let pinned = nested.pin(c).unwrap();
    let inner_bundle = bundle.clone();
    let mut outer = Registry::default();
    outer
        .install(
            c,
            Rc::new(NativeProvider::new(
                c,
                "nested",
                move |_: &Call<'_>, args: &[ValueWire]| {
                    pinned
                        .invoke(
                            args,
                            &ExecutionContext {
                                bundle: &inner_bundle,
                                conversions: None,
                                references: &[],
                            },
                            &ExecutionLimits::default(),
                            Budget::new(100_000),
                        )
                        .map(|v| v.evaluation.value)
                },
            )),
        )
        .unwrap();
    let args = [21_i64.into_wire(c.descriptor().parameters[0]).unwrap()];
    assert_eq!(
        run(&outer, c, &bundle, &args, &[]).unwrap_err().kind,
        ErrorKind::Unsupported
    );
    assert_eq!(run(&nested, c, &bundle, &args, &[]).unwrap(), 42);
    outer
        .replace(
            c,
            Rc::new(NativeProvider::new(
                c,
                "cancel",
                |call: &Call<'_>, _: &[ValueWire]| {
                    call.budget.cancel();
                    let _ = call.budget.check();
                    call.result(42_i64)
                },
            )),
        )
        .unwrap();
    assert_eq!(
        run(&outer, c, &bundle, &args, &[]).unwrap_err().kind,
        ErrorKind::Cancelled
    );
}
#[cfg(feature = "rhai")]
#[test]
fn rhai_collection_results_use_shared_validation_and_other_numbers_are_rejected() {
    use intersect_model::definitions::{
        rcore_behavior::ir_v2::TypeNode, semantic_bindings::compile_sources,
    };
    let source=compile_sources(include_str!("../../../fixtures/rcore/bindings/model.rcore"),"rcore 1; package checks version \"1\"; fn identity(value: Sequence<Int64>) -> Sequence<Int64> { value }").unwrap();
    let c = &source.contracts[0];
    let ty = c.descriptor().parameters[0];
    let TypeNode::Collection { element, .. } = c.descriptor().types[ty.0 as usize] else {
        panic!("Expected collection")
    };
    let argument = ValueWire {
        ty,
        data: DataWire::Collection(vec![21_i64.into_wire(element).unwrap()]),
    };
    let mut registry = Registry::default();
    registry
        .install(
            c,
            Rc::new(
                rhai::RhaiProvider::compile(
                    c,
                    "memory:collection.rhai",
                    "fn binding_0(arg0) { arg0 }",
                )
                .unwrap(),
            ),
        )
        .unwrap();
    let result = registry
        .pin(c)
        .unwrap()
        .invoke(
            std::slice::from_ref(&argument),
            &ExecutionContext {
                bundle: &source.bundle,
                conversions: None,
                references: &[],
            },
            &ExecutionLimits::default(),
            Budget::new(100_000),
        )
        .unwrap();
    assert_eq!(result.evaluation.value, argument);
    let source = compile_sources(
        include_str!("../../../fixtures/rcore/bindings/model.rcore"),
        "rcore 1; package checks version \"1\"; fn identity(value: Int32) -> Int32 { value }",
    )
    .unwrap();
    assert!(matches!(
        rhai::RhaiProvider::compile(
            &source.contracts[0],
            "memory:narrow.rhai",
            "fn binding_0(arg0) { arg0 }"
        ),
        Err(Error {
            kind: ErrorKind::Unsupported,
            ..
        })
    ));
}

fn run(
    registry: &Registry,
    contract: &Contract,
    bundle: &intersect_model::definitions::bundle::DefinitionBundle,
    arguments: &[ValueWire],
    references: &[ReferenceContext],
) -> Result<i64> {
    let result = registry.pin(contract)?.invoke(
        arguments,
        &ExecutionContext {
            bundle,
            conversions: None,
            references,
        },
        &ExecutionLimits::default(),
        Budget::new(100_000),
    )?;
    i64::from_wire(&result.evaluation.value)
}
fn manual(registry: &mut Registry, contract: &Contract) -> Result<()> {
    registry.install(
        contract,
        Rc::new(NativeProvider::new(
            contract,
            "manual-v1",
            |call: &Call<'_>, args: &[ValueWire]| {
                call.result(native::twice(call, i64::from_wire(&args[0])?)?)
            },
        )),
    )
}
#[test]
fn independent_expected_answers_for_reference_generated_and_manual_rust() {
    let (bundle, contracts, _) = contract::contracts().unwrap();
    let c = &contracts[0];
    let expected: serde_json::Value = serde_json::from_str(include_str!(
        "../../../fixtures/rcore/bindings/expected.json"
    ))
    .unwrap();
    for backend in 0..3 {
        let mut registry = Registry::default();
        match backend {
            0 => registry.install_rcore(c).unwrap(),
            1 => generated::register_twice(&mut registry, c).unwrap(),
            _ => manual(&mut registry, c).unwrap(),
        }
        for case in expected["twice"].as_array().unwrap() {
            let input = case["input"]
                .as_str()
                .unwrap()
                .parse::<i64>()
                .unwrap()
                .into_wire(c.descriptor().parameters[0])
                .unwrap();
            assert_eq!(
                run(&registry, c, &bundle, &[input], &[]).unwrap(),
                case["expected"].as_str().unwrap().parse::<i64>().unwrap()
            );
        }
    }
}
fn model_argument(
    bundle: &intersect_model::definitions::bundle::DefinitionBundle,
    c: &Contract,
    receiver: &intersect_model::definitions::generic_types::RuntimeType,
) -> (ReferenceContext, ValueWire) {
    use intersect_model::definitions::{
        bundle::DefinitionBundle, compiler::materialize_runtime_bundle,
    };
    let source = bundle
        .sources
        .iter()
        .find(|source| {
            matches!(receiver.expression(),
            intersect_model::definitions::generic_types::Type::Classifier { declaration, .. }
                if source.id == declaration.source)
        })
        .unwrap();
    let model_bundle = DefinitionBundle {
        version: 1,
        sources: vec![source.clone()],
        namespaces: vec![],
    };
    let schema = materialize_runtime_bundle(
        &model_bundle,
        &source.id,
        "urn:intersect:bindings",
        std::slice::from_ref(receiver),
        None,
    )
    .unwrap();
    let mut model = intersect_model::core::model::Model::default();
    model
        .create(
            &schema,
            "meter",
            schema.class_key_for_type(receiver).unwrap(),
            "memory:meter",
            0,
            0,
        )
        .unwrap();
    model
        .edit(
            &schema,
            &[(
                "meter".into(),
                "value".into(),
                intersect_model::definitions::scalars::int64_wire(21),
            )],
        )
        .unwrap();
    model.validate(&schema).unwrap();
    let reference = ReferenceContext {
        bundle: model_bundle,
        entry: source.id.clone(),
        namespace: "urn:intersect:bindings".into(),
        conversions: vec![],
        materialized_types: vec![receiver.clone()],
        applied_conversions: vec![],
        constructors: vec![],
        model: serde_json::to_value(model).unwrap(),
    };
    let payload = intersect_model::model_reference::ReferenceDomain::new_runtime(
        receiver.clone(),
        reference.clone(),
        &ConversionRegistry::default(),
    )
    .unwrap()
    .bind("meter")
    .unwrap();
    let value = ValueWire {
        ty: c.descriptor().parameters[0],
        data: DataWire::Model(payload),
    };
    (reference, value)
}
#[test]
fn generated_model_operation_and_reference_share_checked_reads() {
    let (bundle, contracts, receiver) = contract::contracts().unwrap();
    let c = &contracts[1];
    let (reference, argument) = model_argument(&bundle, c, &receiver);
    for native in [false, true] {
        let mut registry = Registry::default();
        if native {
            generated::register_doubled(&mut registry, c).unwrap();
        } else {
            registry.install_rcore(c).unwrap();
        }
        assert_eq!(
            run(
                &registry,
                c,
                &bundle,
                std::slice::from_ref(&argument),
                std::slice::from_ref(&reference)
            )
            .unwrap(),
            42
        );
        assert!(run(&registry, c, &bundle, std::slice::from_ref(&argument), &[]).is_err());
        let mut wrong = reference.clone();
        wrong.model["invalid"] = true.into();
        assert!(
            run(
                &registry,
                c,
                &bundle,
                std::slice::from_ref(&argument),
                &[wrong]
            )
            .is_err()
        );
    }
}
#[test]
fn registry_replacement_is_explicit_atomic_and_pinned() {
    let (bundle, contracts, _) = contract::contracts().unwrap();
    let c = &contracts[0];
    let mut registry = Registry::default();
    assert!(matches!(
        registry.pin(c),
        Err(Error {
            kind: ErrorKind::MissingImplementation,
            ..
        })
    ));
    manual(&mut registry, c).unwrap();
    let pinned = registry.pin(c).unwrap();
    assert_eq!(
        manual(&mut registry, c).unwrap_err().kind,
        ErrorKind::DuplicateImplementation
    );
    let wrong = NativeProvider::new(
        &contracts[1],
        "wrong",
        |call: &Call<'_>, _: &[ValueWire]| call.result(7_i64),
    );
    assert_eq!(
        registry.replace(c, Rc::new(wrong)).unwrap_err().kind,
        ErrorKind::Contract
    );
    let replacement = NativeProvider::new(c, "v2", |call: &Call<'_>, _: &[ValueWire]| {
        call.result(7_i64)
    });
    registry.replace(c, Rc::new(replacement)).unwrap();
    let arguments = [21_i64.into_wire(c.descriptor().parameters[0]).unwrap()];
    assert_eq!(run(&registry, c, &bundle, &arguments, &[]).unwrap(), 7);
    registry.remove(c).unwrap();
    let result = pinned
        .invoke(
            &arguments,
            &ExecutionContext {
                bundle: &bundle,
                conversions: None,
                references: &[],
            },
            &ExecutionLimits::default(),
            Budget::new(100_000),
        )
        .unwrap();
    assert_eq!(i64::from_wire(&result.evaluation.value).unwrap(), 42);
}
#[test]
fn shared_boundary_rejects_bad_arguments_results_and_caught_exhaustion() {
    let (bundle, contracts, _) = contract::contracts().unwrap();
    let c = &contracts[0];
    let arguments = [21_i64.into_wire(c.descriptor().parameters[0]).unwrap()];
    for exhausted in [false, true] {
        let mut registry = Registry::default();
        registry
            .install(
                c,
                Rc::new(NativeProvider::new(
                    c,
                    "bad",
                    move |call: &Call<'_>, _: &[ValueWire]| {
                        if exhausted {
                            let _ = call.budget.charge(u64::MAX);
                            call.result(1_i64)
                        } else {
                            call.result(true)
                        }
                    },
                )),
            )
            .unwrap();
        let e = run(&registry, c, &bundle, &arguments, &[]).unwrap_err();
        assert_eq!(
            e.kind,
            if exhausted {
                ErrorKind::ResourceExhausted
            } else {
                ErrorKind::Result
            }
        );
        assert!(run(&registry, c, &bundle, &[], &[]).is_err());
        let budget = Budget::new(100_000);
        budget.cancel();
        assert_eq!(
            registry
                .pin(c)
                .unwrap()
                .invoke(
                    &arguments,
                    &ExecutionContext {
                        bundle: &bundle,
                        conversions: None,
                        references: &[]
                    },
                    &ExecutionLimits::default(),
                    budget
                )
                .err()
                .unwrap()
                .kind,
            ErrorKind::Cancelled
        );
    }
}
#[cfg(feature = "rhai")]
#[test]
fn rhai_and_mixed_package_execute_exact_scalars_and_model_reads() {
    let (bundle, contracts, receiver) = contract::contracts().unwrap();
    for mixed in [false, true] {
        let mut registry = Registry::default();
        for (index, c) in contracts.iter().enumerate() {
            if mixed && index == 0 {
                generated::register_twice(&mut registry, c).unwrap();
            } else {
                registry
                    .install(
                        c,
                        Rc::new(
                            rhai::RhaiProvider::compile(
                                c,
                                "memory:behavior.rhai",
                                include_str!("../../../fixtures/rcore/bindings/behavior.rhai"),
                            )
                            .unwrap(),
                        ),
                    )
                    .unwrap();
            }
        }
        let c = &contracts[0];
        // Above JavaScript's exact integer range: transport must never use f64.
        let argument = 4_503_599_627_370_497_i64
            .into_wire(c.descriptor().parameters[0])
            .unwrap();
        assert_eq!(
            run(&registry, c, &bundle, &[argument], &[]).unwrap(),
            9_007_199_254_740_994
        );
        let c = &contracts[1];
        let (reference, argument) = model_argument(&bundle, c, &receiver);
        assert_eq!(
            run(&registry, c, &bundle, &[argument], &[reference]).unwrap(),
            42
        );
    }
}
#[cfg(feature = "rhai")]
#[test]
fn property_reads_share_model_admission_and_sticky_work_accounting() {
    let (bundle, contracts, receiver) = contract::contracts().unwrap();
    let c = &contracts[1];
    let (reference, argument) = model_argument(&bundle, c, &receiver);
    let mut registry = Registry::default();
    registry
        .install(
            c,
            Rc::new(
                rhai::RhaiProvider::compile(
                    c,
                    "memory:properties.rhai",
                    "fn binding_2(renamed) { renamed.value * 2 }",
                )
                .unwrap(),
            ),
        )
        .unwrap();
    assert_eq!(
        run(
            &registry,
            c,
            &bundle,
            std::slice::from_ref(&argument),
            std::slice::from_ref(&reference)
        )
        .unwrap(),
        42
    );
    assert!(run(&registry, c, &bundle, std::slice::from_ref(&argument), &[]).is_err());
    registry.replace(c, Rc::new(rhai::RhaiProvider::compile(c, "memory:budget.rhai",
        "fn binding_2(m) { try { loop { let value = m.value; } } catch (error) { 42 } }").unwrap())).unwrap();
    assert_eq!(
        run(&registry, c, &bundle, &[argument], &[reference])
            .unwrap_err()
            .kind,
        ErrorKind::ResourceExhausted
    );
}

#[cfg(feature = "rhai")]
#[test]
fn imported_ecore_narrow_integer_properties_widen_exactly() {
    use intersect_model::definitions::{
        bundle::{DefinitionBundle, DefinitionSource, SourceKind},
        rcore_behavior::{
            ir_v2::{EntrySelection, TypeNode},
            modules::SymbolId,
        },
    };
    // Int8/Int16 are imported Ecore capabilities; the bounded authored Rcore
    // structure profile currently exposes Int32/Int64 only.
    let model = DefinitionSource::new(
        "memory:bindings/model.ecore",
        SourceKind::Ecore,
        r#"
<ecore:EPackage xmlns:ecore="http://www.eclipse.org/emf/2002/Ecore"
 xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance" name="bindings"
 nsURI="urn:intersect:bindings" nsPrefix="m">
 <eClassifiers xsi:type="ecore:EClass" name="Meter">
  <eStructuralFeatures xsi:type="ecore:EAttribute" name="value" lowerBound="1"
   eType="ecore:EDataType http://www.eclipse.org/emf/2002/Ecore#//ELong"/>
  <eStructuralFeatures xsi:type="ecore:EAttribute" name="byte" lowerBound="1" defaultValueLiteral="127"
   eType="ecore:EDataType http://www.eclipse.org/emf/2002/Ecore#//EByte"/>
  <eStructuralFeatures xsi:type="ecore:EAttribute" name="short" lowerBound="1" defaultValueLiteral="-32768"
   eType="ecore:EDataType http://www.eclipse.org/emf/2002/Ecore#//EShort"/>
 </eClassifiers>
</ecore:EPackage>"#,
    );
    let report =
        intersect_model::migration::migrate_ecore(model, "memory:bindings/narrow.rcore").unwrap();
    let model = report
        .converted
        .unwrap_or_else(|| panic!("{:?}", report.issues));
    let mut entry = DefinitionSource::new(
        "memory:narrow.rcore",
        SourceKind::RcoreBehavior,
        "rcore 1; package narrow version \"1\"; import model \"urn:intersect:bindings\" as m; extern fn sum(meter:m.Meter)->Int64;",
    );
    entry.dependencies.push(model.id.clone());
    let bundle = DefinitionBundle {
        version: 1,
        sources: vec![model, entry.clone()],
        namespaces: vec![],
    };
    let contract = Contract::function(
        &bundle,
        &entry.id,
        &EntrySelection {
            symbol: SymbolId {
                source: entry.id.clone(),
                declaration: 1,
            },
            types: vec![],
            arguments: vec![],
        },
    )
    .unwrap();
    let TypeNode::Model(receiver) =
        &contract.descriptor().types[contract.descriptor().parameters[0].0 as usize]
    else {
        panic!("Expected model receiver");
    };
    let (reference, argument) = model_argument(&bundle, &contract, receiver);
    let mut registry = Registry::default();
    registry
        .install(
            &contract,
            Rc::new(
                rhai::RhaiProvider::compile(
                    &contract,
                    "memory:narrow.rhai",
                    "fn sum(meter) { meter.value + meter.byte + meter.short }",
                )
                .unwrap(),
            ),
        )
        .unwrap();
    assert_eq!(
        run(&registry, &contract, &bundle, &[argument], &[reference]).unwrap(),
        -32620
    );
}
#[cfg(feature = "rhai")]
#[test]
fn rhai_faults_are_bounded_and_failed_compile_preserves_registration() {
    let (bundle, contracts, _) = contract::contracts().unwrap();
    let c = &contracts[0];
    let mut registry = Registry::default();
    manual(&mut registry, c).unwrap();
    assert!(
        rhai::RhaiProvider::compile(c, "memory:bad.rhai", "let x=0; fn binding_1(a) {a}").is_err()
    );
    assert!(rhai::RhaiProvider::compile(c, "memory:bad.rhai", "fn binding_1(a) {").is_err());
    let arguments = [21_i64.into_wire(c.descriptor().parameters[0]).unwrap()];
    assert_eq!(run(&registry, c, &bundle, &arguments, &[]).unwrap(), 42);
    for (source, kind) in [
        ("fn binding_1(a) {loop {}}", ErrorKind::ResourceExhausted),
        ("fn binding_1(a) {true}", ErrorKind::Result),
        ("fn binding_1(a) {throw \"failure\";}", ErrorKind::Semantic),
    ] {
        registry
            .replace(
                c,
                Rc::new(rhai::RhaiProvider::compile(c, "memory:fault.rhai", source).unwrap()),
            )
            .unwrap();
        assert_eq!(
            run(&registry, c, &bundle, &arguments, &[])
                .unwrap_err()
                .kind,
            kind
        );
    }
}
