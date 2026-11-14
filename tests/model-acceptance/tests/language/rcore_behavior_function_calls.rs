//! Independent source execution controls for call order, callable views and cycles.
use intersect_model::definitions::{
    bundle::{DefinitionBundle, DefinitionSource, SourceKind},
    rcore_behavior::{
        function_lowering::compile_function,
        ir_v2::{
            DataWire, EntrySelection, FunctionIdentity, Limits, ValueWire, VerifiedExecutable,
        },
        modules::SymbolId,
    },
};

use intersect_model::{
    rcore_behavior_functions::{ExecutionContext, ExecutionErrorKind, ExecutionLimits, evaluate},
    rcore_behavior_scalar::ArithmeticErrorKind,
};
fn fixture(body: &str) -> (DefinitionBundle, DefinitionSource) {
    let source = DefinitionSource::new(
        "memory:calls-behavior.rcore",
        SourceKind::RcoreBehavior,
        format!("rcore 1;package call_cases version \"1\";{body}"),
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
fn selection(source: &DefinitionSource, declaration: usize) -> EntrySelection {
    EntrySelection {
        symbol: SymbolId {
            source: source.id.clone(),
            declaration,
        },
        types: vec![],
        arguments: vec![],
    }
}
fn image(
    bundle: &DefinitionBundle,
    source: &DefinitionSource,
    declaration: usize,
) -> VerifiedExecutable {
    compile_function(
        bundle,
        &source.id,
        &selection(source, declaration),
        &Limits::default(),
    )
    .unwrap_or_else(|errors| panic!("{}\n{errors:?}", source.text))
}
fn number(value: &ValueWire) -> i64 {
    let DataWire::Scalar(value) = &value.data else {
        panic!("Expected numeric scalar")
    };
    value
        .payload
        .as_i64()
        .or_else(|| {
            value
                .payload
                .get("value")
                .and_then(|s| s.as_str())
                .and_then(|s| s.parse().ok())
        })
        .unwrap()
}
fn run(body: &str) -> ValueWire {
    let (bundle, source) = fixture(body);
    let code = image(&bundle, &source, 0);
    evaluate(
        &code,
        &[],
        &ExecutionContext::new(&bundle),
        &ExecutionLimits::default(),
    )
    .unwrap_or_else(|error| panic!("{body}: {error:?}"))
    .value
}
#[test]
fn competing_failures_prove_callee_arguments_and_initializers_are_evaluated_in_order() {
    for (body, kind, failed, not_called) in [
        (
            "fn main()->Int8=callee()(argument());fn callee()->(Int8)->Int8={let doomed:Int8=1/0;|x:Int8|x};fn argument()->Int8=127+1;",
            ArithmeticErrorKind::DivisionByZero,
            1,
            2,
        ),
        (
            "fn main()->Int8=choose(first(),second());fn choose(a:Int8,b:Int8)->Int8=a;fn first()->Int8=127+1;fn second()->Int8=1/0;",
            ArithmeticErrorKind::Overflow,
            2,
            3,
        ),
        (
            "fn main()->Int8={let unused=first();let chosen=second();chosen};fn first()->Int8=1/0;fn second()->Int8=127+1;",
            ArithmeticErrorKind::DivisionByZero,
            1,
            2,
        ),
    ] {
        let (bundle, source) = fixture(body);
        let code = image(&bundle, &source, 0);
        let error = evaluate(
            &code,
            &[],
            &ExecutionContext::new(&bundle),
            &ExecutionLimits::default(),
        )
        .unwrap_err();
        assert_eq!(error.kind, ExecutionErrorKind::Arithmetic);
        assert_eq!(error.arithmetic_kind, Some(kind));
        assert!(error.trace.iter().any(|frame|matches!(&frame.function,FunctionIdentity::Named{symbol,..} if symbol.declaration==failed)));
        assert!(!error.trace.iter().any(|frame|matches!(&frame.function,FunctionIdentity::Named{symbol,..} if symbol.declaration==not_called)));
        let span = error.span.unwrap();
        assert_eq!(span.source, source.id);
        assert!(matches!(&source.text[span.start..span.end], "/" | "+"));
    }
}
#[test]
fn calls_obey_boolean_and_optional_laziness_and_conditional_callable_selection() {
    for (body, expected) in [
        (
            "fn main()->Bool=false&&fail();fn fail()->Bool={let doomed:Int8=1/0;true};",
            false,
        ),
        (
            "fn main()->Bool=true||fail();fn fail()->Bool={let doomed:Int8=1/0;false};",
            true,
        ),
    ] {
        let DataWire::Scalar(value) = run(body).data else {
            panic!("Boolean")
        };
        assert_eq!(value.payload, serde_json::json!(expected));
    }
    for (body, expected) in [
        (
            "fn main()->Int8=Some::<Int8>(41)??fail();fn fail()->Int8=1/0;",
            41,
        ),
        (
            "fn main()->Int8=None::<Int8>()??good();fn good()->Int8=42;",
            42,
        ),
        (
            "fn main()->Int8={let f:(Int8)->Int8=if true{add}else{sub};f(41)};fn add(x:Int8)->Int8=x+1;fn sub(x:Int8)->Int8=x-1;",
            42,
        ),
        (
            "fn main()->Int8={let f:(Int8)->Int8=if false{add}else{sub};f(41)};fn add(x:Int8)->Int8=x+1;fn sub(x:Int8)->Int8=x-1;",
            40,
        ),
    ] {
        assert_eq!(number(&run(body)), expected);
    }
}
#[test]
fn pattern_bound_callable_and_structural_captures_survive_creator_scopes() {
    for body in [
        "fn main()->Int8=match Some(add){Some(callback)=>{let f=|x:Int8|callback(x);f(41)},none=>fail()};fn add(x:Int8)->Int8=x+1;fn fail()->Int8=1/0;",
        "fn main()->Int8={let data:Result<(Int8,(Int8)->Int8),String> =Ok((1,add));match data{Ok(pair)=>{let f=|x:Int8|pair[1](x)+pair[0];f(40)},Err(problem)=>fail()}};fn add(x:Int8)->Int8=x+1;fn fail()->Int8=1/0;",
        "fn main()->Int8={let f=choose(Some::<Int8>(40));f(2)};fn choose(value:Int8?)->(Int8)->Int8=match value{Some(captured)=>|extra:Int8|captured+extra,none=>|extra:Int8|extra};",
    ] {
        assert_eq!(number(&run(body)), 42);
    }
}
#[test]
fn dynamic_alias_callback_and_changed_generic_instance_cycles_exhaust_frames() {
    for body in [
        "fn main()->Int8={let alias=main;alias()};",
        "fn main()->Int8=through(main);fn through(callback:()->Int8)->Int8=callback();",
        "fn main()->Int8=step::<Int8>(1);fn step<T>(x:T)->Int8=step::<Bool>(true);",
    ] {
        let (bundle, source) = fixture(body);
        let code = image(&bundle, &source, 0);
        let error = evaluate(
            &code,
            &[],
            &ExecutionContext::new(&bundle),
            &ExecutionLimits::default(),
        )
        .unwrap_err();
        assert_eq!(
            error.kind,
            ExecutionErrorKind::ResourceExhausted,
            "{body}: {error:?}"
        );
        assert_eq!(&*error.message, "Function frame depth limit exceeded");
        assert!(!error.trace.is_empty());
    }
}
#[test]
fn phantom_actuals_define_exact_instances_and_expanding_specialization_is_bounded() {
    let (bundle, source) = fixture(
        "fn main()->Int8=keep::<Int8,Bool>(1)+keep::<Int8,String>(2);fn keep<T,U>(x:T)->T=x;",
    );
    let code = image(&bundle, &source, 0);
    let instances: Vec<_> = code
        .program()
        .functions
        .iter()
        .filter_map(|function| match &function.identity {
            FunctionIdentity::Named {
                symbol,
                type_arguments,
            } if symbol.declaration == 1 => Some(type_arguments),
            _ => None,
        })
        .collect();
    assert_eq!(instances.len(), 2);
    assert_ne!(instances[0], instances[1]);
    assert_eq!(
        number(
            &evaluate(
                &code,
                &[],
                &ExecutionContext::new(&bundle),
                &ExecutionLimits::default()
            )
            .unwrap()
            .value
        ),
        3
    );
    let (expanding, entry) =
        fixture("fn main()->Unit=expand::<Int>(1);fn expand<T>(x:T)->Unit=expand([x]);");
    let limits = Limits {
        max_functions: 6,
        ..Limits::default()
    };
    let errors =
        compile_function(&expanding, &entry.id, &selection(&entry, 0), &limits).unwrap_err();
    assert!(
        errors
            .iter()
            .any(|error| error.message.contains("Function instance limit")),
        "{errors:?}"
    );
    assert_eq!(number(&run("fn main()->Int8=42;")), 42);
}
#[test]
fn model_subtyping_function_variance_preserves_actual_reference_identity() {
    use intersect_model::core::model::Model;
    use intersect_model::definitions::{
        compiler::materialize_runtime_bundle,
        conversion::{ConversionRegistry, ReferenceContext},
        generic_types::{RuntimeType, Type},
        resources::DeclarationId,
    };
    use intersect_model::model_reference::ReferenceDomain;
    let model_source = intersect_model_acceptance::migration_fixture::migrate_source_fixture(DefinitionSource::new(
        "memory:variance.ecore",
        SourceKind::Ecore,
        r##"<e:EPackage xmlns:e="http://www.eclipse.org/emf/2002/Ecore" xmlns:x="http://www.w3.org/2001/XMLSchema-instance" name="m" nsURI="urn:variance"><eClassifiers x:type="e:EClass" name="Base"/><eClassifiers x:type="e:EClass" name="Child" eSuperTypes="#//Base"/></e:EPackage>"##,
    )).0;
    let runtime = |ordinal| {
        RuntimeType::new(Type::Classifier {
            declaration: DeclarationId {
                source: model_source.id.clone(),
                path: vec![ordinal],
            },
            arguments: vec![],
            raw: false,
        })
        .unwrap()
    };
    let model_bundle = DefinitionBundle {
        version: 1,
        sources: vec![model_source.clone()],
        namespaces: vec![],
    };
    let requested = vec![runtime(0), runtime(1)];
    let schema = materialize_runtime_bundle(
        &model_bundle,
        &model_source.id,
        "urn:variance",
        &requested,
        None,
    )
    .unwrap();
    let mut model = Model::default();
    model
        .create(
            &schema,
            "child",
            schema.class_key_for_type(&runtime(1)).unwrap(),
            "memory:variance",
            0,
            1,
        )
        .unwrap();
    model
        .create(
            &schema,
            "base",
            schema.class_key_for_type(&runtime(0)).unwrap(),
            "memory:variance",
            2,
            3,
        )
        .unwrap();
    let reference = ReferenceContext {
        bundle: model_bundle.clone(),
        entry: model_source.id.clone(),
        namespace: "urn:variance".into(),
        conversions: vec![],
        materialized_types: requested,
        applied_conversions: vec![],
        constructors: vec![],
        model: serde_json::to_value(model).unwrap(),
    };
    let registry = ConversionRegistry::default();
    let domain = ReferenceDomain::new_runtime(runtime(0), reference.clone(), &registry).unwrap();
    let child = domain.bind("child").unwrap();
    let base = domain.bind("base").unwrap();
    for (body, valid) in [
        (
            "fn main(child:m.Child)->m.Base={let broad:(m.Base)->m.Child=|ignored:m.Base|child;let narrowed:(m.Child)->m.Base=broad;narrowed(child)};",
            true,
        ),
        (
            "fn main(child:m.Child)->m.Child={let narrow:(m.Child)->m.Base=|value:m.Child|value;let invalid:(m.Base)->m.Child=narrow;invalid(child)};",
            false,
        ),
    ] {
        let mut source = DefinitionSource::new(
            "memory:variance-behavior.rcore",
            SourceKind::RcoreBehavior,
            format!(
                "rcore 1;package variance version \"1\";import model \"urn:variance\" as m;{body}"
            ),
        );
        source.dependencies.push(model_source.id.clone());
        let bundle = DefinitionBundle {
            version: 1,
            sources: vec![model_source.clone(), source.clone()],
            namespaces: vec![],
        };
        let compiled = compile_function(
            &bundle,
            &source.id,
            &selection(&source, 1),
            &Limits::default(),
        );
        if !valid {
            assert!(compiled.is_err());
            continue;
        }
        let code = compiled.unwrap();
        let ty = code.program().functions[code.program().entry.0 as usize].parameters[0];
        let references = vec![reference.clone()];
        let context = ExecutionContext {
            bundle: &bundle,
            conversions: Some(&registry),
            references: &references,
        };
        let value = evaluate(
            &code,
            &[ValueWire {
                ty,
                data: DataWire::Model(child.clone()),
            }],
            &context,
            &ExecutionLimits::default(),
        )
        .unwrap()
        .value;
        assert_eq!(value.data, DataWire::Model(child.clone()));
        assert!(
            evaluate(
                &code,
                &[ValueWire {
                    ty,
                    data: DataWire::Model(base.clone())
                }],
                &context,
                &ExecutionLimits::default()
            )
            .is_err()
        );
    }
}

#[test]
fn direct_mutual_generic_and_callback_recursion_unwind_exact_results() {
    for (body, expected) in [
        (
            "fn main()->Int8=sum(5);fn sum(n:Int8)->Int8=if n==0 {0} else {n+sum(n-1)};",
            15,
        ),
        (
            "fn main()->Int8=even(6);fn even(n:Int8)->Int8=if n==0 {42} else {odd(n-1)};fn odd(n:Int8)->Int8=if n==0 {0} else {even(n-1)};",
            42,
        ),
        (
            "fn main()->Int8=keep::<Int8>(5,42);fn keep<T>(n:Int8,x:T)->T=if n==0 {x} else {keep::<T>(n-1,x)};",
            42,
        ),
        (
            "fn main()->Int8=step(5);fn step(n:Int8)->Int8=if n==0 {42} else {through(step,n-1)};fn through(f:(Int8)->Int8,n:Int8)->Int8=f(n);",
            42,
        ),
        (
            "fn main()->Int8=make(5)(0);fn make(n:Int8)->(Int8)->Int8=|x:Int8|if n==0 {x} else {make(n-1)(x)+n};",
            15,
        ),
    ] {
        assert_eq!(number(&run(body)), expected, "{body}");
    }
}

#[test]
fn recursive_frame_and_step_limits_have_exact_boundaries_and_preserve_causes() {
    let (bundle, source) =
        fixture("fn main()->Int8=sum(5);fn sum(n:Int8)->Int8=if n==0 {0} else {n+sum(n-1)};");
    let code = image(&bundle, &source, 0);
    let context = ExecutionContext::new(&bundle);
    let mut limits = ExecutionLimits {
        max_frames: 7,
        ..Default::default()
    };
    let good = evaluate(&code, &[], &context, &limits).unwrap();
    assert_eq!(number(&good.value), 15);
    limits.max_frames = 6;
    let failure = evaluate(&code, &[], &context, &limits).unwrap_err();
    assert_eq!(failure.kind, ExecutionErrorKind::ResourceExhausted);
    assert_eq!(&*failure.message, "Function frame depth limit exceeded");
    assert_eq!(failure.trace.len(), 6);
    limits.max_frames = 7;
    limits.scalar.max_steps = good.consumed.steps;
    assert_eq!(
        serde_json::to_value(evaluate(&code, &[], &context, &limits).unwrap()).unwrap(),
        serde_json::to_value(&good).unwrap()
    );
    limits.scalar.max_steps -= 1;
    let exhausted = evaluate(&code, &[], &context, &limits).unwrap_err();
    assert_eq!(exhausted.kind, ExecutionErrorKind::ResourceExhausted);
    assert_eq!(exhausted.consumed.steps, limits.scalar.max_steps);
    let (bundle, source) =
        fixture("fn main()->Int8=fail(3);fn fail(n:Int8)->Int8=if n==0 {1/0} else {fail(n-1)};");
    let code = image(&bundle, &source, 0);
    let error = evaluate(
        &code,
        &[],
        &ExecutionContext::new(&bundle),
        &ExecutionLimits::default(),
    )
    .unwrap_err();
    assert_eq!(error.kind, ExecutionErrorKind::Arithmetic);
    assert_eq!(
        error.arithmetic_kind,
        Some(ArithmeticErrorKind::DivisionByZero)
    );
    assert_eq!(error.trace.len(), 5);
    assert!(
        error
            .trace
            .iter()
            .all(|frame| frame.definition.source == source.id)
    );
}

#[test]
fn imported_mutual_recursion_retains_exact_module_identities() {
    let mut a = DefinitionSource::new(
        "memory:a-behavior.rcore",
        SourceKind::RcoreBehavior,
        "rcore 1;package a version \"1\";import package b version \"1\" as b;pub fn even(n:Int8)->Int8=if n==0 {42} else {b.odd(n-1)};",
    );
    let mut b = DefinitionSource::new(
        "memory:b-behavior.rcore",
        SourceKind::RcoreBehavior,
        "rcore 1;package b version \"1\";import package a version \"1\" as a;pub fn odd(n:Int8)->Int8=if n==0 {0} else {a.even(n-1)};",
    );
    a.dependencies.push(b.id.clone());
    b.dependencies.push(a.id.clone());
    let mut bundle = DefinitionBundle {
        version: 1,
        sources: vec![a.clone(), b.clone()],
        namespaces: vec![],
    };
    for _ in 0..2 {
        let code = image(&bundle, &a, 1);
        let ids: Vec<_> = code
            .program()
            .functions
            .iter()
            .filter_map(|f| match &f.identity {
                FunctionIdentity::Named { symbol, .. } => Some(symbol.source.clone()),
                _ => None,
            })
            .collect();
        assert!(ids.contains(&a.id) && ids.contains(&b.id));
        let ty = code.program().functions[code.program().entry.0 as usize].parameters[0];
        for (n, expected) in [(6, 42), (5, 0)] {
            let argument = ValueWire {
                ty,
                data: DataWire::Scalar(intersect_model::definitions::rcore_behavior::ir::Value {
                    ty: intersect_model::definitions::rcore_behavior::ir::ValueType::Scalar(
                        intersect_model::definitions::rcore_behavior::types::Scalar::Int8,
                    ),
                    payload: serde_json::json!(n),
                }),
            };
            assert_eq!(
                number(
                    &evaluate(
                        &code,
                        &[argument],
                        &ExecutionContext::new(&bundle),
                        &ExecutionLimits::default()
                    )
                    .unwrap()
                    .value
                ),
                expected
            );
        }
        bundle.sources.reverse();
    }
    bundle.sources.retain(|s| s.id == a.id);
    assert!(compile_function(&bundle, &a.id, &selection(&a, 1), &Limits::default()).is_err());
}

#[test]
fn imported_generic_cycles_keep_phantom_instances_distinct() {
    let mut a = DefinitionSource::new(
        "memory:generic-a-behavior.rcore",
        SourceKind::RcoreBehavior,
        "rcore 1;package a version \"1\";import package b version \"1\" as b;pub fn bounce<T,U>(n:Int8,x:T)->T=if n==0 {x} else {b.back::<T,U>(n-1,x)};",
    );
    let mut b = DefinitionSource::new(
        "memory:generic-b-behavior.rcore",
        SourceKind::RcoreBehavior,
        "rcore 1;package b version \"1\";import package a version \"1\" as a;pub fn back<T,U>(n:Int8,x:T)->T=a.bounce::<T,U>(n,x);",
    );
    let mut entry = DefinitionSource::new(
        "memory:generic-entry-behavior.rcore",
        SourceKind::RcoreBehavior,
        "rcore 1;package entry version \"1\";import package a version \"1\" as a;fn run()->Int8=a.bounce::<Int8,Bool>(4,20)+a.bounce::<Int8,String>(4,22);",
    );
    a.dependencies.push(b.id.clone());
    b.dependencies.push(a.id.clone());
    entry.dependencies.push(a.id.clone());
    let mut bundle = DefinitionBundle {
        version: 1,
        sources: vec![entry.clone(), a.clone(), b.clone()],
        namespaces: vec![],
    };
    let mut outcomes = Vec::new();
    for _ in 0..2 {
        let code = image(&bundle, &entry, 1);
        for source in [&a, &b] {
            let instances: Vec<_> = code
                .program()
                .functions
                .iter()
                .filter_map(|f| match &f.identity {
                    FunctionIdentity::Named {
                        symbol,
                        type_arguments,
                    } if symbol.source == source.id => Some(type_arguments),
                    _ => None,
                })
                .collect();
            assert_eq!(instances.len(), 2);
            assert_eq!(instances[0].len(), 2);
            assert_eq!(instances[0][0], instances[1][0]);
            assert_ne!(instances[0][1], instances[1][1]);
        }
        let result = evaluate(
            &code,
            &[],
            &ExecutionContext::new(&bundle),
            &ExecutionLimits::default(),
        )
        .unwrap();
        assert_eq!(number(&result.value), 42);
        outcomes.push(serde_json::to_value(result).unwrap());
        bundle.sources.reverse();
    }
    assert_eq!(outcomes[0], outcomes[1]);
}

#[test]
fn required_source_revision_has_a_typed_manifest_cause() {
    use intersect_model::definitions::rcore_behavior::ir_v2::{
        ContextDependency, ContextValidationError,
    };
    use intersect_model::rcore_behavior_functions::DependencyCause;
    let (bundle, source) = fixture("fn main()->Int8=42;");
    let code = image(&bundle, &source, 0);
    let mut missing = bundle.clone();
    missing.sources.clear();
    assert!(matches!(
        code.check_context_detailed(&missing, None),
        Err(ContextValidationError::Dependency(
            ContextDependency::SourceRevision { manifest_index: 0 }
        ))
    ));
    assert!(code.check_context(&missing, None).is_err());
    let error = evaluate(
        &code,
        &[],
        &ExecutionContext::new(&missing),
        &ExecutionLimits::default(),
    )
    .unwrap_err();
    assert_eq!(error.kind, ExecutionErrorKind::MissingDependency);
    assert_eq!(
        error.dependency.as_deref(),
        Some(&DependencyCause::ExecutableContext(
            ContextDependency::SourceRevision { manifest_index: 0 }
        ))
    );
    assert_eq!(code.program().sources[0].source, source.id);
    assert_eq!(error.consumed.steps, 0);
    let mut tampered = bundle.clone();
    tampered.sources[0].text.push(' ');
    assert!(matches!(
        code.check_context_detailed(&tampered, None),
        Err(ContextValidationError::Invalid(_))
    ));
    assert_eq!(
        evaluate(
            &code,
            &[],
            &ExecutionContext::new(&tampered),
            &ExecutionLimits::default()
        )
        .unwrap_err()
        .kind,
        ExecutionErrorKind::Arguments
    );
    assert!(
        evaluate(
            &code,
            &[],
            &ExecutionContext::new(&bundle),
            &ExecutionLimits::default()
        )
        .is_ok()
    );
}

#[test]
fn long_lazy_chains_keep_short_circuit_results_and_selected_failure_traces() {
    for (operator, first, rest, expected) in [
        ("&&", "false", "true", false),
        ("||", "true", "false", true),
    ] {
        let expression = std::iter::once(first)
            .chain(std::iter::once("fail()"))
            .chain(std::iter::repeat_n(rest, 30))
            .collect::<Vec<_>>()
            .join(operator);
        let value = run(&format!(
            "fn main()->Bool={expression};fn fail()->Bool={{let x:Int8=1/0;true}};"
        ));
        let DataWire::Scalar(value) = value.data else {
            panic!("Bool")
        };
        assert_eq!(value.payload, serde_json::json!(expected));
    }
    for (operator, value) in [("&&", "true"), ("||", "false")] {
        let expression = std::iter::repeat_n(value, 31)
            .chain(std::iter::once("fail()"))
            .collect::<Vec<_>>()
            .join(operator);
        let (bundle, source) = fixture(&format!(
            "fn main()->Bool={expression};fn fail()->Bool={{let x:Int8=1/0;true}};"
        ));
        let code = image(&bundle, &source, 0);
        let failure = evaluate(
            &code,
            &[],
            &ExecutionContext::new(&bundle),
            &ExecutionLimits::default(),
        )
        .unwrap_err();
        assert_eq!(failure.kind, ExecutionErrorKind::Arithmetic);
        assert_eq!(
            failure.arithmetic_kind,
            Some(ArithmeticErrorKind::DivisionByZero)
        );
        assert!(failure.trace.iter().any(|frame|matches!(&frame.function,FunctionIdentity::Named{symbol,..} if symbol.source==source.id && symbol.declaration==1)));
    }
    assert_eq!(
        number(&run(
            "fn main()->Int8=Some::<Int8>(42) ?? fail();fn fail()->Int8=1/0;"
        )),
        42
    );
    assert_eq!(
        number(&run(
            "fn main()->Int8={let missing:Option<Int8> =None;missing ?? 42};"
        )),
        42
    );
}

#[test]
fn nested_lowering_preserves_lexical_restoration_and_branch_failures() {
    let mut expression = "x".to_string();
    for _ in 0..12 {
        expression = format!("keep(if true {{let x:Int8=42;({expression})}} else {{fail()}})");
    }
    assert_eq!(
        number(&run(&format!(
            "fn main()->Int8={{let x:Int8=7;let y:Int8={expression};y+x}};fn keep(x:Int8)->Int8=x;fn fail()->Int8=1/0;"
        ))),
        49
    );
    assert_eq!(
        number(&run(
            "fn main()->Int8={let x:Int8=7;let y:Int8=match Some::<Int8>(42) {Some(x)=>{let z={let x:Int8=1;x};x+z},None=>0};x+y};"
        )),
        50
    );
    let (bundle, source) = fixture(
        "fn main()->Int8={let x:Int8=7;if true {let y=fail();x} else {0}};fn fail()->Int8=1/0;",
    );
    let code = image(&bundle, &source, 0);
    let failure = evaluate(
        &code,
        &[],
        &ExecutionContext::new(&bundle),
        &ExecutionLimits::default(),
    )
    .unwrap_err();
    assert_eq!(failure.kind, ExecutionErrorKind::Arithmetic);
    assert_eq!(
        failure.arithmetic_kind,
        Some(ArithmeticErrorKind::DivisionByZero)
    );
    assert!(failure.trace.iter().any(|frame|matches!(&frame.function,FunctionIdentity::Named{symbol,..} if symbol.source==source.id && symbol.declaration==1)));
}
