use intersect_model::definitions::scalars::Builtin;
use serde_json::{Value, json};

#[test]
fn arbitrary_integers_keep_exact_digits_and_canonical_wire_without_machine_limits() {
    let codec = Builtin::EBigInteger;
    assert_eq!(codec.default_value(), Value::Null);
    assert_eq!(codec.format(&Value::Null).unwrap(), None);
    for (text, expected) in [
        ("+００١२", "12"),
        ("-000", "0"),
        ("-١٢", "-12"),
        ("18446744073709551616", "18446744073709551616"),
        ("-9223372036854775809", "-9223372036854775809"),
    ] {
        let value = codec.parse(text).unwrap();
        assert_eq!(value, json!({"kind":"bigint", "value":expected}));
        assert_eq!(codec.format(&value).unwrap().as_deref(), Some(expected));
    }
    for sign in ["", "-"] {
        let text = format!("{sign}{}", "1234567890".repeat(2000));
        let value = codec.parse(&text).unwrap();
        let restored: Value =
            serde_json::from_str(&serde_json::to_string(&value).unwrap()).unwrap();
        assert_eq!(codec.format(&restored).unwrap().unwrap(), text);
    }
    for text in [
        "", "+", "-", "1.0", "1e3", "0x10", " 1", "1\n", "𝟙", "²", "−1",
    ] {
        assert!(codec.parse(text).is_err(), "{text}");
    }
    for text in ["+1", "01", "-0", "١", "", "1.0"] {
        assert!(
            codec
                .validate(&json!({"kind":"bigint", "value":text}))
                .is_err()
        );
    }
    for value in [
        json!(12),
        json!("12"),
        json!({"kind":"int64", "value":"12"}),
        json!({"kind":"bigint", "value":12}),
        json!({"kind":"bigint", "value":"12", "extra":true}),
    ] {
        assert!(codec.validate(&value).is_err());
    }
}

#[test]
fn standard_and_nominal_big_integer_features_have_distinct_operation_and_rcore_behavior_types() {
    use intersect_model::definitions::{
        bundle::{DefinitionBundle, DefinitionSource, SourceKind},
        rcore_behavior::checking::check,
    };
    let original = "<e:EPackage xmlns:e='http://www.eclipse.org/emf/2002/Ecore' xmlns:xsi='http://www.w3.org/2001/XMLSchema-instance' name='big' nsURI='urn:big'><eClassifiers xsi:type='e:EDataType' name='Count' instanceClassName='java.math.BigInteger'/><eClassifiers xsi:type='e:EClass' name='Item'><eStructuralFeatures xsi:type='e:EAttribute' name='value' lowerBound='1' eType='e:EDataType http://www.eclipse.org/emf/2002/Ecore#//EBigInteger' defaultValueLiteral='18446744073709551616'/><eStructuralFeatures xsi:type='e:EAttribute' name='count' lowerBound='1' eType='#//Count' defaultValueLiteral='18446744073709551616'/><eOperations name='f'><eParameters name='p' eType='#//Count'/></eOperations><eOperations name='f'><eParameters name='p' eType='e:EDataType http://www.eclipse.org/emf/2002/Ecore#//EBigInteger'/></eOperations></eClassifiers></e:EPackage>";
    for (builtin, instance, scalar, literal) in [
        (
            "EBigInteger",
            "java.math.BigInteger",
            "BigInt",
            "18446744073709551616",
        ),
        (
            "EBigDecimal",
            "java.math.BigDecimal",
            "Decimal",
            "18446744073709551616.00",
        ),
    ] {
        let xml = original
            .replace("EBigInteger", builtin)
            .replace("java.math.BigInteger", instance)
            .replace("18446744073709551616", literal);
        let schema =
            intersect_model_acceptance::migration_fixture::compile_migrated_schema_fixture(&xml)
                .unwrap();
        assert_eq!(schema.resolved_operations("Item").unwrap().len(), 2);
        for field in schema.classes["Item"].features.values() {
            schema.validate_attribute(field, &field.default).unwrap();
        }
        let model = DefinitionSource::new("memory:big.ecore", SourceKind::Ecore, xml);
        let (model, _) =
            intersect_model_acceptance::migration_fixture::migrate_source_fixture(model);
        for (body, valid) in [
            ("fn f(x: m.Item) -> BigInt = x.value;", true),
            ("fn f(x: m.Item) -> m.Count = x.count;", true),
            ("fn f(x: m.Item) -> Int64 = x.value;", false),
            ("fn f(x: m.Item) -> BigInt = x.count;", false),
        ] {
            let mut source = DefinitionSource::new(
                "memory:big-behavior.rcore",
                SourceKind::RcoreBehavior,
                format!(
                    "rcore 1; package big version \"1\"; import model \"urn:big\" as m; {body}"
                )
                .replace("BigInt", scalar),
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
}
