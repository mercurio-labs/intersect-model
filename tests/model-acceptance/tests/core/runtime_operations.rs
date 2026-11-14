use intersect_model::core::model::Model;
use intersect_model::definitions::{
    bundle::{DefinitionBundle, DefinitionSource, SourceKind},
    compiler::materialize_runtime_types,
    generic_resolution::Resolver,
    generic_types::{RuntimeType, Type},
};
#[test]
fn models_with_raw_method_templates_keep_owner_checked_applications() {
    let source = DefinitionSource::new(
        "memory:runtime-operations",
        SourceKind::Ecore,
        include_str!("../../../../fixtures/ecore-generics/runtime-operations.ecore"),
    );
    let (source, declarations) =
        intersect_model_acceptance::migration_fixture::migrate_source_fixture(source);
    let id = |path: Vec<usize>| declarations[&path].clone();
    let ty = |index, raw| {
        RuntimeType::new(Type::Classifier {
            declaration: id(vec![index]),
            arguments: vec![],
            raw,
        })
        .unwrap()
    };
    let bundle = DefinitionBundle {
        version: 1,
        sources: vec![source.clone()],
        namespaces: vec![],
    };
    let resolver = Resolver::new(&bundle).unwrap();
    let child = ty(5, true);
    let mixed = ty(8, false);
    let actual = ty(2, false);
    let roots = [
        ty(0, false),
        ty(1, false),
        actual.clone(),
        child.clone(),
        mixed.clone(),
    ];
    let schema = materialize_runtime_types(&bundle, &roots, None).unwrap();
    let mut model = Model::default();
    for (name, receiver) in [("child", &child), ("mixed", &mixed)] {
        model
            .create(
                &schema,
                name,
                schema.class_key_for_type(receiver).unwrap(),
                "memory:model",
                0,
                0,
            )
            .unwrap();
    }
    model.validate(&schema).unwrap();
    let before = model.clone();
    let operation = id(vec![4, 1]);
    let checked = schema
        .instantiate_runtime_operation(
            &resolver,
            &child,
            &operation,
            None,
            std::slice::from_ref(&actual),
        )
        .unwrap();
    assert_ne!(
        checked.projection.source.owner,
        *checked.projection.storage_owner.expression()
    );
    assert!(
        schema
            .instantiate_runtime_operation(&resolver, &child, &operation, None, &[ty(3, false)])
            .is_err()
    );
    assert_eq!(model, before);
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
        schema
            .instantiate_runtime_operation(
                &resolver,
                &mixed,
                &operation,
                Some(&projection.source.owner),
                std::slice::from_ref(&actual),
            )
            .unwrap();
    }
    let replay = materialize_runtime_types(&bundle, &roots, None).unwrap();
    model.validate(&replay).unwrap();
    assert_eq!(schema, replay);
    assert_eq!(model, before);
}
