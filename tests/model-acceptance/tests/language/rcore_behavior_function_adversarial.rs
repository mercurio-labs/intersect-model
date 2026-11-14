//! Adversarial checked-image consumers: modified images test IR/runtime contracts,
//! not correspondence of a modified body to its retained source text.
use intersect_model::core::model::Model;
use intersect_model::definitions::{
    bundle::{DefinitionBundle, DefinitionSource, SourceKind},
    compiler::{import_datatype_bundle, materialize_runtime_bundle},
    conversion::{
        ConversionContract, ConversionRegistry, MemberContract, MemberType, ProviderId,
        ReferenceContext,
    },
    generic_types::{RuntimeType, Type},
    rcore_behavior::{
        self,
        function_lowering::compile_function,
        ir,
        ir_v2::{
            self, Block, DataWire, EntrySelection, Instruction, InstructionKind, Limits, Operand,
            Terminator, TypeId, TypeNode, ValueWire, VerifiedExecutable,
        },
        modules::SymbolId,
        types::Scalar,
    },
    resources::DeclarationId,
};

use intersect_model::{
    model_reference::ReferenceDomain,
    rcore_behavior_functions::{ExecutionContext, ExecutionErrorKind, ExecutionLimits, evaluate},
};

fn fixture(body: &str, model: Option<&DefinitionSource>) -> (DefinitionBundle, DefinitionSource) {
    let mut source = DefinitionSource::new(
        "memory:adversarial-behavior.rcore",
        SourceKind::RcoreBehavior,
        format!("rcore 1;package adversarial version \"1\";{body}"),
    );
    let mut sources = vec![];
    if let Some(model) = model {
        source.dependencies.push(model.id.clone());
        sources.push(model.clone());
    }
    sources.push(source.clone());
    (
        DefinitionBundle {
            version: 1,
            sources,
            namespaces: vec![],
        },
        source,
    )
}
fn compile(bundle: &DefinitionBundle, source: &DefinitionSource) -> VerifiedExecutable {
    let document = rcore_behavior::parse(source.clone());
    let declaration = document
        .declarations
        .iter()
        .position(|d| matches!(d.value, rcore_behavior::ast::Declaration::Function { .. }))
        .unwrap();
    compile_function(
        bundle,
        &source.id,
        &EntrySelection {
            symbol: SymbolId {
                source: source.id.clone(),
                declaration,
            },
            types: vec![],
            arguments: vec![],
        },
        &Limits::default(),
    )
    .unwrap()
}
fn scalar(ty: TypeId, value: i8) -> ValueWire {
    ValueWire {
        ty,
        data: DataWire::Scalar(ir::Value {
            ty: ir::ValueType::Scalar(Scalar::Int8),
            payload: serde_json::json!(value),
        }),
    }
}
fn unreachable_constant(
    image: &VerifiedExecutable,
    bundle: &DefinitionBundle,
    function: usize,
    value: ValueWire,
) -> VerifiedExecutable {
    let mut program = image.program().clone();
    let boolean = if let Some(index) = program
        .types
        .iter()
        .position(|t| *t == TypeNode::Scalar(Scalar::Bool))
    {
        TypeId(index as u32)
    } else {
        let ty = TypeId(program.types.len() as u32);
        program.types.push(TypeNode::Scalar(Scalar::Bool));
        ty
    };
    let f = &mut program.functions[function];
    assert_eq!(
        f.blocks.len(),
        1,
        "fixture helper expects a straight-line function"
    );
    let Terminator::Return(original) = f.blocks[0].terminator.clone() else {
        panic!("return")
    };
    let condition = Operand::Register(f.blocks[0].instructions.len() as u32);
    f.blocks[0].instructions.push(Instruction {
        ty: boolean,
        kind: InstructionKind::Constant(ValueWire {
            ty: boolean,
            data: DataWire::Scalar(ir::Value {
                ty: ir::ValueType::Scalar(Scalar::Bool),
                payload: serde_json::json!(true),
            }),
        }),
        span: f.span.clone(),
    });
    // Both edges are structurally reachable. The bad constant sits in the
    // false arm, which this actual execution never selects.
    f.blocks[0].terminator = Terminator::Branch {
        condition,
        then_edge: ir_v2::Edge {
            block: 1,
            arguments: vec![original],
        },
        else_edge: ir_v2::Edge {
            block: 2,
            arguments: vec![],
        },
    };
    f.blocks.push(Block {
        parameters: vec![f.result],
        instructions: vec![],
        terminator: Terminator::Return(Operand::Parameter(0)),
        span: f.span.clone(),
    });
    f.blocks.push(Block {
        parameters: vec![],
        instructions: vec![Instruction {
            ty: value.ty,
            kind: InstructionKind::Constant(value),
            span: f.span.clone(),
        }],
        terminator: Terminator::Return(Operand::Register(0)),
        span: f.span.clone(),
    });
    ir_v2::verify(program, bundle, &Limits::default()).unwrap()
}
#[test]
fn unreachable_unique_constants_are_prepared_in_entry_and_uncalled_function() {
    let (bundle, source) = fixture(
        "fn run()->Set<Int8> ={let unused=helper;Set::<Int8>([1])};fn helper()->Set<Int8> =Set::<Int8>([2]);",
        None,
    );
    let image = compile(&bundle, &source);
    assert_eq!(image.program().functions.len(), 2);
    let context = ExecutionContext::new(&bundle);
    for function in 0..2 {
        let ty = image.program().functions[function].result;
        let TypeNode::Collection { element, .. } = image.type_node(ty).unwrap() else {
            panic!("set")
        };
        for (second, accepted) in [(1, false), (2, true)] {
            let value = ValueWire {
                ty,
                data: DataWire::Collection(vec![scalar(*element, 1), scalar(*element, second)]),
            };
            let modified = unreachable_constant(&image, &bundle, function, value);
            let result = evaluate(&modified, &[], &context, &ExecutionLimits::default());
            if accepted {
                assert!(result.is_ok(), "{result:?}");
            } else {
                let error = result.unwrap_err();
                assert_eq!(error.kind, ExecutionErrorKind::Arguments);
                assert!(error.message.contains("Duplicate"), "{error:?}");
                assert_eq!(error.consumed.steps, 0);
            }
        }
    }
    assert!(evaluate(&image, &[], &context, &ExecutionLimits::default()).is_ok());
}
fn model_source() -> DefinitionSource {
    intersect_model_acceptance::migration_fixture::migrate_source_fixture(DefinitionSource::new(
        "memory:adversarial.ecore",
        SourceKind::Ecore,
        r##"<e:EPackage xmlns:e="http://www.eclipse.org/emf/2002/Ecore" xmlns:x="http://www.w3.org/2001/XMLSchema-instance" name="objects" nsURI="urn:objects"><eClassifiers x:type="e:EClass" name="Base"/><eClassifiers x:type="e:EClass" name="Child" eSuperTypes="#//Base"/><eClassifiers x:type="e:EDataType" name="Text" instanceClassName="java.lang.String"/><eClassifiers x:type="e:EDataType" name="OtherText" instanceClassName="java.lang.String"/></e:EPackage>"##,
    )).0
}
fn declaration(source: &DefinitionSource, index: usize) -> DeclarationId {
    DeclarationId {
        source: source.id.clone(),
        path: vec![index],
    }
}
fn runtime(source: &DefinitionSource, index: usize) -> RuntimeType {
    RuntimeType::new(Type::Classifier {
        declaration: declaration(source, index),
        arguments: vec![],
        raw: false,
    })
    .unwrap()
}
fn reference(source: &DefinitionSource) -> ReferenceContext {
    let bundle = DefinitionBundle {
        version: 1,
        sources: vec![source.clone()],
        namespaces: vec![],
    };
    let targets = vec![runtime(source, 0), runtime(source, 1)];
    let schema =
        materialize_runtime_bundle(&bundle, &source.id, "urn:objects", &targets, None).unwrap();
    let mut model = Model::default();
    model
        .create(
            &schema,
            "object",
            schema.class_key_for_type(&targets[1]).unwrap(),
            "memory:objects",
            0,
            1,
        )
        .unwrap();
    ReferenceContext {
        bundle,
        entry: source.id.clone(),
        namespace: "urn:objects".into(),
        conversions: vec![],
        materialized_types: targets,
        applied_conversions: vec![],
        constructors: vec![],
        model: serde_json::to_value(model).unwrap(),
    }
}
#[test]
fn unreachable_nominal_constant_cannot_hide_a_foreign_datatype_tag() {
    let model = model_source();
    let (bundle, source) = fixture(
        "import model \"urn:objects\" as m;fn run()->Int8={let unused=helper;42};fn helper(x:m.Text)->m.Text=x;",
        Some(&model),
    );
    let image = compile(&bundle, &source);
    let helper = image
        .program()
        .functions
        .iter()
        .position(|f| matches!(image.type_node(f.result).unwrap(), TypeNode::Datatype(_)))
        .unwrap();
    let ty = image.program().functions[helper].result;
    let context = ExecutionContext::new(&bundle);
    for (index, accepted) in [(2, true), (3, false)] {
        let payload = import_datatype_bundle(&bundle, &declaration(&model, index))
            .unwrap()
            .bind_value(serde_json::json!("same scalar, different nominal identity"))
            .unwrap();
        let modified = unreachable_constant(
            &image,
            &bundle,
            helper,
            ValueWire {
                ty,
                data: DataWire::Datatype(serde_json::to_value(payload).unwrap()),
            },
        );
        let result = evaluate(&modified, &[], &context, &ExecutionLimits::default());
        if accepted {
            assert!(result.is_ok(), "{result:?}");
        } else {
            let error = result.unwrap_err();
            assert_eq!(error.kind, ExecutionErrorKind::Nominal);
            assert_eq!(error.consumed.steps, 0);
        }
    }
    assert!(evaluate(&image, &[], &context, &ExecutionLimits::default()).is_ok());
}
#[test]
fn exact_model_domain_cache_limits_charge_once_per_context_and_target() {
    let model = model_source();
    let reference = reference(&model);
    let registry = ConversionRegistry::default();
    let payload = ReferenceDomain::new_runtime(runtime(&model, 0), reference.clone(), &registry)
        .unwrap()
        .bind("object")
        .unwrap();
    for (second, domains) in [("Base", 1), ("Child", 2)] {
        let (bundle, source) = fixture(
            &format!(
                "import model \"urn:objects\" as m;fn run(a:m.Base,b:m.{second})->m.Base={{let f=|unused:Int8|a;f(0)}};"
            ),
            Some(&model),
        );
        let image = compile(&bundle, &source);
        let args = image.program().functions[image.program().entry.0 as usize]
            .parameters
            .iter()
            .map(|ty| ValueWire {
                ty: *ty,
                data: DataWire::Model(payload.clone()),
            })
            .collect::<Vec<_>>();
        let references = [reference.clone()];
        let context = ExecutionContext {
            bundle: &bundle,
            conversions: Some(&registry),
            references: &references,
        };
        let exact = ExecutionLimits {
            max_model_contexts: 1,
            max_reference_domains: domains,
            max_model_elements: 1,
            max_membership_checks: domains,
            ..Default::default()
        };
        let result = evaluate(&image, &args, &context, &exact).unwrap();
        assert_eq!(result.consumed.model_contexts, 1);
        assert_eq!(result.consumed.reference_domains, domains);
        assert_eq!(result.consumed.model_elements, 1);
        assert_eq!(result.consumed.membership_checks, domains);
        for (limits, message) in [
            (
                ExecutionLimits {
                    max_reference_domains: domains - 1,
                    ..exact.clone()
                },
                "Reference domain count",
            ),
            (
                ExecutionLimits {
                    max_model_elements: 0,
                    ..exact.clone()
                },
                "Model element validation",
            ),
            (
                ExecutionLimits {
                    max_membership_checks: domains - 1,
                    ..exact.clone()
                },
                "Model membership work",
            ),
            (
                ExecutionLimits {
                    max_model_contexts: 0,
                    ..exact.clone()
                },
                "Model context count",
            ),
            (
                ExecutionLimits {
                    max_context_bytes: 1,
                    ..exact.clone()
                },
                "byte",
            ),
        ] {
            let error = evaluate(&image, &args, &context, &limits).unwrap_err();
            assert_eq!(error.kind, ExecutionErrorKind::ResourceExhausted);
            assert!(
                error
                    .message
                    .to_lowercase()
                    .contains(&message.to_lowercase()),
                "{error:?}"
            );
            assert!(evaluate(&image, &args, &context, &exact).is_ok());
        }
        let mut stale = args.clone();
        let DataWire::Model(value) = &mut stale[0].data else {
            panic!("model")
        };
        value["context"] = serde_json::json!("foreign snapshot");
        assert_eq!(
            evaluate(&image, &stale, &context, &exact).unwrap_err().kind,
            ExecutionErrorKind::MissingDependency
        );
        assert!(evaluate(&image, &args, &context, &exact).is_ok());
    }
}
#[test]
fn nested_policy_metadata_is_bounded_before_digest_or_nominal_reconstruction() {
    let model = model_source();
    let (bundle, source) = fixture(
        "import model \"urn:objects\" as m;fn run()->Int8=42;",
        Some(&model),
    );
    let image = compile(&bundle, &source);
    let valid = reference(&model);
    let mut hostile = valid.clone();
    let mut contract = ConversionContract {
        declarations: vec![],
        members: vec![],
        datatype: declaration(&model, 2),
        provider: ProviderId {
            name: "uninstalled".into(),
            version: "1".into(),
        },
        serializable: true,
    };
    for _ in 0..80 {
        contract = ConversionContract {
            members: vec![MemberContract {
                nullable: false,
                value: MemberType::Converted {
                    contract: Box::new(contract),
                },
            }],
            declarations: vec![],
            datatype: declaration(&model, 2),
            provider: ProviderId {
                name: "uninstalled".into(),
                version: "1".into(),
            },
            serializable: true,
        };
    }
    hostile.conversions.push(contract);
    let references = [hostile];
    let context = ExecutionContext {
        bundle: &bundle,
        conversions: None,
        references: &references,
    };
    let error = evaluate(&image, &[], &context, &ExecutionLimits::default()).unwrap_err();
    assert_eq!(error.kind, ExecutionErrorKind::ResourceExhausted);
    assert!(error.message.contains("depth"), "{error:?}");
    assert_eq!(error.consumed.steps, 0);
    assert_eq!(error.consumed.value_nodes, 0);
    let references = [valid];
    let fresh = ExecutionContext {
        references: &references,
        ..context
    };
    assert!(evaluate(&image, &[], &fresh, &ExecutionLimits::default()).is_ok());
}

#[test]
fn shared_context_pool_keeps_identical_object_ids_in_distinct_snapshots_separate() {
    let model = model_source();
    let first = reference(&model);
    let mut second = first.clone();
    // This independently authored snapshot relocates the same object identity.
    // Keep its spans and values while finalizing the matching resource record.
    let mut second_model: Model = serde_json::from_value(second.model.clone()).unwrap();
    let element = &mut second_model.elements[0];
    let mut record = second_model.resources.remove(&element.resource).unwrap();
    element.resource = "memory:second-snapshot".into();
    record.uri = element.resource.clone();
    record.roots = std::sync::Arc::new(vec![element.id.clone()]);
    record.touch().unwrap();
    second_model.resources.insert(record.uri.clone(), record);
    second.model = serde_json::to_value(second_model).unwrap();
    let registry = ConversionRegistry::default();
    let references = [first, second];
    let payloads: Vec<_> = references
        .iter()
        .map(|reference| {
            ReferenceDomain::new_runtime(runtime(&model, 0), reference.clone(), &registry)
                .unwrap()
                .bind("object")
                .unwrap()
        })
        .collect();
    assert_ne!(payloads[0], payloads[1]);
    let (bundle, source) = fixture(
        "import model \"urn:objects\" as m;fn run(a:m.Base,b:m.Base)->Bool=a==b;",
        Some(&model),
    );
    let image = compile(&bundle, &source);
    let arguments: Vec<_> = image.program().functions[image.program().entry.0 as usize]
        .parameters
        .iter()
        .zip(payloads)
        .map(|(ty, payload)| ValueWire {
            ty: *ty,
            data: DataWire::Model(payload),
        })
        .collect();
    let context = ExecutionContext {
        bundle: &bundle,
        conversions: Some(&registry),
        references: &references,
    };
    let result = evaluate(&image, &arguments, &context, &ExecutionLimits::default()).unwrap();
    assert_eq!(result.consumed.model_contexts, 2);
    assert_eq!(result.consumed.reference_domains, 2);
    assert_eq!(result.consumed.model_elements, 2);
    assert_eq!(result.consumed.membership_checks, 2);
    assert_eq!(
        result.consumed.context_bytes,
        references
            .iter()
            .map(|r| serde_json::to_vec(r).unwrap().len())
            .sum::<usize>()
    );
    let DataWire::Scalar(value) = result.value.data else {
        panic!("boolean")
    };
    assert_eq!(value.payload, serde_json::json!(false));
    for limits in [
        ExecutionLimits {
            max_model_elements: 1,
            ..Default::default()
        },
        ExecutionLimits {
            max_reference_domains: 1,
            ..Default::default()
        },
    ] {
        assert_eq!(
            evaluate(&image, &arguments, &context, &limits)
                .unwrap_err()
                .kind,
            ExecutionErrorKind::ResourceExhausted
        );
    }
}
