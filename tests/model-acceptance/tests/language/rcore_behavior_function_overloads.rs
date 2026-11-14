//! Actual overload execution, captures, exact selected identities and replay.
use intersect_model::definitions::{
    bundle::{DefinitionBundle, DefinitionSource, SourceKind},
    rcore_behavior::{
        function_lowering::compile_function,
        ir_v2::{DataWire, EntrySelection, FunctionIdentity, Limits, VerifiedExecutable},
        modules::SymbolId,
    },
};
use intersect_model::rcore_behavior_functions::{ExecutionContext, ExecutionLimits, evaluate};
fn source(name: &str, body: &str) -> DefinitionSource {
    DefinitionSource::new(
        format!("memory:{name}-behavior.rcore"),
        SourceKind::RcoreBehavior,
        format!("rcore 1;package {name} version \"1\";{body}"),
    )
}
fn bundle(sources: Vec<DefinitionSource>) -> DefinitionBundle {
    DefinitionBundle {
        version: 1,
        sources,
        namespaces: vec![],
    }
}
fn image(b: &DefinitionBundle, s: &DefinitionSource, decl: usize) -> VerifiedExecutable {
    compile_function(
        b,
        &s.id,
        &EntrySelection {
            symbol: SymbolId {
                source: s.id.clone(),
                declaration: decl,
            },
            types: vec![],
            arguments: vec![],
        },
        &Limits::default(),
    )
    .unwrap_or_else(|e| panic!("{}\n{e:?}", s.text))
}
fn number(b: &DefinitionBundle, p: &VerifiedExecutable) -> i64 {
    let result = evaluate(
        p,
        &[],
        &ExecutionContext::new(b),
        &ExecutionLimits::default(),
    )
    .unwrap();
    let DataWire::Scalar(value) = result.value.data else {
        panic!("expected scalar")
    };
    value.payload.as_i64().unwrap()
}
#[test]
fn local_overload_selection_executes_arity_context_and_source_order_independently() {
    for overloads in [
        "fn pick(x:Bool)->Int8=90;fn pick(x:Int8)->Int8=x+1;fn pick(x:Int8,b:Bool)->Int8=x+2;",
        "fn pick(x:Int8,b:Bool)->Int8=x+2;fn pick(x:Int8)->Int8=x+1;fn pick(x:Bool)->Int8=90;",
    ] {
        let s = source(
            "selection",
            &format!(
                "fn main()->Int8={{let x:Int8=3;pick(x)+pick(x,true)+result()}};{overloads}fn result()->Bool=false;fn result()->Int8=1;"
            ),
        );
        let b = bundle(vec![s.clone()]);
        let p = image(&b, &s, 0);
        assert_eq!(number(&b, &p), 10);
        assert_eq!(p.program().functions.iter().filter(|f|matches!(&f.identity,FunctionIdentity::Named{symbol,..} if symbol.declaration!=0)).count(),3);
    }
}
#[test]
fn imported_grouped_contextual_callable_is_captured_and_invoked_after_creator_returns() {
    let helper = source(
        "helper",
        "pub fn pick(x:Int8)->Int8=x+1;pub fn pick(x:Bool)->Bool=x;fn pick(x:Int16)->Int8=90;",
    );
    let mut entry = source(
        "entry",
        "import package helper version \"1\" as h;fn main()->Int8=make()(6);fn make()->(Int8)->Int8={let f:(Int8)->Int8=((h.pick));|x:Int8|f(x)};",
    );
    entry.dependencies.push(helper.id.clone());
    let mut b = bundle(vec![entry.clone(), helper.clone()]);
    let p = image(&b, &entry, 1);
    assert_eq!(number(&b, &p), 7);
    let selected: Vec<_> = p
        .program()
        .functions
        .iter()
        .filter_map(|f| match &f.identity {
            FunctionIdentity::Named {
                symbol,
                type_arguments,
            } if symbol.source == helper.id => Some((symbol, type_arguments)),
            _ => None,
        })
        .collect();
    assert_eq!(selected.len(), 1);
    assert_eq!(selected[0].0.declaration, 0);
    assert!(selected[0].1.is_empty());
    b.sources.reverse();
    let replay: DefinitionBundle =
        serde_json::from_value(serde_json::to_value(&b).unwrap()).unwrap();
    let fresh = image(&replay, &entry, 1);
    assert_eq!(p.program(), fresh.program());
    assert_eq!(number(&replay, &fresh), 7);
}
#[test]
fn generic_explicit_filter_and_contextual_callback_execute_exact_instances() {
    let s = source(
        "generics",
        "fn main()->Int8={let x:Int8=3;pick::<Int8,Bool>(x)+apply(identity,x)};fn pick<T,U>(x:T)->T=x;fn pick(x:Bool)->Bool=x;fn apply<T>(f:(T)->T,x:T)->T=f(x);fn identity(x:Bool)->Bool=x;fn identity<T:Int8>(x:T)->T=x;",
    );
    let b = bundle(vec![s.clone()]);
    let p = image(&b, &s, 0);
    assert_eq!(number(&b, &p), 6);
    assert!(p.program().functions.iter().any(|f|matches!(&f.identity,FunctionIdentity::Named{symbol,type_arguments} if symbol.declaration==1&&type_arguments.len()==2)));
    assert!(!p.program().functions.iter().any(|f|matches!(&f.identity,FunctionIdentity::Named{symbol,..} if [2,4].contains(&symbol.declaration))));
}
#[test]
fn selected_imported_failure_trace_uses_exact_overload_definition_and_call_site() {
    let helper = source(
        "helper",
        "pub fn fail(x:Bool)->Int8=1;pub fn fail(x:Int8)->Int8=x/0;",
    );
    let mut entry = source(
        "entry",
        "import package helper version \"1\" as h;fn main()->Int8={let x:Int8=2;h.fail(x)};",
    );
    entry.dependencies.push(helper.id.clone());
    let b = bundle(vec![entry.clone(), helper.clone()]);
    let p = image(&b, &entry, 1);
    let e = evaluate(
        &p,
        &[],
        &ExecutionContext::new(&b),
        &ExecutionLimits::default(),
    )
    .unwrap_err();
    assert_eq!(
        e.arithmetic_kind,
        Some(intersect_model::rcore_behavior_scalar::ArithmeticErrorKind::DivisionByZero)
    );
    assert_eq!(e.span.as_ref().unwrap().source, helper.id);
    assert!(e.trace.iter().any(|f|matches!(&f.function,FunctionIdentity::Named{symbol,..} if symbol.source==helper.id&&symbol.declaration==1)&&f.call_site.as_ref().is_some_and(|s|s.source==entry.id)));
    assert!(!e.trace.iter().any(|f|matches!(&f.function,FunctionIdentity::Named{symbol,..} if symbol.source==helper.id&&symbol.declaration==0)));
}

#[test]
fn callback_overloads_use_the_entire_collection_or_generic_call_context() {
    for (overloads, selected) in [
        ("fn pick(x:Bool)->Bool=x;fn pick(x:Int)->Int=x+1;", 2),
        ("fn pick(x:Int)->Int=x+1;fn pick(x:Bool)->Bool=x;", 1),
    ] {
        for expression in [
            "map([],((pick)))",
            "sortedBy([],pick)",
            "apply([],pick)",
            "applyReverse(pick,[])",
        ] {
            let s = source(
                "context",
                &format!(
                    "fn main()->Int8={{let xs:Sequence<Int> ={expression};if isEmpty(xs) {{42}} else {{0}}}};{overloads}fn apply<T,U>(xs:Sequence<T>,f:(T)->U)->Sequence<U> =map(xs,f);fn applyReverse<T,U>(f:(T)->U,xs:Sequence<T>)->Sequence<U> =map(xs,f);"
                ),
            );
            let b = bundle(vec![s.clone()]);
            let p = image(&b, &s, 0);
            assert_eq!(number(&b, &p), 42);
            let choices = p
                .program()
                .functions
                .iter()
                .filter_map(|f| match &f.identity {
                    FunctionIdentity::Named { symbol, .. }
                        if symbol.source == s.id && [1, 2].contains(&symbol.declaration) =>
                    {
                        Some(symbol.declaration)
                    }
                    _ => None,
                })
                .collect::<Vec<_>>();
            assert_eq!(choices, vec![selected]);
            assert_eq!(p.program(), image(&b, &s, 0).program());
        }
    }
    for body in [
        "fn main()->Int8={let xs:Sequence<(Int,Sequence<Int>)> =groupBy([],pick);if isEmpty(xs) {42} else {0}};fn pick(x:Bool)->Bool=x;fn pick(x:Int)->Int=x;",
        "fn main()->Int8={let xs:Sequence<Int> =filter([],pick);if isEmpty(xs) {42} else {0}};fn pick(x:Bool)->Int=0;fn pick(x:Int)->Bool=true;",
        "fn main()->Int8={let xs:Sequence<Int> =map([],pick);if isEmpty(xs) {42} else {0}};fn pick(a:Int,b:Int)->Int=a+b;fn pick(x:Int)->Int=x;",
        "fn main()->Int8=map(pick,1);fn map<T>(f:(T)->T,x:T)->Int8=37;fn pick(x:Bool)->Bool=x;fn pick(x:Int)->Int=x;",
    ] {
        let s = source("context", body);
        let b = bundle(vec![s.clone()]);
        assert_eq!(
            number(&b, &image(&b, &s, 0)),
            if body.contains("=map(pick,1)") {
                37
            } else {
                42
            }
        );
    }
}

#[test]
fn callback_trials_preserve_generic_and_multiple_argument_selections() {
    for body in [
        "fn main()->Int8={let xs:Sequence<Int> =map([],pick);if isEmpty(xs) {42} else {0}};fn pick(x:Bool)->Bool=x;fn pick<T>(x:T)->T=x;",
        "fn main()->Int8={let xs:Sequence<Int> =combine(pick,other,[]);if isEmpty(xs) {42} else {0}};fn pick(x:Bool)->Bool=x;fn pick<T>(x:T)->T=x;fn other(x:Int)->Int=x;fn other(x:Bool)->Bool=x;fn combine<T>(a:(T)->T,b:(T)->T,xs:Sequence<T>)->Sequence<T> =map(map(xs,a),b);",
    ] {
        let s = source("genericcontext", body);
        let b = bundle(vec![s.clone()]);
        let p = image(&b, &s, 0);
        assert_eq!(number(&b, &p), 42);
        assert!(p.program().functions.iter().any(|f| matches!(&f.identity,
            FunctionIdentity::Named {symbol,type_arguments} if symbol.declaration==2 && type_arguments.len()==1)));
        assert!(!p.program().functions.iter().any(|f| matches!(&f.identity,
            FunctionIdentity::Named {symbol,..} if symbol.declaration==1)));
    }
}

#[test]
fn imported_callback_trials_keep_exact_identity_and_replay() {
    let helper = source(
        "helper",
        "pub fn pick(x:Bool)->Bool=x;pub fn pick(x:Int)->Int=x+1;",
    );
    let mut entry = source(
        "entry",
        r#"import package helper version "1" as h;fn main()->Int8={let xs:Sequence<Int> =map([],((h.pick)));if isEmpty(xs) {42} else {0}};"#,
    );
    entry.dependencies.push(helper.id.clone());
    let mut b = bundle(vec![entry.clone(), helper.clone()]);
    let p = image(&b, &entry, 1);
    assert_eq!(number(&b, &p), 42);
    let selections = p
        .program()
        .functions
        .iter()
        .filter_map(|f| match &f.identity {
            FunctionIdentity::Named { symbol, .. } if symbol.source == helper.id => {
                Some(symbol.declaration)
            }
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(selections, vec![1]);
    b.sources.reverse();
    assert_eq!(p.program(), image(&b, &entry, 1).program());
}

#[test]
fn callback_trials_reject_ambiguity_inapplicability_and_dead_branch_errors() {
    for (body, message) in [
        (
            "fn main()->Sequence<Int> =map([],pick);fn pick(x:Int)->Int=x;fn pick(x:Bool)->Int=0;",
            "Ambiguous overload",
        ),
        (
            "fn main()->Sequence<String> =map([],pick);fn pick(x:Int)->Int=x;fn pick(x:Bool)->Bool=x;",
            "No applicable overload",
        ),
        (
            "fn main()->Int8=if true {42} else {let xs:Sequence<Int> =map([],pick);0};fn pick(x:Int)->Int=x;fn pick(x:Bool)->Int=0;",
            "Ambiguous overload",
        ),
        (
            "fn main()->Sequence<Int> =apply([],pick);fn apply<T,U>(xs:Sequence<T>,f:(T)->U)->Sequence<U> =map(xs,f);fn pick(x:Int)->Int=x;fn pick(x:Bool)->Int=0;",
            "Ambiguous overload",
        ),
    ] {
        let s = source("badcontext", body);
        let b = bundle(vec![s.clone()]);
        let selection = EntrySelection {
            symbol: SymbolId {
                source: s.id.clone(),
                declaration: 0,
            },
            types: vec![],
            arguments: vec![],
        };
        let errors = compile_function(&b, &s.id, &selection, &Limits::default()).unwrap_err();
        assert!(
            errors.iter().any(|error| error.message.contains(message)),
            "{errors:?}"
        );
        for error in errors {
            assert_eq!(error.span.source, s.id);
            assert!(error.span.end > error.span.start);
        }
    }
}

#[test]
fn partial_context_instantiates_generic_callbacks_before_empty_arguments() {
    for expression in ["apply([],((keep)))", "applyReverse(keep,[])"] {
        for callback in ["fn keep<T>(x:T)->T=x;", "fn keep<T:Int>(x:T)->T=x;"] {
            let s = source(
                "partial",
                &format!(
                    "fn main()->Int8={{let xs:Sequence<Int> ={expression};if isEmpty(xs) {{42}} else {{0}}}};{callback}fn apply<T,U>(xs:Sequence<T>,f:(T)->U)->Sequence<U> =map(xs,f);fn applyReverse<T,U>(f:(T)->U,xs:Sequence<T>)->Sequence<U> =map(xs,f);"
                ),
            );
            let b = bundle(vec![s.clone()]);
            let p = image(&b, &s, 0);
            assert_eq!(number(&b, &p), 42);
            assert!(p.program().functions.iter().any(|f| matches!(&f.identity,
                FunctionIdentity::Named {symbol,type_arguments} if symbol.source==s.id && symbol.declaration==1 && type_arguments.len()==1)));
            assert_eq!(p.program(), image(&b, &s, 0).program());
        }
    }
}

#[test]
fn partial_callback_context_propagates_nested_components_and_overloads() {
    for body in [
        "fn main()->Int8={let xs:Sequence<Int> =invoke([],keep);if isEmpty(xs) {42} else {0}};fn keep<T>(x:T)->T=x;fn invoke<T,U>(xs:Sequence<T>,f:(Sequence<T>)->U)->U=f(xs);",
        "fn main()->Int8={let xs:Sequence<Int> =project([],pair);if isEmpty(xs) {42} else {0}};fn pair<T>(x:T)->(T,Bool)=(x,true);fn project<T,U,V>(xs:Sequence<T>,f:(T)->(U,V))->Sequence<U> =[];",
        "fn main()->Int8={let xs:Sequence<Int> =project([],pair);if isEmpty(xs) {42} else {0}};fn pair<T>(x:T)->(Option<T>,Bool)=(Some(x),true);fn project<T,U,V>(xs:Sequence<T>,f:(T)->(Option<U>,V))->Sequence<U> =[];",
        "fn main()->Int8={let xs:Sequence<Int> =apply([],keep);if isEmpty(xs) {42} else {0}};fn keep(x:Bool)->Bool=x;fn keep<T>(x:T)->T=x;fn apply<T,U>(xs:Sequence<T>,f:(T)->U)->Sequence<U> =map(xs,f);",
    ] {
        let s = source("partialnested", body);
        let b = bundle(vec![s.clone()]);
        let p = image(&b, &s, 0);
        assert_eq!(number(&b, &p), 42);
        assert_eq!(p.program(), image(&b, &s, 0).program());
    }
}

#[test]
fn partial_callback_context_retains_bounds_and_rejects_missing_constraints() {
    for body in [
        "fn main()->Sequence<Int> =apply([],keep);fn keep<T:Bool>(x:T)->T=x;fn apply<T,U>(xs:Sequence<T>,f:(T)->U)->Sequence<U> =map(xs,f);",
        "fn main()->Int8={let xs=apply([],keep);42};fn keep<T>(x:T)->T=x;fn apply<T,U>(xs:Sequence<T>,f:(T)->U)->Sequence<U> =map(xs,f);",
        "fn main()->Sequence<Int> =apply([],keep);fn keep<T,V>(x:T)->T=x;fn apply<T,U>(xs:Sequence<T>,f:(T)->U)->Sequence<U> =map(xs,f);",
        "fn main()->Sequence<Int> =apply([true],keep);fn keep<T>(x:T)->T=x;fn apply<T,U>(xs:Sequence<T>,f:(T)->U)->Sequence<U> =map(xs,f);",
    ] {
        let s = source("badpartial", body);
        let b = bundle(vec![s.clone()]);
        let errors = compile_function(
            &b,
            &s.id,
            &EntrySelection {
                symbol: SymbolId {
                    source: s.id.clone(),
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
                .all(|error| error.span.source == s.id && error.span.end > error.span.start)
        );
    }
}
