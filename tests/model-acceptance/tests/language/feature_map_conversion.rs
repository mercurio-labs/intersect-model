use intersect_model::definitions::{
    bundle::{DefinitionBundle, DefinitionSource, SourceKind},
    compiler::{import_datatype_bundle, import_enum_bundle},
    conversion::{ConversionContract, ConversionProvider, ConversionRegistry, ProviderId},
    resources::DeclarationId,
    scalars::Builtin,
};

use intersect_model::{
    collection_conversion::Member,
    conversion::RgbConverter,
    feature_map_conversion::{Entry, FeatureMapConverter, FeatureMapKind},
};
use serde_json::{Value, json};
const ECORE: &str = r##"<e:EPackage xmlns:e="http://www.eclipse.org/emf/2002/Ecore" xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance" name="fm" nsURI="urn:fm">
<eClassifiers xsi:type="e:EDataType" name="Count" instanceClassName="int"/>
<eClassifiers xsi:type="e:EDataType" name="RGB" instanceClassName="example.RGB"/>
<eClassifiers xsi:type="e:EEnum" name="Mode"><eLiterals name="First" value="7"/><eLiterals name="Alias" value="7"/></eClassifiers>
<eClassifiers xsi:type="e:EClass" name="Holder">
<eStructuralFeatures xsi:type="e:EAttribute" name="text" eType="e:EDataType http://www.eclipse.org/emf/2002/Ecore#//EString"/>
<eStructuralFeatures xsi:type="e:EAttribute" name="count" eType="#//Count"/>
<eStructuralFeatures xsi:type="e:EAttribute" name="mode" eType="#//Mode"/>
<eStructuralFeatures xsi:type="e:EAttribute" name="color" eType="#//RGB"/>
<eStructuralFeatures xsi:type="e:EReference" name="peer" eType="#//Peer"/>
</eClassifiers>
<eClassifiers xsi:type="e:EClass" name="Peer"/>
<eClassifiers xsi:type="e:EDataType" name="Entry" instanceClassName="org.eclipse.emf.ecore.util.FeatureMap$Entry" serializable="false"/>
<eClassifiers xsi:type="e:EDataType" name="Map" instanceClassName="org.eclipse.emf.ecore.util.FeatureMap" serializable="false"/>
</e:EPackage>"##;
fn bundle() -> DefinitionBundle {
    DefinitionBundle {
        version: 1,
        sources: vec![DefinitionSource::new(
            "memory:features",
            SourceKind::Ecore,
            ECORE,
        )],
        namespaces: vec![],
    }
}
fn id(bundle: &DefinitionBundle, path: &[usize]) -> DeclarationId {
    DeclarationId {
        source: bundle.sources[0].id.clone(),
        path: path.to_vec(),
    }
}
fn contract(bundle: &DefinitionBundle, path: &[usize], serializable: bool) -> ConversionContract {
    ConversionContract {
        declarations: vec![],
        members: vec![],
        datatype: id(bundle, path),
        provider: ProviderId {
            name: "test.features".into(),
            version: "1".into(),
        },
        serializable,
    }
}
fn members(bundle: &DefinitionBundle) -> Vec<Member> {
    vec![
        Member::builtin(true, Builtin::EString),
        Member::datatype(
            false,
            import_datatype_bundle(bundle, &id(bundle, &[0])).unwrap(),
        )
        .unwrap(),
        Member::enumeration(
            false,
            import_enum_bundle(bundle, &id(bundle, &[2])).unwrap(),
        ),
        Member::converted(
            false,
            import_datatype_bundle(bundle, &id(bundle, &[1])).unwrap(),
            Box::new(RgbConverter::new(contract(bundle, &[1], true))),
        )
        .unwrap(),
    ]
}
fn adapter(kind: FeatureMapKind) -> (DefinitionBundle, FeatureMapConverter) {
    let bundle = bundle();
    let members = members(&bundle);
    let mut selected = contract(
        &bundle,
        &[if kind == FeatureMapKind::Entry { 5 } else { 6 }],
        false,
    );
    selected.declarations = (0..4).map(|i| id(&bundle, &[3, i])).collect();
    selected.members = members
        .iter()
        .map(|member| member.contract().clone())
        .collect();
    let adapter = FeatureMapConverter::new(selected, kind, &bundle, members).unwrap();
    (bundle, adapter)
}
fn values(bundle: &DefinitionBundle) -> Vec<Value> {
    let count = import_datatype_bundle(bundle, &id(bundle, &[0])).unwrap();
    let enumeration = import_enum_bundle(bundle, &id(bundle, &[2])).unwrap();
    let rgb = import_datatype_bundle(bundle, &id(bundle, &[1])).unwrap();
    let converter = RgbConverter::new(contract(bundle, &[1], true));
    let mut registry = ConversionRegistry::default();
    registry.register(&rgb, &converter).unwrap();
    vec![
        json!("hello"),
        serde_json::to_value(count.bind_value(json!(7)).unwrap()).unwrap(),
        serde_json::to_value(enumeration.parse("Alias").unwrap()).unwrap(),
        serde_json::to_value(registry.parse(rgb.declaration(), "#010203").unwrap()).unwrap(),
    ]
}
#[test]
fn entries_check_feature_and_builtin_nominal_enum_provider_value_identity() {
    let (bundle, adapter) = adapter(FeatureMapKind::Entry);
    for (index, value) in values(&bundle).into_iter().enumerate() {
        let entry = adapter.entry(id(&bundle, &[3, index]), value).unwrap();
        let wire = serde_json::to_value(&entry).unwrap();
        assert_eq!(adapter.read_entry(&wire).unwrap(), entry);
        let mut stale = wire.clone();
        stale["feature"]["source"]["sha256"] = json!("0".repeat(64));
        assert!(adapter.validate(&stale).is_err());
        let mut wrong = wire;
        wrong["feature"] = json!(id(&bundle, &[3, (index + 1) % 4]));
        assert!(adapter.validate(&wrong).is_err());
    }
    let null = adapter.entry(id(&bundle, &[3, 0]), Value::Null).unwrap();
    adapter
        .validate(&serde_json::to_value(null).unwrap())
        .unwrap();
    assert!(adapter.entry(id(&bundle, &[3, 1]), Value::Null).is_err());
    assert!(adapter.entry(id(&bundle, &[3, 1]), json!(7)).is_err());
    for bad in [
        Value::Null,
        json!({"feature":id(&bundle,&[3,0])}),
        json!({"feature":id(&bundle,&[3,0]),"value":"a","extra":1}),
    ] {
        assert!(adapter.validate(&bad).is_err());
    }
    assert!(adapter.empty().is_err());
    assert!(adapter.parse("a").is_err());
    assert!(adapter.format(&json!({})).is_err());
    assert_eq!(adapter.default_value().unwrap(), None);
}
#[test]
fn maps_preserve_global_order_duplicates_projection_and_atomic_edits() {
    let (bundle, adapter) = adapter(FeatureMapKind::Map);
    let values = values(&bundle);
    let mut map = adapter.empty().unwrap();
    for index in [0, 2, 0, 1, 3] {
        let end = map["entries"].as_array().unwrap().len();
        adapter
            .insert(
                &mut map,
                end,
                adapter
                    .entry(id(&bundle, &[3, index]), values[index].clone())
                    .unwrap(),
            )
            .unwrap();
    }
    assert_eq!(
        adapter.values(&map, &id(&bundle, &[3, 0])).unwrap(),
        vec![json!("hello"), json!("hello")]
    );
    let moved = adapter.move_entry(&mut map, 0, 4).unwrap();
    assert_eq!(map["entries"][0], json!(moved));
    let replacement = adapter
        .entry(id(&bundle, &[3, 0]), json!("changed"))
        .unwrap();
    assert_eq!(adapter.set(&mut map, 0, replacement).unwrap(), moved);
    let before = map.clone();
    assert!(
        adapter
            .insert(
                &mut map,
                0,
                Entry {
                    feature: id(&bundle, &[3, 1]),
                    value: json!("wrong")
                }
            )
            .is_err()
    );
    assert!(adapter.move_entry(&mut map, 99, 0).is_err());
    assert!(adapter.remove(&mut map, 99).is_err());
    assert_eq!(map, before);
    assert!(adapter.values(&map, &id(&bundle, &[3, 4])).is_err());
    adapter.remove(&mut map, 0).unwrap();
    adapter.clear(&mut map).unwrap();
    assert_eq!(map, json!({"entries":[]}));
    assert!(
        adapter
            .validate(&json!({"entries":[{"feature":id(&bundle,&[3,0])}]}))
            .is_err()
    );
}
#[test]
fn policy_mismatches_unsorted_scopes_and_reference_context_gaps_reject() {
    let bundle = bundle();
    for (scope, member) in [
        (id(&bundle, &[3, 0]), Member::builtin(false, Builtin::EInt)),
        (id(&bundle, &[3, 1]), Member::builtin(false, Builtin::EInt)),
        (
            id(&bundle, &[3, 4]),
            Member::builtin(true, Builtin::EString),
        ),
        (id(&bundle, &[3]), Member::builtin(false, Builtin::EString)),
    ] {
        let mut selected = contract(&bundle, &[6], false);
        selected.declarations = vec![scope];
        selected.members = vec![member.contract().clone()];
        assert!(
            FeatureMapConverter::new(selected, FeatureMapKind::Map, &bundle, vec![member]).is_err()
        );
    }
    let (_, valid) = adapter(FeatureMapKind::Map);
    let mut selected = valid.contract().clone();
    selected.declarations.swap(0, 1);
    assert!(
        FeatureMapConverter::new(selected, FeatureMapKind::Map, &bundle, members(&bundle)).is_err()
    );
    let mut selected = valid.contract().clone();
    selected.serializable = true;
    assert!(
        FeatureMapConverter::new(selected, FeatureMapKind::Map, &bundle, members(&bundle)).is_err()
    );
}
