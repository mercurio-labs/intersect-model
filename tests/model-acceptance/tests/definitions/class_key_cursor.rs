use intersect_model::definitions::{
    bundle::{DefinitionBundle, DefinitionSource, SourceKind},
    compiler,
    generic_types::{RuntimeType, Type},
    resources::DeclarationId,
    schema::{ClassKeyPreparation, Schema},
};

fn drain(schema: &Schema, cursor: &mut ClassKeyPreparation) -> (usize, Result<(), String>) {
    for step in 1..10_000 {
        match cursor.advance(schema) {
            Ok(false) => {}
            Ok(true) => return (step, Ok(())),
            Err(error) => return (step, Err(error)),
        }
    }
    panic!("class key preparation did not terminate")
}
struct Expression(Type);
impl AsRef<Type> for Expression {
    fn as_ref(&self) -> &Type {
        &self.0
    }
}

#[test]
fn wide_class_selection_yields_and_keeps_literal_identity_and_error_precedence() {
    let classes = (0..64)
        .map(|n| format!(r#"<eClassifiers x:type="e:EClass" name="A{n:02}"/>"#))
        .collect::<String>();
    let source = DefinitionSource::new(
        "memory:class-keys.ecore",
        SourceKind::Ecore,
        format!(
            r#"<e:EPackage xmlns:e="http://www.eclipse.org/emf/2002/Ecore" xmlns:x="http://www.w3.org/2001/XMLSchema-instance" name="m" nsURI="urn:class-keys">{classes}<eClassifiers x:type="e:EClass" name="Z"/></e:EPackage>"#
        ),
    );
    let (source, _) = intersect_model_acceptance::migration_fixture::migrate_source_fixture(source);
    let mut schema = compiler::compile_structure_bundle_with_conversions(
        &DefinitionBundle {
            version: 1,
            sources: vec![source.clone()],
            namespaces: vec![],
        },
        &source.id,
        "urn:class-keys",
        &Default::default(),
    )
    .unwrap();
    let target = RuntimeType::new(Type::Classifier {
        declaration: DeclarationId {
            source: source.id.clone(),
            path: vec![64],
        },
        arguments: vec![],
        raw: false,
    })
    .unwrap();
    assert!(ClassKeyPreparation::new(&target).finish().is_err());
    let mut cursor = ClassKeyPreparation::new(&target);
    let (steps, result) = drain(&schema, &mut cursor);
    result.unwrap();
    assert!(steps >= 128);
    assert!(cursor.advance(&schema).unwrap());
    assert_eq!(cursor.finish().unwrap(), "Z");
    assert_eq!(schema.class_key_for_type(&target).unwrap(), "Z");
    let declaration = DeclarationId {
        source: source.id.clone(),
        path: vec![64],
    };
    let mut legacy = ClassKeyPreparation::reference_declaration(&declaration);
    drain(&schema, &mut legacy).1.unwrap();
    assert_eq!(legacy.finish().unwrap(), "Z");
    assert_eq!(
        schema.instantiated_class_key(&target).unwrap_err(),
        "Unknown instantiated class identity"
    );
    for budget in [0, 1, 64, steps / 2, steps - 1] {
        let mut partial = ClassKeyPreparation::new(&target);
        for _ in 0..budget {
            assert!(!partial.advance(&schema).unwrap());
        }
        assert!(partial.finish().is_err());
    }
    let mut foreign = target.expression().clone();
    if let Type::Classifier { declaration, .. } = &mut foreign {
        declaration.source.sha256 = "0".repeat(64);
    }
    let mut rejected = ClassKeyPreparation::new(&Expression(foreign));
    assert_eq!(
        drain(&schema, &mut rejected).1.unwrap_err(),
        "Unknown closed class declaration"
    );
    assert_eq!(
        rejected.advance(&schema).unwrap_err(),
        "Class key preparation has failed"
    );
    assert!(rejected.finish().is_err());
    let mut duplicate = schema.classes["Z"].clone();
    duplicate.name = "ZZ".into();
    schema.classes.insert("ZZ".into(), duplicate);
    schema
        .declarations
        .insert("ZZ".into(), schema.declarations["Z"].clone());
    assert_eq!(
        drain(&schema, &mut ClassKeyPreparation::new(&target))
            .1
            .unwrap_err(),
        "Ambiguous closed class declaration"
    );
    assert_eq!(
        drain(
            &schema,
            &mut ClassKeyPreparation::reference_declaration(&declaration)
        )
        .1
        .unwrap_err(),
        "Generic reference target requires an exact runtime type"
    );
    schema
        .raw_reference_bounds
        .insert("Missing".into(), Default::default());
    let parameter = Expression(Type::Parameter {
        declaration: DeclarationId {
            source: source.id,
            path: vec![64],
        },
    });
    assert_eq!(
        drain(&schema, &mut ClassKeyPreparation::new(&parameter))
            .1
            .unwrap_err(),
        "Expected a runtime classifier"
    );
    assert_eq!(
        drain(&schema, &mut ClassKeyPreparation::instantiated(&parameter))
            .1
            .unwrap_err(),
        "Unknown class Missing"
    );
}

#[test]
fn instantiated_selection_distinguishes_full_closed_arguments_and_raw_identity() {
    let source = DefinitionSource::new(
        "memory:generic-class-keys.ecore",
        SourceKind::Ecore,
        r##"<e:EPackage xmlns:e="http://www.eclipse.org/emf/2002/Ecore" xmlns:x="http://www.w3.org/2001/XMLSchema-instance" name="m" nsURI="urn:generic-keys">
        <eClassifiers x:type="e:EClass" name="Base"/><eClassifiers x:type="e:EClass" name="Leaf" eSuperTypes="#//Base"/>
        <eClassifiers x:type="e:EClass" name="G"><eTypeParameters name="T"><eBounds eClassifier="#//Base"/></eTypeParameters>
          <eStructuralFeatures x:type="e:EReference" name="value"><eGenericType eTypeParameter="#//G/T"/></eStructuralFeatures>
        </eClassifiers></e:EPackage>"##,
    );
    let (source, _) = intersect_model_acceptance::migration_fixture::migrate_source_fixture(source);
    let expression = |n, arguments, raw| Type::Classifier {
        declaration: DeclarationId {
            source: source.id.clone(),
            path: vec![n],
        },
        arguments,
        raw,
    };
    let types = vec![
        RuntimeType::new(expression(0, vec![], false)).unwrap(),
        RuntimeType::new(expression(1, vec![], false)).unwrap(),
        RuntimeType::new(expression(2, vec![], true)).unwrap(),
        RuntimeType::new(expression(2, vec![expression(1, vec![], false)], false)).unwrap(),
    ];
    let schema = compiler::materialize_runtime_bundle(
        &DefinitionBundle {
            version: 1,
            sources: vec![source.clone()],
            namespaces: vec![],
        },
        &source.id,
        "urn:generic-keys",
        &types,
        None,
    )
    .unwrap();
    let mut keys = Vec::new();
    for target in &types[2..] {
        let mut cursor = ClassKeyPreparation::new(target);
        drain(&schema, &mut cursor).1.unwrap();
        let key = cursor.finish().unwrap();
        assert_eq!(
            schema.instantiations[&key].expression(),
            target.expression()
        );
        assert_eq!(schema.instantiated_class_key(target).unwrap(), key);
        assert_eq!(schema.class_key_for_type(target).unwrap(), key);
        keys.push(key);
    }
    assert_ne!(keys[0], keys[1]);
    let legacy = DeclarationId {
        source: source.id.clone(),
        path: vec![2],
    };
    assert_eq!(
        drain(
            &schema,
            &mut ClassKeyPreparation::reference_declaration(&legacy)
        )
        .1
        .unwrap_err(),
        "Generic reference target requires an exact runtime type"
    );
    let absent =
        RuntimeType::new(expression(2, vec![expression(0, vec![], false)], false)).unwrap();
    assert_eq!(
        drain(&schema, &mut ClassKeyPreparation::new(&absent))
            .1
            .unwrap_err(),
        "Unknown instantiated class identity"
    );
}
