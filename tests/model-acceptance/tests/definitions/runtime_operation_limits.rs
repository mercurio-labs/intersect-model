use intersect_model::definitions::{
    bundle::{DefinitionBundle, DefinitionSource, SourceKind},
    ecore::ErrorKind,
    generic_resolution::Resolver,
    generic_types::{RuntimeType, Type},
    resources::DeclarationId,
};

// Raw parents come first: their equivalence removes later applied entries only
// from the superclass-consistency view, not from the retained owner projections.
fn fixture(
    first: usize,
    second: usize,
    singletons: usize,
    reverse_sources: bool,
    methods: usize,
) -> (Resolver, RuntimeType) {
    assert!(first >= second && second > 0);
    let mut text = String::from(
        r#"<e:EPackage xmlns:e="http://www.eclipse.org/emf/2002/Ecore" xmlns:x="http://www.w3.org/2001/XMLSchema-instance" name="limits" nsURI="urn:runtime-operation-limits"><eClassifiers x:type="e:EClass" name="F"><eTypeParameters name="T"/><eOperations name="f"/></eClassifiers><eClassifiers x:type="e:EClass" name="G"><eTypeParameters name="T"/><eOperations name="g"/></eClassifiers>"#,
    );
    for name in ["f", "g"] {
        let operations = (0..methods)
            .map(|i| format!(r#"<eOperations name="{name}{i}"/>"#))
            .collect::<String>();
        text = text.replace(&format!(r#"<eOperations name="{name}"/>"#), &operations);
    }
    for i in 1..first {
        text.push_str(&format!(r#"<eClassifiers x:type="e:EClass" name="V{i}"/>"#));
    }
    for i in 1..first {
        text.push_str(&format!(r##"<eClassifiers x:type="e:EClass" name="B{i}"><eGenericSuperTypes eClassifier="#//F"><eTypeArguments eClassifier="#//V{i}"/></eGenericSuperTypes>"##));
        if i < second {
            text.push_str(&format!(r##"<eGenericSuperTypes eClassifier="#//G"><eTypeArguments eClassifier="#//V{i}"/></eGenericSuperTypes>"##));
        }
        text.push_str("</eClassifiers>");
    }
    let parents = (1..first).map(|i| format!(" #//B{i}")).collect::<String>();
    text.push_str(&format!(
        r##"<eClassifiers x:type="e:EClass" name="Root" eSuperTypes="#//F #//G{parents}">"##
    ));
    for i in 0..singletons {
        text.push_str(&format!(r#"<eOperations name="unique{i}"/>"#));
    }
    text.push_str("</eClassifiers></e:EPackage>");
    let source = DefinitionSource::new("memory:runtime-operation-limits", SourceKind::Ecore, text);
    let receiver = RuntimeType::new(Type::Classifier {
        declaration: DeclarationId {
            source: source.id.clone(),
            path: vec![2 * first],
        },
        arguments: vec![],
        raw: false,
    })
    .unwrap();
    let unrelated = DefinitionSource::new(
        "memory:unrelated-limits",
        SourceKind::Ecore,
        r#"<e:EPackage xmlns:e="http://www.eclipse.org/emf/2002/Ecore" name="other" nsURI="urn:other-limits"/>"#,
    );
    let mut sources = vec![source, unrelated];
    if reverse_sources {
        sources.reverse();
    }
    (
        Resolver::new(&DefinitionBundle {
            version: 1,
            sources,
            namespaces: vec![],
        })
        .unwrap(),
        receiver,
    )
}

fn assert_groups(
    resolver: &Resolver,
    receiver: &RuntimeType,
    first: usize,
    second: usize,
    singletons: usize,
    methods: usize,
) {
    let report = resolver.resolved_runtime_operations(receiver).unwrap();
    assert_eq!(report.operations.len(), singletons + 2 * methods);
    for (prefix, count) in [("f", first), ("g", second)] {
        for i in 0..methods {
            let name = format!("{prefix}{i}");
            let group = report
                .operations
                .iter()
                .find(|g| g.selected.member.name == name)
                .unwrap();
            assert_eq!(group.selected_projections.len() + 1, count);
            assert!(group.overridden.is_empty());
            let owners: std::collections::BTreeSet<_> = std::iter::once(&group.selected)
                .chain(&group.selected_projections)
                .map(|p| {
                    assert_eq!(p.member.declaration, group.selected.member.declaration);
                    p.member.owner.clone()
                })
                .collect();
            assert_eq!(owners.len(), count);
        }
    }
}

#[test]
fn candidate_limit_counts_distinct_projections_of_the_same_declaration() {
    for reverse in [false, true] {
        let (resolver, receiver) = fixture(8, 8, 0, reverse, 8);
        assert_groups(&resolver, &receiver, 8, 8, 0, 8);
        let (over, over_receiver) = fixture(8, 8, 1, reverse, 8);
        let error = over
            .resolved_runtime_operations(&over_receiver)
            .unwrap_err();
        assert_eq!(error.kind, ErrorKind::ResourceLimit);
        assert!(
            error.message.contains("candidate budget exceeded (128)"),
            "{error:?}"
        );
        assert_groups(&resolver, &receiver, 8, 8, 0, 8);
        assert_recovery(&over, &over_receiver, 8);
    }
}

#[test]
fn inheritance_guard_preempts_dense_selection_comparison_graph_and_recovers() {
    for reverse in [false, true] {
        // Hypothetical selection cost: C(115,2)+99^2+4^2+12=16384.
        // Public selection validates inheritance first. Its independent budget
        // exhausts before selection; this is not selection-boundary evidence.
        let (over, receiver) = fixture(99, 4, 12, reverse, 1);
        let error = over.resolved_runtime_operations(&receiver).unwrap_err();
        assert_eq!(error.kind, ErrorKind::ResourceLimit);
        assert_eq!(
            error.message,
            "Generic inheritance traversal budget exceeded"
        );
        assert_recovery(&over, &receiver, 1);
        let (small, receiver) = fixture(8, 4, 12, reverse, 1);
        assert_groups(&small, &receiver, 8, 4, 12, 1);
    }
}
fn assert_recovery(resolver: &Resolver, receiver: &RuntimeType, methods: usize) {
    let Type::Classifier { declaration, .. } = receiver.expression() else {
        panic!("classifier fixture");
    };
    let base = RuntimeType::new(Type::Classifier {
        declaration: DeclarationId {
            source: declaration.source.clone(),
            path: vec![0],
        },
        arguments: vec![],
        raw: true,
    })
    .unwrap();
    let recovered = resolver.resolved_runtime_operations(&base).unwrap();
    assert_eq!(recovered.operations.len(), methods);
    assert_eq!(recovered.operations[0].selected.member.name, "f0");
}

// The grid shares nominal bridge declarations, reducing consistency-validation
// work while retaining all distinct F<T,U> source-owner applications.
fn grid(singletons: usize, reverse: bool) -> (Resolver, RuntimeType) {
    let mut text = String::from(
        r#"<e:EPackage xmlns:e="http://www.eclipse.org/emf/2002/Ecore" xmlns:x="http://www.w3.org/2001/XMLSchema-instance" name="grid" nsURI="urn:operation-grid"><eClassifiers x:type="e:EClass" name="F"><eTypeParameters name="T"/><eTypeParameters name="U"/><eOperations name="f0"/></eClassifiers><eClassifiers x:type="e:EClass" name="G"><eTypeParameters name="T"/><eOperations name="g0"/></eClassifiers>"#,
    );
    for i in 0..10 {
        text.push_str(&format!(r#"<eClassifiers x:type="e:EClass" name="V{i}"/>"#));
    }
    for j in 0..9 {
        text.push_str(&format!(r##"<eClassifiers x:type="e:EClass" name="B{j}"><eTypeParameters name="T"/><eGenericSuperTypes eClassifier="#//F"><eTypeArguments eTypeParameter="#//B{j}/T"/><eTypeArguments eClassifier="#//V{j}"/></eGenericSuperTypes></eClassifiers>"##));
    }
    for i in 0..10 {
        text.push_str(&format!(r##"<eClassifiers x:type="e:EClass" name="A{i}"><eGenericSuperTypes eClassifier="#//F"/>"##));
        for j in 0..if i == 9 { 8 } else { 9 } {
            text.push_str(&format!(r##"<eGenericSuperTypes eClassifier="#//B{j}"><eTypeArguments eClassifier="#//V{i}"/></eGenericSuperTypes>"##));
        }
        if i < 3 {
            text.push_str(&format!(r##"<eGenericSuperTypes eClassifier="#//G"><eTypeArguments eClassifier="#//V{i}"/></eGenericSuperTypes>"##));
        }
        text.push_str("</eClassifiers>");
    }
    let parents = (0..9)
        .map(|j| format!(" #//B{j}"))
        .chain((0..10).map(|i| format!(" #//A{i}")))
        .collect::<String>();
    text.push_str(&format!(
        r##"<eClassifiers x:type="e:EClass" name="Root" eSuperTypes="#//F #//G{parents}">"##
    ));
    for i in 0..singletons {
        text.push_str(&format!(r#"<eOperations name="unique{i}"/>"#));
    }
    text.push_str("</eClassifiers></e:EPackage>");
    let source = DefinitionSource::new("memory:operation-grid", SourceKind::Ecore, text);
    let receiver = RuntimeType::new(Type::Classifier {
        declaration: DeclarationId {
            source: source.id.clone(),
            path: vec![31],
        },
        arguments: vec![],
        raw: false,
    })
    .unwrap();
    let unrelated = DefinitionSource::new(
        "memory:grid-unrelated",
        SourceKind::Ecore,
        r#"<e:EPackage xmlns:e="http://www.eclipse.org/emf/2002/Ecore" name="other" nsURI="urn:grid-other"/>"#,
    );
    let mut sources = vec![source, unrelated];
    if reverse {
        sources.reverse();
    }
    (
        Resolver::new(&DefinitionBundle {
            version: 1,
            sources,
            namespaces: vec![],
        })
        .unwrap(),
        receiver,
    )
}

#[test]
fn compressed_projection_grid_reaches_exact_selection_comparison_budget() {
    for reverse in [false, true] {
        // 99 F owners, 4 G owners, 12 singleton declarations: 115 candidates.
        // C(115,2) + 99^2 + 4^2 + 12 = 16384 selection steps.
        let (resolver, receiver) = grid(12, reverse);
        assert_groups(&resolver, &receiver, 99, 4, 12, 1);
        let (over, receiver) = grid(13, reverse);
        let error = over.resolved_runtime_operations(&receiver).unwrap_err();
        assert_eq!(error.kind, ErrorKind::ResourceLimit);
        assert!(
            error
                .message
                .contains("selection comparison budget exceeded (16384)"),
            "{error:?}"
        );
        assert_recovery(&over, &receiver, 1);
    }
}
