//! Full canonical JSONL row/stream assertions against immutable original pins.
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::io::Write;
pub(super) struct FrozenWriter {
    name: String,
    expected: Vec<String>,
    expected_stream: String,
    buffer: Vec<u8>,
    stream: Sha256,
    index: usize,
}
impl FrozenWriter {
    pub(super) fn new(name: &str) -> Self {
        let pins: Value = serde_json::from_str(include_str!(
            "generics_decoder_inventory_original_full_hashes.json"
        ))
        .unwrap();
        let row = &pins[name];
        Self {
            name: name.into(),
            expected: serde_json::from_value(row["rows"].clone()).unwrap(),
            expected_stream: row["sha256"].as_str().unwrap().into(),
            buffer: vec![],
            stream: Sha256::new(),
            index: 0,
        }
    }
    pub(super) fn row(&mut self, row: &Value) {
        self.write_all(&serde_json::to_vec(row).unwrap()).unwrap();
        self.write_all(b"\n").unwrap();
    }
    pub(super) fn finish(self) {
        assert!(self.buffer.is_empty(), "{} complete final row", self.name);
        assert_eq!(self.index, self.expected.len(), "{} row count", self.name);
        assert_eq!(
            format!("{:x}", self.stream.finalize()),
            self.expected_stream,
            "{} full serialized stream",
            self.name
        );
    }
}
impl Write for FrozenWriter {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.stream.update(bytes);
        for byte in bytes {
            if *byte == b'\n' {
                let digest = format!("{:x}", Sha256::digest(&self.buffer));
                assert_eq!(
                    Some(&digest),
                    self.expected.get(self.index),
                    "{} complete row{}",
                    self.name,
                    self.index
                );
                self.index += 1;
                self.buffer.clear();
            } else {
                self.buffer.push(*byte);
            }
        }
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
