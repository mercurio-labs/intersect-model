use intersect_model::definitions::scalars::Builtin;
use serde_json::{Value, json};

#[test]
fn bytes_use_ecore_odd_nibble_rule_and_canonical_uppercase_wire() {
    let codec = Builtin::EByteArray;
    for (text, hex) in [
        ("", ""),
        ("a", "0A"),
        ("AbC", "AB0C"),
        ("12345", "123405"),
        ("00ff80", "00FF80"),
    ] {
        let wire = codec.parse(text).unwrap();
        assert_eq!(wire, json!({"kind":"bytes", "hex":hex}));
        assert_eq!(codec.format(&wire).unwrap().as_deref(), Some(hex));
        assert_eq!(codec.parse(hex).unwrap(), wire);
    }
    let all: String = (0..=255).map(|byte| format!("{byte:02x}")).collect();
    assert_eq!(
        codec.format(&codec.parse(&all).unwrap()).unwrap().unwrap(),
        all.to_ascii_uppercase()
    );
    for text in [" 00", "00\n", "0x12", "GG", "１２", "🙂"] {
        assert!(codec.parse(text).is_err());
    }
    for wire in [
        json!([]),
        json!("00"),
        json!({"kind":"bytes", "hex":"a0"}),
        json!({"kind":"bytes", "hex":"A"}),
        json!({"kind":"bytes", "hex":"GG"}),
        json!({"kind":"bytes", "hex":"00", "extra":true}),
    ] {
        assert!(codec.validate(&wire).is_err());
    }
    assert_eq!(codec.default_value(), Value::Null);
    assert_ne!(codec.parse("").unwrap(), Value::Null);
}

#[test]
fn char_numeric_cast_fallback_and_full_utf16_domain_are_exact() {
    for codec in [Builtin::EChar, Builtin::ECharacterObject] {
        for (text, unit) in [
            ("65", 65),
            ("-1", 65535),
            ("65536", 0),
            ("2147483647", 65535),
            ("-2147483648", 0),
            ("2147483648", 50),
            ("-2147483649", 45),
            ("A", 65),
            ("Abc", 65),
            (" 65", 32),
            ("+", 43),
            ("６５", 65),
            ("🙂", 0xd83d),
        ] {
            assert_eq!(
                codec.parse(text).unwrap(),
                json!({"kind":"char16", "unit":unit}),
                "{text}"
            );
        }
        assert!(codec.parse("").is_err());
        for wire in [
            json!(65),
            json!("A"),
            json!({"kind":"char16", "unit":-1}),
            json!({"kind":"char16", "unit":65536}),
            json!({"kind":"char16", "unit":65.0}),
            json!({"kind":"char16", "unit":65,"extra":true}),
        ] {
            assert!(codec.validate(&wire).is_err());
        }
    }
    for unit in 0..=u16::MAX {
        let wire = json!({"kind":"char16", "unit":unit});
        let restored: Value = serde_json::from_str(&serde_json::to_string(&wire).unwrap()).unwrap();
        let text = Builtin::EChar.format(&restored).unwrap().unwrap();
        assert_eq!(text, unit.to_string());
        assert_eq!(Builtin::EChar.parse(&text).unwrap(), wire);
    }
    assert_eq!(
        Builtin::EChar.default_value(),
        json!({"kind":"char16", "unit":0})
    );
    assert_eq!(Builtin::ECharacterObject.default_value(), Value::Null);
    assert!(Builtin::EChar.validate(&Value::Null).is_err());
}

#[test]
fn binary_domains_have_shared_nominal_defaults_operations_and_rcore_behavior_reads() {
    use intersect_model::definitions::{
        bundle::{DefinitionBundle, DefinitionSource, SourceKind},
        rcore_behavior::checking::check,
    };
    for (builtin, instance, scalar, text) in [
        ("EDate", "java.util.Date", "DateInstant", "1970-01-01"),
        ("EByteArray", "byte[]", "Bytes", "ABC"),
        ("EChar", "char", "Char16", "55357"),
        ("ECharacterObject", "java.lang.Character", "Char16", "65"),
    ] {
        let xml = format!(
            "<e:EPackage xmlns:e='http://www.eclipse.org/emf/2002/Ecore' xmlns:xsi='http://www.w3.org/2001/XMLSchema-instance' name='binary' nsURI='urn:binary'><eClassifiers xsi:type='e:EDataType' name='Declared' instanceClassName='{instance}'/><eClassifiers xsi:type='e:EClass' name='Item'><eStructuralFeatures xsi:type='e:EAttribute' name='value' lowerBound='1' eType='e:EDataType http://www.eclipse.org/emf/2002/Ecore#//{builtin}' defaultValueLiteral='{text}'/><eStructuralFeatures xsi:type='e:EAttribute' name='declared' lowerBound='1' eType='#//Declared' defaultValueLiteral='{text}'/><eOperations name='f'><eParameters name='x' eType='#//Declared'/></eOperations><eOperations name='f'><eParameters name='x' eType='e:EDataType http://www.eclipse.org/emf/2002/Ecore#//{builtin}'/></eOperations></eClassifiers></e:EPackage>"
        );
        let schema =
            intersect_model_acceptance::migration_fixture::compile_migrated_schema_fixture(&xml)
                .unwrap();
        assert_eq!(schema.resolved_operations("Item").unwrap().len(), 2);
        for field in schema.classes["Item"].features.values() {
            schema.validate_attribute(field, &field.default).unwrap();
        }
        let model = DefinitionSource::new("memory:binary.ecore", SourceKind::Ecore, xml);
        let (model, _) =
            intersect_model_acceptance::migration_fixture::migrate_source_fixture(model);
        for (body, ok) in [
            (format!("fn f(x: m.Item) -> {scalar} = x.value;"), true),
            ("fn f(x: m.Item) -> m.Declared = x.declared;".into(), true),
            ("fn f(x: m.Item) -> String = x.value;".into(), false),
            ("fn f(x: m.Item) -> UInt16 = x.value;".into(), false),
            (
                format!("fn f(x: m.Item) -> {scalar} = x.value + x.value;"),
                false,
            ),
        ] {
            let mut source = DefinitionSource::new(
                "memory:binary-behavior.rcore",
                SourceKind::RcoreBehavior,
                format!(
                    "rcore 1; package binary version \"1\"; import model \"urn:binary\" as m; {body}"
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
    }
}
