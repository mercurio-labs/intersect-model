use intersect_model::definitions::{
    bundle::{DefinitionBundle, DefinitionSource, SourceKind},
    rcore_behavior::{
        function_lowering::compile_function,
        ir,
        ir_v2::*,
        modules::SymbolId,
        types::{CollectionKind, Scalar},
    },
};
fn fixture(body: &str, declaration: usize) -> (DefinitionBundle, VerifiedExecutable) {
    let source = DefinitionSource::new(
        "memory:v2",
        SourceKind::RcoreBehavior,
        format!("rcore 1; package v2 version \"1\"; {body}"),
    );
    let bundle = DefinitionBundle {
        version: 1,
        sources: vec![source.clone()],
        namespaces: vec![],
    };
    let selection = EntrySelection {
        symbol: SymbolId {
            source: source.id.clone(),
            declaration,
        },
        types: vec![],
        arguments: vec![],
    };
    let image = compile_function(&bundle, &source.id, &selection, &Limits::default())
        .unwrap_or_else(|e| panic!("{body}: {e:?}"));
    (bundle, image)
}
#[test]
fn all_image_bodies_and_capture_signatures_are_checked() {
    let (bundle, image) = fixture(
        "fn helper(x:Int32)->Int32=x+1; fn main(x:Int32)->Int32={let unused=helper;x};",
        1,
    );
    let mut bad = image.program().clone();
    let helper = bad
        .functions
        .iter_mut()
        .find(|f| matches!(&f.identity,FunctionIdentity::Named{symbol,..} if symbol.declaration==0))
        .unwrap();
    helper.blocks[0].terminator = Terminator::Return(Operand::Parameter(99));
    assert!(verify(bad, &bundle, &Limits::default()).is_err());
    let wire = serde_json::to_value(image.program()).unwrap();
    let mut bad = wire.clone();
    bad["functions"][0]["identity"]["Named"]["symbol"]["trusted"] = serde_json::json!(true);
    assert!(serde_json::from_value::<Program>(bad).is_err());
    let declaration = intersect_model::definitions::resources::DeclarationId {
        source: bundle.sources[0].id.clone(),
        path: vec![0],
    };
    let mut node = serde_json::to_value(TypeNode::Enum(declaration.clone())).unwrap();
    node["Enum"]["ignored"] = serde_json::json!(true);
    assert!(serde_json::from_value::<TypeNode>(node).is_err());
    let expression = intersect_model::definitions::generic_types::Type::Classifier {
        declaration: declaration.clone(),
        arguments: vec![],
        raw: false,
    };
    let model =
        intersect_model::definitions::generic_types::RuntimeType::new(expression.clone()).unwrap();
    let mut node = serde_json::to_value(TypeNode::Model(model)).unwrap();
    node["Model"]["declaration"]["ignored"] = serde_json::json!(true);
    assert!(serde_json::from_value::<TypeNode>(node).is_err());
    let application =
        intersect_model::definitions::generic_types::ClosedType::new(expression).unwrap();
    let mut node = serde_json::to_value(TypeNode::AppliedDatatype(application)).unwrap();
    node["AppliedDatatype"]["declaration"]["ignored"] = serde_json::json!(true);
    assert!(serde_json::from_value::<TypeNode>(node).is_err());
    let enum_value = intersect_model::definitions::enums::EnumValue::Enum {
        enumeration: declaration.clone(),
        literal: declaration,
    };
    let mut node = serde_json::to_value(DataWire::Enum(enum_value)).unwrap();
    node["Enum"]["literal"]["ignored"] = serde_json::json!(true);
    assert!(serde_json::from_value::<DataWire>(node).is_err());
    let mut bad = wire;
    bad["functions"][0]["blocks"][0]["instructions"][0]["kind"] =
        serde_json::json!({"HostCall":"clock"});
    assert!(serde_json::from_value::<Program>(bad).is_err());
    let (bundle, image) = fixture("fn main(x:Int32)->Int32={let f=|y:Int32|x+y;f(1)};", 0);
    let mut bad = image.program().clone();
    let lambda = bad
        .functions
        .iter_mut()
        .find(|f| matches!(f.identity, FunctionIdentity::Lambda { .. }))
        .unwrap();
    assert_eq!(lambda.captures.len(), 1);
    lambda.captures[0].declaration.start += 1;
    assert!(
        verify(bad, &bundle, &Limits::default())
            .unwrap_err()
            .message
            .contains("captures")
    );
    verify(image.program().clone(), &bundle, &Limits::default()).unwrap();
}
#[test]
fn verified_context_includes_dependency_and_kind_metadata() {
    let (bundle, image) = fixture("fn main(x:Int32)->Int32=x;", 0);
    image.check_context(&bundle, None).unwrap();
    let mut changed = bundle.clone();
    changed.sources[0].kind = SourceKind::Ecore;
    assert!(image.check_context(&changed, None).is_err());
    let mut changed = bundle.clone();
    let id = changed.sources[0].id.clone();
    changed.sources[0].dependencies.push(id);
    assert!(image.check_context(&changed, None).is_err());
    let mut forged = image.program().clone();
    forged.entry_source.sha256 = "0".repeat(64);
    assert!(verify(forged, &bundle, &Limits::default()).is_err());
    image.check_context(&bundle, None).unwrap();
}
#[test]
fn graph_metadata_and_full_pair_relations_have_finite_collective_limits() {
    let (bundle, image) = fixture("fn main(x:Int32)->Int32=x;", 0);
    let selection = EntrySelection {
        symbol: SymbolId {
            source: bundle.sources[0].id.clone(),
            declaration: 0,
        },
        types: vec![TypeNode::Scalar(Scalar::Int8); 4097],
        arguments: vec![],
    };
    assert_eq!(
        preflight_selection(&selection, &Limits::default())
            .unwrap_err()
            .kind,
        IrErrorKind::ResourceExhausted
    );
    let mut selection = EntrySelection {
        symbol: SymbolId {
            source: bundle.sources[0].id.clone(),
            declaration: 0,
        },
        types: vec![],
        arguments: vec![],
    };
    selection.symbol.source.uri = "u".repeat(2048);
    let limits = Limits {
        scalar: ir::VerificationLimits {
            max_source_bytes: 1024,
            ..ir::VerificationLimits::default()
        },
        ..Limits::default()
    };
    assert_eq!(
        preflight_selection(&selection, &limits).unwrap_err().kind,
        IrErrorKind::ResourceExhausted
    );
    let mut program = image.program().clone();
    for scalar in [
        Scalar::Bool,
        Scalar::String,
        Scalar::Bytes,
        Scalar::Char16,
        Scalar::DateInstant,
        Scalar::Unit,
        Scalar::Int8,
        Scalar::Int16,
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
        if !program.types.contains(&TypeNode::Scalar(scalar)) {
            program.types.push(TypeNode::Scalar(scalar));
        }
    }
    verify(program.clone(), &bundle, &Limits::default()).unwrap();
    let limits = Limits {
        scalar: ir::VerificationLimits {
            max_work: 200,
            ..ir::VerificationLimits::default()
        },
        ..Limits::default()
    };
    assert_eq!(
        verify(program.clone(), &bundle, &limits).unwrap_err().kind,
        IrErrorKind::ResourceExhausted
    );
    verify(program, &bundle, &Limits::default()).unwrap();
}
struct NoSemantics;
impl ValueVisitor for NoSemantics {
    fn nominal_leaf(&mut self, _: &TypeNode, _: &serde_json::Value) -> Result<(), IrError> {
        Ok(())
    }
    fn unique_collection(&mut self, _: TypeId, _: &[ValueWire]) -> Result<(), IrError> {
        Ok(())
    }
}
#[test]
fn structural_shape_rejects_wide_inputs_before_pending_allocation() {
    let (_, image) = fixture("fn main(x:Sequence<Int32>)->Sequence<Int32> = x;", 0);
    let types = &image.program().types;
    let scalar = TypeId(
        types
            .iter()
            .position(|t| t == &TypeNode::Scalar(Scalar::Int32))
            .unwrap() as u32,
    );
    let collection = TypeId(
        types
            .iter()
            .position(|t| {
                t == &TypeNode::Collection {
                    kind: CollectionKind::Sequence,
                    element: scalar,
                }
            })
            .unwrap() as u32,
    );
    let value = ValueWire {
        ty: collection,
        data: DataWire::Collection(vec![
            ValueWire {
                ty: scalar,
                data: DataWire::Scalar(ir::Value {
                    ty: ir::ValueType::Scalar(Scalar::Int32),
                    payload: serde_json::json!(1)
                })
            };
            100
        ]),
    };
    let mut budget = ValueBudget {
        remaining_nodes: 2,
        remaining_bytes: 100000,
        max_depth: 64,
        scalar: ir::ValueLimits::default(),
    };
    assert_eq!(
        inspect_value_shape(&image, &value, collection, &mut budget, &mut NoSemantics)
            .unwrap_err()
            .kind,
        IrErrorKind::ResourceExhausted
    );
    let mut budget = ValueBudget {
        remaining_nodes: 1000,
        remaining_bytes: 100000,
        max_depth: 64,
        scalar: ir::ValueLimits::default(),
    };
    inspect_value_shape(&image, &value, collection, &mut budget, &mut NoSemantics).unwrap();
}

#[test]
fn constant_limits_are_shared_across_functions_and_absurd_needs_empty_proof() {
    let (bundle, image) = fixture(
        "fn helper()->Int32=1; fn main()->Int32={let unused=helper;2};",
        1,
    );
    let low = Limits {
        scalar: ir::VerificationLimits {
            max_total_value_bytes: 79,
            ..ir::VerificationLimits::default()
        },
        ..Limits::default()
    };
    assert_eq!(
        verify(image.program().clone(), &bundle, &low)
            .unwrap_err()
            .kind,
        IrErrorKind::ResourceExhausted
    );
    let exact = Limits {
        scalar: ir::VerificationLimits {
            max_total_value_bytes: 80,
            ..ir::VerificationLimits::default()
        },
        ..Limits::default()
    };
    verify(image.program().clone(), &bundle, &exact).unwrap();
    let (bundle, image) = fixture("fn main(x:Int32)->Int32=x;", 0);
    let mut bad = image.program().clone();
    bad.functions[0].blocks[0].terminator = Terminator::Absurd {
        value: Operand::Parameter(0),
    };
    assert!(
        verify(bad, &bundle, &Limits::default())
            .unwrap_err()
            .message
            .contains("uninhabited")
    );
}

#[test]
fn shape_cursor_suspends_wide_values_and_preserves_the_synchronous_contract() {
    let (_, image) = fixture("fn main(x:Sequence<Int32>)->Sequence<Int32> =x;", 0);
    let ty = image.program().functions[image.program().entry.0 as usize].parameters[0];
    let TypeNode::Collection { element, .. } = image.type_node(ty).unwrap() else {
        panic!("Collection");
    };
    let value = ValueWire {
        ty,
        data: DataWire::Collection(
            (0..100)
                .map(|i| ValueWire {
                    ty: *element,
                    data: DataWire::Scalar(ir::Value {
                        ty: ir::ValueType::Scalar(Scalar::Int32),
                        payload: serde_json::json!(i),
                    }),
                })
                .collect(),
        ),
    };
    let budget = || ValueBudget {
        remaining_nodes: 1000,
        remaining_bytes: 100000,
        max_depth: 64,
        scalar: ir::ValueLimits::default(),
    };
    let mut synchronous = budget();
    inspect_value_shape(&image, &value, ty, &mut synchronous, &mut NoSemantics).unwrap();
    let mut stepped = budget();
    let mut cursor = ValueShapeCursor::new(ty, &mut stepped, 100000).unwrap();
    let mut ticks = 0;
    loop {
        let before = stepped.remaining_nodes;
        let done = cursor
            .advance(&image, &value, &mut stepped, &mut NoSemantics)
            .unwrap();
        assert!(before - stepped.remaining_nodes <= 2);
        ticks += 1;
        if done {
            break;
        }
        assert!(ticks < 1000);
    }
    assert!(ticks > 100);
    assert_eq!(synchronous.remaining_nodes, stepped.remaining_nodes);
    assert_eq!(synchronous.remaining_bytes, stepped.remaining_bytes);
    let retained = cursor.storage_bytes();
    for limit in [0, retained - 1, retained, retained + 1] {
        let mut budget = budget();
        let result = ValueShapeCursor::new(ty, &mut budget, limit).and_then(|mut cursor| {
            while !cursor.advance(&image, &value, &mut budget, &mut NoSemantics)? {}
            Ok(())
        });
        assert_eq!(result.is_ok(), limit >= retained);
        if let Err(error) = result {
            assert_eq!(error.kind, IrErrorKind::ResourceExhausted);
        }
    }
    let mut malformed = value.clone();
    let DataWire::Collection(values) = &mut malformed.data else {
        panic!("Collection");
    };
    values[99].data = DataWire::None;
    let mut expected_budget = budget();
    let expected = inspect_value_shape(
        &image,
        &malformed,
        ty,
        &mut expected_budget,
        &mut NoSemantics,
    )
    .unwrap_err();
    let mut budget = budget();
    let mut cursor = ValueShapeCursor::new(ty, &mut budget, 100000).unwrap();
    let error = loop {
        match cursor.advance(&image, &malformed, &mut budget, &mut NoSemantics) {
            Ok(false) => {}
            Ok(true) => panic!("Accepted malformed value"),
            Err(error) => break error,
        }
    };
    assert_eq!(error, expected);
    assert!(
        cursor
            .advance(&image, &value, &mut budget, &mut NoSemantics)
            .is_err()
    );
}
