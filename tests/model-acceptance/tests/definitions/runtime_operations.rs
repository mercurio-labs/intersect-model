use intersect_model::definitions::{
    bundle::{DefinitionBundle, DefinitionSource, SourceKind},
    generic_resolution::Resolver,
    generic_types::{RuntimeType, Type},
    resources::DeclarationId,
};
const FIXTURE: &str = include_str!("../../../../fixtures/ecore-generics/runtime-operations.ecore");
struct FixtureSource {
    declarations: std::collections::BTreeMap<Vec<usize>, DeclarationId>,
}
fn context() -> (DefinitionBundle, FixtureSource, Resolver) {
    let source = DefinitionSource::new("memory:runtime-operations", SourceKind::Ecore, FIXTURE);
    let (source, declarations) =
        intersect_model_acceptance::migration_fixture::migrate_source_fixture(source);
    let bundle = DefinitionBundle {
        version: 1,
        sources: vec![source.clone()],
        namespaces: vec![],
    };
    let resolver = Resolver::new(&bundle).unwrap();
    (bundle, FixtureSource { declarations }, resolver)
}
fn id(source: &FixtureSource, path: Vec<usize>) -> DeclarationId {
    source.declarations[&path].clone()
}
fn ty(source: &FixtureSource, index: usize, arguments: Vec<Type>, raw: bool) -> RuntimeType {
    RuntimeType::new(Type::Classifier {
        declaration: id(source, vec![index]),
        arguments,
        raw,
    })
    .unwrap()
}
#[test]
fn raw_method_actuals_check_all_class_bounds_and_preserve_class_formals() {
    let (_, source, resolver) = context();
    let raw = ty(&source, 4, vec![], true);
    let both = ty(&source, 2, vec![], false);
    let op = id(&source, vec![4, 1]);
    let report = resolver
        .instantiate_runtime_operation_in_owner(&raw, &op, None, std::slice::from_ref(&both))
        .unwrap();
    assert_eq!(report.projection.source.owner, *raw.expression());
    assert!(!report.warnings.is_empty());
    assert!(report.argument_bounds.iter().all(|check| check.satisfied));
    assert!(
        report
            .operation
            .signatures
            .iter()
            .flat_map(|s| &s.specialized)
            .any(|t| *t
                == Type::Parameter {
                    declaration: id(&source, vec![4, 0])
                })
    );
    assert!(
        report
            .operation
            .signatures
            .iter()
            .flat_map(|s| &s.specialized)
            .any(|t| t == both.expression())
    );
    assert!(
        resolver
            .instantiate_runtime_operation_in_owner(
                &raw,
                &op,
                None,
                &[ty(&source, 3, vec![], false)]
            )
            .is_err()
    );
    assert!(
        resolver
            .instantiate_runtime_operation_in_owner(&raw, &op, None, &[])
            .is_err()
    );
    let child = ty(&source, 5, vec![], true);
    let projected = resolver
        .instantiate_runtime_operation_in_owner(&child, &op, None, &[both])
        .unwrap();
    assert_ne!(
        projected.projection.source.owner,
        *projected.projection.storage_owner.expression()
    );
}
#[test]
fn exact_runtime_method_owner_selection_preserves_distinct_paths() {
    let (_, source, resolver) = context();
    let receiver = ty(&source, 8, vec![], false);
    let actual = ty(&source, 2, vec![], false);
    let operation = id(&source, vec![4, 1]);
    assert!(
        resolver
            .instantiate_runtime_operation_in_owner(
                &receiver,
                &operation,
                None,
                std::slice::from_ref(&actual)
            )
            .is_err()
    );
    let owners: Vec<_> = resolver
        .runtime_member_projections(&receiver)
        .unwrap()
        .into_iter()
        .filter(|p| p.source.declaration == operation)
        .collect();
    assert_eq!(owners.len(), 2);
    for owner in owners {
        let checked = resolver
            .instantiate_runtime_operation_in_owner(
                &receiver,
                &operation,
                Some(&owner.source.owner),
                std::slice::from_ref(&actual),
            )
            .unwrap();
        assert_eq!(checked.projection, owner);
    }
    assert!(
        resolver
            .instantiate_runtime_operation_in_owner(
                &receiver,
                &operation,
                Some(actual.expression()),
                std::slice::from_ref(&actual)
            )
            .is_err()
    );
    let diamond = ty(&source, 10, vec![], false);
    assert!(
        resolver
            .instantiate_runtime_operation_in_owner(&diamond, &operation, None, &[actual])
            .is_ok()
    );
}

#[test]
fn materialized_raw_templates_and_exact_owner_applications_replay() {
    let (bundle, source, resolver) = context();
    let actual = ty(&source, 2, vec![], false);
    let child = ty(&source, 5, vec![], true);
    let mixed = ty(&source, 8, vec![], false);
    let operation = id(&source, vec![4, 1]);
    let roots = [
        ty(&source, 0, vec![], false),
        ty(&source, 1, vec![], false),
        actual.clone(),
        child.clone(),
        mixed.clone(),
    ];
    let schema =
        intersect_model::definitions::compiler::materialize_runtime_types(&bundle, &roots, None)
            .unwrap();
    let child_report = schema
        .instantiate_runtime_operation(
            &resolver,
            &child,
            &operation,
            None,
            std::slice::from_ref(&actual),
        )
        .unwrap();
    assert_ne!(
        child_report.projection.source.owner,
        *child_report.projection.storage_owner.expression()
    );
    let kind = schema.class_key_for_type(&mixed).unwrap();
    schema.validate_declared_operation_inventory(kind).unwrap();
    assert!(schema.resolved_operations(kind).is_err());
    assert!(
        schema
            .instantiate_runtime_operation(
                &resolver,
                &mixed,
                &operation,
                None,
                std::slice::from_ref(&actual)
            )
            .is_err()
    );
    for projection in resolver
        .runtime_member_projections(&mixed)
        .unwrap()
        .into_iter()
        .filter(|projection| projection.source.declaration == operation)
    {
        let report = schema
            .instantiate_runtime_operation(
                &resolver,
                &mixed,
                &operation,
                Some(&projection.source.owner),
                std::slice::from_ref(&actual),
            )
            .unwrap();
        assert_eq!(report.projection, projection);
    }
    let read = id(&source, vec![4, 2]);
    let report = schema
        .instantiate_runtime_operation(&resolver, &child, &read, None, &[])
        .unwrap();
    assert!(report.operation.formals.is_empty());
    assert!(
        report
            .operation
            .signatures
            .iter()
            .flat_map(|signature| &signature.specialized)
            .any(|ty| matches!(ty, Type::Parameter { .. }))
    );
    let replay =
        intersect_model::definitions::compiler::materialize_runtime_types(&bundle, &roots, None)
            .unwrap();
    assert_eq!(schema, replay);
}

#[test]
fn raw_actuals_and_dependent_method_formals_remain_distinct() {
    let (_, source, resolver) = context();
    let raw = ty(&source, 4, vec![], true);
    let both = ty(&source, 2, vec![], false);
    let accepted = resolver
        .instantiate_runtime_operation_in_owner(
            &raw,
            &id(&source, vec![4, 3]),
            None,
            std::slice::from_ref(&raw),
        )
        .unwrap();
    assert!(
        accepted
            .warnings
            .iter()
            .filter(|warning| warning.code == "raw_type")
            .count()
            >= 2
    );
    assert_eq!(accepted.arguments[0], raw);
    let dependent = id(&source, vec![4, 4]);
    assert!(
        resolver
            .instantiate_runtime_operation_in_owner(
                &raw,
                &dependent,
                None,
                &[both.clone(), both.clone()]
            )
            .is_ok()
    );
    assert!(
        resolver
            .instantiate_runtime_operation_in_owner(
                &raw,
                &dependent,
                None,
                &[both, ty(&source, 3, vec![], false)]
            )
            .is_err()
    );
    let mut forged = raw.expression().clone();
    if let Type::Classifier { declaration, .. } = &mut forged {
        declaration.source.uri = "memory:foreign".into();
    }
    assert!(
        resolver
            .instantiate_runtime_operation_in_owner(
                &raw,
                &dependent,
                Some(&forged),
                &[raw.clone(), raw.clone()]
            )
            .is_err()
    );
}

#[test]
fn closed_operation_contracts_keep_legacy_wire_and_positions() {
    let (bundle, source, resolver) = context();
    let receiver = ty(&source, 6, vec![], false);
    let both = ty(&source, 2, vec![], false);
    let roots = [
        ty(&source, 0, vec![], false),
        ty(&source, 1, vec![], false),
        both.clone(),
        receiver.clone(),
    ];
    let schema =
        intersect_model::definitions::compiler::materialize_runtime_types(&bundle, &roots, None)
            .unwrap();
    let kind = schema.class_key_for_type(&receiver).unwrap();
    let effective = schema.effective_operations(kind).unwrap();
    for resolution in schema.resolved_operations(kind).unwrap() {
        assert_eq!(
            resolution.selected.position,
            effective
                .iter()
                .find(|entry| entry.operation.declaration
                    == resolution.selected.operation.declaration)
                .unwrap()
                .position
        );
    }
    let groups = &schema.declared_operation_selections.as_ref().unwrap()[kind].groups;
    for group in groups {
        let wire = serde_json::to_value(group).unwrap();
        assert!(wire.get("selected_owners").is_none());
        for operation in &group.operations {
            assert!(
                serde_json::to_value(operation)
                    .unwrap()
                    .get("projection")
                    .is_none()
            );
        }
    }
    let closed =
        intersect_model::definitions::generic_types::ClosedType::new(receiver.expression().clone())
            .unwrap();
    let argument =
        intersect_model::definitions::generic_types::ClosedType::new(both.expression().clone())
            .unwrap();
    let operation = id(&source, vec![4, 1]);
    let legacy = schema
        .instantiate_generic_operation(&resolver, &closed, &operation, &[argument])
        .unwrap();
    assert_eq!(legacy.receiver, closed);
}

#[test]
fn raw_method_nested_bounds_do_not_capture_class_formals_and_f_bounds_terminate() {
    let (_, source, resolver) = context();
    let receiver = ty(&source, 4, vec![], true);
    let a = ty(&source, 2, vec![], false);
    let b = ty(&source, 12, vec![], false);
    let bad = ty(&source, 3, vec![], false);
    let pair = |left: &RuntimeType, right: &RuntimeType| {
        ty(
            &source,
            11,
            vec![left.expression().clone(), right.expression().clone()],
            false,
        )
    };
    let nested = id(&source, vec![4, 5]);
    assert!(
        resolver
            .instantiate_runtime_operation_in_owner(&receiver, &nested, None, &[pair(&a, &b)])
            .is_ok()
    );
    assert!(
        resolver
            .instantiate_runtime_operation_in_owner(&receiver, &nested, None, &[pair(&a, &bad)])
            .is_err()
    );
    let concrete = ty(&source, 4, vec![a.expression().clone()], false);
    assert!(
        resolver
            .instantiate_runtime_operation_in_owner(&concrete, &nested, None, &[pair(&a, &b)])
            .is_err()
    );
    let recursive = id(&source, vec![4, 6]);
    assert!(
        resolver
            .instantiate_runtime_operation_in_owner(
                &receiver,
                &recursive,
                None,
                &[ty(&source, 14, vec![], false)]
            )
            .is_ok()
    );
    assert!(
        resolver
            .instantiate_runtime_operation_in_owner(&receiver, &recursive, None, &[a])
            .is_err()
    );
}

#[test]
fn runtime_method_arguments_and_bounds_share_one_aggregate_budget() {
    let mut text = String::from(
        r#"<e:EPackage xmlns:e="http://www.eclipse.org/emf/2002/Ecore" xmlns:x="http://www.w3.org/2001/XMLSchema-instance" name="budget" nsURI="urn:runtime-method-budget">"#,
    );
    for index in 0..128 {
        text.push_str(&format!(
            r#"<eClassifiers x:type="e:EClass" name="Leaf{index}"/>"#
        ));
    }
    text.push_str(r#"<eClassifiers x:type="e:EClass" name="Wide">"#);
    for index in 0..128 {
        text.push_str(&format!(r#"<eTypeParameters name="P{index}"/>"#));
    }
    text.push_str(r#"</eClassifiers><eClassifiers x:type="e:EClass" name="Owner">"#);
    for (name, count, bounds) in [
        ("single", 1, 0),
        ("within", 127, 0),
        ("over", 128, 0),
        ("one_bound", 127, 1),
        ("two_bounds", 127, 2),
    ] {
        text.push_str(&format!(r#"<eOperations name="{name}">"#));
        for index in 0..count {
            if index < bounds {
                text.push_str(&format!(r##"<eTypeParameters name="P{index}"><eBounds eClassifier="#//Wide"/></eTypeParameters>"##));
            } else {
                text.push_str(&format!(r#"<eTypeParameters name="P{index}"/>"#));
            }
        }
        text.push_str("</eOperations>");
    }
    text.push_str("</eClassifiers></e:EPackage>");
    let source = DefinitionSource::new("memory:runtime-operation-budget", SourceKind::Ecore, text);
    let (source, declarations) =
        intersect_model_acceptance::migration_fixture::migrate_source_fixture(source);
    let bundle = DefinitionBundle {
        version: 1,
        sources: vec![source.clone()],
        namespaces: vec![],
    };
    let resolver = Resolver::new(&bundle).unwrap();
    let source = FixtureSource { declarations };
    let receiver = ty(&source, 129, vec![], false);
    let arguments: Vec<_> = (0..128)
        .map(|index| {
            let leaf = ty(&source, index, vec![], false);
            ty(&source, 128, vec![leaf.expression().clone(); 128], false)
        })
        .collect();
    // Check representative individual applications without repeating the same shape 128 times.
    for argument in [&arguments[0], &arguments[63], &arguments[127]] {
        resolver
            .instantiate_runtime_operation_in_owner(
                &receiver,
                &id(&source, vec![129, 0]),
                None,
                std::slice::from_ref(argument),
            )
            .unwrap();
    }
    // 127 * (one application + 128 arguments) = 16383 checks.
    resolver
        .instantiate_runtime_operation_in_owner(
            &receiver,
            &id(&source, vec![129, 1]),
            None,
            &arguments[..127],
        )
        .unwrap();
    let over = resolver
        .instantiate_runtime_operation_in_owner(
            &receiver,
            &id(&source, vec![129, 2]),
            None,
            &arguments,
        )
        .unwrap_err();
    assert_eq!(
        over.kind,
        intersect_model::definitions::ecore::ErrorKind::ResourceLimit
    );
    assert!(over.message.contains("budget"), "{}", over.message);
    // One remaining step can check one bound, but cannot check two bounds.
    let one = resolver
        .instantiate_runtime_operation_in_owner(
            &receiver,
            &id(&source, vec![129, 3]),
            None,
            &arguments[..127],
        )
        .unwrap();
    assert_eq!(one.argument_bounds.len(), 1);
    let two = resolver
        .instantiate_runtime_operation_in_owner(
            &receiver,
            &id(&source, vec![129, 4]),
            None,
            &arguments[..127],
        )
        .unwrap_err();
    assert_eq!(
        two.kind,
        intersect_model::definitions::ecore::ErrorKind::ResourceLimit
    );
    assert!(two.message.contains("budget"), "{}", two.message);
    resolver
        .instantiate_runtime_operation_in_owner(
            &receiver,
            &id(&source, vec![129, 0]),
            None,
            &arguments[..1],
        )
        .unwrap();
}

#[test]
fn symbolic_formals_cannot_be_injected_into_retained_runtime_operation_templates() {
    let (bundle, source, _) = context();
    let receiver = ty(&source, 4, vec![], true);
    let roots = [
        ty(&source, 0, vec![], false),
        ty(&source, 1, vec![], false),
        receiver.clone(),
    ];
    let mut schema =
        intersect_model::definitions::compiler::materialize_runtime_types(&bundle, &roots, None)
            .unwrap();
    let kind = schema.class_key_for_type(&receiver).unwrap().to_owned();
    schema.validate_declared_operation_inventory(&kind).unwrap();
    let semantic = DefinitionSource::new(
        "memory:injected-behavior.rcore",
        SourceKind::RcoreBehavior,
        "rcore 1; package injected version \"1\";",
    );
    let rigid = Type::RigidParameter {
        identity: intersect_model::definitions::generic_types::RigidParameterId {
            source: semantic.id,
            declaration: 0,
            parameter: 0,
        },
    };
    let operation = id(&source, vec![4, 2]);
    let retained = schema
        .declared_operation_selections
        .as_mut()
        .unwrap()
        .get_mut(&kind)
        .unwrap()
        .groups
        .iter_mut()
        .flat_map(|group| &mut group.operations)
        .find(|entry| entry.operation.declaration == operation)
        .unwrap();
    let template = retained.template.as_mut().unwrap();
    let signature = template
        .signatures
        .iter_mut()
        .find(|signature| signature.owner == operation && signature.feature == "eType")
        .unwrap();
    signature.specialized = vec![rigid.clone()];
    retained.projection.as_mut().unwrap().source = template.clone();
    retained.operation.result.target =
        Some(intersect_model::definitions::operations::TypeTarget::Expression(rigid));
    let replay: intersect_model::definitions::schema::Schema =
        serde_json::from_value(serde_json::to_value(schema).unwrap()).unwrap();
    assert!(replay.validate_declared_operation_inventory(&kind).is_err());
}
