use intersect_model::definitions::{
    bundle::{DefinitionBundle, DefinitionSource, SourceKind},
    rcore_behavior::{
        function_lowering::compile_function,
        ir_v2::{self, DataWire, EntrySelection, Limits},
        modules::SymbolId,
    },
};
use intersect_model::rcore_behavior_functions::{
    EvaluationSession, ExecutionContext, ExecutionControl, ExecutionErrorKind, ExecutionLimits,
    SessionProgress, evaluate,
};
use std::num::NonZeroUsize;

fn fixture(body: &str) -> (DefinitionBundle, DefinitionSource) {
    let source = DefinitionSource::new(
        "memory:sorting-behavior.rcore",
        SourceKind::RcoreBehavior,
        format!("rcore 1;package sorting version \"1\";{body}"),
    );
    (
        DefinitionBundle {
            version: 1,
            sources: vec![source.clone()],
            namespaces: vec![],
        },
        source,
    )
}
fn compile(bundle: &DefinitionBundle, source: &DefinitionSource) -> ir_v2::VerifiedExecutable {
    compile_function(
        bundle,
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
    .unwrap_or_else(|e| panic!("{}: {e:?}", source.text))
}
fn assert_true(body: &str) {
    let (bundle, source) = fixture(body);
    let image = compile(&bundle, &source);
    let result = evaluate(
        &image,
        &[],
        &ExecutionContext::new(&bundle),
        &ExecutionLimits::default(),
    )
    .unwrap_or_else(|e| panic!("{body}: {e:?}"));
    let DataWire::Scalar(value) = result.value.data else {
        panic!("Expected Bool")
    };
    assert_eq!(value.payload, serde_json::json!(true), "{body}");
}

#[test]
fn key_sort_covers_every_scalar_order_and_preserves_tied_wires() {
    use intersect_model::definitions::rcore_behavior::{ir, types::Scalar};
    use serde_json::json;
    let mut cases = vec![
        (
            Scalar::Bool,
            vec![json!(true), json!(false), json!(true)],
            vec![1, 0, 2],
        ),
        (
            Scalar::String,
            vec![json!("z"), json!(""), json!("é"), json!("a"), json!("aa")],
            vec![1, 3, 4, 0, 2],
        ),
        (
            Scalar::Bytes,
            vec![
                json!({"kind":"bytes","hex":"FF"}),
                json!({"kind":"bytes","hex":"00FF"}),
                json!({"kind":"bytes","hex":"00"}),
            ],
            vec![2, 1, 0],
        ),
        (
            Scalar::Char16,
            vec![
                json!({"kind":"char16","unit":65535}),
                json!({"kind":"char16","unit":0}),
                json!({"kind":"char16","unit":55296}),
            ],
            vec![1, 2, 0],
        ),
        (
            Scalar::DateInstant,
            vec![
                json!({"kind":"date","milliseconds":"9223372036854775807"}),
                json!({"kind":"date","milliseconds":"-9223372036854775808"}),
                json!({"kind":"date","milliseconds":"0"}),
            ],
            vec![1, 2, 0],
        ),
        (
            Scalar::Unit,
            vec![json!({"kind":"unit"}), json!({"kind":"unit"})],
            vec![0, 1],
        ),
        (
            Scalar::Int64,
            vec![
                json!({"kind":"int64","value":"9223372036854775807"}),
                json!({"kind":"int64","value":"-9223372036854775808"}),
            ],
            vec![1, 0],
        ),
        (
            Scalar::UInt64,
            vec![
                json!({"kind":"uint64","value":"18446744073709551615"}),
                json!({"kind":"uint64","value":"0"}),
            ],
            vec![1, 0],
        ),
        (
            Scalar::BigInt,
            vec![
                json!({"kind":"bigint","value":"123456789012345678901234567890"}),
                json!({"kind":"bigint","value":"-123456789012345678901234567890"}),
            ],
            vec![1, 0],
        ),
        (
            Scalar::Decimal,
            vec![
                json!({"kind":"decimal","coefficient":"10","scale":1}),
                json!({"kind":"decimal","coefficient":"-1","scale":-2147483648i64}),
                json!({"kind":"decimal","coefficient":"100","scale":2}),
            ],
            vec![1, 0, 2],
        ),
    ];
    for scalar in [Scalar::Int8, Scalar::Int16, Scalar::Int32] {
        cases.push((scalar, vec![json!(3), json!(-2), json!(0)], vec![1, 2, 0]));
    }
    for scalar in [Scalar::UInt8, Scalar::UInt16, Scalar::UInt32] {
        cases.push((scalar, vec![json!(3), json!(2), json!(0)], vec![2, 1, 0]));
    }
    for (scalar, kind, bits) in [
        (
            Scalar::Float32,
            "float32",
            vec![
                "7fc00001", "00000000", "80000000", "7f800000", "ff800000", "7fc00002",
            ],
        ),
        (
            Scalar::Float64,
            "float64",
            vec![
                "7ff8000000000001",
                "0000000000000000",
                "8000000000000000",
                "7ff0000000000000",
                "fff0000000000000",
                "7ff8000000000002",
            ],
        ),
    ] {
        cases.push((
            scalar,
            bits.into_iter()
                .map(|bits| json!({"kind":kind,"bits":bits}))
                .collect(),
            vec![4, 1, 2, 3, 0, 5],
        ));
    }
    for (scalar, payloads, order) in cases {
        let (bundle, source) = fixture(&format!(
            "fn run(xs:Sequence<{scalar:?}>)->Sequence<{scalar:?}> =sortedBy(xs,|x:{scalar:?}|x);"
        ));
        let image = compile(&bundle, &source);
        let program = image.program();
        let ty = program.functions[program.entry.0 as usize].parameters[0];
        let ir_v2::TypeNode::Collection { element, .. } = program.types[ty.0 as usize] else {
            panic!("Collection")
        };
        let values: Vec<_> = payloads
            .into_iter()
            .map(|payload| ir_v2::ValueWire {
                ty: element,
                data: DataWire::Scalar(ir::Value {
                    ty: ir::ValueType::Scalar(scalar),
                    payload,
                }),
            })
            .collect();
        let input = ir_v2::ValueWire {
            ty,
            data: DataWire::Collection(values.clone()),
        };
        let result = evaluate(
            &image,
            &[input],
            &ExecutionContext::new(&bundle),
            &ExecutionLimits::default(),
        )
        .unwrap_or_else(|e| panic!("{scalar:?}: {e:?}"));
        let DataWire::Collection(actual) = result.value.data else {
            panic!("Collection")
        };
        let expected: Vec<_> = order.into_iter().map(|i| &values[i]).collect();
        assert_eq!(
            serde_json::to_value(actual).unwrap(),
            serde_json::to_value(expected).unwrap(),
            "{scalar:?}"
        );
    }
}

#[test]
fn key_callbacks_are_checked_and_execute_on_singletons_but_not_empty_inputs() {
    for body in [
        "fn run()->Bool=isEmpty(sortedBy([],|x:Int|1/0));",
        "fn run()->Bool=first(sortedBy([3,1,2],key))==Some(1);fn key<T:Int>(x:T)->T=x;",
        "fn run()->Bool={let n= -1;first(sortedBy([1,3,2],|x:Int|n*x))==Some(3)};",
        "fn run()->Bool=first(sortedBy([3,1,2],|x:Int|first(sortedBy([x],|y:Int|y))??0))==Some(1);",
    ] {
        assert_true(body);
    }
    for body in [
        "fn run()->Sequence<Int> =sortedBy([1],|x:Int|Some(x));",
        "fn run()->Sequence<Int> =sortedBy([1],|x:Int|[x]);",
        "fn run()->Set<Int> =sortedBy([1],|x:Int|x);",
    ] {
        let (bundle, source) = fixture(body);
        assert!(
            compile_function(
                &bundle,
                &source.id,
                &EntrySelection {
                    symbol: SymbolId {
                        source: source.id.clone(),
                        declaration: 0
                    },
                    types: vec![],
                    arguments: vec![]
                },
                &Limits::default()
            )
            .is_err()
        );
    }
    let (bundle, source) = fixture("fn run()->Sequence<Int> =sortedBy([1],|x:Int|1/0);");
    let image = compile(&bundle, &source);
    assert_eq!(
        evaluate(
            &image,
            &[],
            &ExecutionContext::new(&bundle),
            &ExecutionLimits::default()
        )
        .unwrap_err()
        .kind,
        ExecutionErrorKind::Arithmetic
    );
}
#[test]
fn key_sorting_slices_limits_and_every_cancellation_boundary_preserve_shared_state() {
    struct Cancel;
    impl ExecutionControl for Cancel {
        fn cancellation_requested(&self) -> bool {
            true
        }
    }
    let (bundle, source) = fixture(
        "fn run()->Sequence<Int> ={let n=3;sortedBy([3,1,2,3,4,5,6,7,8,9,10,11],|x:Int|n*x)};",
    );
    let image = compile(&bundle, &source);
    let context = ExecutionContext::new(&bundle);
    let limits = ExecutionLimits::default();
    let baseline = evaluate(&image, &[], &context, &limits).unwrap();
    let expected = serde_json::to_value(&baseline).unwrap();
    let mut boundaries = 0;
    for quantum in [1, 3, 4096] {
        let mut job = EvaluationSession::new(&image, &[], &context, &limits).unwrap();
        let mut pending = 0;
        loop {
            match job.resume(NonZeroUsize::new(quantum).unwrap()) {
                SessionProgress::Pending => {
                    pending += 1;
                    assert!(pending < 10000)
                }
                SessionProgress::Complete(result) => {
                    assert_eq!(serde_json::to_value(result.unwrap()).unwrap(), expected);
                    break;
                }
                SessionProgress::Finished => panic!("Missing result"),
            }
        }
        if quantum == 1 {
            boundaries = pending;
        }
    }
    for boundary in 0..=boundaries {
        let mut job = EvaluationSession::new(&image, &[], &context, &limits).unwrap();
        for _ in 0..boundary {
            assert!(matches!(
                job.resume(NonZeroUsize::MIN),
                SessionProgress::Pending
            ))
        }
        let SessionProgress::Complete(result) = job.resume_with_control(NonZeroUsize::MIN, &Cancel)
        else {
            panic!("Missing cancellation")
        };
        assert_eq!(result.unwrap_err().kind, ExecutionErrorKind::Cancelled);
        assert!(matches!(
            job.resume(NonZeroUsize::MAX),
            SessionProgress::Finished
        ));
    }
    for dimension in 0..5 {
        let amount = match dimension {
            0 => baseline.consumed.steps,
            1 => baseline.consumed.value_bytes,
            2 => baseline.consumed.value_nodes,
            3 => baseline.consumed.collection_comparisons,
            _ => baseline.consumed.captures,
        };
        for bound in [0, amount - 1, amount, amount + 1] {
            let mut limits = ExecutionLimits::default();
            match dimension {
                0 => limits.scalar.max_steps = bound,
                1 => limits.scalar.max_value_bytes = bound,
                2 => limits.max_value_nodes = bound,
                3 => limits.max_collection_comparisons = bound,
                _ => limits.max_captures = bound,
            };
            let result = evaluate(&image, &[], &context, &limits);
            assert_eq!(
                result.is_ok(),
                bound >= amount,
                "dimension {dimension}, bound {bound}: {result:?}"
            );
            if let Err(error) = result {
                assert_eq!(error.kind, ExecutionErrorKind::ResourceExhausted)
            }
        }
    }
    assert_eq!(
        serde_json::to_value(evaluate(&image, &[], &context, &limits).unwrap()).unwrap(),
        expected
    );
}

#[test]
fn cached_keys_execute_once_and_numeric_ordering_observes_exact_work_budget() {
    let (bundle, source) =
        fixture("fn run()->Sequence<Int> ={let n=1;sortedBy([4,1,3,2],|x:Int|n*x)};");
    let image = compile(&bundle, &source);
    let context = ExecutionContext::new(&bundle);
    let baseline = evaluate(&image, &[], &context, &ExecutionLimits::default()).unwrap();
    // One captured slot at closure creation, then one for each of four key calls.
    assert_eq!(baseline.consumed.captures, 5);
    let work = baseline.consumed.numeric_work;
    for limit in [0, work - 1, work, work + 1] {
        let mut limits = ExecutionLimits::default();
        limits.scalar.numeric.max_work = limit;
        let result = evaluate(&image, &[], &context, &limits);
        assert_eq!(result.is_ok(), limit >= work, "limit {limit}: {result:?}");
        if let Err(error) = result {
            assert_eq!(error.kind, ExecutionErrorKind::ResourceExhausted);
        }
    }
}
