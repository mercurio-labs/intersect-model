use intersect_model::definitions::{
    bundle::{DefinitionBundle, DefinitionSource, SourceKind},
    compiler::materialize_runtime_types,
    generic_resolution::Resolver,
    generic_types::{ClosedType, RuntimeType, Type},
    resources::DeclarationId,
};

struct FixtureSource {
    declarations: std::collections::BTreeMap<Vec<usize>, DeclarationId>,
}

fn fixture(body: &str) -> (DefinitionBundle, FixtureSource) {
    try_fixture(body).unwrap()
}
fn try_fixture(body: &str) -> Result<(DefinitionBundle, FixtureSource), String> {
    let source = DefinitionSource::new(
        "memory:operation-policy.ecore",
        SourceKind::Ecore,
        format!(
            "<e:EPackage xmlns:e='http://www.eclipse.org/emf/2002/Ecore' xmlns:x='http://www.w3.org/2001/XMLSchema-instance' name='policy' nsURI='urn:operation-policy'>{body}</e:EPackage>"
        ),
    );
    let report =
        intersect_model::migration::migrate_ecore(source, "memory:operation-policy.rcore").unwrap();
    let source = report.converted.ok_or_else(|| {
        report
            .issues
            .iter()
            .map(|issue| issue.message.as_str())
            .collect::<Vec<_>>()
            .join("; ")
    })?;
    let migration = report
        .correspondence
        .into_iter()
        .map(|row| (row.original.path, row.converted))
        .collect();
    Ok((
        DefinitionBundle {
            version: 1,
            sources: vec![source.clone()],
            namespaces: vec![],
        },
        FixtureSource {
            declarations: migration,
        },
    ))
}
fn id(source: &FixtureSource, path: &[usize]) -> DeclarationId {
    source.declarations[path].clone()
}
fn ty(source: &FixtureSource, index: usize) -> RuntimeType {
    RuntimeType::new(Type::Classifier {
        declaration: id(source, &[index]),
        arguments: vec![],
        raw: false,
    })
    .unwrap()
}
fn operation(target: &str, generic: bool, many: bool) -> String {
    let bounds = if many { " upperBound='-1'" } else { "" };
    let parameter = if generic {
        format!(
            "<eParameters name='p'{bounds}><eGenericType eClassifier='#//{target}'/></eParameters>"
        )
    } else {
        format!("<eParameters name='p'{bounds} eType='#//{target}'/>")
    };
    format!("<eOperations name='f'>{parameter}</eOperations>")
}
const SCALARS: &str = "<eClassifiers x:type='e:EDataType' name='A' instanceClassName='java.lang.String'/><eClassifiers x:type='e:EDataType' name='B' instanceClassName='java.lang.String'/>";

#[test]
fn concrete_alias_and_exact_identity_policies_agree_through_schema_bridge() {
    for generic in [false, true] {
        for same in [false, true] {
            for many in [false, true] {
                let base = operation("A", generic, false);
                let child = operation(if same { "A" } else { "B" }, generic, many);
                let (bundle, source) = fixture(&format!(
                    "{SCALARS}<eClassifiers x:type='e:EClass' name='Base'>{base}</eClassifiers><eClassifiers x:type='e:EClass' name='Child' eSuperTypes='#//Base'>{child}</eClassifiers>"
                ));
                let resolver = Resolver::new(&bundle).unwrap();
                let receiver = ty(&source, 3);
                let comparison = resolver
                    .operation_override(&id(&source, &[3, 0]), &id(&source, &[2, 0]))
                    .unwrap();
                assert_eq!(comparison.overrides, same);
                assert_eq!(
                    comparison.parameters[0].raw_types,
                    vec![id(&source, &[if same { 0 } else { 1 }])]
                );
                let selections = resolver
                    .resolved_generic_operations(
                        &ClosedType::new(receiver.expression().clone()).unwrap(),
                    )
                    .unwrap();
                assert_eq!(selections.operations.len(), if same { 1 } else { 2 });
                if same {
                    assert_eq!(
                        selections.operations[0].selected.member.declaration,
                        id(&source, &[3, 0])
                    );
                    assert_eq!(
                        selections.operations[0].overridden[0].member.declaration,
                        id(&source, &[2, 0])
                    );
                }
                let schema =
                    materialize_runtime_types(&bundle, std::slice::from_ref(&receiver), None)
                        .unwrap();
                let key = schema.class_key_for_type(&receiver).unwrap();
                assert_eq!(
                    schema.resolved_operations(key).unwrap().len(),
                    selections.operations.len()
                );
                for selection in &selections.operations {
                    let declaration = &selection.selected.member.declaration;
                    let report = schema
                        .instantiate_runtime_operation(&resolver, &receiver, declaration, None, &[])
                        .unwrap();
                    assert_eq!(report.operation.declaration, *declaration);
                    assert!(report.operation.formals.is_empty());
                }
                let replay: DefinitionBundle =
                    serde_json::from_value(serde_json::to_value(&bundle).unwrap()).unwrap();
                assert_eq!(
                    schema,
                    materialize_runtime_types(&replay, std::slice::from_ref(&receiver), None)
                        .unwrap()
                );
            }
        }
    }
}

#[test]
fn local_multiplicity_is_not_a_native_overload_discriminator() {
    for generic in [false, true] {
        for many in [false, true] {
            let first = operation("A", generic, false);
            for same in [false, true] {
                let second = operation(if same { "A" } else { "B" }, generic, many);
                let attempted = try_fixture(&format!(
                    "{SCALARS}<eClassifiers x:type='e:EClass' name='Host'>{first}{second}</eClassifiers>"
                ));
                let (bundle, source) = match attempted {
                    Ok(fixture) => fixture,
                    Err(error) => {
                        assert!(same && error.contains("Duplicate local"), "{error}");
                        continue;
                    }
                };
                let resolver = Resolver::new(&bundle).unwrap();
                let receiver = ty(&source, 2);
                let selected = resolver.resolved_generic_operations(
                    &ClosedType::new(receiver.expression().clone()).unwrap(),
                );
                let compiled = materialize_runtime_types(&bundle, &[receiver], None);
                if same {
                    assert!(
                        selected
                            .unwrap_err()
                            .message
                            .contains("Duplicate local generic operation signature")
                    );
                    assert!(
                        compiled
                            .unwrap_err()
                            .message
                            .contains("Duplicate local generic operation signature")
                    );
                } else {
                    assert_eq!(selected.unwrap().operations.len(), 2);
                    assert!(compiled.is_ok());
                }
            }
        }
    }
}

#[test]
fn instance_class_and_type_name_distinction_is_source_evidence_not_java_execution() {
    // Valid Java type-name erasures match the shared List class name. These source
    // metadata checks do not assert a native value codec for arbitrary Java types.
    let declarations = "<eClassifiers x:type='e:EDataType' name='A' instanceClassName='java.util.List' instanceTypeName='java.util.List&lt;java.lang.String&gt;'/><eClassifiers x:type='e:EDataType' name='B' instanceClassName='java.util.List' instanceTypeName='java.util.List&lt;java.lang.Integer&gt;'/>";
    for generic in [false, true] {
        let (bundle, source) = fixture(&format!(
            "{declarations}<eClassifiers x:type='e:EClass' name='Base'>{}</eClassifiers><eClassifiers x:type='e:EClass' name='Child' eSuperTypes='#//Base'>{}</eClassifiers>",
            operation("A", generic, false),
            operation("B", generic, false)
        ));
        let resolver = Resolver::new(&bundle).unwrap();
        assert!(
            !resolver
                .operation_override(&id(&source, &[3, 0]), &id(&source, &[2, 0]))
                .unwrap()
                .overrides
        );
        let receiver = ClosedType::new(ty(&source, 3).expression().clone()).unwrap();
        assert_eq!(
            resolver
                .resolved_generic_operations(&receiver)
                .unwrap()
                .operations
                .len(),
            2
        );
        assert!(materialize_runtime_types(&bundle, &[ty(&source, 3)], None).is_err());
    }
}
