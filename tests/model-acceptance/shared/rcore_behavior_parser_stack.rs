//! Complete pre-repair parser observations; same consumer in native and WASM.
use intersect_model::definitions::{bundle::DefinitionSource, rcore_behavior::parse};
use serde_json::Value;

pub fn verify() -> usize {
    let baseline: Value = serde_json::from_str(include_str!(
        "../../../crates/intersect-model/assets/fixtures/rcore-behavior-parser-stack-baseline.json"
    ))
    .unwrap();
    assert_eq!(baseline["version"], 1);
    let cases = baseline["cases"].as_array().unwrap();
    assert_eq!(cases.len(), 8);
    for case in cases {
        #[cfg(not(target_arch = "wasm32"))]
        eprintln!("public parser {}", case["id"]);
        let source: DefinitionSource = serde_json::from_value(case["source"].clone()).unwrap();
        assert_eq!(
            DefinitionSource::new(source.id.uri.clone(), source.kind, source.text.clone()).id,
            source.id
        );
        let actual = serde_json::to_value(parse(source)).unwrap();
        assert_eq!(actual, case["document"], "{}", case["id"]);
    }
    cases.len()
}
