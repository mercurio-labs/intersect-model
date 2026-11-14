//! Source-backed hostile input controls; every positive traverses a captured closure.
use intersect_model::definitions::{
    bundle::{DefinitionBundle, DefinitionSource, SourceKind},
    rcore_behavior::{
        function_lowering::compile_function,
        ir,
        ir_v2::{
            DataWire, EntrySelection, Limits, TypeId, TypeNode, ValueWire, VerifiedExecutable,
        },
        modules::SymbolId,
        types::Scalar,
    },
};
use intersect_model::rcore_behavior_functions::{
    ExecutionContext, ExecutionErrorKind, ExecutionLimits, evaluate,
};
use serde_json::json;

fn fixture(annotation: &str) -> (DefinitionBundle, VerifiedExecutable, TypeId) {
    let source = DefinitionSource::new(
        "memory:structural-inputs-behavior.rcore",
        SourceKind::RcoreBehavior,
        format!(
            "rcore 1;package inputs version \"1\";fn run(x:{annotation})->{annotation} ={{let f=|unused:Int8|x;f(0)}};"
        ),
    );
    let bundle = DefinitionBundle {
        version: 1,
        sources: vec![source.clone()],
        namespaces: vec![],
    };
    let image = compile_function(
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
    .unwrap();
    let ty = image.program().functions[image.program().entry.0 as usize].parameters[0];
    (bundle, image, ty)
}
fn value(image: &VerifiedExecutable, ty: TypeId) -> ValueWire {
    let data = match image.type_node(ty).unwrap() {
        TypeNode::Scalar(Scalar::Int8) => DataWire::Scalar(ir::Value {
            ty: ir::ValueType::Scalar(Scalar::Int8),
            payload: json!(42),
        }),
        TypeNode::Scalar(Scalar::Bool) => DataWire::Scalar(ir::Value {
            ty: ir::ValueType::Scalar(Scalar::Bool),
            payload: json!(true),
        }),
        TypeNode::Option(element) => DataWire::Some(Box::new(value(image, *element))),
        TypeNode::Result { ok, .. } => DataWire::Ok(Box::new(value(image, *ok))),
        TypeNode::Tuple(items) => DataWire::Tuple(items.iter().map(|t| value(image, *t)).collect()),
        TypeNode::Record(fields) => DataWire::Record(
            fields
                .iter()
                .map(|(n, t)| (n.clone(), value(image, *t)))
                .collect(),
        ),
        TypeNode::Collection { element, .. } => DataWire::Collection(vec![value(image, *element)]),
        other => panic!("Unexpected test type {other:?}"),
    };
    ValueWire { ty, data }
}
fn succeeds(
    bundle: &DefinitionBundle,
    image: &VerifiedExecutable,
    input: &ValueWire,
    limits: &ExecutionLimits,
) {
    let output = evaluate(
        image,
        std::slice::from_ref(input),
        &ExecutionContext::new(bundle),
        limits,
    )
    .unwrap();
    assert_eq!(output.value, *input);
    assert!(
        output.consumed.captures >= 2,
        "capture creation and invocation must execute"
    );
}
fn rejects(bundle: &DefinitionBundle, image: &VerifiedExecutable, input: ValueWire) {
    let error = evaluate(
        image,
        &[input],
        &ExecutionContext::new(bundle),
        &ExecutionLimits::default(),
    )
    .unwrap_err();
    assert_eq!(error.kind, ExecutionErrorKind::Arguments);
    assert_eq!(
        error.consumed.steps, 0,
        "malformed arguments must reject before body execution"
    );
}
#[test]
fn exact_record_tuple_option_and_result_inputs_reject_shape_and_payload_forgery() {
    for annotation in [
        "{left:Int8,right:Bool}",
        "(Int8,Bool)",
        "Option<Int8>",
        "Result<Int8,Bool>",
    ] {
        let (bundle, image, ty) = fixture(annotation);
        let good = value(&image, ty);
        succeeds(&bundle, &image, &good, &ExecutionLimits::default());
        let mut malformed = vec![];
        match &good.data {
            DataWire::Record(fields) => {
                let mut missing = fields.clone();
                missing.remove("right");
                malformed.push(ValueWire {
                    ty,
                    data: DataWire::Record(missing),
                });
                let mut extra = fields.clone();
                extra.insert("extra".into(), fields["left"].clone());
                malformed.push(ValueWire {
                    ty,
                    data: DataWire::Record(extra),
                });
                let mut swapped = fields.clone();
                swapped.insert("right".into(), fields["left"].clone());
                malformed.push(ValueWire {
                    ty,
                    data: DataWire::Record(swapped),
                });
            }
            DataWire::Tuple(items) => {
                malformed.push(ValueWire {
                    ty,
                    data: DataWire::Tuple(items[..1].to_vec()),
                });
                let mut extra = items.clone();
                extra.push(items[0].clone());
                malformed.push(ValueWire {
                    ty,
                    data: DataWire::Tuple(extra),
                });
                let mut swapped = items.clone();
                swapped.swap(0, 1);
                malformed.push(ValueWire {
                    ty,
                    data: DataWire::Tuple(swapped),
                });
            }
            DataWire::Some(inner) => {
                let mut bad = inner.as_ref().clone();
                bad.data = DataWire::Scalar(ir::Value {
                    ty: ir::ValueType::Scalar(Scalar::Bool),
                    payload: json!(true),
                });
                malformed.push(ValueWire {
                    ty,
                    data: DataWire::Some(Box::new(bad)),
                });
                let none = ValueWire {
                    ty,
                    data: DataWire::None,
                };
                succeeds(&bundle, &image, &none, &ExecutionLimits::default());
            }
            DataWire::Ok(inner) => {
                let TypeNode::Result { error, .. } = image.type_node(ty).unwrap() else {
                    panic!("result")
                };
                let err = value(&image, *error);
                succeeds(
                    &bundle,
                    &image,
                    &ValueWire {
                        ty,
                        data: DataWire::Err(Box::new(err.clone())),
                    },
                    &ExecutionLimits::default(),
                );
                malformed.push(ValueWire {
                    ty,
                    data: DataWire::Ok(Box::new(err)),
                });
                malformed.push(ValueWire {
                    ty,
                    data: DataWire::Err(inner.clone()),
                });
            }
            _ => panic!("shape"),
        }
        for bad in malformed {
            rejects(&bundle, &image, bad);
            succeeds(&bundle, &image, &good, &ExecutionLimits::default());
        }
    }
}
#[test]
fn deep_and_wide_external_values_hit_runtime_preflight_limits_then_recover() {
    let mut annotation = "Int8".to_owned();
    for _ in 0..20 {
        annotation = format!("Option<{annotation}>");
    }
    let (bundle, image, ty) = fixture(&annotation);
    let deep = value(&image, ty);
    succeeds(&bundle, &image, &deep, &ExecutionLimits::default());
    let limits = ExecutionLimits {
        max_value_depth: 20,
        ..ExecutionLimits::default()
    };
    let error = evaluate(
        &image,
        std::slice::from_ref(&deep),
        &ExecutionContext::new(&bundle),
        &limits,
    )
    .unwrap_err();
    assert_eq!(error.kind, ExecutionErrorKind::ResourceExhausted);
    assert_eq!(error.consumed.steps, 0);
    assert!(error.message.to_lowercase().contains("depth"), "{error:?}");
    succeeds(&bundle, &image, &deep, &ExecutionLimits::default());

    let (bundle, image, ty) = fixture("Sequence<Int8>");
    let TypeNode::Collection { element, .. } = image.type_node(ty).unwrap() else {
        panic!("collection")
    };
    let wide = ValueWire {
        ty,
        data: DataWire::Collection(vec![value(&image, *element); 2048]),
    };
    succeeds(&bundle, &image, &wide, &ExecutionLimits::default());
    let limits = ExecutionLimits {
        max_value_nodes: 128,
        ..ExecutionLimits::default()
    };
    let error = evaluate(
        &image,
        std::slice::from_ref(&wide),
        &ExecutionContext::new(&bundle),
        &limits,
    )
    .unwrap_err();
    assert_eq!(error.kind, ExecutionErrorKind::ResourceExhausted);
    assert_eq!(error.consumed.steps, 0);
    assert!(error.message.to_lowercase().contains("node"), "{error:?}");
    succeeds(&bundle, &image, &wide, &ExecutionLimits::default());
}
#[test]
fn external_function_handles_have_no_deserializable_executable_representation() {
    let (bundle, image, ty) = fixture("(Int8)->Int8");
    for tag in ["Function", "Closure"] {
        let wire = json!({"ty":ty,"data":{(tag):{"function":0,"captures":[],"image":"forged"}}});
        let error = serde_json::from_value::<ValueWire>(wire).unwrap_err();
        assert!(error.to_string().contains("unknown variant"), "{error}");
    }
    // A valid non-executable wire cannot masquerade as a function parameter either.
    rejects(
        &bundle,
        &image,
        ValueWire {
            ty,
            data: DataWire::None,
        },
    );
    let (bundle, image, ty) = fixture("Int8");
    succeeds(
        &bundle,
        &image,
        &value(&image, ty),
        &ExecutionLimits::default(),
    );
}
