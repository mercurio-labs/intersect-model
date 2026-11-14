use intersect_model::definitions::{
    bundle::{DefinitionBundle, DefinitionSource, SourceKind},
    generic_resolution::Resolver,
    generic_types::{ParameterBinding, Substitution, Type},
    resources::DeclarationId,
};
const BODY: &str = r##"
<eClassifiers x:type="e:EClass" name="Value"/>
<eClassifiers x:type="e:EClass" name="Other"/>
<eClassifiers x:type="e:EClass" name="Base"><eTypeParameters name="T"/></eClassifiers>
<eClassifiers x:type="e:EClass" name="Left"><eTypeParameters name="U"/><eGenericSuperTypes eClassifier="#//Base"><eTypeArguments eTypeParameter="#//Left/U"/></eGenericSuperTypes></eClassifiers>
<eClassifiers x:type="e:EClass" name="Right"><eTypeParameters name="V"/><eGenericSuperTypes eClassifier="#//Base"><eTypeArguments eTypeParameter="#//Right/V"/></eGenericSuperTypes></eClassifiers>
<eClassifiers x:type="e:EClass" name="Diamond"><eTypeParameters name="W"/><eGenericSuperTypes eClassifier="#//Left"><eTypeArguments eTypeParameter="#//Diamond/W"/></eGenericSuperTypes><eGenericSuperTypes eClassifier="#//Right"><eTypeArguments eTypeParameter="#//Diamond/W"/></eGenericSuperTypes></eClassifiers>
<eClassifiers x:type="e:EClass" name="Use"><eStructuralFeatures x:type="e:EReference" name="value"><eGenericType eClassifier="#//Diamond"><eTypeArguments eClassifier="#//Value"/></eGenericType></eStructuralFeatures></eClassifiers>
"##;
fn setup(body: &str) -> (DefinitionSource, Resolver) {
    let source = DefinitionSource::new(
        "memory:inheritance",
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
    (source, resolver)
}
fn id(s: &DefinitionSource, path: &[usize]) -> DeclarationId {
    DeclarationId {
        source: s.id.clone(),
        path: path.to_vec(),
    }
}
fn class(s: &DefinitionSource, n: usize, arguments: Vec<Type>) -> Type {
    Type::Classifier {
        declaration: id(s, &[n]),
        arguments,
        raw: false,
    }
}
#[test]
fn instantiated_diamond_composes_edges_and_preserves_first_declared_path() {
    let (s, r) = setup(BODY);
    let report = r.inherited_types(&id(&s, &[6, 0, 0])).unwrap();
    let value = class(&s, 0, vec![]);
    assert_eq!(
        report
            .supertypes
            .iter()
            .map(|t| t.expression.clone())
            .collect::<Vec<_>>(),
        vec![
            class(&s, 3, vec![value.clone()]),
            class(&s, 2, vec![value.clone()]),
            class(&s, 4, vec![value])
        ]
    );
    assert_eq!(
        report.supertypes[1].path,
        vec![id(&s, &[5]), id(&s, &[3]), id(&s, &[2])]
    );
    assert_eq!(r.inherited_types(&id(&s, &[6, 0, 0])).unwrap(), report);
}
#[test]
fn nested_arguments_are_substituted_without_erasing_classifiers() {
    let body = BODY.replace(r##"<eTypeArguments eTypeParameter="#//Left/U"/>"##,
        r##"<eTypeArguments eClassifier="#//Right"><eTypeArguments eTypeParameter="#//Left/U"/></eTypeArguments>"##)
        .replace(r##"<eGenericSuperTypes eClassifier="#//Right"><eTypeArguments eTypeParameter="#//Diamond/W"/></eGenericSuperTypes>"##, "");
    let (s, r) = setup(&body);
    let report = r.inherited_types(&id(&s, &[6, 0, 0])).unwrap();
    assert_eq!(
        report.supertypes[1].expression,
        class(&s, 2, vec![class(&s, 4, vec![class(&s, 0, vec![])])])
    );
}
#[test]
fn incompatible_diamonds_cycles_and_legacy_disagreement_reject() {
    for (body, message) in [
        (BODY.replace(r##"<eGenericSuperTypes eClassifier="#//Right"><eTypeArguments eTypeParameter="#//Diamond/W"/></eGenericSuperTypes>"##,
            r##"<eGenericSuperTypes eClassifier="#//Right"><eTypeArguments eClassifier="#//Other"/></eGenericSuperTypes>"##), "Incompatible generic diamond"),
        (BODY.replace(r##"eClassifier="#//Base""##, r##"eClassifier="#//Diamond""##), "Cyclic generic inheritance"),
        (BODY.replace("name=\"Diamond\"", "name=\"Diamond\" eSuperTypes=\"#//Other\""), "Legacy eSuperTypes disagrees"),
    ] {
        let (s,r) = setup(&body);
        let error = r.inherited_types(&id(&s,&[6,0,0])).unwrap_err();
        assert!(error.message.contains(message), "{}",error.message);
        assert_eq!(error.span.source,s.id);
    }
}
#[test]
fn concrete_legacy_ancestry_and_depth_limits_use_the_same_traversal() {
    let body = r##"<eClassifiers x:type="e:EClass" name="Root"/><eClassifiers x:type="e:EClass" name="Child" eSuperTypes="#//Root"/><eClassifiers x:type="e:EClass" name="Use"><eStructuralFeatures x:type="e:EReference" name="child"><eGenericType eClassifier="#//Child"/></eStructuralFeatures></eClassifiers>"##;
    let (s, r) = setup(body);
    assert_eq!(
        r.inherited_types(&id(&s, &[2, 0, 0])).unwrap().supertypes[0].expression,
        class(&s, 0, vec![])
    );
    let mut deep = String::new();
    for n in 0..129 {
        deep.push_str(&format!(
            r##"<eClassifiers x:type="e:EClass" name="C{n}" eSuperTypes="#//C{}"/>"##,
            n + 1
        ));
    }
    deep.push_str(r##"<eClassifiers x:type="e:EClass" name="C129"/><eClassifiers x:type="e:EClass" name="Use"><eStructuralFeatures x:type="e:EReference" name="child"><eGenericType eClassifier="#//C0"/></eStructuralFeatures></eClassifiers>"##);
    let (s, r) = setup(&deep);
    assert!(
        r.inherited_types(&id(&s, &[130, 0, 0]))
            .unwrap_err()
            .message
            .contains("budget")
    );
}
#[test]
fn simultaneous_substitution_does_not_capture_receiver_arguments() {
    let (s, _) = setup(BODY);
    let t = Type::Parameter {
        declaration: id(&s, &[2, 0]),
    };
    let u = Type::Parameter {
        declaration: id(&s, &[3, 0]),
    };
    let env = Substitution::new(vec![
        ParameterBinding {
            parameter: id(&s, &[2, 0]),
            argument: u.clone(),
        },
        ParameterBinding {
            parameter: id(&s, &[3, 0]),
            argument: t.clone(),
        },
    ])
    .unwrap();
    let pair = class(&s, 5, vec![t.clone(), u.clone()]);
    assert_eq!(
        env.apply_simultaneous(&pair).unwrap(),
        class(&s, 5, vec![u, t.clone()])
    );
    assert!(env.apply(&pair).is_err());
    let nested = class(&s, 2, vec![t.clone()]);
    let env = Substitution::new(vec![ParameterBinding {
        parameter: id(&s, &[2, 0]),
        argument: nested.clone(),
    }])
    .unwrap();
    assert_eq!(env.apply_simultaneous(&t).unwrap(), nested);
    assert!(env.apply(&t).is_err());
}

#[test]
fn duplicate_direct_superclasses_reject_before_diamond_deduplication() {
    let edge = r##"<eGenericSuperTypes eClassifier="#//Left"><eTypeArguments eTypeParameter="#//Diamond/W"/></eGenericSuperTypes>"##;
    for duplicate in [
        edge.to_owned(),
        edge.replace("#//Diamond/W", "#//Value")
            .replace("eTypeParameter=", "eClassifier="),
    ] {
        let body = BODY.replace(edge, &format!("{edge}{duplicate}"));
        let (s, r) = setup(&body);
        let error = r.inherited_types(&id(&s, &[6, 0, 0])).unwrap_err();
        assert!(
            error
                .message
                .contains("Duplicate direct generic superclass"),
            "{}",
            error.message
        );
        assert_eq!(error.span.source, s.id);
    }
}

#[test]
fn raw_receiver_preserves_symbolic_ancestry_and_argument_warning() {
    let body = BODY.replace(r##"<eTypeArguments eClassifier="#//Value"/>"##, "");
    let (s, r) = setup(&body);
    let usage = id(&s, &[6, 0, 0]);
    let report = r.inherited_types(&usage).unwrap();
    assert!(matches!(
        report.receiver.expression,
        Type::Classifier { raw: true, .. }
    ));
    let formal = Type::Parameter {
        declaration: id(&s, &[5, 0]),
    };
    assert_eq!(
        report
            .supertypes
            .iter()
            .map(|t| t.expression.clone())
            .collect::<Vec<_>>(),
        vec![
            class(&s, 3, vec![formal.clone()]),
            class(&s, 2, vec![formal.clone()]),
            class(&s, 4, vec![formal])
        ]
    );
    let checked = r.application_bounds(&usage).unwrap();
    assert!(checked.valid);
    assert_eq!(checked.warnings[0].code, "raw_type");
    assert!(
        intersect_model::definitions::generic_types::ClosedType::new(report.receiver.expression)
            .is_err()
    );
}

#[test]
fn legacy_raw_parent_keeps_original_formals_through_further_edges() {
    let body = r##"
<eClassifiers x:type="e:EClass" name="Base"><eTypeParameters name="T"/></eClassifiers>
<eClassifiers x:type="e:EClass" name="Mid"><eTypeParameters name="T"/><eGenericSuperTypes eClassifier="#//Base"><eTypeArguments eTypeParameter="#//Mid/T"/></eGenericSuperTypes></eClassifiers>
<eClassifiers x:type="e:EClass" name="Child" eSuperTypes="#//Mid"/>
<eClassifiers x:type="e:EClass" name="Use"><eStructuralFeatures x:type="e:EReference" name="child"><eGenericType eClassifier="#//Child"/></eStructuralFeatures></eClassifiers>
"##;
    let (s, r) = setup(body);
    let report = r.inherited_types(&id(&s, &[3, 0, 0])).unwrap();
    assert_eq!(
        report.supertypes[0].expression,
        Type::Classifier {
            declaration: id(&s, &[1]),
            arguments: vec![],
            raw: true
        }
    );
    assert_eq!(
        report.supertypes[1].expression,
        class(
            &s,
            0,
            vec![Type::Parameter {
                declaration: id(&s, &[1, 0])
            }]
        )
    );
    assert_eq!(
        report.supertypes[1].path,
        vec![id(&s, &[2]), id(&s, &[1]), id(&s, &[0])]
    );
}

#[test]
fn mixed_raw_and_applied_ancestor_paths_are_retained_in_both_orders() {
    let raw = r##"<eClassifiers x:type="e:EClass" name="Raw" eSuperTypes="#//Base"/>"##;
    for parents in ["#//Raw #//Applied", "#//Applied #//Raw"] {
        let body = format!(
            r##"
<eClassifiers x:type="e:EClass" name="Value"/>
<eClassifiers x:type="e:EClass" name="Base"><eTypeParameters name="T"/></eClassifiers>
{raw}
<eClassifiers x:type="e:EClass" name="Applied"><eGenericSuperTypes eClassifier="#//Base"><eTypeArguments eClassifier="#//Value"/></eGenericSuperTypes></eClassifiers>
<eClassifiers x:type="e:EClass" name="Child" eSuperTypes="{parents}"/>
<eClassifiers x:type="e:EClass" name="Use"><eStructuralFeatures x:type="e:EReference" name="child"><eGenericType eClassifier="#//Child"/></eStructuralFeatures></eClassifiers>
"##
        );
        let (s, r) = setup(&body);
        let report = r.inherited_types(&id(&s, &[5, 0, 0])).unwrap();
        let bases = report
            .supertypes
            .iter()
            .filter(|t| {
                matches!(
                    &t.expression, Type::Classifier { declaration, .. } if declaration.path == [1]
                )
            })
            .collect::<Vec<_>>();
        assert_eq!(bases.len(), 2);
        assert!(
            bases
                .iter()
                .any(|t| matches!(&t.expression, Type::Classifier { raw: true, .. }))
        );
        assert!(
            bases
                .iter()
                .any(|t| matches!(&t.expression, Type::Classifier { raw: false, .. }))
        );
    }
}

#[test]
fn raw_equivalence_uses_pinned_reverse_elimination_order_without_losing_applications() {
    for (parents, valid) in [
        ("#//Raw #//Left #//Right", true),
        ("#//Left #//Raw #//Right", true),
        ("#//Left #//Right #//Raw", false),
    ] {
        let body = format!(
            r##"
<eClassifiers x:type="e:EClass" name="Value"/>
<eClassifiers x:type="e:EClass" name="Other"/>
<eClassifiers x:type="e:EClass" name="Base"><eTypeParameters name="T"/></eClassifiers>
<eClassifiers x:type="e:EClass" name="Raw" eSuperTypes="#//Base"/>
<eClassifiers x:type="e:EClass" name="Left"><eGenericSuperTypes eClassifier="#//Base"><eTypeArguments eClassifier="#//Value"/></eGenericSuperTypes></eClassifiers>
<eClassifiers x:type="e:EClass" name="Right"><eGenericSuperTypes eClassifier="#//Base"><eTypeArguments eClassifier="#//Other"/></eGenericSuperTypes></eClassifiers>
<eClassifiers x:type="e:EClass" name="Child" eSuperTypes="{parents}"/>
<eClassifiers x:type="e:EClass" name="Use"><eStructuralFeatures x:type="e:EReference" name="child"><eGenericType eClassifier="#//Child"/></eStructuralFeatures></eClassifiers>
"##
        );
        let (s, r) = setup(&body);
        let result = r.inherited_types(&id(&s, &[7, 0, 0]));
        if valid {
            let report = result.unwrap();
            let bases = report.supertypes.iter().filter(|t| matches!(
                &t.expression, Type::Classifier { declaration, .. } if declaration.path == [2]
            )).count();
            assert_eq!(bases, 3);
        } else {
            assert!(
                result
                    .unwrap_err()
                    .message
                    .contains("Incompatible generic diamond")
            );
        }
    }
}

#[test]
fn mixed_owner_applications_resolve_ancestor_formals_only_for_consistency() {
    let fixture = include_str!("../../../../fixtures/ecore-generics/mixed-ancestry.ecore");
    let body = fixture
        .split_once('>')
        .unwrap()
        .1
        .rsplit_once("</e:EPackage>")
        .unwrap()
        .0;
    let mid = r##"<eClassifiers x:type="e:EClass" name="Mid"><eTypeParameters name="M"/><eGenericSuperTypes eClassifier="#//Base"><eTypeArguments eTypeParameter="#//Mid/M"/></eGenericSuperTypes></eClassifiers>"##;
    let body = body.replace(r##"name="Raw" eSuperTypes="#//Base""##, r##"name="Raw" eSuperTypes="#//Mid""##)
        .replace(r##"<eGenericSuperTypes eClassifier="#//Base"><eTypeArguments eClassifier="#//Value"/></eGenericSuperTypes>"##,
            r##"<eGenericSuperTypes eClassifier="#//Mid"><eTypeArguments eClassifier="#//Value"/></eGenericSuperTypes>"##);
    for parents in ["#//Raw #//Applied", "#//Applied #//Raw"] {
        let (s, r) = setup(&format!(
            "{}{mid}",
            body.replace("#//Raw #//Applied", parents)
        ));
        let report = r.inherited_types(&id(&s, &[7, 1, 0])).unwrap();
        let bases = report
            .supertypes
            .iter()
            .filter(|t| {
                matches!(
                    &t.expression, Type::Classifier { declaration, .. } if declaration.path == [2]
                )
            })
            .collect::<Vec<_>>();
        assert_eq!(bases.len(), 2);
        assert!(bases.iter().any(|t| t.expression
            == class(
                &s,
                2,
                vec![Type::Parameter {
                    declaration: id(&s, &[8, 0])
                }]
            )));
        assert!(
            bases
                .iter()
                .any(|t| t.expression == class(&s, 2, vec![class(&s, 0, vec![])]))
        );
    }
}

#[test]
fn scalar_alias_ancestor_arguments_match_without_erasing_declarations() {
    let body = BODY
        .replace(r##"<eClassifiers x:type="e:EClass" name="Value"/>"##,
            r##"<eClassifiers x:type="e:EDataType" name="Value" instanceClassName="java.lang.String"/>"##)
        .replace(r##"<eClassifiers x:type="e:EClass" name="Other"/>"##,
            r##"<eClassifiers x:type="e:EDataType" name="Other" instanceClassName="java.lang.String"/>"##)
        .replace(r##"<eGenericSuperTypes eClassifier="#//Right"><eTypeArguments eTypeParameter="#//Diamond/W"/></eGenericSuperTypes>"##,
            r##"<eGenericSuperTypes eClassifier="#//Right"><eTypeArguments eClassifier="#//Other"/></eGenericSuperTypes>"##);
    let (s, r) = setup(&body);
    let report = r.inherited_types(&id(&s, &[6, 0, 0])).unwrap();
    let bases = report
        .supertypes
        .iter()
        .filter(|t| {
            matches!(
                &t.expression, Type::Classifier { declaration, .. } if declaration.path == [2]
            )
        })
        .collect::<Vec<_>>();
    assert_eq!(bases.len(), 2);
    assert_ne!(bases[0].expression, bases[1].expression);
    let unknown = body.replace(
        r#"name="Other" instanceClassName="java.lang.String""#,
        r#"name="Other" instanceClassName="example.Unknown""#,
    );
    let (s, r) = setup(&unknown);
    assert!(
        r.inherited_types(&id(&s, &[6, 0, 0]))
            .unwrap_err()
            .message
            .contains("provider contract")
    );
}
