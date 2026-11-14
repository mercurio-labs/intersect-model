use intersect_model::conversion::{constructor_contract, with_catalog_policies};
use intersect_model::definitions::{
    bundle::{DefinitionBundle, DefinitionSource, SourceKind},
    compiler::{materialize_runtime_types, standard_datatype_source},
    conversion::{
        AppliedConversionContract, ConversionContract, MemberContract, MemberType, ProviderId,
    },
    generic_types::{ClosedType, RuntimeType, Type},
    rcore_behavior::checking::check_with_conversions,
    resources::ResourceSet,
    scalars::Builtin,
};
#[test]
fn unresolved_runtime_candidates_cannot_certify_rcore_behavior_feature_reads() {
    let model = DefinitionSource::new(
        "memory:rcore-behavior-unresolved.ecore",
        SourceKind::Ecore,
        r##"<e:EPackage xmlns:e="http://www.eclipse.org/emf/2002/Ecore" xmlns:x="http://www.w3.org/2001/XMLSchema-instance" name="pending" nsURI="urn:rcore-behavior-unresolved">
<eClassifiers x:type="e:EDataType" name="EList" instanceClassName="org.eclipse.emf.common.util.EList" serializable="false"><eTypeParameters name="E"/></eClassifiers>
<eClassifiers x:type="e:EDataType" name="List" instanceClassName="java.util.List" serializable="false"><eTypeParameters name="E"/></eClassifiers>
<eClassifiers x:type="e:EClass" name="Holder"><eTypeParameters name="T"><eBounds eClassifier="#//List"><eTypeArguments eClassifier="http://www.eclipse.org/emf/2002/Ecore#//EString"/></eBounds></eTypeParameters><eStructuralFeatures x:type="e:EAttribute" name="name" eType="e:EDataType http://www.eclipse.org/emf/2002/Ecore#//EString"/><eStructuralFeatures x:type="e:EAttribute" name="value"><eGenericType eTypeParameter="#//Holder/T"/></eStructuralFeatures></eClassifiers>
</e:EPackage>"##,
    );
    let (model, migration) =
        intersect_model_acceptance::migration_fixture::migrate_source_fixture(model);
    let id = |n| migration[&vec![n]].clone();
    let ty = |n, args, raw| Type::Classifier {
        declaration: id(n),
        arguments: args,
        raw,
    };
    let standard = standard_datatype_source();
    let resources = ResourceSet::new(&DefinitionBundle {
        version: 1,
        sources: vec![standard.clone()],
        namespaces: vec![],
    })
    .unwrap();
    let string = Type::Classifier {
        declaration: resources.resolve(&standard.id, "#//EString").unwrap(),
        arguments: vec![],
        raw: false,
    };
    let receiver = RuntimeType::new(ty(2, vec![], true)).unwrap();
    let applications: Vec<_> = [(0, "ecore.list"), (1, "java.list")]
        .into_iter()
        .map(|(n, name)| AppliedConversionContract {
            application: ClosedType::new(ty(n, vec![string.clone()], false)).unwrap(),
            conversion: ConversionContract {
                datatype: id(n),
                provider: ProviderId {
                    name: name.into(),
                    version: "1".into(),
                },
                serializable: false,
                declarations: vec![],
                members: vec![MemberContract {
                    nullable: false,
                    value: MemberType::Builtin {
                        codec: Builtin::EString,
                    },
                }],
            },
        })
        .collect();
    let source = |uri: &str, body: &str| {
        let mut s = DefinitionSource::new(
            uri,
            SourceKind::RcoreBehavior,
            format!(
                "rcore 1; package {} version \"1\"; import model \"urn:rcore-behavior-unresolved\" as m; {body}",
                if uri.contains("names") {
                    "names"
                } else {
                    "reads"
                }
            ),
        );
        s.dependencies.push(model.id.clone());
        s
    };
    let read = source(
        "memory:read-behavior.rcore",
        "fn read(x: m.Holder) -> Bool = x.value == x.value;",
    );
    let names = source(
        "memory:names-behavior.rcore",
        "fn name(x: m.Holder) -> String? = x.name;",
    );
    let bundle = DefinitionBundle {
        version: 1,
        sources: vec![model.clone(), read.clone(), names.clone()],
        namespaces: vec![],
    };
    let constructors = [(0, "ecore.list.constructor"), (1, "java.list.constructor")]
        .into_iter()
        .map(|(n, name)| {
            constructor_contract(
                &bundle,
                &id(n),
                ProviderId {
                    name: name.into(),
                    version: "1".into(),
                },
            )
            .unwrap()
        })
        .collect::<Vec<_>>();
    for resolved in [false, true] {
        with_catalog_policies(&bundle,&[],&applications,if resolved {&constructors}else{&[]},|registry| {
            let schema=materialize_runtime_types(&bundle,std::slice::from_ref(&receiver),Some(registry)).map_err(|e|e.to_string())?;
            let key=schema.class_key_for_type(&receiver)?;let domains=schema.feature_domains(key,"value");assert!(!domains.is_empty());
            assert_eq!(domains.iter().any(|d|d.unresolved.iter().any(|u|u.candidate.expression()==applications[0].application.expression())),!resolved);
            if resolved {assert!(domains.iter().all(|d|d.unresolved.is_empty()), "{domains:?}");}
            let errors=check_with_conversions(&bundle,&read.id,registry).unwrap_err();
            let expected=if resolved {"Rcore constrained attribute domain requires an explicit static domain type; checked source projection is available"}else{"Rcore feature domain contains unresolved runtime candidates; complete static read coverage is unavailable"};
            assert!(errors.iter().any(|error|error.message==expected),"{errors:?}");
            assert!(errors.iter().filter(|error|error.message==expected).all(|error|error.span.source==read.id && error.span.start>0));
            // A neighboring exact member remains usable; unresolved coverage must
            // neither certify value nor poison independent checked feature reads.
            let checked=check_with_conversions(&bundle,&names.id,registry).map_err(|e|format!("{e:?}"))?;
            assert!(!checked.generic_models.is_empty());
            for reverse in [false, true] {
                let model_candidate="fn choose(f:(m.Holder)->Bool)->Bool=true;";
                let record_candidate="fn choose(f:({value:String?})->Bool)->Bool=true;";
                let candidates=if reverse {format!("{record_candidate}{model_candidate}")}else{format!("{model_candidate}{record_candidate}")};
                let mut overload=DefinitionSource::new("memory:overload-behavior.rcore",SourceKind::RcoreBehavior,
                    format!("rcore 1;package overload version \"1\";import model \"urn:rcore-behavior-unresolved\" as m;{candidates}fn main()->Bool=choose(|x:m.Holder|x.value==x.value);"));
                overload.dependencies.push(model.id.clone());
                let mut overload_bundle=bundle.clone();
                overload_bundle.sources.push(overload.clone());
                let errors=check_with_conversions(&overload_bundle,&overload.id,registry).unwrap_err();
                assert!(errors.iter().any(|error|error.message==expected && error.span.source==overload.id),"{errors:?}");
                assert!(!errors.iter().any(|error|error.message=="No applicable overload"),"{errors:?}");
            }
            Ok(())
        }).unwrap();
    }
}
