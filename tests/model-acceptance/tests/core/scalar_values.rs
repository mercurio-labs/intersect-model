use intersect_model::core::model::Model;
use intersect_model::definitions::scalars::Builtin;
use serde_json::json;

#[test]
fn binary_and_character_values_preserve_empty_surrogates_and_atomicity() {
    for (name, codec, first, second) in [
        ("EDate", Builtin::EDate, "1969-12-31", "1970-01-01"),
        ("EByteArray", Builtin::EByteArray, "", "ABC"),
        ("EChar", Builtin::EChar, "0", "55296"),
        ("ECharacterObject", Builtin::ECharacterObject, "0", "65535"),
    ] {
        let text = source().replace("#//ELong'", &format!("#//{name}'"));
        let schema =
            intersect_model_acceptance::migration_fixture::compile_migrated_schema_fixture(&text)
                .unwrap();
        let mut model = Model::default();
        model.create(&schema, "item", "Item", "api", 0, 0).unwrap();
        assert_eq!(model.elements[0].values["v3"], codec.default_value());
        assert!(!model.is_set(&schema, "item", "v3").unwrap());
        assert!(!model.is_set(&schema, "item", "longs").unwrap());
        let first = codec.parse(first).unwrap();
        let second = codec.parse(second).unwrap();
        model
            .edit(
                &schema,
                &[
                    ("item".into(), "v3".into(), second.clone()),
                    ("item".into(), "longs".into(), json!([first, second])),
                ],
            )
            .unwrap();
        assert!(model.is_set(&schema, "item", "v3").unwrap());
        assert!(model.is_set(&schema, "item", "longs").unwrap());
        let before = model.clone();
        for invalid in [
            json!([first, first]),
            json!([null]),
            json!([65]),
            json!([{"kind":"char16","unit":65536}]),
            json!([{"kind":"bytes","hex":"ab"}]),
        ] {
            assert!(
                model
                    .edit(
                        &schema,
                        &[
                            ("item".into(), "v3".into(), first.clone()),
                            ("item".into(), "longs".into(), invalid),
                        ]
                    )
                    .is_err()
            );
            assert_eq!(model, before);
        }
        let restored: Model =
            serde_json::from_value(serde_json::to_value(&model).unwrap()).unwrap();
        restored.validate(&schema).unwrap();
        assert_eq!(restored, model);
    }
}

#[test]
fn decimal_scale_is_preserved_by_model_uniqueness_rollback_and_reload() {
    let text = source().replace("#//ELong'", "#//EBigDecimal'");
    let schema =
        intersect_model_acceptance::migration_fixture::compile_migrated_schema_fixture(&text)
            .unwrap();
    let mut model = Model::default();
    model.create(&schema, "item", "Item", "api", 0, 0).unwrap();
    assert!(model.elements[0].values["v3"].is_null());
    let one = Builtin::EBigDecimal.parse("1.0").unwrap();
    let two = Builtin::EBigDecimal.parse("1.00").unwrap();
    model
        .edit(
            &schema,
            &[
                ("item".into(), "v3".into(), two.clone()),
                ("item".into(), "longs".into(), json!([one, two])),
            ],
        )
        .unwrap();
    let before = model.clone();
    for value in [
        json!([one, one]),
        json!([{"kind":"decimal", "coefficient":"01", "scale":1}]),
        json!([1.0]),
        json!([null]),
    ] {
        assert!(
            model
                .edit(
                    &schema,
                    &[
                        ("item".into(), "v3".into(), one.clone()),
                        ("item".into(), "longs".into(), value)
                    ]
                )
                .is_err()
        );
        assert_eq!(model, before);
    }
    let restored: Model = serde_json::from_value(serde_json::to_value(&model).unwrap()).unwrap();
    restored.validate(&schema).unwrap();
    assert_eq!(restored, model);
}

#[test]
fn arbitrary_integer_model_defaults_lists_and_atomic_edits_are_exact() {
    let text = source().replace("#//ELong'", "#//EBigInteger'");
    let schema =
        intersect_model_acceptance::migration_fixture::compile_migrated_schema_fixture(&text)
            .unwrap();
    let mut model = Model::default();
    model.create(&schema, "item", "Item", "api", 0, 0).unwrap();
    assert!(model.elements[0].values["v3"].is_null());
    let huge = Builtin::EBigInteger.parse(&"9".repeat(300)).unwrap();
    model
        .edit(
            &schema,
            &[
                ("item".into(), "v3".into(), huge.clone()),
                ("item".into(), "longs".into(), json!([huge])),
            ],
        )
        .unwrap();
    let before = model.clone();
    for bad in [
        json!(42),
        json!({"kind":"bigint", "value":"01"}),
        Builtin::ELong.parse("1").unwrap(),
    ] {
        assert!(
            model
                .edit(
                    &schema,
                    &[
                        ("item".into(), "v2".into(), json!(3)),
                        ("item".into(), "v3".into(), bad)
                    ]
                )
                .is_err()
        );
        assert_eq!(model, before);
    }
    let restored: Model = serde_json::from_value(serde_json::to_value(&model).unwrap()).unwrap();
    restored.validate(&schema).unwrap();
    assert_eq!(restored, model);
}

#[test]
fn declared_datatypes_preserve_identity_in_defaults_lists_edits_and_json() {
    let text = "<e:EPackage xmlns:e='http://www.eclipse.org/emf/2002/Ecore' xmlns:xsi='http://www.w3.org/2001/XMLSchema-instance' name='test' nsURI='urn:declared'><eClassifiers xsi:type='e:EDataType' name='Counter' instanceClassName='long'/><eClassifiers xsi:type='e:EDataType' name='Other' instanceClassName='long'/><eClassifiers xsi:type='e:EDataType' name='Boxed' instanceClassName='java.lang.Long'/><eClassifiers xsi:type='e:EClass' name='Item'><eStructuralFeatures xsi:type='e:EAttribute' name='count' lowerBound='1' eType='#//Counter' defaultValueLiteral='9007199254740993'/><eStructuralFeatures xsi:type='e:EAttribute' name='counts' upperBound='-1' eType='#//Counter'/><eStructuralFeatures xsi:type='e:EAttribute' name='boxed' eType='#//Boxed'/></eClassifiers></e:EPackage>";
    let schema =
        intersect_model_acceptance::migration_fixture::compile_migrated_schema_fixture(text)
            .unwrap();
    let mut model = Model::default();
    model.create(&schema, "item", "Item", "api", 0, 0).unwrap();
    model.validate(&schema).unwrap();
    let encode = |ty: &str, text: &str| {
        serde_json::to_value(schema.datatypes[ty].parse(text).unwrap()).unwrap()
    };
    assert_eq!(
        model.elements[0].values["count"],
        encode("Counter", "9007199254740993")
    );
    assert!(model.elements[0].values["boxed"].is_null());
    let value = encode("Counter", "9223372036854775807");
    model
        .edit(&schema, &[("item".into(), "counts".into(), json!([value]))])
        .unwrap();
    let before = model.clone();
    for (field, invalid) in [
        ("count", encode("Other", "1")),
        ("boxed", value.clone()),
        ("count", Builtin::ELong.parse("1").unwrap()),
        ("counts", json!([value, value])),
        ("counts", json!([null])),
        ("count", json!(null)),
    ] {
        assert!(
            model
                .edit(
                    &schema,
                    &[
                        ("item".into(), "count".into(), encode("Counter", "1")),
                        ("item".into(), field.into(), invalid)
                    ]
                )
                .is_err()
        );
        assert_eq!(model, before);
    }
    let restored: Model = serde_json::from_value(serde_json::to_value(&model).unwrap()).unwrap();
    restored.validate(&schema).unwrap();
    assert_eq!(restored, model);
}

fn source() -> String {
    let fields: String = ["EByte", "EShort", "EInt", "ELong", "EBoolean", "EByteObject", "EShortObject", "EIntegerObject", "ELongObject", "EBooleanObject", "EString"].iter().enumerate().map(|(index, name)| format!("<eStructuralFeatures xsi:type='e:EAttribute' name='v{index}' eType='e:EDataType http://www.eclipse.org/emf/2002/Ecore#//{name}'/>")).collect();
    format!(
        "<e:EPackage xmlns:e='http://www.eclipse.org/emf/2002/Ecore' xmlns:xsi='http://www.w3.org/2001/XMLSchema-instance' name='test' nsURI='urn:scalars'><eClassifiers xsi:type='e:EClass' name='Item'>{fields}<eStructuralFeatures xsi:type='e:EAttribute' name='longs' upperBound='-1' eType='e:EDataType http://www.eclipse.org/emf/2002/Ecore#//ELong'/></eClassifiers></e:EPackage>"
    )
}

#[test]
fn scalar_defaults_assignments_lists_and_json_reload_share_codecs() {
    let schema =
        intersect_model_acceptance::migration_fixture::compile_migrated_schema_fixture(&source())
            .unwrap();
    let mut model = Model::default();
    model.create(&schema, "item", "Item", "api", 0, 0).unwrap();
    model.validate(&schema).unwrap();
    for index in 0..3 {
        assert_eq!(model.elements[0].values[&format!("v{index}")], json!(0));
    }
    assert_eq!(
        model.elements[0].values["v3"],
        Builtin::ELong.default_value()
    );
    assert_eq!(model.elements[0].values["v4"], json!(false));
    for index in 5..11 {
        assert!(model.elements[0].values[&format!("v{index}")].is_null());
    }
    let long = Builtin::ELong.parse("9007199254740993").unwrap();
    model
        .edit(
            &schema,
            &[
                ("item".into(), "v3".into(), long.clone()),
                ("item".into(), "v8".into(), long.clone()),
                (
                    "item".into(),
                    "longs".into(),
                    json!([long, Builtin::ELong.parse("9223372036854775807").unwrap()]),
                ),
            ],
        )
        .unwrap();
    let restored: Model = serde_json::from_str(&serde_json::to_string(&model).unwrap()).unwrap();
    restored.validate(&schema).unwrap();
    assert_eq!(restored, model);
}

#[test]
fn invalid_scalar_edits_roll_back_including_large_json_numbers() {
    let schema =
        intersect_model_acceptance::migration_fixture::compile_migrated_schema_fixture(&source())
            .unwrap();
    let mut model = Model::default();
    model.create(&schema, "item", "Item", "api", 0, 0).unwrap();
    let before = model.clone();
    for (field, value) in [
        ("v0", json!(128)),
        ("v1", json!(-32769)),
        ("v2", json!(2147483648_i64)),
        ("v3", json!(9007199254740993_i64)),
        ("v4", json!(1)),
        ("v8", json!({"kind":"int64", "value":"01"})),
        ("longs", json!(["12"])),
    ] {
        assert!(
            model
                .edit(
                    &schema,
                    &[
                        ("item".into(), "v2".into(), json!(42)),
                        ("item".into(), field.into(), value)
                    ]
                )
                .is_err()
        );
        assert_eq!(model, before);
    }
}

#[test]
fn floats_preserve_nonfinite_and_signed_zero_values_with_atomic_list_validation() {
    let text = source().replace("</eClassifiers></e:EPackage>", "<eStructuralFeatures xsi:type='e:EAttribute' name='f' eType='e:EDataType http://www.eclipse.org/emf/2002/Ecore#//EFloat' defaultValueLiteral='-0x1p-150'/><eStructuralFeatures xsi:type='e:EAttribute' name='d' eType='e:EDataType http://www.eclipse.org/emf/2002/Ecore#//EDouble' defaultValueLiteral='NaN'/><eStructuralFeatures xsi:type='e:EAttribute' name='boxed' eType='e:EDataType http://www.eclipse.org/emf/2002/Ecore#//EFloatObject'/><eStructuralFeatures xsi:type='e:EAttribute' name='floats' upperBound='-1' eType='e:EDataType http://www.eclipse.org/emf/2002/Ecore#//EDouble'/></eClassifiers></e:EPackage>");
    let schema =
        intersect_model_acceptance::migration_fixture::compile_migrated_schema_fixture(&text)
            .unwrap();
    let mut model = Model::default();
    model.create(&schema, "item", "Item", "api", 0, 0).unwrap();
    assert_eq!(
        model.elements[0].values["f"],
        Builtin::EFloat.parse("-0").unwrap()
    );
    assert_eq!(
        model.elements[0].values["d"],
        Builtin::EDouble.parse("NaN").unwrap()
    );
    assert!(model.elements[0].values["boxed"].is_null());
    model.validate(&schema).unwrap();
    let zero = Builtin::EDouble.parse("0").unwrap();
    let negative = Builtin::EDouble.parse("-0").unwrap();
    let nan = Builtin::EDouble.parse("NaN").unwrap();
    let infinity = Builtin::EDouble.parse("Infinity").unwrap();
    model
        .edit(
            &schema,
            &[(
                "item".into(),
                "floats".into(),
                json!([zero, negative, nan, infinity]),
            )],
        )
        .unwrap();
    let before = model.clone();
    assert!(
        model
            .edit(
                &schema,
                &[("item".into(), "floats".into(), json!([nan, nan]))]
            )
            .is_err()
    );
    assert_eq!(model, before);
    assert!(
        model
            .edit(
                &schema,
                &[(
                    "item".into(),
                    "f".into(),
                    Builtin::EDouble.parse("1").unwrap()
                )]
            )
            .is_err()
    );
    assert_eq!(model, before);
    let restored: Model = serde_json::from_str(&serde_json::to_string(&model).unwrap()).unwrap();
    restored.validate(&schema).unwrap();
    assert_eq!(restored, model);
}
