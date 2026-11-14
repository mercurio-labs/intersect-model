use intersect_model::definitions::{
    bundle::{DefinitionSource, SourceKind},
    conversion::{ConversionContract, ConversionProvider, ConversionRegistry, ProviderId},
    datatypes::Datatype,
    resources::DeclarationId,
    scalars::Builtin,
};

use intersect_model::{
    collection_conversion::{CollectionConverter, CollectionKind, Member},
    conversion::RgbConverter,
};
use serde_json::{Value, json};

fn datatype(name: &str, class: &str, serializable: bool) -> Datatype {
    let source = DefinitionSource::new("memory:collections", SourceKind::Ecore, name);
    Datatype::new(
        DeclarationId {
            source: source.id,
            path: vec![0],
        },
        name.into(),
        serializable,
        Some(class.into()),
        None,
    )
    .unwrap()
}
fn contract(data: &Datatype) -> ConversionContract {
    ConversionContract {
        declarations: vec![],
        members: vec![],
        datatype: data.declaration().clone(),
        provider: ProviderId {
            name: "test.collection".into(),
            version: "1".into(),
        },
        serializable: data.serializable(),
    }
}
fn collection(kind: CollectionKind, members: Vec<Member>) -> CollectionConverter {
    let data = datatype("Collection", "example.Collection", false);
    let mut contract = contract(&data);
    contract.members = members.iter().map(|m| m.contract().clone()).collect();
    CollectionConverter::new(contract, kind, members).unwrap()
}
#[test]
fn lists_preserve_duplicates_order_moves_and_atomic_failures() {
    let provider = collection(
        CollectionKind::List,
        vec![Member::builtin(false, Builtin::EInt)],
    );
    let mut value = provider.empty().unwrap();
    provider.insert(&mut value, 0, json!(2)).unwrap();
    provider.insert(&mut value, 1, json!(2)).unwrap();
    provider.insert(&mut value, 1, json!(3)).unwrap();
    assert_eq!(provider.move_item(&mut value, 2, 0).unwrap(), json!(2));
    assert_eq!(value, json!({"items":[3,2,2]}));
    assert_eq!(provider.move_item(&mut value, 0, 2).unwrap(), json!(2));
    assert_eq!(value, json!({"items":[2,3,2]}));
    assert_eq!(provider.set(&mut value, 1, json!(4)).unwrap(), json!(3));
    let before = value.clone();
    assert!(provider.insert(&mut value, 0, Value::Null).is_err());
    assert!(provider.set(&mut value, 0, json!("4")).is_err());
    assert!(provider.move_item(&mut value, 3, 0).is_err());
    assert!(provider.remove_at(&mut value, usize::MAX).is_err());
    assert!(provider.insert(&mut value, usize::MAX, json!(1)).is_err());
    assert_eq!(value, before);
    assert_eq!(provider.remove_at(&mut value, 0).unwrap(), json!(2));
    provider.clear(&mut value).unwrap();
    assert_eq!(value, json!({"items":[]}));
    assert!(provider.move_item(&mut value, 0, 0).is_err());
    for malformed in [
        Value::Null,
        json!([]),
        json!({"items":null}),
        json!({"items":[],"extra":0}),
    ] {
        assert!(provider.validate(&malformed).is_err());
    }
    assert!(provider.parse("[]").is_err());
    assert!(provider.format(&value).is_err());
    assert_eq!(provider.default_value().unwrap(), None);
}
#[test]
fn maps_preserve_insertion_order_and_distinguish_absence_from_null() {
    let provider = collection(
        CollectionKind::Map,
        vec![
            Member::builtin(false, Builtin::EString),
            Member::builtin(true, Builtin::EInt),
        ],
    );
    let mut value = provider.empty().unwrap();
    assert_eq!(
        provider.put(&mut value, json!("a"), Value::Null).unwrap(),
        None
    );
    provider.put(&mut value, json!("b"), json!(2)).unwrap();
    assert_eq!(
        provider.get(&value, &json!("a")).unwrap(),
        Some(Value::Null)
    );
    assert_eq!(provider.get(&value, &json!("missing")).unwrap(), None);
    assert_eq!(
        provider.put(&mut value, json!("a"), json!(1)).unwrap(),
        Some(Value::Null)
    );
    assert_eq!(
        value,
        json!({"entries":[{"key":"a","value":1},{"key":"b","value":2}]})
    );
    let before = value.clone();
    assert!(provider.put(&mut value, Value::Null, json!(3)).is_err());
    assert!(provider.put(&mut value, json!("a"), json!("bad")).is_err());
    assert_eq!(value, before);
    assert_eq!(
        provider.remove_key(&mut value, &json!("a")).unwrap(),
        Some(json!(1))
    );
    assert_eq!(provider.remove_key(&mut value, &json!("a")).unwrap(), None);
    assert!(
        provider
            .validate(&json!({"entries":[{"key":"a","value":1},{"key":"a","value":2}]}))
            .is_err()
    );
    assert!(
        provider
            .validate(&json!({"entries":[{"key":"a"}]}))
            .is_err()
    );
    assert!(provider.insert(&mut value, 0, json!(1)).is_err());
}
#[test]
fn map_key_equality_preserves_decimal_scale_and_float_bits() {
    for (codec, first, second) in [
        (
            Builtin::EBigDecimal,
            json!({"kind":"decimal","coefficient":"10","scale":1}),
            json!({"kind":"decimal","coefficient":"100","scale":2}),
        ),
        (
            Builtin::EDouble,
            Builtin::EDouble.parse("0").unwrap(),
            Builtin::EDouble.parse("-0").unwrap(),
        ),
    ] {
        let provider = collection(
            CollectionKind::Map,
            vec![
                Member::builtin(false, codec),
                Member::builtin(false, Builtin::EInt),
            ],
        );
        let mut value = provider.empty().unwrap();
        provider.put(&mut value, first.clone(), json!(1)).unwrap();
        provider.put(&mut value, second.clone(), json!(2)).unwrap();
        assert_eq!(provider.get(&value, &first).unwrap(), Some(json!(1)));
        assert_eq!(provider.get(&value, &second).unwrap(), Some(json!(2)));
    }
}
#[test]
fn nominal_and_nested_converted_members_reject_foreign_values() {
    let nominal = datatype("Count", "int", true);
    let other = datatype("Other", "int", true);
    let member = Member::datatype(false, nominal.clone()).unwrap();
    member
        .validate(&serde_json::to_value(nominal.bind_value(json!(7)).unwrap()).unwrap())
        .unwrap();
    assert!(member.validate(&json!(7)).is_err());
    assert!(
        member
            .validate(&serde_json::to_value(other.bind_value(json!(7)).unwrap()).unwrap())
            .is_err()
    );
    let rgb = datatype("RGB", "example.RGB", true);
    let mut selected = contract(&rgb);
    selected.provider.name = "example.rgb".into();
    let converter = RgbConverter::new(selected.clone());
    let mut registry = ConversionRegistry::default();
    registry.register(&rgb, &converter).unwrap();
    let wire = serde_json::to_value(registry.parse(rgb.declaration(), "#010203").unwrap()).unwrap();
    let member = Member::converted(false, rgb, Box::new(RgbConverter::new(selected))).unwrap();
    let provider = collection(CollectionKind::List, vec![member]);
    let mut value = provider.empty().unwrap();
    provider.insert(&mut value, 0, wire.clone()).unwrap();
    let mut bad = wire;
    bad["provider"]["version"] = json!("2");
    let before = value.clone();
    assert!(provider.insert(&mut value, 0, bad).is_err());
    assert_eq!(before, value);
    let list_type = datatype("Inner", "example.List", false);
    // Use the adapter's own exact declaration for the nested datatype.
    let mut inner_contract = contract(&list_type);
    let inner_member = Member::builtin(false, Builtin::EInt);
    inner_contract.members = vec![inner_member.contract().clone()];
    let inner = CollectionConverter::new(
        inner_contract.clone(),
        CollectionKind::List,
        vec![inner_member],
    )
    .unwrap();
    let nested = Member::converted(false, list_type.clone(), Box::new(inner)).unwrap();
    let outer = collection(CollectionKind::List, vec![nested]);
    let payload = json!({"kind":"converted","datatype":list_type.declaration(),"provider":inner_contract.provider,"value":{"items":[1,2]}});
    outer.validate(&json!({"items":[payload]})).unwrap();
}
#[test]
fn member_contract_arity_and_nullability_are_explicit() {
    let data = datatype("Collection", "example.List", false);
    assert!(CollectionConverter::new(contract(&data), CollectionKind::List, vec![]).is_err());
    let member = Member::builtin(true, Builtin::EInt);
    let mut selected = contract(&data);
    selected.members = vec![member.contract().clone()];
    selected.members[0].nullable = false;
    assert!(CollectionConverter::new(selected, CollectionKind::List, vec![member]).is_err());
    let nullable = collection(
        CollectionKind::List,
        vec![Member::builtin(true, Builtin::EInt)],
    );
    nullable.validate(&json!({"items":[null]})).unwrap();
}

#[test]
fn nested_provider_mutation_during_validation_cannot_publish() {
    use std::cell::Cell;
    struct Changing {
        original: ConversionContract,
        changed: ConversionContract,
        switched: Cell<bool>,
    }
    impl ConversionProvider for Changing {
        fn contract(&self) -> &ConversionContract {
            if self.switched.get() {
                &self.changed
            } else {
                &self.original
            }
        }
        fn parse(&self, _: &str) -> Result<Value, String> {
            Err("no lexical conversion".into())
        }
        fn format(&self, _: &Value) -> Result<String, String> {
            Err("no lexical conversion".into())
        }
        fn validate(&self, _: &Value) -> Result<(), String> {
            self.switched.set(true);
            Ok(())
        }
        fn default_value(&self) -> Result<Option<Value>, String> {
            Ok(None)
        }
    }
    let data = datatype("Mutable", "example.Mutable", false);
    let original = contract(&data);
    let mut changed = original.clone();
    changed
        .members
        .push(intersect_model::definitions::conversion::MemberContract {
            nullable: true,
            value: intersect_model::definitions::conversion::MemberType::Builtin {
                codec: Builtin::EInt,
            },
        });
    let member = Member::converted(
        true,
        data.clone(),
        Box::new(Changing {
            original: original.clone(),
            changed,
            switched: Cell::new(false),
        }),
    )
    .unwrap();
    let provider = collection(CollectionKind::List, vec![member]);
    let mut value = provider.empty().unwrap();
    let before = value.clone();
    let payload = json!({"kind":"converted","datatype":data.declaration(),"provider":original.provider,"value":1});
    assert!(provider.insert(&mut value, 0, payload).is_err());
    assert_eq!(value, before);
    assert!(provider.validate(&before).is_err());
    assert!(provider.default_value().is_err());
}

#[test]
fn enum_members_keep_alias_and_source_identity() {
    use intersect_model::definitions::{bundle::DefinitionBundle, compiler::import_enum_bundle};
    let source = DefinitionSource::new(
        "memory:enum",
        SourceKind::Ecore,
        r#"<e:EPackage xmlns:e="http://www.eclipse.org/emf/2002/Ecore" xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance" name="m" nsURI="urn:m"><eClassifiers xsi:type="e:EEnum" name="Mode"><eLiterals name="First" value="7"/><eLiterals name="Alias" value="7"/></eClassifiers></e:EPackage>"#,
    );
    let id = DeclarationId {
        source: source.id.clone(),
        path: vec![0],
    };
    let bundle = DefinitionBundle {
        version: 1,
        sources: vec![source],
        namespaces: vec![],
    };
    let enumeration = import_enum_bundle(&bundle, &id).unwrap();
    let first = serde_json::to_value(enumeration.parse("First").unwrap()).unwrap();
    let alias = serde_json::to_value(enumeration.parse("Alias").unwrap()).unwrap();
    let provider = collection(
        CollectionKind::Map,
        vec![
            Member::enumeration(false, enumeration),
            Member::builtin(false, Builtin::EInt),
        ],
    );
    let mut value = provider.empty().unwrap();
    provider.put(&mut value, first.clone(), json!(1)).unwrap();
    provider.put(&mut value, alias.clone(), json!(2)).unwrap();
    assert_eq!(provider.get(&value, &first).unwrap(), Some(json!(1)));
    assert_eq!(provider.get(&value, &alias).unwrap(), Some(json!(2)));
    let mut foreign = first;
    foreign["enumeration"]["source"]["sha256"] = json!("0".repeat(64));
    assert!(provider.put(&mut value, foreign, json!(3)).is_err());
}

#[test]
fn installed_collection_catalog_enforces_recursive_budgets_and_pinned_metadata() {
    use intersect_model::conversion::with_catalog;
    use intersect_model::definitions::{
        bundle::DefinitionBundle,
        compiler::standard_datatype,
        conversion::{MemberContract, MemberType},
    };
    let bundle = DefinitionBundle {
        version: 1,
        sources: vec![],
        namespaces: vec![],
    };
    let list_type = standard_datatype("EEList").unwrap();
    let map_type = standard_datatype("EMap").unwrap();
    assert_eq!(
        list_type.instance_class_name(),
        Some("org.eclipse.emf.common.util.EList")
    );
    assert_eq!(map_type.instance_class_name(), Some("java.util.Map"));
    assert!(
        list_type
            .source_payload()
            .unwrap()
            .contains("eTypeParameters name=\"E\"")
    );
    assert!(
        map_type
            .source_payload()
            .unwrap()
            .contains("eTypeParameters name=\"K\"")
    );
    assert!(
        map_type
            .source_payload()
            .unwrap()
            .contains("eTypeParameters name=\"V\"")
    );
    assert!(!list_type.serializable());
    assert!(!map_type.serializable());
    let mut leaf = contract(&list_type);
    leaf.provider.name = "ecore.list".into();
    leaf.members = vec![Member::builtin(false, Builtin::EInt).contract().clone()];
    let mut nested = leaf.clone();
    for _ in 1..16 {
        let mut parent = leaf.clone();
        parent.members = vec![MemberContract {
            nullable: false,
            value: MemberType::Converted {
                contract: Box::new(nested),
            },
        }];
        nested = parent;
    }
    with_catalog(&bundle, &[nested.clone()], |_| Ok(())).unwrap();
    let mut too_deep = leaf.clone();
    too_deep.members = vec![MemberContract {
        nullable: false,
        value: MemberType::Converted {
            contract: Box::new(nested),
        },
    }];
    assert!(
        with_catalog(&bundle, &[too_deep], |_| Ok(()))
            .unwrap_err()
            .contains("budget")
    );
    let mut tree = leaf;
    for _ in 0..7 {
        let mut parent = contract(&map_type);
        parent.provider.name = "ecore.map".into();
        let member = MemberContract {
            nullable: false,
            value: MemberType::Converted {
                contract: Box::new(tree),
            },
        };
        parent.members = vec![member.clone(), member];
        tree = parent;
    }
    assert!(
        with_catalog(&bundle, &[tree], |_| Ok(()))
            .unwrap_err()
            .contains("budget")
    );
}
