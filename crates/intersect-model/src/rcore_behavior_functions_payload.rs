//! Runtime adapter for already shape-admitted immutable canonical JSON.
//! It creates no logical credits; output ownership is bounded by original
//! node/byte limits and a single affine arena debit. Map allocation/comparison,
//! reservation and destruction remain separately qualified atomic operations.
use super::*;
use intersect_model::definitions::retained_json::{Admission, Allocation};
use intersect_model::definitions::retained_json_frontier::CopyJob;
pub(super) struct JsonAdmission<'a> {
    pub limits: &'a ExecutionLimits,
    pub admitted_bytes: Option<usize>,
}
impl Admission for JsonAdmission<'_> {
    fn allocate(&mut self, request: Allocation) -> std::result::Result<(), String> {
        let node_limit = self.limits.max_value_nodes;
        let allowed = match request {
            Allocation::Frames {
                old_slots: 0,
                new_slots: 1,
                element_bytes,
            } if element_bytes == CopyJob::OWNER_BYTES => self.admitted_bytes.is_some(),
            Allocation::Frames {
                new_slots,
                element_bytes,
                ..
            } => new_slots <= node_limit && element_bytes == CopyJob::TASK_BYTES,
            Allocation::Array {
                elements,
                element_bytes,
            } => {
                self.admitted_bytes.is_some()
                    && elements <= node_limit
                    && element_bytes == std::mem::size_of::<serde_json::Value>()
            }
            Allocation::Object {
                entries,
                key_value_bytes,
            } => {
                self.admitted_bytes.is_some()
                    && entries <= node_limit
                    && key_value_bytes == std::mem::size_of::<(String, serde_json::Value)>()
            }
            Allocation::String { bytes, .. } => {
                self.admitted_bytes.is_some_and(|limit| bytes <= limit)
            }
            Allocation::Number { encoded_bytes } => self
                .admitted_bytes
                .is_some_and(|limit| encoded_bytes <= limit),
        };
        if allowed {
            Ok(())
        } else {
            Err("JSON ownership exceeds original source admission".into())
        }
    }
}
