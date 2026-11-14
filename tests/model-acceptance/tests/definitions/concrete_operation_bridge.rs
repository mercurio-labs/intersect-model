use intersect_model::definitions::{
    bundle::{DefinitionBundle, DefinitionSource, SourceKind},
    compiler::materialize_runtime_types,
    generic_resolution::Resolver,
    generic_types::{RuntimeType, Type},
    operations::{Operation, TypeTarget},
    resources::DeclarationId,
    schema::Schema,
};
const TEXT: &str =
    include_str!("../../../../fixtures/ecore-generics/concrete-operation-bridge.ecore");
struct Fixture {
    bundle: DefinitionBundle,
    source: DefinitionSource,
    resolver: Resolver,
    schema: Schema,
    declarations: std::collections::BTreeMap<Vec<usize>, DeclarationId>,
}
impl Fixture {
    fn new() -> Self {
        let source =
            DefinitionSource::new("memory:concrete-operation-bridge", SourceKind::Ecore, TEXT);
        let (source, declarations) =
            intersect_model_acceptance::migration_fixture::migrate_source_fixture(source);
        let bundle = DefinitionBundle {
            version: 1,
            sources: vec![source.clone()],
            namespaces: vec![],
        };
        let resolver = Resolver::new(&bundle).unwrap();
        let ty = |index| {
            RuntimeType::new(Type::Classifier {
                declaration: declarations[&vec![index]].clone(),
                arguments: vec![],
                raw: false,
            })
            .unwrap()
        };
        let schema =
            materialize_runtime_types(&bundle, &[ty(0), ty(1), ty(6), ty(8), ty(9)], None).unwrap();
        Self {
            bundle,
            source,
            resolver,
            schema,
            declarations,
        }
    }
    fn id(&self, path: Vec<usize>) -> DeclarationId {
        self.declarations
            .get(&path)
            .cloned()
            .unwrap_or_else(|| DeclarationId {
                source: self.source.id.clone(),
                path,
            })
    }
    fn ty(&self, index: usize) -> RuntimeType {
        RuntimeType::new(Type::Classifier {
            declaration: self.id(vec![index]),
            arguments: vec![],
            raw: false,
        })
        .unwrap()
    }
}
fn mutate(schema: &mut Schema, id: &DeclarationId, change: impl Fn(&mut Operation)) {
    for class in schema.classes.values_mut() {
        for operation in &mut class.operations {
            if &operation.declaration == id {
                change(operation);
            }
        }
    }
    for set in schema
        .declared_operation_selections
        .as_mut()
        .unwrap()
        .values_mut()
    {
        for group in &mut set.groups {
            for entry in &mut group.operations {
                if &entry.operation.declaration == id {
                    change(&mut entry.operation);
                }
            }
        }
    }
}
#[test]
fn concrete_local_inherited_specialized_and_void_operations_share_source_bridge() {
    let fixture = Fixture::new();
    for (receiver, operation) in [
        (6, vec![6, 0]),
        (6, vec![6, 1]),
        (9, vec![6, 0]),
        (8, vec![7, 1]),
    ] {
        let report = fixture
            .schema
            .instantiate_runtime_operation(
                &fixture.resolver,
                &fixture.ty(receiver),
                &fixture.id(operation),
                None,
                &[],
            )
            .unwrap();
        assert!(report.operation.formals.is_empty());
    }
    let retained = serde_json::to_value(&fixture.schema).unwrap();
    for set in fixture
        .schema
        .declared_operation_selections
        .as_ref()
        .unwrap()
        .values()
    {
        for group in &set.groups {
            assert!(group.selected_owners.is_empty());
            for operation in &group.operations {
                assert!(operation.projection.is_none());
                assert!(operation.template.is_none());
            }
        }
    }
    let replay = materialize_runtime_types(
        &fixture.bundle,
        &[
            fixture.ty(0),
            fixture.ty(1),
            fixture.ty(6),
            fixture.ty(8),
            fixture.ty(9),
        ],
        None,
    )
    .unwrap();
    assert_eq!(retained, serde_json::to_value(&replay).unwrap());
}
#[test]
fn valid_known_target_and_metadata_forgeries_fail_fresh_source_comparison() {
    let fixture = Fixture::new();
    let operation = fixture.id(vec![6, 0]);
    let receiver = fixture.ty(6);
    let key = fixture.schema.class_key_for_type(&receiver).unwrap();
    let other = fixture
        .schema
        .class_key_for_type(&fixture.ty(1))
        .unwrap()
        .to_owned();
    let alias = fixture
        .schema
        .datatypes
        .iter()
        .find(|(_, datatype)| datatype.declaration() == &fixture.id(vec![3]))
        .unwrap()
        .0
        .clone();
    let other_enum = fixture
        .schema
        .enumerations
        .iter()
        .find(|(_, enumeration)| enumeration.declaration() == &fixture.id(vec![5]))
        .unwrap()
        .0
        .clone();
    for case in 0..10 {
        let mut forged = fixture.schema.clone();
        mutate(&mut forged, &operation, |op| match case {
            0 => op.parameters[0].value.target = Some(TypeTarget::Class(other.clone())),
            1 => op.result.target = Some(TypeTarget::Class(other.clone())),
            2 => op.parameters[1].value.target = Some(TypeTarget::Datatype(alias.clone())),
            3 => op.parameters[2].value.target = Some(TypeTarget::Builtin("EBoolean".into())),
            4 => op.parameters[3].value.target = Some(TypeTarget::Enumeration(other_enum.clone())),
            5 => op.result.target = None,
            6 => op.parameters[0].value.lower = 0,
            7 => op.parameters[1].value.upper = 1,
            8 => op.parameters[0].value.ordered = true,
            _ => op.result.unique = true,
        });
        forged.validate_declared_operation_inventory(key).unwrap();
        let error = forged
            .instantiate_runtime_operation(&fixture.resolver, &receiver, &operation, None, &[])
            .unwrap_err();
        assert!(
            error.message.contains("differs from checked source"),
            "case {case}: {}",
            error.message
        );
    }
}
#[test]
fn source_owner_parameter_order_and_request_identity_are_checked() {
    let fixture = Fixture::new();
    let operation = fixture.id(vec![6, 0]);
    let receiver = fixture.ty(6);
    assert!(
        fixture
            .schema
            .instantiate_runtime_operation(
                &fixture.resolver,
                &receiver,
                &operation,
                None,
                &[fixture.ty(0)]
            )
            .is_err()
    );
    assert!(
        fixture
            .schema
            .instantiate_runtime_operation(
                &fixture.resolver,
                &receiver,
                &operation,
                Some(fixture.ty(1).expression()),
                &[]
            )
            .is_err()
    );
    for case in 0..4 {
        let mut forged = fixture.schema.clone();
        mutate(&mut forged, &operation, |op| match case {
            0 => op.parameters.swap(0, 1),
            1 => op.parameters[0].name = "changed".into(),
            2 => op.parameters[0].declaration.path.push(0),
            _ => op.source_payload.push(' '),
        });
        assert!(
            forged
                .instantiate_runtime_operation(&fixture.resolver, &receiver, &operation, None, &[])
                .is_err()
        );
    }
    let altered = DefinitionSource::new(
        &fixture.source.id.uri,
        SourceKind::Ecore,
        TEXT.replace("exact-source", "changed-source"),
    );
    let (altered, _) =
        intersect_model_acceptance::migration_fixture::migrate_source_fixture(altered);
    let resolver = Resolver::new(&DefinitionBundle {
        version: 1,
        sources: vec![altered],
        namespaces: vec![],
    })
    .unwrap();
    assert!(
        fixture
            .schema
            .instantiate_runtime_operation(&resolver, &receiver, &operation, None, &[])
            .is_err()
    );
}

struct OpaqueProvider(intersect_model::definitions::conversion::ConversionContract);
impl intersect_model::definitions::conversion::ConversionProvider for OpaqueProvider {
    fn contract(&self) -> &intersect_model::definitions::conversion::ConversionContract {
        &self.0
    }
    fn parse(&self, text: &str) -> Result<serde_json::Value, String> {
        Ok(serde_json::Value::String(text.into()))
    }
    fn format(&self, value: &serde_json::Value) -> Result<String, String> {
        value
            .as_str()
            .map(str::to_owned)
            .ok_or("Expected string payload".into())
    }
    fn validate(&self, value: &serde_json::Value) -> Result<(), String> {
        if value.is_string() {
            Ok(())
        } else {
            Err("Expected string payload".into())
        }
    }
    fn default_value(&self) -> Result<Option<serde_json::Value>, String> {
        Ok(None)
    }
}
struct AppliedProvider {
    contract: intersect_model::definitions::conversion::AppliedConversionContract,
    implementation: OpaqueProvider,
}
impl intersect_model::definitions::conversion::AppliedConversionProvider for AppliedProvider {
    fn contract(&self) -> &intersect_model::definitions::conversion::AppliedConversionContract {
        &self.contract
    }
    fn implementation(&self) -> &dyn intersect_model::definitions::conversion::ConversionProvider {
        &self.implementation
    }
}
#[test]
fn custom_and_applied_operation_targets_keep_exact_provider_backed_identity() {
    use intersect_model::definitions::{
        compiler::import_datatype_bundle,
        conversion::{
            AppliedConversionContract, ConversionContract, ConversionRegistry, ProviderId,
        },
        generic_types::ClosedType,
    };
    let text = r##"<e:EPackage xmlns:e="http://www.eclipse.org/emf/2002/Ecore" xmlns:x="http://www.w3.org/2001/XMLSchema-instance" name="provided" nsURI="urn:provided-operations">
 <eClassifiers x:type="e:EDataType" name="Text" instanceClassName="java.lang.String"/>
 <eClassifiers x:type="e:EDataType" name="Alias" instanceClassName="java.lang.String"/>
 <eClassifiers x:type="e:EDataType" name="Custom" instanceClassName="example.Custom"/>
 <eClassifiers x:type="e:EDataType" name="Container" instanceClassName="example.Container"><eTypeParameters name="T"/></eClassifiers>
 <eClassifiers x:type="e:EClass" name="Owner">
 <eOperations name="custom" eType="#//Custom"><eParameters name="value" eType="#//Custom"/></eOperations>
 <eOperations name="container"><eGenericType eClassifier="#//Container"><eTypeArguments eClassifier="#//Text"/></eGenericType><eParameters name="value"><eGenericType eClassifier="#//Container"><eTypeArguments eClassifier="#//Text"/></eGenericType></eParameters></eOperations>
 <eOperations name="other"><eGenericType eClassifier="#//Container"><eTypeArguments eClassifier="#//Alias"/></eGenericType></eOperations>
 </eClassifiers></e:EPackage>"##;
    let source = DefinitionSource::new("memory:provided-operation-bridge", SourceKind::Ecore, text);
    let (source, declarations) =
        intersect_model_acceptance::migration_fixture::migrate_source_fixture(source);
    let id = |path| declarations[&path].clone();
    let ty = |index, arguments| Type::Classifier {
        declaration: id(vec![index]),
        arguments,
        raw: false,
    };
    let bundle = DefinitionBundle {
        version: 1,
        sources: vec![source.clone()],
        namespaces: vec![],
    };
    let resolver = Resolver::new(&bundle).unwrap();
    let contract = |index| ConversionContract {
        datatype: id(vec![index]),
        provider: ProviderId {
            name: "test.operation-identity".into(),
            version: "1".into(),
        },
        serializable: true,
        declarations: vec![],
        members: vec![],
    };
    let custom = OpaqueProvider(contract(2));
    let providers: Vec<_> = [0, 1]
        .into_iter()
        .map(|index| AppliedProvider {
            contract: AppliedConversionContract {
                application: ClosedType::new(ty(3, vec![ty(index, vec![])])).unwrap(),
                conversion: contract(3),
            },
            implementation: OpaqueProvider(contract(3)),
        })
        .collect();
    let mut registry = ConversionRegistry::default();
    registry
        .register(
            &import_datatype_bundle(&bundle, &id(vec![2])).unwrap(),
            &custom,
        )
        .unwrap();
    let raw = import_datatype_bundle(&bundle, &id(vec![3])).unwrap();
    for provider in &providers {
        let checked = resolver
            .instantiate_datatype(&provider.contract.application)
            .unwrap();
        registry
            .register_application(&raw, &checked, provider)
            .unwrap();
    }
    let receiver = RuntimeType::new(ty(4, vec![])).unwrap();
    let schema =
        materialize_runtime_types(&bundle, std::slice::from_ref(&receiver), Some(&registry))
            .unwrap();
    for index in 0..3 {
        schema
            .instantiate_runtime_operation(&resolver, &receiver, &id(vec![4, index]), None, &[])
            .unwrap();
    }
    let wrong = schema
        .datatypes
        .iter()
        .find(|(_, datatype)| datatype.application() == Some(&providers[1].contract.application))
        .unwrap()
        .0
        .clone();
    let mut forged = schema.clone();
    let operation = id(vec![4, 1]);
    mutate(&mut forged, &operation, |op| {
        op.parameters[0].value.target = Some(TypeTarget::Datatype(wrong.clone()))
    });
    forged
        .validate_declared_operation_inventory(schema.class_key_for_type(&receiver).unwrap())
        .unwrap();
    assert!(
        forged
            .instantiate_runtime_operation(&resolver, &receiver, &operation, None, &[])
            .is_err()
    );
    assert!(materialize_runtime_types(&bundle, std::slice::from_ref(&receiver), None).is_err());
    let replay =
        materialize_runtime_types(&bundle, std::slice::from_ref(&receiver), Some(&registry))
            .unwrap();
    assert_eq!(schema, replay);
}
