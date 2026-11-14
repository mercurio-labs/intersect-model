//! Missing-many delegated values must reach the explicit full structural audit.
use crate::handle_id;
use intersect_model::{
    core::{
        api::BasicModel,
        frozen::FrozenStore,
        model::Model,
        query::QueryBudget,
        resource_set::{ResourceContext, ResourceSet},
    },
    definitions::{
        bundle::{DefinitionBundle, DefinitionSource, SourceKind},
        rcore::compile_structure,
        schema::Schema,
    },
    storage::{
        self, InvocationContext, ModelRead, StructuralValueRequest, StructuralValueSelector,
    },
};
use serde_json::{Value, json};
use std::{cell::Cell, collections::BTreeMap};

fn schema() -> Result<Schema, String> {
    let source = DefinitionSource::new(
        "memory:adopt-delegates.rcore",
        SourceKind::Rcore,
        r#"rcore 1; package adoption version "1";
metamodel "urn:adopt-delegates" prefix a {
 class Holder { ref backing: Target [0,*]; ref members: Target [1,*]; ref peer: Target [0,1] opposite Target.holder; }
 class Target { ref holder: Holder [0,1] opposite Holder.peer; }
}"#,
    );
    let bundle = DefinitionBundle {
        version: 1,
        sources: vec![source.clone()],
        namespaces: vec![],
    };
    Ok(compile_structure(&bundle, &source.id)
        .map_err(|e| format!("{e:?}"))?
        .schema)
}
fn raw(s: &Schema) -> Result<FrozenStore, String> {
    let mut model = Model::default();
    model.create(s, "holder", "Holder", "memory:adopt.xmi", 0, 8)?;
    model.create(s, "target", "Target", "memory:adopt.xmi", 9, 12)?;
    let h = model
        .elements
        .iter_mut()
        .find(|e| e.id == "holder")
        .ok_or("missing holder")?;
    h.values.remove("members");
    h.values.insert("backing".into(), json!(["target"]));
    h.values.insert("peer".into(), json!("target"));
    model
        .elements
        .iter_mut()
        .find(|e| e.id == "target")
        .ok_or("missing target")?
        .values
        .insert("holder".into(), json!("holder"));
    FrozenStore::from_materialized(s, &model)
}
#[derive(Clone, Copy)]
enum Choice {
    Backing,
    None,
    WrongShape,
    WrongTarget,
}
struct Values {
    calls: Cell<usize>,
    choice: Choice,
}
impl StructuralValueSelector for Values {
    fn select(
        &self,
        store: &dyn ModelRead,
        _: &Schema,
        r: StructuralValueRequest<'_>,
        b: &mut QueryBudget,
    ) -> Result<Option<Value>, String> {
        b.charge()?;
        if r.actual == "Holder" && r.feature.name == "members" {
            self.calls.set(self.calls.get() + 1);
            Ok(match self.choice {
                Choice::Backing => store.value(r.object, "backing")?,
                Choice::None => None,
                Choice::WrongShape => Some(json!("target")),
                Choice::WrongTarget => Some(json!(["holder"])),
            })
        } else {
            Ok(None)
        }
    }
}
#[test]
fn delegated_many_adoption_runs_one_full_audit_and_keeps_the_same_store() -> Result<(), String> {
    let schema = schema()?;
    let mut store = raw(&schema)?;
    let handle = store.handle("holder").ok_or("missing handle")?;
    storage::atomic(&mut store, |same| storage::bind_schema(same, &schema))?;
    assert_eq!(handle_id(&store, &handle)?, "holder");
    assert_eq!(store.value("holder", "members")?, None);
    let values = Values {
        calls: Cell::new(0),
        choice: Choice::Backing,
    };
    let called = Cell::new(false);
    let result = ResourceSet::from_store_with_structural_values(
        schema.clone(),
        ResourceContext::new(&schema, BTreeMap::new())?,
        store,
        None,
        &values,
        &mut QueryBudget::new(100_000),
        &InvocationContext::default(),
        |_, _| {
            called.set(true);
            Ok(())
        },
    );
    let set = result?;
    assert_eq!(values.calls.get(), 1, "one full provider-backed audit");
    assert!(called.get());
    assert_eq!(handle_id(set.reader(), &handle)?, "holder");
    assert_eq!(set.reader().value("holder", "members")?, None);
    assert_eq!(
        set.reader().value("holder", "backing")?,
        Some(json!(["target"]))
    );
    assert!(BasicModel::from_store(schema.clone(), raw(&schema)?).is_err());
    assert!(
        ResourceSet::from_store(
            schema.clone(),
            ResourceContext::new(&schema, BTreeMap::new())?,
            raw(&schema)?
        )
        .is_err()
    );
    Ok(())
}

#[derive(Clone)]
struct Shared {
    store: std::rc::Rc<std::cell::RefCell<FrozenStore>>,
    configured: std::rc::Rc<Cell<usize>>,
}
impl Shared {
    fn new(store: FrozenStore) -> Self {
        Self {
            store: std::rc::Rc::new(std::cell::RefCell::new(store)),
            configured: std::rc::Rc::new(Cell::new(0)),
        }
    }
}
impl ModelRead for Shared {
    fn revision(&self) -> Result<String, String> {
        self.store.borrow().revision()
    }
    fn get(&self, id: &str) -> Result<Option<intersect_model::core::model::Element>, String> {
        self.store.borrow().get(id)
    }
    fn class(&self, id: &str) -> Result<Option<String>, String> {
        self.store.borrow().class(id)
    }
    fn page(
        &self,
        after: Option<u64>,
        limit: usize,
    ) -> Result<Vec<intersect_model::storage::Row>, String> {
        self.store.borrow().page(after, limit)
    }
    fn query(&self, q: intersect_model::storage::Query<'_>) -> Result<Vec<String>, String> {
        self.store.borrow().query(q)
    }
    fn matches(&self, id: &str, q: intersect_model::storage::Query<'_>) -> Result<bool, String> {
        self.store.borrow().matches(id, q)
    }
    fn value(&self, id: &str, field: &str) -> Result<Option<Value>, String> {
        self.store.borrow().value(id, field)
    }
    fn inspect(
        &self,
        id: &str,
        f: &mut dyn FnMut(intersect_model::storage::ObjectView<'_>) -> Result<(), String>,
    ) -> Result<bool, String> {
        self.store.borrow().inspect(id, f)
    }
    fn resource_records(
        &self,
    ) -> Result<BTreeMap<String, intersect_model::core::resources::ResourceRecord>, String> {
        self.store.borrow().resource_records()
    }
}
impl intersect_model::storage::ModelStore for Shared {
    fn is_transaction_active(&self) -> bool {
        self.store.borrow().is_transaction_active()
    }
    fn begin(&mut self) -> Result<intersect_model::storage::Checkpoint, String> {
        self.store.borrow_mut().begin()
    }
    fn commit(&mut self, c: &intersect_model::storage::Checkpoint) -> Result<(), String> {
        self.store.borrow_mut().commit(c)
    }
    fn rollback(&mut self, c: &intersect_model::storage::Checkpoint) -> Result<(), String> {
        self.store.borrow_mut().rollback(c)
    }
    fn set_revision(&mut self, r: &str) -> Result<(), String> {
        self.store.borrow_mut().set_revision(r)
    }
    fn put(&mut self, e: intersect_model::core::model::Element) -> Result<(), String> {
        self.store.borrow_mut().put(e)
    }
    fn remove(&mut self, id: &str) -> Result<(), String> {
        self.store.borrow_mut().remove(id)
    }
    fn configure_schema(&mut self, s: &Schema) -> Result<(), String> {
        self.store.borrow_mut().configure_schema(s)?;
        self.configured.set(self.configured.get() + 1);
        Ok(())
    }
}
struct LateControl(Cell<bool>);
impl intersect_model::definitions::conversion::InvocationObserver for LateControl {
    fn before_invoke(
        &self,
        _: &intersect_model::definitions::conversion::ProviderId,
    ) -> Result<(), String> {
        self.checkpoint()
    }
    fn checkpoint(&self) -> Result<(), String> {
        if self.0.get() {
            Err("late control denial".into())
        } else {
            Ok(())
        }
    }
}
#[test]
fn failed_many_adoption_restores_binding_data_resources_and_live_handles() -> Result<(), String> {
    use intersect_model::storage::ModelStore;
    let schema = schema()?;
    // None is an unsupported read, not an empty-list waiver. Wrong shape/type
    // and a later physical reciprocal failure still use the same full audit.
    for mode in 0..6 {
        let shared = Shared::new(raw(&schema)?);
        if mode == 3 {
            storage::atomic(&mut *shared.store.borrow_mut(), |s| {
                s.set_value("target", "holder", Some(Value::Null))
            })?;
        }
        let before = storage::export(&*shared.store.borrow())?;
        let had_schema = shared.store.borrow().metamodel().schema().clone();
        let handle = shared
            .store
            .borrow()
            .handle("holder")
            .ok_or("missing handle")?;
        let selector = Values {
            calls: Cell::new(0),
            choice: match mode {
                0 => Choice::None,
                1 => Choice::WrongShape,
                2 => Choice::WrongTarget,
                _ => Choice::Backing,
            },
        };
        let control = std::rc::Rc::new(LateControl(Cell::new(false)));
        let context = InvocationContext::new(control.clone());
        let language = Cell::new(false);
        let result = ResourceSet::from_store_with_structural_values(
            schema.clone(),
            ResourceContext::new(&schema, BTreeMap::new())?,
            shared.clone(),
            None,
            &selector,
            &mut QueryBudget::new(100_000),
            &context,
            |same, _| {
                assert!(same.is_transaction_active());
                language.set(true);
                if mode >= 4 {
                    same.store
                        .borrow_mut()
                        .set_value("holder", "backing", Some(json!([])))?;
                }
                if mode == 4 {
                    return Err("late language denial".into());
                }
                if mode == 5 {
                    control.0.set(true);
                }
                Ok(())
            },
        );
        let error = result
            .err()
            .ok_or("failed adoption unexpectedly published a facade")?;
        assert_eq!(
            selector.calls.get(),
            1,
            "one full audit; mode={mode}: {error}"
        );
        assert_eq!(language.get(), mode >= 4);
        assert_eq!(
            shared.configured.get(),
            1,
            "configure_schema succeeded before validation"
        );
        assert_eq!(shared.store.borrow().metamodel().schema(), &had_schema);
        assert_eq!(
            storage::export(&*shared.store.borrow())?,
            before,
            "mode={mode}: {error}"
        );
        assert_eq!(handle_id(&shared.store.borrow(), &handle)?, "holder");
        assert!(!shared.is_transaction_active());
        match mode {
            0 | 1 => assert!(
                error.contains("Multiple feature requires a list"),
                "{error}"
            ),
            2 => assert!(
                error.contains("Reference type mismatch Holder.members"),
                "{error}"
            ),
            3 => assert!(error.contains("Missing reciprocal opposite"), "{error}"),
            4 => assert_eq!(error, "late language denial"),
            5 => assert_eq!(error, "late control denial"),
            _ => {}
        }
    }
    Ok(())
}
#[test]
fn structural_adoption_requires_an_idle_store_before_binding_or_provider() -> Result<(), String> {
    use intersect_model::storage::ModelStore;
    let schema = schema()?;
    let mut shared = Shared::new(raw(&schema)?);
    let checkpoint = shared.begin()?;
    let selector = Values {
        calls: Cell::new(0),
        choice: Choice::Backing,
    };
    let result = ResourceSet::from_store_with_structural_values(
        schema.clone(),
        ResourceContext::new(&schema, BTreeMap::new())?,
        shared.clone(),
        None,
        &selector,
        &mut QueryBudget::new(100_000),
        &InvocationContext::default(),
        |_, _| Ok(()),
    );
    assert!(result.is_err());
    assert_eq!(shared.configured.get(), 0);
    assert_eq!(selector.calls.get(), 0);
    assert!(shared.is_transaction_active());
    shared.rollback(&checkpoint)?;
    Ok(())
}
