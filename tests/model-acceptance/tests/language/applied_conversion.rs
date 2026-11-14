use intersect_model::conversion::with_application_catalog;
use intersect_model::definitions::{
    bundle::{DefinitionBundle, DefinitionSource, SourceKind},
    compiler::import_datatype_bundle,
    conversion::{
        AppliedConversionContract, AppliedConvertedValue, ConversionContract, MemberContract,
        MemberType, ProviderId,
    },
    generic_types::{ClosedType, Type},
    resources::DeclarationId,
};
use serde_json::json;
fn input() -> (DefinitionBundle, Vec<AppliedConversionContract>) {
    let source = DefinitionSource::new(
        "memory:applied-collections.ecore",
        SourceKind::Ecore,
        include_str!("../../../../fixtures/ecore-generics/applied-collections.ecore"),
    );
    let id = |n| DeclarationId {
        source: source.id.clone(),
        path: vec![n],
    };
    let ty = |n, arguments| Type::Classifier {
        declaration: id(n),
        arguments,
        raw: false,
    };
    let member = |n| MemberContract {
        nullable: false,
        value: MemberType::Datatype { declaration: id(n) },
    };
    let contract = |n, name: &str, arguments, members| AppliedConversionContract {
        application: ClosedType::new(ty(n, arguments)).unwrap(),
        conversion: ConversionContract {
            datatype: id(n),
            provider: ProviderId {
                name: name.into(),
                version: "1".into(),
            },
            serializable: false,
            declarations: vec![],
            members,
        },
    };
    let text = contract(2, "ecore.list", vec![ty(0, vec![])], vec![member(0)]);
    let number = contract(2, "ecore.list", vec![ty(1, vec![])], vec![member(1)]);
    let nested = contract(
        2,
        "ecore.list",
        vec![text.application.expression().clone()],
        vec![MemberContract {
            nullable: false,
            value: MemberType::Applied {
                contract: Box::new(text.clone()),
            },
        }],
    );
    let map = contract(
        3,
        "ecore.map",
        vec![ty(0, vec![]), ty(1, vec![])],
        vec![member(0), member(1)],
    );
    (
        DefinitionBundle {
            version: 1,
            sources: vec![source],
            namespaces: vec![],
        },
        vec![text, number, nested, map],
    )
}
#[test]
fn installed_applications_coexist_validate_nested_values_and_rebuild_from_wire() {
    let (bundle, contracts) = input();
    let nominal = |n, value| {
        let id = DeclarationId {
            source: bundle.sources[0].id.clone(),
            path: vec![n],
        };
        serde_json::to_value(
            import_datatype_bundle(&bundle, &id)
                .unwrap()
                .bind_value(value)
                .unwrap(),
        )
        .unwrap()
    };
    let word = nominal(0, json!("hello"));
    let number = nominal(1, json!(42));
    let values = with_application_catalog(&bundle, &contracts, |registry| {
        let text = registry.bind(&contracts[0].application, json!({"items":[word.clone()]}))?;
        let integer = registry.bind(&contracts[1].application, json!({"items":[number.clone()]}))?;
        assert_ne!(text.application(), integer.application());
        assert!(registry.validate_for(&contracts[1].application, &text).is_err());
        assert!(registry.bind(&contracts[0].application, json!({"items":[number.clone()]})).is_err());
        let nested = registry.bind(&contracts[2].application, json!({"items":[text.clone()]}))?;
        assert!(registry.bind(&contracts[2].application, json!({"items":[integer.clone()]})).is_err());
        let map = registry.bind(&contracts[3].application, json!({"entries":[{"key":word.clone(),"value":number.clone()}]}))?;
        assert!(registry.bind(&contracts[3].application, json!({"entries":[{"key":word.clone(),"value":number.clone()},{"key":word.clone(),"value":number.clone()}]})).is_err());
        assert!(registry.bind(&contracts[0].application, serde_json::Value::Null).is_err());
        assert!(registry.default_value(&contracts[0].application)?.is_none());
        assert!(registry.parse(&contracts[0].application,"[]").is_err());
        assert!(registry.format(&text).is_err());
        Ok(vec![text,integer,nested,map])
    }).unwrap();
    let wire = serde_json::to_value(&values).unwrap();
    assert_eq!(wire[0]["kind"], "converted_application_v1");
    let restored: Vec<AppliedConvertedValue> = serde_json::from_value(wire.clone()).unwrap();
    let selected: Vec<AppliedConversionContract> =
        serde_json::from_value(serde_json::to_value(&contracts).unwrap()).unwrap();
    with_application_catalog(&bundle, &selected, |registry| {
        for (value, contract) in restored.iter().zip(&contracts) {
            registry.validate_for(&contract.application, value)?;
        }
        Ok(())
    })
    .unwrap();
    for (field, value) in [
        ("kind", json!("converted_application_v2")),
        ("extra", json!(true)),
    ] {
        let mut bad = wire[0].clone();
        bad[field] = value;
        assert!(serde_json::from_value::<AppliedConvertedValue>(bad).is_err());
    }
    let mut wrong = wire[0].clone();
    wrong["converted"]["provider"]["version"] = json!("wrong");
    let wrong: AppliedConvertedValue = serde_json::from_value(wrong).unwrap();
    with_application_catalog(&bundle, &contracts, |registry| {
        assert!(registry.validate(&wrong).is_err());
        Ok(())
    })
    .unwrap();
}
#[test]
fn application_catalog_rejects_mismatched_arguments_contracts_and_missing_registrations() {
    let (bundle, contracts) = input();
    for index in 0..6 {
        let mut selected = contracts.clone();
        match index {
            0 => selected[0].conversion.members = selected[1].conversion.members.clone(),
            1 => selected[0].conversion.provider.version = "2".into(),
            2 => selected[0].conversion.datatype.path = vec![3],
            3 => selected.push(selected[0].clone()),
            4 => selected[0].conversion.serializable = true,
            _ => selected[0].conversion.members.clear(),
        }
        assert!(with_application_catalog(&bundle, &selected, |_| Ok(())).is_err());
    }
    with_application_catalog(&bundle, &contracts[..1], |registry| {
        assert!(
            registry
                .bind(&contracts[1].application, json!({"items":[]}))
                .is_err()
        );
        Ok(())
    })
    .unwrap();
}

#[test]
fn pinned_standard_list_specializes_builtin_arguments_without_nominal_erasure() {
    use intersect_model::definitions::{
        compiler::standard_datatype_source, ecore::Document, scalars::Builtin,
    };
    let source = standard_datatype_source();
    let document = Document::parse(source.clone()).unwrap();
    let id = |name| {
        let index = document
            .root
            .children
            .iter()
            .position(|node| {
                node.attributes.iter().any(|a| {
                    a.name.namespace.is_none() && a.name.local == "name" && a.value == name
                })
            })
            .unwrap();
        DeclarationId {
            source: source.id.clone(),
            path: vec![index],
        }
    };
    let ty = |name, arguments| Type::Classifier {
        declaration: id(name),
        arguments,
        raw: false,
    };
    let contract = AppliedConversionContract {
        application: ClosedType::new(ty("EEList", vec![ty("EString", vec![])])).unwrap(),
        conversion: ConversionContract {
            datatype: id("EEList"),
            provider: ProviderId {
                name: "ecore.list".into(),
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
    };
    let bundle = DefinitionBundle {
        version: 1,
        sources: vec![],
        namespaces: vec![],
    };
    with_application_catalog(&bundle, std::slice::from_ref(&contract), |registry| {
        let value = registry.bind(&contract.application, json!({"items":["hello"]}))?;
        registry.validate(&value)?;
        assert!(
            registry
                .bind(&contract.application, json!({"items":[42]}))
                .is_err()
        );
        Ok(())
    })
    .unwrap();
    let mut wrong = contract;
    wrong.conversion.members[0].value = MemberType::Builtin {
        codec: Builtin::EIntegerObject,
    };
    assert!(with_application_catalog(&bundle, &[wrong], |_| Ok(())).is_err());
}

#[test]
fn mutation_of_application_contract_during_payload_validation_cannot_publish() {
    use intersect_model::definitions::{
        conversion::{AppliedConversionProvider, AppliedConversionRegistry, ConversionProvider},
        generic_resolution::Resolver,
    };
    use std::cell::Cell;
    struct Payload {
        contract: ConversionContract,
        touched: Cell<bool>,
    }
    impl ConversionProvider for Payload {
        fn contract(&self) -> &ConversionContract {
            &self.contract
        }
        fn parse(&self, _: &str) -> Result<serde_json::Value, String> {
            Err("unused".into())
        }
        fn format(&self, _: &serde_json::Value) -> Result<String, String> {
            Err("unused".into())
        }
        fn default_value(&self) -> Result<Option<serde_json::Value>, String> {
            Ok(None)
        }
        fn validate(&self, _: &serde_json::Value) -> Result<(), String> {
            self.touched.set(true);
            Ok(())
        }
    }
    struct Provider {
        contract: AppliedConversionContract,
        changed: AppliedConversionContract,
        payload: Payload,
    }
    impl AppliedConversionProvider for Provider {
        fn contract(&self) -> &AppliedConversionContract {
            if self.payload.touched.get() {
                &self.changed
            } else {
                &self.contract
            }
        }
        fn implementation(&self) -> &dyn ConversionProvider {
            &self.payload
        }
    }
    let (bundle, contracts) = input();
    let contract = contracts[0].clone();
    let mut changed = contract.clone();
    changed.conversion.provider.version = "changed".into();
    let provider = Provider {
        payload: Payload {
            contract: contract.conversion.clone(),
            touched: Cell::new(false),
        },
        contract: contract.clone(),
        changed,
    };
    let datatype = import_datatype_bundle(&bundle, &contract.conversion.datatype).unwrap();
    let checked = Resolver::new(&bundle)
        .unwrap()
        .instantiate_datatype(&contract.application)
        .unwrap();
    let mut registry = AppliedConversionRegistry::default();
    registry.register(&datatype, &checked, &provider).unwrap();
    assert!(
        registry
            .bind(&contract.application, json!({"items":[]}))
            .unwrap_err()
            .contains("changed its registered contract")
    );
    assert!(registry.require(&contract.application).is_err());
}
