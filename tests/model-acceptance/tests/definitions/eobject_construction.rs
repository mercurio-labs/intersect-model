use intersect_model::definitions::{
    bundle::{DefinitionBundle, DefinitionSource, SourceKind},
    compiler::materialize_runtime_types,
    generic_resolution::Resolver,
    generic_types::{RuntimeType, Type},
    operations::TypeTarget,
    schema::{EOBJECT_REFERENCE_TOP, eobject_reference_top_declaration},
};
#[test]
fn pinned_eobject_constructible_identity_preserves_every_source_signature() {
    let source = DefinitionSource::new(
        "memory:eobject-user",
        SourceKind::Ecore,
        r#"<e:EPackage xmlns:e="http://www.eclipse.org/emf/2002/Ecore" name="user" nsURI="urn:eobject-user"/>"#,
    );
    let (source, _) = intersect_model_acceptance::migration_fixture::migrate_source_fixture(source);
    let bundle = DefinitionBundle {
        version: 1,
        sources: vec![source],
        namespaces: vec![],
    };
    let ty = RuntimeType::new(Type::Classifier {
        declaration: eobject_reference_top_declaration().unwrap(),
        arguments: vec![],
        raw: false,
    })
    .unwrap();
    let schema = materialize_runtime_types(&bundle, std::slice::from_ref(&ty), None).unwrap();
    let key = schema.class_key_for_type(&ty).unwrap();
    assert_ne!(key, EOBJECT_REFERENCE_TOP);
    assert_eq!(schema.classes.len(), 1);
    assert!(schema.datatypes.is_empty());
    assert!(schema.conversion_contracts.is_empty());
    assert!(schema.constructor_contracts.is_empty());
    assert!(schema.features(key).unwrap().is_empty());
    let operations = schema.effective_operations(key).unwrap();
    assert_eq!(operations.len(), 15);
    assert_eq!(schema.resolved_operations(key).unwrap().len(), 15);
    assert_eq!(
        operations
            .iter()
            .filter(|o| o.operation.name == "eGet")
            .count(),
        2
    );
    let resolver = Resolver::new(&bundle).unwrap();
    for operation in &operations {
        let checked = schema
            .instantiate_runtime_operation(
                &resolver,
                &ty,
                &operation.operation.declaration,
                None,
                &[],
            )
            .unwrap();
        assert_eq!(
            checked.projection.source.source_payload,
            operation.operation.source_payload
        );
        assert!(checked.projection.source.formals.is_empty());
        assert!(operation.operation.generic); // Expression metadata; no method-local formals.
        for target in std::iter::once(&operation.operation.result)
            .chain(operation.operation.parameters.iter().map(|p| &p.value))
            .filter_map(|v| v.target.as_ref())
        {
            assert!(matches!(target, TypeTarget::Expression(_)));
        }
    }
    let invoke = operations
        .iter()
        .find(|o| o.operation.name == "eInvoke")
        .unwrap();
    assert!(
        matches!(&invoke.operation.parameters[1].value.target, Some(TypeTarget::Expression(Type::Classifier {arguments,..})) if matches!(arguments.as_slice(), [Type::Wildcard {upper:None, lower:None}]))
    );
    assert!(schema.conforms(key, EOBJECT_REFERENCE_TOP));
    let retained: intersect_model::definitions::schema::Schema =
        serde_json::from_str(&serde_json::to_string(&schema).unwrap()).unwrap();
    retained.validate_declared_operation_inventory(key).unwrap();
    assert_eq!(retained, schema);
    assert_eq!(
        schema,
        materialize_runtime_types(&bundle, &[ty], None).unwrap()
    );
}

#[test]
fn user_operation_eobject_targets_keep_real_class_identity_and_source_bridge() {
    let source = DefinitionSource::new(
        "memory:eobject-operations",
        SourceKind::Ecore,
        r#"<e:EPackage xmlns:e="http://www.eclipse.org/emf/2002/Ecore" xmlns:x="http://www.w3.org/2001/XMLSchema-instance" name="user" nsURI="urn:eobject-operations"><eClassifiers x:type="e:EClass" name="Owner"><eOperations name="echo" eType="e:EClass http://www.eclipse.org/emf/2002/Ecore#//EObject"><eParameters name="value" eType="e:EClass http://www.eclipse.org/emf/2002/Ecore#//EObject"/></eOperations></eClassifiers></e:EPackage>"#,
    );
    let (source, _) = intersect_model_acceptance::migration_fixture::migrate_source_fixture(source);
    let owner = RuntimeType::new(Type::Classifier {
        declaration: intersect_model::definitions::resources::DeclarationId {
            source: source.id.clone(),
            path: vec![0],
        },
        arguments: vec![],
        raw: false,
    })
    .unwrap();
    let top = RuntimeType::new(Type::Classifier {
        declaration: eobject_reference_top_declaration().unwrap(),
        arguments: vec![],
        raw: false,
    })
    .unwrap();
    let bundle = DefinitionBundle {
        version: 1,
        sources: vec![source],
        namespaces: vec![],
    };
    let schema = materialize_runtime_types(&bundle, &[owner.clone(), top.clone()], None).unwrap();
    let owner_key = schema.class_key_for_type(&owner).unwrap();
    let top_key = schema.class_key_for_type(&top).unwrap();
    let operations = schema.resolved_operations(owner_key).unwrap();
    assert_eq!(operations.len(), 1); // Implicit top does not inject reflection operations.
    let operation = operations[0].selected.operation;
    assert_eq!(
        operation.result.target,
        Some(TypeTarget::Class(top_key.into()))
    );
    assert_eq!(
        operation.parameters[0].value.target,
        operation.result.target
    );
    assert!(!operation.generic); // Ordinary closed wire remains concrete.
    schema
        .instantiate_runtime_operation(
            &Resolver::new(&bundle).unwrap(),
            &owner,
            &operation.declaration,
            None,
            &[],
        )
        .unwrap();
    assert!(schema.conforms(owner_key, top_key));
    assert!(!schema.conforms(top_key, owner_key));
}
