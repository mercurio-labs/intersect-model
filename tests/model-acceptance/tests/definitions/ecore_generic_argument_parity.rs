use intersect_model::definitions::{
    bundle::{DefinitionBundle, DefinitionSource, SourceKind},
    compiler::materialize_closed_types,
    generic_resolution::Resolver,
    generic_types::{ClosedType, Type},
    resources::DeclarationId,
};
const FIXTURE: &str = include_str!("../../../../fixtures/ecore-generics/explicit-arguments.ecore");
fn context(text: &str) -> (DefinitionBundle, Resolver) {
    let bundle = DefinitionBundle {
        version: 1,
        sources: vec![DefinitionSource::new(
            "memory:argument-parity",
            SourceKind::Ecore,
            text,
        )],
        namespaces: vec![],
    };
    let resolver = Resolver::new(&bundle).unwrap();
    (bundle, resolver)
}
fn id(bundle: &DefinitionBundle, path: &[usize]) -> DeclarationId {
    DeclarationId {
        source: bundle.sources[0].id.clone(),
        path: path.to_vec(),
    }
}
fn ty(bundle: &DefinitionBundle, index: usize, arguments: Vec<Type>) -> ClosedType {
    ClosedType::new(Type::Classifier {
        declaration: id(bundle, &[index]),
        arguments,
        raw: false,
    })
    .unwrap()
}

fn materializes_after_migration(bundle: &DefinitionBundle, selected: &ClosedType) -> bool {
    let (source, declarations) =
        intersect_model_acceptance::migration_fixture::migrate_source_fixture(
            bundle.sources[0].clone(),
        );
    fn remap(
        ty: &Type,
        declarations: &std::collections::BTreeMap<Vec<usize>, DeclarationId>,
    ) -> Type {
        let Type::Classifier {
            declaration,
            arguments,
            raw,
        } = ty
        else {
            panic!("This fixture selects classifier arguments only")
        };
        Type::Classifier {
            declaration: declarations[&declaration.path].clone(),
            arguments: arguments
                .iter()
                .map(|argument| remap(argument, declarations))
                .collect(),
            raw: *raw,
        }
    }
    let selected = ClosedType::new(remap(selected.expression(), &declarations)).unwrap();
    let canonical = DefinitionBundle {
        version: 1,
        sources: vec![source],
        namespaces: vec![],
    };
    materialize_closed_types(&canonical, &[selected]).is_ok()
}

#[test]
fn explicit_generic_class_and_operation_requests_reject_primitive_arguments() {
    for primitive in [
        "boolean", "byte", "char", "double", "float", "int", "long", "short",
    ] {
        for metadata in ["instanceClassName", "instanceTypeName"] {
            let text = FIXTURE
                .replace("instanceClassName", metadata)
                .replace(r#""int""#, &format!("\"{primitive}\""));
            let (bundle, resolver) = context(&text);
            let value = ty(&bundle, 0, vec![]);
            let boxed = ty(&bundle, 1, vec![value.expression().clone()]);
            let nested = ty(&bundle, 1, vec![boxed.expression().clone()]);
            for receiver in [&boxed, &nested] {
                let error = resolver.instantiate_members(receiver).unwrap_err();
                assert!(
                    error.message.contains("Primitive classifier"),
                    "{}",
                    error.message
                );
                assert_eq!(error.span.source, bundle.sources[0].id);
                assert!(!materializes_after_migration(&bundle, receiver));
            }
            let operation = id(&bundle, &[2, 1]);
            for argument in [value, boxed] {
                let error = resolver
                    .instantiate_operation(&ty(&bundle, 2, vec![]), &operation, &[argument])
                    .unwrap_err();
                assert!(
                    error.message.contains("Primitive classifier"),
                    "{}",
                    error.message
                );
            }
            // Typed attributes retain their legal primitive types.
            assert!(resolver.type_expression(&id(&bundle, &[2, 0, 0])).is_ok());
        }
    }
}

#[test]
fn boxed_arguments_remain_accepted_and_source_arguments_follow_the_same_rule() {
    for (binding, valid) in [
        ("int", false),
        ("java.lang.Integer", true),
        ("byte[]", true),
    ] {
        for metadata in ["instanceClassName", "instanceTypeName"] {
            let text = FIXTURE
                .replace("instanceClassName", metadata)
                .replace(r#""int""#, &format!("\"{binding}\""));
            let (bundle, resolver) = context(&text);
            let argument = ty(&bundle, 0, vec![]);
            let boxed = ty(&bundle, 1, vec![argument.expression().clone()]);
            assert_eq!(resolver.instantiate_members(&boxed).is_ok(), valid);
            assert_eq!(materializes_after_migration(&bundle, &boxed), valid);
            assert_eq!(
                resolver
                    .instantiate_operation(
                        &ty(&bundle, 2, vec![]),
                        &id(&bundle, &[2, 1]),
                        &[argument]
                    )
                    .is_ok(),
                valid
            );
            let source_application = text.replace("</e:EPackage>", r##"<eClassifiers x:type="e:EClass" name="Use"><eStructuralFeatures x:type="e:EReference" name="box"><eGenericType eClassifier="#//Box"><eTypeArguments eClassifier="#//Value"/></eGenericType></eStructuralFeatures></eClassifiers></e:EPackage>"##);
            let (bundle, resolver) = context(&source_application);
            assert_eq!(
                resolver
                    .application_bounds(&id(&bundle, &[3, 0, 0]))
                    .is_ok(),
                valid
            );
        }
    }
}
