use intersect_model::definitions::{
    bundle::{DefinitionBundle, DefinitionSource, SourceKind},
    rcore_behavior::{
        function_lowering::compile_function, ir_v2::*, modules::SymbolId, types::Scalar,
    },
};

fn fixture(body: &str) -> (DefinitionBundle, Program) {
    let source = DefinitionSource::new(
        "memory:adversarial-behavior.rcore",
        SourceKind::RcoreBehavior,
        format!("rcore 1; package adversarial version \"1\"; {body}"),
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
    .unwrap_or_else(|e| panic!("{body}: {e:?}"));
    (bundle, image.program().clone())
}
fn rejects(bundle: &DefinitionBundle, program: Program, diagnostic: &str) {
    let error = verify(program, bundle, &Limits::default()).unwrap_err();
    assert!(
        error.message.contains(diagnostic),
        "Expected {diagnostic:?}, got {error:?}"
    );
}
fn scalar(program: &Program, value: Scalar) -> TypeId {
    TypeId(
        program
            .types
            .iter()
            .position(|t| *t == TypeNode::Scalar(value))
            .unwrap() as u32,
    )
}
fn instruction(
    program: &mut Program,
    predicate: impl Fn(&InstructionKind) -> bool,
) -> &mut Instruction {
    program
        .functions
        .iter_mut()
        .flat_map(|f| &mut f.blocks)
        .flat_map(|b| &mut b.instructions)
        .find(|i| predicate(&i.kind))
        .unwrap()
}

fn intern(program: &mut Program, ty: TypeNode) -> TypeId {
    if let Some(index) = program.types.iter().position(|node| node == &ty) {
        TypeId(index as u32)
    } else {
        let id = TypeId(program.types.len() as u32);
        program.types.push(ty);
        id
    }
}
fn collection_fixture(output: &str, expression: &str) -> (DefinitionBundle, Program) {
    fixture(&format!(
        "fn main(b:Bool,xs:Sequence<Int>,ys:Sequence<Int>,bag:Bag<Int>,flags:Sequence<Bool>,n:Int,nested:Sequence<Sequence<Int>>)->{output} ={expression};"
    ))
}

#[test]
fn algebra_operations_reject_operand_arity_domain_and_result_forgery() {
    for name in ["union", "intersection", "difference", "concat", "distinct"] {
        let distinct = name == "distinct";
        let output = if distinct {
            "OrderedSet<Int>"
        } else {
            "Sequence<Int>"
        };
        let args = if distinct { "xs" } else { "xs,ys" };
        let (bundle, baseline) = collection_fixture(output, &format!("{name}({args})"));
        for mutation in 0..6 {
            let mut bad = baseline.clone();
            let bool_ty = scalar(&bad, Scalar::Bool);
            let int_ty = scalar(&bad, Scalar::Int64);
            let alternate = intern(
                &mut bad,
                TypeNode::Collection {
                    kind: intersect_model::definitions::rcore_behavior::types::CollectionKind::Bag,
                    element: int_ty,
                },
            );
            let distinct_bool = intern(
                &mut bad,
                TypeNode::Collection {
                    kind: intersect_model::definitions::rcore_behavior::types::CollectionKind::OrderedSet,
                    element: bool_ty,
                },
            );
            let call = instruction(&mut bad, |k| {
                matches!(k, InstructionKind::CollectionAlgebra { .. })
            });
            let InstructionKind::CollectionAlgebra { value, other, .. } = &mut call.kind else {
                unreachable!()
            };
            let message = match mutation {
                0 => {
                    *value = Operand::Parameter(0);
                    "Algebra requires a collection"
                }
                1 => {
                    *other = if distinct {
                        Some(Operand::Parameter(2))
                    } else {
                        None
                    };
                    if distinct {
                        "Invalid distinct operand/result contract"
                    } else {
                        "Binary algebra requires second collection"
                    }
                }
                2 => {
                    call.ty = bool_ty;
                    "Algebra must produce a concrete collection"
                }
                3 => {
                    call.ty = alternate;
                    "Algebra result kind mismatch"
                }
                4 if !distinct => {
                    *other = Some(Operand::Parameter(0));
                    "Algebra requires second collection"
                }
                5 if !distinct => {
                    *other = Some(Operand::Parameter(4));
                    "Incompatible function value types"
                }
                5 => {
                    call.ty = distinct_bool;
                    "Invalid distinct operand/result contract"
                }
                _ => {
                    *value = Operand::Register(u32::MAX);
                    "Unknown or not-yet-defined function operand"
                }
            };
            rejects(&bundle, bad, message);
        }
        if !distinct {
            let mut bad = baseline.clone();
            let call = instruction(&mut bad, |k| {
                matches!(k, InstructionKind::CollectionAlgebra { .. })
            });
            let InstructionKind::CollectionAlgebra { other, .. } = &mut call.kind else {
                unreachable!()
            };
            *other = Some(Operand::Parameter(3));
            rejects(
                &bundle,
                bad,
                if name == "concat" {
                    "Concat requires ordered inputs"
                } else {
                    "Algebra collection kind mismatch"
                },
            );
        }
        assert!(verify(baseline, &bundle, &Limits::default()).is_ok());
    }
}

#[test]
fn membership_operations_reject_wrong_domains_operands_and_results() {
    for name in [
        "contains",
        "excludes",
        "count",
        "includesAll",
        "includesAny",
    ] {
        let multiple = name.starts_with("includes");
        let output = if name == "count" { "Int" } else { "Bool" };
        let other = if multiple { "ys" } else { "n" };
        let (bundle, baseline) = collection_fixture(output, &format!("{name}(xs,{other})"));
        for mutation in 0..6 {
            let mut bad = baseline.clone();
            let bool_ty = scalar(&bad, Scalar::Bool);
            let int_ty = scalar(&bad, Scalar::Int64);
            let call = instruction(&mut bad, |k| {
                matches!(k, InstructionKind::CollectionMembership { .. })
            });
            let InstructionKind::CollectionMembership {
                value,
                other,
                element,
                ..
            } = &mut call.kind
            else {
                unreachable!()
            };
            let message = match mutation {
                0 => {
                    *value = Operand::Parameter(0);
                    "Membership requires a collection"
                }
                1 => {
                    *other = Operand::Parameter(0);
                    if multiple {
                        "Membership requires a collection right operand"
                    } else {
                        "Incompatible function value types"
                    }
                }
                2 => {
                    *element = bool_ty;
                    "Incompatible function value types"
                }
                3 => {
                    call.ty = if name == "count" { bool_ty } else { int_ty };
                    "Membership result type mismatch"
                }
                4 => {
                    *value = Operand::Register(u32::MAX);
                    "Unknown or not-yet-defined function operand"
                }
                _ => {
                    *other = Operand::Parameter(4);
                    "Incompatible function value types"
                }
            };
            rejects(&bundle, bad, message);
        }
        assert!(verify(baseline, &bundle, &Limits::default()).is_ok());
    }
}

#[test]
fn conversion_cannot_forge_order_element_types_or_multiplicity() {
    let (bundle, baseline) = collection_fixture("Set<Int>", "Set(xs)");
    for mutation in 0..6 {
        let mut bad = baseline.clone();
        let bool_ty = scalar(&bad, Scalar::Bool);
        let int_ty = scalar(&bad, Scalar::Int64);
        let target = match mutation {
            2 => TypeNode::Collection {
                kind: intersect_model::definitions::rcore_behavior::types::CollectionKind::Sequence,
                element: int_ty,
            },
            3 => TypeNode::Collection {
                kind: intersect_model::definitions::rcore_behavior::types::CollectionKind::Set,
                element: bool_ty,
            },
            _ => TypeNode::Many {
                element: int_ty,
                lower: if mutation == 4 { 1 } else { 0 },
                upper: if mutation == 5 { 2 } else { -1 },
                ordered: false,
                unique: true,
            },
        };
        let ty = intern(&mut bad, target);
        let call = instruction(&mut bad, |k| {
            matches!(k, InstructionKind::ConvertCollection { .. })
        });
        let InstructionKind::ConvertCollection { value } = &mut call.kind else {
            unreachable!()
        };
        let message = match mutation {
            0 => {
                *value = Operand::Parameter(0);
                "Collection conversion source mismatch"
            }
            1 => {
                call.ty = bool_ty;
                "Collection conversion target mismatch"
            }
            2 => {
                *value = Operand::Parameter(3);
                call.ty = ty;
                "Unordered collection cannot acquire ordering"
            }
            3 => {
                call.ty = ty;
                "Incompatible function value types"
            }
            _ => {
                call.ty = ty;
                "Conversion cannot establish stronger multiplicity bounds"
            }
        };
        rejects(&bundle, bad, message);
    }
    assert!(verify(baseline, &bundle, &Limits::default()).is_ok());
}

#[test]
fn flatten_index_and_measures_reject_invalid_shapes_and_exact_result_types() {
    for (output, expression) in [
        ("Sequence<Int>", "flatten(nested)"),
        ("Option<Int>", "first(xs)"),
        ("Option<Int>", "last(xs)"),
        ("Option<Int>", "at(xs,n)"),
        ("Int", "size(xs)"),
        ("Bool", "isEmpty(xs)"),
    ] {
        let (bundle, baseline) = collection_fixture(output, expression);
        let locate = |k: &InstructionKind| {
            matches!(
                k,
                InstructionKind::CollectionFlatten { .. }
                    | InstructionKind::Index { .. }
                    | InstructionKind::CollectionSize { .. }
                    | InstructionKind::CollectionIsEmpty { .. }
            )
        };
        for mutation in 0..2 {
            let mut bad = baseline.clone();
            let wrong = TypeId(bad.types.len() as u32);
            bad.types.push(TypeNode::Scalar(Scalar::String));
            let call = instruction(&mut bad, locate);
            let message = match &mut call.kind {
                InstructionKind::CollectionFlatten { value } => {
                    if mutation == 0 {
                        *value = Operand::Parameter(1);
                        "Flatten requires collection elements"
                    } else {
                        call.ty = wrong;
                        "Flatten result contract mismatch"
                    }
                }
                InstructionKind::Index { value, .. } => {
                    if mutation == 0 {
                        *value = Operand::Parameter(3);
                        "Invalid collection indexing operands"
                    } else {
                        call.ty = wrong;
                        "Collection index result must be exact Option element"
                    }
                }
                InstructionKind::CollectionSize { value }
                | InstructionKind::CollectionIsEmpty { value } => {
                    if mutation == 0 {
                        *value = Operand::Parameter(0);
                        "Collection measure requires a collection operand"
                    } else {
                        call.ty = wrong;
                        "Collection measure has an invalid result type"
                    }
                }
                _ => unreachable!(),
            };
            rejects(&bundle, bad, message);
        }
        if output == "Option<Int>" {
            let mut bad = baseline.clone();
            let call = instruction(&mut bad, |k| matches!(k, InstructionKind::Index { .. }));
            let InstructionKind::Index { index, .. } = &mut call.kind else {
                unreachable!()
            };
            *index = Operand::Parameter(0);
            rejects(&bundle, bad, "Invalid collection indexing operands");
        }
        assert!(verify(baseline, &bundle, &Limits::default()).is_ok());
    }
}

#[test]
fn collection_wire_variants_reject_unknown_operations_fields_and_operand_shapes() {
    for (output, expression, kind) in [
        ("Sequence<Int>", "union(xs,ys)", "CollectionAlgebra"),
        ("Bool", "contains(xs,n)", "CollectionMembership"),
        ("Set<Int>", "Set(xs)", "ConvertCollection"),
        ("Sequence<Int>", "flatten(nested)", "CollectionFlatten"),
        ("Option<Int>", "at(xs,n)", "Index"),
        ("Int", "size(xs)", "CollectionSize"),
        ("Bool", "isEmpty(xs)", "CollectionIsEmpty"),
    ] {
        let (_, program) = collection_fixture(output, expression);
        for mutation in 0..2 {
            let mut wire = serde_json::to_value(&program).unwrap();
            let call = wire["functions"]
                .as_array_mut()
                .unwrap()
                .iter_mut()
                .flat_map(|f| f["blocks"].as_array_mut().unwrap())
                .flat_map(|b| b["instructions"].as_array_mut().unwrap())
                .find(|i| i["kind"].get(kind).is_some())
                .unwrap();
            if mutation == 0 {
                call["kind"][kind]["unknown_authority"] = serde_json::json!(true);
            } else {
                call["kind"][kind]["value"] = serde_json::json!({"Register":-1});
            }
            let error = serde_json::from_value::<Program>(wire)
                .unwrap_err()
                .to_string();
            assert!(
                error.contains(if mutation == 0 {
                    "unknown field"
                } else {
                    "invalid value"
                }),
                "{kind}: {error}"
            );
        }
        if matches!(kind, "CollectionAlgebra" | "CollectionMembership") {
            let mut wire = serde_json::to_value(&program).unwrap();
            let call = wire["functions"]
                .as_array_mut()
                .unwrap()
                .iter_mut()
                .flat_map(|f| f["blocks"].as_array_mut().unwrap())
                .flat_map(|b| b["instructions"].as_array_mut().unwrap())
                .find(|i| i["kind"].get(kind).is_some())
                .unwrap();
            call["kind"][kind]["operation"] = serde_json::json!("UnknownOperation");
            assert!(
                serde_json::from_value::<Program>(wire)
                    .unwrap_err()
                    .to_string()
                    .contains("unknown variant")
            );
        }
    }
}
