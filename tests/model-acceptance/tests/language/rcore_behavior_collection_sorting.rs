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

fn run(body: &str) -> ir_v2::ValueWire {
    let (bundle, source) = fixture(body);
    let image = compile(&bundle, &source);
    evaluate(
        &image,
        &[],
        &ExecutionContext::new(&bundle),
        &ExecutionLimits::default(),
    )
    .unwrap_or_else(|e| panic!("{body}: {e:?}"))
    .value
}

#[test]
fn comparator_sort_preserves_occurrences_and_stable_representatives_for_every_kind() {
    for kind in ["Sequence", "Bag", "Set", "OrderedSet"] {
        let (bundle, source) = fixture(&format!(
            "fn run()->Sequence<Int> =sortedWith({kind}([3,1,2,1]),|a:Int,b:Int|a-b);"
        ));
        let image = compile(&bundle, &source);
        let result = evaluate(
            &image,
            &[],
            &ExecutionContext::new(&bundle),
            &ExecutionLimits::default(),
        )
        .unwrap();
        let DataWire::Collection(values) = result.value.data else {
            panic!("Collection")
        };
        let actual: Vec<_> = values
            .iter()
            .map(|v| {
                let DataWire::Scalar(v) = &v.data else {
                    panic!("Scalar")
                };
                v.payload["value"].as_str().unwrap().to_owned()
            })
            .collect();
        assert_eq!(
            actual,
            if ["Set", "OrderedSet"].contains(&kind) {
                vec!["1", "2", "3"]
            } else {
                vec!["1", "1", "2", "3"]
            }
        );
    }
    assert_true(
        "fn run()->Bool={let xs=sortedWith([(2,10),(1,20),(2,30),(1,40)],|a:(Int,Int),b:(Int,Int)|a[0]-b[0]);map(xs,|x:(Int,Int)|x[1])[0]==Some(20) && map(xs,|x:(Int,Int)|x[1])[1]==Some(40) && map(xs,|x:(Int,Int)|x[1])[2]==Some(10) && map(xs,|x:(Int,Int)|x[1])[3]==Some(30)};",
    );
    let DataWire::Collection(values) =
        run("fn run()->Sequence<Decimal> =sortedWith([1.0,1.00,1.000],|a:Decimal,b:Decimal|0);")
            .data
    else {
        panic!("Collection")
    };
    let scales: Vec<_> = values
        .iter()
        .map(|value| {
            let DataWire::Scalar(value) = &value.data else {
                panic!("Decimal")
            };
            value.payload["scale"].clone()
        })
        .collect();
    assert_eq!(
        scales,
        vec![
            serde_json::json!(1),
            serde_json::json!(2),
            serde_json::json!(3)
        ]
    );
}

#[test]
fn comparator_sort_handles_empty_singleton_inconsistent_and_nested_callbacks() {
    for body in [
        "fn run()->Bool=isEmpty(sortedWith([],|a:Int,b:Int|1/0));",
        "fn run()->Bool=first(sortedWith([7],|a:Int,b:Int|1/0))==Some(7);",
        "fn run()->Bool=size(sortedWith([4,3,2,1],|a:Int,b:Int|1))==4;",
        "fn run()->Bool=size(sortedWith([4,3,2,1],|a:Int,b:Int|-1))==4;",
        "fn run()->Bool={let direction= -1;first(sortedWith([1,3,2],|a:Int,b:Int|direction*(a-b)))==Some(3)};",
        "fn run()->Bool=first(sortedWith([3,1,2],compare))==Some(1);fn compare<T:Int>(a:T,b:T)->Int=a-b;",
        "fn run()->Bool=first(sortedWith([3,1,2],|a:Int,b:Int|size(sortedWith([a,b],|x:Int,y:Int|x-y))*(a-b)))==Some(1);",
    ] {
        assert_true(body);
    }
}

#[test]
fn comparator_sort_rejects_invalid_contracts_and_propagates_callback_failure() {
    for body in [
        "fn run()->Sequence<Int> =sortedWith([1,2],|a:Int|a);",
        "fn run()->Sequence<Int> =sortedWith([1,2],|a:Int,b:Int|true);",
        "fn run()->Sequence<Int> =sortedWith([1,2],|a:Bool,b:Bool|0);",
        "fn run()->Set<Int> =sortedWith([1,2],|a:Int,b:Int|a-b);",
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
    let (bundle, source) = fixture("fn run()->Sequence<Int> =sortedWith([1,2],|a:Int,b:Int|1/0);");
    let image = compile(&bundle, &source);
    let error = evaluate(
        &image,
        &[],
        &ExecutionContext::new(&bundle),
        &ExecutionLimits::default(),
    )
    .unwrap_err();
    assert_eq!(error.kind, ExecutionErrorKind::Arithmetic);
}
#[test]
fn sorting_slices_limits_and_every_cancellation_boundary_preserve_shared_state() {
    struct Cancel;
    impl ExecutionControl for Cancel {
        fn cancellation_requested(&self) -> bool {
            true
        }
    }
    let (bundle, source) = fixture(
        "fn run()->Sequence<Int> ={let n=3;sortedWith([3,1,2,3,4,5,6,7,8,9,10,11],|x:Int,y:Int|n*(x-y))};",
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
fn merge_passes_cover_empty_odd_power_of_two_and_partial_runs() {
    use intersect_model::definitions::rcore_behavior::{ir, types::Scalar};
    let (bundle, source) =
        fixture("fn run(xs:Sequence<Int>)->Sequence<Int> =sortedWith(xs,|a:Int,b:Int|a-b);");
    let image = compile(&bundle, &source);
    let program = image.program();
    let ty = program.functions[program.entry.0 as usize].parameters[0];
    let ir_v2::TypeNode::Collection { element, .. } = program.types[ty.0 as usize] else {
        panic!("Sequence")
    };
    for len in 0..=129 {
        let mut expected: Vec<i64> = (0..len)
            .map(|n| ((n * 37 + len * 19) % 23) as i64 - 11)
            .collect();
        let input = ir_v2::ValueWire {
            ty,
            data: DataWire::Collection(
                expected
                    .iter()
                    .map(|n| ir_v2::ValueWire {
                        ty: element,
                        data: DataWire::Scalar(ir::Value {
                            ty: ir::ValueType::Scalar(Scalar::Int64),
                            payload: serde_json::json!({"kind":"int64","value":n.to_string()}),
                        }),
                    })
                    .collect(),
            ),
        };
        expected.sort();
        let result = evaluate(
            &image,
            &[input],
            &ExecutionContext::new(&bundle),
            &ExecutionLimits::default(),
        )
        .unwrap();
        let DataWire::Collection(values) = result.value.data else {
            panic!("Collection")
        };
        let actual: Vec<i64> = values
            .iter()
            .map(|v| {
                let DataWire::Scalar(v) = &v.data else {
                    panic!("Int")
                };
                v.payload["value"].as_str().unwrap().parse().unwrap()
            })
            .collect();
        assert_eq!(actual, expected, "length {len}");
    }
}
