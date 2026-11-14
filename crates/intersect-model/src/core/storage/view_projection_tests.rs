use super::*;
use intersect_model::definitions::conversion::{InvocationContext, InvocationObserver, ProviderId};
use serde_json::json;
use std::{cell::Cell as Counter, rc::Rc};

struct Probe {
    bytes: Counter<usize>,
    limit: usize,
    work: Counter<usize>,
    stop: usize,
}
impl InvocationObserver for Probe {
    fn before_invoke(&self, _: &ProviderId) -> Result<(), String> {
        panic!("projection invokes no providers");
    }
    fn work(&self, units: usize) -> Result<(), String> {
        assert_eq!(units, 1);
        if self.work.get() == self.stop {
            return Err("projection cancelled".into());
        }
        self.work.set(self.work.get() + units);
        Ok(())
    }
    fn retain_storage(&self, bytes: usize) -> Result<(), String> {
        let next = self
            .bytes
            .get()
            .checked_add(bytes)
            .filter(|next| *next <= self.limit)
            .ok_or("projection storage denied")?;
        self.bytes.set(next);
        Ok(())
    }
}
fn probe(limit: usize, stop: usize) -> Rc<Probe> {
    Rc::new(Probe {
        bytes: Counter::new(0),
        limit,
        work: Counter::new(0),
        stop,
    })
}
fn check(value: ValueView<'_>, expected: Value, bytes: usize, work: usize) {
    assert_eq!(value.to_json(), expected);
    assert!(value.equals_json(&expected));
    assert!(!value.equals_json(&json!({"unexpected": "value"})));
    assert_eq!(serde_json::to_value(value).unwrap(), expected);
    for limit in [0, bytes - 1, bytes, bytes + 1] {
        let budget = probe(limit, usize::MAX);
        let result = value.to_json_observed(&InvocationContext::new(budget.clone()));
        assert_eq!(budget.work.get(), work);
        if limit < bytes {
            assert_eq!(result.unwrap_err(), "projection storage denied");
            assert_eq!(budget.bytes.get(), 0);
        } else {
            assert_eq!(result.unwrap(), expected);
            assert_eq!(budget.bytes.get(), bytes);
        }
    }
    for stop in 0..work {
        let budget = probe(usize::MAX, stop);
        assert_eq!(
            value
                .to_json_observed(&InvocationContext::new(budget.clone()))
                .unwrap_err(),
            "projection cancelled"
        );
        assert_eq!(budget.work.get(), stop);
        assert_eq!(budget.bytes.get(), 0);
        assert_eq!(
            value
                .to_json_observed(&InvocationContext::default())
                .unwrap(),
            expected
        );
    }
    let budget = probe(2 * bytes, usize::MAX);
    let observation = InvocationContext::new(budget.clone());
    assert_eq!(value.to_json_observed(&observation).unwrap(), expected);
    assert_eq!(value.to_json_observed(&observation).unwrap(), expected);
    assert_eq!(
        value.to_json_observed(&observation).unwrap_err(),
        "projection storage denied"
    );
    assert_eq!(budget.bytes.get(), 2 * bytes);
    assert_eq!(budget.work.get(), 3 * work);
}
#[test]
fn json_and_frozen_projections_share_the_literal_policy() -> StorageResult<()> {
    use crate::core::{
        frozen::FrozenStore,
        storage::{self, ModelRead, ModelStore},
    };
    use crate::definitions::dynamic::{ScalarType, SchemaBuilder};

    let mut builder = SchemaBuilder::new("projection", "urn:frozen:projection");
    let row = builder.class("Row")?;
    for field in ["null", "flag", "number", "text", "opaque"] {
        row.attribute(field, ScalarType::String).optional()?;
    }
    row.attribute("list", ScalarType::String).many().build()?;
    for field in ["resolved", "unavailable", "pending", "nested"] {
        row.reference(field, &row).optional()?;
    }
    for field in ["refs", "mixed"] {
        row.reference(field, &row).many().unique(false).build()?;
    }
    let schema = builder.build()?;
    let schema = schema.as_schema();
    let mut store = FrozenStore::for_schema(schema).map_err(|error| error.to_string())?;
    let opaque = json!({"s":"é", "n":true, "items":[null,7]});
    check(ValueView::json(&opaque), opaque.clone(), 1241, 9);
    check(ValueView::null(), Value::Null, 132, 1);
    for (raw, bytes, work) in [
        (json!(["é", "foreign", null]), 531, 4),
        (json!(["é", "?left", "foreign", null]), 666, 5),
    ] {
        check(ValueView::json(&raw), raw.clone(), bytes, work);
    }
    let cases = [
        ("null", Value::Null, 132, 1),
        ("flag", json!(true), 160, 1),
        ("number", json!(7), 160, 1),
        ("text", json!("é"), 132, 1),
        ("resolved", json!("é"), 132, 1),
        ("unavailable", json!("foreign"), 137, 1),
        ("pending", json!("?é"), 133, 1),
        ("list", json!(["é", null, true, 7]), 714, 5),
        ("refs", json!(["é", "foreign"]), 399, 3),
        ("mixed", json!(["é", "?left", "foreign"]), 534, 4),
        ("opaque", opaque.clone(), 1241, 9),
    ];
    storage::atomic(&mut store, |store| {
        for id in ["row", "é"] {
            store.create_element(schema, id, "Row", "r", 0, 0)?;
        }
        for (field, value, _, _) in &cases {
            store.set_value("row", field, Some(value.clone()))?;
        }
        store.set_unresolved_reference("row", "nested", "?nested", false)
    })?;
    let before = storage::export(&store)?;
    assert!(
        storage::atomic(&mut store, |s| {
            s.set_value("row", "refs", Some(json!(["é", null])))
        })
        .is_err()
    );
    assert_eq!(storage::export(&store)?, before);
    assert!(store.inspect("row", &mut |view| {
        for (field, expected, bytes, work) in &cases {
            check(
                view.value(field).ok_or("Missing fixture field")?,
                expected.clone(),
                *bytes,
                *work,
            );
        }
        let nested = view
            .value("nested")
            .ok_or("Missing pending fixture field")?;
        assert!(nested.equals_json(&json!("??nested")));
        assert!(!nested.equals_json(&json!("?nested")));
        assert!(!nested.equals_json(&Value::Null));
        Ok(())
    })?);
    let array = json!(["é", null, true, 7]);
    let collection = ValueView::json(&array).collection().unwrap();
    assert_eq!(serde_json::to_value(collection).unwrap(), array);
    Ok(())
}
#[test]
fn observed_projection_rejects_shape_before_copy_and_unobserved_export_keeps_compatibility() {
    let wide = Value::Array(vec![Value::Null; 65536]);
    let deep = (0..65).fold(Value::Null, |v, _| json!([v]));
    for value in [&wide, &deep] {
        let budget = probe(usize::MAX, usize::MAX);
        assert!(
            ValueView::json(value)
                .to_json_observed(&InvocationContext::new(budget.clone()))
                .is_err()
        );
        assert_eq!(budget.bytes.get(), 0);
        assert_eq!(
            ValueView::json(value)
                .to_json_observed(&InvocationContext::default())
                .unwrap(),
            *value
        );
    }
}
