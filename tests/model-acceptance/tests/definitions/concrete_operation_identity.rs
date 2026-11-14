use intersect_model::definitions::{
    bundle::{DefinitionBundle, DefinitionSource, SourceKind},
    compiler::materialize_closed_types,
    generic_resolution::Resolver,
    generic_types::{ClosedType, Type},
    rcore_behavior::checking::check,
};

#[test]
fn concrete_nominal_overloads_remain_distinct_in_both_shared_compilers_and_rcore_behavior() {
    for (builtin, instance, scalar) in [
        ("EBigInteger", "java.math.BigInteger", "BigInt"),
        ("EBigDecimal", "java.math.BigDecimal", "Decimal"),
        ("EByteArray", "byte[]", "Bytes"),
    ] {
        for generic_spelling in [false, true] {
            let parameter = if generic_spelling {
                "<eParameters name='p'><eGenericType eClassifier='#//Count'/></eParameters>"
                    .to_owned()
            } else {
                "<eParameters name='p' eType='#//Count'/>".to_owned()
            };
            let xml = format!(
                "<e:EPackage xmlns:e='http://www.eclipse.org/emf/2002/Ecore' xmlns:xsi='http://www.w3.org/2001/XMLSchema-instance' name='test' nsURI='urn:nominal'><eClassifiers xsi:type='e:EDataType' name='Count' instanceClassName='{instance}'/><eClassifiers xsi:type='e:EClass' name='Item'><eStructuralFeatures xsi:type='e:EAttribute' name='value' lowerBound='1' eType='e:EDataType http://www.eclipse.org/emf/2002/Ecore#//{builtin}'/><eStructuralFeatures xsi:type='e:EAttribute' name='count' lowerBound='1' eType='#//Count'/><eOperations name='f'>{parameter}</eOperations><eOperations name='f'><eParameters name='p' eType='e:EDataType http://www.eclipse.org/emf/2002/Ecore#//{builtin}'/></eOperations></eClassifiers></e:EPackage>"
            );
            // The old facade need not support generic XML spelling; the runtime path must.
            if !generic_spelling {
                assert_eq!(
                    intersect_model_acceptance::migration_fixture::compile_migrated_schema_fixture(
                        &xml
                    )
                    .unwrap()
                    .resolved_operations("Item")
                    .unwrap()
                    .len(),
                    2
                );
            }
            let model = DefinitionSource::new("memory:nominal.ecore", SourceKind::Ecore, xml);
            let (model, declarations) =
                intersect_model_acceptance::migration_fixture::migrate_source_fixture(model);
            let requested = ClosedType::new(Type::Classifier {
                declaration: declarations[&vec![1]].clone(),
                arguments: vec![],
                raw: false,
            })
            .unwrap();
            let mut semantic = DefinitionSource::new(
                "memory:nominal-behavior.rcore",
                SourceKind::RcoreBehavior,
                format!(
                    "rcore 1; package nominal version \"1\"; import model \"urn:nominal\" as m; fn scalar(x: m.Item) -> {scalar} = x.value; fn nominal(x: m.Item) -> m.Count = x.count;"
                ),
            );
            semantic.dependencies.push(model.id.clone());
            let bundle = DefinitionBundle {
                version: 1,
                sources: vec![model, semantic.clone()],
                namespaces: vec![],
            };
            let resolver = Resolver::new(&bundle).unwrap();
            assert_eq!(
                resolver
                    .resolved_generic_operations(&requested)
                    .unwrap()
                    .operations
                    .len(),
                2
            );
            let schema =
                materialize_closed_types(&bundle, std::slice::from_ref(&requested)).unwrap();
            let key = schema.class_key_for_type(&requested).unwrap();
            assert_eq!(schema.resolved_operations(key).unwrap().len(), 2);
            check(&bundle, &semantic.id).unwrap();
        }
    }
}
