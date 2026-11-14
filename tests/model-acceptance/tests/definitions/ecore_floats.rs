use intersect_model::definitions::scalars::Builtin;
use serde_json::json;

fn bits(codec: Builtin, text: &str) -> String {
    codec.parse(text).unwrap()["bits"]
        .as_str()
        .unwrap()
        .to_owned()
}

#[test]
fn ieee_boundaries_and_ties_round_directly_to_the_selected_precision() {
    for (text, expected) in [
        ("0", "00000000"),
        ("-0", "80000000"),
        ("0x1p-149", "00000001"),
        ("0x1p-150", "00000000"),
        ("-0x1p-150", "80000000"),
        ("0x1.000000000000001p-150", "00000001"),
        ("0x3p-150", "00000002"),
        ("0x1p-126", "00800000"),
        ("0x0.fffffep-126", "007fffff"),
        ("0x0.ffffffp-126", "00800000"),
        ("0x1.fffffep127", "7f7fffff"),
        ("0x1.ffffffp127", "7f800000"),
        ("0x1.000001p0", "3f800000"),
        ("0x1.000003p0", "3f800002"),
        ("0x1.000001000000000000001p0", "3f800001"),
        // Java Float.valueOf's documented double-rounding counterexample.
        (
            "1.00000017881393421514957253748434595763683319091796875001d",
            "3f800001",
        ),
        ("3.5e38", "7f800000"),
        ("-1e-9999999999999999999999999", "80000000"),
    ] {
        assert_eq!(bits(Builtin::EFloat, text), expected, "{text}");
    }
    for (text, expected) in [
        ("0x1p-1074", "0000000000000001"),
        ("0x1p-1075", "0000000000000000"),
        ("0x1.0000000000000000001p-1075", "0000000000000001"),
        ("0x1p-1022", "0010000000000000"),
        ("0x0.fffffffffffffp-1022", "000fffffffffffff"),
        ("0x1.fffffffffffffp1023", "7fefffffffffffff"),
        ("0x1.fffffffffffff8p1023", "7ff0000000000000"),
        ("0x1.00000000000008p0", "3ff0000000000000"),
        ("0x1.0000000000000800001p0", "3ff0000000000001"),
        ("-0.0", "8000000000000000"),
        ("1e99999999999999999999", "7ff0000000000000"),
    ] {
        assert_eq!(bits(Builtin::EDouble, text), expected, "{text}");
    }
    assert_ne!(
        bits(Builtin::EDouble, "0.1f"),
        format!("{:016x}", f64::from(0.1_f32).to_bits())
    );
}

#[test]
fn float_lexical_policy_and_nonfinite_wire_values_are_explicit() {
    for codec in [
        Builtin::EFloat,
        Builtin::EDouble,
        Builtin::EFloatObject,
        Builtin::EDoubleObject,
    ] {
        for text in [
            "1",
            "1.",
            ".1",
            "1e+2",
            "+1.25F",
            "0X.8P+1d",
            "\0\t -0x1.p0 \r\n",
            "+NaN",
            "-NaN",
            "Infinity",
            "-Infinity",
        ] {
            assert!(codec.parse(text).is_ok(), "{codec:?}: {text:?}");
        }
        for text in [
            "",
            " ",
            ".",
            "1e",
            "1e+",
            "nan",
            "inf",
            "infinity",
            "NaNf",
            "InfinityD",
            "1_0",
            "1 0",
            "0x1",
            "0xp1",
            "0x.p1",
            "0x1p",
            "0x1p0p1",
            "0x1..2p1",
            "--1",
            "1ff",
            "\u{a0}1",
            "١.٢",
        ] {
            assert!(codec.parse(text).is_err(), "{codec:?}: {text:?}");
        }
        assert_eq!(
            codec
                .format(&codec.parse("-NaN").unwrap())
                .unwrap()
                .as_deref(),
            Some("NaN")
        );
        assert_eq!(
            codec
                .format(&codec.parse("-Infinity").unwrap())
                .unwrap()
                .as_deref(),
            Some("-Infinity")
        );
        for invalid in [
            json!(1.25),
            json!("NaN"),
            json!({"kind":"float32", "bits":"7fc00001"}),
            json!({"kind":"float64", "bits":"fff8000000000000"}),
            json!({"kind":"float32", "bits":"3F800000"}),
            json!({"kind":"float64", "bits":"0"}),
            json!({"kind":"float32", "bits":"3f800000", "extra":true}),
        ] {
            assert!(codec.validate(&invalid).is_err(), "{codec:?}: {invalid}");
        }
    }
    assert_eq!(
        Builtin::EFloatObject.default_value(),
        serde_json::Value::Null
    );
    assert_eq!(
        Builtin::EDoubleObject.default_value(),
        serde_json::Value::Null
    );
    assert_eq!(
        Builtin::EFloat.default_value(),
        Builtin::EFloat.parse("0").unwrap()
    );
    assert_eq!(
        Builtin::EDouble.default_value(),
        Builtin::EDouble.parse("0").unwrap()
    );
    assert!(
        Builtin::EFloat
            .validate(&Builtin::EDouble.parse("1").unwrap())
            .is_err()
    );
}

#[test]
fn canonical_hex_round_trips_ieee_samples_and_agrees_with_decimal_reference() {
    let mut state = 0x123456789abcdef_u64;
    for _ in 0..4096 {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        for single in [true, false] {
            let codec = if single {
                Builtin::EFloat
            } else {
                Builtin::EDouble
            };
            let (decimal, word, nan) = if single {
                let value = f32::from_bits(state as u32);
                (
                    value.to_string(),
                    format!("{:08x}", state as u32),
                    value.is_nan(),
                )
            } else {
                let value = f64::from_bits(state);
                (value.to_string(), format!("{state:016x}"), value.is_nan())
            };
            if nan {
                continue;
            }
            let wire = json!({"kind":if single {"float32"} else {"float64"}, "bits":word});
            let canonical = codec.format(&wire).unwrap().unwrap();
            assert_eq!(codec.parse(&canonical).unwrap(), wire, "{canonical}");
            // Finite decimal reference comes from the standard library formatter,
            // independently of the handwritten hexadecimal parser/formatter.
            if !decimal.contains("inf") {
                assert_eq!(codec.parse(&decimal).unwrap(), wire, "{decimal}");
            }
        }
    }
}

#[test]
fn very_long_significands_keep_sticky_bits_and_saturated_exponents() {
    let tail = "0".repeat(20_000);
    assert_eq!(
        bits(Builtin::EFloat, &format!("0x1.000001{tail}1p0")),
        "3f800001"
    );
    assert_eq!(
        bits(Builtin::EDouble, &format!("0x1{tail}p-80000")),
        "3ff0000000000000"
    );
    assert_eq!(
        bits(Builtin::EFloat, "0x1p999999999999999999999999"),
        "7f800000"
    );
    assert_eq!(
        bits(Builtin::EDouble, "-0x1p-99999999999999999999999"),
        "8000000000000000"
    );
}

#[test]
fn floating_operation_signatures_and_rcore_behavior_feature_types_keep_precision() {
    use intersect_model::definitions::{
        bundle::{DefinitionBundle, DefinitionSource, SourceKind},
        rcore_behavior::checking::check,
    };
    let model = DefinitionSource::new(
        "memory:float.ecore",
        SourceKind::Ecore,
        "<e:EPackage xmlns:e='http://www.eclipse.org/emf/2002/Ecore' xmlns:xsi='http://www.w3.org/2001/XMLSchema-instance' name='floats' nsURI='urn:floats'><eClassifiers xsi:type='e:EClass' name='Item'><eStructuralFeatures xsi:type='e:EAttribute' name='f' lowerBound='1' eType='e:EDataType http://www.eclipse.org/emf/2002/Ecore#//EFloat'/><eStructuralFeatures xsi:type='e:EAttribute' name='d' eType='e:EDataType http://www.eclipse.org/emf/2002/Ecore#//EDoubleObject'/><eOperations name='convert' eType='e:EDataType http://www.eclipse.org/emf/2002/Ecore#//EDouble'><eParameters name='f' eType='e:EDataType http://www.eclipse.org/emf/2002/Ecore#//EFloatObject'/></eOperations></eClassifiers></e:EPackage>",
    );
    let schema =
        intersect_model_acceptance::migration_fixture::compile_migrated_schema_fixture(&model.text)
            .unwrap();
    assert_eq!(
        schema.resolved_operations("Item").unwrap()[0]
            .signature
            .parameters,
        [
            intersect_model::definitions::operations::ParameterType::Builtin {
                name: "EFloatObject".into()
            }
        ]
    );
    let (model, _) = intersect_model_acceptance::migration_fixture::migrate_source_fixture(model);
    for (body, valid) in [
        (
            "fn f(item: m.Item) -> Float32 = item.f; fn d(item: m.Item) -> Float64? = item.d;",
            true,
        ),
        ("fn f(item: m.Item) -> Float64 = item.f;", false),
        ("fn d(item: m.Item) -> Float64 = item.d;", false),
    ] {
        let mut source = DefinitionSource::new(
            "memory:float-behavior.rcore",
            SourceKind::RcoreBehavior,
            format!(
                "rcore 1; package floats version \"1\"; import model \"urn:floats\" as m; {body}"
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
        assert_eq!(result.is_ok(), valid, "{result:?}");
    }
}
