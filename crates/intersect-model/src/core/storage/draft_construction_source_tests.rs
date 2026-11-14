//! Synthetic source-service controls over the canonical prepared/source factory.
//! Metadata/setup are separate components, never new original-request grants.
use super::*;
use crate::core::frozen::FrozenStore;
use crate::core::storage::pending_source::tests::{definition as prepare_definition, inputs};
use crate::definitions::preparation::PreparedModelDefinition;

struct Native {
    rows: Vec<DraftClassContract>,
    equal: Vec<DraftClassContract>,
    changed: Vec<DraftClassContract>,
    mode: Cell<usize>,
    calls: Cell<usize>,
    drift_at: Cell<Option<usize>>,
}
impl DraftPhysicalDefaults for Native {
    fn contracts(&self) -> &[DraftClassContract] {
        let call = self.calls.get() + 1;
        self.calls.set(call);
        if self.drift_at.get() == Some(call) {
            self.mode.set(2);
        }
        match self.mode.get() {
            0 => &self.rows,
            1 => &self.equal,
            _ => &self.changed,
        }
    }
}
fn native(definition: &PreparedModelDefinition<'_>) -> StorageResult<Native> {
    let schema = definition.schema();
    let mut rows = Vec::new();
    for class in ["A", "B", "C"] {
        rows.push(DraftClassContract {
            version: 1,
            schema_revision: schema.revision.clone(),
            class: class.into(),
            class_identity: schema.identity(class),
            sources: definition
                .sources()
                .sources
                .iter()
                .map(|s| s.id.clone())
                .collect(),
            members: schema
                .effective_features(class)?
                .into_iter()
                .filter(|m| !m.feature.derived)
                .map(|m| DraftMemberDisposition {
                    name: m.feature.name.clone(),
                    identity: m.identity,
                    disposition: if m.feature.name == "children" {
                        DraftDisposition::DelegatedAbsent
                    } else {
                        DraftDisposition::StoredDefault
                    },
                })
                .collect(),
        });
    }
    assert_eq!(rows.iter().map(|r| r.members.len()).sum::<usize>(), 12);
    let equal = rows.clone();
    let mut changed = rows.clone();
    changed[0].members[0].name = "changed-source-member".into();
    Ok(Native {
        rows,
        equal,
        changed,
        mode: Cell::new(0),
        calls: Cell::new(0),
        drift_at: Cell::new(None),
    })
}
macro_rules! setup {
    ($definition:ident,$metadata:ident,$native:ident,$context:ident) => {
        let (input, migration) = inputs()?;
        let $definition = prepare_definition(&input, &migration)?;
        let $metadata = $definition
            .prepare_physical_metadata(&InvocationContext::default())
            .map_err(|e| e.to_string())?;
        let $native = native(&$definition)?;
        let $context = DraftConstructionContext::bind(
            $definition.schema(),
            $definition.sources(),
            &$native,
            &mut QueryBudget::new(100_000),
        )?;
        $native.calls.set(0);
    };
}

#[test]
fn constructor_source_same_and_equal_alternate_keep_callbacks_and_literal_fees() -> StorageResult<()>
{
    setup!(definition, metadata, native, context);
    let mut old = QueryBudget::new(100_000);
    context.check_live_physical_contracts(&mut || old.charge())?;
    assert_eq!(old.reads, 16);
    assert_eq!(native.calls.get(), 1);
    native.calls.set(0);
    let mut budget = QueryBudget::new(100_000);
    for _ in 0..7 {
        budget.charge()?;
    }
    let sources = PendingSourceInputs::new(&metadata, &mut budget)?;
    let lease = context.source_lease(&metadata, &sources, &mut budget)?;
    assert_eq!(budget.reads, 11);
    assert_eq!(native.calls.get(), 0);
    let before = budget.reads;
    lease.check_live(&context, &mut || budget.charge())?;
    assert_eq!(budget.reads - before, 21);
    let before = budget.reads;
    lease.check_live(&context, &mut || budget.charge())?;
    assert_eq!(budget.reads - before, 6);
    native.mode.set(1);
    let before = budget.reads;
    lease.check_live(&context, &mut || budget.charge())?;
    assert_eq!(budget.reads - before, 21);
    let before = budget.reads;
    lease.check_live(&context, &mut || budget.charge())?;
    assert_eq!(budget.reads - before, 6);
    assert_eq!(native.calls.get(), 4);
    assert_eq!(budget.reads, 65);
    println!(
        "CONSTRUCTION_SOURCE_FEES old=16 cold=21 warm=6 preconsumed=7 total={} sidecar={}",
        budget.reads,
        lease.logical_bytes()
    );
    Ok(())
}

#[test]
fn constructor_source_warm_zero_exact_short_discards_and_cold_recovery() -> StorageResult<()> {
    setup!(definition, metadata, native, context);
    for window in [0, 5, 6] {
        native.mode.set(0);
        native.calls.set(0);
        let mut budget = QueryBudget::new(100_000);
        for _ in 0..7 {
            budget.charge()?;
        }
        let sources = PendingSourceInputs::new(&metadata, &mut budget)?;
        let lease = context.source_lease(&metadata, &sources, &mut budget)?;
        lease.check_live(&context, &mut || budget.charge())?;
        let setup = budget.reads;
        assert_eq!(setup, 32);
        let calls = native.calls.get();
        let mut left = window;
        let result = lease.check_live(&context, &mut || {
            if left == 0 {
                return Err("Query work budget exhausted; no complete result".into());
            }
            budget.charge()?;
            left -= 1;
            Ok(())
        });
        assert_eq!(budget.reads - setup, window);
        if window == 6 {
            result?;
            assert!(lease.checked.get().is_some());
        } else {
            assert_eq!(
                result,
                Err("Query work budget exhausted; no complete result".into())
            );
            assert!(lease.failed.get());
            assert!(!lease.busy.get());
            assert!(lease.checked.get().is_none());
            assert!(
                context
                    .source_lease(&metadata, &sources, &mut budget)
                    .is_err()
            );
            if window == 0 {
                assert_eq!(native.calls.get(), calls);
            }
        }
        println!(
            "CONSTRUCTION_SOURCE_WINDOW setup={setup} limit={window} total={} calls={}",
            budget.reads,
            native.calls.get()
        );
    }
    // A denied core lease admission cannot invoke the provider. This setup
    // scope is a separate component, not a zero-cost original invocation.
    native.calls.set(0);
    let mut setup_budget = QueryBudget::new(100_000);
    let denied = PendingSourceInputs::new(&metadata, &mut setup_budget)?;
    let mut zero = QueryBudget::new(0);
    let error = context
        .source_lease(&metadata, &denied, &mut zero)
        .err()
        .ok_or("Zero lease admission escaped")?;
    assert_eq!(error, "Query work budget exhausted; no complete result");
    assert_eq!(zero.reads, 0);
    assert_eq!(native.calls.get(), 0);
    // A new independent invocation begins cold, with its own setup costs.
    native.mode.set(0);
    native.calls.set(0);
    let mut budget = QueryBudget::new(100_000);
    let sources = PendingSourceInputs::new(&metadata, &mut budget)?;
    let lease = context.source_lease(&metadata, &sources, &mut budget)?;
    let before = budget.reads;
    lease.check_live(&context, &mut || budget.charge())?;
    assert_eq!(budget.reads - before, 21);
    assert_eq!(native.calls.get(), 1);
    Ok(())
}

#[test]
fn constructor_source_changed_foreign_and_caught_reentry_poison() -> StorageResult<()> {
    setup!(definition, metadata, native, context);
    let mut budget = QueryBudget::new(100_000);
    let sources = PendingSourceInputs::new(&metadata, &mut budget)?;
    let lease = context.source_lease(&metadata, &sources, &mut budget)?;
    lease.check_live(&context, &mut || budget.charge())?;
    native.mode.set(2);
    let mut old = QueryBudget::new(100_000);
    let expected = context.check_live_physical_contracts(&mut || old.charge());
    let actual = lease.check_live(&context, &mut || budget.charge());
    assert_eq!(actual, expected);
    assert!(actual.is_err());
    assert!(lease.checked.get().is_none());
    native.mode.set(0);
    let foreign = DraftConstructionContext::bind(
        definition.schema(),
        definition.sources(),
        &native,
        &mut budget,
    )?;
    let source2 = PendingSourceInputs::new(&metadata, &mut budget)?;
    let lease2 = context.source_lease(&metadata, &source2, &mut budget)?;
    let calls = native.calls.get();
    assert!(
        lease2
            .check_live(&foreign, &mut || budget.charge())
            .is_err()
    );
    assert_eq!(native.calls.get(), calls);
    assert!(lease2.checked.get().is_none());
    let source3 = PendingSourceInputs::new(&metadata, &mut budget)?;
    let lease3 = context.source_lease(&metadata, &source3, &mut budget)?;
    let mut nested = false;
    let result = lease3.check_live(&context, &mut || {
        if !nested {
            nested = true;
            assert!(lease3.check_live(&context, &mut || Ok(())).is_err());
        }
        budget.charge()
    });
    assert!(result.is_err());
    assert!(!lease3.busy.get());
    assert!(lease3.checked.get().is_none());
    assert!(
        context
            .source_lease(&metadata, &source3, &mut budget)
            .is_err()
    );
    let (input2, migration2) = inputs()?;
    let definition2 = prepare_definition(&input2, &migration2)?;
    let metadata2 = definition2
        .prepare_physical_metadata(&InvocationContext::default())
        .map_err(|e| e.to_string())?;
    let source4 = PendingSourceInputs::new(&metadata2, &mut budget)?;
    assert!(
        context
            .source_lease(&metadata, &source4, &mut budget)
            .is_err()
    );
    let source5 = PendingSourceInputs::new(&metadata2, &mut budget)?;
    assert!(
        context
            .source_lease(&metadata2, &source5, &mut budget)
            .is_err()
    );
    assert!(
        source5
            .check_invocation_context(&metadata2, &mut budget)
            .is_err()
    );
    Ok(())
}

#[test]
fn constructor_source_same_atomic_graph_callbacks_and_late_drift_rollback() -> StorageResult<()> {
    setup!(definition, metadata, native, context);
    let mut old =
        FrozenStore::for_schema(definition.schema()).map_err(|error| error.to_string())?;
    let outer = old.begin()?;
    bind_schema(&mut old, definition.schema())?;
    context.create_pending(
        &mut old,
        Construction {
            id: "a",
            resource: "memory:contract-component",
            authored: &[],
        },
        "A",
        0,
        1,
        &metadata,
        &mut QueryBudget::new(100_000),
    )?;
    let expected = export(&old)?;
    let calls = native.calls.get();
    assert_eq!(calls, 5);
    old.rollback(&outer)?;
    native.calls.set(0);
    let mut store =
        FrozenStore::for_schema(definition.schema()).map_err(|error| error.to_string())?;
    let outer = store.begin()?;
    bind_schema(&mut store, definition.schema())?;
    let mut budget = QueryBudget::new(100_000);
    let sources = PendingSourceInputs::new(&metadata, &mut budget)?;
    let lease = context.source_lease(&metadata, &sources, &mut budget)?;
    lease.create_pending(
        &mut store,
        Construction {
            id: "a",
            resource: "memory:contract-component",
            authored: &[],
        },
        "A",
        0,
        1,
        &mut budget,
    )?;
    assert_eq!(export(&store)?, expected);
    assert_eq!(native.calls.get(), calls);
    store.rollback(&outer)?;
    native.calls.set(0);
    native.drift_at.set(Some(3));
    let outer = store.begin()?;
    bind_schema(&mut store, definition.schema())?;
    let before = export(&store)?;
    let sources = PendingSourceInputs::new(&metadata, &mut budget)?;
    let lease = context.source_lease(&metadata, &sources, &mut budget)?;
    assert!(
        lease
            .create_pending(
                &mut store,
                Construction {
                    id: "a",
                    resource: "memory:contract-component",
                    authored: &[]
                },
                "A",
                0,
                1,
                &mut budget
            )
            .is_err()
    );
    assert_eq!(native.calls.get(), 3);
    assert_eq!(export(&store)?, before);
    assert!(lease.checked.get().is_none());
    store.rollback(&outer)?;
    Ok(())
}

#[path = "draft_construction_member_index_tests.rs"]
mod member_index;
