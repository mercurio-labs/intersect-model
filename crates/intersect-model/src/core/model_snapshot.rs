//! Handwritten staged wire decoding; Model/Element Serde derives remain authority.
use super::{Element, Model};
use intersect_model::definitions::conversion::InvocationContext;
use serde::{
    Deserialize,
    de::value::{BorrowedStrDeserializer, MapDeserializer},
};
use serde_json::Value;

#[derive(Debug)]
pub enum SnapshotError {
    NonCanonical,
    Decode(String),
    Observation(String),
    Allocation,
    State,
}
impl std::fmt::Display for SnapshotError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NonCanonical => f.write_str("Model snapshot requires exact explicit fields"),
            Self::Decode(error) | Self::Observation(error) => f.write_str(error),
            Self::Allocation => f.write_str("Model snapshot row allocation failed"),
            Self::State => f.write_str("Model snapshot preparation failed or is unfinished"),
        }
    }
}
impl std::error::Error for SnapshotError {}

/// Decode at most one object per advance from the same immutable snapshot.
/// Callers must retain that snapshot unchanged throughout preparation, as for a
/// ModelRead cursor. No model is exposed until finish. Per-object typed decoding
/// and canonical comparison remain synchronous, including nested stored values.
#[derive(Default)]
pub struct SnapshotPreparation {
    model: Model,
    cursor: usize,
    failed: bool,
    done: bool,
}
impl SnapshotPreparation {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn advance(
        &mut self,
        snapshot: &Value,
        observation: &InvocationContext<'_>,
    ) -> Result<bool, SnapshotError> {
        if self.failed || self.done {
            return Err(SnapshotError::State);
        }
        // Failure is terminal even if it occurs before a row is retained.
        self.failed = true;
        observation.work(1).map_err(SnapshotError::Observation)?;
        let object = snapshot.as_object().ok_or(SnapshotError::NonCanonical)?;
        let rows = match object.get("elements") {
            Some(Value::Array(rows)) => rows,
            Some(value) => {
                // Delegate malformed sequence diagnostics to the actual row DTO.
                return match Vec::<Element>::deserialize(value) {
                    Err(error) => Err(SnapshotError::Decode(error.to_string())),
                    Ok(_) => Err(SnapshotError::NonCanonical),
                };
            }
            None => return Err(SnapshotError::Decode("missing field `elements`".into())),
        };
        if self.cursor == 0 {
            // Reject a noncanonical root before walking unknown fields or
            // allocating any rows. The three-field header is decoded by Model's
            // own derive; only its already borrowed row sequence is replaced.
            if object.len() != 3 {
                return Err(SnapshotError::NonCanonical);
            }
            match object.get("schema_revision") {
                Some(Value::String(revision)) => {
                    intersect_model::definitions::retained_storage::reserve_serialized(
                        revision,
                        observation,
                    )
                    .map_err(SnapshotError::Observation)?;
                }
                Some(value) => {
                    // Diagnose the scalar header before visiting resource metadata.
                    // Valid strings remain borrowed until their storage is reserved.
                    return match String::deserialize(value) {
                        Err(error) => Err(SnapshotError::Decode(error.to_string())),
                        Ok(_) => Err(SnapshotError::NonCanonical),
                    };
                }
                None => {
                    return Err(SnapshotError::Decode(
                        "missing field `schema_revision`".into(),
                    ));
                }
            }
            if let Some(resources) = object.get("resources") {
                intersect_model::definitions::retained_storage::reserve_serialized(
                    resources,
                    observation,
                )
                .map_err(SnapshotError::Observation)?;
            }
            let empty = Value::Array(Vec::new());
            let fields = object.iter().map(|(key, value)| {
                (
                    BorrowedStrDeserializer::<serde_json::Error>::new(key),
                    if key == "elements" { &empty } else { value },
                )
            });
            let header = Model::deserialize(MapDeserializer::new(fields))
                .map_err(|e| SnapshotError::Decode(e.to_string()))?;
            self.model.schema_revision = header.schema_revision;
            self.model.resources = header.resources;
        }
        if let Some(raw) = rows.get(self.cursor) {
            intersect_model::definitions::retained_storage::reserve_serialized(raw, observation)
                .map_err(SnapshotError::Observation)?;
            let row =
                Element::deserialize(raw).map_err(|e| SnapshotError::Decode(e.to_string()))?;
            if !intersect_model::definitions::json_exact::matches_with_work(&row, raw, &|units| {
                observation.work(units)
            })
            .map_err(SnapshotError::Observation)?
            {
                return Err(SnapshotError::NonCanonical);
            }
            self.model
                .elements
                .try_reserve(1)
                .map_err(|_| SnapshotError::Allocation)?;
            self.model.elements.push(row);
            self.cursor += 1;
            self.failed = false;
            return Ok(false);
        }
        self.done = true;
        self.failed = false;
        Ok(true)
    }
    pub fn finish(self) -> Result<Model, SnapshotError> {
        if self.done && !self.failed {
            Ok(self.model)
        } else {
            Err(SnapshotError::State)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use intersect_model::definitions::conversion::{InvocationObserver, ProviderId};
    use std::{cell::Cell, rc::Rc};
    fn fixture() -> Value {
        serde_json::json!({"schema_revision":"pinned", "resources":{}, "elements":(0..3).map(|n| {
            serde_json::json!({"id":format!("o{n}"),"class":"C","type_identity":"T",
                "resource":"r","start":0,"end":2,"values":{"nested":["é",null,1,1]}})
        }).collect::<Vec<_>>()})
    }
    #[test]
    fn rows_yield_individually_and_finish_preserves_the_exact_typed_wire() {
        let wire = fixture();
        let mut preparation = SnapshotPreparation::new();
        for count in 1..=3 {
            assert!(
                !preparation
                    .advance(&wire, &InvocationContext::default())
                    .unwrap()
            );
            assert_eq!(preparation.model.elements.len(), count);
        }
        assert!(
            preparation
                .advance(&wire, &InvocationContext::default())
                .unwrap()
        );
        assert!(
            preparation
                .advance(&wire, &InvocationContext::default())
                .is_err()
        );
        let model = preparation.finish().unwrap();
        assert_eq!(serde_json::to_value(&model).unwrap(), wire);
        assert_eq!(model, Model::deserialize(&wire).unwrap());
        assert!(SnapshotPreparation::new().finish().is_err());
        let empty = serde_json::json!({"schema_revision":"empty","resources":{},"elements":[]});
        let mut preparation = SnapshotPreparation::new();
        assert!(
            preparation
                .advance(&empty, &InvocationContext::default())
                .unwrap()
        );
        assert_eq!(
            serde_json::to_value(preparation.finish().unwrap()).unwrap(),
            empty
        );
    }
    #[test]
    fn malformed_and_noncanonical_snapshots_never_publish_partial_models() {
        for mutation in 0..6 {
            let mut wire = fixture();
            match mutation {
                0 => wire["extra"] = serde_json::json!([1, 2]),
                1 => wire["elements"][1]["extra"] = serde_json::json!(true),
                2 => wire["elements"][1]["initialization"] = Value::Null,
                3 => wire["schema_revision"] = serde_json::json!(1),
                4 => wire["elements"] = Value::Null,
                _ => {
                    wire["elements"][1].as_object_mut().unwrap().remove("id");
                }
            }
            let mut preparation = SnapshotPreparation::new();
            let error = loop {
                match preparation.advance(&wire, &InvocationContext::default()) {
                    Ok(false) => {}
                    Ok(true) => panic!("accepted mutation {mutation}"),
                    Err(error) => break error,
                }
            };
            if mutation < 3 {
                assert!(matches!(error, SnapshotError::NonCanonical));
            } else {
                assert!(matches!(error, SnapshotError::Decode(_)));
            }
            assert!(
                preparation
                    .advance(&wire, &InvocationContext::default())
                    .is_err()
            );
            assert!(preparation.finish().is_err());
        }
    }
    struct Budget {
        work: Cell<usize>,
        limit: usize,
    }
    #[test]
    fn invalid_headers_fail_before_row_decoding_or_unknown_field_traversal() {
        for mutation in 0..3 {
            let mut wire = fixture();
            // A valid but expensive row must never be decoded for these headers.
            wire["elements"][0]["values"]["large"] = Value::String("x".repeat(262_144));
            // Resource metadata must not be traversed before the scalar header
            // is admitted either, even when its diagnostics are expensive.
            let mut resource = crate::core::resources::ResourceRecord::new("r").unwrap();
            resource.roots = std::sync::Arc::new(vec!["o0".into(), "o1".into(), "o2".into()]);
            resource.diagnostics.push("x".repeat(262_144));
            wire["resources"] = serde_json::json!({"r":resource});
            match mutation {
                0 => {
                    let root = wire.as_object_mut().unwrap();
                    for n in 0..1_024 {
                        root.insert(format!("unknown{n}"), serde_json::json!([1, 2, 3]));
                    }
                }
                1 => wire["schema_revision"] = serde_json::json!(42),
                _ => {
                    let root = wire.as_object_mut().unwrap();
                    root.remove("schema_revision");
                    root.insert("unknown".into(), Value::Null);
                }
            }
            let budget = Rc::new(Budget {
                work: Cell::new(0),
                limit: 1,
            });
            let observation = InvocationContext::new(budget.clone());
            let mut preparation = SnapshotPreparation::new();
            let error = preparation.advance(&wire, &observation).unwrap_err();
            match mutation {
                0 => assert!(matches!(error, SnapshotError::NonCanonical)),
                1 => assert_eq!(
                    error.to_string(),
                    "invalid type: integer `42`, expected a string"
                ),
                _ => assert_eq!(error.to_string(), "missing field `schema_revision`"),
            }
            assert_eq!(budget.work.get(), 1);
            assert_eq!(preparation.cursor, 0);
            assert!(preparation.model.elements.is_empty());
            assert!(matches!(
                preparation.advance(&wire, &observation),
                Err(SnapshotError::State)
            ));
            assert_eq!(budget.work.get(), 1);
            assert!(preparation.finish().is_err());
        }
    }
    impl InvocationObserver for Budget {
        fn before_invoke(&self, _: &ProviderId) -> Result<(), String> {
            panic!("no provider call")
        }
        fn work(&self, units: usize) -> Result<(), String> {
            if self.work.get() + units > self.limit {
                return Err("snapshot interrupted".into());
            }
            self.work.set(self.work.get() + units);
            Ok(())
        }
    }
    #[test]
    fn every_insufficient_work_allowance_is_terminal_and_exact_work_recovers() {
        let wire = fixture();
        let run = |budget: Rc<Budget>| {
            let mut preparation = SnapshotPreparation::new();
            let observation = InvocationContext::new(budget);
            loop {
                match preparation.advance(&wire, &observation) {
                    Ok(false) => {}
                    Ok(true) => return preparation.finish(),
                    Err(error) => {
                        assert!(preparation.advance(&wire, &observation).is_err());
                        assert!(preparation.finish().is_err());
                        return Err(error);
                    }
                }
            }
        };
        let budget = Rc::new(Budget {
            work: Cell::new(0),
            limit: usize::MAX,
        });
        let expected = run(budget.clone()).unwrap();
        let needed = budget.work.get();
        for limit in 0..needed {
            let budget = Rc::new(Budget {
                work: Cell::new(0),
                limit,
            });
            assert_eq!(
                run(budget.clone()).unwrap_err().to_string(),
                "snapshot interrupted"
            );
            assert!(budget.work.get() <= limit);
        }
        assert_eq!(
            run(Rc::new(Budget {
                work: Cell::new(0),
                limit: needed
            }))
            .unwrap(),
            expected
        );
        assert_eq!(wire, fixture());
    }
    #[test]
    fn storage_is_reserved_before_header_and_row_ownership() {
        struct Storage {
            retained: Cell<usize>,
            limit: usize,
        }
        impl InvocationObserver for Storage {
            fn before_invoke(&self, _: &ProviderId) -> Result<(), String> {
                panic!("no provider call")
            }
            fn retain_storage(&self, bytes: usize) -> Result<(), String> {
                let next = self.retained.get() + bytes;
                if next > self.limit {
                    return Err("storage interrupted".into());
                }
                self.retained.set(next);
                Ok(())
            }
        }
        // Header: 136 revision bytes + 130 empty-resource-map bytes. Each
        // literal fixture row: 21 nodes + 219 bytes, hence 2907 bytes;
        // total 266 + 3*2907 = 8987 logical bytes.
        for limit in [0, 135, 136, 265, 266, 3172, 3173, 8986, 8987, 8988] {
            let storage = Rc::new(Storage {
                retained: Cell::new(0),
                limit,
            });
            let observation = InvocationContext::new(storage.clone());
            let wire = fixture();
            let mut preparation = SnapshotPreparation::new();
            loop {
                match preparation.advance(&wire, &observation) {
                    Ok(false) => {}
                    Ok(true) => {
                        assert!(limit >= 8987);
                        assert_eq!(storage.retained.get(), 8987);
                        assert_eq!(
                            serde_json::to_value(preparation.finish().unwrap()).unwrap(),
                            wire
                        );
                        break;
                    }
                    Err(error) => {
                        assert!(limit < 8987);
                        assert_eq!(error.to_string(), "storage interrupted");
                        assert!(storage.retained.get() <= limit);
                        if limit < 266 {
                            assert!(preparation.model.schema_revision.is_empty());
                        }
                        if limit < 3173 {
                            assert!(preparation.model.elements.is_empty());
                        }
                        assert!(preparation.advance(&wire, &observation).is_err());
                        assert!(preparation.finish().is_err());
                        break;
                    }
                }
            }
            assert_eq!(wire, fixture());
        }
    }
}
