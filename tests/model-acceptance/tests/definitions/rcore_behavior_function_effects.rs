use intersect_model::definitions::{
    bundle::{DefinitionBundle, DefinitionSource, SourceKind},
    rcore_behavior::{
        function_lowering::compile_function,
        ir_v2::{
            self, Effect, EntrySelection, FunctionId, FunctionIdentity, Limits, Program,
            VerifiedExecutable,
        },
        modules::SymbolId,
    },
};
fn fixture(body: &str) -> (DefinitionBundle, DefinitionSource) {
    let model = intersect_model_acceptance::migration_fixture::migrate_source_fixture(
        DefinitionSource::new(
            "memory:effects.ecore",
            SourceKind::Ecore,
            include_str!("../../../../fixtures/ecore-navigation/projection.ecore"),
        ),
    )
    .0;
    let mut source = DefinitionSource::new(
        "memory:effects-behavior.rcore",
        SourceKind::RcoreBehavior,
        format!(
            "rcore 1;package effects version \"1\";import model \"urn:intersect:navigation\" as m;{body}"
        ),
    );
    source.dependencies.push(model.id.clone());
    (
        DefinitionBundle {
            version: 1,
            sources: vec![model, source.clone()],
            namespaces: vec![],
        },
        source,
    )
}
fn compile(bundle: &DefinitionBundle, source: &DefinitionSource) -> VerifiedExecutable {
    compile_function(
        bundle,
        &source.id,
        &EntrySelection {
            symbol: SymbolId {
                source: source.id.clone(),
                declaration: 1,
            },
            types: vec![],
            arguments: vec![],
        },
        &Limits::default(),
    )
    .unwrap()
}
#[test]
fn model_reads_propagate_through_calls_callbacks_captures_structures_and_recursion() {
    // A record-field closure is called as `(f.callback)(x)`: since adfc19d8
    // `f.callback(x)` is method syntax for `callback(f, x)`.
    for body in [
        "fn main(x:m.Base)->Option<String> =x.label;",
        "fn main(x:m.Base)->Option<String> =helper(x);fn helper(x:m.Base)->Option<String> =x.label;",
        "fn main(x:m.Base)->Sequence<Option<String>> =map([x],|v:m.Base|v.label);",
        "fn main(x:m.Base)->Option<String> ={let f=|v:m.Base|v.label;let invoke=|v:m.Base|f(v);invoke(x)};",
        "fn main(x:m.Base)->Option<String> ={let f={callback:|v:m.Base|v.label};(f.callback)(x)};",
        "fn main(x:m.Base)->Option<String> =apply(x,|v:m.Base|v.label);fn apply<T,U>(x:T,f:(T)->U)->U=f(x);",
        "fn main(x:m.Base,n:Int)->Option<String> =other(x,n);fn other(x:m.Base,n:Int)->Option<String> =if n==0 {x.label} else {main(x,n-1)};",
        "fn main(x:Option<m.Base>)->Option<String> =x?.label;",
    ] {
        let (bundle, source) = fixture(body);
        let image = compile(&bundle, &source);
        assert_eq!(
            image.function_effect(image.program().entry).unwrap(),
            Effect::Query,
            "{body}"
        );
        let wire = serde_json::to_vec(image.program()).unwrap();
        let rebuilt = ir_v2::verify(
            serde_json::from_slice(&wire).unwrap(),
            &bundle,
            &Limits::default(),
        )
        .unwrap();
        assert_eq!(image.effects(), rebuilt.effects());
    }
}
#[test]
fn constructing_query_closures_is_pure_and_direct_pure_calls_stay_precise() {
    let (bundle, source) = fixture(
        "fn main(x:m.Base)->Option<String> ={let unused=|v:m.Base|v.label;pure(x)};fn pure(x:m.Base)->Option<String> =None;",
    );
    let image = compile(&bundle, &source);
    assert_eq!(
        image.function_effect(image.program().entry).unwrap(),
        Effect::Pure
    );
    for (index, function) in image.program().functions.iter().enumerate() {
        let expected = if matches!(function.identity, FunctionIdentity::Lambda { .. }) {
            Effect::Query
        } else {
            Effect::Pure
        };
        assert_eq!(
            image.function_effect(FunctionId(index as u32)).unwrap(),
            expected
        );
    }
    assert!(image.effects().contains(&Effect::Query));
    assert!(image.function_effect(FunctionId(u32::MAX)).is_err());
}
#[test]
fn returned_query_closure_does_not_make_its_factory_query() {
    let (bundle, source) = fixture(
        "fn main(x:m.Base)->Option<String> ={let f=make();f(x)};fn make()->(m.Base)->Option<String> =|v:m.Base|v.label;",
    );
    let image = compile(&bundle, &source);
    assert_eq!(
        image.function_effect(image.program().entry).unwrap(),
        Effect::Query
    );
    let factory=image.program().functions.iter().position(|f| matches!(&f.identity, FunctionIdentity::Named { symbol, .. } if symbol.declaration==2)).unwrap();
    assert_eq!(
        image.function_effect(FunctionId(factory as u32)).unwrap(),
        Effect::Pure
    );
}
#[test]
fn indirect_call_effects_are_a_sound_conservative_upper_bound() {
    let (bundle, source) = fixture(
        "fn main(x:m.Base)->Option<String> ={let unused=|v:m.Base|v.label;let f={callback:|v:m.Base|Some(\"pure\")};(f.callback)(x)};",
    );
    let image = compile(&bundle, &source);
    // Both compatible closures occur in the image. The current analysis does
    // not prove which record field reaches this call, so it cannot claim pure.
    assert_eq!(
        image.function_effect(image.program().entry).unwrap(),
        Effect::Query
    );
}
#[test]
fn effect_claims_cannot_enter_wire_authority_and_analysis_is_budgeted() {
    let (bundle, source) = fixture("fn main(x:m.Base)->Option<String> =x.label;");
    let image = compile(&bundle, &source);
    for function_claim in [false, true] {
        let mut wire = serde_json::to_value(image.program()).unwrap();
        if function_claim {
            wire["functions"][0]["effect"] = serde_json::json!("pure");
        } else {
            wire["effects"] = serde_json::json!(["pure"]);
        }
        assert!(serde_json::from_value::<Program>(wire).is_err());
    }
    let mut lower = 0;
    let mut upper = Limits::default().scalar.max_work;
    while lower < upper {
        let middle = lower + (upper - lower) / 2;
        let mut limits = Limits::default();
        limits.scalar.max_work = middle;
        match ir_v2::verify(image.program().clone(), &bundle, &limits) {
            Ok(result) => {
                assert_eq!(result.effects(), image.effects());
                upper = middle;
            }
            Err(error) => {
                assert_eq!(error.kind, ir_v2::IrErrorKind::ResourceExhausted);
                lower = middle + 1;
            }
        }
    }
    assert!(lower > 0);
    for work in [lower - 1, lower, lower + 1] {
        let mut limits = Limits::default();
        limits.scalar.max_work = work;
        assert_eq!(
            ir_v2::verify(image.program().clone(), &bundle, &limits).is_ok(),
            work >= lower
        );
    }
}
