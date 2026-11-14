//! Concrete scope/protocol controls; delegates use the existing canonical audit.
use super::*;
use storage::{ImmutableStructuralAudit, Row, with_immutable_structural_audit};
#[derive(Clone, Copy)]
enum Mode {
    Literal,
    Accept,
    Missing,
    DeclineAfterCall,
    Twice,
    ErrorAfterCall,
}
struct Hook {
    mode: Mode,
    entered: Cell<usize>,
    selected: Cell<usize>,
}
impl Hook {
    fn new(mode: Mode) -> Self {
        Self {
            mode,
            entered: Cell::new(0),
            selected: Cell::new(0),
        }
    }
}
impl StructuralValueSelector for Hook {
    fn select(
        &self,
        store: &dyn ModelRead,
        schema: &Schema,
        request: StructuralValueRequest<'_>,
        budget: &mut QueryBudget,
    ) -> StorageResult<Option<Value>> {
        self.selected.set(self.selected.get() + 1);
        Values(ResultKind::Backing).select(store, schema, request, budget)
    }
    fn with_immutable_audit(
        &self,
        audit: &ImmutableStructuralAudit<'_>,
        _: &InvocationContext<'_>,
        run: &mut dyn FnMut(&dyn StructuralValueSelector),
    ) -> StorageResult<bool> {
        assert!(audit.matches(audit.store(), audit.schema()));
        self.entered.set(self.entered.get() + 1);
        match self.mode {
            Mode::Literal => Ok(false),
            Mode::Missing => Ok(true),
            Mode::Accept => {
                run(self);
                Ok(true)
            }
            Mode::DeclineAfterCall => {
                run(self);
                Ok(false)
            }
            Mode::Twice => {
                run(self);
                run(self);
                Ok(true)
            }
            Mode::ErrorAfterCall => {
                run(self);
                Err("late hook veto".into())
            }
        }
    }
}
#[test]
fn callback_protocol_and_first_failure_are_enforced() -> StorageResult<()> {
    let schema = schema();
    let store = FrozenStore::from_materialized(&schema, &model(&schema, None))?;
    let token = ImmutableStructuralAudit::native(&store, &schema);
    for (mode, calls, message) in [
        (Mode::Literal, 1, None),
        (Mode::Accept, 1, None),
        (Mode::Missing, 0, Some("did not invoke")),
        (Mode::DeclineAfterCall, 1, Some("Declined")),
        (Mode::Twice, 1, Some("more than once")),
        (Mode::ErrorAfterCall, 1, Some("late hook veto")),
    ] {
        let count = Cell::new(0);
        let hook = Hook::new(mode);
        let result = with_immutable_structural_audit(
            &hook,
            &token,
            &InvocationContext::default(),
            &mut |_| {
                count.set(count.get() + 1);
                Ok(())
            },
        );
        assert_eq!(count.get(), calls);
        match message {
            Some(m) => assert!(result.unwrap_err().contains(m)),
            None => result?,
        }
    }
    for mode in [Mode::Twice, Mode::DeclineAfterCall, Mode::ErrorAfterCall] {
        let count = Cell::new(0);
        let result = with_immutable_structural_audit(
            &Hook::new(mode),
            &token,
            &InvocationContext::default(),
            &mut |_| {
                count.set(count.get() + 1);
                Err("canonical first error".into())
            },
        );
        assert_eq!(result.unwrap_err(), "canonical first error");
        assert_eq!(count.get(), 1);
    }
    Ok(())
}
struct Forward<'a>(&'a FrozenStore);
impl ModelRead for Forward<'_> {
    fn resource_records(
        &self,
    ) -> StorageResult<BTreeMap<String, intersect_model::core::resources::ResourceRecord>> {
        self.0.resource_records()
    }
    fn resource_record(
        &self,
        uri: &str,
    ) -> StorageResult<Option<intersect_model::core::resources::ResourceRecord>> {
        self.0.resource_record(uri)
    }
    fn revision(&self) -> StorageResult<String> {
        self.0.revision()
    }
    fn get(&self, id: &str) -> StorageResult<Option<intersect_model::core::model::Element>> {
        self.0.get(id)
    }
    fn class(&self, id: &str) -> StorageResult<Option<String>> {
        self.0.class(id)
    }
    fn page(&self, after: Option<u64>, limit: usize) -> StorageResult<Vec<Row>> {
        self.0.page(after, limit)
    }
    fn query(&self, q: storage::Query<'_>) -> StorageResult<Vec<String>> {
        self.0.query(q)
    }
    fn matches(&self, id: &str, q: storage::Query<'_>) -> StorageResult<bool> {
        self.0.matches(id, q)
    }
}
#[test]
fn token_is_exact_and_forwarding_cannot_authorize_an_outer_reader() -> StorageResult<()> {
    let schema = schema();
    let first = FrozenStore::from_materialized(&schema, &model(&schema, None))?;
    let second = FrozenStore::from_materialized(&schema, &model(&schema, None))?;
    let token = ImmutableStructuralAudit::native(&first, &schema);
    assert!(token.matches(token.store(), &schema));
    assert!(!token.matches(&second, &schema));
    assert!(!token.matches(token.store(), &schema.clone()));
    assert!(!token.matches(&Forward(&first), &schema));
    let hook = Hook::new(Mode::Accept);
    let result = storage::validate_with_structural_values(
        &Forward(&first),
        &schema,
        None,
        &hook,
        &mut QueryBudget::new(100_000),
        &InvocationContext::default(),
    );
    result?;
    assert_eq!(hook.entered.get(), 0);
    assert!(hook.selected.get() > 0);
    Ok(())
}
#[test]
fn canonical_audits_rebind_and_see_between_operation_mutation() -> StorageResult<()> {
    let schema = schema();
    let mut store = FrozenStore::from_materialized(&schema, &model(&schema, None))?;
    let hook = Hook::new(Mode::Accept);
    storage::validate_native_with_structural_values(
        &store,
        &schema,
        None,
        &hook,
        &mut QueryBudget::new(100_000),
        &InvocationContext::default(),
    )?;
    storage::atomic(&mut store, |same| {
        same.set_value("holder", "backing", Some(json!("other")))
    })?;
    let error = storage::validate_native_with_structural_values(
        &store,
        &schema,
        None,
        &hook,
        &mut QueryBudget::new(100_000),
        &InvocationContext::default(),
    )
    .unwrap_err();
    assert!(!error.is_empty());
    assert_eq!(hook.entered.get(), 2);
    storage::atomic(&mut store, |same| {
        same.set_value("holder", "backing", Some(json!("target")))
    })?;
    storage::validate_native_with_structural_values(
        &store,
        &schema,
        None,
        &hook,
        &mut QueryBudget::new(100_000),
        &InvocationContext::default(),
    )?;
    assert_eq!(hook.entered.get(), 3);
    let context = ResourceContext::new(&schema, BTreeMap::new())?;
    let language_called = Cell::new(false);
    let set = ResourceSet::from_native_with_structural_values(
        schema,
        context,
        store,
        None,
        &hook,
        &mut QueryBudget::new(100_000),
        &InvocationContext::default(),
        |same, caller| {
            assert!(same.value("holder", "member")?.is_none());
            assert_eq!(hook.entered.get(), 4);
            caller.charge()?;
            language_called.set(true);
            Ok(())
        },
    )?;
    assert!(language_called.get());
    assert!(set.reader().value("holder", "member")?.is_none());
    Ok(())
}
