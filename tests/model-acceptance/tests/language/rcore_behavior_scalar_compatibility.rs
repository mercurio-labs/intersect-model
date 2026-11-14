use intersect_model::definitions::{
    bundle::{DefinitionBundle, DefinitionSource, SourceKind, SourceSpan},
    enums::EnumValue,
    rcore_behavior::{
        ir::{
            self, Block, Edge, Instruction, InstructionKind, Operand, Terminator, Value, ValueType,
            VerificationLimits, VerifiedProgram,
        },
        lowering::compile_scalar,
        modules::SymbolId,
        types::Scalar,
    },
    resources::DeclarationId,
};
use intersect_model::rcore_behavior_execution::{ExecutionLimits, evaluate};
use serde_json::{Value as Json, json};

fn value(ty: Scalar, payload: Json) -> Value {
    Value {
        ty: ValueType::Scalar(ty),
        payload,
    }
}
fn size(v: &Value) -> usize {
    serde_json::to_vec(v).unwrap().len()
}
fn source(body: &str) -> (DefinitionBundle, SymbolId) {
    let source = DefinitionSource::new(
        "memory:compat-behavior.rcore",
        SourceKind::RcoreBehavior,
        format!("rcore 1; package compatibility version \"1\"; {body}"),
    );
    let symbol = SymbolId {
        source: source.id.clone(),
        declaration: 0,
    };
    (
        DefinitionBundle {
            version: 1,
            sources: vec![source],
            namespaces: vec![],
        },
        symbol,
    )
}
fn compile(bundle: &DefinitionBundle, symbol: &SymbolId) -> VerifiedProgram {
    compile_scalar(
        bundle,
        &symbol.source,
        symbol,
        &VerificationLimits::default(),
    )
    .unwrap()
}
fn error(symbol: &SymbolId, kind: &str, message: &str, span: Option<&SourceSpan>) -> Json {
    json!({"kind":kind,"message":message,"symbol":symbol,"span":span})
}
fn result(v: &Value, steps: usize, work: u64, bytes: usize) -> Json {
    json!({"value":v,"steps":steps,"numeric_work":work,"value_bytes":bytes})
}

#[test]
fn all_eighteen_scalar_inputs_preserve_wire_and_exact_identity_accounting() {
    let samples = [
        value(Scalar::Bool, json!(true)),
        value(Scalar::String, json!("hello")),
        value(Scalar::Bytes, json!({"kind":"bytes","hex":"00FF"})),
        value(Scalar::Char16, json!({"kind":"char16","unit":55296})),
        value(
            Scalar::DateInstant,
            json!({"kind":"date","milliseconds":"9007199254740993"}),
        ),
        value(Scalar::Unit, json!({"kind":"unit"})),
        value(Scalar::Int8, json!(-128)),
        value(Scalar::Int16, json!(-32768)),
        value(Scalar::Int32, json!(-2147483648i64)),
        value(
            Scalar::Int64,
            json!({"kind":"int64","value":"-9223372036854775808"}),
        ),
        value(Scalar::UInt8, json!(255)),
        value(Scalar::UInt16, json!(65535)),
        value(Scalar::UInt32, json!(4294967295u64)),
        value(
            Scalar::UInt64,
            json!({"kind":"uint64","value":"18446744073709551615"}),
        ),
        value(
            Scalar::BigInt,
            json!({"kind":"bigint","value":"123456789012345678901234567890"}),
        ),
        value(
            Scalar::Decimal,
            json!({"kind":"decimal","coefficient":"125","scale":2}),
        ),
        value(Scalar::Float32, json!({"kind":"float32","bits":"7fc00001"})),
        value(
            Scalar::Float64,
            json!({"kind":"float64","bits":"8000000000000000"}),
        ),
    ];
    for v in samples {
        let ValueType::Scalar(s) = v.ty else {
            unreachable!()
        };
        let (bundle, symbol) = source(&format!("fn identity(x:{s:?})->{s:?}=x;"));
        let verified = compile(&bundle, &symbol);
        let expected = result(&v, 1, 0, size(&v));
        let limits = ExecutionLimits {
            max_steps: 1,
            max_value_bytes: size(&v),
            ..ExecutionLimits::default()
        };
        assert_eq!(
            serde_json::to_value(evaluate(&verified, std::slice::from_ref(&v), &limits).unwrap())
                .unwrap(),
            expected,
            "{s:?}"
        );
        let mut short = limits.clone();
        short.max_value_bytes -= 1;
        assert_eq!(
            serde_json::to_value(
                evaluate(&verified, std::slice::from_ref(&v), &short).unwrap_err()
            )
            .unwrap(),
            error(
                &symbol,
                "ResourceExhausted",
                "Cumulative scalar value byte budget exceeded",
                None
            )
        );
        short = limits;
        short.max_steps = 0;
        assert_eq!(
            serde_json::to_value(
                evaluate(&verified, std::slice::from_ref(&v), &short).unwrap_err()
            )
            .unwrap(),
            error(
                &symbol,
                "ResourceExhausted",
                "Scalar execution step budget exceeded",
                Some(&verified.program().blocks[0].span)
            )
        );
    }
}

#[test]
fn selected_edge_copy_cost_and_lazy_oversized_constants_are_frozen() {
    let (bundle, symbol) = source("fn choose(flag:Bool,x:String)->String=x;");
    let original = compile(&bundle, &symbol);
    let mut p = original.program().clone();
    let span = p.blocks[0].span.clone();
    let huge = value(Scalar::String, json!("x".repeat(1024)));
    p.blocks = vec![
        Block {
            parameters: p.parameters.clone(),
            instructions: vec![],
            terminator: Terminator::Branch {
                condition: Operand::Parameter(0),
                then_edge: Edge {
                    block: 1,
                    arguments: vec![Operand::Parameter(1)],
                },
                else_edge: Edge {
                    block: 2,
                    arguments: vec![],
                },
            },
            span: span.clone(),
        },
        Block {
            parameters: vec![ValueType::Scalar(Scalar::String)],
            instructions: vec![],
            terminator: Terminator::Return(Operand::Parameter(0)),
            span: span.clone(),
        },
        Block {
            parameters: vec![],
            instructions: vec![Instruction {
                ty: huge.ty.clone(),
                kind: InstructionKind::Constant(huge),
                span: span.clone(),
            }],
            terminator: Terminator::Return(Operand::Register(0)),
            span: span.clone(),
        },
    ];
    let verified = ir::verify(p, &bundle, &VerificationLimits::default()).unwrap();
    let yes = value(Scalar::Bool, json!(true));
    let text = value(Scalar::String, json!("ok"));
    let bytes = size(&yes) + 2 * size(&text);
    let mut limits = ExecutionLimits {
        max_steps: 2,
        max_value_bytes: bytes,
        ..ExecutionLimits::default()
    };
    limits.values.max_value_bytes = 128;
    assert_eq!(
        serde_json::to_value(evaluate(&verified, &[yes.clone(), text.clone()], &limits).unwrap())
            .unwrap(),
        result(&text, 2, 0, bytes)
    );
    let mut short = limits.clone();
    short.max_value_bytes -= 1;
    assert_eq!(
        serde_json::to_value(
            evaluate(&verified, &[yes.clone(), text.clone()], &short).unwrap_err()
        )
        .unwrap(),
        error(
            &symbol,
            "ResourceExhausted",
            "Cumulative scalar value byte budget exceeded",
            Some(&span)
        )
    );
    short = limits.clone();
    short.max_steps = 1;
    assert_eq!(
        serde_json::to_value(evaluate(&verified, &[yes, text.clone()], &short).unwrap_err())
            .unwrap(),
        error(
            &symbol,
            "ResourceExhausted",
            "Scalar execution step budget exceeded",
            Some(&span)
        )
    );
    assert_eq!(
        serde_json::to_value(
            evaluate(
                &verified,
                &[value(Scalar::Bool, json!(false)), text],
                &limits
            )
            .unwrap_err()
        )
        .unwrap(),
        error(
            &symbol,
            "ResourceExhausted",
            "Scalar value byte limit exceeded",
            Some(&span)
        )
    );
}

#[test]
fn argument_count_type_and_malformed_value_precedence_precedes_execution_limits() {
    let (bundle, symbol) = source("fn first(a:Bool,b:Bool)->Bool=a;");
    let verified = compile(&bundle, &symbol);
    let mut limits = ExecutionLimits {
        max_steps: 0,
        max_value_bytes: 0,
        ..ExecutionLimits::default()
    };
    let bad = value(Scalar::Bool, Json::Null);
    let wrong = value(Scalar::String, json!("bad"));
    assert_eq!(
        serde_json::to_value(evaluate(&verified, std::slice::from_ref(&bad), &limits).unwrap_err())
            .unwrap(),
        error(
            &symbol,
            "Arguments",
            "Scalar argument count differs from selected signature",
            None
        )
    );
    assert_eq!(
        serde_json::to_value(
            evaluate(&verified, &[wrong.clone(), bad.clone()], &limits).unwrap_err()
        )
        .unwrap(),
        error(
            &symbol,
            "Arguments",
            "Scalar argument type differs from selected signature",
            None
        )
    );
    assert_eq!(
        serde_json::to_value(evaluate(&verified, &[bad, wrong], &limits).unwrap_err()).unwrap(),
        error(&symbol, "Arguments", "Null is not a scalar value", None)
    );
    limits.max_steps = 1;
    limits.max_value_bytes = 1024;
    let yes = value(Scalar::Bool, json!(true));
    let no = value(Scalar::Bool, json!(false));
    assert_eq!(
        serde_json::to_value(evaluate(&verified, &[yes.clone(), no.clone()], &limits).unwrap())
            .unwrap(),
        result(&yes, 1, 0, size(&yes) + size(&no))
    );
}

#[test]
fn enum_alias_equality_charges_exact_v1_values_and_numeric_boundary() {
    let model = intersect_model_acceptance::migration_fixture::migrate_source_fixture(DefinitionSource::new(
        "memory:compat.ecore",
        SourceKind::Ecore,
        r#"<e:EPackage xmlns:e="http://www.eclipse.org/emf/2002/Ecore" xmlns:x="http://www.w3.org/2001/XMLSchema-instance" name="m" nsURI="urn:compat"><eClassifiers x:type="e:EEnum" name="Mode"><eLiterals name="First" value="7"/><eLiterals name="Alias" value="7"/></eClassifiers></e:EPackage>"#,
    )).0;
    let enumeration = DeclarationId {
        source: model.id.clone(),
        path: vec![0],
    };
    let (mut bundle, mut symbol) =
        source("import model \"urn:compat\" as m; fn equal(a:m.Mode,b:m.Mode)->Bool=a==b;");
    symbol.declaration = 1;
    bundle.sources[0].dependencies.push(model.id.clone());
    bundle.sources.push(model);
    let verified = compile(&bundle, &symbol);
    let member = |i| Value {
        ty: ValueType::Enum(enumeration.clone()),
        payload: serde_json::to_value(EnumValue::Enum {
            enumeration: enumeration.clone(),
            literal: DeclarationId {
                source: enumeration.source.clone(),
                path: vec![0, i],
            },
        })
        .unwrap(),
    };
    let a = member(0);
    let b = member(1);
    let answer = value(Scalar::Bool, json!(false));
    let cost = (size(&a) + size(&b)) as u64;
    let bytes = size(&a) + size(&b) + size(&answer);
    let mut limits = ExecutionLimits {
        max_steps: 2,
        max_value_bytes: bytes,
        ..ExecutionLimits::default()
    };
    limits.numeric.max_work = cost;
    assert_eq!(
        serde_json::to_value(evaluate(&verified, &[a.clone(), b.clone()], &limits).unwrap())
            .unwrap(),
        result(&answer, 2, cost, bytes)
    );
    limits.numeric.max_work = cost - 1;
    let span = &verified.program().blocks[0].instructions[0].span;
    let mut expected = error(
        &symbol,
        "ResourceExhausted",
        "Numeric work limit exceeded",
        Some(span),
    );
    expected["arithmetic_kind"] = json!("resource_limit");
    assert_eq!(
        serde_json::to_value(evaluate(&verified, &[a, b], &limits).unwrap_err()).unwrap(),
        expected
    );
}
