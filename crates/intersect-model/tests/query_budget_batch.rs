//! Counter-only batching preserves the original caller ledger on every exit.
use intersect_model::core::query::QueryBudget;

fn literal(budget: &mut QueryBudget, units: usize) -> Result<(), String> {
    for _ in 0..units {
        budget.charge()?;
    }
    Ok(())
}
fn compare(remaining: usize, reads: usize, units: usize) {
    let mut old = QueryBudget::new(remaining);
    old.reads = reads;
    let mut new = QueryBudget::new(remaining);
    new.reads = reads;
    let a = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| literal(&mut old, units)));
    let b = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| new.charge_many(units)));
    match (a, b) {
        (Ok(a), Ok(b)) => assert_eq!(a, b),
        (Err(_), Err(_)) => {}
        _ => panic!("batched accounting changed the original exit"),
    }
    assert_eq!(old.remaining(), new.remaining());
    assert_eq!(old.reads, new.reads);
}
#[test]
fn zero_exact_and_partial_exhaustion_match_original_ledger() {
    for remaining in 0..=16 {
        for reads in [0, 3, 100] {
            for units in 0..=20 {
                compare(remaining, reads, units);
            }
        }
    }
}
#[test]
fn preconsumed_and_repeated_calls_preserve_failure_ledger() {
    let mut old = QueryBudget::new(12);
    let mut new = QueryBudget::new(12);
    for units in [3, 0, 4, 8, 1, 0] {
        assert_eq!(literal(&mut old, units), new.charge_many(units));
        assert_eq!(old.remaining(), new.remaining());
        assert_eq!(old.reads, new.reads);
    }
}
#[test]
fn maximal_grant_and_manually_altered_overflow_match_original() {
    compare(usize::MAX, 0, 1);
    compare(usize::MAX, usize::MAX - 1, 1);
    compare(3, usize::MAX - 1, 3);
    compare(0, usize::MAX, 0);
    compare(0, usize::MAX, 1);
    let mut budget = QueryBudget::new(usize::MAX);
    assert_eq!(budget.charge_many(usize::MAX), Ok(()));
    assert_eq!(budget.remaining(), 0);
    assert_eq!(budget.reads, usize::MAX);
    assert!(budget.charge_many(1).is_err());
    assert_eq!(budget.remaining(), 0);
    assert_eq!(budget.reads, usize::MAX);
}
