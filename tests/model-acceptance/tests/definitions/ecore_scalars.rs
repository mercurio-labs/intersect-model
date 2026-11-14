use intersect_model::definitions::scalars::Builtin;
use serde_json::{Value, json};

#[test]
fn unicode_decimal_acceptance_matches_pinned_database_and_utf16_boundary() {
    let mut expected = std::collections::BTreeMap::new();
    for line in include_str!("../../../../fixtures/unicode-13/UnicodeData.txt").lines() {
        let row: Vec<_> = line.split(';').collect();
        let code = u32::from_str_radix(row[0], 16).unwrap();
        if code <= 0xffff && row[2] == "Nd" {
            expected.insert(code, row[6].parse::<i64>().unwrap());
        }
    }
    assert_eq!(expected.len(), 370);
    for code in 0..=0x10ffff {
        let Some(character) = char::from_u32(code) else {
            continue;
        };
        let actual = Builtin::EInt.parse(&character.to_string());
        match expected.get(&code) {
            Some(value) => assert_eq!(actual.unwrap(), json!(value), "U+{code:04X}"),
            None => assert!(actual.is_err(), "U+{code:04X}"),
        }
    }
    for (text, canonical) in [
        ("+００١२", "12"),
        ("-٠", "0"),
        (
            "-９２２３３７２０３６８５４７７５８０８",
            "-9223372036854775808",
        ),
        ("٩٠٠٧١٩٩٢٥٤٧٤٠٩٩٣", "9007199254740993"),
    ] {
        assert_eq!(
            Builtin::ELong.parse(text).unwrap(),
            json!({"kind":"int64", "value":canonical})
        );
    }
    assert!(
        Builtin::ELong
            .parse("９２２３３７２０３６８５４７７５８０８")
            .is_err()
    );
    assert!(Builtin::EByte.parse("１２８").is_err());
    assert!(
        Builtin::ELong
            .validate(&json!({"kind":"int64", "value":"１２"}))
            .is_err()
    );
    assert!(Builtin::EDouble.parse("１２").is_err());
}

#[test]
fn unicode_boolean_case_mapping_preserves_strict_ecore_lexical_rules() {
    for codec in [Builtin::EBoolean, Builtin::EBooleanObject] {
        for text in ["false", "FALSE", "falſe", "FALſE"] {
            assert_eq!(codec.parse(text).unwrap(), json!(false));
        }
        assert_eq!(codec.parse("TrUe").unwrap(), json!(true));
        for text in [" falſe", "falſe\n", "falße", "ｆａｌｓｅ", "1", "", "tru℮"] {
            assert!(codec.parse(text).is_err(), "{text}");
        }
    }
}

#[test]
fn exact_integer_boundaries_and_lexical_normalization() {
    for (codec, min, max, below, above) in [
        (Builtin::EByte, "-128", "127", "-129", "128"),
        (Builtin::EByteObject, "-128", "127", "-129", "128"),
        (Builtin::EShort, "-32768", "32767", "-32769", "32768"),
        (Builtin::EShortObject, "-32768", "32767", "-32769", "32768"),
        (
            Builtin::EInt,
            "-2147483648",
            "2147483647",
            "-2147483649",
            "2147483648",
        ),
        (
            Builtin::EIntegerObject,
            "-2147483648",
            "2147483647",
            "-2147483649",
            "2147483648",
        ),
        (
            Builtin::ELong,
            "-9223372036854775808",
            "9223372036854775807",
            "-9223372036854775809",
            "9223372036854775808",
        ),
        (
            Builtin::ELongObject,
            "-9223372036854775808",
            "9223372036854775807",
            "-9223372036854775809",
            "9223372036854775808",
        ),
    ] {
        for text in [min, max] {
            assert_eq!(
                codec
                    .format(&codec.parse(text).unwrap())
                    .unwrap()
                    .as_deref(),
                Some(text)
            );
        }
        for text in [
            below, above, "", "+", "-", "1.0", "1e0", " 1", "1\n", "0x10", "1_0", "𝟙", "²", "−1",
            "＋1",
        ] {
            assert!(codec.parse(text).is_err(), "{codec:?}: {text}");
        }
        for text in ["0", "-0", "+0", "000", "-000"] {
            assert_eq!(
                codec
                    .format(&codec.parse(text).unwrap())
                    .unwrap()
                    .as_deref(),
                Some("0")
            );
        }
        assert_eq!(
            codec
                .format(&codec.parse("+0012").unwrap())
                .unwrap()
                .as_deref(),
            Some("12")
        );
    }
    for number in -256..=255 {
        assert_eq!(
            Builtin::EByte.parse(&number.to_string()).is_ok(),
            (-128..=127).contains(&number)
        );
    }
}

#[test]
fn long_wire_is_exact_and_rejects_lossy_or_noncanonical_representations() {
    for text in [
        "9007199254740991",
        "9007199254740992",
        "9007199254740993",
        "9223372036854775807",
        "-9223372036854775808",
    ] {
        let value = Builtin::ELong.parse(text).unwrap();
        assert_eq!(value, json!({"kind":"int64", "value":text}));
        let restored: Value =
            serde_json::from_str(&serde_json::to_string(&value).unwrap()).unwrap();
        assert_eq!(
            Builtin::ELong.format(&restored).unwrap().as_deref(),
            Some(text)
        );
    }
    for invalid in [
        json!(9007199254740993_i64),
        json!("12"),
        json!({"kind":"int64", "value":12}),
        json!({"kind":"int64", "value":"+12"}),
        json!({"kind":"int64", "value":"012"}),
        json!({"kind":"int64", "value":"-0"}),
        json!({"kind":"int64", "value":"9223372036854775808"}),
        json!({"kind":"int64", "value":"12", "extra":true}),
    ] {
        assert!(Builtin::ELong.validate(&invalid).is_err(), "{invalid}");
    }
    assert!(Builtin::EInt.validate(&json!(1.0)).is_err());
    assert!(Builtin::EByte.validate(&json!(128)).is_err());
}

#[test]
fn scalar_null_defaults_boolean_lexical_policy_and_unicode_strings_are_distinct() {
    for codec in [
        Builtin::EString,
        Builtin::EBooleanObject,
        Builtin::EByteObject,
        Builtin::EShortObject,
        Builtin::EIntegerObject,
        Builtin::ELongObject,
    ] {
        assert_eq!(codec.default_value(), Value::Null);
        assert_eq!(codec.format(&Value::Null).unwrap(), None);
    }
    for codec in [
        Builtin::EBoolean,
        Builtin::EByte,
        Builtin::EShort,
        Builtin::EInt,
        Builtin::ELong,
    ] {
        assert!(!codec.default_value().is_null());
        codec.validate(&codec.default_value()).unwrap();
        assert!(codec.validate(&Value::Null).is_err());
    }
    for codec in [Builtin::EBoolean, Builtin::EBooleanObject] {
        for text in ["true", "TRUE", "TrUe"] {
            assert_eq!(codec.parse(text).unwrap(), json!(true));
        }
        for text in ["false", "FALSE", "FaLsE"] {
            assert_eq!(codec.parse(text).unwrap(), json!(false));
        }
        for text in ["", "1", "0", "yes", " true", "false "] {
            assert!(codec.parse(text).is_err());
        }
    }
    for text in ["", "λ🙂", "e\u{301}", "é", "\0\n\t "] {
        assert_eq!(
            Builtin::EString
                .format(&Builtin::EString.parse(text).unwrap())
                .unwrap()
                .as_deref(),
            Some(text)
        );
    }
    assert_eq!(Builtin::named("EJavaObject"), None);
    assert_eq!(Builtin::named("Long"), None);
}

#[test]
fn imported_defaults_operation_signatures_and_rcore_behavior_reads_use_scalar_domains() {
    use intersect_model::definitions::{
        bundle::{DefinitionBundle, DefinitionSource, SourceKind},
        rcore_behavior::checking::check,
    };
    let xml = "<e:EPackage xmlns:e='http://www.eclipse.org/emf/2002/Ecore' xmlns:xsi='http://www.w3.org/2001/XMLSchema-instance' name='test' nsURI='urn:scalars'><eClassifiers xsi:type='e:EClass' name='Item'><eStructuralFeatures xsi:type='e:EAttribute' name='long' lowerBound='1' eType='e:EDataType http://www.eclipse.org/emf/2002/Ecore#//ELong' defaultValueLiteral='9007199254740993'/><eStructuralFeatures xsi:type='e:EAttribute' name='boxed' eType='e:EDataType http://www.eclipse.org/emf/2002/Ecore#//EByteObject'/><eOperations name='f' eType='e:EDataType http://www.eclipse.org/emf/2002/Ecore#//ELong'><eParameters name='x' eType='e:EDataType http://www.eclipse.org/emf/2002/Ecore#//ELongObject'/></eOperations></eClassifiers></e:EPackage>";
    let schema =
        intersect_model_acceptance::migration_fixture::compile_migrated_schema_fixture(xml)
            .unwrap();
    assert!(
        schema
            .validate_attribute(&schema.classes["Item"].features["boxed"], &Value::Null)
            .is_err()
    );
    assert_eq!(
        schema.classes["Item"].features["long"].default,
        Builtin::ELong.parse("9007199254740993").unwrap()
    );
    let operations = schema.resolved_operations("Item").unwrap();
    assert_eq!(
        operations[0].signature.parameters,
        [
            intersect_model::definitions::operations::ParameterType::Builtin {
                name: "ELongObject".into()
            }
        ]
    );
    let model = DefinitionSource::new("memory:scalars.ecore", SourceKind::Ecore, xml);
    let (model, _) = intersect_model_acceptance::migration_fixture::migrate_source_fixture(model);
    for (body, ok) in [
        (
            "fn f(item: m.Item) -> Int64 = item.long; fn g(item: m.Item) -> Int8? = item.boxed;",
            true,
        ),
        ("fn f(item: m.Item) -> Int32 = item.long;", false),
    ] {
        let mut source = DefinitionSource::new(
            "memory:scalars-behavior.rcore",
            SourceKind::RcoreBehavior,
            format!(
                "rcore 1; package scalars version \"1\"; import model \"urn:scalars\" as m; {body}"
            ),
        );
        source.dependencies.push(model.id.clone());
        let result = check(
            &DefinitionBundle {
                version: 1,
                sources: vec![model.clone(), source.clone()],
                namespaces: vec![],
            },
            &source.id,
        );
        assert_eq!(result.is_ok(), ok, "{result:?}");
    }
    assert!(
        intersect_model_acceptance::migration_fixture::compile_migrated_schema_fixture(
            &xml.replace("9007199254740993", "9223372036854775808")
        )
        .is_err()
    );
}
