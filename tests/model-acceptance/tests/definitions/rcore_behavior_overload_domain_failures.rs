use intersect_model::definitions::{
    bundle::{DefinitionBundle, DefinitionSource, SourceKind},
    rcore_behavior::checking::check,
};

#[test]
fn constrained_model_read_is_not_discarded_as_overload_inapplicability() {
    let model = DefinitionSource::new(
        "memory:overload-domains.ecore",
        SourceKind::Ecore,
        include_str!("../../../../fixtures/ecore-generics/raw-feature-domains.ecore"),
    );
    let (model, _) = intersect_model_acceptance::migration_fixture::migrate_source_fixture(model);
    let expected = "Rcore constrained attribute domain requires an explicit static domain type; checked source projection is available";
    for candidates in [
        "fn pick(f:(m.AttrChild)->Bool)->Int8=1;fn pick(f:({value:Int8})->Bool)->Int8=2;",
        "fn pick(f:({value:Int8})->Bool)->Int8=2;fn pick(f:(m.AttrChild)->Bool)->Int8=1;",
    ] {
        let mut source = DefinitionSource::new(
            "memory:overload-domains-behavior.rcore",
            SourceKind::RcoreBehavior,
            format!(
                "rcore 1;package domains version \"1\";import model \"urn:raw-domains\" as m;{candidates}fn main()->Int8=pick(|x:m.AttrChild|x.value==x.value);"
            ),
        );
        source.dependencies.push(model.id.clone());
        let bundle = DefinitionBundle {
            version: 1,
            sources: vec![model.clone(), source.clone()],
            namespaces: vec![],
        };
        let errors = check(&bundle, &source.id).unwrap_err();
        assert!(
            errors
                .iter()
                .any(|error| error.message == expected && error.span.source == source.id),
            "{errors:?}"
        );
        assert!(
            !errors
                .iter()
                .any(|error| error.message == "No applicable overload"),
            "{errors:?}"
        );
    }
}
