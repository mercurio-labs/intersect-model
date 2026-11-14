use intersect_model::definitions::{
    bundle::{DefinitionBundle, DefinitionSource, SourceKind},
    rcore_behavior::{
        SyntaxDiagnostic,
        checking::{TypeReport, check},
    },
};

fn run(body: &str) -> Result<TypeReport, Vec<SyntaxDiagnostic>> {
    let model = DefinitionSource::new(
        "memory:enum.ecore",
        SourceKind::Ecore,
        "<e:EPackage xmlns:e='http://www.eclipse.org/emf/2002/Ecore' xmlns:xsi='http://www.w3.org/2001/XMLSchema-instance' name='test' nsURI='urn:enum'><eClassifiers xsi:type='e:EEnum' name='Mode'><eLiterals name='First' value='7' literal='first'/><eLiterals name='Alias' value='7' literal='alias'/></eClassifiers><eClassifiers xsi:type='e:EEnum' name='Other'><eLiterals name='First' value='7'/></eClassifiers><eClassifiers xsi:type='e:EClass' name='Item'><eStructuralFeatures xsi:type='e:EAttribute' name='mode' eType='#//Mode' lowerBound='1'/><eStructuralFeatures xsi:type='e:EAttribute' name='optional' eType='#//Mode'/><eStructuralFeatures xsi:type='e:EAttribute' name='derivedMode' eType='#//Mode' derived='true' lowerBound='1'/></eClassifiers></e:EPackage>",
    );
    let model = DefinitionSource::new(
        model.id.uri.clone(),
        SourceKind::Ecore,
        model.text.replace(
            "</e:EPackage>",
            "<eClassifiers xsi:type='e:EEnum' name='Empty'/></e:EPackage>",
        ),
    );
    let (model, _) = intersect_model_acceptance::migration_fixture::migrate_source_fixture(model);
    let mut source = DefinitionSource::new(
        "memory:enum-behavior.rcore",
        SourceKind::RcoreBehavior,
        format!("rcore 1; package enums version \"1\"; import model \"urn:enum\" as m; {body}"),
    );
    source.dependencies.push(model.id.clone());
    check(
        &DefinitionBundle {
            version: 1,
            sources: vec![model, source.clone()],
            namespaces: vec![],
        },
        &source.id,
    )
}

#[test]
fn enum_annotations_literals_features_and_derived_signatures_share_identity() {
    for body in [
        "fn f() -> m.Mode = m.Mode.Alias;",
        "fn same(a: m.Mode, b: m.Mode) -> Bool = a == b;",
        "fn different() -> Bool = m.Mode.First != m.Mode.Alias;",
        "fn read(item: m.Item) -> m.Mode = item.mode; fn optional(item: m.Item?) -> m.Mode? = item?.mode;",
        "fn choose(item: m.Item) -> m.Mode = item.optional ?? m.Mode.First;",
        "fn choose(flag: Bool) -> m.Mode = if flag { m.Mode.First } else { m.Mode.Alias };",
        "derive m.Item.derivedMode(self) -> m.Mode = self.mode;",
        "fn capture(value: m.Mode) -> m.Mode = { let get: () -> m.Mode = || value; get() };",
    ] {
        let result = run(body);
        assert!(result.is_ok(), "{body}: {:?}", result.err());
    }
}

#[test]
fn aliases_each_require_coverage_in_nested_option_result_matches() {
    for body in [
        "fn f(x: m.Mode) -> Bool = match x { m.Mode.First => true, m.Mode.Alias => false };",
        "fn f(x: Result<m.Mode?, String>) -> Bool = match x { Ok(Some(m.Mode.First)) => true, Ok(Some(m.Mode.Alias)) => false, Ok(none) => false, Err(_) => false };",
        "fn f(x: m.Mode) -> Bool = match x { m.Mode.First => true, _ => false };",
    ] {
        let result = run(body);
        assert!(result.is_ok(), "{body}: {:?}", result.err());
    }
    for (body, expected) in [
        (
            "fn f(x: m.Mode) -> Bool = match x { m.Mode.First => true };",
            "Non-exhaustive",
        ),
        (
            "fn f(x: m.Mode) -> Bool = match x { m.Mode.First => true, m.Mode.First => false, m.Mode.Alias => false };",
            "Unreachable",
        ),
        (
            "fn f(x: m.Mode) -> Bool = match x { m.Other.First => true, _ => false };",
            "incompatible",
        ),
        (
            "fn f(x: m.Mode) -> Bool = match x { m.Mode.First(_) => true, _ => false };",
            "no arguments",
        ),
    ] {
        let errors = run(body).unwrap_err();
        assert!(
            errors.iter().any(|error| error.message.contains(expected)),
            "{body}: {errors:?}"
        );
    }
}

#[test]
fn enum_names_are_not_scalar_values_types_or_callable_constructors() {
    for body in [
        "fn f() -> m.Other = m.Mode.First;",
        "fn f() -> Int32 = m.Mode.First;",
        "fn f() -> m.Mode = 7;",
        "fn f() -> m.Mode = \"first\";",
        "fn f() -> m.Mode = m.Mode.first;",
        "fn f() -> m.Mode = m.Mode.Missing;",
        "fn f() -> m.Mode.First = m.Mode.First;",
        "fn f() -> m.Mode = m.Mode.First();",
        "fn f() -> m.Mode = m.Mode.First + m.Mode.Alias;",
        "fn f() -> Bool = m.Mode.First == m.Other.First;",
        "fn f() -> Bool = m.Mode.First < m.Mode.Alias;",
    ] {
        assert!(run(body).is_err(), "Unexpected acceptance: {body}");
    }
}

#[test]
fn empty_enum_domains_do_not_require_impossible_constructor_arms() {
    for body in [
        "fn f(x: m.Empty) -> Bool = match x {};",
        "fn f(x: m.Empty?) -> Bool = match x { none => false };",
        "fn f(x: Result<m.Empty, Bool>) -> Bool = match x { Err(true) => true, Err(false) => false };",
    ] {
        let result = run(body);
        assert!(result.is_ok(), "{body}: {:?}", result.err());
    }
    for body in [
        "fn f(x: m.Empty) -> Bool = match x { _ => true };",
        "fn f(x: m.Empty?) -> Bool = match x { Some(_) => true, none => false };",
        "fn f(x: m.Mode) -> Bool = match x {};",
    ] {
        assert!(run(body).is_err(), "{body}");
    }
}

#[test]
fn imported_enum_return_type_does_not_cross_source_revisions() {
    let xml = "<e:EPackage xmlns:e='http://www.eclipse.org/emf/2002/Ecore' xmlns:xsi='http://www.w3.org/2001/XMLSchema-instance' name='model' nsURI='urn:enum'><eClassifiers xsi:type='e:EEnum' name='Mode'><eLiterals name='One'/></eClassifiers></e:EPackage>";
    let first = DefinitionSource::new("memory:enum.ecore", SourceKind::Ecore, xml);
    let (first, _) = intersect_model_acceptance::migration_fixture::migrate_source_fixture(first);
    let second = DefinitionSource::new(
        first.id.uri.clone(),
        SourceKind::Rcore,
        format!("{}\n", first.text),
    );
    let mut helper = DefinitionSource::new(
        "memory:helper-behavior.rcore",
        SourceKind::RcoreBehavior,
        "rcore 1; package helper version \"1\"; import model \"urn:enum\" as m; pub fn value() -> m.Mode = m.Mode.One;",
    );
    helper.dependencies.push(first.id.clone());
    let mut entry = DefinitionSource::new(
        "memory:entry-behavior.rcore",
        SourceKind::RcoreBehavior,
        "rcore 1; package entry version \"1\"; import model \"urn:enum\" as m; import package helper version \"1\" as h; fn value() -> m.Mode = h.value();",
    );
    entry.dependencies = vec![second.id.clone(), helper.id.clone()];
    let bundle = DefinitionBundle {
        version: 1,
        sources: vec![first.clone(), second, helper.clone(), entry.clone()],
        namespaces: vec![],
    };
    let errors = check(&bundle, &entry.id).unwrap_err();
    assert!(
        errors
            .iter()
            .any(|error| error.message.contains("Type mismatch")),
        "{errors:?}"
    );
    entry.dependencies = vec![first.id.clone(), helper.id.clone()];
    let mut bundle = DefinitionBundle {
        version: 1,
        sources: vec![first, helper, entry.clone()],
        namespaces: vec![],
    };
    check(&bundle, &entry.id).unwrap();
    bundle.sources.reverse();
    check(&bundle, &entry.id).unwrap();
}
