use intersect_model::definitions::{
    compiler::standard_datatype,
    conversion::{
        ConversionContract, ConversionProvider, ConversionRegistry, ConvertedValue, ProviderId,
    },
};
use intersect_model::invocation_error_conversion::InvocationErrorConverter;
use serde_json::json;
fn converter() -> InvocationErrorConverter {
    InvocationErrorConverter::new(ConversionContract {
        members: Vec::new(),
        declarations: vec![],
        datatype: standard_datatype("EInvocationTargetException")
            .unwrap()
            .declaration()
            .clone(),
        provider: ProviderId {
            name: "ecore.invocation-error".into(),
            version: "1".into(),
        },
        serializable: false,
    })
    .unwrap()
}
#[test]
fn target_and_wrapper_messages_preserve_null_empty_unicode_and_exact_domain() {
    let converter = converter();
    for message in [json!(null), json!(""), json!("failure\n\"原因\"")] {
        for target in [
            json!(null),
            json!({"domain":{"name":"sysml.evaluate","version":"2"},"code":"constraint_failed","message":message}),
        ] {
            let value = json!({"message":message,"target":target});
            converter.validate(&value).unwrap();
            let descriptor = converter.descriptor(&value).unwrap();
            assert_eq!(serde_json::to_value(&descriptor).unwrap(), value);
            assert_eq!(serde_json::to_value(descriptor.cause()).unwrap(), target);
            let rendered = descriptor.diagnostic().unwrap();
            assert_eq!(rendered, descriptor.diagnostic().unwrap());
            assert_eq!(
                serde_json::from_str::<serde_json::Value>(
                    rendered.strip_prefix("InvocationTargetException ").unwrap()
                )
                .unwrap(),
                value
            );
            assert!(!rendered.contains('\n'));
        }
    }
}
#[test]
fn malformed_opaque_missing_and_extra_fields_reject() {
    let converter = converter();
    let good = json!({"message":null,"target":{"domain":{"name":"domain","version":"1"},"code":"failed","message":null}});
    for value in [
        json!(null),
        json!("java.lang.Exception"),
        json!({}),
        json!({"message":null}),
        json!({"target":null}),
        json!({"message":42,"target":null}),
        json!({"message":null,"target":{"javaObject":7}}),
    ] {
        assert!(converter.validate(&value).is_err(), "{value}");
    }
    for pointer in [
        "/target/domain/name",
        "/target/domain/version",
        "/target/code",
    ] {
        for blank in ["", " \n\t"] {
            let mut value = good.clone();
            *value.pointer_mut(pointer).unwrap() = json!(blank);
            assert!(converter.validate(&value).is_err());
        }
    }
    for pointer in ["", "/target", "/target/domain"] {
        let mut value = good.clone();
        value
            .pointer_mut(pointer)
            .unwrap()
            .as_object_mut()
            .unwrap()
            .insert("extra".into(), json!(true));
        assert!(converter.validate(&value).is_err());
    }
    let mut missing = good.clone();
    missing["target"].as_object_mut().unwrap().remove("message");
    assert!(converter.validate(&missing).is_err());
    let mut invalid = converter.contract().clone();
    invalid.serializable = true;
    assert!(InvocationErrorConverter::new(invalid).is_err());
    let mut invalid = converter.contract().clone();
    invalid.declarations.push(invalid.datatype.clone());
    assert!(InvocationErrorConverter::new(invalid).is_err());
}
#[test]
fn checked_snapshots_are_distinct_from_lexical_serialization_and_wrong_identity() {
    let converter = converter();
    let datatype = standard_datatype("EInvocationTargetException").unwrap();
    assert!(!datatype.serializable());
    assert_eq!(
        datatype.instance_class_name(),
        Some("java.lang.reflect.InvocationTargetException")
    );
    let mut registry = ConversionRegistry::default();
    registry.register(&datatype, &converter).unwrap();
    let payload = json!({"message":null,"target":null});
    let value = registry
        .bind(datatype.declaration(), payload.clone())
        .unwrap();
    registry.validate(&value).unwrap();
    assert!(registry.format(&value).is_err());
    assert!(registry.parse(datatype.declaration(), "anything").is_err());
    assert!(converter.parse("anything").is_err());
    assert!(converter.format(&payload).is_err());
    assert_eq!(converter.default_value().unwrap(), None);
    for pointer in ["/provider/version", "/datatype/source/sha256"] {
        let mut invalid = serde_json::to_value(&value).unwrap();
        *invalid.pointer_mut(pointer).unwrap() = json!("stale");
        let invalid: ConvertedValue = serde_json::from_value(invalid).unwrap();
        assert!(registry.validate(&invalid).is_err());
    }
    assert!(ConversionRegistry::default().validate(&value).is_err());
}
