use intersect_model::definitions::{
    bundle::{DefinitionBundle, DefinitionSource, SourceKind},
    compiler::materialize_runtime_types,
    generic_types::{RuntimeType, Type},
    resources::DeclarationId,
};

#[test]
fn explicit_eobject_inheritance_constructs_but_nonclass_reference_targets_reject() {
    for (index, declaration) in [
        r##"<eClassifiers x:type="e:EClass" name="Owner" eSuperTypes="e:EClass http://www.eclipse.org/emf/2002/Ecore#//EObject"/>"##,
        r##"<eClassifiers x:type="e:EClass" name="Owner"><eStructuralFeatures x:type="e:EReference" name="bad" eType="e:EDataType http://www.eclipse.org/emf/2002/Ecore#//EString"/></eClassifiers>"##,
    ].into_iter().enumerate() {
        let source = DefinitionSource::new(
            "memory:top-boundary",
            SourceKind::Ecore,
            format!(
                r#"<e:EPackage xmlns:e="http://www.eclipse.org/emf/2002/Ecore" xmlns:x="http://www.w3.org/2001/XMLSchema-instance" name="boundary" nsURI="urn:boundary">{declaration}</e:EPackage>"#
            ),
        );
        if index == 1 {
            let report = intersect_model::migration::migrate_ecore(source, "memory:invalid.rcore").unwrap();
            assert!(report.converted.is_none());
            assert!(report.issues.iter().any(|issue| issue.blocks_conversion && issue.message.contains("structural feature or supertype kind")));
            continue;
        }
        let (source, _) = intersect_model_acceptance::migration_fixture::migrate_source_fixture(source);
        let owner = RuntimeType::new(Type::Classifier {
            declaration: DeclarationId {
                source: source.id.clone(),
                path: vec![0],
            },
            arguments: vec![],
            raw: false,
        })
        .unwrap();
        let bundle = DefinitionBundle {
            version: 1,
            sources: vec![source],
            namespaces: vec![],
        };
        let result = materialize_runtime_types(&bundle, std::slice::from_ref(&owner), None);
        if index == 0 {
            let schema = result.unwrap();
            let key = schema.class_key_for_type(&owner).unwrap();
            assert_eq!(schema.classes.len(), 2);
            assert_eq!(schema.effective_operations(key).unwrap().len(), 15);
            let mut model = intersect_model::core::model::Model::default();
            model.create(&schema, "owner", key, "memory:model", 0, 0).unwrap();
            model.validate(&schema).unwrap();
        } else {
            assert!(result.is_err());
        }
    }
}
