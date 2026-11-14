//! Original source graphs, full before outcomes and actual shared runtime consumers.
#[cfg(feature = "allocation-profile")]
#[path = "../../../fixtures/rcore-retained-json/support/lifecycle.rs"]
mod lifecycle;
#[path = "../../../fixtures/rcore-retained-json/support/sessions.rs"]
mod sessions;

#[test]
fn source_backed_general_json_sessions_preserve_complete_before_outcomes() {
    let before: serde_json::Value = serde_json::from_str(include_str!(
        "../../../fixtures/rcore-retained-json/before-session.json"
    ))
    .unwrap();
    let current = sessions::observe();
    let before = before["rows"].as_array().unwrap();
    let current = current["rows"].as_array().unwrap();
    assert_eq!(before.len(), 54);
    assert_eq!(current.len(), 54);
    for (a, b) in before.iter().zip(current) {
        let mut a = a.clone();
        let mut b = b.clone();
        a.as_object_mut().unwrap().remove("polls");
        b.as_object_mut().unwrap().remove("polls");
        assert_eq!(a, b);
    }
}
#[cfg(feature = "allocation-profile")]
#[test]
fn retained_scalar_hex_boundaries_cancel_drop_and_retry() {
    let value = lifecycle::observe();
    assert_eq!(value["rows"].as_array().unwrap().len(), 24);
}
