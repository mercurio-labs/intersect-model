use intersect_model::{
    definitions::{
        rcore_behavior::ir_v2::{DataWire, TypeNode, ValueWire},
        semantic_bindings::compile_sources,
    },
    rcore_behavior_functions::{ExecutionContext, ExecutionLimits},
    semantic_bindings::*,
};
use std::rc::Rc;

fn roundtrip<T: NativeValue + Clone + std::fmt::Debug + PartialEq + 'static>(name: &str, value: T) {
    let behavior =
        format!("rcore 1; package values version \"1\"; extern fn identity(value:{name})->{name};");
    let s = compile_sources(
        include_str!("../../../fixtures/rcore/bindings/model.rcore"),
        &behavior,
    )
    .unwrap();
    let c = &s.contracts[0];
    let budget = Budget::new(100_000);
    let argument = value
        .clone()
        .into_typed(c.descriptor().parameters[0], &c.descriptor().types, &budget)
        .unwrap();
    let mut registry = Registry::default();
    registry
        .install(
            c,
            Rc::new(NativeProvider::new(
                c,
                "identity-v1",
                |call: &Call<'_>, args: &[ValueWire]| call.result(call.argument::<T>(0, args)?),
            )),
        )
        .unwrap();
    let result = registry
        .pin(c)
        .unwrap()
        .invoke(
            &[argument],
            &ExecutionContext::new(&s.bundle),
            &ExecutionLimits::default(),
            budget.clone(),
        )
        .unwrap();
    assert_eq!(
        T::from_typed(&result.evaluation.value, &c.descriptor().types, &budget).unwrap(),
        value
    );
}
#[test]
fn exact_native_integers_options_and_sequences_roundtrip() {
    roundtrip("Int8", i8::MIN);
    roundtrip("Int16", i16::MAX);
    roundtrip("Int32", i32::MIN);
    roundtrip("Int64", i64::MAX);
    roundtrip("UInt8", u8::MAX);
    roundtrip("UInt16", u16::MAX);
    roundtrip("UInt32", u32::MAX);
    roundtrip("UInt64", u64::MAX);
    roundtrip("Unit", ());
    roundtrip("Sequence<Int64>", vec![i64::MIN, 0, i64::MAX]);
    roundtrip("Option<Int32>", Some(i32::MIN));
    roundtrip("Option<Int32>", None::<i32>);
    roundtrip("Sequence<Option<Int32>>", vec![Some(1_i32), None]);
}
#[test]
fn collection_kind_and_duplicates_are_not_silently_changed() {
    let s = compile_sources(
        include_str!("../../../fixtures/rcore/bindings/model.rcore"),
        "rcore 1; package values version \"1\"; extern fn identity(value:Set<Int64>)->Set<Int64>;",
    )
    .unwrap();
    let c = &s.contracts[0];
    let ty = c.descriptor().parameters[0];
    let budget = Budget::new(100_000);
    assert!(
        vec![1_i64]
            .into_typed(ty, &c.descriptor().types, &budget)
            .is_err()
    );
    let argument = Set(vec![1_i64, 2])
        .into_typed(ty, &c.descriptor().types, &budget)
        .unwrap();
    let mut registry = Registry::default();
    registry
        .install(
            c,
            Rc::new(NativeProvider::new(
                c,
                "duplicates",
                |call: &Call<'_>, _: &[ValueWire]| call.result(Set(vec![1_i64, 1])),
            )),
        )
        .unwrap();
    assert_eq!(
        registry
            .pin(c)
            .unwrap()
            .invoke(
                &[argument],
                &ExecutionContext::new(&s.bundle),
                &ExecutionLimits::default(),
                budget
            )
            .unwrap_err()
            .kind,
        ErrorKind::Result
    );
}
#[test]
fn custom_datatype_representations_preserve_exact_nominal_identity() {
    let structure = "rcore 1; package nominal version \"1\"; metamodel \"urn:nominal\" prefix n { datatype A instanceClass \"int\" {} datatype B instanceClass \"int\" {} }";
    let behavior = "rcore 1; package checks version \"1\"; import model \"urn:nominal\" as m; extern fn identity(value:m.A)->m.A; extern fn wrong(value:m.A)->m.B;";
    let s = compile_sources(structure, behavior).unwrap();
    for (index, c) in s.contracts.iter().enumerate() {
        let ty = c.descriptor().parameters[0];
        let TypeNode::Datatype(id) = &c.descriptor().types[ty.0 as usize] else {
            panic!("Expected datatype");
        };
        let argument = ValueWire {
            ty,
            data: DataWire::Datatype(
                serde_json::json!({"kind":"datatype","datatype":id,"value":7}),
            ),
        };
        let mut registry = Registry::default();
        registry
            .install(
                c,
                Rc::new(NativeProvider::new(
                    c,
                    "nominal",
                    |call: &Call<'_>, args: &[ValueWire]| {
                        call.result(call.argument::<NominalValue>(0, args)?)
                    },
                )),
            )
            .unwrap();
        let result = registry.pin(c).unwrap().invoke(
            &[argument],
            &ExecutionContext::new(&s.bundle),
            &ExecutionLimits::default(),
            Budget::new(100_000),
        );
        if index == 0 {
            assert!(result.is_ok(), "{result:?}");
        } else {
            assert_eq!(result.unwrap_err().kind, ErrorKind::Result);
        }
    }
}
