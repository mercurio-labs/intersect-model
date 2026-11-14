//! Shared stored/provider shape checks with resumable uniqueness comparison.
use super::*;
use std::collections::{HashSet, hash_set::IntoIter};
pub(super) struct FeatureShape {
    label: String,
    next: usize,
    end: usize,
    seen: HashSet<Value>,
    cleanup: Option<IntoIter<Value>>,
}
impl FeatureShape {
    pub(super) fn new(
        class: &str,
        field: &intersect_model::definitions::schema::Feature,
        value: Option<ValueView<'_>>,
        require_lower_bounds: bool,
    ) -> StorageResult<Self> {
        let list = value.and_then(|v| v.collection());
        let populated = value.filter(|v| !v.is_null());
        if field.upper == 1 && list.is_some() {
            return Err("Singular feature cannot hold a list".into());
        }
        if field.upper != 1 && list.is_none() {
            return Err("Multiple feature requires a list".into());
        }
        let count = list.map_or(usize::from(populated.is_some()), |v| v.len());
        if (require_lower_bounds && count < field.lower)
            || (field.upper >= 0 && count > field.upper as usize)
        {
            return Err(format!("Multiplicity violation: {}.{}", class, field.name));
        }
        let check_unique = field.unique && count > 1;
        Ok(Self {
            label: if check_unique {
                format!("{}.{}", class, field.name)
            } else {
                String::new()
            },
            next: 0,
            end: if check_unique { count } else { 0 },
            seen: HashSet::new(),
            cleanup: None,
        })
    }
    // Observed pre-growth adapter over the SAME shape/equality algorithm.
    // Logical retained admissions are cumulative, not an RSS/allocator promise.
    pub(super) fn new_observed(
        class: &str,
        field: &intersect_model::definitions::schema::Feature,
        value: Option<ValueView<'_>>,
        require_lower_bounds: bool,
        observation: &InvocationContext<'_>,
        healthy: &dyn Fn(&InvocationContext<'_>) -> StorageResult<()>,
    ) -> StorageResult<Self> {
        observation.work(1)?;
        healthy(observation)?;
        const HEADER: usize = 256;
        const {
            assert!(std::mem::size_of::<Self>() <= HEADER);
        }
        let bytes = HEADER
            .checked_add(class.len())
            .and_then(|n| n.checked_add(field.name.len()))
            .and_then(|n| n.checked_add(1))
            .ok_or("Observed shape label storage overflow")?;
        observation.retain_storage(bytes)?;
        observation.checkpoint()?;
        healthy(observation)?;
        Self::new(class, field, value, require_lower_bounds)
    }
    pub(super) fn advance_observed(
        &mut self,
        value: Option<ValueView<'_>>,
        observation: &InvocationContext<'_>,
        healthy: &dyn Fn(&InvocationContext<'_>) -> StorageResult<()>,
    ) -> StorageResult<bool> {
        observation.checkpoint()?;
        healthy(observation)?;
        if self.next < self.end && self.cleanup.is_none() {
            let atom = value
                .and_then(|v| v.collection())
                .and_then(|v| v.get(self.next))
                .ok_or("Missing uniqueness value")?;
            crate::definitions::retained_storage::reserve_serialized(&atom, observation)?;
            healthy(observation)?;
            // Conservatively admit a complete possible replacement handle table
            // on each reserve, independent of allocator capacity. Existing
            // logical nodes/payload are not refunded if reserve keeps storage.
            let bytes = self
                .seen
                .len()
                .checked_add(1)
                .and_then(|n| n.checked_mul(128))
                .ok_or("Observed shape uniqueness storage overflow")?;
            observation.retain_storage(bytes)?;
            healthy(observation)?;
            for _ in 0..bytes.div_ceil(64) {
                observation.work(1)?;
            }
            observation.checkpoint()?;
            healthy(observation)?;
            self.seen
                .try_reserve(1)
                .map_err(|_| "Observed shape uniqueness allocation failed")?;
        }
        self.advance(value, || {
            observation.work(1)?;
            // The canonical callback may revoke the same source and return Ok.
            // No atom clone/insert (or cleanup) follows a failed health check.
            healthy(observation)
        })
    }
    pub(super) fn complete(&self) -> bool {
        self.next == self.end && self.seen.is_empty() && self.cleanup.is_none()
    }
    /// Compare or release one retained atom. JSON hashing/equality within an atom
    /// is shared with the existing storage uniqueness contract and is synchronous.
    pub(super) fn advance(
        &mut self,
        value: Option<ValueView<'_>>,
        mut charge: impl FnMut() -> StorageResult<()>,
    ) -> StorageResult<bool> {
        if let Some(cleanup) = self.cleanup.as_mut() {
            if cleanup.len() > 0 {
                charge()?;
                cleanup.next();
            }
            if cleanup.len() == 0 {
                self.cleanup = None;
            }
            return Ok(self.complete());
        }
        if self.next < self.end {
            charge()?;
            let atom = value
                .and_then(|v| v.collection())
                .and_then(|v| v.get(self.next))
                .ok_or("Missing uniqueness value")?;
            if !self.seen.insert(atom.to_json()) {
                return Err(format!("Duplicate value: {}", self.label));
            }
            self.next += 1;
            if self.next == self.end {
                self.cleanup = Some(std::mem::take(&mut self.seen).into_iter());
            }
        }
        Ok(self.complete())
    }
}
