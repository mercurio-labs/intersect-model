//! One executable native/WASM consumer of the checked static/dynamic package.
use crate::{checked_contract, contract, generated, native};
use intersect_model::{
    core::model::Model,
    definitions::{
        bundle::DefinitionBundle,
        compiler::materialize_runtime_bundle,
        conversion::{ConversionRegistry, ReferenceContext},
        rcore_behavior::ir_v2::{DataWire, ValueWire},
    },
    model_reference::ReferenceDomain,
    rcore_behavior_functions::{ExecutionContext, ExecutionLimits},
    semantic_bindings::*,
};
use std::{num::NonZeroUsize, rc::Rc};

pub fn qualify() -> std::result::Result<(), Box<dyn std::error::Error>> {
    let (bundle, contracts, receiver) = checked_contract::contracts()?;
    if contracts
        .get(1)
        .is_none_or(|c| c.descriptor().operation.is_none())
    {
        return Err("Expected an exact structural operation binding".into());
    }
    let model_source=bundle.sources.iter().find(|s| matches!(receiver.expression(),intersect_model::definitions::generic_types::Type::Classifier {declaration,..} if declaration.source==s.id)).ok_or("Missing model source")?;
    let model_bundle = DefinitionBundle {
        version: 1,
        sources: vec![model_source.clone()],
        namespaces: vec![],
    };
    let schema = materialize_runtime_bundle(
        &model_bundle,
        &model_source.id,
        "urn:intersect:bindings",
        std::slice::from_ref(&receiver),
        None,
    )?;
    let mut model = Model::default();
    model.create(
        &schema,
        "meter",
        schema.class_key_for_type(&receiver)?,
        "memory:meter",
        0,
        0,
    )?;
    model.edit(
        &schema,
        &[(
            "meter".into(),
            "value".into(),
            intersect_model::definitions::scalars::int64_wire(21),
        )],
    )?;
    let reference = ReferenceContext {
        bundle: model_bundle,
        entry: model_source.id.clone(),
        namespace: "urn:intersect:bindings".into(),
        conversions: vec![],
        materialized_types: vec![receiver.clone()],
        applied_conversions: vec![],
        constructors: vec![],
        model: serde_json::to_value(model)?,
    };
    let payload =
        ReferenceDomain::new_runtime(receiver, reference.clone(), &ConversionRegistry::default())?
            .bind("meter")?;
    let references = [reference];
    let context = ExecutionContext {
        bundle: &bundle,
        conversions: None,
        references: &references,
    };
    let limits = ExecutionLimits::default();
    let c = &contracts[0];
    let mut checked = Registry::default();
    if checked
        .pin(c)
        .err()
        .ok_or("Missing provider was accepted")?
        .kind
        != ErrorKind::MissingImplementation
    {
        return Err("Incorrect missing-provider error".into());
    }
    generated::register_checked_package(&mut checked, &contracts)?;
    let invoke = |registry: &Registry, value: i64, budget: Budget| -> Result<InvocationResult> {
        registry.pin(c)?.invoke(
            &[value.into_wire(c.descriptor().parameters[0])?],
            &context,
            &limits,
            budget,
        )
    };
    for (value, budget, expected) in [
        (-1, Budget::new(100_000), ErrorKind::Precondition),
        (21, Budget::new(0), ErrorKind::ResourceExhausted),
    ] {
        if invoke(&checked, value, budget)
            .err()
            .ok_or("Invalid call succeeded")?
            .kind
            != expected
        {
            return Err("Incorrect contract failure".into());
        }
    }
    let cancelled = Budget::new(100_000);
    cancelled.cancel();
    if invoke(&checked, 21, cancelled)
        .err()
        .ok_or("Cancelled call succeeded")?
        .kind
        != ErrorKind::Cancelled
    {
        return Err("Incorrect cancellation failure".into());
    }
    checked.replace(
        c,
        Rc::new(NativeProvider::new(
            c,
            "incorrect",
            |call: &Call<'_>, _: &[ValueWire]| call.result(43_i64),
        )),
    )?;
    if invoke(&checked, 21, Budget::new(100_000))
        .err()
        .ok_or("Incorrect implementation accepted")?
        .kind
        != ErrorKind::Postcondition
    {
        return Err("Incorrect postcondition failure".into());
    }
    let wrong_args = [true.into_wire(c.descriptor().parameters[0])?];
    if checked
        .pin(c)?
        .invoke(&wrong_args, &context, &limits, Budget::new(100_000))
        .is_ok()
    {
        return Err("Invalid signature accepted".into());
    }
    let (_, unconditioned, _) = contract::contracts()?;
    if generated::register_checked_twice(&mut Registry::default(), &unconditioned[0])
        .err()
        .ok_or("Stale contract accepted")?
        .kind
        != ErrorKind::Contract
    {
        return Err("Incorrect stale-contract failure".into());
    }
    #[cfg(feature = "rhai")]
    {
        let script = format!("fn {}(value) {{ value * 2 }}", c.script_name());
        checked.replace(
            c,
            Rc::new(rhai::RhaiProvider::compile(
                c,
                "memory:checked.rhai",
                &script,
            )?),
        )?;
        if i64::from_wire(&invoke(&checked, 21, Budget::new(100_000))?.evaluation.value)? != 42 {
            return Err("Mixed Rhai function differs".into());
        }
        let op = &contracts[1];
        let result = checked.pin(op)?.invoke(
            &[ValueWire {
                ty: op.descriptor().parameters[0],
                data: DataWire::Model(payload.clone()),
            }],
            &context,
            &limits,
            Budget::new(100_000),
        )?;
        if i64::from_wire(&result.evaluation.value)? != 42 {
            return Err("Mixed Rust operation differs".into());
        }
        qualify_rhai_controls(
            &contracts,
            &[ValueWire {
                ty: op.descriptor().parameters[0],
                data: DataWire::Model(payload.clone()),
            }],
            &context,
            &limits,
        )?;
    }
    for static_binding in [true, false] {
        let mut registry = Registry::default();
        if static_binding {
            generated::register_checked_package(&mut registry, &contracts)?;
        } else {
            registry.install(
                &contracts[0],
                Rc::new(NativeProvider::new(
                    &contracts[0],
                    "manual-v1",
                    |call: &Call<'_>, args: &[ValueWire]| {
                        call.result(native::twice(call, call.argument(0, args)?)?)
                    },
                )),
            )?;
            registry.install(
                &contracts[1],
                Rc::new(NativeProvider::new(
                    &contracts[1],
                    "manual-v1",
                    |call: &Call<'_>, _: &[ValueWire]| {
                        call.result(native::doubled(call, call.model(0)?)?)
                    },
                )),
            )?;
        }
        for (i, c) in contracts.iter().enumerate() {
            let args = if i == 0 {
                vec![21_i64.into_wire(c.descriptor().parameters[0])?]
            } else {
                vec![ValueWire {
                    ty: c.descriptor().parameters[0],
                    data: DataWire::Model(payload.clone()),
                }]
            };
            let pinned = registry.pin(c)?;
            let budget = Budget::new(100_000);
            let mut session = pinned.start(&args, &context, &limits, budget.clone())?;
            if !matches!(session.resume(NonZeroUsize::MIN), BindingProgress::Pending) {
                return Err("Expected suspended admission".into());
            }
            // Registry removal must leave an already started call intact.
            registry.remove(c)?;
            loop {
                match session.resume(NonZeroUsize::MIN) {
                    BindingProgress::Pending => {}
                    BindingProgress::Complete(result) => {
                        if i64::from_wire(&(*result)?.evaluation.value)? != 42 {
                            return Err("Independent expected result differs".into());
                        }
                        break;
                    }
                    BindingProgress::Finished => return Err("Premature finish".into()),
                }
            }
            if !matches!(session.resume(NonZeroUsize::MIN), BindingProgress::Finished) {
                return Err("Repeated execution".into());
            }
        }
    }
    let (typed_bundle, typed, _) = contract::compile(
        include_str!("../../../fixtures/rcore/bindings/model.rcore"),
        include_str!("../../../fixtures/rcore/bindings/typed-behavior.rcore"),
    )?;
    let mut registry = Registry::default();
    generated::register_numbers(&mut registry, &typed[0])?;
    generated::register_optional(&mut registry, &typed[1])?;
    let budget = Budget::new(100_000);
    let args = [vec![i64::MIN, 0, i64::MAX].into_typed(
        typed[0].descriptor().parameters[0],
        &typed[0].descriptor().types,
        &budget,
    )?];
    let result = registry.pin(&typed[0])?.invoke(
        &args,
        &ExecutionContext::new(&typed_bundle),
        &limits,
        budget.clone(),
    )?;
    if Vec::<i64>::from_typed(
        &result.evaluation.value,
        &typed[0].descriptor().types,
        &budget,
    )? != vec![i64::MIN, 0, i64::MAX]
    {
        return Err("Collection changed".into());
    }
    let args = [Some(i32::MIN).into_typed(
        typed[1].descriptor().parameters[0],
        &typed[1].descriptor().types,
        &budget,
    )?];
    let result = registry.pin(&typed[1])?.invoke(
        &args,
        &ExecutionContext::new(&typed_bundle),
        &limits,
        budget.clone(),
    )?;
    if Option::<i32>::from_typed(
        &result.evaluation.value,
        &typed[1].descriptor().types,
        &budget,
    )? != Some(i32::MIN)
    {
        return Err("Optional value changed".into());
    }
    // Identical primitive representations never erase custom datatype identity.
    let nominal = intersect_model::definitions::semantic_bindings::compile_sources(
        "rcore 1; package nominal version \"1\"; metamodel \"urn:nominal\" prefix n { datatype A instanceClass \"int\" {} datatype B instanceClass \"int\" {} }",
        "rcore 1; package checks version \"1\"; import model \"urn:nominal\" as m; extern fn identity(value:m.A)->m.A; extern fn wrong(value:m.A)->m.B;",
    )?;
    for (index, c) in nominal.contracts.iter().enumerate() {
        let ty = c.descriptor().parameters[0];
        let Some(intersect_model::definitions::rcore_behavior::ir_v2::TypeNode::Datatype(id)) =
            c.descriptor().types.get(ty.0 as usize)
        else {
            return Err("Expected nominal type".into());
        };
        let argument = ValueWire {
            ty,
            data: DataWire::Datatype(
                serde_json::json!({"kind":"datatype","datatype":id,"value":7}),
            ),
        };
        let mut registry = Registry::default();
        registry.install(
            c,
            Rc::new(NativeProvider::new(
                c,
                "nominal-v1",
                |call: &Call<'_>, args: &[ValueWire]| {
                    call.result(call.argument::<NominalValue>(0, args)?)
                },
            )),
        )?;
        let result = registry.pin(c)?.invoke(
            std::slice::from_ref(&argument),
            &ExecutionContext::new(&nominal.bundle),
            &limits,
            Budget::new(100_000),
        );
        if index == 0 {
            if result?.evaluation.value.data != argument.data {
                return Err("Nominal representation changed".into());
            }
        } else if result.err().ok_or("Cross-nominal result accepted")?.kind != ErrorKind::Result {
            return Err("Incorrect nominal failure".into());
        }
    }
    Ok(())
}
#[test]
fn executable_qualification() {
    qualify().unwrap();
}

// Verification only: the same read-only provider controls run natively and in WASM.
// Cancellation below occurs between dispatch slices; it does not preempt Rhai.
#[cfg(feature = "rhai")]
fn qualify_rhai_controls(
    contracts: &[Contract],
    model_arguments: &[ValueWire],
    context: &ExecutionContext<'_>,
    limits: &ExecutionLimits,
) -> std::result::Result<(), Box<dyn std::error::Error>> {
    let function = &contracts[0];
    let operation = &contracts[1];
    let mut registry = Registry::default();
    for (contract, parameter, body) in [
        (function, "value", "value * 2"),
        (operation, "meter", "read_int(meter, \"value\") * 2"),
    ] {
        let source = format!("fn {}({parameter}) {{ {body} }}", contract.script_name());
        registry.install(
            contract,
            Rc::new(rhai::RhaiProvider::compile(
                contract,
                "memory:qualification.rhai",
                &source,
            )?),
        )?;
    }
    let arguments = [21_i64.into_wire(function.descriptor().parameters[0])?];
    let invoke =
        |registry: &Registry, contract: &Contract, values: &[ValueWire], budget: Budget| {
            registry
                .pin(contract)?
                .invoke(values, context, limits, budget)
        };
    for (contract, values) in [
        (function, arguments.as_slice()),
        (operation, model_arguments),
    ] {
        if i64::from_wire(
            &invoke(&registry, contract, values, Budget::new(100_000))?
                .evaluation
                .value,
        )? != 42
        {
            return Err("All-Rhai independent expected result differs".into());
        }
    }
    let original_identity = registry.pin(function)?.identity().clone();
    for source in [
        format!("fn {}(value) {{", function.script_name()),
        format!(
            "let initialized = 1; fn {}(value) {{ value * 2 }}",
            function.script_name()
        ),
    ] {
        if rhai::RhaiProvider::compile(function, "memory:invalid.rhai", &source).is_ok() {
            return Err("Invalid Rhai activation accepted".into());
        }
        if registry.pin(function)?.identity() != &original_identity
            || i64::from_wire(
                &invoke(&registry, function, &arguments, Budget::new(100_000))?
                    .evaluation
                    .value,
            )? != 42
        {
            return Err("Failed Rhai compilation changed the active implementation".into());
        }
    }
    let wrong_arguments = [true.into_wire(function.descriptor().parameters[0])?];
    if invoke(&registry, function, &wrong_arguments, Budget::new(100_000))
        .err()
        .ok_or("Invalid Rhai arguments accepted")?
        .kind
        != ErrorKind::Arguments
    {
        return Err("Incorrect Rhai argument failure".into());
    }
    let budget = Budget::new(100_000);
    let pinned = registry.pin(function)?;
    let mut session = pinned.start(&arguments, context, limits, budget.clone())?;
    if !matches!(session.resume(NonZeroUsize::MIN), BindingProgress::Pending) {
        return Err("Expected suspended Rhai admission".into());
    }
    budget.cancel();
    match session.resume(NonZeroUsize::MIN) {
        BindingProgress::Complete(result) => {
            if result.err().ok_or("Cancelled Rhai call succeeded")?.kind != ErrorKind::Cancelled {
                return Err("Incorrect suspended Rhai cancellation".into());
            }
        }
        _ => return Err("Cancelled Rhai call did not complete".into()),
    }
    if !matches!(session.resume(NonZeroUsize::MIN), BindingProgress::Finished) {
        return Err("Cancelled Rhai call executed again".into());
    }
    for (body, expected) in [
        ("true", ErrorKind::Result),
        ("throw \"qualification failure\";", ErrorKind::Semantic),
        ("43", ErrorKind::Postcondition),
        (
            "try { loop {} } catch (error) { 42 }",
            ErrorKind::ResourceExhausted,
        ),
    ] {
        let source = format!("fn {}(value) {{ {body} }}", function.script_name());
        registry.replace(
            function,
            Rc::new(rhai::RhaiProvider::compile(
                function,
                "memory:failure.rhai",
                &source,
            )?),
        )?;
        let budget = Budget::new(100_000);
        if invoke(&registry, function, &arguments, budget.clone())
            .err()
            .ok_or("Invalid Rhai result accepted")?
            .kind
            != expected
        {
            return Err(format!("Incorrect Rhai failure: expected {expected:?}").into());
        }
        if expected == ErrorKind::ResourceExhausted
            && budget
                .check()
                .err()
                .ok_or("Rhai exhaustion was not sticky")?
                .kind
                != expected
        {
            return Err("Incorrect sticky Rhai exhaustion".into());
        }
    }
    Ok(())
}
