//! Original scalar-size policy over the one shared general JSON frontier.
use crate::definitions::retained_json::{Admission, Allocation, Inspection, ResourceCursor};
use serde_json::Value;

struct SizePolicy {
    remaining: usize,
    object_cost: usize,
}
impl SizePolicy {
    fn spend(&mut self, amount: usize) -> Result<(), String> {
        self.remaining = self
            .remaining
            .checked_sub(amount)
            .ok_or("Scalar value byte limit exceeded")?;
        Ok(())
    }
}
impl Inspection for SizePolicy {
    fn node(&mut self, value: &Value, depth: usize, _: usize) -> Result<(), String> {
        if depth > 64 {
            return Err("Scalar payload nesting limit exceeded".into());
        }
        let cost = match value {
            Value::String(text) => text.len().saturating_add(2),
            Value::Array(items) => items.len().saturating_add(2),
            Value::Object(items) if !items.is_empty() => {
                self.object_cost = 2;
                return Ok(());
            }
            Value::Object(_) => 2,
            _ => 32,
        };
        self.spend(cost)
    }
    fn key(&mut self, key: &str) -> Result<(), String> {
        self.object_cost = self
            .object_cost
            .checked_add(key.len().saturating_add(4))
            .ok_or("Value size overflow")?;
        Ok(())
    }
    fn object_keys_finished(&mut self) -> Result<(), String> {
        self.spend(self.object_cost)
    }
}
pub(crate) struct Scratch {
    max_slots: usize,
    reused: bool,
}
impl Scratch {
    pub(crate) fn new(max_slots: usize) -> Self {
        Self {
            max_slots,
            reused: false,
        }
    }
}
impl Admission for Scratch {
    fn allocate(&mut self, request: Allocation) -> Result<(), String> {
        match request {
            Allocation::Frames {
                new_slots,
                element_bytes,
                ..
            } if !self.reused
                && new_slots <= self.max_slots
                && element_bytes == std::mem::size_of::<(&Value, usize)>() =>
            {
                Ok(())
            }
            _ => Err("Scalar size frontier exceeds original byte admission".into()),
        }
    }
}
pub(crate) struct SizeCursor<'a> {
    cursor: ResourceCursor<'a>,
    policy: SizePolicy,
    scratch: Scratch,
}
impl<'a> SizeCursor<'a> {
    pub(crate) fn new(value: &'a Value, max_bytes: usize) -> Self {
        Self {
            cursor: ResourceCursor::new(value),
            policy: SizePolicy {
                remaining: max_bytes,
                object_cost: 0,
            },
            scratch: Scratch::new(max_bytes),
        }
    }
    pub(crate) fn from_completed(
        mut cursor: ResourceCursor<'a>,
        value: &'a Value,
        max_bytes: usize,
    ) -> Result<Self, String> {
        cursor.restart(value)?;
        Ok(Self {
            cursor,
            policy: SizePolicy {
                remaining: max_bytes,
                object_cost: 0,
            },
            scratch: Scratch {
                max_slots: max_bytes,
                reused: true,
            },
        })
    }
    #[cfg(feature = "allocation-profile")]
    pub(crate) fn retained_slots(&self) -> (usize, usize) {
        self.cursor.retained_slots()
    }
    pub(crate) fn advance(&mut self) -> Result<bool, String> {
        self.cursor.advance(&mut self.scratch, &mut self.policy)
    }
    pub(crate) fn into_completed(self) -> Result<ResourceCursor<'a>, String> {
        if !self.cursor.is_complete() {
            return Err("Scalar size cursor is not complete".into());
        }
        Ok(self.cursor)
    }
}
