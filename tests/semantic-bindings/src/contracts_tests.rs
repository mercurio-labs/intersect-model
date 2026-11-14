use intersect_model::{
    definitions::{
        rcore_behavior::{
            ir_v2::{EntrySelection, ValueWire},
            modules::SymbolId,
        },
        semantic_bindings::{Contract, SourceContracts, compile_sources},
    },
    rcore_behavior_functions::{ExecutionContext, ExecutionLimits},
    semantic_bindings::*,
};
use std::{cell::Cell, rc::Rc};

fn checked() -> (SourceContracts, Contract) {
    let sources = compile_sources(
        include_str!("../../../fixtures/rcore/bindings/model.rcore"),
        include_str!("../../../fixtures/rcore/bindings/checked-behavior.rcore"),
    )
    .unwrap();
    let c = &sources.contracts[0];
    let select = |i: usize| EntrySelection {
        symbol: sources.contracts[i].descriptor().symbol.clone(),
        types: vec![],
        arguments: vec![],
    };
    let c = c
        .clone()
        .with_conditions(
            &sources.bundle,
            &c.descriptor().symbol.source,
            &[select(1)],
            &[select(2)],
        )
        .unwrap();
    (sources, c)
}
fn invoke(
    s: &SourceContracts,
    c: &Contract,
    r: &Registry,
    value: i64,
    budget: Budget,
) -> Result<InvocationResult> {
    r.pin(c)?.invoke(
        &[value.into_wire(c.descriptor().parameters[0])?],
        &ExecutionContext::new(&s.bundle),
        &ExecutionLimits::default(),
        budget,
    )
}
#[test]
fn conditions_check_independent_answers_and_prevent_invalid_calls() {
    let (s, c) = checked();
    let count = Rc::new(Cell::new(0));
    let observed = count.clone();
    let mut registry = Registry::default();
    registry
        .install(
            &c,
            Rc::new(NativeProvider::new(
                &c,
                "checked-v1",
                move |call: &Call<'_>, args: &[ValueWire]| {
                    observed.set(observed.get() + 1);
                    crate::native::twice(call, i64::from_wire(&args[0])?)
                        .and_then(|v| call.result(v))
                },
            )),
        )
        .unwrap();
    for (input, expected) in [
        (0, 0),
        (21, 42),
        (4_503_599_627_370_497, 9_007_199_254_740_994),
    ] {
        assert_eq!(
            i64::from_wire(
                &invoke(&s, &c, &registry, input, Budget::new(100_000))
                    .unwrap()
                    .evaluation
                    .value
            )
            .unwrap(),
            expected
        );
    }
    let before = count.get();
    for input in [-1, i64::MAX] {
        let error = invoke(&s, &c, &registry, input, Budget::new(100_000)).unwrap_err();
        assert_eq!(error.kind, ErrorKind::Precondition);
        assert!(error.span.is_some());
    }
    assert_eq!(count.get(), before);
    registry
        .replace(
            &c,
            Rc::new(NativeProvider::new(
                &c,
                "incorrect",
                |call: &Call<'_>, _: &[ValueWire]| call.result(43_i64),
            )),
        )
        .unwrap();
    assert_eq!(
        invoke(&s, &c, &registry, 21, Budget::new(100_000))
            .unwrap_err()
            .kind,
        ErrorKind::Postcondition
    );
    registry.install_rcore(&s.contracts[0]).unwrap();
    registry
        .replace(
            &c,
            Rc::new(NativeProvider::new(
                &c,
                "wrong-type",
                |call: &Call<'_>, _: &[ValueWire]| call.result(true),
            )),
        )
        .unwrap();
    assert_eq!(
        invoke(&s, &c, &registry, 21, Budget::new(100_000))
            .unwrap_err()
            .kind,
        ErrorKind::Result
    );
}
#[test]
fn conditions_have_source_identity_signature_and_shared_budget() {
    let (s, c) = checked();
    assert_ne!(c.id(), s.contracts[0].id());
    for index in [3, 4] {
        let selection = EntrySelection {
            symbol: s.contracts[index].descriptor().symbol.clone(),
            types: vec![],
            arguments: vec![],
        };
        assert!(
            s.contracts[0]
                .clone()
                .with_conditions(&s.bundle, &c.descriptor().symbol.source, &[selection], &[])
                .is_err()
        );
    }
    let missing = EntrySelection {
        symbol: SymbolId {
            source: c.descriptor().symbol.source.clone(),
            declaration: 999,
        },
        types: vec![],
        arguments: vec![],
    };
    assert!(
        s.contracts[0]
            .clone()
            .with_conditions(&s.bundle, &c.descriptor().symbol.source, &[missing], &[])
            .is_err()
    );
    let mut registry = Registry::default();
    registry.install_rcore(&c).unwrap();
    assert_eq!(
        i64::from_wire(
            &invoke(&s, &c, &registry, 21, Budget::new(100_000))
                .unwrap()
                .evaluation
                .value
        )
        .unwrap(),
        42
    );
    assert_eq!(
        invoke(&s, &c, &registry, 21, Budget::new(1))
            .unwrap_err()
            .kind,
        ErrorKind::ResourceExhausted
    );
    let budget = Budget::new(100_000);
    budget.cancel();
    assert_eq!(
        invoke(&s, &c, &registry, 21, budget).unwrap_err().kind,
        ErrorKind::Cancelled
    );
}

#[test]
fn bodyless_model_operations_keep_the_exact_structural_signature() {
    let structure = include_str!("../../../fixtures/rcore/bindings/model.rcore");
    let behavior = "rcore 1; package externalops version \"1\"; import model \"urn:intersect:bindings\" as m; extern fn doubled(meter:m.Meter)->Int64;";
    let s = compile_sources(structure, behavior).unwrap();
    let c = &s.contracts[0];
    assert!(c.descriptor().operation.is_some());
    assert!(c.reference().is_none());
    assert!(compile_sources(structure, &behavior.replace("->Int64", "->Bool")).is_err());
}
