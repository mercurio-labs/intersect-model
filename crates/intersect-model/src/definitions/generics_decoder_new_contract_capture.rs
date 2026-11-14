//! Portable complete-output pins frozen from the immutable first D capture.
use serde_json::Value;
use sha2::{Digest, Sha256};

pub(super) struct Capture {
    name: String,
    expected: Vec<String>,
    expected_stream: String,
    index: usize,
    stream: Sha256,
}
impl Capture {
    pub(super) fn new(name: &str) -> Self {
        let pins: Value = serde_json::from_str(include_str!(
            "generics_decoder_new_contract_full_hashes.json"
        ))
        .unwrap();
        let selected = &pins[name];
        Self {
            name: name.into(),
            expected: selected["rows"]
                .as_array()
                .unwrap()
                .iter()
                .map(|value| value.as_str().unwrap().to_owned())
                .collect(),
            expected_stream: selected["complete_jsonl_sha256"].as_str().unwrap().into(),
            index: 0,
            stream: Sha256::new(),
        }
    }
    pub(super) fn row(&mut self, row: &Value) {
        let bytes = serde_json::to_vec(row).unwrap();
        let digest = format!("{:x}", Sha256::digest(&bytes));
        assert_eq!(
            Some(&digest),
            self.expected.get(self.index),
            "{} complete row {}",
            self.name,
            self.index
        );
        self.stream.update(&bytes);
        self.stream.update(b"\n");
        self.index += 1;
    }
    pub(super) fn finish(self) {
        assert_eq!(self.index, self.expected.len(), "{} row count", self.name);
        assert_eq!(
            format!("{:x}", self.stream.finalize()),
            self.expected_stream,
            "{} complete JSONL stream",
            self.name
        );
    }
}
pub(super) fn positive_events(row: &Value) -> Vec<Value> {
    row["events"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|event| !(event[1] == 0 && (event[0] == "c" || event[0] == "w")))
        .cloned()
        .collect()
}
