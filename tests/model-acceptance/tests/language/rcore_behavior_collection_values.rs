use intersect_model::definitions::{
    bundle::{DefinitionBundle, DefinitionSource, SourceKind},
    rcore_behavior::{
        function_lowering::compile_function,
        ir,
        ir_v2::{DataWire, EntrySelection, Limits, ValueWire},
        modules::SymbolId,
        types::Scalar,
    },
};
use intersect_model::rcore_behavior_functions::{
    ExecutionContext, ExecutionErrorKind, ExecutionLimits, evaluate,
};
fn fixture(body: &str) -> (DefinitionBundle, DefinitionSource) {
    let source = DefinitionSource::new(
        "memory:runtime.rcore_behavior",
        SourceKind::RcoreBehavior,
        format!("rcore 1; package runtime version \"1\"; {body}"),
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
fn selected(source: &DefinitionSource) -> SymbolId {
    let document = intersect_model::definitions::rcore_behavior::parse(source.clone());
    let declaration = document
        .declarations
        .iter()
        .position(|d| {
            matches!(
                d.value,
                intersect_model::definitions::rcore_behavior::ast::Declaration::Function { .. }
            )
        })
        .unwrap();
    SymbolId {
        source: source.id.clone(),
        declaration,
    }
}
fn compile(
    bundle: &DefinitionBundle,
    source: &DefinitionSource,
) -> intersect_model::definitions::rcore_behavior::ir_v2::VerifiedExecutable {
    compile_function(
        bundle,
        &source.id,
        &EntrySelection {
            symbol: selected(source),
            types: vec![],
            arguments: vec![],
        },
        &Limits::default(),
    )
    .unwrap_or_else(|e| panic!("{}\n{e:?}", source.text))
}

#[test]
fn all_scalar_wires_survive_collection_callbacks_grouping_and_algebra() {
    for (name, payload) in [
        ("Bool", serde_json::json!(true)),
        ("String", serde_json::json!("é🙂")),
        ("Bytes", serde_json::json!({"kind":"bytes","hex":"00ABFF"})),
        ("Char16", serde_json::json!({"kind":"char16","unit":55296})),
        (
            "DateInstant",
            serde_json::json!({"kind":"date","milliseconds":"-9223372036854775808"}),
        ),
        ("Unit", serde_json::json!({"kind":"unit"})),
        ("Int8", serde_json::json!(-128)),
        ("Int16", serde_json::json!(-32768)),
        ("Int32", serde_json::json!(-2147483648i64)),
        (
            "Int64",
            serde_json::json!({"kind":"int64","value":"-9223372036854775808"}),
        ),
        ("UInt8", serde_json::json!(255)),
        ("UInt16", serde_json::json!(65535)),
        ("UInt32", serde_json::json!(4294967295u64)),
        (
            "UInt64",
            serde_json::json!({"kind":"uint64","value":"18446744073709551615"}),
        ),
        (
            "BigInt",
            serde_json::json!({"kind":"bigint","value":"123456789012345678901234567890"}),
        ),
        (
            "Decimal",
            serde_json::json!({"kind":"decimal","coefficient":"1234567890123456789012300","scale":4}),
        ),
        (
            "Float32",
            serde_json::json!({"kind":"float32","bits":"7fc01234"}),
        ),
        (
            "Float64",
            serde_json::json!({"kind":"float64","bits":"8000000000000000"}),
        ),
    ] {
        let (bundle, source) = fixture(&format!(
            "fn run(x:{name})->{name}={{let xs=map([x,x],|y:{name}|y);let kept=filter(xs,|y:{name}|contains([x],y));let groups=groupBy(kept,|y:{name}|y);let unique=distinct(flatten(map(groups,|g:({name},Sequence<{name}>)|g[1])));let check:Int8=if size(groups)==1 && count(kept,x)==2 && size(difference(kept,[x]))==1 && size(intersection(kept,[x]))==1 && size(union(kept,[x]))==3 {{0}} else {{1/0}};fold(Sequence(unique),x,|a:{name},b:{name}|b)}};"
        ));
        let image = compile(&bundle, &source);
        let scalar: Scalar = serde_json::from_value(serde_json::json!(name)).unwrap();
        let argument = ValueWire {
            ty: image.program().functions[image.program().entry.0 as usize].parameters[0],
            data: DataWire::Scalar(ir::Value {
                ty: ir::ValueType::Scalar(scalar),
                payload,
            }),
        };
        let result = evaluate(
            &image,
            std::slice::from_ref(&argument),
            &ExecutionContext::new(&bundle),
            &ExecutionLimits::default(),
        )
        .unwrap_or_else(|error| panic!("{name}: {error:?}"));
        assert_eq!(result.value, argument, "{name}");
        assert!(result.consumed.collection_comparisons > 0, "{name}");
        assert_eq!(
            image.program(),
            compile(&bundle, &source).program(),
            "{name}"
        );
    }
}

const ROUNDTRIP: &str = "fn roundtrip<T>(value:T)->T={let xs=map([value,value],|x:T|x);let kept=filter(xs,|x:T|contains([value],x));let groups=groupBy(kept,|x:T|x);let flat=flatten(map(groups,|g:(T,Sequence<T>)|g[1]));let checked:Int8=if size(groups)==1 && count(flat,value)==2 && size(distinct(flat))==1 && size(intersection(flat,[value]))==1 && size(union(flat,[value]))==3 && contains(difference(flat,[value]),value) {0} else {1/0};fold(Sequence(distinct(flat)),value,|a:T,b:T|b)};";

#[test]
fn structural_wires_survive_the_complete_collection_pipeline() {
    for (ty, literal) in [
        ("Option<Int8>", "None"),
        ("Option<Int8>", "Some(42)"),
        ("Result<Int8,String>", "Ok(42)"),
        ("Result<Int8,String>", "Err(\"failure\")"),
        ("(Int8,Option<Bool>)", "(42,Some(true))"),
        (
            "{a:Int8,b:Result<Bool,String>}",
            "{a:42,b:Err(\"failure\")}",
        ),
        ("Sequence<Option<Int8>>", "[None,Some(42),None]"),
        ("Set<Option<Int8>>", "Set([Some(42),None,Some(42)])"),
        (
            "OrderedSet<Option<Int8>>",
            "OrderedSet([Some(42),None,Some(42)])",
        ),
        ("Bag<Option<Int8>>", "Bag([Some(42),None,Some(42)])"),
        ("Sequence<Bag<Int8>>", "[Bag([2,1,2]),Bag([])]"),
        (
            "Option<Result<({value:Int8},Sequence<Bool>),String>>",
            "Some(Ok(({value:42},[true,false])))",
        ),
    ] {
        let (bundle, source) = fixture(&format!(
            "fn run()->({ty},{ty})={{let x:{ty} ={literal};(x,roundtrip(x))}};{ROUNDTRIP}"
        ));
        let image = compile(&bundle, &source);
        let result = evaluate(
            &image,
            &[],
            &ExecutionContext::new(&bundle),
            &ExecutionLimits::default(),
        )
        .unwrap_or_else(|e| panic!("{ty}: {e:?}"));
        let DataWire::Tuple(pair) = result.value.data else {
            panic!("Pair")
        };
        assert_eq!(pair.len(), 2);
        assert_eq!(pair[0], pair[1], "{ty}");
        assert!(result.consumed.collection_comparisons > 0);
        assert_eq!(image.program(), compile(&bundle, &source).program());
    }
}

#[test]
fn private_callable_identity_survives_pipeline_but_cannot_escape_as_wire() {
    let (bundle, source) = fixture(&format!(
        "fn run()->Int8={{let captured:Int8=40;let f=|x:Int8|captured+x;let g=roundtrip(f);g(2)}};{ROUNDTRIP}"
    ));
    let image = compile(&bundle, &source);
    let result = evaluate(
        &image,
        &[],
        &ExecutionContext::new(&bundle),
        &ExecutionLimits::default(),
    )
    .unwrap();
    let DataWire::Scalar(value) = result.value.data else {
        panic!("Int8")
    };
    assert_eq!(value.payload, serde_json::json!(42));
    let (bundle, source) = fixture(&format!(
        "fn run()->(Int8)->Int8=roundtrip(|x:Int8|x);{ROUNDTRIP}"
    ));
    let image = compile(&bundle, &source);
    let error = evaluate(
        &image,
        &[],
        &ExecutionContext::new(&bundle),
        &ExecutionLimits::default(),
    )
    .unwrap_err();
    assert_eq!(error.kind, ExecutionErrorKind::Unsupported);
    assert!(error.message.contains("closure transport"));
}

#[test]
fn many_wires_keep_bounds_and_kinds_as_nested_pipeline_values() {
    use intersect_model::definitions::rcore_behavior::ir_v2::{TypeId, TypeNode};
    for ordered in [false, true] {
        for unique in [false, true] {
            let (bundle, source) = fixture(&format!("fn run<T>(x:T)->T=roundtrip(x);{ROUNDTRIP}"));
            let selection = EntrySelection {
                symbol: selected(&source),
                types: vec![
                    TypeNode::Scalar(Scalar::Int8),
                    TypeNode::Many {
                        element: TypeId(0),
                        lower: 2,
                        upper: 3,
                        ordered,
                        unique,
                    },
                ],
                arguments: vec![TypeId(1)],
            };
            let image =
                compile_function(&bundle, &source.id, &selection, &Limits::default()).unwrap();
            let program = image.program();
            let ty = program.functions[program.entry.0 as usize].parameters[0];
            let TypeNode::Many { element, .. } = program.types[ty.0 as usize] else {
                panic!("Many")
            };
            let argument = ValueWire {
                ty,
                data: DataWire::Collection(
                    [2, 1]
                        .into_iter()
                        .map(|n| ValueWire {
                            ty: element,
                            data: DataWire::Scalar(ir::Value {
                                ty: ir::ValueType::Scalar(Scalar::Int8),
                                payload: serde_json::json!(n),
                            }),
                        })
                        .collect(),
                ),
            };
            let context = ExecutionContext::new(&bundle);
            let limits = ExecutionLimits::default();
            assert_eq!(
                evaluate(&image, std::slice::from_ref(&argument), &context, &limits)
                    .unwrap()
                    .value,
                argument
            );
            let mut invalid = argument.clone();
            let DataWire::Collection(values) = &mut invalid.data else {
                panic!("Many")
            };
            values.pop();
            assert_eq!(
                evaluate(&image, &[invalid], &context, &limits)
                    .unwrap_err()
                    .kind,
                ExecutionErrorKind::Arguments
            );
            assert_eq!(
                evaluate(&image, std::slice::from_ref(&argument), &context, &limits)
                    .unwrap()
                    .value,
                argument
            );
        }
    }
}

#[test]
fn enum_aliases_remain_distinct_pipeline_representatives() {
    let model = intersect_model_acceptance::migration_fixture::migrate_source_fixture(DefinitionSource::new(
        "memory:values.ecore",
        SourceKind::Ecore,
        "<e:EPackage xmlns:e='http://www.eclipse.org/emf/2002/Ecore' xmlns:x='http://www.w3.org/2001/XMLSchema-instance' name='m' nsURI='urn:values'><eClassifiers x:type='e:EEnum' name='Mode'><eLiterals name='First' value='7'/><eLiterals name='Alias' value='7'/></eClassifiers></e:EPackage>",
    )).0;
    let (mut bundle, mut source) = fixture(&format!(
        "import model \"urn:values\" as m;fn run()->(m.Mode,m.Mode)={{let a=roundtrip(m.Mode.First);let b=roundtrip(m.Mode.Alias);let check:Int8=if size(distinct([a,b]))==2 {{0}} else {{1/0}};(a,b)}};{ROUNDTRIP}"
    ));
    source.dependencies.push(model.id.clone());
    bundle.sources = vec![source.clone(), model.clone()];
    let image = compile(&bundle, &source);
    let value = evaluate(
        &image,
        &[],
        &ExecutionContext::new(&bundle),
        &ExecutionLimits::default(),
    )
    .unwrap()
    .value;
    let DataWire::Tuple(pair) = value.data else {
        panic!("Pair")
    };
    for (index, value) in pair.iter().enumerate() {
        let DataWire::Enum(intersect_model::definitions::enums::EnumValue::Enum {
            enumeration,
            literal,
        }) = &value.data
        else {
            panic!("Enum")
        };
        assert_eq!(enumeration.source, model.id);
        assert_eq!(enumeration.path, vec![0]);
        assert_eq!(literal.source, model.id);
        assert_eq!(literal.path, vec![0, index]);
    }
    assert_ne!(pair[0], pair[1]);
}
