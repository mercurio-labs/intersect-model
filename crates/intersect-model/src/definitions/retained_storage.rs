//! Handwritten host-independent logical storage preflight for owned metadata.
use crate::definitions::{conversion::InvocationContext, metadata_limits::TraversalBudget};
use serde::Serialize;
pub const RETAINED_NODE_BYTES: usize = 128;

/// Reserve before copying/decoding a bounded metadata root. The shared metadata
/// serializer charges UTF-8 bytes and atom/container policy bytes; an additional
/// 128 bytes per visited node accounts for logical retained structure. This is a
/// cumulative portable admission policy, not a physical heap-size measurement or
/// an allocator guarantee. Traversal remains synchronous and observes every visit.
/// The input must remain immutable between this preflight and the owned copy.
pub fn reserve_serialized<T: ?Sized + Serialize>(
    value: &T,
    observation: &InvocationContext<'_>,
) -> Result<usize, String> {
    let mut budget = TraversalBudget::new(65_536, 64, usize::MAX);
    budget.inspect_with_work(value, &|units| observation.work(units))?;
    let bytes = bytes_for_budget(&budget)?;
    observation.retain_storage(bytes)?;
    Ok(bytes)
}
pub(crate) fn bytes_for_budget(budget: &TraversalBudget) -> Result<usize, String> {
    budget
        .visited_nodes()
        .checked_mul(RETAINED_NODE_BYTES)
        .and_then(|structure| structure.checked_add(budget.payload_bytes()))
        .ok_or_else(|| "Retained model storage size overflow".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::definitions::conversion::{InvocationObserver, ProviderId};
    use std::{cell::Cell, rc::Rc};
    struct Budget {
        work: Cell<usize>,
        max_work: usize,
        storage: Cell<usize>,
        max_storage: usize,
    }
    impl InvocationObserver for Budget {
        fn before_invoke(&self, _: &ProviderId) -> Result<(), String> {
            panic!("storage inspection cannot invoke a provider")
        }
        fn work(&self, units: usize) -> Result<(), String> {
            let next = self.work.get() + units;
            if next > self.max_work {
                return Err("work interrupted".into());
            }
            self.work.set(next);
            Ok(())
        }
        fn retain_storage(&self, bytes: usize) -> Result<(), String> {
            let next = self.storage.get() + bytes;
            if next > self.max_storage {
                return Err("storage interrupted".into());
            }
            self.storage.set(next);
            Ok(())
        }
    }
    fn budget(work: usize, storage: usize) -> Rc<Budget> {
        Rc::new(Budget {
            work: Cell::new(0),
            max_work: work,
            storage: Cell::new(0),
            max_storage: storage,
        })
    }
    #[test]
    fn literal_policy_and_cumulative_storage_boundaries() {
        // Nine nodes, 89 metadata bytes, and 128 logical bytes per node.
        let value = serde_json::json!({"s":"é","n":true,"items":[null,7]});
        for limit in [0, 1240, 1241, 1242, 2481, 2482] {
            let budget = budget(usize::MAX, limit);
            let observation = InvocationContext::new(budget.clone());
            let first = reserve_serialized(&value, &observation);
            assert_eq!(budget.work.get(), 9);
            if limit < 1241 {
                assert_eq!(first.unwrap_err(), "storage interrupted");
                assert_eq!(budget.storage.get(), 0);
            } else {
                assert_eq!(first.unwrap(), 1241);
                let second = reserve_serialized(&value, &observation);
                if limit < 2482 {
                    assert_eq!(second.unwrap_err(), "storage interrupted");
                    assert_eq!(budget.storage.get(), 1241);
                } else {
                    assert_eq!(second.unwrap(), 1241);
                    assert_eq!(budget.storage.get(), 2482);
                }
                assert_eq!(budget.work.get(), 18);
            }
        }
        assert_eq!(
            reserve_serialized("é", &InvocationContext::default()).unwrap(),
            132
        );
    }
    #[test]
    fn interrupted_traversal_never_reserves_a_partial_copy() {
        let value = serde_json::json!({"s":"é","n":true,"items":[null,7]});
        for limit in 0..9 {
            let budget = budget(limit, usize::MAX);
            let observation = InvocationContext::new(budget.clone());
            assert_eq!(
                reserve_serialized(&value, &observation).unwrap_err(),
                "work interrupted"
            );
            assert_eq!(budget.work.get(), limit);
            assert_eq!(budget.storage.get(), 0);
        }
        let deep = (0..65).fold(serde_json::Value::Null, |inner, _| {
            serde_json::json!([inner])
        });
        let budget = budget(usize::MAX, usize::MAX);
        assert!(reserve_serialized(&deep, &InvocationContext::new(budget.clone())).is_err());
        assert_eq!(budget.storage.get(), 0);
    }
}
