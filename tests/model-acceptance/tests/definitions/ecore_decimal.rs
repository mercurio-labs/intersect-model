use intersect_model::definitions::scalars::Builtin;
use serde_json::{Value, json};

#[test]
fn exact_decimal_coefficients_scales_and_canonical_formatting() {
    let codec = Builtin::EBigDecimal;
    for (text, coefficient, scale, canonical) in [
        ("1.0", "10", 1, "1.0"),
        ("1.00", "100", 2, "1.00"),
        ("+００١.٢٠", "120", 2, "1.20"),
        ("-0.000", "0", 3, "0.000"),
        ("123.", "123", 0, "123"),
        (".123", "123", 3, "0.123"),
        ("0.000001", "1", 6, "0.000001"),
        ("0.0000001", "1", 7, "1E-7"),
        ("123e2", "123", -2, "1.23E+4"),
        ("0e3", "0", -3, "0E+3"),
        ("-1.2300e+٢", "-12300", 2, "-123.00"),
        ("1e2147483647", "1", -2147483647, "1E+2147483647"),
        ("1e-2147483647", "1", 2147483647, "1E-2147483647"),
        ("1.0e-2147483646", "10", 2147483647, "1.0E-2147483646"),
    ] {
        let value = codec.parse(text).unwrap();
        assert_eq!(
            value,
            json!({"kind":"decimal", "coefficient":coefficient, "scale":scale}),
            "{text}"
        );
        assert_eq!(codec.format(&value).unwrap().as_deref(), Some(canonical));
        assert_eq!(codec.parse(canonical).unwrap(), value);
    }
    assert_ne!(codec.parse("1.0").unwrap(), codec.parse("1.00").unwrap());
    assert_eq!(codec.default_value(), Value::Null);
    assert_eq!(codec.format(&Value::Null).unwrap(), None);
    let digits = "1234567890".repeat(2000);
    let value = codec.parse(&format!("-{digits}.00")).unwrap();
    assert_eq!(value["coefficient"], format!("-{digits}00"));
    assert_eq!(
        codec.format(&value).unwrap().unwrap(),
        format!("-{digits}.00")
    );
}

#[test]
fn decimal_rejects_bad_lexical_forms_and_forged_wire_values() {
    let codec = Builtin::EBigDecimal;
    for text in [
        "",
        ".",
        "+",
        "-",
        "1..0",
        "1e",
        "1e+",
        "1e1e1",
        "1_0",
        "NaN",
        "Infinity",
        "0x1p0",
        "1f",
        " 1",
        "1\n",
        "𝟙",
        "1e𝟙",
        "1e2147483648",
        "1e-2147483648",
        "1.0e-2147483647",
        "1e9999999999999999999999999",
    ] {
        assert!(codec.parse(text).is_err(), "{text}");
    }
    assert_eq!(
        codec.parse(&format!("1e{}1", "0".repeat(2000))).unwrap()["scale"],
        -1
    );
    for value in [
        json!(1.0),
        json!("1.0"),
        json!({"kind":"decimal", "coefficient":"01", "scale":0}),
        json!({"kind":"decimal", "coefficient":"-0", "scale":0}),
        json!({"kind":"decimal", "coefficient":"١", "scale":0}),
        json!({"kind":"decimal", "coefficient":"1", "scale":2147483648_i64}),
        json!({"kind":"decimal", "coefficient":"1", "scale":1.0}),
        json!({"kind":"decimal", "coefficient":"1", "scale":0, "extra":true}),
        json!({"kind":"bigint", "value":"1"}),
    ] {
        assert!(codec.validate(&value).is_err(), "{value}");
    }
}

#[test]
fn full_wire_scale_domain_preserves_java_canonical_exponent_asymmetry() {
    let codec = Builtin::EBigDecimal;
    for (coefficient, scale, canonical) in [
        ("1", i32::MIN, "1E+2147483648"),
        ("10", -2147483647, "1.0E+2147483648"),
    ] {
        let wire = json!({"kind":"decimal", "coefficient":coefficient, "scale":scale});
        codec.validate(&wire).unwrap();
        let restored: Value = serde_json::from_str(&serde_json::to_string(&wire).unwrap()).unwrap();
        assert_eq!(codec.format(&restored).unwrap().as_deref(), Some(canonical));
        assert!(codec.parse(canonical).is_err());
    }
    assert_eq!(
        codec.parse("10e2147483647").unwrap(),
        json!({"kind":"decimal", "coefficient":"10", "scale":-2147483647})
    );
    for coefficient in ["0", "1", "-10", "10000", "-987654321"] {
        for scale in [-100, -1, 0, 1, 5, 6, 7, 20, 100, i32::MAX] {
            let wire = json!({"kind":"decimal", "coefficient":coefficient, "scale":scale});
            assert_eq!(
                codec.parse(&codec.format(&wire).unwrap().unwrap()).unwrap(),
                wire
            );
        }
    }
}
