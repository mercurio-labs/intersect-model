use intersect_model::definitions::{
    bundle::{DefinitionBundle, DefinitionSource, SourceKind},
    generic_types::{ClosedType, RuntimeType, Type as GenericType},
    rcore_behavior::{
        checking::check,
        model_types::ModelTypes,
        types::{Type, TypeTable},
    },
    resources::DeclarationId,
};

fn fixture(body: &str) -> (DefinitionBundle, DefinitionSource, DefinitionSource) {
    let model = DefinitionSource::new(
        "memory:handoff.ecore",
        SourceKind::Ecore,
        include_str!("../../../../fixtures/ecore-generics/rcore-behavior-handoff.ecore"),
    );
    let (model, _) = intersect_model_acceptance::migration_fixture::migrate_source_fixture(model);
    let mut source = DefinitionSource::new(
        "memory:handoff-behavior.rcore",
        SourceKind::RcoreBehavior,
        format!(
            "rcore 1; package handoff version \"1\"; import model \"urn:rcore-behavior-handoff\" as m; {body}"
        ),
    );
    source.dependencies.push(model.id.clone());
    (
        DefinitionBundle {
            version: 1,
            sources: vec![model.clone(), source.clone()],
            namespaces: vec![],
        },
        model,
        source,
    )
}
fn application(
    model: &DefinitionSource,
    index: usize,
    arguments: Vec<GenericType>,
    raw: bool,
) -> GenericType {
    GenericType::Classifier {
        declaration: DeclarationId {
            source: model.id.clone(),
            path: vec![index],
        },
        arguments,
        raw,
    }
}
#[test]
fn source_annotations_reads_assignments_and_raw_models_share_runtime_types() {
    let body = "fn unbounded(x: m.Open) -> Bool = x.value == x.value; fn read(x: m.Box<m.Leaf>) -> m.Leaf = x.value; fn nested(x: m.Box<m.Box<m.Leaf>>) -> m.Box<m.Leaf> = x.value; fn inherited(x: m.Diamond) -> m.Leaf = x.value; fn optional(x: m.Box<m.Leaf>?) -> m.Leaf? = x?.optional; fn raw(x: m.Box) -> m.Base = x.value; fn projected(x: m.Left) -> m.Box<m.Leaf> = x; fn joined(flag: Bool, a: m.Left, b: m.Right) -> m.Box<m.Leaf> = if flag { a } else { b };";
    let (bundle, model, source) = fixture(body);
    let report = check(&bundle, &source.id).unwrap();
    let leaf = application(&model, 1, vec![], false);
    let boxed = application(&model, 3, vec![leaf], false);
    let closed = ClosedType::new(boxed.clone()).unwrap();
    assert_eq!(RuntimeType::from(&closed).expression(), &boxed);
    let mut types = ModelTypes::new(&bundle).unwrap();
    let mut table = TypeTable::default();
    let feature = types
        .feature_application(&mut table, &boxed, "items")
        .unwrap();
    assert_eq!(feature.declaration.path, vec![3, 2]);
    assert!(matches!(
        table.get(feature.value).unwrap(),
        Type::Many {
            ordered: false,
            unique: false,
            upper: -1,
            ..
        }
    ));
    assert!(
        report.modules[0]
            .members
            .values()
            .any(|feature| feature.declaration.path == vec![3, 0])
    );
    assert!(
        report
            .generic_models
            .iter()
            .any(|projection| !projection.warnings.is_empty())
    );
    assert!(
        report
            .generic_models
            .iter()
            .all(|projection| projection.declaration_span.source == model.id)
    );
    assert!(
        serde_json::to_value(report)
            .unwrap()
            .to_string()
            .contains("ModelApplication")
    );
    let open = application(&model, 7, vec![], true);
    let open_value = types
        .feature_application(&mut table, &open, "value")
        .unwrap();
    let top_id = intersect_model::definitions::schema::eobject_reference_top_declaration().unwrap();
    assert_eq!(
        table.get(open_value.value).unwrap(),
        &Type::Model(top_id.clone())
    );
    let top = GenericType::Classifier {
        declaration: top_id,
        arguments: vec![],
        raw: false,
    };
    assert!(types.conforms_application(&boxed, &top).unwrap());
    assert!(!types.conforms_application(&top, &boxed).unwrap());
    assert!(types.feature_application(&mut table, &top, "name").is_err());
}
#[test]
fn generic_annotation_failures_keep_exact_source_spans_and_invariance() {
    for body in [
        "fn wrong(x: m.Box<m.Leaf>) -> m.Box<m.Base> = x;",
        "fn wrong(x: m.Box<m.Other>) -> m.Other = x.value;",
        "fn wrong(x: m.Box<m.Leaf, m.Base>) -> m.Leaf = x.value;",
        "fn wrong(x: m.Box<m.Leaf>?) -> m.Leaf = x.value;",
        "fn wrong(x: m.Box<m.Leaf>) -> m.Base = x.optional;",
        "fn wrong(x: m.Base<m.Leaf>) -> m.Base = x;",
    ] {
        let (bundle, _, source) = fixture(body);
        let errors = check(&bundle, &source.id).unwrap_err();
        assert!(!errors.is_empty(), "{body}");
        assert!(
            errors
                .iter()
                .all(|error| error.span.source == source.id && error.span.start < error.span.end),
            "{body}: {errors:?}"
        );
    }
    let (bundle, model, _) = fixture("");
    let mut expression = application(
        &model,
        3,
        vec![application(&model, 1, vec![], false)],
        false,
    );
    if let GenericType::Classifier { declaration, .. } = &mut expression {
        declaration.source.sha256 = "0".repeat(64);
    }
    assert!(
        ModelTypes::new(&bundle)
            .unwrap()
            .conforms_application(&expression, &expression)
            .is_err()
    );
}
