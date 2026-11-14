//! Real providers through ordinary borrowed/owned decoder sessions.
#![cfg(all(feature = "allocation-profile", target_pointer_width = "64"))]
#[path = "../../../fixtures/rcore-retained-provider/driver.rs"]
mod driver;
#[test]
fn exact_provider_failures_limits_current_control_and_release() {
    let actual = driver::observe();
    let expected: serde_json::Value = serde_json::from_str(include_str!(
        "../../../fixtures/rcore-retained-provider/expected/native64.json"
    ))
    .unwrap();
    assert_eq!(actual["rows"].as_array().unwrap().len(), 552);
    assert_eq!(actual, expected);
}
