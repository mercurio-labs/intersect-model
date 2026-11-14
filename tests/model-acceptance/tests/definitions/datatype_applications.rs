use intersect_model::definitions::{
    bundle::{DefinitionBundle, DefinitionSource, SourceKind},
    generic_resolution::Resolver,
    generic_types::{ClosedType, Type},
    resources::DeclarationId,
};
fn input() -> (DefinitionBundle, impl Fn(usize, Vec<Type>) -> Type) {
    let source = DefinitionSource::new(
        "memory:applications.ecore",
        SourceKind::Ecore,
        include_str!("../../../../fixtures/ecore-generics/datatype-applications.ecore"),
    );
    let id = source.id.clone();
    (
        DefinitionBundle {
            version: 1,
            sources: vec![source],
            namespaces: vec![],
        },
        move |index, arguments| Type::Classifier {
            declaration: DeclarationId {
                source: id.clone(),
                path: vec![index],
            },
            arguments,
            raw: false,
        },
    )
}
#[test]
fn checked_datatype_applications_preserve_arguments_identity_and_bound_evidence() {
    let (bundle, ty) = input();
    let resolver = Resolver::new(&bundle).unwrap();
    let base = ty(0, vec![]);
    let child = ty(1, vec![]);
    let mut identities = std::collections::BTreeSet::new();
    for expression in [
        ty(3, vec![base.clone()]),
        ty(3, vec![child.clone()]),
        ty(9, vec![child.clone()]),
        ty(4, vec![base.clone(), child.clone()]),
        ty(4, vec![child.clone(), child.clone()]),
        ty(8, vec![ty(3, vec![child])]),
        ty(6, vec![]),
    ] {
        let closed = ClosedType::new(expression).unwrap();
        let checked = resolver.instantiate_datatype(&closed).unwrap();
        assert_eq!(checked.application(), &closed);
        assert_eq!(checked.declaration_span().source, bundle.sources[0].id);
        assert!(checked.bounds().iter().all(|bound| bound.satisfied));
        assert!(identities.insert(checked.application().identity().to_owned()));
        // Only the expression travels over the wire; resolving it again regenerates proof.
        let restored: ClosedType =
            serde_json::from_value(serde_json::to_value(&closed).unwrap()).unwrap();
        assert_eq!(resolver.instantiate_datatype(&restored).unwrap(), checked);
    }
    let applied = resolver
        .instantiate_datatype(&ClosedType::new(ty(3, vec![base])).unwrap())
        .unwrap();
    assert_eq!(applied.parameters()[0].syntax.declaration.path, [3, 0]);
    assert_eq!(applied.bounds().len(), 1);
    assert_eq!(
        applied.bounds()[0].parameter,
        applied.parameters()[0].syntax.declaration
    );
}
#[test]
fn datatype_application_rechecks_kind_arity_nested_bounds_and_source_revisions() {
    let (bundle, ty) = input();
    let resolver = Resolver::new(&bundle).unwrap();
    for (expression, message) in [
        (ty(0, vec![]), "must be an EDataType"),
        (ty(7, vec![]), "must be an EDataType"),
        (ty(3, vec![]), "argument count"),
        (ty(3, vec![ty(0, vec![]), ty(1, vec![])]), "argument count"),
        (ty(3, vec![ty(2, vec![])]), "violates generic bounds"),
        (
            ty(4, vec![ty(1, vec![]), ty(0, vec![])]),
            "violates generic bounds",
        ),
        (
            ty(8, vec![ty(3, vec![ty(2, vec![])])]),
            "violates generic bounds",
        ),
        (ty(8, vec![ty(5, vec![])]), "Primitive classifier"),
    ] {
        let error = resolver
            .instantiate_datatype(&ClosedType::new(expression).unwrap())
            .unwrap_err();
        assert!(error.message.contains(message), "{error:?}");
        assert_eq!(error.span.source, bundle.sources[0].id);
    }
    let mut stale = ty(1, vec![]);
    if let Type::Classifier { declaration, .. } = &mut stale {
        declaration.source.sha256 = "0".repeat(64);
    }
    assert!(
        resolver
            .instantiate_datatype(&ClosedType::new(ty(3, vec![stale])).unwrap())
            .is_err()
    );
    let mut raw = ty(3, vec![]);
    if let Type::Classifier { raw, .. } = &mut raw {
        *raw = true;
    }
    assert!(ClosedType::new(raw).is_err());
    assert!(
        ClosedType::new(ty(
            8,
            vec![Type::Wildcard {
                upper: None,
                lower: None
            }]
        ))
        .is_err()
    );
}
