#[path = "../../../fixtures/rcore-model-rule-service/read_function.rs"]
mod portable;
#[test]
fn supplemental_read_services_keep_structure_sources_and_caller_work() -> Result<(), String> {
    portable::verify()
}

#[test]
fn supplementary_sources_cannot_replace_structure_or_weaken_read_contracts() -> Result<(), String> {
    use intersect_model::{
        compile_rcore_at,
        definitions::{bundle::SourceKind, rcore_behavior::ir_v2::Limits},
        rcore::ExecutionLimits,
        source_model_functions::SourceModelFunction,
    };
    let package = compile_rcore_at("memory:standalone-model.rcore", portable::MODEL)
        .map_err(|error| error.to_string())?;
    let compile = |source: intersect_model::definitions::bundle::DefinitionSource| {
        let selection = portable::selection(&source);
        SourceModelFunction::from_rcore(
            &package,
            vec![source],
            &selection,
            &Limits::default(),
            ExecutionLimits::default(),
            Default::default(),
        )
    };
    let mut wrong_kind = portable::supplement(&package, portable::BODY);
    wrong_kind.kind = SourceKind::Rcore;
    assert!(compile(wrong_kind).is_err());
    let mut replacement = portable::supplement(&package, portable::BODY);
    replacement.id = intersect_model::definitions::bundle::SourceId::for_text(
        package.entry().uri.clone(),
        &replacement.text,
    );
    assert!(compile(replacement).is_err());
    let mut stale = portable::supplement(&package, portable::BODY);
    stale.text.push(' ');
    assert!(compile(stale).is_err());
    for body in [
        "fn inspect(self:Bool)->Bool = self;",
        "fn inspect(self:m.Node, other:m.Node)->m.Node = other;",
        "extern fn inspect(self:m.Node)->Option<String>;",
        "fn inspect(self:m.Node)->(Bool)->Bool = |value:Bool| value;",
    ] {
        let text = format!(
            "rcore 1; package bad.read version \"1\"; import model \"urn:standalone-model-reads\" as m; {body}"
        );
        assert!(
            compile(portable::supplement(&package, &text)).is_err(),
            "{body}"
        );
    }
    let source = portable::supplement(&package, portable::BODY);
    let limits = intersect_model::definitions::ecore::ParseLimits { source_bytes: 1 };
    assert!(
        SourceModelFunction::from_rcore(
            &package,
            vec![source.clone()],
            &portable::selection(&source),
            &Limits::default(),
            ExecutionLimits::default(),
            limits
        )
        .is_err()
    );
    Ok(())
}
