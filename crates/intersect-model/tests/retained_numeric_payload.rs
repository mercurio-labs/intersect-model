//! Portable default numeric source/owner lifecycle consumers. One test keeps
//! verification-only allocator pointer observations isolated in this process.
#![cfg(feature = "allocation-profile")]
#[path = "../../../fixtures/rcore-numeric-payload/support/lifecycle.rs"]
mod lifecycle;
#[path = "../../../fixtures/rcore-numeric-payload/support/typed.rs"]
mod typed;
#[test]
fn default_numeric_sources_keep_complete_fees_errors_and_owner_lifecycles() {
    let typed = typed::capture();
    let expected: serde_json::Value = serde_json::from_str(include_str!(
        "../../../fixtures/rcore-numeric-payload/expected/typed-session.json"
    ))
    .unwrap();
    assert_eq!(
        typed, expected,
        "complete current source/results/fees/cadence"
    );
    let mut before: serde_json::Value = serde_json::from_str(include_str!(
        "../../../fixtures/rcore-numeric-payload/before/typed-session.json"
    ))
    .unwrap();
    let mut semantic = typed.clone();
    // The frozen BEFORE is retained in full. Only Pending counts are a changed
    // scheduling observation; every complete semantic field remains required.
    for value in [&mut before, &mut semantic] {
        for row in value["rows"].as_array_mut().unwrap() {
            if row["operation"] == "evaluation" {
                row.as_object_mut().unwrap().remove("pending");
            }
        }
    }
    assert_eq!(
        semantic, before,
        "all original complete outcomes/fees/error priorities"
    );
    let lifecycle = lifecycle::capture();
    let expected: serde_json::Value = serde_json::from_str(include_str!(
        "../../../fixtures/rcore-scalar-comparison/numeric-lifecycle-after.json"
    ))
    .unwrap();
    assert_eq!(
        lifecycle, expected,
        "actual selected numeric Pending cancel/drop/root-owner/recovery"
    );
    let mut before: serde_json::Value = serde_json::from_str(include_str!(
        "../../../fixtures/rcore-numeric-payload/expected/lifecycle.json"
    ))
    .unwrap();
    let mut semantic = lifecycle;
    // Keep the complete original golden. The shared retained comparison adds
    // scheduling polls; every other outcome, fee, phase and owner field is exact.
    // The complete new cadence is asserted against its separate golden above.
    for value in [&mut before, &mut semantic] {
        for row in value["rows"].as_array_mut().unwrap() {
            row.as_object_mut().unwrap().remove("polls");
        }
    }
    assert_eq!(
        semantic, before,
        "all original lifecycle fields except polling cadence"
    );
}
