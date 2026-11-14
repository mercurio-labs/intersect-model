use super::*;
use crate::definitions::bundle::DefinitionSource;

#[test]
fn equivalent_constructor_bound_domains_do_not_retag_generic_value_views() {
    let source = DefinitionSource::new(
        "memory:symbolic-value-domains",
        SourceKind::Ecore,
        r##"<e:EPackage xmlns:e="http://www.eclipse.org/emf/2002/Ecore" xmlns:x="http://www.w3.org/2001/XMLSchema-instance" name="domains" nsURI="urn:value-domains">
<eClassifiers x:type="e:EDataType" name="A" instanceClassName="java.lang.String"/>
<eClassifiers x:type="e:EDataType" name="B" instanceClassName="java.lang.String"/>
<eClassifiers x:type="e:EClass" name="Box"><eTypeParameters name="T"><eBounds eClassifier="#//A"/></eTypeParameters><eStructuralFeatures x:type="e:EAttribute" name="value"><eGenericType eTypeParameter="#//Box/T"/></eStructuralFeatures></eClassifiers>
</e:EPackage>"##,
    );
    let (source, ids) = crate::definitions::migration_test_fixture::source_with_ids(source);
    let semantic = DefinitionSource::new(
        "memory:value-domains-behavior.rcore",
        SourceKind::RcoreBehavior,
        "rcore 1; package domains version \"1\";",
    );
    let identity = RigidParameterId {
        source: semantic.id.clone(),
        declaration: 0,
        parameter: 0,
    };
    let span = SourceSpan {
        source: semantic.id.clone(),
        start: 0,
        end: 1,
    };
    let bundle = DefinitionBundle {
        version: 1,
        sources: vec![source.clone(), semantic],
        namespaces: vec![],
    };
    let ty = |index, arguments, raw| Type::Classifier {
        declaration: ids[&vec![index]].clone(),
        arguments,
        raw,
    };
    let a = ty(0, vec![], false);
    let b = ty(1, vec![], false);
    let resolver = Resolver::new(&bundle)
        .unwrap()
        .with_rigid_parameters(vec![(identity.clone(), vec![b.clone()], span)])
        .unwrap();
    let box_a = ty(2, vec![a.clone()], false);
    let box_b = ty(2, vec![b.clone()], false);
    let symbolic = ty(2, vec![Type::RigidParameter { identity }], false);
    assert!(
        resolver
            .symbolic_application(&box_b)
            .unwrap()
            .iter()
            .all(|check| check.satisfied)
    );
    assert!(
        resolver
            .symbolic_application(&symbolic)
            .unwrap()
            .iter()
            .all(|check| check.satisfied)
    );
    assert!(!resolver.symbolic_value_argument_equal(&a, &b).unwrap());
    assert!(!resolver.symbolic_value_conforms(&box_b, &box_a).unwrap());
    assert!(!resolver.symbolic_value_conforms(&symbolic, &box_a).unwrap());
    let raw = ty(2, vec![], true);
    assert!(resolver.symbolic_value_conforms(&symbolic, &raw).unwrap());
    assert!(!resolver.symbolic_value_conforms(&raw, &box_b).unwrap());
    let requests = [
        crate::definitions::generic_types::RuntimeType::new(box_a).unwrap(),
        crate::definitions::generic_types::RuntimeType::new(box_b).unwrap(),
    ];
    let schema =
        crate::definitions::compiler::materialize_runtime_types(&bundle, &requests, None).unwrap();
    assert!(!schema.conforms(
        schema.class_key_for_type(&requests[1]).unwrap(),
        schema.class_key_for_type(&requests[0]).unwrap()
    ));
}

#[test]
fn application_checks_reference_and_attribute_kind_before_any_feature_read() {
    let source = DefinitionSource::new(
        "memory:symbolic-kinds",
        SourceKind::Ecore,
        r##"<e:EPackage xmlns:e="http://www.eclipse.org/emf/2002/Ecore" xmlns:x="http://www.w3.org/2001/XMLSchema-instance" name="kinds" nsURI="urn:kinds">
<eClassifiers x:type="e:EDataType" name="Text" instanceClassName="java.lang.String"/>
<eClassifiers x:type="e:EClass" name="References"><eTypeParameters name="T"/><eStructuralFeatures x:type="e:EReference" name="value"><eGenericType eTypeParameter="#//References/T"/></eStructuralFeatures></eClassifiers>
<eClassifiers x:type="e:EClass" name="Value"/>
<eClassifiers x:type="e:EClass" name="Attributes"><eTypeParameters name="T"/><eStructuralFeatures x:type="e:EAttribute" name="value"><eGenericType eTypeParameter="#//Attributes/T"/></eStructuralFeatures></eClassifiers>
</e:EPackage>"##,
    );
    let semantic = DefinitionSource::new(
        "memory:kinds-behavior.rcore",
        SourceKind::RcoreBehavior,
        "rcore 1; package kinds version \"1\";",
    );
    let identity = RigidParameterId {
        source: semantic.id.clone(),
        declaration: 0,
        parameter: 0,
    };
    let span = SourceSpan {
        source: semantic.id.clone(),
        start: 0,
        end: 1,
    };
    let bundle = DefinitionBundle {
        version: 1,
        sources: vec![source.clone(), semantic],
        namespaces: vec![],
    };
    let ty = |index, arguments, raw| Type::Classifier {
        declaration: DeclarationId {
            source: source.id.clone(),
            path: vec![index],
        },
        arguments,
        raw,
    };
    let rigid = Type::RigidParameter {
        identity: identity.clone(),
    };
    for (bound, allowed, denied) in [(0, 3, 1), (2, 1, 3)] {
        let resolver = Resolver::new(&bundle)
            .unwrap()
            .with_rigid_parameters(vec![(
                identity.clone(),
                vec![ty(bound, vec![], false)],
                span.clone(),
            )])
            .unwrap();
        assert!(
            resolver
                .symbolic_application(&ty(allowed, vec![rigid.clone()], false))
                .is_ok()
        );
        let error = resolver
            .symbolic_application(&ty(denied, vec![rigid.clone()], false))
            .unwrap_err();
        assert!(
            error.message.contains("attribute/reference kind"),
            "{error:?}"
        );
        assert!(
            resolver
                .symbolic_value_argument_equal(&rigid, &rigid)
                .unwrap()
        );
        assert!(
            !resolver
                .symbolic_value_argument_equal(&rigid, &ty(bound, vec![], false))
                .unwrap()
        );
        assert!(
            resolver
                .symbolic_application(&ty(allowed, vec![], true))
                .is_err()
        );
    }
    assert!(
        Resolver::new(&bundle)
            .unwrap()
            .symbolic_value_argument_equal(&rigid, &rigid)
            .is_err()
    );
}
