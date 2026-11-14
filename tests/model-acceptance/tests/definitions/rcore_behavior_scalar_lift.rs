use intersect_model::definitions::{
    bundle::{DefinitionBundle, DefinitionSource, SourceKind},
    enums::EnumValue,
    rcore_behavior::{ir, ir_v2, lowering::compile_scalar, modules::SymbolId, types::Scalar},
    resources::DeclarationId,
};

fn compile(body: &str) -> ir::VerifiedProgram {
    let source = DefinitionSource::new(
        "memory:scalar-lift-behavior.rcore",
        SourceKind::RcoreBehavior,
        format!("rcore 1; package lift version \"1\"; {body}"),
    );
    let symbol = SymbolId {
        source: source.id.clone(),
        declaration: 0,
    };
    compile_scalar(
        &DefinitionBundle {
            version: 1,
            sources: vec![source],
            namespaces: vec![],
        },
        &symbol.source,
        &symbol,
        &ir::VerificationLimits::default(),
    )
    .unwrap()
}

#[test]
fn all_scalar_types_lift_from_existing_authority_without_changing_v1_wire() {
    for scalar in [
        Scalar::Bool,
        Scalar::String,
        Scalar::Bytes,
        Scalar::Char16,
        Scalar::DateInstant,
        Scalar::Unit,
        Scalar::Int8,
        Scalar::Int16,
        Scalar::Int32,
        Scalar::Int64,
        Scalar::UInt8,
        Scalar::UInt16,
        Scalar::UInt32,
        Scalar::UInt64,
        Scalar::BigInt,
        Scalar::Decimal,
        Scalar::Float32,
        Scalar::Float64,
    ] {
        let original = compile(&format!("fn identity(x:{scalar:?})->{scalar:?}=x;"));
        let wire = serde_json::to_value(original.program()).unwrap();
        let lifted = ir_v2::lift_scalar(&original).unwrap();
        assert!(std::ptr::eq(lifted.original(), &original));
        let ty = lifted.type_id(&ir::ValueType::Scalar(scalar)).unwrap();
        assert_eq!(
            lifted.type_node(ty).unwrap(),
            &ir_v2::TypeNode::Scalar(scalar)
        );
        assert_eq!(
            lifted.value_type(ty).unwrap(),
            &ir::ValueType::Scalar(scalar)
        );
        assert!(lifted.permits(ty, ty).unwrap());
        assert!(
            lifted
                .permits(ir_v2::TypeId(u32::MAX), ir_v2::TypeId(u32::MAX))
                .is_err()
        );
        assert_eq!(lifted.program().functions.len(), 1);
        assert!(lifted.program().functions[0].captures.is_empty());
        assert_eq!(lifted.program().functions[0].parameters, vec![ty]);
        assert_eq!(lifted.program().functions[0].result, ty);
        assert_eq!(serde_json::to_value(original.program()).unwrap(), wire);
        assert_eq!(
            ir_v2::lift_scalar(&original).unwrap().program(),
            lifted.program()
        );
    }
}

#[test]
fn normalized_blocks_preserve_operations_edges_spans_and_exact_type_relations() {
    let original = compile("fn choose(x:Int32,b:Bool)->Int32=if b {-x} else {x+1};");
    let lifted = ir_v2::lift_scalar(&original).unwrap();
    let int = lifted
        .type_id(&ir::ValueType::Scalar(Scalar::Int32))
        .unwrap();
    let boolean = lifted
        .type_id(&ir::ValueType::Scalar(Scalar::Bool))
        .unwrap();
    assert!(!lifted.permits(int, boolean).unwrap());
    assert!(!lifted.permits(boolean, int).unwrap());
    let function = &lifted.program().functions[0];
    assert_eq!(lifted.program().sources, original.program().sources);
    assert_eq!(
        function.identity,
        ir_v2::FunctionIdentity::Named {
            symbol: original.program().symbol.clone(),
            type_arguments: vec![]
        }
    );
    assert_eq!(function.blocks.len(), original.program().blocks.len());
    for (before, after) in original.program().blocks.iter().zip(&function.blocks) {
        assert_eq!(before.span, after.span);
        assert_eq!(
            before
                .parameters
                .iter()
                .map(|ty| lifted.type_id(ty).unwrap())
                .collect::<Vec<_>>(),
            after.parameters
        );
        assert_eq!(before.instructions.len(), after.instructions.len());
        // Scalar-only operation/edge wire shapes are retained; only result type IDs change.
        assert_eq!(
            serde_json::to_value(&before.terminator).unwrap(),
            serde_json::to_value(&after.terminator).unwrap()
        );
        for (before, after) in before.instructions.iter().zip(&after.instructions) {
            assert_eq!(before.span, after.span);
            assert_eq!(lifted.value_type(after.ty).unwrap(), &before.ty);
            match (&before.kind, &after.kind) {
                (
                    ir::InstructionKind::Constant(value),
                    ir_v2::ScalarInstructionKind::Constant(borrowed),
                ) => {
                    assert!(std::ptr::eq(value, *borrowed));
                }
                (_, ir_v2::ScalarInstructionKind::Operation(operation)) => assert_eq!(
                    serde_json::to_value(&before.kind).unwrap(),
                    serde_json::to_value(operation).unwrap()
                ),
                _ => panic!("Lift changed instruction kind"),
            }
        }
    }
    let large = compile(&format!(
        "fn choose(b:Bool)->String=if b {{\"ok\"}} else {{\"{}\"}};",
        "x".repeat(65_536)
    ));
    let lifted = ir_v2::lift_scalar(&large).unwrap();
    let original_constant = large
        .program()
        .blocks
        .iter()
        .flat_map(|b| &b.instructions)
        .find_map(|i| match &i.kind {
            ir::InstructionKind::Constant(value)
                if value.payload.as_str().is_some_and(|s| s.len() == 65_536) =>
            {
                Some(value)
            }
            _ => None,
        })
        .unwrap();
    let borrowed = lifted.program().functions[0]
        .blocks
        .iter()
        .flat_map(|b| &b.instructions)
        .find_map(|i| match &i.kind {
            ir_v2::ScalarInstructionKind::Constant(value)
                if value.payload.as_str().is_some_and(|s| s.len() == 65_536) =>
            {
                Some(*value)
            }
            _ => None,
        })
        .unwrap();
    assert!(
        std::ptr::eq(original_constant, borrowed),
        "An untaken large constant must retain its original allocation"
    );
}

#[test]
fn lifted_enum_validation_retains_exact_private_membership_and_lazy_runtime_limits() {
    let model = intersect_model_acceptance::migration_fixture::migrate_source_fixture(DefinitionSource::new(
        "memory:lift-enums.ecore",
        SourceKind::Ecore,
        r##"<e:EPackage xmlns:e="http://www.eclipse.org/emf/2002/Ecore" xmlns:x="http://www.w3.org/2001/XMLSchema-instance" name="m" nsURI="urn:lift-enums"><eClassifiers x:type="e:EEnum" name="Mode"><eLiterals name="First" value="7"/><eLiterals name="Alias" value="7"/></eClassifiers></e:EPackage>"##,
    )).0;
    let mut source = DefinitionSource::new(
        "memory:lift-enums-behavior.rcore",
        SourceKind::RcoreBehavior,
        "rcore 1; package lift_enums version \"1\"; import model \"urn:lift-enums\" as m; fn choose(x:m.Mode)->m.Mode=match x {m.Mode.First=>m.Mode.Alias,m.Mode.Alias=>m.Mode.First};",
    );
    source.dependencies.push(model.id.clone());
    let symbol = SymbolId {
        source: source.id.clone(),
        declaration: 1,
    };
    let bundle = DefinitionBundle {
        version: 1,
        sources: vec![source, model.clone()],
        namespaces: vec![],
    };
    let original = compile_scalar(
        &bundle,
        &symbol.source,
        &symbol,
        &ir::VerificationLimits::default(),
    )
    .unwrap();
    let lifted = ir_v2::lift_scalar(&original).unwrap();
    let enumeration = DeclarationId {
        source: model.id.clone(),
        path: vec![0],
    };
    let ty = ir::ValueType::Enum(enumeration.clone());
    let make = |index| ir::Value {
        ty: ty.clone(),
        payload: serde_json::to_value(EnumValue::Enum {
            enumeration: enumeration.clone(),
            literal: DeclarationId {
                source: model.id.clone(),
                path: vec![0, index],
            },
        })
        .unwrap(),
    };
    for value in [make(0), make(1)] {
        lifted
            .validate_value_checked(&value, &ir::ValueLimits::default())
            .unwrap();
    }
    assert_ne!(make(0), make(1));
    assert!(
        lifted
            .validate_value(&make(2), &ir::ValueLimits::default())
            .is_err()
    );
    let mut unknown = make(0);
    unknown.payload["literal"]["extra"] = serde_json::json!(true);
    assert!(
        lifted
            .validate_value(&unknown, &ir::ValueLimits::default())
            .is_err()
    );
    let mut stale = make(0);
    stale.payload["literal"]["source"]["sha256"] = serde_json::json!("0".repeat(64));
    assert!(
        lifted
            .validate_value(&stale, &ir::ValueLimits::default())
            .is_err()
    );
    let tiny = ir::ValueLimits {
        max_value_bytes: 1,
        max_digits: 1,
    };
    assert_eq!(
        lifted
            .validate_value_checked(&make(0), &tiny)
            .unwrap_err()
            .kind,
        ir::IrErrorKind::ResourceExhausted
    );
    let mut enum_constants = 0;
    for (original_block, lifted_block) in original
        .program()
        .blocks
        .iter()
        .zip(&lifted.program().functions[0].blocks)
    {
        for (original_instruction, lifted_instruction) in original_block
            .instructions
            .iter()
            .zip(&lifted_block.instructions)
        {
            if let ir::InstructionKind::Constant(value) = &original_instruction.kind {
                let ir_v2::ScalarInstructionKind::Constant(borrowed) = &lifted_instruction.kind
                else {
                    panic!("Lost borrowed constant")
                };
                assert!(std::ptr::eq(value, *borrowed));
                if matches!(value.ty, ir::ValueType::Enum(_)) {
                    enum_constants += 1;
                }
            }
        }
    }
    assert!(enum_constants > 0);
    // Lifting itself has no execution-limit input: stricter limits apply on the
    // visited runtime path through the borrowed original capability.
    assert_eq!(
        ir_v2::lift_scalar(&original).unwrap().program(),
        lifted.program()
    );
}
