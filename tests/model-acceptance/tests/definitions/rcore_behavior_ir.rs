use intersect_model::definitions::{
    bundle::{DefinitionBundle, DefinitionSource, SourceKind, SourceSpan},
    rcore_behavior::{ir::*, modules::SymbolId, types::Scalar},
};

fn fixture() -> (DefinitionBundle, Program) {
    let source = DefinitionSource::new(
        "memory:ir",
        SourceKind::RcoreBehavior,
        "rcore 1; package ir version \"1\"; fn sum(a:Int32,b:Int32)->Int32=a+b;",
    );
    let span = SourceSpan {
        source: source.id.clone(),
        start: 0,
        end: source.text.len(),
    };
    let int = ValueType::Scalar(Scalar::Int32);
    let program = Program {
        version: VERSION,
        sources: vec![SourceManifestEntry {
            source: source.id.clone(),
        }],
        symbol: SymbolId {
            source: source.id.clone(),
            declaration: 0,
        },
        parameters: vec![int.clone(), int.clone()],
        result: int.clone(),
        blocks: vec![Block {
            parameters: vec![int.clone(), int.clone()],
            instructions: vec![Instruction {
                ty: int,
                kind: InstructionKind::Binary {
                    op: BinaryOp::Add,
                    left: Operand::Parameter(0),
                    right: Operand::Parameter(1),
                },
                span: span.clone(),
            }],
            terminator: Terminator::Return(Operand::Register(0)),
            span,
        }],
    };
    (
        DefinitionBundle {
            version: 1,
            sources: vec![source],
            namespaces: vec![],
        },
        program,
    )
}

#[test]
fn scalar_ir_roundtrip_and_hostile_identity_type_and_operand_controls() {
    let (bundle, program) = fixture();
    verify(program.clone(), &bundle, &VerificationLimits::default()).unwrap();
    let wire = serde_json::to_string(&program).unwrap();
    let decoded: Program = serde_json::from_str(&wire).unwrap();
    verify(decoded, &bundle, &VerificationLimits::default()).unwrap();
    let mut controls = Vec::new();
    let mut p = program.clone();
    p.version += 1;
    controls.push(p);
    let mut p = program.clone();
    p.sources[0].source.sha256 = "stale".into();
    controls.push(p);
    let mut p = program.clone();
    p.symbol.declaration = 99;
    controls.push(p);
    let mut p = program.clone();
    p.result = ValueType::Scalar(Scalar::Int64);
    controls.push(p);
    let mut p = program.clone();
    p.blocks[0].instructions[0].ty = ValueType::Scalar(Scalar::Bool);
    controls.push(p);
    let mut p = program.clone();
    p.blocks[0].instructions[0].kind = InstructionKind::Unary {
        op: UnaryOp::Plus,
        value: Operand::Register(0),
    };
    controls.push(p);
    let mut p = program.clone();
    p.blocks[0].terminator = Terminator::Return(Operand::Parameter(2));
    controls.push(p);
    let mut p = program.clone();
    p.blocks[0].span.end += 1;
    controls.push(p);
    let mut p = program.clone();
    p.blocks[0].instructions[0].kind = InstructionKind::Constant(Value {
        ty: ValueType::Scalar(Scalar::Int32),
        payload: serde_json::json!({"kind":"int64","value":"1"}),
    });
    controls.push(p);
    for control in controls {
        assert!(verify(control, &bundle, &VerificationLimits::default()).is_err());
    }
    let mut raw = serde_json::to_value(&program).unwrap();
    raw["trusted"] = serde_json::json!(true);
    assert!(serde_json::from_value::<Program>(raw).is_err());
    let mut raw = serde_json::to_value(&program).unwrap();
    raw["symbol"]["trusted"] = serde_json::json!(true);
    assert!(serde_json::from_value::<Program>(raw).is_err());
    let enum_type = ValueType::Enum(intersect_model::definitions::resources::DeclarationId {
        source: bundle.sources[0].id.clone(),
        path: vec![0],
    });
    let mut raw = serde_json::to_value(&enum_type).unwrap();
    assert!(serde_json::from_value::<ValueType>(raw.clone()).is_ok());
    raw["Enum"]["trusted"] = serde_json::json!(true);
    assert!(serde_json::from_value::<ValueType>(raw).is_err());
    verify(program, &bundle, &VerificationLimits::default()).unwrap();
}

#[test]
fn forward_edges_have_exact_parameters_and_reject_cycles_and_dead_islands() {
    let (bundle, mut program) = fixture();
    let int = ValueType::Scalar(Scalar::Int32);
    let span = program.blocks[0].span.clone();
    program.blocks.push(Block {
        parameters: vec![int.clone()],
        instructions: vec![],
        terminator: Terminator::Return(Operand::Parameter(0)),
        span,
    });
    program.blocks[0].terminator = Terminator::Jump(Edge {
        block: 1,
        arguments: vec![Operand::Register(0)],
    });
    verify(program.clone(), &bundle, &VerificationLimits::default()).unwrap();
    let mut wrong = program.clone();
    wrong.blocks[0].terminator = Terminator::Jump(Edge {
        block: 1,
        arguments: vec![],
    });
    assert!(
        verify(wrong, &bundle, &VerificationLimits::default())
            .unwrap_err()
            .message
            .contains("arity")
    );
    let mut wrong = program.clone();
    wrong.blocks[1].parameters[0] = ValueType::Scalar(Scalar::Bool);
    assert!(
        verify(wrong, &bundle, &VerificationLimits::default())
            .unwrap_err()
            .message
            .contains("type")
    );
    let mut wrong = program.clone();
    wrong.blocks[1].terminator = Terminator::Jump(Edge {
        block: 0,
        arguments: vec![],
    });
    assert!(
        verify(wrong, &bundle, &VerificationLimits::default())
            .unwrap_err()
            .message
            .contains("forward")
    );
    let mut wrong = program.clone();
    wrong.blocks[0].terminator = Terminator::Return(Operand::Register(0));
    assert!(
        verify(wrong, &bundle, &VerificationLimits::default())
            .unwrap_err()
            .message
            .contains("Unreachable")
    );
    let mut wrong = program.clone();
    wrong.blocks[0].terminator = Terminator::Branch {
        condition: Operand::Parameter(0),
        then_edge: Edge {
            block: 1,
            arguments: vec![Operand::Register(0)],
        },
        else_edge: Edge {
            block: 1,
            arguments: vec![Operand::Register(0)],
        },
    };
    assert!(
        verify(wrong, &bundle, &VerificationLimits::default())
            .unwrap_err()
            .message
            .contains("Bool")
    );
    verify(program, &bundle, &VerificationLimits::default()).unwrap();
}

#[test]
fn verifier_aggregate_limits_and_bounded_exact_literals_recover() {
    let (bundle, program) = fixture();
    let limits = VerificationLimits {
        max_work: 1,
        ..VerificationLimits::default()
    };
    assert_eq!(
        verify(program.clone(), &bundle, &limits).unwrap_err().kind,
        IrErrorKind::ResourceExhausted
    );
    verify(program, &bundle, &VerificationLimits::default()).unwrap();
    let limits = ValueLimits {
        max_digits: 8,
        ..ValueLimits::default()
    };
    use intersect_model::definitions::rcore_behavior::ast::Literal;
    assert!(literal_value(Scalar::BigInt, &Literal::Number("1e999999".into()), &limits).is_err());
    assert_eq!(
        literal_value(
            Scalar::BigInt,
            &Literal::Number("0e9999999999999999999999999".into()),
            &limits
        )
        .unwrap()
        .payload,
        serde_json::json!({"kind":"bigint","value":"0"})
    );
    assert_eq!(
        literal_value(Scalar::BigInt, &Literal::Number("1.2e2".into()), &limits)
            .unwrap()
            .payload,
        serde_json::json!({"kind":"bigint","value":"120"})
    );
    assert_eq!(
        literal_value(
            Scalar::UInt64,
            &Literal::Number("18446744073709551615".into()),
            &ValueLimits::default()
        )
        .unwrap()
        .payload,
        serde_json::json!({"kind":"uint64","value":"18446744073709551615"})
    );
    assert!(
        validate_scalar_value(
            &Value {
                ty: ValueType::Scalar(Scalar::String),
                payload: serde_json::Value::Null
            },
            &limits
        )
        .is_err()
    );
}

#[test]
fn all_scalar_tags_preserve_exact_payloads_and_resource_errors() {
    use serde_json::json;
    let values = [
        (Scalar::Bool, json!(true)),
        (Scalar::String, json!("hello")),
        (Scalar::Bytes, json!({"kind":"bytes","hex":"00FF"})),
        (Scalar::Char16, json!({"kind":"char16","unit":55296})),
        (
            Scalar::DateInstant,
            json!({"kind":"date","milliseconds":"9223372036854775807"}),
        ),
        (Scalar::Unit, json!({"kind":"unit"})),
        (Scalar::Int8, json!(-128)),
        (Scalar::Int16, json!(-32768)),
        (Scalar::Int32, json!(-2147483648i64)),
        (
            Scalar::Int64,
            json!({"kind":"int64","value":"-9223372036854775808"}),
        ),
        (Scalar::UInt8, json!(255)),
        (Scalar::UInt16, json!(65535)),
        (Scalar::UInt32, json!(4294967295u64)),
        (
            Scalar::UInt64,
            json!({"kind":"uint64","value":"18446744073709551615"}),
        ),
        (
            Scalar::BigInt,
            json!({"kind":"bigint","value":"123456789012345678901234567890"}),
        ),
        (
            Scalar::Decimal,
            json!({"kind":"decimal","coefficient":"12300","scale":4}),
        ),
        (Scalar::Float32, json!({"kind":"float32","bits":"80000000"})),
        (
            Scalar::Float64,
            json!({"kind":"float64","bits":"7ff0000000000000"}),
        ),
    ];
    for (scalar, payload) in values {
        let value = Value {
            ty: ValueType::Scalar(scalar),
            payload,
        };
        validate_scalar_value(&value, &ValueLimits::default()).unwrap();
        let restored: Value =
            serde_json::from_str(&serde_json::to_string(&value).unwrap()).unwrap();
        assert_eq!(restored, value);
        assert_eq!(
            binary_type(BinaryOp::Equal, &value.ty, &value.ty).unwrap(),
            ValueType::Scalar(Scalar::Bool)
        );
    }
    let (bundle, program) = fixture();
    let checked = verify(program, &bundle, &VerificationLimits::default()).unwrap();
    let too_large = Value {
        ty: ValueType::Scalar(Scalar::BigInt),
        payload: json!({"kind":"bigint","value":"12345"}),
    };
    assert_eq!(
        checked
            .validate_value_checked(
                &too_large,
                &ValueLimits {
                    max_digits: 4,
                    ..ValueLimits::default()
                }
            )
            .unwrap_err()
            .kind,
        IrErrorKind::ResourceExhausted
    );
    let invalid = Value {
        ty: ValueType::Scalar(Scalar::Int8),
        payload: json!(128),
    };
    assert_eq!(
        checked
            .validate_value_checked(&invalid, &ValueLimits::default())
            .unwrap_err()
            .kind,
        IrErrorKind::Invalid
    );
    let mut nested = json!(0);
    for _ in 0..66 {
        nested = json!([nested]);
    }
    assert_eq!(
        validate_value_resources(
            &Value {
                ty: ValueType::Scalar(Scalar::Unit),
                payload: nested
            },
            &ValueLimits::default()
        )
        .unwrap_err()
        .kind,
        IrErrorKind::ResourceExhausted
    );
}

#[test]
fn rcore_behavior_ieee_input_preserves_nan_payloads_without_weakening_ecore_storage() {
    use intersect_model::definitions::scalars::Builtin;
    for (scalar, builtin, kind, bits) in [
        (Scalar::Float32, Builtin::EFloat, "float32", "7fc01234"),
        (Scalar::Float32, Builtin::EFloat, "float32", "ffc01234"),
        (Scalar::Float32, Builtin::EFloat, "float32", "7f800001"),
        (Scalar::Float32, Builtin::EFloat, "float32", "ff800001"),
        (
            Scalar::Float64,
            Builtin::EDouble,
            "float64",
            "7ff8000000001234",
        ),
        (
            Scalar::Float64,
            Builtin::EDouble,
            "float64",
            "fff8000000001234",
        ),
        (
            Scalar::Float64,
            Builtin::EDouble,
            "float64",
            "7ff0000000000001",
        ),
        (
            Scalar::Float64,
            Builtin::EDouble,
            "float64",
            "fff0000000000001",
        ),
    ] {
        let payload = serde_json::json!({"kind":kind,"bits":bits});
        let value = Value {
            ty: ValueType::Scalar(scalar),
            payload: payload.clone(),
        };
        validate_scalar_value(&value, &ValueLimits::default()).unwrap();
        assert!(
            builtin
                .validate(&payload)
                .unwrap_err()
                .contains("Noncanonical NaN")
        );
        assert_eq!(value.payload, payload);
    }
    for bits in ["7FC01234", "7fc0123", "7fc012345", "gfc01234"] {
        assert!(
            validate_scalar_value(
                &Value {
                    ty: ValueType::Scalar(Scalar::Float32),
                    payload: serde_json::json!({"kind":"float32","bits":bits})
                },
                &ValueLimits::default()
            )
            .is_err()
        );
    }
}

#[test]
fn unknown_effects_and_opcodes_cannot_enter_executable_ir() {
    let (_, program) = fixture();
    let mut opcode = serde_json::to_value(&program).unwrap();
    opcode["blocks"][0]["instructions"][0]["kind"] =
        serde_json::json!({"HostCall":{"name":"clock"}});
    assert!(serde_json::from_value::<Program>(opcode).is_err());
    let mut effect = serde_json::to_value(&program).unwrap();
    effect["effect"] = serde_json::json!("host");
    assert!(serde_json::from_value::<Program>(effect).is_err());
}

#[test]
fn untaken_branch_still_requires_well_typed_instructions() {
    let (bundle, mut program) = fixture();
    let span = program.blocks[0].span.clone();
    program.blocks[0].instructions.push(Instruction {
        ty: ValueType::Scalar(Scalar::Bool),
        kind: InstructionKind::Constant(Value {
            ty: ValueType::Scalar(Scalar::Bool),
            payload: serde_json::json!(true),
        }),
        span: span.clone(),
    });
    let branch = Block {
        parameters: vec![],
        instructions: vec![Instruction {
            ty: ValueType::Scalar(Scalar::Int32),
            kind: InstructionKind::Constant(Value {
                ty: ValueType::Scalar(Scalar::Int32),
                payload: serde_json::json!(1),
            }),
            span: span.clone(),
        }],
        terminator: Terminator::Return(Operand::Register(0)),
        span,
    };
    program.blocks.extend([branch.clone(), branch]);
    program.blocks[0].terminator = Terminator::Branch {
        condition: Operand::Register(1),
        then_edge: Edge {
            block: 1,
            arguments: vec![],
        },
        else_edge: Edge {
            block: 2,
            arguments: vec![],
        },
    };
    verify(program.clone(), &bundle, &VerificationLimits::default()).unwrap();
    let span = program.blocks[2].span.clone();
    program.blocks[2].instructions.push(Instruction {
        ty: ValueType::Scalar(Scalar::Bool),
        kind: InstructionKind::Unary {
            op: UnaryOp::Not,
            value: Operand::Register(0),
        },
        span,
    });
    assert!(
        verify(program, &bundle, &VerificationLimits::default())
            .unwrap_err()
            .message
            .contains("unary operand")
    );
}

#[test]
fn constants_share_one_aggregate_byte_budget() {
    let (bundle, mut program) = fixture();
    let span = program.blocks[0].span.clone();
    let constant = Instruction {
        ty: ValueType::Scalar(Scalar::Int32),
        kind: InstructionKind::Constant(Value {
            ty: ValueType::Scalar(Scalar::Int32),
            payload: serde_json::json!(1234),
        }),
        span,
    };
    program.blocks[0].instructions = vec![constant.clone(), constant];
    let limits = VerificationLimits {
        max_total_value_bytes: 7,
        ..VerificationLimits::default()
    };
    assert_eq!(
        verify(program.clone(), &bundle, &limits).unwrap_err().kind,
        IrErrorKind::ResourceExhausted
    );
    let limits = VerificationLimits {
        max_total_value_bytes: 8,
        ..limits
    };
    verify(program, &bundle, &limits).unwrap();
}

#[test]
fn metadata_preflight_bounds_empty_sources_ids_dependencies_and_program_spans() {
    let (bundle, program) = fixture();
    let limits = VerificationLimits {
        max_source_bytes: 1024,
        max_work: 100,
        ..VerificationLimits::default()
    };
    preflight_bundle(&bundle, &limits).unwrap();
    let mut oversized = bundle.clone();
    oversized.sources[0].id.uri = "u".repeat(2048);
    assert_eq!(
        preflight_bundle(&oversized, &limits).unwrap_err().kind,
        IrErrorKind::ResourceExhausted
    );
    let mut oversized = bundle.clone();
    let mut id = oversized.sources[0].id.clone();
    id.uri = "d".repeat(2048);
    oversized.sources[0].dependencies.push(id);
    assert_eq!(
        preflight_bundle(&oversized, &limits).unwrap_err().kind,
        IrErrorKind::ResourceExhausted
    );
    let empty = DefinitionSource::new("memory:empty", SourceKind::RcoreBehavior, "");
    let many = DefinitionBundle {
        version: 1,
        sources: vec![empty; 101],
        namespaces: vec![],
    };
    assert_eq!(
        preflight_bundle(&many, &limits).unwrap_err().kind,
        IrErrorKind::ResourceExhausted
    );
    let mut oversized = program.clone();
    oversized.blocks[0].span.source.uri = "s".repeat(2048);
    assert_eq!(
        verify(oversized, &bundle, &limits).unwrap_err().kind,
        IrErrorKind::ResourceExhausted
    );
    let mut oversized = program.clone();
    oversized.blocks[0].instructions[0].ty =
        ValueType::Enum(intersect_model::definitions::resources::DeclarationId {
            source: bundle.sources[0].id.clone(),
            path: vec![0; 1024],
        });
    assert_eq!(
        verify(oversized, &bundle, &limits).unwrap_err().kind,
        IrErrorKind::ResourceExhausted
    );
    verify(program, &bundle, &VerificationLimits::default()).unwrap();
}
