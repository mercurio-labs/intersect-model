use intersect_model::definitions::{
    bundle::{DefinitionBundle, DefinitionSource, SourceKind},
    generic_types::{RuntimeType, Type as GenericType},
    rcore_behavior::{
        checking::check,
        model_types::ModelTypes,
        types::{Type, TypeTable},
    },
    resources::DeclarationId,
};

type Declarations = std::collections::BTreeMap<Vec<usize>, DeclarationId>;

fn application(
    declarations: &Declarations,
    index: usize,
    arguments: Vec<GenericType>,
    raw: bool,
) -> GenericType {
    GenericType::Classifier {
        declaration: declarations[&vec![index]].clone(),
        arguments,
        raw,
    }
}
fn fixture(
    xml: &str,
    namespace: &str,
    body: &str,
) -> (DefinitionBundle, Declarations, DefinitionSource) {
    let model = DefinitionSource::new("memory:precision.ecore", SourceKind::Ecore, xml);
    let (model, declarations) =
        intersect_model_acceptance::migration_fixture::migrate_source_fixture(model);
    let mut source = DefinitionSource::new(
        "memory:precision-behavior.rcore",
        SourceKind::RcoreBehavior,
        format!(
            "rcore 1; package precision version \"1\"; import model \"{namespace}\" as m; {body}"
        ),
    );
    source.dependencies.push(model.id.clone());
    (
        DefinitionBundle {
            version: 1,
            sources: vec![model.clone(), source.clone()],
            namespaces: vec![],
        },
        declarations,
        source,
    )
}

#[test]
fn inherited_concrete_feature_retains_receiver_owner_and_exact_type() {
    let (bundle, declarations, source) = fixture(
        include_str!("../../../../fixtures/ecore-generics/rcore-behavior-handoff.ecore"),
        "urn:rcore-behavior-handoff",
        "fn inherited(x: m.Diamond) -> m.Leaf = x.value;",
    );
    let mut models = ModelTypes::new(&bundle).unwrap();
    let mut table = TypeTable::default();
    let receiver = application(&declarations, 6, vec![], false);
    let leaf = application(&declarations, 1, vec![], false);
    let owner = application(&declarations, 3, vec![leaf], false);
    let feature = models
        .feature_application(&mut table, &receiver, "value")
        .unwrap();
    assert_eq!(feature.receiver.expression(), &receiver);
    assert_eq!(feature.owner.expression(), &owner);
    assert_eq!(feature.declaration, declarations[&vec![3, 1]]);
    assert!(feature.assignment_exact);
    assert_eq!(
        table.get(feature.value).unwrap(),
        &Type::Model(declarations[&vec![1]].clone())
    );
    let report = check(&bundle, &source.id).unwrap();
    assert!(
        report.modules[0]
            .members
            .values()
            .any(|member| member.owner.expression() == &owner && member.assignment_exact)
    );
}

#[test]
fn raw_intersection_read_is_widened_without_certifying_derived_assignment() {
    let xml = include_str!("../../../../fixtures/ecore-generics/raw-feature-domains.ecore")
        .replace("name=\"value\"", "name=\"value\" derived=\"true\"")
        .replace("</e:EPackage>", "<eClassifiers x:type=\"e:EClass\" name=\"RawIntersection\" eSuperTypes=\"#//Intersection\"/></e:EPackage>");
    let (bundle, declarations, source) = fixture(
        &xml,
        "urn:raw-domains",
        "fn read(x: m.Intersection) -> m.Base? = x.value;",
    );
    let mut models = ModelTypes::new(&bundle).unwrap();
    let mut table = TypeTable::default();
    let receiver = application(&declarations, 12, vec![], true);
    let feature = models
        .feature_application(&mut table, &receiver, "value")
        .unwrap();
    assert!(!feature.assignment_exact);
    let Type::Option(element) = table.get(feature.value).unwrap() else {
        panic!("expected optional reference")
    };
    assert_eq!(
        table.get(*element).unwrap(),
        &Type::Model(declarations[&vec![0]].clone())
    );
    assert!(!feature.domains.is_empty());
    assert!(
        feature
            .domains
            .iter()
            .all(|domain| domain.source.projection.receiver.expression() == &receiver)
    );
    let report = check(&bundle, &source.id).unwrap();
    assert!(!report.model_domains.is_empty());
    assert!(
        report
            .model_domains
            .iter()
            .all(|context| !context.schema_revision.is_empty())
    );
    let (bundle, _, source) = fixture(
        &xml,
        "urn:raw-domains",
        "fn overclaim(x: m.Intersection) -> m.Both? = x.value;",
    );
    let errors = check(&bundle, &source.id).unwrap_err();
    assert!(
        errors
            .iter()
            .any(|error| error.message.contains("Type mismatch")),
        "{errors:?}"
    );
    let (bundle, _, source) = fixture(
        &xml,
        "urn:raw-domains",
        "derive m.RawIntersection.value(self) -> m.Base? = None;",
    );
    let errors = check(&bundle, &source.id).unwrap_err();
    assert!(
        errors
            .iter()
            .any(|error| error.message.contains("exact result domain")),
        "{errors:?}"
    );
}

#[test]
fn raw_attribute_preserves_projection_but_rejects_unsupported_static_domain() {
    let (bundle, declarations, source) = fixture(
        include_str!("../../../../fixtures/ecore-generics/raw-feature-domains.ecore"),
        "urn:raw-domains",
        "fn read(x: m.AttrChild) -> Bool = x.value == x.value;",
    );
    let mut models = ModelTypes::new(&bundle).unwrap();
    let receiver = application(&declarations, 9, vec![], true);
    let projection = models.feature_projection(&receiver, "value").unwrap();
    assert_eq!(projection.receiver.expression(), &receiver);
    assert_eq!(
        projection.owner.expression(),
        &application(&declarations, 8, vec![], true)
    );
    assert_eq!(projection.declaration, declarations[&vec![8, 1]]);
    assert!(!projection.domains.is_empty());
    assert!(
        projection
            .domains
            .iter()
            .any(|domain| domain.source.projection.source.owner
                != *domain.source.projection.storage_owner.expression())
    );
    assert!(
        projection
            .domains
            .iter()
            .any(|domain| RuntimeType::new(domain.source.expression.clone()).is_err())
    );
    let error = models
        .feature_application(&mut TypeTable::default(), &receiver, "value")
        .unwrap_err();
    assert!(error.contains("explicit static domain type"), "{error}");
    let errors = check(&bundle, &source.id).unwrap_err();
    assert!(
        errors
            .iter()
            .any(|error| error.message.contains("explicit static domain type")),
        "{errors:?}"
    );
}
