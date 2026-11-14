use intersect_model::definitions::{
    bundle::{DefinitionBundle, DefinitionSource, SourceKind},
    generic_resolution::Resolver,
    generic_types::{RuntimeType, Type},
    resources::DeclarationId,
};
#[test]
fn source_domains_preserve_owners_bounds_and_independent_raw_occurrences() {
    let source = DefinitionSource::new(
        "memory:feature-domains",
        SourceKind::Ecore,
        include_str!("../../../../fixtures/ecore-generics/raw-feature-domains.ecore"),
    );
    let ty = |n, arguments, raw| {
        RuntimeType::new(Type::Classifier {
            declaration: DeclarationId {
                source: source.id.clone(),
                path: vec![n],
            },
            arguments,
            raw,
        })
        .unwrap()
    };
    let a = ty(1, vec![], false);
    let b = ty(2, vec![], false);
    let other = ty(3, vec![], false);
    let pair = |left: &RuntimeType, right: &RuntimeType| {
        ty(
            4,
            vec![left.expression().clone(), right.expression().clone()],
            false,
        )
    };
    let bundle = DefinitionBundle {
        version: 1,
        sources: vec![source.clone()],
        namespaces: vec![],
    };
    let resolver = Resolver::new(&bundle).unwrap();
    let domain = |receiver: &RuntimeType| {
        resolver
            .runtime_feature_domains(receiver)
            .unwrap()
            .remove(0)
    };
    let raw = domain(&ty(5, vec![], true));
    assert!(
        resolver
            .runtime_feature_accepts(&raw, &pair(&a, &b))
            .unwrap()
    );
    assert!(
        !resolver
            .runtime_feature_accepts(&raw, &pair(&a, &other))
            .unwrap()
    );
    let concrete = domain(&ty(5, vec![a.expression().clone()], false));
    assert!(
        resolver
            .runtime_feature_accepts(&concrete, &pair(&a, &a))
            .unwrap()
    );
    assert!(
        !resolver
            .runtime_feature_accepts(&concrete, &pair(&a, &b))
            .unwrap()
    );
    let inherited = domain(&ty(9, vec![], true));
    assert_ne!(
        inherited.projection.source.owner,
        *inherited.projection.storage_owner.expression()
    );
    assert!(
        resolver
            .runtime_feature_accepts(&inherited, &ty(6, vec![], false))
            .unwrap()
    );
    assert!(
        !resolver
            .runtime_feature_accepts(&inherited, &ty(7, vec![], false))
            .unwrap()
    );
    let intersection = domain(&ty(12, vec![], true));
    assert!(
        resolver
            .runtime_feature_accepts(&intersection, &ty(11, vec![], false))
            .unwrap()
    );
    assert!(!resolver.runtime_feature_accepts(&intersection, &a).unwrap());
    let nested = domain(&ty(16, vec![], true));
    assert!(
        resolver
            .runtime_feature_accepts(&nested, &pair(&pair(&a, &b), &a))
            .unwrap()
    );
    assert!(
        !resolver
            .runtime_feature_accepts(&nested, &pair(&pair(&a, &other), &a))
            .unwrap()
    );
    let dependent = domain(&ty(17, vec![], true));
    assert!(resolver.runtime_feature_accepts(&dependent, &b).unwrap());
    assert!(
        !resolver
            .runtime_feature_accepts(&dependent, &other)
            .unwrap()
    );
    let recursive = domain(&ty(15, vec![], true));
    assert!(
        resolver
            .runtime_feature_accepts(&recursive, &ty(14, vec![], false))
            .unwrap()
    );
    assert!(
        !resolver
            .runtime_feature_accepts(&recursive, &other)
            .unwrap()
    );
    let mut forged = raw.clone();
    forged.expression = a.expression().clone();
    assert!(resolver.runtime_feature_accepts(&forged, &a).is_err());
}
