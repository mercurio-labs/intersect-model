use intersect_model::definitions::{
    bundle::{DefinitionBundle, DefinitionSource, SourceKind},
    generic_resolution::{MemberTypes, Resolver},
    generic_types::Type,
    resources::DeclarationId,
};
const BODY: &str = r##"
<eClassifiers x:type="e:EClass" name="Value"/>
<eClassifiers x:type="e:EClass" name="Base">
  <eTypeParameters name="T"/>
  <eStructuralFeatures x:type="e:EReference" name="value" lowerBound="1" upperBound="-1"><eGenericType eTypeParameter="#//Base/T"/><eAnnotations source="retained"/></eStructuralFeatures>
  <eOperations name="convert">
    <eTypeParameters name="T"><eBounds eTypeParameter="#//Base/T"/></eTypeParameters>
    <eGenericType eTypeParameter="#//Base/convert/T"/>
    <eParameters name="input"><eGenericType eTypeParameter="#//Base/T"/></eParameters>
    <eGenericExceptions eClassifier="#//Value"/>
  </eOperations>
</eClassifiers>
<eClassifiers x:type="e:EClass" name="Child"><eGenericSuperTypes eClassifier="#//Base"><eTypeArguments eClassifier="#//Value"/></eGenericSuperTypes></eClassifiers>
<eClassifiers x:type="e:EClass" name="Use"><eStructuralFeatures x:type="e:EReference" name="child"><eGenericType eClassifier="#//Child"/></eStructuralFeatures></eClassifiers>
"##;
fn run(body: &str) -> Result<MemberTypes, intersect_model::definitions::ecore::ImportError> {
    let source = DefinitionSource::new(
        "memory:members",
        SourceKind::Ecore,
        format!(
            r#"<e:EPackage xmlns:e="http://www.eclipse.org/emf/2002/Ecore" xmlns:x="http://www.w3.org/2001/XMLSchema-instance" name="g" nsURI="urn:g">{body}</e:EPackage>"#
        ),
    );
    let usage = DeclarationId {
        source: source.id.clone(),
        path: vec![3, 0, 0],
    };
    Resolver::new(&DefinitionBundle {
        version: 1,
        sources: vec![source],
        namespaces: vec![],
    })
    .unwrap()
    .member_types(&usage)
}
#[test]
fn inherited_members_specialize_class_formals_and_preserve_operation_shadowing() {
    let report = run(BODY).unwrap();
    assert_eq!(report.members.len(), 2);
    let feature = &report.members[0];
    assert_eq!(feature.name, "value");
    assert_eq!(feature.declaration.path, vec![1, 1]);
    let value = Type::Classifier {
        declaration: DeclarationId {
            source: feature.declaration.source.clone(),
            path: vec![0],
        },
        arguments: vec![],
        raw: false,
    };
    assert_eq!(feature.signatures[0].specialized, vec![value.clone()]);
    assert!(matches!(
        feature.signatures[0].declared[0],
        Type::Parameter { .. }
    ));
    assert!(feature.source_payload.contains("upperBound=\"-1\""));
    assert!(feature.source_payload.contains("source=\"retained\""));
    let operation = &report.members[1];
    assert_eq!(operation.declaration.path, vec![1, 2]);
    assert_eq!(operation.formals[0].bounds, vec![value.clone()]);
    assert_eq!(
        operation.formals[0].declaration.declaration.path,
        vec![1, 2, 0]
    );
    assert_eq!(
        operation.signatures[0].specialized,
        vec![Type::Parameter {
            declaration: operation.formals[0].declaration.declaration.clone()
        }]
    );
    assert_eq!(operation.signatures[1].feature, "eExceptions");
    assert_eq!(operation.signatures[1].specialized, vec![value.clone()]);
    assert_eq!(operation.signatures[2].owner.path, vec![1, 2, 2]);
    assert_eq!(operation.signatures[2].specialized, vec![value]);
}
#[test]
fn substituted_feature_kind_rejects_class_values_in_attributes_and_datatypes_in_references() {
    let bad_attribute = BODY.replace("e:EReference\" name=\"value", "e:EAttribute\" name=\"value");
    assert!(
        run(&bad_attribute)
            .unwrap_err()
            .message
            .contains("attribute/reference kind")
    );
    let datatype = BODY.replace(r##"<eTypeArguments eClassifier="#//Value"/>"##,
        r##"<eTypeArguments eClassifier="e:EDataType http://www.eclipse.org/emf/2002/Ecore#//EString"/>"##);
    assert!(
        run(&datatype)
            .unwrap_err()
            .message
            .contains("attribute/reference kind")
    );
    let valid_attribute =
        datatype.replace("e:EReference\" name=\"value", "e:EAttribute\" name=\"value");
    assert!(run(&valid_attribute).is_ok());
}
#[test]
fn legacy_types_void_results_and_colliding_declarations_are_retained() {
    let body = BODY.replace(r##"<eGenericType eTypeParameter="#//Base/convert/T"/>"##, "")
        .replace(r##"<eParameters name="input"><eGenericType eTypeParameter="#//Base/T"/></eParameters>"##,
            r##"<eParameters name="input" eType="#//Value"/>"##)
        .replace(r##"name="Child"><eGenericSuperTypes"##,
            r##"name="Child"><eStructuralFeatures x:type="e:EReference" name="value" eType="#//Value"/><eGenericSuperTypes"##);
    let report = run(&body).unwrap();
    assert_eq!(
        report
            .members
            .iter()
            .filter(|member| member.name == "value")
            .count(),
        2
    );
    assert_ne!(report.members[0].declaration, report.members[1].declaration);
    let op = report
        .members
        .iter()
        .find(|member| member.name == "convert")
        .unwrap();
    assert!(op.signatures[0].specialized.is_empty());
    assert_eq!(op.signatures[2].declared, op.signatures[2].specialized);
}

#[test]
fn explicit_requests_need_no_source_application_and_recheck_every_identity() {
    use intersect_model::definitions::generic_types::ClosedType;
    let body = BODY
        .split(r#"<eClassifiers x:type="e:EClass" name="Child""#)
        .next()
        .unwrap();
    let source = DefinitionSource::new(
        "memory:template",
        SourceKind::Ecore,
        format!(
            r#"<e:EPackage xmlns:e="http://www.eclipse.org/emf/2002/Ecore" xmlns:x="http://www.w3.org/2001/XMLSchema-instance" name="g" nsURI="urn:g">{body}</e:EPackage>"#
        ),
    );
    let resolver = Resolver::new(&DefinitionBundle {
        version: 1,
        sources: vec![source.clone()],
        namespaces: vec![],
    })
    .unwrap();
    let id = |path: Vec<usize>| DeclarationId {
        source: source.id.clone(),
        path,
    };
    let expression = Type::Classifier {
        declaration: id(vec![1]),
        arguments: vec![Type::Classifier {
            declaration: id(vec![0]),
            arguments: vec![],
            raw: false,
        }],
        raw: false,
    };
    let requested = ClosedType::new(expression.clone()).unwrap();
    let report = resolver.instantiate_members(&requested).unwrap();
    assert_eq!(report.receiver, requested);
    assert_eq!(report.members.len(), 2);
    assert_eq!(report.members[0].declaration.path, vec![1, 1]);
    assert_eq!(
        report.members[0].signatures[0].specialized,
        vec![Type::Classifier {
            declaration: id(vec![0]),
            arguments: vec![],
            raw: false
        }]
    );
    assert_eq!(report.declaration_span.source, source.id);
    assert_eq!(report, resolver.instantiate_members(&requested).unwrap());
    for mode in 0..4 {
        let mut forged = expression.clone();
        if let Type::Classifier {
            declaration,
            arguments,
            ..
        } = &mut forged
        {
            match mode {
                0 => arguments.clear(),
                1 => declaration.source.sha256 = "1".repeat(64),
                2 => {
                    if let Type::Classifier { declaration, .. } = &mut arguments[0] {
                        declaration.path = vec![1, 0];
                    }
                }
                _ => {
                    if let Type::Classifier { declaration, .. } = &mut arguments[0] {
                        declaration.source.sha256 = "2".repeat(64);
                    }
                }
            }
        }
        // Structural closedness alone accepts these; source rechecking must reject.
        assert!(
            resolver
                .instantiate_members(&ClosedType::new(forged).unwrap())
                .is_err()
        );
    }
}
#[test]
fn explicit_requests_enforce_formal_bounds_before_projecting_members() {
    use intersect_model::definitions::generic_types::ClosedType;
    let body = BODY.split(r#"<eClassifiers x:type="e:EClass" name="Child""#).next().unwrap()
        .replace(r#"<eTypeParameters name="T"/>"#,r##"<eTypeParameters name="T"><eBounds eClassifier="#//Constraint"/></eTypeParameters>"##);
    let source = DefinitionSource::new(
        "memory:bounded-template",
        SourceKind::Ecore,
        format!(
            r#"<e:EPackage xmlns:e="http://www.eclipse.org/emf/2002/Ecore" xmlns:x="http://www.w3.org/2001/XMLSchema-instance" name="g" nsURI="urn:g">{body}<eClassifiers x:type="e:EClass" name="Constraint"/></e:EPackage>"#
        ),
    );
    let resolver = Resolver::new(&DefinitionBundle {
        version: 1,
        sources: vec![source.clone()],
        namespaces: vec![],
    })
    .unwrap();
    let id = |path: Vec<usize>| DeclarationId {
        source: source.id.clone(),
        path,
    };
    for (actual, valid) in [(0, false), (2, true)] {
        let requested = ClosedType::new(Type::Classifier {
            declaration: id(vec![1]),
            arguments: vec![Type::Classifier {
                declaration: id(vec![actual]),
                arguments: vec![],
                raw: false,
            }],
            raw: false,
        })
        .unwrap();
        let result = resolver.instantiate_members(&requested);
        assert_eq!(result.is_ok(), valid);
        if let Err(error) = result {
            assert!(error.message.contains("violates generic bounds"));
            assert_eq!(error.span.source, source.id);
        }
    }
}
