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
fn comprehensions_preserve_dependent_generators_filters_and_lexical_scope() {
    for body in [
        "fn run()->Bool=first([for x in [1,2] yield x+1])==Some(2);",
        "fn run()->Bool=size([for x in [1,2], y in [x,x+1] yield x+y])==4;",
        "fn run()->Bool=fold([for x in [1,2], y in [x,x+1] where y>x yield x+y],0,|a,x|a+x)==8;",
        "fn run()->Bool={let x=[1,2];first([for x in x yield x+1])==Some(2)};",
        "fn run()->Bool=first([for x in [1], x in [x+1] yield x])==Some(2);",
        "fn run()->Bool={let n=3;(first([for x in [1] yield (|y:Int|x+y+n)])??(|y:Int|0))(2)==6};",
        "fn run()->Bool=first([for x in map([1],|a|a+1) yield x])==Some(2);",
        "fn run()->Bool=first([for x in [1] yield size([for y in [x] yield y])])==Some(1);",
        "fn run()->Bool=isEmpty([for x in Sequence::<Int>([]), y in [1/0] where 1/0==0 yield x+y]);",
        "fn run()->Bool=isEmpty([for x in [1] where false yield 1/0]);",
        "fn run()->Bool=first([for x in [1] yield x])==Some(1);fn map(x:Int)->Int=0;fn flatMap(x:Int)->Int=0;",
    ] {
        assert_true(body);
    }
}
#[test]
fn comprehension_kinds_follow_every_generator_order_guarantee() {
    for outer in ["Sequence", "Bag", "Set", "OrderedSet"] {
        for inner in ["Sequence", "Bag", "Set", "OrderedSet"] {
            let ordered = ["Sequence", "OrderedSet"].contains(&outer)
                && ["Sequence", "OrderedSet"].contains(&inner);
            let result = if ordered { "Sequence" } else { "Bag" };
            let (bundle, source) = fixture(&format!(
                "fn run()->{result}<Int> =[for x in {outer}([1,2]), y in {inner}([3,4]) where x+y>0 yield x+y];"
            ));
            let image = compile(&bundle, &source);
            let output = evaluate(
                &image,
                &[],
                &ExecutionContext::new(&bundle),
                &ExecutionLimits::default(),
            )
            .unwrap()
            .value;
            let DataWire::Collection(values) = output.data else {
                panic!("Collection")
            };
            let actual: Vec<_> = values
                .iter()
                .map(|v| {
                    let DataWire::Scalar(v) = &v.data else {
                        panic!("Int")
                    };
                    v.payload["value"].as_str().unwrap()
                })
                .collect();
            assert_eq!(actual, vec!["4", "5", "5", "6"]);
        }
    }
}
#[test]
fn malformed_and_ill_typed_comprehensions_have_located_diagnostics() {
    for body in [
        "fn run()->Sequence<Int> =[for x [1] yield x];",
        "fn run()->Sequence<Int> =[for x in [1] x];",
        "fn run()->Sequence<Int> =[for x in [1], yield x];",
        "fn run()->Sequence<Int> =[for x in 1 yield x];",
        "fn run()->Sequence<Int> =[for x in [1] where 1 yield x];",
        "fn run()->Sequence<Int> =[for x in [y], y in [1] yield x];",
        "fn run()->Sequence<Int> =[for x in [x] yield x];",
        "fn run()->Sequence<Int> =[for x in [1] yield true];",
    ] {
        let (bundle, source) = fixture(body);
        let errors = compile_function(
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
        .unwrap_err();
        assert!(!errors.is_empty());
        assert!(
            errors
                .iter()
                .all(|e| e.span.source == source.id && e.span.end <= source.text.len())
        );
    }
}

use std::num::NonZeroUsize;
#[test]
fn comprehension_slices_limits_and_every_cancellation_boundary_preserve_shared_state() {
    struct Cancel;
    impl ExecutionControl for Cancel {
        fn cancellation_requested(&self) -> bool {
            true
        }
    }
    let (bundle, source) = fixture(
        "fn run()->Sequence<Int> ={let n=3;[for x in [3,1,2,3,4,5,6,7,8,9,10,11] where x%n==0 yield x+n]};",
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
    for dimension in 0..4 {
        let amount = match dimension {
            0 => baseline.consumed.steps,
            1 => baseline.consumed.value_bytes,
            2 => baseline.consumed.value_nodes,
            3 => baseline.consumed.captures,
            _ => baseline.consumed.captures,
        };
        for bound in [0, amount - 1, amount, amount + 1] {
            let mut limits = ExecutionLimits::default();
            match dimension {
                0 => limits.scalar.max_steps = bound,
                1 => limits.scalar.max_value_bytes = bound,
                2 => limits.max_value_nodes = bound,
                3 => limits.max_captures = bound,
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
fn comprehension_failures_retain_generator_predicate_and_yield_source_locations() {
    for (body, fragment) in [
        (
            "fn run()->Sequence<Int> =[for x in [1], y in [10/(x-1)] yield y];",
            "10/(x-1)",
        ),
        (
            "fn run()->Sequence<Int> =[for x in [1] where 10/(x-1)>0 yield x];",
            "10/(x-1)",
        ),
        (
            "fn run()->Sequence<Int> =[for x in [1] yield 10/(x-1)];",
            "10/(x-1)",
        ),
    ] {
        let (bundle, source) = fixture(body);
        let image = compile(&bundle, &source);
        let error = evaluate(
            &image,
            &[],
            &ExecutionContext::new(&bundle),
            &ExecutionLimits::default(),
        )
        .unwrap_err();
        assert_eq!(error.kind, ExecutionErrorKind::Arithmetic);
        let span = error.span.unwrap();
        assert_eq!(span.source, source.id);
        assert_eq!(&source.text[span.start..span.end], "/");
        assert_eq!(span.start, source.text.find(fragment).unwrap() + 2);
    }
}

#[test]
fn contextual_yields_cross_generator_boundaries_without_inventing_order() {
    for outer in ["Sequence", "Bag", "Set", "OrderedSet"] {
        for inner in ["Sequence", "Bag", "Set", "OrderedSet"] {
            let kind = if ["Sequence", "OrderedSet"].contains(&outer)
                && ["Sequence", "OrderedSet"].contains(&inner)
            {
                "Sequence"
            } else {
                "Bag"
            };
            for (element, yielded) in [
                ("Option<Int>", "None"),
                ("Sequence<Int>", "[]"),
                ("Result<Int,String>", "Err(\"no\")"),
            ] {
                for predicate in ["", "where x+y>0"] {
                    let (bundle, source) = fixture(&format!(
                        "fn run()->{kind}<{element}> =[for x in {outer}([1]), y in {inner}([2]) {predicate} yield {yielded}];"
                    ));
                    let image = compile(&bundle, &source);
                    let value = evaluate(
                        &image,
                        &[],
                        &ExecutionContext::new(&bundle),
                        &ExecutionLimits::default(),
                    )
                    .unwrap()
                    .value;
                    let DataWire::Collection(values) = value.data else {
                        panic!("Collection")
                    };
                    assert_eq!(values.len(), 1);
                    assert_eq!(
                        serde_json::to_value(&image.program().types[value.ty.0 as usize]).unwrap()
                            ["Collection"]["kind"],
                        kind
                    );
                    match element {
                        "Option<Int>" => assert!(matches!(values[0].data, DataWire::None)),
                        "Sequence<Int>" => assert!(
                            matches!(&values[0].data,DataWire::Collection(xs) if xs.is_empty())
                        ),
                        _ => assert!(matches!(&values[0].data, DataWire::Err(_))),
                    }
                }
            }
        }
    }
}

#[test]
fn contextual_yields_remain_isolated_between_overload_candidates() {
    assert_true(
        "fn run()->Bool=choose([for x in [1], y in [2] where true yield if x>0 {None} else {Some(x+y)}]);fn choose(xs:Sequence<Option<Bool>>)->Bool=false;fn choose(xs:Sequence<Option<Int>>)->Bool=true;",
    );
    for body in [
        "fn run()->Bool=choose([for x in [1], y in [2] where true yield None]);fn choose(xs:Sequence<Option<Bool>>)->Bool=false;fn choose(xs:Sequence<Option<Int>>)->Bool=true;",
        "fn run()->Sequence<Option<Int>> =[for x in Bag([1]), y in [2] yield None];",
        "fn run()->Sequence<Option<Int>> =[for x in [1], y in [2] where false yield Some(true)];",
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
}

#[test]
fn generic_call_siblings_supply_comprehension_yield_context_in_both_orders() {
    for body in [
        "fn run()->Bool=check([for x in [1], y in [2] where true yield None],3);fn check<T>(xs:Sequence<Option<T>>,value:T)->Bool=size(xs)==1;",
        "fn run()->Bool=check(3,[for x in [1], y in [2] yield None]);fn check<T>(value:T,xs:Sequence<Option<T>>)->Bool=size(xs)==1;",
        "fn run()->Bool=check([for x in [1] where true yield x]);fn check<T>(xs:Sequence<T>)->Bool=size(xs)==1;",
    ] {
        assert_true(body);
    }
}
