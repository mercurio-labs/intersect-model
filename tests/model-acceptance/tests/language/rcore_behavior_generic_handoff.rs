use intersect_model::conversion::with_catalog_policies;
use intersect_model::definitions::{
    bundle::{DefinitionBundle, DefinitionSource, SourceKind},
    conversion::{ConversionContract, ProviderId},
    rcore_behavior::checking::{check, check_with_conversions},
};

#[test]
fn rcore_behavior_generic_model_annotations_consume_selected_provider_bounds() {
    let (model, declarations) =
        intersect_model_acceptance::migration_fixture::migrate_source_fixture(
            DefinitionSource::new(
                "memory:provider-handoff",
                SourceKind::Ecore,
                include_str!(
                    "../../../../fixtures/ecore-generics/rcore-behavior-provider-handoff.ecore"
                ),
            ),
        );
    let mut source = DefinitionSource::new(
        "memory:handoff-rcore_behavior",
        SourceKind::RcoreBehavior,
        "rcore 1; package handoff version \"1\"; import model \"urn:rcore-behavior-provider-handoff\" as m; fn read(x: m.Box<m.Alias>) -> m.Alias = x.value;",
    );
    source.dependencies.push(model.id.clone());
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
    let bundle = DefinitionBundle {
        version: 1,
        sources: vec![model, source.clone()],
        namespaces: vec![],
    };
    assert!(check(&bundle, &source.id).is_err());
    let result = with_catalog_policies(&bundle, &contracts, &[], &[], |registry| {
        Ok(check_with_conversions(&bundle, &source.id, registry))
    })
    .unwrap()
    .unwrap();
    assert_eq!(result.modules[0].members.len(), 1);
    let reversed: Vec<_> = contracts.iter().rev().cloned().collect();
    let again = with_catalog_policies(&bundle, &reversed, &[], &[], |registry| {
        Ok(check_with_conversions(&bundle, &source.id, registry))
    })
    .unwrap()
    .unwrap();
    assert_eq!(
        serde_json::to_value(result).unwrap(),
        serde_json::to_value(again).unwrap()
    );
    let missing = with_catalog_policies(&bundle, &contracts[1..], &[], &[], |registry| {
        Ok(check_with_conversions(&bundle, &source.id, registry))
    })
    .unwrap();
    assert!(missing.is_err());
    let mut stale = contracts;
    stale[0].provider.version = "stale".into();
    assert!(with_catalog_policies(&bundle, &stale, &[], &[], |_| Ok(())).is_err());
}

#[test]
fn generic_provider_bound_navigation_executes_without_losing_alias_identity() {
    use intersect_model::core::model::Model;
    use intersect_model::definitions::{
        compiler::materialize_runtime_bundle,
        conversion::ReferenceContext,
        generic_types::{RuntimeType, Type},
        rcore_behavior::{
            function_lowering::compile_function_with_conversions,
            ir_v2::{DataWire, EntrySelection, Limits, ValueWire},
            modules::SymbolId,
        },
    };
    use intersect_model::rcore_behavior_functions::{
        ExecutionContext, ExecutionErrorKind, ExecutionLimits, evaluate,
    };
    use serde_json::json;
    let (model, declarations) =
        intersect_model_acceptance::migration_fixture::migrate_source_fixture(
            DefinitionSource::new(
                "memory:provider-handoff",
                SourceKind::Ecore,
                include_str!(
                    "../../../../fixtures/ecore-generics/rcore-behavior-provider-handoff.ecore"
                ),
            ),
        );
    let id = |n| declarations[&vec![n]].clone();
    let expression = |n, arguments| Type::Classifier {
        declaration: id(n),
        arguments,
        raw: false,
    };
    let target = RuntimeType::new(expression(2, vec![expression(1, vec![])])).unwrap();
    let contracts: Vec<_> = (0..2)
        .map(|n| ConversionContract {
            datatype: id(n),
            provider: ProviderId {
                name: "example.rgb".into(),
                version: "1".into(),
            },
            serializable: true,
            declarations: vec![],
            members: vec![],
        })
        .collect();
    let mut source = DefinitionSource::new(
        "memory:provider-navigation",
        SourceKind::RcoreBehavior,
        "rcore 1;package handoff version \"1\";import model \"urn:rcore-behavior-provider-handoff\" as m;fn read(x:m.Box<m.Alias>)->m.Alias ={let read=|unused:Int8|x.value;keep(read(0))};fn keep<T>(x:T)->T=x;",
    );
    source.dependencies.push(model.id.clone());
    let model_bundle = DefinitionBundle {
        version: 1,
        sources: vec![model.clone()],
        namespaces: vec![],
    };
    let bundle = DefinitionBundle {
        version: 1,
        sources: vec![model.clone(), source.clone()],
        namespaces: vec![],
    };
    with_catalog_policies(&bundle, &contracts, &[], &[], |registry| {
        let schema = materialize_runtime_bundle(&model_bundle, &model.id, "urn:rcore-behavior-provider-handoff", std::slice::from_ref(&target), Some(registry)).unwrap();
        let class = schema.instantiations.iter().find(|(_, ty)| **ty == target).unwrap().0;
        let payload = json!({"kind":"converted", "datatype":id(1), "provider":contracts[1].provider, "value":{"red":18,"green":58,"blue":188}});
        let mut snapshot = Model::default(); snapshot.create(&schema, "box", class, "r", 0, 0).unwrap();
        snapshot.elements[0].values.insert("value".into(), payload.clone()); snapshot.validate_with_conversions(&schema, registry).unwrap();
        let reference = ReferenceContext { bundle: model_bundle.clone(), entry: model.id.clone(), namespace: "urn:rcore-behavior-provider-handoff".into(), conversions: schema.conversion_contracts.clone(), materialized_types: vec![target.clone()], applied_conversions: vec![], constructors: vec![], model: serde_json::to_value(snapshot).unwrap() };
        let image = compile_function_with_conversions(&bundle, &source.id, &EntrySelection { symbol: SymbolId { source: source.id.clone(), declaration: 1 }, types: vec![], arguments: vec![] }, registry, &Limits::default()).unwrap();
        let ty = image.program().functions[image.program().entry.0 as usize].parameters[0];
        let run = |reference: &ReferenceContext| {
            let mut context = ExecutionContext::new(&bundle); context.conversions = Some(registry); context.references = std::slice::from_ref(reference);
            evaluate(&image, &[ValueWire { ty, data: DataWire::Model(json!({"kind":"model_reference", "context":reference.digest().unwrap(), "element":"box"})) }], &context, &ExecutionLimits::default())
        };
        let before = reference.clone(); let result = run(&reference).unwrap();
        assert_eq!(result.value.data, DataWire::Datatype(payload.clone()));
        assert_eq!(result.consumed_providers, vec![contracts[1].provider.clone()]); assert_eq!(reference, before);
        for stale in [false, true] {
            let mut invalid = reference.clone();
            if stale { invalid.model["elements"][0]["values"]["value"]["provider"]["version"] = json!("stale"); }
            else { invalid.model["elements"][0]["values"]["value"]["datatype"] = json!(id(0)); }
            let before = invalid.clone(); let error = run(&invalid).unwrap_err();
            assert_eq!(error.kind, ExecutionErrorKind::Nominal, "{error:?}"); assert_eq!(invalid, before);
        }
        let mut reversed = reference.clone(); reversed.conversions.reverse();
        assert_eq!(run(&reversed).unwrap().value.data, DataWire::Datatype(payload));
        assert!(run(&reference).is_ok());
        Ok(())
    }).unwrap();
}

#[test]
#[cfg_attr(not(feature = "milestone-tests"), ignore = "milestone")]
fn rcore_behavior_model_context_preserves_applied_types_and_requires_constructor_policies() {
    use intersect_model::definitions::{
        conversion::{AppliedConversionContract, MemberContract, MemberType},
        generic_types::{ClosedType, Type},
    };
    let (model, declarations) =
        intersect_model_acceptance::migration_fixture::migrate_source_fixture(
            DefinitionSource::new(
                "memory:constructor-handoff",
                SourceKind::Ecore,
                include_str!(
                    "../../../../fixtures/ecore-generics/rcore-behavior-constructor-handoff.ecore"
                ),
            ),
        );
    let id = |index| declarations[&vec![index]].clone();
    let ty = |index, arguments| Type::Classifier {
        declaration: id(index),
        arguments,
        raw: false,
    };
    let provider = |name: &str| ProviderId {
        name: name.into(),
        version: "1".into(),
    };
    let codec = |index, members| ConversionContract {
        datatype: id(index),
        provider: provider("ecore.list"),
        serializable: false,
        declarations: vec![],
        members,
    };
    let alias = AppliedConversionContract {
        application: ClosedType::new(ty(2, vec![ty(0, vec![])])).unwrap(),
        conversion: codec(
            2,
            vec![MemberContract {
                nullable: false,
                value: MemberType::Datatype { declaration: id(0) },
            }],
        ),
    };
    let bounded = AppliedConversionContract {
        application: ClosedType::new(ty(3, vec![alias.application.expression().clone()])).unwrap(),
        conversion: codec(
            3,
            vec![MemberContract {
                nullable: false,
                value: MemberType::Applied {
                    contract: Box::new(alias.clone()),
                },
            }],
        ),
    };
    let mut bundle = DefinitionBundle {
        version: 1,
        sources: vec![model.clone()],
        namespaces: vec![],
    };
    let constructors: Vec<_> = [1, 2]
        .iter()
        .map(|index| {
            intersect_model::conversion::constructor_contract(
                &bundle,
                &id(*index),
                provider("ecore.list.constructor"),
            )
            .unwrap()
        })
        .collect();
    let applications = vec![alias, bounded];
    let make_source = |body: &str| {
        let mut source = DefinitionSource::new(
            "memory:constructor-handoff-rcore_behavior",
            SourceKind::RcoreBehavior,
            format!(
                "rcore 1; package handoff version \"1\"; import model \"urn:constructors\" as m; {body}"
            ),
        );
        source.dependencies.push(model.id.clone());
        source
    };
    let source = make_source(
        "fn identity(x: m.Holder) -> m.Holder = x; fn name(x: m.Holder) -> String? = x.name; fn values(x: m.Holder) -> m.Bounded<m.Alias<m.Text>>? = x.values; fn many(x: m.Holder) -> Bool = { let values = x.many; true };",
    );
    bundle.sources.push(source.clone());
    let checked = with_catalog_policies(&bundle, &[], &applications, &constructors, |registry| {
        Ok(check_with_conversions(&bundle, &source.id, registry))
    })
    .unwrap()
    .unwrap();
    assert_eq!(checked.modules[0].members.len(), 3);
    assert_eq!(checked.generic_datatypes.len(), 2);
    // Exercise the previously static handoff through actual stored source reads.
    for many in [false, true] {
        let source = make_source(if many {
            "fn read(x:m.Holder)->Bag<m.Bounded<m.Alias<m.Text>>> =Bag(x.many);"
        } else {
            "fn read(x:m.Holder)->Option<m.Bounded<m.Alias<m.Text>>> =x.values;"
        });
        let model_bundle = DefinitionBundle {
            version: 1,
            sources: vec![model.clone()],
            namespaces: vec![],
        };
        let executable_bundle = DefinitionBundle {
            version: 1,
            sources: vec![model.clone(), source.clone()],
            namespaces: vec![],
        };
        with_catalog_policies(&executable_bundle, &[], &applications, &constructors, |registry| {
            use intersect_model::definitions::{compiler::materialize_runtime_bundle, conversion::ReferenceContext,
                generic_types::RuntimeType, rcore_behavior::{function_lowering::compile_function_with_conversions,
                ir_v2::{DataWire, EntrySelection, Limits, ValueWire}, modules::SymbolId}};
            use intersect_model::rcore_behavior_functions::{evaluate, ExecutionContext, ExecutionErrorKind, ExecutionLimits};
            use serde_json::json;
            let target = RuntimeType::new(ty(5, vec![])).unwrap();
            let schema = materialize_runtime_bundle(&model_bundle, &model.id, "urn:constructors", std::slice::from_ref(&target), Some(registry)).unwrap();
            let text = json!({"kind":"datatype","datatype":id(0),"value":"retained"});
            let alias_value = json!({"kind":"converted_application_v1","application":applications[0].application,"converted":{"kind":"converted","datatype":id(2),"provider":applications[0].conversion.provider,"value":{"items":[text]}}});
            let payload = json!({"kind":"converted_application_v1","application":applications[1].application,"converted":{"kind":"converted","datatype":id(3),"provider":applications[1].conversion.provider,"value":{"items":[alias_value]}}});
            let mut snapshot = intersect_model::core::model::Model::default();
            snapshot.create(&schema, "holder", schema.class_key_for_type(&target).unwrap(), "r", 0, 0).unwrap();
            let field = if many { "many" } else { "values" };
            let stored = if many { json!([payload, payload]) } else { payload.clone() };
            snapshot.edit_with_conversions(&schema, &[("holder".into(), field.into(), stored)], registry).unwrap();
            snapshot.validate_with_conversions(&schema, registry).unwrap();
            let reference = ReferenceContext { bundle:model_bundle.clone(),entry:model.id.clone(),namespace:"urn:constructors".into(),conversions:schema.conversion_contracts.clone(),materialized_types:vec![target],applied_conversions:schema.applied_conversion_contracts.clone(),constructors:schema.constructor_contracts.clone(),model:serde_json::to_value(snapshot).unwrap() };
            let image = compile_function_with_conversions(&executable_bundle,&source.id,&EntrySelection {symbol:SymbolId {source:source.id.clone(),declaration:1},types:vec![],arguments:vec![]},registry,&Limits::default()).unwrap();
            let parameter = image.program().functions[image.program().entry.0 as usize].parameters[0];
            let run = |reference:&ReferenceContext| {
                let mut context=ExecutionContext::new(&executable_bundle);context.conversions=Some(registry);context.references=std::slice::from_ref(reference);
                evaluate(&image,&[ValueWire {ty:parameter,data:DataWire::Model(json!({"kind":"model_reference","context":reference.digest().unwrap(),"element":"holder"}))}],&context,&ExecutionLimits::default())
            };
            let before=reference.clone();let result=run(&reference).unwrap();
            let expected=serde_json::to_value(&result).unwrap();
            {
                use intersect_model::rcore_behavior_functions::{EvaluationSession, ExecutionControl, SessionProgress};
                use std::num::NonZeroUsize;
                struct Cancel;
                impl ExecutionControl for Cancel { fn cancellation_requested(&self)->bool {true} }
                let mut context=ExecutionContext::new(&executable_bundle);context.conversions=Some(registry);context.references=std::slice::from_ref(&reference);
                let arguments=[ValueWire {ty:parameter,data:DataWire::Model(json!({"kind":"model_reference","context":reference.digest().unwrap(),"element":"holder"}))}];
                let limits=ExecutionLimits::default();let mut boundaries=0;
                for quantum in [1,3,4096] {
                    let mut session=EvaluationSession::new(&image,&arguments,&context,&limits).unwrap();let mut pending=0;
                    loop {
                        match session.resume(NonZeroUsize::new(quantum).unwrap()) {
                            SessionProgress::Pending=>{pending+=1;assert!(pending<10000);},
                            SessionProgress::Complete(outcome)=>{assert_eq!(serde_json::to_value(outcome.unwrap()).unwrap(),expected);break;},
                            SessionProgress::Finished=>panic!("missing applied result"),
                        }
                    }
                    if quantum==1 {boundaries=pending;}
                }
                assert!(boundaries>10);
                let mut saw_provider=false;
                for boundary in 0..=boundaries {
                    let mut session=EvaluationSession::new(&image,&arguments,&context,&limits).unwrap();
                    for _ in 0..boundary {assert!(matches!(session.resume(NonZeroUsize::MIN),SessionProgress::Pending));}
                    let SessionProgress::Complete(outcome)=session.resume_with_control(NonZeroUsize::MIN,&Cancel) else {panic!("missing applied cancellation")};
                    let failure=outcome.unwrap_err();assert_eq!(failure.kind,ExecutionErrorKind::Cancelled);saw_provider|=failure.consumed.provider_invocations>0;
                    assert!(matches!(session.resume(NonZeroUsize::MAX),SessionProgress::Finished));assert_eq!(reference,before);
                }
                assert!(saw_provider,"cancelled after actual provider invocation");
                assert_eq!(serde_json::to_value(run(&reference).unwrap()).unwrap(),expected);
            }
            if many {
                let DataWire::Collection(items)=result.value.data else {panic!("Many")};
                assert_eq!(items.len(),2); for item in items {assert_eq!(item.data,DataWire::Datatype(payload.clone()));}
            } else {let DataWire::Some(item)=result.value.data else {panic!("optional application")};assert_eq!(item.data,DataWire::Datatype(payload.clone()));}
            assert_eq!(reference,before);
            assert!(result.consumed_providers.contains(&provider("ecore.list")));
            let mut empty=reference.clone();
            if many {empty.model["elements"][0]["values"][field]=json!([]);} else {empty.model["elements"][0]["values"].as_object_mut().unwrap().remove(field);}
            let empty_result=run(&empty).unwrap();assert_eq!(empty_result.value.data,if many {DataWire::Collection(vec![])}else{DataWire::None});
            for mutation in 0..3 {
                let mut invalid=reference.clone();let value=if many {&mut invalid.model["elements"][0]["values"][field][0]}else{&mut invalid.model["elements"][0]["values"][field]};
                let nested=&mut value["converted"]["value"]["items"][0];
                match mutation {
                    0=>nested["converted"]["value"]["items"][0]["value"]=json!(7),
                    1=>nested["converted"]["provider"]["version"]=json!("stale"),
                    _=>nested["application"]["arguments"][0]["declaration"]["path"]=json!([7]),
                }
                let before=invalid.clone();let error=run(&invalid).unwrap_err();assert_eq!(error.kind,ExecutionErrorKind::Nominal,"{error:?}");assert_eq!(invalid,before);
            }
            assert!(run(&reference).is_ok());Ok(())
        }).unwrap();
    }
    assert!(
        checked
            .generic_datatypes
            .iter()
            .any(|checked| checked.application() == &applications[1].application)
    );
    let many = checked.modules[0]
        .members
        .values()
        .find(|feature| feature.declaration == declarations[&vec![5, 2]])
        .unwrap();
    let intersect_model::definitions::rcore_behavior::types::Type::Many {
        element,
        ordered,
        unique,
        ..
    } = checked.signatures.types.get(many.value).unwrap()
    else {
        panic!("many type");
    };
    assert!(!ordered && !unique);
    assert_eq!(
        checked.signatures.types.get(*element).unwrap(),
        &intersect_model::definitions::rcore_behavior::types::Type::DatatypeApplication(
            applications[1].application.expression().clone()
        )
    );
    let missing_apps = with_catalog_policies(&bundle, &[], &[], &constructors, |registry| {
        Ok(check_with_conversions(&bundle, &source.id, registry))
    })
    .unwrap();
    assert!(missing_apps.is_err());
    let missing_constructors =
        with_catalog_policies(&bundle, &[], &applications, &[], |registry| {
            check_with_conversions(&bundle, &source.id, registry)
                .map_err(|errors| format!("{errors:?}"))
        });
    assert!(missing_constructors.is_err());
    let mut stale = constructors.clone();
    stale[0].provider.version = "stale".into();
    assert!(with_catalog_policies(&bundle, &[], &applications, &stale, |_| Ok(())).is_err());
    let mut stale_apps = applications.clone();
    stale_apps[1].conversion.provider.version = "stale".into();
    assert!(with_catalog_policies(&bundle, &[], &stale_apps, &constructors, |_| Ok(())).is_err());
    let alternate = AppliedConversionContract {
        application: ClosedType::new(ty(2, vec![ty(7, vec![])])).unwrap(),
        conversion: codec(
            2,
            vec![MemberContract {
                nullable: false,
                value: MemberType::Datatype { declaration: id(7) },
            }],
        ),
    };
    let mut expanded = applications.clone();
    expanded.push(alternate);
    for (body, expected) in [
        (
            "fn wrong(x: m.Alias<m.Text>) -> m.Alias<m.Number> = x;",
            "Type mismatch",
        ),
        (
            "fn wrong(x: m.Bounded<m.Alias<m.Number>>) -> Bool = true;",
            "bounds",
        ),
        (
            "fn wrong(x: m.Alias) -> Bool = true;",
            "explicit closed arguments",
        ),
        (
            "fn wrong(x: m.Alias<m.Text,m.Number>) -> Bool = true;",
            "argument",
        ),
        (
            "fn wrong(x: m.Holder) -> m.Bounded<m.Alias<m.Text>> = x.values;",
            "Type mismatch",
        ),
    ] {
        let source = make_source(body);
        bundle.sources[1] = source.clone();
        let errors = with_catalog_policies(&bundle, &[], &expanded, &constructors, |registry| {
            Ok(check_with_conversions(&bundle, &source.id, registry))
        })
        .unwrap()
        .unwrap_err();
        assert!(
            errors.iter().any(|error| error
                .message
                .to_lowercase()
                .contains(&expected.to_lowercase())),
            "{body}: {errors:?}"
        );
        assert!(
            errors
                .iter()
                .all(|error| error.span.source == source.id && error.span.start < error.span.end)
        );
    }
    with_catalog_policies(&bundle, &[], &applications, &constructors, |registry| {
        let mut types = intersect_model::definitions::rcore_behavior::model_types::ModelTypes::with_conversions(
            &bundle, registry,
        )?;
        let mut stale = applications[0].application.expression().clone();
        if let Type::Classifier { declaration, .. } = &mut stale {
            declaration.source.sha256 = "0".repeat(64);
        }
        assert!(types.applied_datatype(&stale).is_err());
        Ok(())
    })
    .unwrap();
}
