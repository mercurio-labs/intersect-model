//! Focused literal checks before extensive replay or D owner adoption.
use super::*;
use crate::definitions::{
    bundle::DefinitionBundle,
    conversion::{InvocationContext, InvocationObserver, ProviderId},
};
use serde_json::{Value, json};
use std::{cell::RefCell, rc::Rc};
struct Lease(Rc<RefCell<Vec<(char, usize)>>>);
impl InvocationObserver for Lease {
    fn checkpoint(&self) -> Result<(), String> {
        self.0.borrow_mut().push(('c', 0));
        Ok(())
    }
    fn work(&self, n: usize) -> Result<(), String> {
        self.0.borrow_mut().push(('w', n));
        Ok(())
    }
    fn retain_storage(&self, n: usize) -> Result<(), String> {
        self.0.borrow_mut().push(('s', n));
        Ok(())
    }
    fn before_invoke(&self, _: &ProviderId) -> Result<(), String> {
        Err("Unexpected decoder provider".into())
    }
}
#[test]
fn original_six_literals_match_same_engine_with_moved_fresh_leases() {
    let rows: Vec<Value> =
        serde_json::from_str(include_str!("generics_decoder_initial_rows.json")).unwrap();
    for row in rows {
        let bundle: DefinitionBundle = serde_json::from_value(row["source"].clone()).unwrap();
        let resources = ResourceSet::new_diagnostic(&bundle).unwrap();
        let id: DeclarationId = serde_json::from_value(row["selection"].clone()).unwrap();
        for quantum in [0, 1, 3, 256] {
            let events = Rc::new(RefCell::new(vec![]));
            let mut preparation = GenericTypePreparation::begin();
            let output = 'run: loop {
                preparation = std::hint::black_box(preparation);
                let observer = Rc::new(Lease(events.clone()));
                let weak = Rc::downgrade(&observer);
                let result = {
                    let context = InvocationContext::new(observer.clone());
                    let observation = ParserObservation::new(&context);
                    if quantum == 0 {
                        Some(observation.finish(parse_type_observed(&resources, &id, &observation)))
                    } else {
                        let mut result = None;
                        for _ in 0..quantum {
                            match observation.finish(preparation.advance(
                                &resources,
                                &id,
                                &observation,
                            )) {
                                Ok(true) => {
                                    assert!(
                                        preparation.advance(&resources, &id, &observation).unwrap()
                                    );
                                    result = Some(observation.finish(preparation.finish(&id)));
                                    preparation = GenericTypePreparation::begin();
                                    break;
                                }
                                Ok(false) => {}
                                Err(e) => {
                                    let before = events.borrow().len();
                                    assert!(
                                        preparation.advance(&resources, &id, &observation).is_err()
                                    );
                                    assert_eq!(events.borrow().len(), before);
                                    assert!(preparation.finish(&id).is_err());
                                    preparation = GenericTypePreparation::begin();
                                    result = Some(Err(e));
                                    break;
                                }
                            }
                        }
                        result
                    }
                };
                drop(observer);
                assert!(weak.upgrade().is_none());
                if let Some(result) = result {
                    break 'run match result {
                        Ok(v) => json!({"ok":v}),
                        Err(e) => json!({"error":format!("{e:?}")}),
                    };
                }
            };
            assert_eq!(output, row["output"], "{} q{quantum}", row["id"]);
            assert_eq!(
                json!(*events.borrow()),
                row["events"],
                "{} q{quantum}",
                row["id"]
            );
        }
    }
}

#[test]
fn decoder_incomplete_and_failed_lifecycle_errors_keep_exact_selected_source() {
    let rows: Vec<Value> =
        serde_json::from_str(include_str!("generics_decoder_initial_rows.json")).unwrap();
    let bundle: DefinitionBundle = serde_json::from_value(rows[0]["source"].clone()).unwrap();
    let resources = ResourceSet::new_diagnostic(&bundle).unwrap();
    let selection: DeclarationId = serde_json::from_value(rows[0]["selection"].clone()).unwrap();
    let failure = GenericTypePreparation::begin()
        .finish(&selection)
        .unwrap_err();
    assert_eq!(failure.span.source, selection.source);
    assert_eq!((failure.span.start, failure.span.end), (0, 0));
    let invalid = DeclarationId {
        source: selection.source.clone(),
        path: vec![],
    };
    let context = InvocationContext::default();
    let observation = ParserObservation::new(&context);
    let mut preparation = GenericTypePreparation::begin();
    let authored = preparation
        .advance(&resources, &invalid, &observation)
        .unwrap_err();
    assert_eq!(authored.span.source, invalid.source);
    assert!(authored.span.end > authored.span.start);
    let poisoned = preparation
        .advance(&resources, &invalid, &observation)
        .unwrap_err();
    assert_eq!(poisoned.span.source, invalid.source);
    assert_eq!((poisoned.span.start, poisoned.span.end), (0, 0));
    assert_eq!(
        preparation.finish(&invalid).unwrap_err().span.source,
        invalid.source
    );
}
