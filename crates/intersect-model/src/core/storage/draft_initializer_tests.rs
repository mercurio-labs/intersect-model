//! Real authored-source construction through the shared native initializer.
//! Current-parent initializer projection: no held required-null marker is selected.
use super::*;
use crate::core::frozen::FrozenStore;
use crate::core::storage::pending_source::tests::{definition, inputs_for};
use crate::definitions::{
    conversion::{InvocationContext, ProviderId},
    preparation::PreparedModelDefinition,
};
use std::cell::{Cell, RefCell};

const SOURCE: &str = r##"<e:EPackage xmlns:e="http://www.eclipse.org/emf/2002/Ecore" xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance" name="reuse" nsURI="urn:source-reuse">
<eClassifiers xsi:type="e:EEnum" name="Kind"><eLiterals name="first"/><eLiterals name="second" value="1"/></eClassifiers>
<eClassifiers xsi:type="e:EClass" name="A"><eStructuralFeatures xsi:type="e:EAttribute" name="operator" lowerBound="1" changeable="false" defaultValueLiteral="+" eType="e:EDataType http://www.eclipse.org/emf/2002/Ecore#//EString"/><eStructuralFeatures xsi:type="e:EAttribute" name="requiredKind" lowerBound="1" defaultValueLiteral="first" eType="#//Kind"/><eStructuralFeatures xsi:type="e:EAttribute" name="notes" upperBound="-1" eType="e:EDataType http://www.eclipse.org/emf/2002/Ecore#//EString"/></eClassifiers>
<eClassifiers xsi:type="e:EClass" name="B"><eStructuralFeatures xsi:type="e:EAttribute" name="operator" lowerBound="1" defaultValueLiteral="+" eType="e:EDataType http://www.eclipse.org/emf/2002/Ecore#//EString"/></eClassifiers>
</e:EPackage>"##;

struct Native(Vec<DraftClassContract>);
impl DraftPhysicalDefaults for Native {
    fn contracts(&self) -> &[DraftClassContract] {
        &self.0
    }
}
fn native(d: &PreparedModelDefinition<'_>) -> StorageResult<Native> {
    let schema = d.schema();
    let mut rows = Vec::new();
    for class in ["A", "B"] {
        rows.push(DraftClassContract {
            version: 1,
            schema_revision: schema.revision.clone(),
            class: class.into(),
            class_identity: schema.identity(class),
            sources: d.sources().sources.iter().map(|s| s.id.clone()).collect(),
            members: schema
                .effective_features(class)?
                .into_iter()
                .map(|m| DraftMemberDisposition {
                    name: m.feature.name.clone(),
                    identity: m.identity,
                    disposition: if m.feature.name == "notes" {
                        DraftDisposition::DelegatedAbsent
                    } else {
                        DraftDisposition::StoredDefault
                    },
                })
                .collect(),
        });
    }
    Ok(Native(rows))
}
struct Initializer<'a> {
    contract: ClassInitializationContract,
    drift: ClassInitializationContract,
    mode: Cell<usize>,
    changed: Cell<bool>,
    calls: Cell<usize>,
    contract_calls: Cell<usize>,
    on_final: Option<&'a dyn Fn()>,
    required_value: Value,
    initial_default: Value,
    seen_pending_enum_null: Cell<bool>,
    trace: RefCell<Vec<String>>,
}
impl<'a> Initializer<'a> {
    fn new(d: &PreparedModelDefinition<'_>) -> StorageResult<Self> {
        let contract = ClassInitializationContract {
            version: 1,
            schema_revision: d.schema().revision.clone(),
            class: "A".into(),
            class_identity: d.schema().identity("A"),
            provider: ProviderId {
                name: "authored.native-operator".into(),
                version: "1".into(),
            },
            sources: d.sources().sources.iter().map(|s| s.id.clone()).collect(),
            members: vec![
                d.schema().effective_feature("A", "operator")?.identity,
                d.schema().effective_feature("A", "requiredKind")?.identity,
            ],
        };
        let mut drift = contract.clone();
        drift.provider.version = "changed-during-plan".into();
        Ok(Self {
            contract,
            drift,
            mode: Cell::new(0),
            changed: Cell::new(false),
            calls: Cell::new(0),
            contract_calls: Cell::new(0),
            on_final: None,
            required_value: serde_json::to_value(d.schema().enumerations["Kind"].parse("second")?)
                .map_err(|e| e.to_string())?,
            initial_default: d
                .schema()
                .effective_feature("A", "operator")?
                .feature
                .default
                .clone(),
            seen_pending_enum_null: Cell::new(false),
            trace: RefCell::new(Vec::new()),
        })
    }
}
impl ClassInitializer for Initializer<'_> {
    fn contract(&self) -> &ClassInitializationContract {
        let call = self.contract_calls.get() + 1;
        self.contract_calls.set(call);
        if call >= 3
            && let Some(callback) = self.on_final
        {
            callback();
        }
        if self.changed.get() || (self.mode.get() == 6 && call >= 3) {
            &self.drift
        } else {
            &self.contract
        }
    }
    fn plan(
        &self,
        store: &dyn ModelRead,
        id: &str,
        budget: &mut QueryBudget,
    ) -> StorageResult<Vec<InitialValue>> {
        budget.charge()?;
        self.calls.set(self.calls.get() + 1);
        self.trace.borrow_mut().push(id.into());
        if store.class(id)?.as_deref() != Some("A")
            || store.value(id, "operator")? != Some(self.initial_default.clone())
        {
            return Err("Initializer must observe SAME freshly inserted A and default".into());
        }
        self.seen_pending_enum_null
            .set(store.value(id, "requiredKind")? == Some(Value::Null));
        if self.mode.get() == 1 {
            return Err("native initializer cancelled".into());
        }
        if self.mode.get() == 7 {
            panic!("authored initializer unwind");
        }
        if self.mode.get() == 4 {
            return Ok(Vec::new());
        }
        if self.mode.get() == 5 {
            self.changed.set(true);
        }
        let feature = self
            .contract
            .members
            .first()
            .ok_or("No actual initializer member")?
            .clone();
        let mut plan = vec![InitialValue {
            feature: feature.clone(),
            value: if self.mode.get() == 2 {
                Value::Bool(true)
            } else {
                Value::String(".".into())
            },
        }];
        plan.push(InitialValue {
            feature: self
                .contract
                .members
                .get(1)
                .ok_or("No required enum member")?
                .clone(),
            value: self.required_value.clone(),
        });
        if self.mode.get() == 3 {
            plan.push(InitialValue {
                feature,
                value: Value::String(".".into()),
            });
        }
        Ok(plan)
    }
}
fn with_definition(
    f: impl FnOnce(&PreparedModelDefinition<'_>) -> StorageResult<()>,
) -> StorageResult<()> {
    with_source_definition(SOURCE, f)
}
fn with_source_definition(
    source: &str,
    f: impl FnOnce(&PreparedModelDefinition<'_>) -> StorageResult<()>,
) -> StorageResult<()> {
    let (input, migration) = inputs_for(source)?;
    f(&definition(&input, &migration)?)
}
fn empty_store(schema: &Schema) -> StorageResult<FrozenStore> {
    let mut store = FrozenStore::for_schema(schema).map_err(|error| error.to_string())?;
    atomic(&mut store, |s| bind_schema(s, schema))?;
    Ok(store)
}
fn request(id: &str) -> Construction<'_> {
    Construction {
        id,
        resource: "memory:draft-initializer",
        authored: &[],
    }
}
fn binding<'a>(
    p: &'a Initializer<'_>,
    d: &'a PreparedModelDefinition<'_>,
) -> InitializationBinding<'a> {
    InitializationBinding {
        expected: &p.contract,
        sources: &d.sources().sources,
        provider: Some(p),
        conversions: None,
    }
}

#[test]
fn draft_native_initializer_same_new_object_shared_plan_readonly_receipt_and_reload()
-> StorageResult<()> {
    with_definition(|d| {
        let metadata = d
            .prepare_physical_metadata(&InvocationContext::default())
            .map_err(|e| e.to_string())?;
        let native = native(d)?;
        let context = DraftConstructionContext::bind(
            d.schema(),
            d.sources(),
            &native,
            &mut QueryBudget::new(100_000),
        )?;
        let provider = Initializer::new(d)?;
        let initial = binding(&provider, d);
        let mut store = empty_store(d.schema())?;
        let mut budget = QueryBudget::new(1_000_000);
        let sources = PendingSourceInputs::new(&metadata, &mut budget)?;
        let lease = context.source_lease(&metadata, &sources, &mut budget)?;
        atomic(&mut store, |s| {
            lease.create_pending_with_initializer(
                s,
                request("a"),
                "A",
                17,
                23,
                &initial,
                &mut budget,
            )
        })?;
        assert_eq!(provider.calls.get(), 1);
        assert!(!provider.seen_pending_enum_null.get());
        assert_eq!(*provider.trace.borrow(), ["a"]);
        assert_eq!(
            store.value("a", "operator")?,
            Some(Value::String(".".into()))
        );
        assert_eq!(
            store.value("a", "notes")?,
            None,
            "Existing absent-many route must stay usable"
        );
        let model = export(&store)?;
        assert_eq!(model.elements.len(), 1);
        assert_eq!((model.elements[0].start, model.elements[0].end), (17, 23));
        let receipt = model.elements[0]
            .initialization
            .as_ref()
            .ok_or("Missing canonical initialization receipt")?;
        assert_eq!(receipt.contract, provider.contract);
        assert_eq!(receipt.object, "a");
        assert!(receipt.authored_members.is_empty());
        let reloaded = FrozenStore::from_materialized(d.schema(), &model)?;
        assert_eq!(export(&reloaded)?, model);
        let before = serde_json::to_value(export(&store)?).map_err(|e| e.to_string())?;
        assert!(
            atomic(&mut store, |s| set_inner(
                s,
                d.schema(),
                "a",
                "operator",
                Value::String("changed".into()),
                None,
            ))
            .is_err()
        );
        assert_eq!(
            serde_json::to_value(export(&store)?).map_err(|e| e.to_string())?,
            before
        );
        // The ordinary initializer API uses this SAME native plan and receipt.
        let ordinary = Initializer::new(d)?;
        let mut old = empty_store(d.schema())?;
        let receipt = InitializationContext {
            schema: d.schema(),
            expected: &ordinary.contract,
            sources: &d.sources().sources,
            provider: Some(&ordinary),
            conversions: None,
        }
        .create(&mut old, request("a"), &mut QueryBudget::new(100_000))?;
        assert_eq!(
            receipt,
            model.elements[0]
                .initialization
                .clone()
                .ok_or("Missing receipt")?
        );
        assert_eq!(old.value("a", "operator")?, store.value("a", "operator")?);
        assert_eq!(*ordinary.trace.borrow(), *provider.trace.borrow());
        assert!(
            !ordinary.seen_pending_enum_null.get(),
            "Both current routes retain the literal enum default before the shared native plan"
        );
        assert_eq!(
            store.value("a", "requiredKind")?,
            Some(provider.required_value.clone())
        );
        Ok(())
    })?;
    // Additional optional schema-null scalar, with an ordinary StoredDefault
    // disposition. No null-default hack or schema literal replacement.
    let null_source = SOURCE.replace(
        "name=\"operator\" lowerBound=\"1\" changeable=\"false\" defaultValueLiteral=\"+\"",
        "name=\"operator\" changeable=\"false\"",
    );
    assert_ne!(null_source, SOURCE);
    with_source_definition(&null_source, |d| {
        let field = d.schema().effective_feature("A", "operator")?;
        assert_eq!(field.feature.default, Value::Null);
        assert_eq!(field.feature.lower, 0);
        let metadata = d
            .prepare_physical_metadata(&InvocationContext::default())
            .map_err(|e| e.to_string())?;
        let native = native(d)?;
        assert_eq!(
            native.0[0]
                .members
                .iter()
                .find(|m| m.name == "operator")
                .ok_or("Missing operator disposition")?
                .disposition,
            DraftDisposition::StoredDefault
        );
        let context = DraftConstructionContext::bind(
            d.schema(),
            d.sources(),
            &native,
            &mut QueryBudget::new(100_000),
        )?;
        let provider = Initializer::new(d)?;
        assert_eq!(provider.initial_default, Value::Null);
        let mut store = empty_store(d.schema())?;
        let mut budget = QueryBudget::new(1_000_000);
        let sources = PendingSourceInputs::new(&metadata, &mut budget)?;
        let lease = context.source_lease(&metadata, &sources, &mut budget)?;
        atomic(&mut store, |s| {
            lease.create_pending_with_initializer(
                s,
                request("null-default"),
                "A",
                5,
                9,
                &binding(&provider, d),
                &mut budget,
            )
        })?;
        assert_eq!(provider.calls.get(), 1);
        assert!(!provider.seen_pending_enum_null.get());
        assert_eq!(
            store.value("null-default", "operator")?,
            Some(Value::String(".".into()))
        );
        assert_eq!(
            d.schema()
                .effective_feature("A", "operator")?
                .feature
                .default,
            Value::Null
        );
        assert_eq!(store.value("null-default", "notes")?, None);
        let model = export(&store)?;
        let reloaded = FrozenStore::from_materialized(d.schema(), &model)?;
        assert_eq!(export(&reloaded)?, model);
        Ok(())
    })
}

#[test]
fn draft_native_initializer_wrong_plan_source_class_collision_and_cancel_roll_back()
-> StorageResult<()> {
    with_definition(|d| {
        let metadata = d
            .prepare_physical_metadata(&InvocationContext::default())
            .map_err(|e| e.to_string())?;
        let native = native(d)?;
        let context = DraftConstructionContext::bind(
            d.schema(),
            d.sources(),
            &native,
            &mut QueryBudget::new(100_000),
        )?;
        for mode in 1..=7 {
            let provider = Initializer::new(d)?;
            provider.mode.set(mode);
            let initial = binding(&provider, d);
            let mut store = empty_store(d.schema())?;
            let before = serde_json::to_value(export(&store)?).map_err(|e| e.to_string())?;
            let mut budget = QueryBudget::new(1_000_000);
            let sources = PendingSourceInputs::new(&metadata, &mut budget)?;
            let lease = context.source_lease(&metadata, &sources, &mut budget)?;
            let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                atomic(&mut store, |s| {
                    lease.create_pending_with_initializer(
                        s,
                        request("a"),
                        "A",
                        0,
                        1,
                        &initial,
                        &mut budget,
                    )
                })
            }));
            if mode == 7 {
                assert!(outcome.is_err(), "Expected genuine provider unwind");
            } else {
                let error = outcome
                    .map_err(|_| "Unexpected provider unwind")?
                    .err()
                    .ok_or("Bad native plan escaped")?;
                assert!(!error.to_ascii_lowercase().contains("budget"), "{error}");
            }
            assert_eq!(provider.calls.get(), 1);
            assert_eq!(
                serde_json::to_value(export(&store)?).map_err(|e| e.to_string())?,
                before
            );
            assert!(
                context
                    .source_lease(&metadata, &sources, &mut QueryBudget::new(1_000_000))
                    .is_err(),
                "Failed source invocation cannot revive"
            );
        }
        for corruption in 0..3 {
            let mut provider = Initializer::new(d)?;
            match corruption {
                0 => provider.contract.sources[0].sha256 = "foreign".into(),
                1 => {
                    provider.contract.members[0] =
                        d.schema().effective_feature("B", "operator")?.identity
                }
                _ => {
                    provider.contract.class = "B".into();
                    provider.contract.class_identity = d.schema().identity("B");
                }
            }
            let initial = binding(&provider, d);
            let mut store = empty_store(d.schema())?;
            let mut budget = QueryBudget::new(1_000_000);
            let sources = PendingSourceInputs::new(&metadata, &mut budget)?;
            let lease = context.source_lease(&metadata, &sources, &mut budget)?;
            assert!(
                atomic(&mut store, |s| lease.create_pending_with_initializer(
                    s,
                    request("a"),
                    "A",
                    0,
                    1,
                    &initial,
                    &mut budget
                ))
                .is_err()
            );
            assert_eq!(provider.calls.get(), 0);
            assert!(export(&store)?.elements.is_empty());
        }
        // Collision cannot turn the fresh-only API into a readonly existing-object write.
        let provider = Initializer::new(d)?;
        let mut store = empty_store(d.schema())?;
        atomic(&mut store, |s| {
            context.create(s, request("a"), "A", 0, 1, &mut QueryBudget::new(100_000))
        })?;
        let before = serde_json::to_value(export(&store)?).map_err(|e| e.to_string())?;
        let mut budget = QueryBudget::new(1_000_000);
        let sources = PendingSourceInputs::new(&metadata, &mut budget)?;
        let lease = context.source_lease(&metadata, &sources, &mut budget)?;
        assert!(
            atomic(&mut store, |s| lease.create_pending_with_initializer(
                s,
                request("a"),
                "A",
                0,
                1,
                &binding(&provider, d),
                &mut budget
            ))
            .is_err()
        );
        assert_eq!(provider.calls.get(), 0);
        assert_eq!(
            serde_json::to_value(export(&store)?).map_err(|e| e.to_string())?,
            before
        );
        // The final provider callback catches a nested source denial and still
        // returns its unchanged contract. The late source guard must reject it.
        let mut store = empty_store(d.schema())?;
        let before = serde_json::to_value(export(&store)?).map_err(|e| e.to_string())?;
        let mut budget = QueryBudget::new(1_000_000);
        let sources = PendingSourceInputs::new(&metadata, &mut budget)?;
        let lease = context.source_lease(&metadata, &sources, &mut budget)?;
        let denied = Cell::new(false);
        let nested = std::cell::RefCell::new(empty_store(d.schema())?);
        let caught = || {
            denied.set(
                lease
                    .create_pending(
                        &mut *nested.borrow_mut(),
                        request("never-created"),
                        "A",
                        0,
                        1,
                        &mut QueryBudget::new(0),
                    )
                    .is_err(),
            );
        };
        let mut provider = Initializer::new(d)?;
        provider.on_final = Some(&caught);
        let result = atomic(&mut store, |s| {
            lease.create_pending_with_initializer(
                s,
                request("late-source"),
                "A",
                0,
                1,
                &binding(&provider, d),
                &mut budget,
            )
        });
        assert!(result.is_err());
        assert!(denied.get());
        assert_eq!(provider.calls.get(), 1);
        assert_eq!(provider.contract_calls.get(), 3);
        assert_eq!(
            serde_json::to_value(export(&store)?).map_err(|e| e.to_string())?,
            before
        );
        Ok(())
    })
}

#[test]
fn draft_native_initializer_same_preconsumed_caller_exact_short_zero_and_default_route()
-> StorageResult<()> {
    with_definition(|d| {
        let metadata = d
            .prepare_physical_metadata(&InvocationContext::default())
            .map_err(|e| e.to_string())?;
        let native = native(d)?;
        let context = DraftConstructionContext::bind(
            d.schema(),
            d.sources(),
            &native,
            &mut QueryBudget::new(100_000),
        )?;
        let execute = |grant: usize| -> StorageResult<(usize, bool)> {
            let provider = Initializer::new(d)?;
            let mut store = empty_store(d.schema())?;
            let mut setup = QueryBudget::new(100_000);
            let sources = PendingSourceInputs::new(&metadata, &mut setup)?;
            let lease = context.source_lease(&metadata, &sources, &mut setup)?;
            let mut budget =
                QueryBudget::new(grant.checked_add(7).ok_or("Fixture grant overflow")?);
            for _ in 0..7 {
                budget.charge()?;
            }
            let result = atomic(&mut store, |s| {
                lease.create_pending_with_initializer(
                    s,
                    request("a"),
                    "A",
                    0,
                    1,
                    &binding(&provider, d),
                    &mut budget,
                )
            });
            let success = result.is_ok();
            if !success {
                assert!(export(&store)?.elements.is_empty());
                assert!(
                    context
                        .source_lease(&metadata, &sources, &mut QueryBudget::new(100_000))
                        .is_err()
                );
            } else {
                assert_eq!(
                    store.value("a", "operator")?,
                    Some(Value::String(".".into()))
                );
            }
            Ok((budget.reads - 7, success))
        };
        let (used, success) = execute(1_000_000)?;
        assert!(success && used > 0);
        for (grant, expected) in [(used, true), (used - 1, false), (0, false)] {
            let (actual, ok) = execute(grant)?;
            assert_eq!(ok, expected);
            assert_eq!(actual, grant);
        }
        // Unselected ordinary draft creation still stores its original default,
        // has no initialization receipt, and invokes no native initializer.
        let mut store = empty_store(d.schema())?;
        let mut work = QueryBudget::new(100_000);
        let sources = PendingSourceInputs::new(&metadata, &mut work)?;
        let lease = context.source_lease(&metadata, &sources, &mut work)?;
        atomic(&mut store, |s| {
            lease.create_pending(s, request("old"), "A", 0, 1, &mut work)
        })?;
        let model = export(&store)?;
        assert_eq!(
            store.value("old", "operator")?,
            Some(Value::String("+".into()))
        );
        assert!(model.elements[0].initialization.is_none());
        Ok(())
    })
}
