use intersect_model::definitions::{
    bundle::{DefinitionBundle, DefinitionSource, SourceKind},
    rcore_behavior::{
        function_lowering::compile_function,
        ir_v2::{self, DataWire, EntrySelection, Limits},
        modules::SymbolId,
    },
};
use intersect_model::rcore_behavior_functions::{ExecutionContext, ExecutionLimits, evaluate};

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
fn collection_callbacks_infer_input_types_without_requiring_output_context() {
    for body in [
        "fn run()->Bool=first(map([1,2],|x|x+1))==Some(2);",
        "fn run()->Bool=first(filter([1,2],|x|x>1))==Some(2);",
        "fn run()->Bool=first(flatMap([1,2],|x|map([x],|y|x+y)))==Some(2);",
        "fn run()->Bool=fold([1,2],0,|a,x|a+x)==3;",
        "fn run()->Bool=exists([1,2],|x|x==2) && forAll([1,2],|x|x>0) && one([1,2],|x|x==1);",
        "fn run()->Bool=find([1,2],|x|x==2)==Some(2);",
        "fn run()->Bool=size(groupBy([1,2,3],|x|x%2))==2;",
        "fn run()->Bool=first(sortedBy([3,1,2],|x|x))==Some(1);",
        "fn run()->Bool=first(sortedWith([3,1,2],|a,b|a-b))==Some(1);",
        "fn run()->Bool={let n=2;first(map([1],|x|{let f=|y:Int|x+y+n;f(3)}))==Some(6)};",
    ] {
        assert_true(body);
    }
}
#[test]
fn ordinary_function_contexts_and_generic_siblings_infer_lambda_parameters() {
    for body in [
        "fn run()->Bool={let f:(Int)->Int=|x|x+1;f(1)==2};",
        "fn run()->Bool=apply(3,|x|x+1)==4;fn apply(x:Int,f:(Int)->Int)->Int=f(x);",
        "fn run()->Bool=apply(|x|x+1,3)==4;fn apply<T:Int>(f:(T)->T,x:T)->T=f(x);",
        "fn run()->Bool=apply(3,|x|x+1)==4;fn apply<T:Int>(x:T,f:(T)->T)->T=f(x);",
        "fn run()->Bool={let make:(Int)->(Int)->Int=|x|(|y|x+y);make(2)(3)==5};",
        "fn run()->Bool=fold([1,2],0,|a:Int,x|a+x)==3;",
    ] {
        assert_true(body);
    }
}
#[test]
fn inference_never_fabricates_context_or_overrides_explicit_types() {
    for body in [
        "fn run()->Int={let f=|x|x;f(1)};",
        "fn run()->Sequence<Int> =map([],|x|x);",
        "fn run()->Sequence<Int> =map([1],|x:Bool|1);",
        "fn run()->Sequence<Int> =map([1],|x,y|x+y);",
        "fn run()->Int={let f:(Int)->Int=|x:Bool|1;f(2)};",
        "fn run(x)->Int=1;",
        "fn run()->Int=if true {1} else {{let f=|x|x;1}};",
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
        assert!(errors.iter().all(|e| e.span.source == source.id
            && e.span.start <= e.span.end
            && e.span.end <= source.text.len()));
    }
}
