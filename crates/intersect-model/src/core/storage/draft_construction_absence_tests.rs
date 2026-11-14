//! Construction-only physical absence; no union getter or validation authority.
use super::*;
use crate::core::frozen::FrozenStore;
use std::cell::Cell;

struct Policy {
    rows: Vec<DraftClassContract>,
    changed_rows: Vec<DraftClassContract>,
    changed: Cell<bool>,
}
impl DraftPhysicalDefaults for Policy {
    fn contracts(&self) -> &[DraftClassContract] {
        if self.changed.get() {
            &self.changed_rows
        } else {
            &self.rows
        }
    }
}
fn package() -> StorageResult<crate::RcorePackage> {
    crate::compile_rcore_at(
        "memory:construction-absence.rcore",
        r#"rcore 1; package constructionAbsence version "1";
        metamodel "urn:construction-absence" prefix c { class Item {
            attr stored: String [0,*];
            attr delegated: String [0,*];
            ref links: Item [0,*];
        } }"#,
    )
    .map_err(|e| e.to_string())
}
fn policy(package: &crate::RcorePackage) -> StorageResult<Policy> {
    let schema = package.schema();
    let entry = &package.sources().sources.first().ok_or("No source")?.id;
    let class = schema.class_key(entry, "urn:construction-absence", "Item")?;
    let members = ["delegated", "links", "stored"]
        .into_iter()
        .map(|name| {
            Ok(DraftMemberDisposition {
                name: name.into(),
                identity: schema.effective_feature(class, name)?.identity,
                disposition: if name == "stored" {
                    DraftDisposition::StoredDefault
                } else {
                    DraftDisposition::DelegatedAbsent
                },
            })
        })
        .collect::<StorageResult<Vec<_>>>()?;
    let row = DraftClassContract {
        version: 1,
        schema_revision: schema.revision.clone(),
        class: class.into(),
        class_identity: schema.identity(class),
        sources: package
            .sources()
            .sources
            .iter()
            .map(|s| s.id.clone())
            .collect(),
        members,
    };
    let mut changed = row.clone();
    changed.class_identity = "stale-native-policy".into();
    Ok(Policy {
        rows: vec![row],
        changed_rows: vec![changed],
        changed: Cell::new(false),
    })
}
fn audit(
    store: &impl ModelStore,
    schema: &Schema,
    context: &DraftConstructionContext<'_>,
) -> StorageResult<()> {
    let mut preparation = StructuralPreparation::draft(DraftReferencePolicy::Resolved);
    let mut budget = QueryBudget::new(100_000);
    while !preparation.advance_construction(store, schema, context, &mut budget)? {}
    preparation.finish()?;
    Ok(())
}

#[test]
fn absent_multiple_creation_is_atomic_and_ordinary_validation_stays_strict() -> StorageResult<()> {
    let package = package()?;
    let schema = package.schema();
    let policy = policy(&package)?;
    let class = policy.rows[0].class.as_str();
    let context = DraftConstructionContext::bind(
        schema,
        package.sources(),
        &policy,
        &mut QueryBudget::new(100_000),
    )?;
    let mut store = FrozenStore::for_schema(schema).map_err(|error| error.to_string())?;
    let before = export(&store)?;
    let mut zero = QueryBudget::new(0);
    assert!(
        atomic(&mut store, |s| context.create(
            s,
            Construction {
                id: "zero",
                resource: "memory:absence",
                authored: &[]
            },
            class,
            2,
            4,
            &mut zero,
        ))
        .is_err()
    );
    assert_eq!(zero.reads, 0);
    assert_eq!(export(&store)?, before);
    let mut exact = QueryBudget::new(100_000);
    atomic(&mut store, |s| {
        context.create(
            s,
            Construction {
                id: "item",
                resource: "memory:absence",
                authored: &[],
            },
            class,
            2,
            4,
            &mut exact,
        )
    })?;
    let measured = exact.reads;
    // One initial create + three two-unit feature guards, then two paid
    // exact source/provider/member absence checks (ten and eleven units).
    assert_eq!(measured, 28);
    for (grant, success) in [(measured - 1, false), (measured, true)] {
        let mut fresh = FrozenStore::for_schema(schema).map_err(|error| error.to_string())?;
        let before = export(&fresh)?;
        let mut ledger = QueryBudget::new(grant);
        let result = atomic(&mut fresh, |s| {
            context.create(
                s,
                Construction {
                    id: "boundary",
                    resource: "memory:boundary",
                    authored: &[],
                },
                class,
                0,
                1,
                &mut ledger,
            )
        });
        assert_eq!(result.is_ok(), success);
        assert_eq!(ledger.reads, grant);
        if !success {
            assert_eq!(export(&fresh)?, before);
        }
    }
    let mut prepaid = FrozenStore::for_schema(schema).map_err(|error| error.to_string())?;
    let mut ledger = QueryBudget::new(measured + 7);
    for _ in 0..7 {
        ledger.charge()?;
    }
    atomic(&mut prepaid, |s| {
        context.create(
            s,
            Construction {
                id: "prepaid",
                resource: "memory:prepaid",
                authored: &[],
            },
            class,
            0,
            1,
            &mut ledger,
        )
    })?;
    assert_eq!(ledger.reads, measured + 7);
    let item = store.get("item")?.ok_or("No item")?;
    assert_eq!(item.values.len(), 1);
    assert_eq!(item.values.get("stored"), Some(&serde_json::json!([])));
    assert!(!item.values.contains_key("delegated"));
    assert!(!item.values.contains_key("links"));
    assert_eq!(
        validate_draft(&store, schema)
            .as_ref()
            .err()
            .map(String::as_str),
        Some("Multiple feature requires a list")
    );
    assert!(validate(&store, schema).is_err());
    assert_eq!(
        audit(&store, schema, &context)
            .as_ref()
            .err()
            .map(String::as_str),
        Some("Storage write requires a transaction")
    );
    atomic(&mut store, |s| audit(s, schema, &context))?;
    // The same borrowed policy cannot be used to mint a strict OwnershipIndex.
    assert!(
        atomic(&mut store, |s| {
            let mut strict = StructuralPreparation::ownership();
            strict.advance_construction(s, schema, &context, &mut QueryBudget::new(100_000))
        })
        .is_err()
    );
    println!(
        "CONSTRUCTION_ABSENCE_ATOMIC {}",
        serde_json::json!({"zero":0,"exact":measured,"one_short":measured-1,"preconsumed":7,"ordinary_missing_many_rejects":true,"strict_authority":false,"context_bytes":std::mem::size_of::<DraftConstructionContext<'_>>(),"structural_bytes":std::mem::size_of::<StructuralPreparation>(),"private_per_class_selection_bytes":std::mem::size_of::<StoredSelection>()})
    );
    Ok(())
}

#[test]
fn present_values_and_missing_stored_slots_use_shared_structural_checks() -> StorageResult<()> {
    let package = package()?;
    let schema = package.schema();
    let policy = policy(&package)?;
    let class = policy.rows[0].class.as_str();
    let context = DraftConstructionContext::bind(
        schema,
        package.sources(),
        &policy,
        &mut QueryBudget::new(100_000),
    )?;
    for (name, value, expected) in [
        (
            "stored",
            None,
            "Missing stored draft physical construction slot",
        ),
        (
            "delegated",
            Some(serde_json::json!(null)),
            "Multiple feature requires a list",
        ),
        (
            "delegated",
            Some(serde_json::json!("not-a-list")),
            "Multiple feature requires a list",
        ),
        ("delegated", Some(serde_json::json!([42])), "Invalid"),
        (
            "links",
            Some(serde_json::json!(["missing"])),
            "Missing reference target",
        ),
    ] {
        let mut store = FrozenStore::for_schema(schema).map_err(|error| error.to_string())?;
        let before = export(&store)?;
        let mut object = Element::new(schema, "bad", class, "memory:bad", 1, 2)?;
        if name != "stored" {
            object.values.remove("delegated");
            object.values.remove("links");
        }
        if let Some(value) = value {
            object.values.insert(name.into(), value);
        } else {
            object.values.remove(name);
        }
        let result = atomic(&mut store, |s| {
            bind_schema(s, schema)?;
            s.put(object)?;
            if name == "stored" {
                assert_eq!(
                    validate_draft(s, schema).as_ref().err().map(String::as_str),
                    Some("Multiple feature requires a list")
                );
            }
            audit(s, schema, &context)
        });
        assert!(
            result
                .as_ref()
                .err()
                .is_some_and(|error| error.contains(expected)),
            "{name}: {result:?}"
        );
        assert_eq!(export(&store)?, before);
    }
    // A present, correctly shaped/value-checked delegate is not excused or
    // rejected merely by its absence disposition. Observation is separate.
    let mut store = FrozenStore::for_schema(schema).map_err(|error| error.to_string())?;
    let mut object = Element::new(schema, "present", class, "memory:present", 0, 1)?;
    object
        .values
        .insert("delegated".into(), serde_json::json!(["real"]));
    atomic(&mut store, |s| {
        bind_schema(s, schema)?;
        s.put(object)?;
        audit(s, schema, &context)
    })?;
    assert!(
        context
            .observe(&store, "present", &mut QueryBudget::new(5))
            .is_err()
    );
    Ok(())
}

#[test]
fn source_receiver_member_and_live_provider_guards_dominate_absence() -> StorageResult<()> {
    let package = package()?;
    let schema = package.schema();
    let policy = policy(&package)?;
    let class = policy.rows[0].class.as_str();
    let context = DraftConstructionContext::bind(
        schema,
        package.sources(),
        &policy,
        &mut QueryBudget::new(100_000),
    )?;
    let mut foreign = policy.rows.clone();
    foreign[0].members[0].identity = schema.effective_feature(class, "stored")?.identity;
    let bad = Policy {
        rows: foreign,
        changed_rows: Vec::new(),
        changed: Cell::new(false),
    };
    assert!(
        DraftConstructionContext::bind(
            schema,
            package.sources(),
            &bad,
            &mut QueryBudget::new(100_000)
        )
        .is_err()
    );
    let object = Element::new(schema, "probe", class, "memory:probe", 0, 1)?;
    let mut wrong = object.clone();
    wrong.type_identity = "foreign".into();
    let member = &policy.rows[0].members[0];
    assert!(
        context
            .permits_absent_construction_slot(
                ObjectView::from_element(&wrong),
                &member.name,
                &member.identity,
                &mut QueryBudget::new(100_000)
            )
            .is_err()
    );
    assert!(
        context
            .permits_absent_construction_slot(
                ObjectView::from_element(&object),
                &member.name,
                &policy.rows[0].members[2].identity,
                &mut QueryBudget::new(100_000)
            )
            .is_err()
    );
    let schema_clone = schema.clone();
    assert!(context.check_construction_schema(&schema_clone).is_err());
    policy.changed.set(true);
    let mut store = FrozenStore::for_schema(schema).map_err(|error| error.to_string())?;
    let before = export(&store)?;
    let mut budget = QueryBudget::new(1);
    assert!(
        atomic(&mut store, |s| context.create(
            s,
            Construction {
                id: "stale",
                resource: "memory:stale",
                authored: &[]
            },
            class,
            0,
            1,
            &mut budget
        ))
        .is_err()
    );
    assert_eq!(budget.reads, 1);
    assert_eq!(export(&store)?, before);
    policy.changed.set(false);
    let mut object = object;
    object.values.remove("delegated");
    object.values.remove("links");
    let mut late_policy_work = QueryBudget::new(100_000);
    let result = atomic(&mut store, |s| {
        bind_schema(s, schema)?;
        s.put(object)?;
        let mut preparation = StructuralPreparation::draft(DraftReferencePolicy::Resolved);
        let budget = &mut late_policy_work;
        assert!(!preparation.advance_construction(s, schema, &context, budget)?);
        policy.changed.set(true);
        while !preparation.advance_construction(s, schema, &context, budget)? {}
        preparation.finish()?;
        Ok(())
    });
    assert_eq!(
        result.as_ref().err().map(String::as_str),
        Some("Draft physical capability changed after binding")
    );
    assert_eq!(late_policy_work.reads, 5);
    assert_eq!(export(&store)?, before);
    policy.changed.set(false);
    let mut budget = QueryBudget::new(100_000);
    atomic(&mut store, |s| {
        context.create(
            s,
            Construction {
                id: "recovery",
                resource: "memory:recovery",
                authored: &[],
            },
            class,
            0,
            1,
            &mut budget,
        )
    })?;
    Ok(())
}
