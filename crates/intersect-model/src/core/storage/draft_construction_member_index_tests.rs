//! Authored wide-source components; not original K01 or parser-budget evidence.
//! Metadata/bind setup is separate; each operation window uses one underlying
//! ledger without refund, reset or a new invocation warming allowance.
use super::*;
use crate::core::frozen::FrozenStore;
use crate::core::storage::pending_source::tests::inputs_for;

fn wide_xml() -> String {
    let mut text = String::from(
        r#"<e:EPackage xmlns:e="http://www.eclipse.org/emf/2002/Ecore" xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance" name="reuse" nsURI="urn:source-reuse"><eClassifiers xsi:type="e:EClass" name="A">"#,
    );
    for index in 0..64 {
        text.push_str(&format!(r#"<eStructuralFeatures xsi:type="e:EAttribute" name="f{index:03}" upperBound="-1" eType="e:EDataType http://www.eclipse.org/emf/2002/Ecore#//EString"/>"#));
    }
    text.push_str("</eClassifiers></e:EPackage>");
    text
}

fn wide_native(definition: &PreparedModelDefinition<'_>) -> StorageResult<Native> {
    let schema = definition.schema();
    let members = schema
        .effective_features("A")?
        .into_iter()
        .filter(|field| !field.feature.derived)
        .map(|field| DraftMemberDisposition {
            name: field.feature.name.clone(),
            identity: field.identity,
            disposition: if field.feature.name == "f000" {
                DraftDisposition::StoredDefault
            } else {
                DraftDisposition::DelegatedAbsent
            },
        })
        .collect::<Vec<_>>();
    assert_eq!(members.len(), 64);
    assert_eq!(
        members.first().map(|member| member.name.as_str()),
        Some("f000")
    );
    assert_eq!(
        members.last().map(|member| member.name.as_str()),
        Some("f063")
    );
    let rows = vec![DraftClassContract {
        version: 1,
        schema_revision: schema.revision.clone(),
        class: "A".into(),
        class_identity: schema.identity("A"),
        sources: definition
            .sources()
            .sources
            .iter()
            .map(|source| source.id.clone())
            .collect(),
        members,
    }];
    let equal = rows.clone();
    let mut changed = rows.clone();
    changed[0].members[63].disposition = DraftDisposition::StoredDefault;
    Ok(Native {
        rows,
        equal,
        changed,
        mode: Cell::new(0),
        calls: Cell::new(0),
        drift_at: Cell::new(None),
    })
}

macro_rules! wide_setup {
    ($definition:ident,$metadata:ident,$native:ident,$context:ident,$element:ident) => {
        let xml = wide_xml();
        let (input, migration) = inputs_for(&xml)?;
        let $definition = prepare_definition(&input, &migration)?;
        let $metadata = $definition
            .prepare_physical_metadata(&InvocationContext::default())
            .map_err(|error| error.to_string())?;
        let $native = wide_native(&$definition)?;
        let $context = DraftConstructionContext::bind(
            $definition.schema(),
            $definition.sources(),
            &$native,
            &mut QueryBudget::new(100_000),
        )?;
        let $element = Element::new_with_stored_selection(
            $definition.schema(),
            "a",
            "A",
            "memory:member-index",
            2,
            4,
            Some(&$context.selected["A"].names),
        )?;
        assert_eq!($element.values.len(), 1);
        assert!($element.values.contains_key("f000"));
        assert!(!$element.values.contains_key("f063"));
        $native.calls.set(0);
    };
}

// Verification-only frozen source4 body, mechanically extracted verbatim.
// This is not a second product algorithm or an old binary replay. The source4
// body runs on this component's source owner; only new sidecar byte size differs.
fn source4_permits_absence(
    lease: &DraftConstructionSource<'_, '_>,
    context: &DraftConstructionContext<'_>,
    element: ObjectView<'_>,
    name: &str,
    identity: &MemberIdentity,
    charge: &mut impl FnMut() -> StorageResult<()>,
) -> StorageResult<bool> {
    let result = (|| {
        lease.check_live(context, charge)?;
        let value = context.checked_absent_physical_slot(element, name, identity, charge)?;
        lease.association(context, charge)?;
        Ok(value)
    })();
    if result.is_err() {
        lease.invalidate();
    }
    result
}

#[test]
fn constructor_member_index_literal_cold_warm_work_and_logical_admission() -> StorageResult<()> {
    wide_setup!(definition, metadata, native, context, element);
    let member = &native.rows[0].members[63];
    let mut old = QueryBudget::new(100_000);
    assert!(context.permits_absent_physical_slot(
        ObjectView::from_element(&element),
        &member.name,
        &member.identity,
        &mut || old.charge()
    )?);
    assert_eq!(old.reads, 133);
    assert_eq!(native.calls.get(), 1);
    native.calls.set(0);
    // Actual frozen-source4 worker sequence, including paid proof warming.
    // This cannot be described as a free warm or a cold original request.
    let mut source4_budget = QueryBudget::new(100_000);
    for _ in 0..7 {
        source4_budget.charge()?;
    }
    let source4_inputs = PendingSourceInputs::new(&metadata, &mut source4_budget)?;
    let source4_lease = context.source_lease(&metadata, &source4_inputs, &mut source4_budget)?;
    assert_eq!(source4_budget.reads, 11);
    let before = source4_budget.reads;
    source4_lease.check_live(&context, &mut || source4_budget.charge())?;
    assert_eq!(source4_budget.reads - before, 71);
    let before = source4_budget.reads;
    assert!(source4_permits_absence(
        &source4_lease,
        &context,
        ObjectView::from_element(&element),
        &member.name,
        &member.identity,
        &mut || source4_budget.charge()
    )?);
    assert_eq!(source4_budget.reads - before, 75);
    assert_eq!(source4_budget.reads, 157);
    assert_eq!(native.calls.get(), 2);
    assert!(
        source4_lease
            .members
            .try_borrow()
            .map_err(|_| "Test member borrow")?
            .is_empty()
    );
    println!(
        "CONSTRUCTION_MEMBER_INDEX_SOURCE4_BASELINE old_default=133 setup=11 cold_contract_proof=71 warm_absence=75 same_ledger_total=157 callbacks=2"
    );
    native.calls.set(0);
    let mut budget = QueryBudget::new(100_000);
    for _ in 0..7 {
        budget.charge()?;
    }
    let sources = PendingSourceInputs::new(&metadata, &mut budget)?;
    let lease = context.source_lease(&metadata, &sources, &mut budget)?;
    assert_eq!(budget.reads, 11);
    let before = budget.reads;
    assert!(lease.permits_absence(
        &context,
        ObjectView::from_element(&element),
        &member.name,
        &member.identity,
        &mut || budget.charge()
    )?);
    assert_eq!(budget.reads - before, 211);
    let logical = lease.logical_bytes();
    let source_logical = sources.logical_bytes();
    assert_eq!(
        logical - std::mem::size_of::<DraftConstructionSource<'_, '_>>(),
        3136
    );
    assert_eq!(source_logical, 160 + logical);
    for _ in 0..2 {
        let before = budget.reads;
        assert!(lease.permits_absence(
            &context,
            ObjectView::from_element(&element),
            &member.name,
            &member.identity,
            &mut || budget.charge()
        )?);
        assert_eq!(budget.reads - before, 13);
        assert_eq!(lease.logical_bytes(), logical);
        assert_eq!(sources.logical_bytes(), source_logical);
    }
    assert_eq!(native.calls.get(), 3);
    assert_eq!(budget.reads, 248);
    assert_eq!(
        lease
            .members
            .try_borrow()
            .map_err(|_| "Test member borrow")?
            .len(),
        1
    );
    println!(
        "CONSTRUCTION_MEMBER_INDEX_FEES {}",
        serde_json::json!({"members":64,"old":133,"cold":211,"warm":13,"preconsumed":7,"same_ledger_total":budget.reads,"source_calls":native.calls.get(),"index_logical":3136,"sidecar_bytes":std::mem::size_of::<DraftConstructionSource<'_, '_>>(),"source_cumulative_logical":source_logical,"logical_is_not_RSS":true})
    );
    Ok(())
}

#[test]
fn constructor_member_index_warm_zero_exact_short_and_cold_denial_poison() -> StorageResult<()> {
    wide_setup!(definition, metadata, native, context, element);
    let member = &native.rows[0].members[63];
    for window in [0, 12, 13] {
        let mut budget = QueryBudget::new(100_000);
        for _ in 0..7 {
            budget.charge()?;
        }
        let sources = PendingSourceInputs::new(&metadata, &mut budget)?;
        let lease = context.source_lease(&metadata, &sources, &mut budget)?;
        lease.permits_absence(
            &context,
            ObjectView::from_element(&element),
            &member.name,
            &member.identity,
            &mut || budget.charge(),
        )?;
        assert_eq!(budget.reads, 222);
        let setup = budget.reads;
        let admitted = sources.logical_bytes();
        let calls = native.calls.get();
        let mut left = window;
        let result = lease.permits_absence(
            &context,
            ObjectView::from_element(&element),
            &member.name,
            &member.identity,
            &mut || {
                if left == 0 {
                    return Err("Query work budget exhausted; no complete result".into());
                }
                budget.charge()?;
                left -= 1;
                Ok(())
            },
        );
        assert_eq!(budget.reads - setup, window);
        assert_eq!(sources.logical_bytes(), admitted);
        if window == 13 {
            assert!(result?);
        } else {
            assert_eq!(
                result,
                Err("Query work budget exhausted; no complete result".into())
            );
            assert!(lease.failed.get());
            assert!(!lease.busy.get() && !lease.member_busy.get());
            assert!(
                lease
                    .members
                    .try_borrow()
                    .map_err(|_| "Test member borrow")?
                    .is_empty()
            );
            if window == 0 {
                assert_eq!(native.calls.get(), calls);
            }
            assert!(
                context
                    .source_lease(&metadata, &sources, &mut budget)
                    .is_err()
            );
        }
        println!(
            "CONSTRUCTION_MEMBER_INDEX_WARM_WINDOW setup={setup} window={window} total={} success={}",
            budget.reads,
            window == 13
        );
    }
    for window in [0, 210, 211] {
        let mut budget = QueryBudget::new(100_000);
        let sources = PendingSourceInputs::new(&metadata, &mut budget)?;
        let lease = context.source_lease(&metadata, &sources, &mut budget)?;
        let setup = budget.reads;
        let mut left = window;
        let result = lease.permits_absence(
            &context,
            ObjectView::from_element(&element),
            &member.name,
            &member.identity,
            &mut || {
                if left == 0 {
                    return Err("Query work budget exhausted; no complete result".into());
                }
                budget.charge()?;
                left -= 1;
                Ok(())
            },
        );
        assert_eq!(budget.reads - setup, window);
        if window == 211 {
            assert!(result?);
        } else {
            assert_eq!(
                result,
                Err("Query work budget exhausted; no complete result".into())
            );
            assert!(lease.failed.get());
            assert!(
                lease
                    .members
                    .try_borrow()
                    .map_err(|_| "Test member borrow")?
                    .is_empty()
            );
        }
        println!(
            "CONSTRUCTION_MEMBER_INDEX_COLD_WINDOW setup={setup} window={window} total={} source_logical={}",
            budget.reads,
            sources.logical_bytes()
        );
    }
    Ok(())
}

#[test]
fn constructor_member_index_equal_alternate_and_actual_source_guards_remain_fresh()
-> StorageResult<()> {
    wide_setup!(definition, metadata, native, context, element);
    let member = &native.rows[0].members[63];
    let mut budget = QueryBudget::new(100_000);
    let sources = PendingSourceInputs::new(&metadata, &mut budget)?;
    let lease = context.source_lease(&metadata, &sources, &mut budget)?;
    lease.permits_absence(
        &context,
        ObjectView::from_element(&element),
        &member.name,
        &member.identity,
        &mut || budget.charge(),
    )?;
    let logical = lease.logical_bytes();
    native.mode.set(1);
    let before = budget.reads;
    assert!(lease.permits_absence(
        &context,
        ObjectView::from_element(&element),
        &member.name,
        &member.identity,
        &mut || budget.charge()
    )?);
    assert_eq!(budget.reads - before, 78);
    assert_eq!(lease.logical_bytes(), logical);
    native.mode.set(2);
    let expected = context.permits_absent_physical_slot(
        ObjectView::from_element(&element),
        &member.name,
        &member.identity,
        &mut || Ok(()),
    );
    let actual = lease.permits_absence(
        &context,
        ObjectView::from_element(&element),
        &member.name,
        &member.identity,
        &mut || budget.charge(),
    );
    assert_eq!(actual, expected);
    assert_eq!(
        actual,
        Err("Draft physical capability changed after binding".into())
    );
    assert!(
        lease
            .members
            .try_borrow()
            .map_err(|_| "Test member borrow")?
            .is_empty()
    );
    native.mode.set(0);
    // These authored Element-backed header adversaries do not force invalid
    // canonical FrozenStore records past its own earlier descriptor checks.
    for case in 0..5 {
        let sources = PendingSourceInputs::new(&metadata, &mut budget)?;
        let lease = context.source_lease(&metadata, &sources, &mut budget)?;
        lease.permits_absence(
            &context,
            ObjectView::from_element(&element),
            &member.name,
            &member.identity,
            &mut || budget.charge(),
        )?;
        let mut wrong = element.clone();
        let (name, identity) = match case {
            0 => {
                wrong.class = "Unknown".into();
                (member.name.as_str(), &member.identity)
            }
            1 => {
                wrong.type_identity = "foreign".into();
                (member.name.as_str(), &member.identity)
            }
            2 => (member.name.as_str(), &native.rows[0].members[0].identity),
            3 => ("absent-name", &member.identity),
            _ => ("f000", &native.rows[0].members[0].identity),
        };
        let expected = context.permits_absent_physical_slot(
            ObjectView::from_element(&wrong),
            name,
            identity,
            &mut || Ok(()),
        );
        let actual = lease.permits_absence(
            &context,
            ObjectView::from_element(&wrong),
            name,
            identity,
            &mut || budget.charge(),
        );
        assert_eq!(actual, expected, "case={case}");
        assert!(actual.is_err());
        assert!(lease.failed.get());
        assert!(
            lease
                .members
                .try_borrow()
                .map_err(|_| "Test member borrow")?
                .is_empty()
        );
    }
    println!(
        "CONSTRUCTION_MEMBER_INDEX_SOURCE_GUARDS equal_alternate=78 changed_and_fresh_headers=preserved"
    );
    Ok(())
}

#[test]
fn constructor_member_index_caught_reentry_and_foreign_context_discard_warm_rows()
-> StorageResult<()> {
    wide_setup!(definition, metadata, native, context, element);
    let member = &native.rows[0].members[63];
    let mut budget = QueryBudget::new(100_000);
    let sources = PendingSourceInputs::new(&metadata, &mut budget)?;
    let lease = context.source_lease(&metadata, &sources, &mut budget)?;
    lease.permits_absence(
        &context,
        ObjectView::from_element(&element),
        &member.name,
        &member.identity,
        &mut || budget.charge(),
    )?;
    let mut callback = 0;
    let mut caught = false;
    let result = lease.permits_absence(
        &context,
        ObjectView::from_element(&element),
        &member.name,
        &member.identity,
        &mut || {
            callback += 1;
            if callback == 9 {
                caught = true;
                assert_eq!(
                    lease.check_live(&context, &mut || Ok(())),
                    Err("Draft construction source is already in use".into())
                );
            }
            budget.charge()
        },
    );
    assert!(caught && result.is_err());
    assert!(lease.failed.get());
    assert!(!lease.busy.get() && !lease.member_busy.get());
    assert!(
        lease
            .members
            .try_borrow()
            .map_err(|_| "Test member borrow")?
            .is_empty()
    );
    assert!(
        context
            .source_lease(&metadata, &sources, &mut budget)
            .is_err()
    );
    let foreign = DraftConstructionContext::bind(
        definition.schema(),
        definition.sources(),
        &native,
        &mut budget,
    )?;
    let sources = PendingSourceInputs::new(&metadata, &mut budget)?;
    let lease = context.source_lease(&metadata, &sources, &mut budget)?;
    lease.permits_absence(
        &context,
        ObjectView::from_element(&element),
        &member.name,
        &member.identity,
        &mut || budget.charge(),
    )?;
    let calls = native.calls.get();
    assert!(
        lease
            .permits_absence(
                &foreign,
                ObjectView::from_element(&element),
                &member.name,
                &member.identity,
                &mut || budget.charge()
            )
            .is_err()
    );
    assert_eq!(native.calls.get(), calls);
    assert!(
        lease
            .members
            .try_borrow()
            .map_err(|_| "Test member borrow")?
            .is_empty()
    );
    // The same original component ledger pays for a NEW independent cold owner.
    let sources = PendingSourceInputs::new(&metadata, &mut budget)?;
    let lease = context.source_lease(&metadata, &sources, &mut budget)?;
    let before = budget.reads;
    assert!(lease.permits_absence(
        &context,
        ObjectView::from_element(&element),
        &member.name,
        &member.identity,
        &mut || budget.charge()
    )?);
    assert_eq!(budget.reads - before, 211);
    println!(
        "CONSTRUCTION_MEMBER_INDEX_REENTRY caught=true foreign_context=true fresh_recovery_cold=211"
    );
    Ok(())
}

#[test]
fn constructor_member_index_same_atomic_graph_callbacks_and_late_provider_rollback()
-> StorageResult<()> {
    wide_setup!(definition, metadata, native, context, _element);
    let mut old =
        FrozenStore::for_schema(definition.schema()).map_err(|error| error.to_string())?;
    let transaction = old.begin()?;
    bind_schema(&mut old, definition.schema())?;
    let mut old_budget = QueryBudget::new(100_000);
    for id in ["a", "b"] {
        context.create_pending(
            &mut old,
            Construction {
                id,
                resource: "memory:member-index",
                authored: &[],
            },
            "A",
            2,
            4,
            &metadata,
            &mut old_budget,
        )?;
    }
    let expected = export(&old)?;
    let calls = native.calls.get();
    old.rollback(&transaction)?;
    native.calls.set(0);
    let mut store =
        FrozenStore::for_schema(definition.schema()).map_err(|error| error.to_string())?;
    let transaction = store.begin()?;
    bind_schema(&mut store, definition.schema())?;
    let mut budget = QueryBudget::new(100_000);
    let sources = PendingSourceInputs::new(&metadata, &mut budget)?;
    let lease = context.source_lease(&metadata, &sources, &mut budget)?;
    for id in ["a", "b"] {
        lease.create_pending(
            &mut store,
            Construction {
                id,
                resource: "memory:member-index",
                authored: &[],
            },
            "A",
            2,
            4,
            &mut budget,
        )?;
    }
    assert_eq!(export(&store)?, expected);
    assert_eq!(native.calls.get(), calls);
    assert!(budget.reads < old_budget.reads);
    assert_eq!(
        lease
            .members
            .try_borrow()
            .map_err(|_| "Test member borrow")?
            .len(),
        1
    );
    println!(
        "CONSTRUCTION_MEMBER_INDEX_ATOMIC old_work={} scoped_work={} callbacks={} same_graph=true",
        old_budget.reads, budget.reads, calls
    );
    store.rollback(&transaction)?;
    native.calls.set(0);
    native.drift_at.set(Some(8));
    let transaction = store.begin()?;
    bind_schema(&mut store, definition.schema())?;
    let before = export(&store)?;
    let sources = PendingSourceInputs::new(&metadata, &mut budget)?;
    let lease = context.source_lease(&metadata, &sources, &mut budget)?;
    let result = lease.create_pending(
        &mut store,
        Construction {
            id: "late",
            resource: "memory:member-index",
            authored: &[],
        },
        "A",
        2,
        4,
        &mut budget,
    );
    assert_eq!(
        result.err().as_deref(),
        Some("Draft physical capability changed after binding")
    );
    assert_eq!(native.calls.get(), 8);
    assert_eq!(export(&store)?, before);
    assert!(
        lease
            .members
            .try_borrow()
            .map_err(|_| "Test member borrow")?
            .is_empty()
    );
    store.rollback(&transaction)?;
    Ok(())
}

#[test]
fn constructor_member_index_cumulative_cold_break_even_and_exact_short_ledger() -> StorageResult<()>
{
    wide_setup!(definition, metadata, native, context, element);
    let member = &native.rows[0].members[63];
    let mut prior = QueryBudget::new(301);
    for _ in 0..7 {
        prior.charge()?;
    }
    let prior_inputs = PendingSourceInputs::new(&metadata, &mut prior)?;
    let prior_lease = context.source_lease(&metadata, &prior_inputs, &mut prior)?;
    let mut source4_cumulative = Vec::new();
    for expected in [151, 226, 301] {
        assert!(source4_permits_absence(
            &prior_lease,
            &context,
            ObjectView::from_element(&element),
            &member.name,
            &member.identity,
            &mut || prior.charge()
        )?);
        assert_eq!(prior.reads, expected);
        source4_cumulative.push(prior.reads);
    }
    assert_eq!(native.calls.get(), 3);
    assert!(
        prior_lease
            .members
            .try_borrow()
            .map_err(|_| "Test member borrow")?
            .is_empty()
    );
    for grant in [247, 248] {
        native.calls.set(0);
        let mut budget = QueryBudget::new(grant);
        for _ in 0..7 {
            budget.charge()?;
        }
        let inputs = PendingSourceInputs::new(&metadata, &mut budget)?;
        let lease = context.source_lease(&metadata, &inputs, &mut budget)?;
        assert_eq!(budget.reads, 11);
        let mut cumulative = Vec::new();
        let result = (|| {
            for expected in [222, 235, 248] {
                assert!(lease.permits_absence(
                    &context,
                    ObjectView::from_element(&element),
                    &member.name,
                    &member.identity,
                    &mut || budget.charge()
                )?);
                assert_eq!(budget.reads, expected);
                cumulative.push(budget.reads);
            }
            Ok::<(), String>(())
        })();
        assert_eq!(native.calls.get(), 3);
        assert_eq!(budget.reads, grant);
        if grant == 248 {
            result?;
            assert_eq!(cumulative, [222, 235, 248]);
            assert!(!lease.failed.get());
        } else {
            assert_eq!(
                result,
                Err("Query work budget exhausted; no complete result".into())
            );
            assert_eq!(cumulative, [222, 235]);
            assert!(lease.failed.get());
            assert!(
                lease
                    .members
                    .try_borrow()
                    .map_err(|_| "Test member borrow")?
                    .is_empty()
            );
        }
        println!(
            "CONSTRUCTION_MEMBER_INDEX_CUMULATIVE {}",
            serde_json::json!({"grant":grant,"preconsumed":7,"source_setup":4,"source4_with_same_setup":[151,226,301],"indexed_with_same_setup":cumulative,"work":budget.reads,"calls":native.calls.get(),"success":grant==248,"one_use_index_extra":71,"two_use_index_extra":9,"three_use_index_net_saving":53})
        );
    }
    assert_eq!(source4_cumulative, [151, 226, 301]);
    Ok(())
}
