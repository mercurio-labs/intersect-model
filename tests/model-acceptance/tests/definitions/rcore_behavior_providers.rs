use intersect_model::definitions::{
    bundle::{DefinitionBundle, DefinitionSource, SourceKind},
    rcore_behavior::{checking::check, types::Type},
};

fn input(body: &str) -> (DefinitionBundle, DefinitionSource) {
    let model = DefinitionSource::new(
        "memory:model.ecore",
        SourceKind::Ecore,
        r##"<e:EPackage xmlns:e="http://www.eclipse.org/emf/2002/Ecore" xmlns:x="http://www.w3.org/2001/XMLSchema-instance" name="test" nsURI="urn:test">
<eClassifiers x:type="e:EClass" name="Base">
 <eStructuralFeatures x:type="e:EAttribute" name="name" lowerBound="1" eType="e:EDataType http://www.eclipse.org/emf/2002/Ecore#//EString"/>
 <eStructuralFeatures x:type="e:EAttribute" name="count" lowerBound="1" derived="true" eType="e:EDataType http://www.eclipse.org/emf/2002/Ecore#//EInt"/>
 <eStructuralFeatures x:type="e:EAttribute" name="optional" derived="true" eType="e:EDataType http://www.eclipse.org/emf/2002/Ecore#//EInt"/>
 <eStructuralFeatures x:type="e:EAttribute" name="many" upperBound="-1" derived="true" eType="e:EDataType http://www.eclipse.org/emf/2002/Ecore#//EInt"/>
</eClassifiers><eClassifiers x:type="e:EClass" name="Child" eSuperTypes="#//Base"/>
</e:EPackage>"##,
    );
    let (model, _) = intersect_model_acceptance::migration_fixture::migrate_source_fixture(model);
    let mut source = DefinitionSource::new(
        "memory:provider",
        SourceKind::RcoreBehavior,
        format!("rcore 1; package provider version \"1\"; import model \"urn:test\" as m; {body}"),
    );
    source.dependencies.push(model.id.clone());
    (
        DefinitionBundle {
            version: 1,
            sources: vec![model, source.clone()],
            namespaces: vec![],
        },
        source,
    )
}

#[test]
fn inherited_provider_receiver_and_feature_owner_retain_distinct_identities() {
    let (bundle, entry) = input(
        "derive m.Child.count(self) -> Int32 = if self.name == \"x\" { 1 } else { 0 }; derive m.Child.optional(self) -> Int32? = Some(1); validate m.Child.named(self) { require self.name != \"\" code \"name\" message self.name; }",
    );
    let report = check(&bundle, &entry.id).unwrap();
    let module = report
        .modules
        .iter()
        .find(|module| module.source == entry.id)
        .unwrap();
    assert_eq!(module.providers.len(), 3);
    for provider in module.providers.values() {
        let Type::Model(receiver) = report.signatures.types.get(provider.receiver).unwrap() else {
            panic!("model receiver")
        };
        assert_eq!(receiver.source, bundle.sources[0].id);
        assert_eq!(receiver.path, vec![1]);
        if let Some(feature) = &provider.feature {
            assert_eq!(feature.declaration.source, receiver.source);
            assert_eq!(feature.declaration.path[0], 0);
            assert!(feature.derived);
        }
    }
}

#[test]
fn provider_signatures_reject_wrong_result_multiplicity_and_stored_targets() {
    for (body, expected) in [
        (
            "derive m.Child.count(self) -> Int = 1;",
            "match feature type",
        ),
        (
            "derive m.Child.optional(self) -> Int32 = 1;",
            "multiplicity",
        ),
        (
            "derive m.Child.count(self) -> Int32? = Some(1);",
            "multiplicity",
        ),
        ("derive m.Child.many(self) -> Int32 = 1;", "multiplicity"),
        (
            "derive m.Child.name(self) -> String = \"name\";",
            "requires a derived",
        ),
        (
            "derive m.Child.count(self) -> Int32 = true;",
            "Type mismatch",
        ),
        (
            "derive m.Child.count(self) -> Int32 = 2147483648;",
            "outside Int32",
        ),
        (
            "validate m.Child.rule(self) { require 1 code \"bad\" message \"text\"; }",
            "Type mismatch",
        ),
        (
            "validate m.Child.rule(self) { require true code \"bad\" message 1; }",
            "Type mismatch",
        ),
    ] {
        let (bundle, entry) = input(body);
        let errors = check(&bundle, &entry.id).unwrap_err();
        assert!(
            errors.iter().any(|e| e.message.contains(expected)),
            "{body}: {errors:?}"
        );
        assert!(
            errors
                .iter()
                .all(|e| e.span.source == entry.id && e.span.start < e.span.end)
        );
    }
}

#[test]
fn validation_checks_each_requirement_and_derived_reads_do_not_claim_availability() {
    let (bundle, entry) = input(
        "validate m.Child.rule(self) { require 1 code \"condition\" message \"ok\"; require true code \"message\" message 2; }",
    );
    let errors = check(&bundle, &entry.id).unwrap_err();
    assert_eq!(
        errors
            .iter()
            .filter(|e| e.message.contains("Type mismatch"))
            .count(),
        2
    );
    let (bundle, entry) = input("fn read(self: m.Child) -> Int32 = self.count;");
    let report = check(&bundle, &entry.id).unwrap();
    assert!(report.modules[0].providers.is_empty());
    assert!(
        report.modules[0]
            .members
            .values()
            .all(|feature| feature.derived)
    );
    // A type report records a legal read; it does not invent a provider or value.
}
