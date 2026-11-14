//! Verification consumer of host-owned transactions and read-only checked providers.
//! Candidate export/binding occurs outside invocation accounting. Rhai cannot edit
//! the model; cancellation below is delivered before entering the script body.
use crate::native;
use intersect_model::{
    core::{
        api::{BasicModel, ModelEdit, TransactionalModel},
        storage::{self, ModelRead},
    },
    definitions::{
        bundle::DefinitionBundle,
        compiler::materialize_runtime_bundle,
        conversion::{ConversionRegistry, ReferenceContext},
        rcore_behavior::ir_v2::{DataWire, ValueWire},
        scalars::int64_wire,
    },
    model_reference::ReferenceDomain,
    rcore_behavior_functions::{ExecutionContext, ExecutionLimits},
    semantic_bindings::{
        Budget, Call, Error as BindingError, ErrorKind, NativeProvider, NativeValue, Registry,
        rhai::RhaiProvider,
    },
};
use serde_json::{Value, json};
use std::{
    cell::{Cell, RefCell},
    error::Error,
    rc::Rc,
};

#[derive(Clone, Copy, PartialEq, Eq)]
enum Failure {
    None,
    Backend(ErrorKind),
    InvalidArguments,
    CancelBeforeBody,
    StructuralCommit,
    LanguageCommit,
    RustSemantic,
}

pub fn qualify_transactions() -> Result<Value, Box<dyn Error>> {
    // Authored guard adds a genuine mandatory-reference commit failure; the
    // original binding fixtures and their compiled contracts remain unchanged.
    let sources = intersect_model::definitions::semantic_bindings::compile_sources(
        include_str!("../../../fixtures/rcore/bindings/transaction-model.rcore"),
        include_str!("../../../fixtures/rcore/bindings/checked-behavior.rcore"),
    )?;
    let select =
        |index: usize| intersect_model::definitions::rcore_behavior::ir_v2::EntrySelection {
            symbol: sources.contracts[index].descriptor().symbol.clone(),
            types: vec![],
            arguments: vec![],
        };
    let entry = &sources.contracts[0].descriptor().symbol.source;
    let contracts = [
        sources.contracts[0].clone().with_conditions(
            &sources.bundle,
            entry,
            &[select(1)],
            &[select(2)],
        )?,
        sources.contracts[5]
            .clone()
            .with_conditions(&sources.bundle, entry, &[], &[select(6)])?,
    ];
    let receiver = sources
        .model_classes
        .get("Meter")
        .ok_or("Missing authored Meter")?
        .clone();
    let guard = sources
        .model_classes
        .get("TransactionGuard")
        .ok_or("Missing authored transaction guard")?
        .clone();
    let materialized = [receiver.clone(), guard.clone()];
    let bundle = sources.bundle;
    let model_source = bundle.sources.iter().find(|source| {
        matches!(receiver.expression(), intersect_model::definitions::generic_types::Type::Classifier { declaration, .. } if declaration.source == source.id)
    }).ok_or("Missing exact Meter structure source")?;
    let model_bundle = DefinitionBundle {
        version: 1,
        sources: vec![model_source.clone()],
        namespaces: vec![],
    };
    let schema = materialize_runtime_bundle(
        &model_bundle,
        &model_source.id,
        "urn:intersect:bindings",
        &materialized,
        None,
    )?;
    let class = schema.class_key_for_type(&receiver)?;
    let guard_class = schema.class_key_for_type(&guard)?;
    let mut rows = Vec::new();
    let mut success_work = None;
    let cases = [
        ("commit", "read_int(meter, \"value\") * 2", Failure::None),
        (
            "semantic",
            "throw (\"pending=\" + read_int(meter, \"value\").to_string());",
            Failure::Backend(ErrorKind::Semantic),
        ),
        (
            "result",
            "let pending = read_int(meter, \"value\"); if pending != 22 { throw \"wrong pending model\"; } pending == 22",
            Failure::Backend(ErrorKind::Result),
        ),
        (
            "postcondition",
            "let pending = read_int(meter, \"value\"); if pending != 22 { throw \"wrong pending model\"; } pending * 2 + 1",
            Failure::Backend(ErrorKind::Postcondition),
        ),
        (
            "exhaustion",
            "let pending = read_int(meter, \"value\"); if pending != 22 { throw \"wrong pending model\"; } try { loop {} } catch (error) { pending * 2 }",
            Failure::Backend(ErrorKind::ResourceExhausted),
        ),
        (
            "arguments",
            "read_int(meter, \"value\") * 2",
            Failure::InvalidArguments,
        ),
        (
            "cancel_before_body",
            "read_int(meter, \"value\") * 2",
            Failure::CancelBeforeBody,
        ),
        (
            "structural_commit",
            "read_int(meter, \"value\") * 2",
            Failure::StructuralCommit,
        ),
        (
            "language_commit",
            "read_int(meter, \"value\") * 2",
            Failure::LanguageCommit,
        ),
        (
            "rust_semantic_after_rhai",
            "read_int(meter, \"value\") * 2",
            Failure::RustSemantic,
        ),
    ];
    for (name, body, failure) in cases {
        let mut basic = BasicModel::new(schema.clone())?;
        basic.create("meter", class, "memory:transaction")?;
        basic.set("meter", "value", int64_wire(21))?;
        basic.validate()?;
        let mut model = TransactionalModel::new(basic);
        let before = storage::export(&model)?;
        let mut registry = Registry::default();
        registry.install(
            &contracts[0],
            Rc::new(NativeProvider::new(
                &contracts[0],
                "transaction-fixture-native-twice-v1",
                |call: &Call<'_>, arguments: &[ValueWire]| {
                    call.result(native::twice(call, call.argument::<i64>(0, arguments)?)?)
                },
            )),
        )?;
        let source = format!("fn {}(meter) {{ {body} }}", contracts[1].script_name());
        registry.install(
            &contracts[1],
            Rc::new(RhaiProvider::compile(
                &contracts[1],
                &format!("memory:transaction/{name}.rhai"),
                &source,
            )?),
        )?;
        let identity = registry.pin(&contracts[1])?.identity().clone();
        let budget = Budget::new(1_000_000);
        let observed = RefCell::new(json!({}));
        let validated = Cell::new(false);
        let transaction = model.transaction_with(
            |edit| {
                edit.set("meter", "value", int64_wire(22))?;
                edit.create_resource("memory:pending-transaction")?;
                edit.create("pending", class, "memory:pending-transaction")?;
                edit.set("pending", "value", int64_wire(7))?;
                let candidate = storage::export(edit)?;
                if candidate
                    .elements
                    .iter()
                    .find(|e| e.id == "meter")
                    .is_none_or(|e| e.values.get("value") != Some(&int64_wire(22)))
                {
                    return Err("Missing pending Meter edit".into());
                }
                observed.borrow_mut()["pending"] =
                    serde_json::to_value(&candidate).map_err(|e| e.to_string())?;
                let reference = ReferenceContext {
                    bundle: model_bundle.clone(),
                    entry: model_source.id.clone(),
                    namespace: "urn:intersect:bindings".into(),
                    conversions: vec![],
                    materialized_types: materialized.to_vec(),
                    applied_conversions: vec![],
                    constructors: vec![],
                    model: serde_json::to_value(candidate).map_err(|e| e.to_string())?,
                };
                let payload = ReferenceDomain::new_runtime(
                    receiver.clone(),
                    reference.clone(),
                    &ConversionRegistry::default(),
                )
                .and_then(|domain| domain.bind("meter"))
                .map_err(|e| e.to_string())?;
                let references = [reference];
                let context = ExecutionContext {
                    bundle: &bundle,
                    conversions: None,
                    references: &references,
                };
                let limits = ExecutionLimits::default();
                let function_argument = 22_i64
                    .into_wire(contracts[0].descriptor().parameters[0])
                    .map_err(|e| e.to_string())?;
                let rust_result = registry
                    .pin(&contracts[0])
                    .and_then(|provider| {
                        provider.invoke(&[function_argument], &context, &limits, budget.clone())
                    })
                    .map_err(|e| e.to_string())?;
                let rust_value =
                    i64::from_wire(&rust_result.evaluation.value).map_err(|e| e.to_string())?;
                if rust_value != 44 {
                    return Err("Rust result differs from independent expected 44".into());
                }
                observed.borrow_mut()["rust_result"] = json!(rust_value);
                if failure == Failure::CancelBeforeBody {
                    budget.cancel();
                }
                let argument = if failure == Failure::InvalidArguments {
                    true.into_wire(contracts[1].descriptor().parameters[0])
                        .map_err(|e| e.to_string())?
                } else {
                    ValueWire {
                        ty: contracts[1].descriptor().parameters[0],
                        data: DataWire::Model(payload),
                    }
                };
                let answer = registry.pin(&contracts[1]).and_then(|provider| {
                    provider.invoke(&[argument], &context, &limits, budget.clone())
                });
                match answer {
                    Err(error) => {
                        let expected = match failure {
                            Failure::Backend(kind) => kind,
                            Failure::InvalidArguments => ErrorKind::Arguments,
                            Failure::CancelBeforeBody => ErrorKind::Cancelled,
                            _ => return Err(format!("Unexpected provider failure: {error}")),
                        };
                        if error.kind != expected {
                            return Err(format!(
                                "Expected {expected:?}, received {:?}",
                                error.kind
                            ));
                        }
                        if expected == ErrorKind::Semantic && !error.message.contains("pending=22")
                        {
                            return Err(
                                "Semantic failure did not observe the actual pending value".into(),
                            );
                        }
                        observed.borrow_mut()["provider_error"] =
                            json!({"kind": error.kind, "message": error.message});
                        Err(format!("expected provider failure: {expected:?}"))
                    }
                    Ok(result) => {
                        if !matches!(
                            failure,
                            Failure::None
                                | Failure::StructuralCommit
                                | Failure::LanguageCommit
                                | Failure::RustSemantic
                        ) {
                            return Err("Expected provider failure was accepted".into());
                        }
                        let value =
                            i64::from_wire(&result.evaluation.value).map_err(|e| e.to_string())?;
                        if value != 44 {
                            return Err("Rhai did not read pending value 22 and return 44".into());
                        }
                        observed.borrow_mut()["rhai_result"] = json!(value);
                        if failure == Failure::RustSemantic {
                            registry
                                .replace(
                                    &contracts[0],
                                    Rc::new(NativeProvider::new(
                                        &contracts[0],
                                        "fixture-native-failure-after-pending-rhai",
                                        |call: &Call<'_>, _: &[ValueWire]| {
                                            call.budget.charge(7)?;
                                            Err(BindingError::new(
                                                ErrorKind::Semantic,
                                                "fixture native rejection after pending query",
                                            ))
                                        },
                                    )),
                                )
                                .map_err(|error| error.to_string())?;
                            let argument = 22_i64
                                .into_wire(contracts[0].descriptor().parameters[0])
                                .map_err(|error| error.to_string())?;
                            let error = registry
                                .pin(&contracts[0])
                                .and_then(|provider| {
                                    provider.invoke(&[argument], &context, &limits, budget.clone())
                                })
                                .err()
                                .ok_or("Failing native backend was accepted")?;
                            if error.kind != ErrorKind::Semantic
                                || error.message != "fixture native rejection after pending query"
                            {
                                return Err(format!("Unexpected native failure: {error}"));
                            }
                            observed.borrow_mut()["rust_error"] =
                                json!({"kind":error.kind,"message":error.message});
                            return Err("expected native semantic failure".into());
                        }
                        if failure == Failure::StructuralCommit {
                            edit.create("missing_required", guard_class, "memory:transaction")?;
                            observed.borrow_mut()["before_structural_validation"] =
                                serde_json::to_value(storage::export(edit)?)
                                    .map_err(|error| error.to_string())?;
                        }
                        observed.borrow_mut()["edit_callback_return"] = json!(value);
                        Ok(value)
                    }
                }
            },
            |edit| {
                validated.set(true);
                if edit.get("pending")?.is_none() {
                    return Err("Validator missed pending object".into());
                }
                if failure == Failure::LanguageCommit {
                    Err("fixture language invariant rejected".into())
                } else {
                    Ok(())
                }
            },
        );
        let after = storage::export(&model)?;
        let observation = observed.into_inner();
        if observation.get("rust_result") != Some(&json!(44)) {
            return Err("Rust checked function was not invoked".into());
        }
        let expected_provider_failure = match failure {
            Failure::Backend(kind) => Some(kind),
            Failure::InvalidArguments => Some(ErrorKind::Arguments),
            Failure::CancelBeforeBody => Some(ErrorKind::Cancelled),
            _ => None,
        };
        if let Some(expected) = expected_provider_failure {
            if observation.pointer("/provider_error/kind") != Some(&json!(expected))
                || transaction.as_ref().err()
                    != Some(&format!("expected provider failure: {expected:?}"))
            {
                return Err(format!(
                    "{name} failed before observing its required provider error: {transaction:?}"
                )
                .into());
            }
        } else if observation.get("rhai_result") != Some(&json!(44)) {
            return Err(format!(
                "{name} did not complete the checked Rhai model read: {transaction:?}"
            )
            .into());
        }
        if failure == Failure::RustSemantic
            && (observation.pointer("/rust_error/kind") != Some(&json!(ErrorKind::Semantic))
                || transaction.as_ref().err().map(String::as_str)
                    != Some("expected native semantic failure"))
        {
            return Err(format!(
                "Native failure case stopped before its required callback: {transaction:?}"
            )
            .into());
        }
        if matches!(
            failure,
            Failure::None | Failure::StructuralCommit | Failure::LanguageCommit
        ) && observation.get("edit_callback_return") != Some(&json!(44))
        {
            return Err("Expected edit callback success before commit validation".into());
        }
        if failure == Failure::StructuralCommit {
            let candidate: intersect_model::core::model::Model = serde_json::from_value(
                observation
                    .get("before_structural_validation")
                    .ok_or("Missing real pending guard snapshot")?
                    .clone(),
            )?;
            let incomplete = candidate
                .elements
                .iter()
                .find(|e| e.id == "missing_required")
                .ok_or("Mandatory-reference guard was not actually created")?;
            if incomplete.class != guard_class
                || incomplete.resource != "memory:transaction"
                || incomplete.values.get("required") != Some(&Value::Null)
                || candidate.elements.len() != 3
                || transaction.as_ref().err()
                    != Some(&format!("Multiplicity violation: {guard_class}.required"))
            {
                return Err(
                    "Expected the exact final structural issue for the pending guard".into(),
                );
            }
        }
        if failure == Failure::None {
            if transaction.as_ref().ok() != Some(&44)
                || !validated.get()
                || after == before
                || serde_json::to_value(&after)? != observation["pending"]
                || after.elements.len() != 2
                || after
                    .elements
                    .iter()
                    .find(|e| e.id == "meter")
                    .is_none_or(|e| e.values.get("value") != Some(&int64_wire(22)))
                || after
                    .elements
                    .iter()
                    .find(|e| e.id == "pending")
                    .is_none_or(|e| e.values.get("value") != Some(&int64_wire(7)))
            {
                return Err("Successful mixed transaction did not publish expected edits".into());
            }
            if budget.used() >= 500_000 {
                return Err("Positive read no longer leaves the exhaustion fixture's intended ample admission budget".into());
            }
            success_work = Some(budget.used());
        } else {
            if transaction.is_ok()
                || after != before
                || model.get("pending")?.is_some()
                || model.get("missing_required")?.is_some()
            {
                return Err(
                    format!("{name} did not roll back the complete model/resources").into(),
                );
            }
            let expected_validation = failure == Failure::LanguageCommit;
            if validated.get() != expected_validation {
                return Err(format!("Unexpected final validation phase for {name}").into());
            }
            if matches!(
                failure,
                Failure::Backend(ErrorKind::ResourceExhausted) | Failure::CancelBeforeBody
            ) {
                let expected = if failure == Failure::CancelBeforeBody {
                    ErrorKind::Cancelled
                } else {
                    ErrorKind::ResourceExhausted
                };
                if budget
                    .check()
                    .err()
                    .is_none_or(|error| error.kind != expected)
                {
                    return Err("Transaction handling cleared sticky binding failure".into());
                }
                if expected == ErrorKind::ResourceExhausted
                    && budget.used() <= success_work.ok_or("Missing positive work receipt")?
                {
                    return Err(
                        "Exhaustion fixture did not exceed successful model-read work".into(),
                    );
                }
            }
            if failure == Failure::StructuralCommit
                && !transaction
                    .as_ref()
                    .err()
                    .is_some_and(|e| e.contains("required"))
            {
                return Err("Expected structural mandatory-reference validation error".into());
            }
            if failure == Failure::LanguageCommit
                && transaction.as_ref().err().map(String::as_str)
                    != Some("fixture language invariant rejected")
            {
                return Err("Expected final language-validator error".into());
            }
        }
        model.validate()?;
        let recovery = if failure != Failure::None {
            model.transaction(|edit| {
                edit.set("meter", "value", int64_wire(23))?;
                Ok(())
            })?;
            model.validate()?;
            let exported = serde_json::to_value(storage::export(&model)?)?;
            let mut expected = serde_json::to_value(&before)?;
            let old_revision = expected["resources"]["memory:transaction"]["content_revision"]
                .as_str()
                .ok_or("Missing original resource revision")?
                .parse::<u64>()?;
            let new_revision = exported["resources"]["memory:transaction"]["content_revision"]
                .as_str()
                .ok_or("Missing recovery resource revision")?
                .parse::<u64>()?;
            if new_revision <= old_revision {
                return Err("Successful recovery did not advance its resource revision".into());
            }
            expected["elements"][0]["values"]["value"] = int64_wire(23);
            expected["resources"]["memory:transaction"]["content_revision"] =
                json!(new_revision.to_string());
            if exported != expected {
                return Err(
                    "Recovery changed more than the intended value and resource revision".into(),
                );
            }
            Some(exported)
        } else {
            None
        };
        rows.push(
            json!({"case":name,"before":before,"observed":observation,"after":after,
            "transaction":transaction,"language_validator_called":validated.get(),
            "binding_work":budget.used(),"provider":identity,"recovery_after_rollback":recovery}),
        );
    }
    Ok(
        json!({"scope":"Host-owned transactions around checked Rust/Rhai Pure/Query calls; snapshot export/binding is outside invocation accounting; no provider mutation or executing-script preemption.","cases":rows}),
    )
}

#[test]
fn checked_rhai_model_reads_participate_in_host_transaction_commit_and_rollback() {
    qualify_transactions().unwrap();
}
