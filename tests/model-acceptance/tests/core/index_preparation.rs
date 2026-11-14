use intersect_model::core::{
    model::{Element, Model},
    storage::{ModelRead, ModelView, OwnedModelView, Query, index_keys},
};
use intersect_model::definitions::conversion::{InvocationContext, InvocationObserver, ProviderId};
use serde_json::json;
use std::{cell::Cell, rc::Rc};
fn fixture() -> Model {
    let mut model = Model::default();
    for id in ["z", "a"] {
        model.elements.push(Element {
            implicit: false,
            initialization: None,
            assignment_state: None,
            id: id.into(),
            class: "C".into(),
            type_identity: "test".into(),
            resource: "r".into(),
            start: 0,
            end: 0,
            values: [
                ("class".into(), json!("feature")),
                ("empty".into(), json!([])),
                ("null".into(), json!(null)),
                ("many".into(), json!([null, "x", "x", "y"])),
            ]
            .into(),
        });
    }
    model
}
struct Probe {
    work: Cell<usize>,
    stop: usize,
}
impl InvocationObserver for Probe {
    fn before_invoke(&self, _: &ProviderId) -> Result<(), String> {
        panic!("indexing invokes no providers")
    }
    fn work(&self, count: usize) -> Result<(), String> {
        assert_eq!(count, 1);
        if self.work.get() == self.stop {
            return Err("interrupted".into());
        }
        self.work.set(self.work.get() + 1);
        Ok(())
    }
}
#[test]
fn index_preparation_yields_bounded_transitions_and_preserves_keys_and_row_order() {
    let model = fixture();
    assert_eq!(
        index_keys(&model.elements[0]),
        vec![
            ("class".into(), "C".into()),
            ("resource".into(), "r".into()),
            ("value:class".into(), "\"feature\"".into()),
            ("value:many".into(), "\"x\"".into()),
            ("value:many".into(), "\"y\"".into())
        ]
    );
    let probe = Rc::new(Probe {
        work: Cell::new(0),
        stop: usize::MAX,
    });
    let observation = InvocationContext::new(probe.clone());
    let mut pending = OwnedModelView::begin_observed(model.clone(), &observation).unwrap();
    let mut transitions = 0;
    loop {
        let before = probe.work.get();
        let done = pending.advance(&observation).unwrap();
        // These literal string atoms emit opening/body/closing chunks. The
        // cursor charges its transition plus those three serializer chunks.
        assert!(probe.work.get() - before <= 4);
        transitions += 1;
        if done {
            break;
        }
        assert!(transitions < 100);
    }
    assert!(transitions > 20);
    let view = pending.finish(&observation).unwrap();
    let borrowed = ModelView::new(&model).unwrap();
    for query in [
        Query::Class("C"),
        Query::Resource("r"),
        Query::Value("many", &json!("x")),
        Query::Value("class", &json!("feature")),
    ] {
        assert_eq!(view.query(query).unwrap(), vec!["z", "a"]);
        assert_eq!(borrowed.query(query).unwrap(), vec!["z", "a"]);
    }
    assert!(
        view.query(Query::Value("empty", &json!(null)))
            .unwrap()
            .is_empty()
    );
    assert_eq!(view.row_for_id("a"), Some(1));
    assert!(
        OwnedModelView::begin_observed(Model::default(), &InvocationContext::default())
            .unwrap()
            .finish(&InvocationContext::default())
            .is_err()
    );
    let mut empty =
        OwnedModelView::begin_observed(Model::default(), &InvocationContext::default()).unwrap();
    assert!(empty.advance(&InvocationContext::default()).unwrap());
    assert!(
        empty
            .finish(&InvocationContext::default())
            .unwrap()
            .model()
            .elements
            .is_empty()
    );
}
#[test]
fn index_preparation_cancellation_and_invalid_ids_never_publish_partial_views() {
    let model = fixture();
    let measure = Rc::new(Probe {
        work: Cell::new(0),
        stop: usize::MAX,
    });
    OwnedModelView::new_observed(model.clone(), &InvocationContext::new(measure.clone())).unwrap();
    for stop in 0..measure.work.get() {
        let probe = Rc::new(Probe {
            work: Cell::new(0),
            stop,
        });
        let observation = InvocationContext::new(probe);
        let pending = OwnedModelView::begin_observed(model.clone(), &observation);
        if stop == 0 {
            assert!(pending.is_err());
            continue;
        }
        let mut pending = pending.unwrap();
        loop {
            match pending.advance(&observation) {
                Err(_) => break,
                Ok(false) => {}
                Ok(true) => panic!("missed interruption"),
            }
        }
        assert!(pending.advance(&InvocationContext::default()).is_err());
        assert!(pending.finish(&InvocationContext::default()).is_err());
    }
    for id in ["", "z"] {
        let mut invalid = model.clone();
        invalid.elements[1].id = id.into();
        assert!(OwnedModelView::new(invalid).is_err());
    }
    assert!(OwnedModelView::new(model).is_ok());
}

struct StorageProbe {
    used: Cell<usize>,
    limit: usize,
}
impl InvocationObserver for StorageProbe {
    fn before_invoke(&self, _: &ProviderId) -> Result<(), String> {
        panic!("indexing invokes no providers")
    }
    fn retain_storage(&self, bytes: usize) -> Result<(), String> {
        let next = self
            .used
            .get()
            .checked_add(bytes)
            .filter(|n| *n <= self.limit)
            .ok_or("index storage denied")?;
        self.used.set(next);
        Ok(())
    }
}

#[test]
fn index_storage_is_reserved_before_retention_with_literal_boundaries() {
    // Each row: identity 65; metadata 134+137; cursor fields 274;
    // encoded keys 148+3*141 (including duplicate x). First-row postings
    // 5*160; second-row postings 5*32. Cumulative logical policy: 3322.
    let model = fixture();
    for limit in [0, 64, 65, 198, 199, 3321, 3322, 3323] {
        let probe = Rc::new(StorageProbe {
            used: Cell::new(0),
            limit,
        });
        let observation = InvocationContext::new(probe.clone());
        let mut pending = OwnedModelView::begin_observed(model.clone(), &observation).unwrap();
        loop {
            match pending.advance(&observation) {
                Ok(false) => {}
                Ok(true) => {
                    assert!(limit >= 3322);
                    assert_eq!(probe.used.get(), 3322);
                    let view = pending.finish(&observation).unwrap();
                    assert_eq!(
                        view.query(Query::Value("many", &json!("x"))).unwrap(),
                        vec!["z", "a"]
                    );
                    break;
                }
                Err(error) => {
                    assert!(limit < 3322);
                    assert_eq!(error, "index storage denied");
                    assert!(pending.advance(&InvocationContext::default()).is_err());
                    assert!(pending.finish(&InvocationContext::default()).is_err());
                    break;
                }
            }
        }
        assert!(probe.used.get() <= limit);
    }
    assert_eq!(
        model.elements[0].values["many"],
        json!([null, "x", "x", "y"])
    );
    assert!(OwnedModelView::new(model).is_ok());
}

#[test]
fn index_storage_counts_escaped_utf8_keys_and_rejects_ids_before_reservation() {
    let mut model = fixture();
    model.elements.truncate(1);
    model.elements[0].values = [("escaped".into(), json!("\0\né"))].into();
    // 65 identity + 271 metadata + 71 cursor + (134+7+12) key
    // + three new posting buckets at 160 each = 1040.
    for limit in [1039, 1040] {
        let probe = Rc::new(StorageProbe {
            used: Cell::new(0),
            limit,
        });
        let result =
            OwnedModelView::new_observed(model.clone(), &InvocationContext::new(probe.clone()));
        assert_eq!(result.is_ok(), limit == 1040);
        if let Ok(view) = result {
            assert_eq!(probe.used.get(), 1040);
            assert_eq!(
                view.query(Query::Value("escaped", &json!("\0\né")))
                    .unwrap(),
                vec!["z"]
            );
        }
    }
    model.elements[0].id.clear();
    let probe = Rc::new(StorageProbe {
        used: Cell::new(0),
        limit: 0,
    });
    let result = OwnedModelView::new_observed(model, &InvocationContext::new(probe.clone()));
    assert_eq!(result.err().unwrap(), "Duplicate or empty model identity");
    assert_eq!(probe.used.get(), 0);
}
