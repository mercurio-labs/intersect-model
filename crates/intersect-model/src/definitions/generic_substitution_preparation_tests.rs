//! Complete pre-change structural substitution observations, before retention.
use super::*;
use crate::definitions::{
    bundle::{DefinitionSource, SourceKind},
    conversion::{InvocationObserver, ProviderId},
};
use sha2::{Digest, Sha256};
use std::{cell::RefCell, rc::Rc};
type Events = Rc<RefCell<Vec<(char, usize)>>>;
struct Lease {
    events: Events,
    deny: Option<usize>,
}
impl Lease {
    fn record(&self, kind: char, n: usize) -> Result<(), String> {
        let mut events = self.events.borrow_mut();
        events.push((kind, n));
        if self.deny == Some(events.len()) {
            Err("substitution cursor denied".into())
        } else {
            Ok(())
        }
    }
}
impl InvocationObserver for Lease {
    fn before_invoke(&self, _: &ProviderId) -> Result<(), String> {
        panic!("structural substitution cannot invoke providers")
    }
    fn work(&self, n: usize) -> Result<(), String> {
        self.record('w', n)
    }
    fn retain_storage(&self, n: usize) -> Result<(), String> {
        self.record('s', n)
    }
}
fn fixture(case: usize) -> (Substitution, Type) {
    let source = DefinitionSource::new(
        if case == 24 {
            "memory:substitution-\u{1f30d}-\u{e9}".repeat(64)
        } else {
            "memory:substitution-preparation".into()
        },
        SourceKind::Ecore,
        "original immutable declaration identities",
    )
    .id;
    let id = |n| DeclarationId {
        source: source.clone(),
        path: vec![n],
    };
    let parameter = |n| Type::Parameter { declaration: id(n) };
    let classifier = |n, arguments, raw| Type::Classifier {
        declaration: id(n),
        arguments,
        raw,
    };
    let wildcard = || Type::Wildcard {
        upper: None,
        lower: None,
    };
    let nested = |depth| {
        let mut t = wildcard();
        for _ in 0..depth {
            t = classifier(8, vec![t], false);
        }
        t
    };
    let wide = |width| classifier(8, (0..width).map(|_| wildcard()).collect(), false);
    let (bindings, root) = match case {
        0 => (vec![], classifier(8, vec![], false)),
        1 => (
            vec![],
            Type::RigidParameter {
                identity: RigidParameterId {
                    source: source.clone(),
                    declaration: 8,
                    parameter: 3,
                },
            },
        ),
        2 => (vec![], parameter(0)),
        3 => (vec![], wildcard()),
        4 => (
            vec![],
            Type::Wildcard {
                upper: Some(Box::new(parameter(0))),
                lower: None,
            },
        ),
        5 => (
            vec![],
            Type::Wildcard {
                upper: None,
                lower: Some(Box::new(parameter(0))),
            },
        ),
        6 => (
            vec![
                (0, classifier(8, vec![], false)),
                (1, parameter(2)),
                (2, classifier(9, vec![], false)),
            ],
            classifier(
                7,
                vec![
                    parameter(0),
                    parameter(1),
                    Type::Wildcard {
                        upper: None,
                        lower: Some(Box::new(parameter(1))),
                    },
                ],
                false,
            ),
        ),
        7 => (vec![(0, classifier(8, vec![], false))], parameter(0)),
        8 => (vec![(0, parameter(0))], parameter(0)),
        9 | 12 => (
            vec![(0, parameter(1)), (1, classifier(8, vec![], false))],
            if case == 12 {
                classifier(7, vec![parameter(0), parameter(1)], false)
            } else {
                parameter(0)
            },
        ),
        10 => (vec![(0, parameter(1)), (1, parameter(0))], parameter(0)),
        11 => (
            vec![(0, classifier(8, vec![parameter(0)], false))],
            parameter(0),
        ),
        13 => (
            vec![],
            Type::Wildcard {
                upper: Some(Box::new(wildcard())),
                lower: Some(Box::new(wildcard())),
            },
        ),
        14 => (vec![], classifier(8, vec![wildcard()], true)),
        15 => (
            vec![],
            classifier(
                8,
                vec![
                    classifier(8, vec![wildcard()], true),
                    Type::Wildcard {
                        upper: Some(Box::new(wildcard())),
                        lower: Some(Box::new(wildcard())),
                    },
                ],
                false,
            ),
        ),
        16 => (vec![], nested(128)),
        17 => (vec![], nested(129)),
        18 => (vec![], wide(16_383)),
        19 => (vec![], wide(16_384)),
        20 => (vec![(0, nested(128))], parameter(0)),
        21 => (vec![(0, nested(127))], parameter(0)),
        22 => (vec![(0, wide(16_382))], parameter(0)),
        23 => (vec![(0, wide(16_383))], parameter(0)),
        24 => (vec![(0, classifier(8, vec![], false))], parameter(0)),
        25 => (vec![(0, classifier(8, vec![], true))], parameter(0)),
        26 => (
            vec![(0, classifier(8, vec![], false))],
            classifier(7, vec![parameter(1)], false),
        ),
        27 => (
            vec![(0, parameter(0))],
            classifier(7, vec![parameter(0)], false),
        ),
        28 => (
            vec![(0, classifier(8, vec![], false))],
            Type::Wildcard {
                upper: Some(Box::new(classifier(9, vec![], true))),
                lower: None,
            },
        ),
        29 => (
            (0..128).map(|n| (n, parameter((n + 1) % 128))).collect(),
            parameter(0),
        ),
        30 => (
            vec![(
                1,
                Type::Wildcard {
                    upper: Some(Box::new(classifier(8, vec![], false))),
                    lower: None,
                },
            )],
            Type::Wildcard {
                upper: None,
                lower: Some(Box::new(classifier(7, vec![parameter(1)], false))),
            },
        ),
        31 => (
            vec![(0, classifier(8, vec![], false))],
            classifier(9, vec![], true),
        ),
        32 => (
            vec![
                (0, classifier(8, vec![parameter(1)], false)),
                (1, classifier(9, vec![], false)),
            ],
            classifier(7, vec![parameter(0), parameter(1)], false),
        ),
        _ => unreachable!(),
    };
    (
        Substitution::new(
            bindings
                .into_iter()
                .map(|(n, argument)| ParameterBinding {
                    parameter: id(n),
                    argument,
                })
                .collect(),
        )
        .unwrap(),
        root,
    )
}
fn run(
    environment: &Substitution,
    root: &Type,
    mode: usize,
    context: &InvocationContext<'_>,
) -> serde_json::Value {
    match mode {
        0 => match shape_observed(root, context) {
            Ok(()) => serde_json::json!({"shape":"ok"}),
            Err(e) => serde_json::json!({"error":format!("{e:?}")}),
        },
        1 | 2 => match if mode == 1 {
            environment.apply_observed(root, context)
        } else {
            environment.apply_simultaneous_observed(root, context)
        } {
            Ok(output) => serde_json::json!({"owned":output}),
            Err(e) => serde_json::json!({"error":format!("{e:?}")}),
        },
        3 => match environment.apply_view_observed(root, context) {
            Ok(output) => {
                serde_json::json!({"borrowed":matches!(output,std::borrow::Cow::Borrowed(_)),"view":output})
            }
            Err(e) => serde_json::json!({"error":format!("{e:?}")}),
        },
        _ => unreachable!(),
    }
}
fn observation(case: usize, mode: usize) -> serde_json::Value {
    let (environment, root) = fixture(case);
    let events = Rc::new(RefCell::new(vec![]));
    let context = InvocationContext::new(Rc::new(Lease {
        events: events.clone(),
        deny: None,
    }));
    let output = run(&environment, &root, mode, &context);
    let events = events.borrow();
    let payload =
        serde_json::to_vec(&serde_json::json!({"output":output,"events":*events})).unwrap();
    serde_json::json!({"case":case,"mode":mode,"sha256":format!("{:x}",Sha256::digest(&payload)),"callbacks":events.len(),"work":events.iter().filter(|v|v.0=='w').map(|v|v.1).sum::<usize>(),"storage":events.iter().filter(|v|v.0=='s').map(|v|v.1).sum::<usize>(),"error":output.get("error"),"borrowed":output.get("borrowed")})
}
#[test]
fn frozen_original_substitution_outputs_errors_and_ordered_receipts() {
    let expected: Vec<serde_json::Value> = serde_json::from_str(include_str!(
        "generic_substitution_preparation_baseline.json"
    ))
    .unwrap();
    let rows: Vec<_> = (0..33)
        .flat_map(|case| (0..4).map(move |mode| observation(case, mode)))
        .collect();
    assert_eq!(rows, expected);
    assert_eq!(rows.len(), 132);
}

enum Cursor {
    Shape(ShapePreparation),
    Apply(SubstitutionPreparation),
}
impl Cursor {
    fn begin(mode: usize) -> Self {
        match mode {
            0 => Self::Shape(ShapePreparation::begin()),
            1 => Self::Apply(SubstitutionPreparation::transitive()),
            2 => Self::Apply(SubstitutionPreparation::simultaneous()),
            _ => unreachable!(),
        }
    }
    fn advance(
        &mut self,
        root: &Type,
        environment: &Substitution,
        context: &InvocationContext<'_>,
    ) -> Result<bool, PreparationError> {
        match self {
            Self::Shape(job) => job.advance(root, context),
            Self::Apply(job) => job.advance(root, environment, context),
        }
    }
    fn finish(self) -> Result<serde_json::Value, PreparationError> {
        match self {
            Self::Shape(job) => job.finish().map(|()| serde_json::json!({"shape":"ok"})),
            Self::Apply(job) => job.finish().map(|owned| serde_json::json!({"owned":owned})),
        }
    }
}
fn drive(
    case: usize,
    mode: usize,
    quantum: usize,
    deny: Option<usize>,
) -> (serde_json::Value, Events, usize) {
    let mut inputs = fixture(case);
    let events = Rc::new(RefCell::new(vec![]));
    let mut cursor = Cursor::begin(mode);
    let mut advances = 0;
    loop {
        cursor = std::hint::black_box(cursor);
        inputs = std::hint::black_box(inputs);
        let lease = Rc::new(Lease {
            events: events.clone(),
            deny,
        });
        let weak = Rc::downgrade(&lease);
        let result = {
            let context = InvocationContext::new(lease.clone());
            let mut result = Ok(false);
            for _ in 0..quantum {
                advances += 1;
                assert!(advances < 2_000_000);
                result = cursor.advance(&inputs.1, &inputs.0, &context);
                if result != Ok(false) {
                    break;
                }
            }
            result
        };
        drop(lease);
        assert!(weak.upgrade().is_none());
        match result {
            Ok(false) => {}
            Ok(true) => {
                let n = events.borrow().len();
                assert!(
                    cursor
                        .advance(&inputs.1, &inputs.0, &InvocationContext::default())
                        .unwrap()
                );
                assert_eq!(events.borrow().len(), n);
                return (cursor.finish().unwrap(), events, advances);
            }
            Err(error) => {
                let n = events.borrow().len();
                assert!(
                    cursor
                        .advance(&inputs.1, &inputs.0, &InvocationContext::default())
                        .is_err()
                );
                assert!(cursor.finish().is_err());
                assert_eq!(events.borrow().len(), n);
                return (
                    serde_json::json!({"error":format!("{error:?}")}),
                    events,
                    advances,
                );
            }
        }
    }
}
fn complete_digest(output: &serde_json::Value, events: &[(char, usize)]) -> String {
    format!(
        "{:x}",
        Sha256::digest(
            serde_json::to_vec(&serde_json::json!({"output":output,"events":events})).unwrap()
        )
    )
}
#[test]
fn substitution_cursors_match_every_frozen_profile_with_moved_inputs_and_fresh_leases() {
    let expected: Vec<serde_json::Value> = serde_json::from_str(include_str!(
        "generic_substitution_preparation_baseline.json"
    ))
    .unwrap();
    let mut checked = 0;
    for row in expected {
        let case = row["case"].as_u64().unwrap() as usize;
        let mode = row["mode"].as_u64().unwrap() as usize;
        if mode == 3 {
            continue;
        }
        let mut count = None;
        for quantum in [1, 3, 256] {
            let (output, events, advances) = drive(case, mode, quantum, None);
            let events = events.borrow();
            assert_eq!(
                complete_digest(&output, &events),
                row["sha256"],
                "case {case} mode {mode} q{quantum}"
            );
            assert_eq!(*count.get_or_insert(advances), advances);
            assert_eq!(events.len() as u64, row["callbacks"].as_u64().unwrap());
        }
        checked += 1;
    }
    assert_eq!(checked, 99);
}
#[test]
fn substitution_cursors_keep_literal_simultaneous_capture_errors_and_limits() {
    let transitive = drive(9, 1, 1, None).0;
    assert_eq!(
        transitive["owned"]["declaration"]["path"],
        serde_json::json!([8])
    );
    let simultaneous = drive(9, 2, 1, None).0;
    assert_eq!(simultaneous["owned"]["kind"], "parameter");
    assert_eq!(
        simultaneous["owned"]["declaration"]["path"],
        serde_json::json!([1])
    );
    let nested = drive(32, 2, 3, None).0;
    assert_eq!(
        nested["owned"]["arguments"][0]["arguments"][0]["kind"],
        "parameter"
    );
    assert_eq!(
        nested["owned"]["arguments"][0]["arguments"][0]["declaration"]["path"],
        serde_json::json!([1])
    );
    assert_eq!(
        drive(15, 0, 3, None).0["error"],
        "Schema(\"Wildcard cannot have both upper and lower bounds\")"
    );
    assert!(drive(16, 1, 256, None).0.get("owned").is_some());
    assert_eq!(
        drive(17, 1, 256, None).0["error"],
        "Schema(\"Generic expression size/depth budget exceeded\")"
    );
    assert_eq!(
        drive(20, 1, 256, None).0["error"],
        "Schema(\"Generic substitution size/depth budget exceeded\")"
    );
    assert!(drive(18, 1, 256, None).0.get("owned").is_some());
    assert_eq!(
        drive(19, 1, 256, None).0["error"],
        "Schema(\"Generic expression size/depth budget exceeded\")"
    );
    assert_eq!(
        drive(23, 1, 256, None).0["error"],
        "Schema(\"Generic substitution size/depth budget exceeded\")"
    );
}
#[test]
fn substitution_cursors_deny_every_callback_terminally_and_recover() {
    for (case, mode) in [(6, 0), (6, 1), (6, 2), (9, 1), (9, 2), (10, 1), (15, 0)] {
        let (expected, full, _) = drive(case, mode, 1, None);
        let full = full.borrow().clone();
        for callback in 1..=full.len() {
            let (output, events, _) = drive(case, mode, 3, Some(callback));
            assert_eq!(
                output["error"],
                "Observation(\"substitution cursor denied\")"
            );
            assert_eq!(*events.borrow(), full[..callback]);
        }
        assert_eq!(drive(case, mode, 256, None).0, expected);
    }
}
#[test]
fn substitution_cursors_drop_every_frontier_and_keep_prior_outputs_and_inputs() {
    for (case, mode) in [(6, 0), (6, 1), (9, 1), (9, 2)] {
        let (expected, _, advances) = drive(case, mode, 1, None);
        let (environment, root) = fixture(case);
        let root_before = root.clone();
        let entries_before = environment.entries();
        let previous = run(&environment, &root, mode, &InvocationContext::default());
        for prefix in 0..advances {
            let mut cursor = Cursor::begin(mode);
            for _ in 0..prefix {
                assert!(
                    !cursor
                        .advance(&root, &environment, &InvocationContext::default())
                        .unwrap()
                );
            }
            assert!(cursor.finish().is_err());
            assert_eq!(
                run(&environment, &root, mode, &InvocationContext::default()),
                previous
            );
            assert_eq!(root, root_before);
            assert_eq!(environment.entries(), entries_before);
        }
        assert_eq!(drive(case, mode, 256, None).0, expected);
    }
}
struct Budget {
    work: std::cell::Cell<usize>,
    storage: std::cell::Cell<usize>,
    work_limit: usize,
    storage_limit: usize,
}
impl InvocationObserver for Budget {
    fn before_invoke(&self, _: &ProviderId) -> Result<(), String> {
        panic!("structural substitution cannot invoke providers")
    }
    fn work(&self, n: usize) -> Result<(), String> {
        let next = self.work.get() + n;
        if next > self.work_limit {
            return Err("substitution work denied".into());
        }
        self.work.set(next);
        Ok(())
    }
    fn retain_storage(&self, n: usize) -> Result<(), String> {
        let next = self.storage.get() + n;
        if next > self.storage_limit {
            return Err("substitution storage denied".into());
        }
        self.storage.set(next);
        Ok(())
    }
}
#[test]
fn substitution_cursors_preserve_exact_and_one_short_observed_budgets() {
    for (case, mode) in [(0, 0), (6, 1), (9, 1), (9, 2), (24, 1)] {
        let (expected, events, _) = drive(case, mode, 256, None);
        let events = events.borrow();
        let work = events
            .iter()
            .filter(|e| e.0 == 'w')
            .map(|e| e.1)
            .sum::<usize>();
        let storage = events
            .iter()
            .filter(|e| e.0 == 's')
            .map(|e| e.1)
            .sum::<usize>();
        for (w, s, denied) in [
            (work, storage, None),
            (work - 1, storage, Some("substitution work denied")),
            (work, storage - 1, Some("substitution storage denied")),
        ] {
            let (environment, root) = fixture(case);
            let budget = Rc::new(Budget {
                work: std::cell::Cell::new(0),
                storage: std::cell::Cell::new(0),
                work_limit: w,
                storage_limit: s,
            });
            let context = InvocationContext::new(budget);
            let mut cursor = Cursor::begin(mode);
            let output = loop {
                match cursor.advance(&root, &environment, &context) {
                    Ok(false) => {}
                    Ok(true) => break cursor.finish(),
                    Err(error) => break Err(error),
                }
            };
            match denied {
                Some(message) => assert_eq!(
                    output.unwrap_err(),
                    PreparationError::Observation(message.into())
                ),
                None => assert_eq!(output.unwrap(), expected),
            }
        }
    }
}
