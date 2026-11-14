use intersect_model::definitions::{
    bundle::{DefinitionBundle, DefinitionSource, SourceKind},
    generic_resolution::{OperationOverride, Resolver},
    resources::DeclarationId,
};
const FIXTURE: &str = include_str!("../../../../fixtures/ecore-generics/operation-overrides.ecore");
fn context(text: &str) -> (Resolver, impl Fn(&[usize]) -> DeclarationId) {
    let source = DefinitionSource::new("memory:overrides", SourceKind::Ecore, text);
    let source_id = source.id.clone();
    let resolver = Resolver::new(&DefinitionBundle {
        version: 1,
        sources: vec![source],
        namespaces: vec![],
    })
    .unwrap();
    (resolver, move |path| DeclarationId {
        source: source_id.clone(),
        path: path.to_vec(),
    })
}
fn compare(
    text: &str,
) -> Result<OperationOverride, intersect_model::definitions::ecore::ImportError> {
    let (resolver, id) = context(text);
    resolver.operation_override(&id(&[3, 1]), &id(&[2, 1]))
}
#[test]
fn generic_overrides_preserve_declared_erasure_and_parameter_identities() {
    let (resolver, id) = context(FIXTURE);
    let report = resolver
        .operation_override(&id(&[3, 1]), &id(&[2, 1]))
        .unwrap();
    assert!(report.overrides);
    assert_eq!(report.parameters[0].owner, id(&[3, 1, 1]));
    assert_eq!(report.base_parameters[0].owner, id(&[2, 1, 0]));
    assert_eq!(report.parameters[0].raw_types, vec![id(&[1])]);
    assert_eq!(report.base_parameters[0].raw_types, vec![id(&[0])]);
    assert!(
        !resolver
            .operation_override(&id(&[2, 1]), &id(&[3, 1]))
            .unwrap()
            .overrides
    );
    assert!(
        !resolver
            .operation_override(&id(&[4, 0]), &id(&[2, 1]))
            .unwrap()
            .overrides
    );
    assert!(
        resolver
            .operation_override(&id(&[3, 1]), &id(&[3, 1]))
            .unwrap()
            .overrides
    );
    let unbounded = FIXTURE
        .replace(r##"<eBounds eClassifier="#//A"/>"##, "")
        .replace(r##"<eBounds eClassifier="#//B"/>"##, "");
    let report = compare(&unbounded).unwrap();
    assert!(report.overrides);
    assert_eq!(
        report.parameters[0].raw_types,
        report.base_parameters[0].raw_types
    );
    assert_ne!(
        report.parameters[0].raw_types[0].source.uri,
        "memory:overrides"
    );
}
#[test]
fn names_arity_and_order_discriminate_but_parameter_names_and_results_do_not() {
    assert!(
        compare(&FIXTURE.replace("renamed", "anotherName"))
            .unwrap()
            .overrides
    );
    assert!(
        compare(&FIXTURE.replace(r##" eType="#//B">"##, ">"))
            .unwrap()
            .overrides
    );
    assert!(
        !compare(
            &FIXTURE
                .replace(r##"name="f" eType"##, r##"name="g" eType"##)
                .replace("#//Derived/f/U", "#//Derived/g/U")
        )
        .unwrap()
        .overrides
    );
    let extra = FIXTURE.replace(
        "    </eOperations>",
        r##"<eParameters name="extra" eType="#//A"/></eOperations>"##,
    );
    assert!(!compare(&extra).unwrap().overrides);
    let distinct = FIXTURE.replace(
        "instanceTypeName=\"java.lang.String\"",
        "instanceTypeName=\"java.lang.Integer\"",
    );
    assert!(!compare(&distinct).unwrap().overrides);
    let ordered = distinct
        .replace(
            "</eParameters></eOperations>",
            r##"</eParameters><eParameters name="second" eType="#//B"/></eOperations>"##,
        )
        .replace(
            "    </eOperations>",
            r##"<eParameters name="second" eType="#//A"/></eOperations>"##,
        );
    assert!(!compare(&ordered).unwrap().overrides);
    let matching_order = ordered
        .replace(
            r##"<eBounds eClassifier="#//B"/></eTypeParameters>"##,
            r##"<eBounds eClassifier="#//A"/></eTypeParameters>"##,
        )
        .replace(
            r##"name="second" eType="#//A""##,
            r##"name="second" eType="#//B""##,
        );
    assert!(compare(&matching_order).unwrap().overrides);
}
#[test]
fn first_bound_erasure_is_not_receiver_argument_substitution() {
    let text = FIXTURE
        .replace(
            r##"<eBounds eClassifier="#//B"/></eTypeParameters>"##,
            r##"<eBounds eClassifier="#//A"/></eTypeParameters>"##,
        )
        .replacen(
            r##"<eBounds eClassifier="#//A"/>"##,
            r##"<eBounds eClassifier="#//A"/><eBounds eClassifier="#//B"/>"##,
            1,
        );
    let (resolver, id) = context(&text);
    let report = resolver
        .operation_override(&id(&[3, 1]), &id(&[2, 1]))
        .unwrap();
    assert!(report.overrides);
    assert_eq!(report.base_parameters[0].raw_types, vec![id(&[0])]);
    // The inherited receiver actually supplies B; comparison still uses declared A.
    assert!(text.contains(r##"<eTypeArguments eClassifier="#//B"/>"##));
}
#[test]
fn missing_types_unknown_equivalence_and_stale_sources_are_explicit_errors() {
    let missing = FIXTURE.replace(r##"<eGenericType eTypeParameter="#//Derived/f/U"/>"##, "");
    assert!(
        compare(&missing)
            .unwrap_err()
            .message
            .contains("exactly one raw type")
    );
    let unknown = FIXTURE.replace(
        "instanceTypeName=\"java.lang.String\"",
        "instanceTypeName=\"example.Unknown\"",
    );
    assert!(
        compare(&unknown)
            .unwrap_err()
            .message
            .contains("provider contract")
    );
    let (resolver, id) = context(FIXTURE);
    let mut stale = id(&[3, 1]);
    stale.source.sha256 = "0".repeat(64);
    assert!(resolver.operation_override(&stale, &id(&[2, 1])).is_err());
    assert!(
        resolver
            .operation_override(&id(&[3]), &id(&[2, 1]))
            .is_err()
    );
}

#[test]
fn operation_comparison_budget_has_a_checked_boundary() {
    for count in [1024, 1025] {
        let parameters = (0..count)
            .map(|n| format!(r##"<eParameters name="p{n}" eType="#//A"/>"##))
            .collect::<String>();
        let text = format!(
            r##"<e:EPackage xmlns:e="http://www.eclipse.org/emf/2002/Ecore" xmlns:x="http://www.w3.org/2001/XMLSchema-instance" name="p" nsURI="urn:p"><eClassifiers x:type="e:EDataType" name="A" instanceClassName="java.lang.String"/><eClassifiers x:type="e:EClass" name="C"><eOperations name="f">{parameters}</eOperations></eClassifiers></e:EPackage>"##
        );
        let (resolver, id) = context(&text);
        let result = resolver.operation_override(&id(&[1, 0]), &id(&[1, 0]));
        if count == 1024 {
            assert!(result.unwrap().overrides);
        } else {
            assert_eq!(
                result.unwrap_err().kind,
                intersect_model::definitions::ecore::ErrorKind::ResourceLimit
            );
        }
    }
}

#[test]
fn native_class_and_enum_identity_is_not_an_unchecked_instance_name() {
    for kind in ["EClass", "EEnum"] {
        let text = FIXTURE.replace("e:EDataType", &format!("e:{kind}"));
        assert!(!compare(&text).unwrap().overrides);
        let same = text.replace(
            r##"<eBounds eClassifier="#//B"/></eTypeParameters>"##,
            r##"<eBounds eClassifier="#//A"/></eTypeParameters>"##,
        );
        assert!(compare(&same).unwrap().overrides);
    }
}

fn receiver(id: DeclarationId) -> intersect_model::definitions::generic_types::ClosedType {
    intersect_model::definitions::generic_types::ClosedType::new(
        intersect_model::definitions::generic_types::Type::Classifier {
            declaration: id,
            arguments: vec![],
            raw: false,
        },
    )
    .unwrap()
}
#[test]
fn generic_selection_preserves_projected_winner_and_overridden_declarations() {
    let (resolver, id) = context(FIXTURE);
    let result = resolver
        .resolved_generic_operations(&receiver(id(&[3])))
        .unwrap();
    assert_eq!(result.operations.len(), 1);
    let operation = &result.operations[0];
    assert_eq!(operation.selected.member.declaration, id(&[3, 1]));
    assert_eq!(operation.selected.member.formals.len(), 1);
    assert_eq!(operation.overridden.len(), 1);
    assert_eq!(operation.overridden[0].member.declaration, id(&[2, 1]));
    assert_eq!(
        operation.selected.raw_parameters[0].raw_types,
        vec![id(&[1])]
    );
    assert_eq!(
        operation.overridden[0].raw_parameters[0].raw_types,
        vec![id(&[0])]
    );
    assert!(
        resolver
            .resolved_generic_operations(&receiver(id(&[0])))
            .is_err()
    );
    let chain = FIXTURE.replace("</e:EPackage>", r##"<eClassifiers x:type="e:EClass" name="Further" eSuperTypes="#//Derived"><eOperations name="f"><eParameters name="p" eType="#//A"/></eOperations></eClassifiers></e:EPackage>"##);
    let (resolver, id) = context(&chain);
    let result = resolver
        .resolved_generic_operations(&receiver(id(&[5])))
        .unwrap();
    assert_eq!(
        result.operations[0].selected.member.declaration,
        id(&[5, 0])
    );
    assert_eq!(
        result.operations[0]
            .overridden
            .iter()
            .map(|op| op.member.declaration.clone())
            .collect::<Vec<_>>(),
        vec![id(&[3, 1]), id(&[2, 1])]
    );
}
#[test]
fn sibling_ambiguity_and_explicit_resolution_are_independent_of_parent_order() {
    let right = r##"<eClassifiers x:type="e:EClass" name="Right"><eGenericSuperTypes eClassifier="#//Base"><eTypeArguments eClassifier="#//B"/></eGenericSuperTypes><eOperations name="f"><eParameters name="p" eType="#//A"/></eOperations></eClassifiers>"##;
    for parents in ["#//Derived #//Right", "#//Right #//Derived"] {
        let ending = format!(
            r##"{right}<eClassifiers x:type="e:EClass" name="Diamond" eSuperTypes="{parents}"/></e:EPackage>"##
        );
        let text = FIXTURE.replace("</e:EPackage>", &ending);
        let (resolver, id) = context(&text);
        assert!(
            resolver
                .resolved_generic_operations(&receiver(id(&[6])))
                .unwrap_err()
                .message
                .contains("Ambiguous inherited")
        );
        let resolved = text.replace(&format!(r##"name="Diamond" eSuperTypes="{parents}"/>"##), &format!(r##"name="Diamond" eSuperTypes="{parents}"><eOperations name="f"><eParameters name="p" eType="#//A"/></eOperations></eClassifiers>"##));
        let (resolver, id) = context(&resolved);
        let report = resolver
            .resolved_generic_operations(&receiver(id(&[6])))
            .unwrap();
        assert_eq!(
            report.operations[0].selected.member.declaration,
            id(&[6, 0])
        );
        let mut overridden = report.operations[0]
            .overridden
            .iter()
            .map(|op| op.member.declaration.clone())
            .collect::<Vec<_>>();
        overridden.sort();
        assert_eq!(overridden, vec![id(&[2, 1]), id(&[3, 1]), id(&[5, 1])]);
        let shared = text.replace(
            r##"<eOperations name="f"><eParameters name="p" eType="#//A"/></eOperations>"##,
            "",
        );
        let (resolver, id) = context(&shared);
        let report = resolver
            .resolved_generic_operations(&receiver(id(&[6])))
            .unwrap();
        assert_eq!(report.operations.len(), 1);
        assert_eq!(
            report.operations[0].selected.member.declaration,
            id(&[3, 1])
        );
        assert_eq!(report.operations[0].overridden.len(), 1);
    }
}
#[test]
fn overloads_and_local_erasure_collisions_are_distinct() {
    let source = FIXTURE.replace(
        "#//Derived/f/U",
        "#//@eClassifiers.3/@eOperations.0/@eTypeParameters.0",
    );
    let overloads = source.replace(
        "    </eOperations>",
        r##"</eOperations><eOperations name="f"/><eOperations name="g"/>"##,
    );
    let (resolver, id) = context(&overloads);
    let report = resolver
        .resolved_generic_operations(&receiver(id(&[3])))
        .unwrap();
    assert_eq!(report.operations.len(), 3);
    let duplicate = source.replace("    </eOperations>", r##"</eOperations><eOperations name="f"><eParameters name="differentName" eType="#//A"/></eOperations>"##);
    let (resolver, id) = context(&duplicate);
    assert!(
        resolver
            .resolved_generic_operations(&receiver(id(&[3])))
            .unwrap_err()
            .message
            .contains("Duplicate local")
    );
}
#[test]
fn generic_selection_handles_empty_classes_and_candidate_budget_boundary() {
    for count in [0, 128, 129] {
        let ops = (0..count)
            .map(|n| format!(r#"<eOperations name="f{n}"/>"#))
            .collect::<String>();
        let text = format!(
            r#"<e:EPackage xmlns:e="http://www.eclipse.org/emf/2002/Ecore" xmlns:x="http://www.w3.org/2001/XMLSchema-instance" name="p" nsURI="urn:p"><eClassifiers x:type="e:EClass" name="C">{ops}</eClassifiers></e:EPackage>"#
        );
        let (resolver, id) = context(&text);
        let result = resolver.resolved_generic_operations(&receiver(id(&[0])));
        if count <= 128 {
            assert_eq!(result.unwrap().operations.len(), count);
        } else {
            assert_eq!(
                result.unwrap_err().kind,
                intersect_model::definitions::ecore::ErrorKind::ResourceLimit
            );
        }
    }
}

#[test]
fn derived_declaration_overrides_both_raw_and_applied_base_projections() {
    use intersect_model::definitions::generic_types::Type;
    let text = FIXTURE.replace("</e:EPackage>", r##"<eClassifiers x:type="e:EClass" name="Raw" eSuperTypes="#//Base"/><eClassifiers x:type="e:EClass" name="Mixed" eSuperTypes="#//Raw #//Derived"/></e:EPackage>"##);
    let (resolver, id) = context(&text);
    let report = resolver
        .resolved_generic_operations(&receiver(id(&[6])))
        .unwrap();
    assert_eq!(report.operations.len(), 1);
    let group = &report.operations[0];
    assert_eq!(group.selected.member.declaration, id(&[3, 1]));
    assert_eq!(
        group.selected.member.owner,
        Type::Classifier {
            declaration: id(&[3]),
            arguments: vec![],
            raw: false
        }
    );
    assert!(group.selected_projections.is_empty());
    assert_eq!(group.overridden.len(), 2);
    let expected = [
        Type::Classifier {
            declaration: id(&[2]),
            arguments: vec![],
            raw: true,
        },
        Type::Classifier {
            declaration: id(&[2]),
            arguments: vec![Type::Classifier {
                declaration: id(&[1]),
                arguments: vec![],
                raw: false,
            }],
            raw: false,
        },
    ];
    for owner in expected {
        let overridden = group
            .overridden
            .iter()
            .find(|candidate| candidate.member.owner == owner)
            .unwrap();
        assert_eq!(overridden.member.declaration, id(&[2, 1]));
        assert_eq!(overridden.raw_parameters[0].raw_types, vec![id(&[0])]);
        assert_eq!(overridden.projection.as_ref().unwrap().source.owner, owner);
    }
}
