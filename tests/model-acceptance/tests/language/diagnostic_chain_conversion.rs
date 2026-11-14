use intersect_model::definitions::{
    compiler::standard_datatype,
    conversion::{ConversionContract, ConversionProvider, ConversionRegistry, ProviderId},
};
use intersect_model::diagnostic_chain_conversion::{
    Diagnostic, DiagnosticChain, DiagnosticChainConverter, Severity,
};
use serde_json::{Value, json};
fn leaf(severity: &str, code: i32) -> Value {
    json!({"severity":severity,"source":null,"code":code,"message":"message","data":[],"children":[]})
}
fn converter() -> DiagnosticChainConverter {
    DiagnosticChainConverter::new(ConversionContract {
        members: Vec::new(),
        declarations: vec![],
        datatype: standard_datatype("EDiagnosticChain")
            .unwrap()
            .declaration()
            .clone(),
        provider: ProviderId {
            name: "ecore.diagnostic-chain".into(),
            version: "1".into(),
        },
        serializable: false,
    })
    .unwrap()
}
#[test]
fn add_add_all_and_merge_preserve_structure_order_duplicates_and_reported_severity() {
    let a = leaf("info", 1);
    let b = leaf("error", 2);
    let mut parent = leaf("warning", 3);
    parent["children"] = json!([a, b]);
    let parent = Diagnostic::from_wire(&parent).unwrap();
    let mut chain = DiagnosticChain::default();
    assert_eq!(chain.severity(), Severity::Ok);
    chain.add(parent.clone()).unwrap();
    assert_eq!(chain.diagnostics(), std::slice::from_ref(&parent));
    assert_eq!(chain.severity(), Severity::Warning);
    chain.add_all(parent.clone()).unwrap();
    chain.merge(parent.clone()).unwrap();
    assert_eq!(
        serde_json::to_value(&chain).unwrap()["diagnostics"],
        json!([parent, a, b, a, b])
    );
    assert_eq!(chain.severity(), Severity::Error);
    let before = chain.clone();
    chain.add_all(Diagnostic::from_wire(&a).unwrap()).unwrap();
    assert_eq!(chain, before);
    let cancel = Diagnostic::from_wire(&leaf("cancel", i32::MIN)).unwrap();
    chain.merge(cancel).unwrap();
    assert_eq!(chain.severity(), Severity::Cancel);
    let payload = serde_json::to_value(&chain).unwrap();
    assert_eq!(DiagnosticChain::from_wire(&payload).unwrap(), chain);
    // Invalid nested input must reject the whole append, with no partial mutation.
    let mut invalid = parent;
    invalid.children[1].data.push(
        serde_json::from_value(json!({"kind":"scalar","datatype":"EInt","value":2147483648u64}))
            .unwrap(),
    );
    for op in [
        DiagnosticChain::add,
        DiagnosticChain::add_all,
        DiagnosticChain::merge,
    ] {
        let before = chain.clone();
        assert!(op(&mut chain, invalid.clone()).is_err());
        assert_eq!(chain, before);
    }
}
#[test]
fn exact_scalar_reference_and_error_data_share_existing_validation() {
    let mut diagnostic = leaf("ok", i32::MAX);
    diagnostic["data"] = json!([
        {"kind":"scalar","datatype":"ELong","value":{"kind":"int64","value":"9223372036854775807"}},
        {"kind":"reference","resource":"model.sysml","revision":"sha256:exact","fragment":"part/0"},
        {"kind":"invocation_error","value":{"message":null,"target":{"domain":{"name":"sysml","version":"1"},"code":"invalid","message":""}}}
    ]);
    let checked = Diagnostic::from_wire(&diagnostic).unwrap();
    assert_eq!(serde_json::to_value(checked).unwrap(), diagnostic);
    for (pointer, bad) in [
        ("/data/0/value/value", json!("01")),
        ("/data/0/datatype", json!("Unknown")),
        ("/data/1/revision", json!(" ")),
        ("/data/2/value/target/domain/version", json!("")),
        ("/severity", json!("fatal")),
        ("/code", json!(2147483648u64)),
    ] {
        let mut invalid = diagnostic.clone();
        *invalid.pointer_mut(pointer).unwrap() = bad;
        assert!(Diagnostic::from_wire(&invalid).is_err(), "{invalid}");
    }
    for severity in ["ok", "info", "warning", "error", "cancel"] {
        Diagnostic::from_wire(&leaf(severity, 0)).unwrap();
    }
    for missing in ["source", "message", "code", "data", "children"] {
        let mut invalid = diagnostic.clone();
        invalid.as_object_mut().unwrap().remove(missing);
        assert!(Diagnostic::from_wire(&invalid).is_err());
    }
    for pointer in ["", "/data/1", "/data/2/value"] {
        let mut invalid = diagnostic.clone();
        invalid
            .pointer_mut(pointer)
            .unwrap()
            .as_object_mut()
            .unwrap()
            .insert("hostObject".into(), json!(1));
        assert!(Diagnostic::from_wire(&invalid).is_err());
    }
    for value in [
        json!(null),
        json!({}),
        json!({"diagnostics":null}),
        json!({"diagnostics":[],"extra":true}),
        json!({"diagnostics":[{"javaObject":1}]}),
    ] {
        assert!(DiagnosticChain::from_wire(&value).is_err());
    }
}
#[test]
fn adapter_requires_exact_nonserializable_contract_and_never_creates_implicit_chain() {
    let converter = converter();
    let datatype = standard_datatype("EDiagnosticChain").unwrap();
    assert!(!datatype.serializable());
    assert_eq!(
        datatype.instance_class_name(),
        Some("org.eclipse.emf.common.util.DiagnosticChain")
    );
    let mut registry = ConversionRegistry::default();
    registry.register(&datatype, &converter).unwrap();
    let value = registry
        .bind(datatype.declaration(), json!({"diagnostics":[]}))
        .unwrap();
    registry.validate(&value).unwrap();
    assert!(registry.format(&value).is_err());
    assert!(registry.parse(datatype.declaration(), "[]").is_err());
    assert!(converter.parse("[]").is_err());
    assert!(converter.format(&json!({"diagnostics":[]})).is_err());
    assert_eq!(converter.default_value().unwrap(), None);
    assert!(registry.bind(datatype.declaration(), Value::Null).is_err());
    assert!(ConversionRegistry::default().validate(&value).is_err());
    let mut invalid = converter.contract().clone();
    invalid.serializable = true;
    assert!(DiagnosticChainConverter::new(invalid).is_err());
    let mut invalid = converter.contract().clone();
    invalid.declarations.push(invalid.datatype.clone());
    assert!(DiagnosticChainConverter::new(invalid).is_err());
}
