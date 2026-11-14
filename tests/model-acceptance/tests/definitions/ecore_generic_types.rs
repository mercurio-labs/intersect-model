use intersect_model::definitions::{
    bundle::{DefinitionBundle, DefinitionSource, SourceKind},
    generic_resolution::Resolver,
    generic_types::{ParameterBinding, Substitution, Type},
    resources::DeclarationId,
};
fn source() -> DefinitionSource {
    DefinitionSource::new(
        "memory:generics",
        SourceKind::Ecore,
        include_str!("../../../../fixtures/ecore-generics/signatures.ecore").replace("\r\n", "\n"),
    )
}
fn id(s: &DefinitionSource, path: &[usize]) -> DeclarationId {
    DeclarationId {
        source: s.id.clone(),
        path: path.to_vec(),
    }
}
fn parameter(s: &DefinitionSource, path: &[usize]) -> Type {
    Type::Parameter {
        declaration: id(s, path),
    }
}
fn classifier(s: &DefinitionSource, path: &[usize], arguments: Vec<Type>) -> Type {
    Type::Classifier {
        declaration: id(s, path),
        arguments,
        raw: false,
    }
}
fn binding(s: &DefinitionSource, path: &[usize], argument: Type) -> ParameterBinding {
    ParameterBinding {
        parameter: id(s, path),
        argument,
    }
}
#[test]
fn normalization_preserves_resolved_identity_rawness_wildcards_and_use_origin() {
    let s = source();
    let r = Resolver::new(&DefinitionBundle {
        version: 1,
        sources: vec![s.clone()],
        namespaces: vec![],
    })
    .unwrap();
    let value = r.type_expression(&id(&s, &[1, 1, 0])).unwrap();
    let input = r.type_expression(&id(&s, &[1, 2, 2, 0])).unwrap();
    assert_ne!(value.declaration, input.declaration);
    assert_ne!(value.span, input.span);
    assert_eq!(value.expression, input.expression);
    assert_eq!(value.expression, parameter(&s, &[1, 0]));
    assert_eq!(
        r.type_expression(&id(&s, &[1, 2, 1])).unwrap().expression,
        parameter(&s, &[1, 2, 0])
    );
    assert_eq!(
        r.type_expression(&id(&s, &[2, 0, 0])).unwrap().expression,
        Type::Classifier {
            declaration: id(&s, &[1]),
            arguments: vec![],
            raw: true
        }
    );
    assert_eq!(
        r.type_expression(&id(&s, &[2, 1, 0])).unwrap().expression,
        classifier(
            &s,
            &[1],
            vec![Type::Wildcard {
                upper: None,
                lower: None
            }]
        )
    );
    let bound = r.type_expression(&id(&s, &[1, 0, 0])).unwrap();
    assert_eq!(
        bound.expression,
        classifier(&s, &[0], vec![parameter(&s, &[1, 0])])
    );
    // F-bounds are not expanded as if they were substitution mappings.
    assert_eq!(
        Substitution::new(vec![])
            .unwrap()
            .apply(&value.expression)
            .unwrap(),
        value.expression
    );
    let changed = DefinitionSource::new(&s.id.uri, SourceKind::Ecore, format!("{}\n", s.text));
    assert_ne!(parameter(&s, &[1, 0]), parameter(&changed, &[1, 0]));
    assert!(r.type_expression(&id(&changed, &[1, 1, 0])).is_err());
    let wire = serde_json::to_string(&bound).unwrap();
    assert_eq!(
        serde_json::from_str::<intersect_model::definitions::generic_types::TypeUse>(&wire)
            .unwrap(),
        bound
    );
}
#[test]
fn nested_chains_shadowed_formals_and_wildcard_bounds_substitute_exact_ids() {
    let s = source();
    let concrete = classifier(&s, &[0], vec![]);
    let env = Substitution::new(vec![
        binding(&s, &[1, 0], parameter(&s, &[1, 2, 0])),
        binding(&s, &[1, 2, 0], concrete.clone()),
    ])
    .unwrap();
    let ty = classifier(
        &s,
        &[1],
        vec![
            Type::Wildcard {
                upper: Some(Box::new(parameter(&s, &[1, 0]))),
                lower: None,
            },
            Type::Wildcard {
                upper: None,
                lower: Some(Box::new(parameter(&s, &[1, 2, 0]))),
            },
            parameter(&s, &[0, 0]),
        ],
    );
    let expected = classifier(
        &s,
        &[1],
        vec![
            Type::Wildcard {
                upper: Some(Box::new(concrete.clone())),
                lower: None,
            },
            Type::Wildcard {
                upper: None,
                lower: Some(Box::new(concrete.clone())),
            },
            parameter(&s, &[0, 0]),
        ],
    );
    assert_eq!(env.apply(&ty).unwrap(), expected);
    let class_only = Substitution::new(vec![binding(&s, &[1, 0], concrete)]).unwrap();
    assert_eq!(
        class_only.apply(&parameter(&s, &[1, 2, 0])).unwrap(),
        parameter(&s, &[1, 2, 0])
    );
    let identity = Substitution::new(vec![binding(&s, &[1, 0], parameter(&s, &[1, 0]))]).unwrap();
    assert_eq!(identity.apply(&ty).unwrap(), ty);
    let mut reverse = env.entries();
    reverse.reverse();
    assert_eq!(Substitution::new(reverse).unwrap().entries(), env.entries());
}
#[test]
fn cycles_duplicates_and_impossible_shapes_are_explicit_errors() {
    let s = source();
    let t = parameter(&s, &[1, 0]);
    let u = parameter(&s, &[1, 2, 0]);
    for entries in [
        vec![
            binding(&s, &[1, 0], u.clone()),
            binding(&s, &[1, 2, 0], t.clone()),
        ],
        vec![binding(&s, &[1, 0], classifier(&s, &[1], vec![t.clone()]))],
    ] {
        assert!(
            Substitution::new(entries)
                .unwrap()
                .apply(&t)
                .unwrap_err()
                .contains("Cyclic")
        );
    }
    assert!(
        Substitution::new(vec![
            binding(&s, &[1, 0], t.clone()),
            binding(&s, &[1, 0], u)
        ])
        .unwrap_err()
        .contains("Duplicate")
    );
    for ty in [
        Type::Classifier {
            declaration: id(&s, &[1]),
            arguments: vec![t.clone()],
            raw: true,
        },
        Type::Wildcard {
            upper: Some(Box::new(t.clone())),
            lower: Some(Box::new(t.clone())),
        },
    ] {
        assert!(Substitution::new(vec![binding(&s, &[1, 0], ty.clone())]).is_err());
        assert!(Substitution::new(vec![]).unwrap().apply(&ty).is_err());
    }
    // Repeated independent occurrences are not recursive cycles.
    let env = Substitution::new(vec![binding(&s, &[1, 0], classifier(&s, &[0], vec![]))]).unwrap();
    assert!(env.apply(&classifier(&s, &[1], vec![t.clone(), t])).is_ok());
}
#[test]
fn deterministic_input_and_expansion_budgets_reject_without_partial_results() {
    let s = source();
    let leaf = parameter(&s, &[1, 0]);
    let empty = Substitution::new(vec![]).unwrap();
    let mut nested = leaf.clone();
    for _ in 0..128 {
        nested = classifier(&s, &[1], vec![nested]);
    }
    assert!(empty.apply(&nested).is_ok());
    nested = classifier(&s, &[1], vec![nested]);
    assert!(empty.apply(&nested).unwrap_err().contains("budget"));
    assert!(
        empty
            .apply(&classifier(&s, &[1], vec![leaf.clone(); 16_383]))
            .is_ok()
    );
    assert!(
        empty
            .apply(&classifier(&s, &[1], vec![leaf.clone(); 16_384]))
            .unwrap_err()
            .contains("budget")
    );
    let mut entries = vec![];
    for n in 0..20 {
        let next = parameter(&s, &[100, n + 1]);
        entries.push(binding(
            &s,
            &[100, n],
            classifier(&s, &[1], vec![next.clone(), next]),
        ));
    }
    let expanding = Substitution::new(entries).unwrap();
    assert!(
        expanding
            .apply(&parameter(&s, &[100, 0]))
            .unwrap_err()
            .contains("budget")
    );
    // An unrelated subsequent operation gets a fresh budget and recursion set.
    assert_eq!(expanding.apply(&leaf).unwrap(), leaf);
}

#[test]
fn runtime_identity_distinguishes_raw_without_relaxing_closed_identity() {
    use intersect_model::definitions::bundle::DefinitionSource;
    use intersect_model::definitions::generic_types::{ClosedType, RuntimeType, Type};
    use intersect_model::definitions::resources::DeclarationId;
    let source = DefinitionSource::new(
        "memory:runtime",
        intersect_model::definitions::bundle::SourceKind::Ecore,
        "identity",
    );
    let id = DeclarationId {
        source: source.id,
        path: vec![0],
    };
    let concrete = Type::Classifier {
        declaration: id.clone(),
        arguments: vec![],
        raw: false,
    };
    let raw = Type::Classifier {
        declaration: id.clone(),
        arguments: vec![],
        raw: true,
    };
    let closed = ClosedType::new(concrete.clone()).unwrap();
    let runtime = RuntimeType::new(concrete.clone()).unwrap();
    assert_eq!(closed.identity(), runtime.identity());
    let raw_type = RuntimeType::new(raw.clone()).unwrap();
    assert_ne!(runtime.identity(), raw_type.identity());
    assert!(ClosedType::new(raw.clone()).is_err());
    assert!(raw_type.has_raw());
    let nested = Type::Classifier {
        declaration: id.clone(),
        arguments: vec![raw.clone()],
        raw: false,
    };
    assert!(RuntimeType::new(nested.clone()).unwrap().has_raw());
    assert!(ClosedType::new(nested).is_err());
    for bad in [
        Type::Parameter {
            declaration: id.clone(),
        },
        Type::Wildcard {
            upper: None,
            lower: None,
        },
        Type::Classifier {
            declaration: id,
            arguments: vec![concrete],
            raw: true,
        },
    ] {
        assert!(RuntimeType::new(bad).is_err());
    }
    assert_eq!(
        serde_json::from_value::<RuntimeType>(serde_json::to_value(&raw_type).unwrap()).unwrap(),
        raw_type
    );
}
