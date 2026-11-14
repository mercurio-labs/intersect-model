//! Preserve original outcomes and fixed schedules; add independently witnessed input-copy prefixes.
#[path = "../../../fixtures/rcore-input-string-transport/native/src/driver.rs"]
mod driver;
use serde_json::Value;

#[test]
fn retained_input_preserves_original_outcomes_and_admitted_cancel_prefixes() {
    let requests: Vec<Value> =
        include_str!("../../../fixtures/rcore-input-string-transport/requests.jsonl")
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
    let original: Vec<Value> = serde_json::from_str(include_str!(
        "../../../fixtures/rcore-input-string-transport/original-terminals.json"
    ))
    .unwrap();
    let plan: Value = serde_json::from_str(include_str!(
        "../../../fixtures/rcore-input-string-transport/exact-plan.json"
    ))
    .unwrap();
    let witness: Value = serde_json::from_str(include_str!(
        "../../../fixtures/rcore-input-string-transport/typed-phase-witness.json"
    ))
    .unwrap();
    assert_eq!(witness["original_controls"].as_array().unwrap().len(), 48);
    assert_eq!(witness["copy_controls"].as_array().unwrap().len(), 48);
    assert_eq!(witness["source_records"].as_array().unwrap().len(), 8);
    assert_eq!(
        witness["accepted_retained_record_sha256"],
        "cd6b87588d1dc8e76765977951860d40d18c6155f9b1d6764c9a203e225220fd"
    );
    assert_eq!(requests.len(), 282);
    assert_eq!(requests.len(), original.len());
    let mut cancellation_prefixes = 0;
    let mut pending_drops = 0;
    for (request, before) in requests.iter().cloned().zip(original.iter()) {
        let observation = driver::run(serde_json::from_value(request.clone()).unwrap()).unwrap();
        assert_eq!(observation["case"], before["case"]);
        assert_eq!(observation["source_ids"], before["source_ids"]);
        assert_eq!(observation["dropped"], before["dropped"]);
        assert_eq!(observation["finished"], before["finished"]);
        let actual: Vec<_> = observation["events"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|event| event.get("complete").cloned())
            .collect();
        if request["cancel"].is_null() {
            assert_eq!(
                Value::Array(actual.clone()),
                before["complete"],
                "{}",
                request["case"]
            );
        } else {
            assert_eq!(actual.len(), 1);
            let mut prior = before["complete"][0].clone();
            let mut current = actual[0].clone();
            let consumed = current["Err"]
                .as_object_mut()
                .unwrap()
                .remove("consumed")
                .unwrap();
            prior["Err"].as_object_mut().unwrap().remove("consumed");
            assert_eq!(current, prior, "{}", request["case"]);
            let proof = witness["original_controls"]
                .as_array()
                .unwrap()
                .iter()
                .find(|p| p["request"]["case"] == request["case"])
                .unwrap();
            assert_eq!(proof["request"], request);
            assert_eq!(observation, proof["observation"], "{}", request["case"]);
            assert_eq!(
                consumed,
                proof["observation"]["events"]
                    .as_array()
                    .unwrap()
                    .last()
                    .unwrap()["complete"]["Err"]["consumed"]
            );
            cancellation_prefixes += 1;
        }
        if !request["drop_at"].is_null() {
            assert!(actual.is_empty());
            assert!(
                observation["events"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .all(|event| event["pending"] == true)
            );
            pending_drops += 1;
        }
    }
    assert_eq!(cancellation_prefixes, 24);
    assert_eq!(pending_drops, 24);
    let mut copy_cancellations = 0;
    let mut copy_drops = 0;
    for proof in witness["copy_controls"].as_array().unwrap() {
        let index = requests
            .iter()
            .position(|r| r["case"] == proof["original_request"]["case"])
            .unwrap();
        let original_request = &requests[index];
        assert_eq!(*original_request, proof["original_request"]);
        let action = if original_request["cancel"].is_null() {
            "drop_at"
        } else {
            "cancel"
        };
        let mut unchanged = proof["request"].clone();
        unchanged[action] = original_request[action].clone();
        assert_eq!(
            unchanged, *original_request,
            "Only the observed phase poll may change"
        );
        assert_eq!(proof["phase"][0], "decode");
        assert_eq!(proof["phase"][1], "string-copy");
        let offset = proof["phase"][2][7].as_u64().unwrap();
        let length = proof["phase"][2][6].as_u64().unwrap();
        assert!(offset > 0 && offset < length);
        let observation =
            driver::run(serde_json::from_value(proof["request"].clone()).unwrap()).unwrap();
        assert_eq!(observation, proof["observation"]);
        let complete: Vec<_> = observation["events"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|e| e.get("complete").cloned())
            .collect();
        // The pre-retained historical recording publishes a node earlier.
        // Use the independently pinned accepted retained-String record here;
        // keep all actual and expected error/fee fields in the comparison.
        assert_eq!(Value::Array(complete.clone()), proof["accepted_complete"]);
        if action == "cancel" {
            let formula = plan["formulas"]
                .as_array()
                .unwrap()
                .iter()
                .find(|f| f["id"] == original_request["id"])
                .unwrap();
            assert_eq!(
                complete[0]["Err"]["consumed"]["value_bytes"],
                formula["decoder_admission"]
            );
            copy_cancellations += 1;
        } else {
            assert!(observation["dropped"].as_bool().unwrap());
            assert!(
                observation["events"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .all(|e| e["pending"] == true)
            );
            copy_drops += 1;
        }
    }
    assert_eq!(copy_cancellations, 24);
    assert_eq!(copy_drops, 24);
}
