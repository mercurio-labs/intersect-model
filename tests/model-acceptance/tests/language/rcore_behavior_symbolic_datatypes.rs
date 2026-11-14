//! Installed datatype context for symbolic source typing; no symbolic value codec.
use intersect_model::conversion::with_catalog_policies;
use intersect_model::definitions::{
    bundle::{DefinitionBundle, DefinitionSource, SourceKind},
    conversion::{
        AppliedConversionContract, ConversionContract, MemberContract, MemberType, ProviderId,
    },
    generic_types::{ClosedType, Type},
    rcore_behavior::{
        checking::{check, check_with_conversions},
        types::Type as SemType,
    },
};
const MODEL: &str = r##"<e:EPackage xmlns:e="http://www.eclipse.org/emf/2002/Ecore" xmlns:x="http://www.w3.org/2001/XMLSchema-instance" name="datatypes" nsURI="urn:symbolic-datatypes"><eClassifiers x:type="e:EDataType" name="Text" instanceClassName="java.lang.String"/><eClassifiers x:type="e:EDataType" name="Sequence" instanceClassName="intersect.Sequence"><eTypeParameters name="T"><eBounds eClassifier="#//Text"/></eTypeParameters></eClassifiers><eClassifiers x:type="e:EClass" name="Holder"><eTypeParameters name="T"><eBounds eClassifier="#//Text"/></eTypeParameters><eStructuralFeatures x:type="e:EAttribute" name="values" lowerBound="1"><eGenericType eClassifier="#//Sequence"><eTypeArguments eTypeParameter="#//Holder/T"/></eGenericType></eStructuralFeatures></eClassifiers></e:EPackage>"##;
fn fixture(
    body: &str,
) -> (
    DefinitionBundle,
    DefinitionSource,
    AppliedConversionContract,
) {
    let model = DefinitionSource::new("memory:symbolic-datatypes.ecore", SourceKind::Ecore, MODEL);
    let (model, declarations) =
        intersect_model_acceptance::migration_fixture::migrate_source_fixture(model);
    let id = |index| declarations[&vec![index]].clone();
    let argument = Type::Classifier {
        declaration: id(0),
        arguments: vec![],
        raw: false,
    };
    let contract = AppliedConversionContract {
        application: ClosedType::new(Type::Classifier {
            declaration: id(1),
            arguments: vec![argument],
            raw: false,
        })
        .unwrap(),
        conversion: ConversionContract {
            datatype: id(1),
            provider: ProviderId {
                name: "intersect.sequence".into(),
                version: "1".into(),
            },
            serializable: true,
            declarations: vec![],
            members: vec![MemberContract {
                nullable: false,
                value: MemberType::Datatype { declaration: id(0) },
            }],
        },
    };
    let mut entry = DefinitionSource::new(
        "memory:symbolic-datatypes-behavior.rcore",
        SourceKind::RcoreBehavior,
        format!(
            "rcore 1; package datatypes version \"1\"; import model \"urn:symbolic-datatypes\" as m; {body}"
        ),
    );
    entry.dependencies.push(model.id.clone());
    (
        DefinitionBundle {
            version: 1,
            sources: vec![model, entry.clone()],
            namespaces: vec![],
        },
        entry,
        contract,
    )
}
#[test]
fn symbolic_datatype_annotations_and_features_use_installed_concrete_codec_on_substitution() {
    let (bundle, entry, contract) = fixture(
        "fn identity<T:m.Text>(x:m.Sequence<T>) -> m.Sequence<T> = x; fn read<T:m.Text>(x:m.Holder<T>) -> m.Sequence<T> = x.values; fn concrete(x:m.Sequence<m.Text>) -> m.Sequence<m.Text> = identity(x); fn concrete_read(x:m.Holder<m.Text>) -> m.Sequence<m.Text> = read(x);",
    );
    assert!(check(&bundle, &entry.id).is_err());
    let report = with_catalog_policies(
        &bundle,
        &[],
        std::slice::from_ref(&contract),
        &[],
        |registry| Ok(check_with_conversions(&bundle, &entry.id, registry)),
    )
    .unwrap()
    .unwrap();
    let identity = &report.signatures.functions[0];
    let SemType::SymbolicDatatypeApplication {
        declaration,
        arguments,
    } = report.signatures.types.get(identity.parameters[0]).unwrap()
    else {
        panic!("Symbolic datatype identity must remain distinct from a concrete codec application")
    };
    assert_eq!(declaration, &contract.conversion.datatype);
    assert_eq!(arguments, &vec![identity.generics[0].ty]);
    assert_eq!(identity.parameters[0], identity.result);
    let mut replay: DefinitionBundle =
        serde_json::from_value(serde_json::to_value(&bundle).unwrap()).unwrap();
    replay.sources.reverse();
    let again = with_catalog_policies(
        &replay,
        &[],
        std::slice::from_ref(&contract),
        &[],
        |registry| Ok(check_with_conversions(&replay, &entry.id, registry)),
    )
    .unwrap()
    .unwrap();
    assert_eq!(
        serde_json::to_value(report).unwrap(),
        serde_json::to_value(again).unwrap()
    );
    let mut stale = contract.clone();
    stale.conversion.provider.version = "stale".into();
    assert!(with_catalog_policies(&bundle, &[], &[stale], &[], |_| Ok(())).is_err());
    let mut stale = contract;
    stale.conversion.datatype.source.sha256 = "0".repeat(64);
    assert!(with_catalog_policies(&bundle, &[], &[stale], &[], |_| Ok(())).is_err());
}
#[test]
fn symbolic_datatype_kind_and_invariance_reject_even_with_valid_installed_codec() {
    for body in [
        "fn unbounded<T>(x:m.Sequence<T>) -> m.Sequence<T> = x;",
        "fn structural<T:Sequence<Int>>(x:m.Sequence<T>) -> m.Sequence<T> = x;",
        "fn erase<T:m.Text>(x:m.Sequence<T>) -> m.Sequence<m.Text> = x;",
        "fn unrelated<T:m.Text,U:m.Text>(x:m.Sequence<T>) -> m.Sequence<U> = x;",
    ] {
        let (bundle, entry, contract) = fixture(body);
        let errors = with_catalog_policies(&bundle, &[], &[contract], &[], |registry| {
            Ok(check_with_conversions(&bundle, &entry.id, registry))
        })
        .unwrap()
        .unwrap_err();
        assert!(!errors.is_empty());
        assert!(
            errors
                .iter()
                .all(|e| e.span.source == entry.id && e.span.start < e.span.end),
            "{body}: {errors:?}"
        );
    }
}

#[test]
fn universal_nominal_bounds_consume_provider_equivalence_before_any_concrete_call() {
    let model = DefinitionSource::new(
        "memory:symbolic-provider-bound.ecore",
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
    for concrete in [false, true] {
        let caller = if concrete {
            "fn caller(x:m.Box<m.Alias>) -> m.Alias = read(x);"
        } else {
            ""
        };
        let mut entry = DefinitionSource::new(
            "memory:symbolic-provider-bound-behavior.rcore",
            SourceKind::RcoreBehavior,
            format!(
                "rcore 1; package provider_bound version \"1\"; import model \"urn:rcore-behavior-provider-handoff\" as m; fn read<T:m.Alias>(x:m.Box<T>) -> T = x.value; {caller}"
            ),
        );
        entry.dependencies.push(model.id.clone());
        let bundle = DefinitionBundle {
            version: 1,
            sources: vec![model.clone(), entry.clone()],
            namespaces: vec![],
        };
        assert!(check(&bundle, &entry.id).is_err());
        let report = with_catalog_policies(&bundle, &contracts, &[], &[], |registry| {
            Ok(check_with_conversions(&bundle, &entry.id, registry))
        })
        .unwrap()
        .unwrap();
        assert_eq!(report.modules[0].symbolic_members.len(), 1);
        let feature = report.modules[0].symbolic_members.values().next().unwrap();
        assert_eq!(feature.declaration.source, model.id);
        assert_eq!(feature.declaration, declarations[&vec![2, 1]]);
        assert_eq!(feature.value, report.signatures.functions[0].generics[0].ty);
        for remaining in [&contracts[..1], &contracts[1..]] {
            let result = with_catalog_policies(&bundle, remaining, &[], &[], |registry| {
                Ok(check_with_conversions(&bundle, &entry.id, registry))
            })
            .unwrap();
            assert!(result.is_err());
        }
        let mut reversed = contracts.clone();
        reversed.reverse();
        let replay = with_catalog_policies(&bundle, &reversed, &[], &[], |registry| {
            Ok(check_with_conversions(&bundle, &entry.id, registry))
        })
        .unwrap()
        .unwrap();
        assert_eq!(
            serde_json::to_value(report).unwrap(),
            serde_json::to_value(replay).unwrap()
        );
    }
}
