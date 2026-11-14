use intersect_model::conversion::{constructor_contract, with_catalog_policies};
use intersect_model::core::model::Model;
use intersect_model::definitions::{
    bundle::{DefinitionBundle, DefinitionSource, SourceKind},
    compiler::{import_datatype_bundle, materialize_runtime_types},
    conversion::{
        AppliedConversionContract, ConversionContract, MemberContract, MemberType, ProviderId,
    },
    generic_types::{ClosedType, RuntimeType, Type},
};
use serde_json::json;
const SOURCE: &str = r##"<e:EPackage xmlns:e="http://www.eclipse.org/emf/2002/Ecore" xmlns:x="http://www.w3.org/2001/XMLSchema-instance" name="lowerattrs" nsURI="urn:lower-attrs">
<eClassifiers x:type="e:EDataType" name="Text" instanceClassName="java.lang.String"/>
<eClassifiers x:type="e:EDataType" name="Number" instanceClassName="java.lang.Integer"/>
<eClassifiers x:type="e:EDataType" name="EList" instanceClassName="org.eclipse.emf.common.util.EList" serializable="false"><eTypeParameters name="E"/></eClassifiers>
<eClassifiers x:type="e:EDataType" name="List" instanceClassName="java.util.List" serializable="false"><eTypeParameters name="E"/></eClassifiers>
<eClassifiers x:type="e:EDataType" name="Map" instanceClassName="java.util.Map" serializable="false"><eTypeParameters name="K"/><eTypeParameters name="V"/></eClassifiers>
<eClassifiers x:type="e:EClass" name="Holder"><eTypeParameters name="T"><eBounds eClassifier="#//EList"><eTypeArguments eClassifier="#//Text"/></eBounds></eTypeParameters><eStructuralFeatures x:type="e:EAttribute" name="name" eType="e:EDataType http://www.eclipse.org/emf/2002/Ecore#//EString"/><eStructuralFeatures x:type="e:EAttribute" name="value"><eGenericType eClassifier="#//Map"><eTypeArguments><eLowerBound eTypeParameter="#//Holder/T"/></eTypeArguments><eTypeArguments eTypeParameter="#//Holder/T"/></eGenericType></eStructuralFeatures></eClassifiers>
<eClassifiers x:type="e:EClass" name="Reverse"><eTypeParameters name="T"><eBounds eClassifier="#//List"><eTypeArguments eClassifier="#//Text"/></eBounds></eTypeParameters><eStructuralFeatures x:type="e:EAttribute" name="name" eType="e:EDataType http://www.eclipse.org/emf/2002/Ecore#//EString"/><eStructuralFeatures x:type="e:EAttribute" name="value"><eGenericType eClassifier="#//Map"><eTypeArguments><eLowerBound eTypeParameter="#//Reverse/T"/></eTypeArguments><eTypeArguments eTypeParameter="#//Reverse/T"/></eGenericType></eStructuralFeatures></eClassifiers>
</e:EPackage>"##;
#[test]
fn installed_lower_attribute_domains_preserve_direction_tags_and_rollback() {
    let source =
        DefinitionSource::new("memory:runtime-lower-attributes", SourceKind::Ecore, SOURCE);
    let (source, migration) =
        intersect_model_acceptance::migration_fixture::migrate_source_fixture(source);
    let id = |n| migration[&vec![n]].clone();
    let ty = |n, args| Type::Classifier {
        declaration: id(n),
        arguments: args,
        raw: false,
    };
    let scalar = |n| MemberContract {
        nullable: false,
        value: MemberType::Datatype { declaration: id(n) },
    };
    let contract = |n, args, members| AppliedConversionContract {
        application: ClosedType::new(ty(n, args)).unwrap(),
        conversion: ConversionContract {
            datatype: id(n),
            provider: ProviderId {
                name: match n {
                    2 => "ecore.list",
                    3 => "java.list",
                    _ => "ecore.map",
                }
                .into(),
                version: "1".into(),
            },
            serializable: false,
            declarations: vec![],
            members,
        },
    };
    let elist = contract(2, vec![ty(0, vec![])], vec![scalar(0)]);
    let list = contract(3, vec![ty(0, vec![])], vec![scalar(0)]);
    let numbers = contract(3, vec![ty(1, vec![])], vec![scalar(1)]);
    let nested = |c: &AppliedConversionContract| MemberContract {
        nullable: false,
        value: MemberType::Applied {
            contract: Box::new(c.clone()),
        },
    };
    let map = |a: &AppliedConversionContract, b: &AppliedConversionContract| {
        contract(
            4,
            vec![
                a.application.expression().clone(),
                b.application.expression().clone(),
            ],
            vec![nested(a), nested(b)],
        )
    };
    let good = map(&list, &elist);
    let wrong_nested = map(&numbers, &elist);
    let wrong_second = map(&list, &list);
    let reverse = map(&elist, &list);
    let unrelated = contract(
        4,
        vec![ty(0, vec![]), elist.application.expression().clone()],
        vec![scalar(0), nested(&elist)],
    );
    let applications = vec![
        elist.clone(),
        list.clone(),
        numbers.clone(),
        good.clone(),
        wrong_nested.clone(),
        wrong_second.clone(),
        reverse.clone(),
        unrelated.clone(),
    ];
    let receivers: Vec<_> = [5, 6]
        .into_iter()
        .map(|n| {
            RuntimeType::new(Type::Classifier {
                declaration: id(n),
                arguments: vec![],
                raw: true,
            })
            .unwrap()
        })
        .collect();
    let bundle = DefinitionBundle {
        version: 1,
        sources: vec![source.clone()],
        namespaces: vec![],
    };
    let constructors = [
        (2, "ecore.list.constructor"),
        (3, "java.list.constructor"),
        (4, "ecore.map.constructor"),
    ]
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
    let (expected, saved_schema, value) =
        with_catalog_policies(&bundle, &[], &applications, &constructors, |registry| {
            let schema = materialize_runtime_types(&bundle, &receivers, Some(registry))
                .map_err(|e| e.to_string())?;
            let text = json!(
                import_datatype_bundle(&bundle, &id(0))
                    .map_err(|e| e.to_string())?
                    .parse("text")?
            );
            let number = json!(
                import_datatype_bundle(&bundle, &id(1))
                    .map_err(|e| e.to_string())?
                    .parse("7")?
            );
            let e = json!(
                registry
                    .applied()
                    .bind(&elist.application, json!({"items":[text.clone()]}))?
            );
            let l = json!(
                registry
                    .applied()
                    .bind(&list.application, json!({"items":[text.clone()]}))?
            );
            let n = json!(
                registry
                    .applied()
                    .bind(&numbers.application, json!({"items":[number]}))?
            );
            let bind = |c: &AppliedConversionContract,
                        key: serde_json::Value,
                        value: serde_json::Value| {
                registry
                    .applied()
                    .bind(
                        &c.application,
                        json!({"entries":[{"key":key,"value":value}]}),
                    )
                    .map(|v| json!(v))
            };
            let accepted = bind(&good, l.clone(), e.clone())?;
            let mut model = Model::default();
            for (name, receiver) in ["holder", "reverse"].into_iter().zip(&receivers) {
                model.create(
                    &schema,
                    name,
                    schema.class_key_for_type(receiver)?,
                    "memory:model",
                    0,
                    0,
                )?;
            }
            assert!(model.elements[0].values["value"].is_null());
            model.edit_with_conversions(
                &schema,
                &[("holder".into(), "value".into(), accepted.clone())],
                registry,
            )?;
            let before = model.clone();
            let mut stale = accepted.clone();
            stale["application"]["declaration"]["source"]["sha256"] = json!("0".repeat(64));
            for (owner, rejected) in [
                ("holder", bind(&wrong_nested, n, e.clone())?),
                ("holder", bind(&wrong_second, l.clone(), l.clone())?),
                ("holder", bind(&unrelated, text, e.clone())?),
                ("reverse", bind(&reverse, e, l)?),
                ("holder", stale),
            ] {
                assert!(
                    model
                        .edit_with_conversions(
                            &schema,
                            &[
                                ("holder".into(), "name".into(), json!("rollback")),
                                (owner.into(), "value".into(), rejected)
                            ],
                            registry
                        )
                        .is_err()
                );
                assert_eq!(model, before);
            }
            model.validate_with_conversions(&schema, registry)?;
            Ok((model, schema, accepted))
        })
        .unwrap();
    let replay: DefinitionBundle = serde_json::from_value(json!(bundle)).unwrap();
    with_catalog_policies(&replay, &[], &applications, &constructors, |registry| {
        let schema = materialize_runtime_types(&replay, &receivers, Some(registry))
            .map_err(|e| e.to_string())?;
        assert_eq!(schema, saved_schema);
        expected.validate_with_conversions(&schema, registry)
    })
    .unwrap();
    with_catalog_policies(&bundle, &[], &applications, &[], |registry| {
        let schema = materialize_runtime_types(&bundle, &receivers, Some(registry))
            .map_err(|e| e.to_string())?;
        let key = schema.class_key_for_type(&receivers[0])?;
        assert!(schema.feature_domains(key, "value").iter().any(|d| {
            d.unresolved
                .iter()
                .any(|u| u.candidate.expression() == good.application.expression())
        }));
        let mut model = Model::default();
        model.create(&schema, "holder", key, "memory:model", 0, 0)?;
        let before = model.clone();
        let error = model
            .edit_with_conversions(
                &schema,
                &[("holder".into(), "value".into(), value)],
                registry,
            )
            .unwrap_err();
        assert!(error.contains("Unresolved"), "{error}");
        assert_eq!(model, before);
        Ok(())
    })
    .unwrap();
}

#[test]
fn lower_any_bound_proof_is_independent_of_unknown_relation_order() {
    for reversed in [false, true] {
        for lower_only in [true, false] {
            let elist = r##"<eBounds eClassifier="#//EList"><eTypeArguments eClassifier="#//Text"/></eBounds>"##;
            let list = r##"<eBounds eClassifier="#//List"><eTypeArguments eClassifier="#//Text"/></eBounds>"##;
            let bounds = if reversed {
                format!("{list}{elist}")
            } else {
                format!("{elist}{list}")
            };
            let mut text = SOURCE.replacen(elist, &bounds, 1);
            if lower_only {
                text = text.replace(
                    r##"<eTypeArguments eTypeParameter="#//Holder/T"/>"##,
                    r##"<eTypeArguments eClassifier="#//Text"/>"##,
                );
            }
            let source =
                DefinitionSource::new("memory:lower-any-bound-order", SourceKind::Ecore, text);
            let (source, migration) =
                intersect_model_acceptance::migration_fixture::migrate_source_fixture(source);
            let id = |n| migration[&vec![n]].clone();
            let ty = |n, args| Type::Classifier {
                declaration: id(n),
                arguments: args,
                raw: false,
            };
            let scalar = MemberContract {
                nullable: false,
                value: MemberType::Datatype { declaration: id(0) },
            };
            let list = AppliedConversionContract {
                application: ClosedType::new(ty(3, vec![ty(0, vec![])])).unwrap(),
                conversion: ConversionContract {
                    datatype: id(3),
                    provider: ProviderId {
                        name: "java.list".into(),
                        version: "1".into(),
                    },
                    serializable: false,
                    declarations: vec![],
                    members: vec![scalar.clone()],
                },
            };
            let member = MemberContract {
                nullable: false,
                value: MemberType::Applied {
                    contract: Box::new(list.clone()),
                },
            };
            let second = if lower_only {
                ty(0, vec![])
            } else {
                list.application.expression().clone()
            };
            let map = AppliedConversionContract {
                application: ClosedType::new(ty(
                    4,
                    vec![list.application.expression().clone(), second],
                ))
                .unwrap(),
                conversion: ConversionContract {
                    datatype: id(4),
                    provider: ProviderId {
                        name: "ecore.map".into(),
                        version: "1".into(),
                    },
                    serializable: false,
                    declarations: vec![],
                    members: vec![member.clone(), if lower_only { scalar } else { member }],
                },
            };
            let receiver = RuntimeType::new(Type::Classifier {
                declaration: id(5),
                arguments: vec![],
                raw: true,
            })
            .unwrap();
            let bundle = DefinitionBundle {
                version: 1,
                sources: vec![source.clone()],
                namespaces: vec![],
            };
            with_catalog_policies(&bundle,&[],&[list.clone(),map.clone()],&[],|registry| {
    let schema=materialize_runtime_types(&bundle,std::slice::from_ref(&receiver),Some(registry)).map_err(|e|e.to_string())?;
    let scalar=json!(import_datatype_bundle(&bundle,&id(0)).map_err(|e|e.to_string())?.parse("ordered")?);
    let item=json!(registry.applied().bind(&list.application,json!({"items":[scalar.clone()]}))?);
    let value=json!(registry.applied().bind(&map.application,json!({"entries":[{"key":item,"value":if lower_only {scalar}else{item.clone()}}]}))?);
    let key=schema.class_key_for_type(&receiver)?;let mut model=Model::default();model.create(&schema,"h",key,"memory:model",0,0)?;
    let before=model.clone();let result=model.edit_with_conversions(&schema,&[("h".into(),"name".into(),json!("changed")),("h".into(),"value".into(),value.clone())],registry);
    if lower_only {result?;assert_eq!(model.elements[0].values["value"],value);model.validate_with_conversions(&schema,registry)?;} else {let error=result.unwrap_err();assert!(error.contains("Unresolved"),"{error}");assert_eq!(model,before);}
    Ok(())
   }).unwrap();
        }
    }
}
