//! Provider equivalence proves constructor bounds, never a model value view.
//! Box<Alias>.value carries Alias's tag: accepting it as Box<RGB> would allow
//! reading RGB without conversion. Explicit type arguments cannot repair that.
use intersect_model::conversion::with_catalog_policies;
use intersect_model::definitions::{
    bundle::{DefinitionBundle, DefinitionSource, SourceKind},
    conversion::{ConversionContract, ProviderId},
    generic_types::Type as GenericType,
    rcore_behavior::{
        ast::Declaration,
        checking::{check, check_with_conversions},
        types::Type,
    },
};
#[test]
fn provider_bound_proof_preserves_exact_model_value_arguments() {
    let model = DefinitionSource::new(
        "memory:symbolic-alias.ecore",
        SourceKind::Ecore,
        include_str!("../../../../fixtures/ecore-generics/rcore-behavior-provider-handoff.ecore"),
    );
    let (model, declarations) =
        intersect_model_acceptance::migration_fixture::migrate_source_fixture(model);
    let contracts: Vec<_> = (0..2)
        .map(|index| ConversionContract {
            datatype: declarations[&vec![index]].clone(),
            provider: ProviderId {
                name: "example.rgb".into(),
                version: "1".into(),
            },
            serializable: true,
            declarations: vec![],
            members: vec![],
        })
        .collect();
    for (body, valid) in [
        (
            "fn carry<T:m.Alias>(x:m.Box<T>)->m.Box<T> =x; fn caller(x:m.Box<m.Alias>)->m.Box<m.Alias> =carry(x);",
            true,
        ),
        (
            "fn carry<T:m.Alias>(x:m.Box<T>)->m.Box<T> =x; fn caller(x:m.Box<m.Alias>)->m.Box<m.Alias> =carry::<m.Alias>(x);",
            true,
        ),
        (
            "fn carry<T:m.Alias>(x:m.Box<T>)->m.Box<T> =x; fn caller(x:m.Box<m.Alias>)->m.Box<m.RGB> =carry(x);",
            false,
        ),
        (
            "fn read(x:m.Box<m.RGB>)->m.RGB =x.value; fn wrong(x:m.Box<m.Alias>)->m.RGB =read(x);",
            false,
        ),
        ("fn bad(x:m.Alias)->m.RGB =x;", false),
        (
            "fn up<T:m.RGB>(x:T)->m.RGB =x; fn bad(x:m.Alias)->m.RGB =up(x);",
            false,
        ),
    ] {
        let mut entry = DefinitionSource::new(
            "memory:symbolic-alias-behavior.rcore",
            SourceKind::RcoreBehavior,
            format!(
                "rcore 1; package alias_policy version \"1\"; import model \"urn:rcore-behavior-provider-handoff\" as m; {body}"
            ),
        );
        entry.dependencies.push(model.id.clone());
        let bundle = DefinitionBundle {
            version: 1,
            sources: vec![model.clone(), entry.clone()],
            namespaces: vec![],
        };
        assert!(check(&bundle, &entry.id).is_err());
        let result = with_catalog_policies(&bundle, &contracts, &[], &[], |registry| {
            Ok(check_with_conversions(&bundle, &entry.id, registry))
        })
        .unwrap();
        if valid {
            let report = result.unwrap();
            let document = &report.signatures.bound.index.modules[0].document;
            let caller = &report.signatures.functions[1];
            let Declaration::Function { body, .. } =
                &document.declarations[caller.symbol.declaration].value
            else {
                panic!("Expected caller function")
            };
            let value = report.modules[0].expressions[&body.0];
            let Type::ModelApplication(GenericType::Classifier {
                declaration,
                arguments,
                ..
            }) = report.signatures.types.get(value).unwrap()
            else {
                panic!("Expected concrete call application")
            };
            assert_eq!(*declaration, declarations[&vec![2]]);
            let GenericType::Classifier {
                declaration: argument,
                ..
            } = &arguments[0]
            else {
                panic!("Expected nominal argument")
            };
            assert_eq!(*argument, declarations[&vec![1]]);
            assert_eq!(argument.source, model.id);
        } else {
            let errors = result.unwrap_err();
            assert!(!errors.is_empty());
            assert!(
                errors
                    .iter()
                    .all(|e| e.span.source == entry.id && e.span.start < e.span.end),
                "{body}: {errors:?}"
            );
        }
    }
}

#[test]
fn equivalent_provider_domains_do_not_choose_an_arbitrary_inferred_nominal_argument() {
    let model = DefinitionSource::new(
        "memory:symbolic-alias.ecore",
        SourceKind::Ecore,
        include_str!("../../../../fixtures/ecore-generics/rcore-behavior-provider-handoff.ecore"),
    );
    let (model, declarations) =
        intersect_model_acceptance::migration_fixture::migrate_source_fixture(model);
    let contracts: Vec<_> = (0..2)
        .map(|index| ConversionContract {
            datatype: declarations[&vec![index]].clone(),
            provider: ProviderId {
                name: "example.rgb".into(),
                version: "1".into(),
            },
            serializable: true,
            declarations: vec![],
            members: vec![],
        })
        .collect();
    for arguments in ["alias,rgb", "rgb,alias", "alias,alias"] {
        for explicit in [false, true] {
            let call = if explicit { "pair::<m.Alias>" } else { "pair" };
            let mut entry = DefinitionSource::new(
                "memory:alias-ambiguity-behavior.rcore",
                SourceKind::RcoreBehavior,
                format!(
                    "rcore 1; package alias_ambiguity version \"1\"; import model \"urn:rcore-behavior-provider-handoff\" as m; fn pair<T:m.Alias>(a:m.Box<T>,b:m.Box<T>)->m.Box<T> =a; fn caller(alias:m.Box<m.Alias>,rgb:m.Box<m.RGB>)->m.Box<m.Alias> ={{let inferred={call}({arguments}); inferred}};"
                ),
            );
            entry.dependencies.push(model.id.clone());
            let bundle = DefinitionBundle {
                version: 1,
                sources: vec![model.clone(), entry.clone()],
                namespaces: vec![],
            };
            let result = with_catalog_policies(&bundle, &contracts, &[], &[], |registry| {
                Ok(check_with_conversions(&bundle, &entry.id, registry))
            })
            .unwrap();
            if arguments == "alias,alias" {
                assert!(result.is_ok(), "{arguments}: {result:?}");
            } else {
                let errors = result.unwrap_err();
                assert!(!errors.is_empty(), "{arguments}: {errors:?}");
                assert!(
                    errors
                        .iter()
                        .all(|e| e.span.source == entry.id && e.span.start < e.span.end),
                    "{errors:?}"
                );
            }
        }
    }
}

#[test]
fn a_still_symbolic_application_cannot_retag_a_different_concrete_argument() {
    let original =
        include_str!("../../../../fixtures/ecore-generics/rcore-behavior-provider-handoff.ecore");
    let pair = r##"<eClassifiers x:type="e:EClass" name="Pair"><eTypeParameters name="A"><eBounds eClassifier="#//RGB"/></eTypeParameters><eTypeParameters name="B"><eBounds eClassifier="#//RGB"/></eTypeParameters><eStructuralFeatures x:type="e:EAttribute" name="second" lowerBound="1"><eGenericType eTypeParameter="#//Pair/B"/></eStructuralFeatures></eClassifiers>"##;
    let model = DefinitionSource::new(
        "memory:nested-alias.ecore",
        SourceKind::Ecore,
        original.replace("</e:EPackage>", &format!("{pair}</e:EPackage>")),
    );
    let (model, declarations) =
        intersect_model_acceptance::migration_fixture::migrate_source_fixture(model);
    let contracts: Vec<_> = (0..2)
        .map(|index| ConversionContract {
            datatype: declarations[&vec![index]].clone(),
            provider: ProviderId {
                name: "example.rgb".into(),
                version: "1".into(),
            },
            serializable: true,
            declarations: vec![],
            members: vec![],
        })
        .collect();
    for (body, valid) in [
        (
            "fn preserve<T:m.Alias>(x:m.Pair<T,m.Alias>)->m.Pair<T,m.Alias> =x; fn read<T:m.Alias>(x:m.Pair<T,m.Alias>)->m.Alias =x.second;",
            true,
        ),
        (
            "fn retag<T:m.Alias>(x:m.Pair<T,m.Alias>)->m.Pair<T,m.RGB> =x;",
            false,
        ),
        (
            "fn retag<T:m.Alias>(x:m.Pair<T,m.RGB>)->m.Pair<T,m.Alias> =x;",
            false,
        ),
        (
            "fn wrong<T:m.Alias>(x:m.Pair<T,m.Alias>)->m.RGB =x.second;",
            false,
        ),
    ] {
        let mut entry = DefinitionSource::new(
            "memory:nested-alias-behavior.rcore",
            SourceKind::RcoreBehavior,
            format!(
                "rcore 1; package nested_alias version \"1\"; import model \"urn:rcore-behavior-provider-handoff\" as m; {body}"
            ),
        );
        entry.dependencies.push(model.id.clone());
        let bundle = DefinitionBundle {
            version: 1,
            sources: vec![model.clone(), entry.clone()],
            namespaces: vec![],
        };
        let result = with_catalog_policies(&bundle, &contracts, &[], &[], |registry| {
            Ok(check_with_conversions(&bundle, &entry.id, registry))
        })
        .unwrap();
        if valid {
            assert!(result.is_ok(), "{body}: {result:?}");
        } else {
            let errors = result.unwrap_err();
            assert!(!errors.is_empty());
            assert!(
                errors
                    .iter()
                    .all(|e| e.span.source == entry.id && e.span.start < e.span.end),
                "{body}: {errors:?}"
            );
        }
    }
}
