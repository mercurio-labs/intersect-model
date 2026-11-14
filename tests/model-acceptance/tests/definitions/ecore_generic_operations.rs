use intersect_model::definitions::{
    bundle::{DefinitionBundle, DefinitionSource, SourceKind},
    generic_resolution::Resolver,
    generic_types::{ClosedType, Type},
    resources::DeclarationId,
};
const FIXTURE: &str = include_str!("../../../../fixtures/ecore-generics/members.ecore");
fn context(text: &str) -> (Resolver, DefinitionSource) {
    let source = DefinitionSource::new("memory:operations", SourceKind::Ecore, text);
    let resolver = Resolver::new(&DefinitionBundle {
        version: 1,
        sources: vec![source.clone()],
        namespaces: vec![],
    })
    .unwrap();
    (resolver, source)
}
fn id(source: &DefinitionSource, path: Vec<usize>) -> DeclarationId {
    DeclarationId {
        source: source.id.clone(),
        path,
    }
}
fn class(source: &DefinitionSource, index: usize) -> ClosedType {
    ClosedType::new(Type::Classifier {
        declaration: id(source, vec![index]),
        arguments: vec![],
        raw: false,
    })
    .unwrap()
}
#[test]
fn explicit_operation_arguments_specialize_inherited_shadowed_formals() {
    let (resolver, source) = context(FIXTURE);
    let receiver = class(&source, 2);
    let argument = class(&source, 0);
    let operation = id(&source, vec![1, 2]);
    let report = resolver
        .instantiate_operation(&receiver, &operation, std::slice::from_ref(&argument))
        .unwrap();
    assert_eq!(report.operation.declaration, operation);
    assert_eq!(
        report.operation.formals[0].declaration.declaration.path,
        vec![1, 2, 0]
    );
    assert_eq!(report.argument_bounds.len(), 1);
    assert!(report.argument_bounds[0].satisfied);
    assert_eq!(
        report.argument_bounds[0].substituted_bound,
        argument.expression().clone()
    );
    for signature in &report.operation.signatures {
        for actual in &signature.specialized {
            assert_eq!(actual, argument.expression());
        }
    }
    assert_eq!(
        report.operation.source_payload,
        resolver.instantiate_members(&receiver).unwrap().members[1].source_payload
    );
    for arguments in [
        vec![],
        vec![argument.clone(), argument],
        vec![class(&source, 3)],
    ] {
        assert!(
            resolver
                .instantiate_operation(&receiver, &operation, &arguments)
                .is_err()
        );
    }
    assert!(
        resolver
            .instantiate_operation(&receiver, &id(&source, vec![1, 1]), &[])
            .is_err()
    );
    let mut stale = operation.clone();
    stale.source.sha256 = "0".repeat(64);
    assert!(
        resolver
            .instantiate_operation(&receiver, &stale, &[])
            .is_err()
    );
    assert!(
        resolver
            .instantiate_operation(&class(&source, 0), &operation, &[])
            .is_err()
    );
}
#[test]
fn operation_bounds_see_all_local_arguments_before_substitution() {
    let text = FIXTURE.replace(
        "<eTypeParameters name=\"T\"><eBounds eTypeParameter=\"#//Base/T\"/></eTypeParameters>",
        "<eTypeParameters name=\"U\"><eBounds eTypeParameter=\"#//Base/T\"/></eTypeParameters><eTypeParameters name=\"T\"><eBounds eTypeParameter=\"#//Base/convert/U\"/></eTypeParameters>");
    let (resolver, source) = context(&text);
    let receiver = class(&source, 2);
    let operation = id(&source, vec![1, 2]);
    let value = class(&source, 0);
    let report = resolver
        .instantiate_operation(&receiver, &operation, &[value.clone(), value.clone()])
        .unwrap();
    assert_eq!(report.argument_bounds.len(), 2);
    assert!(
        report
            .argument_bounds
            .iter()
            .all(|b| b.satisfied && &b.substituted_bound == value.expression())
    );
    let error = resolver
        .instantiate_operation(&receiver, &operation, &[value, class(&source, 3)])
        .unwrap_err();
    assert!(error.message.contains("violates generic bounds"));
    assert_eq!(error.span.source, source.id);
}

#[test]
fn explicit_owner_selects_exact_inherited_method_projection() {
    let fixture = include_str!("../../../../fixtures/ecore-generics/multiple-method-owners.ecore");
    let (resolver, source) = context(fixture);
    let receiver = class(&source, 5);
    let operation = id(&source, vec![1, 2]);
    let value = class(&source, 0);
    let projections: Vec<_> = resolver
        .instantiate_members(&receiver)
        .unwrap()
        .members
        .into_iter()
        .filter(|member| member.declaration == operation)
        .collect();
    assert_eq!(projections.len(), 2);
    assert!(
        resolver
            .instantiate_operation(&receiver, &operation, std::slice::from_ref(&value))
            .is_err()
    );
    for projection in &projections {
        let report = resolver
            .instantiate_operation_in_owner(
                &receiver,
                &operation,
                Some(&projection.owner),
                std::slice::from_ref(&value),
            )
            .unwrap();
        assert_eq!(report.operation.owner, projection.owner);
        let input = report
            .operation
            .signatures
            .iter()
            .find(|signature| signature.owner.path == vec![1, 2, 2])
            .unwrap();
        if matches!(&projection.owner, Type::Classifier { raw: true, .. }) {
            assert_eq!(
                input.specialized,
                vec![Type::Parameter {
                    declaration: id(&source, vec![1, 0])
                }]
            );
        } else {
            assert_eq!(input.specialized, vec![value.expression().clone()]);
        }

        assert_eq!(report.operation.source_payload, projection.source_payload);
        assert!(report.argument_bounds.iter().all(|bound| bound.satisfied));
        assert_eq!(
            report.operation.formals[0].declaration.declaration.path,
            vec![1, 2, 0]
        );
        for arguments in [vec![], vec![value.clone(), value.clone()]] {
            assert!(
                resolver
                    .instantiate_operation_in_owner(
                        &receiver,
                        &operation,
                        Some(&projection.owner),
                        &arguments
                    )
                    .is_err()
            );
        }
    }
    let applied_owner = projections
        .iter()
        .find(|p| matches!(&p.owner, Type::Classifier { raw: false, .. }))
        .unwrap();
    assert!(
        resolver
            .instantiate_operation_in_owner(
                &receiver,
                &operation,
                Some(&applied_owner.owner),
                &[class(&source, 3)]
            )
            .is_err()
    );
    assert!(
        resolver
            .instantiate_operation_in_owner(
                &receiver,
                &operation,
                Some(value.expression()),
                std::slice::from_ref(&value)
            )
            .is_err()
    );
    let mut stale = applied_owner.owner.clone();
    if let Type::Classifier { declaration, .. } = &mut stale {
        declaration.source.sha256 = "0".repeat(64);
    }
    assert!(
        resolver
            .instantiate_operation_in_owner(
                &receiver,
                &operation,
                Some(&stale),
                std::slice::from_ref(&value)
            )
            .is_err()
    );
    assert!(
        resolver
            .instantiate_operation_in_owner(
                &receiver,
                &id(&source, vec![1, 1]),
                Some(&applied_owner.owner),
                &[]
            )
            .is_err()
    );
    // Same exact application reached through two paths still needs no selector.
    let diamond = resolver
        .instantiate_operation(&class(&source, 7), &operation, std::slice::from_ref(&value))
        .unwrap();
    assert_eq!(diamond.operation.owner, applied_owner.owner);
    let (reversed, revised) = context(&fixture.replace("#//Raw #//Child", "#//Child #//Raw"));
    let projected = reversed.instantiate_members(&class(&revised, 5)).unwrap();
    assert_eq!(
        projected
            .members
            .iter()
            .filter(|m| m.declaration == id(&revised, vec![1, 2]))
            .count(),
        2
    );
}

#[test]
fn explicit_owner_does_not_certify_unresolved_raw_bounds_or_features() {
    let fixture = include_str!("../../../../fixtures/ecore-generics/multiple-method-owners.ecore");
    for text in [
        fixture.replace(
            "<eBounds eClassifier=\"#//Value\"/>",
            "<eBounds eTypeParameter=\"#//Base/T\"/>",
        ),
        fixture.replace(
            "<eGenericType eClassifier=\"#//Value\"/><eAnnotations",
            "<eGenericType eTypeParameter=\"#//Base/T\"/><eAnnotations",
        ),
    ] {
        let (resolver, source) = context(&text);
        let owner = Type::Classifier {
            declaration: id(&source, vec![1]),
            arguments: vec![],
            raw: true,
        };
        assert!(
            resolver
                .instantiate_operation_in_owner(
                    &class(&source, 5),
                    &id(&source, vec![1, 2]),
                    Some(&owner),
                    &[class(&source, 0)]
                )
                .is_err()
        );
    }
}
