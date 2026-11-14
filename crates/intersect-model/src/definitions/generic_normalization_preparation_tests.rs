//! Complete original normalization observations; fixed AST cases are supplementary.
use super::*;
use crate::definitions::{
    bundle::{DefinitionBundle, DefinitionSource, SourceKind},
    conversion::{InvocationObserver, ProviderId},
    ecore::{ObservedImportError, ParserObservation},
    generic_resolution::TargetKind,
    generics::TypeReference,
};
use sha2::{Digest, Sha256};
use std::{cell::RefCell, rc::Rc};
type Events = Rc<RefCell<Vec<(char, usize)>>>;
struct Lease {
    events: Events,
    deny: Option<usize>,
}
impl Lease {
    fn record(&self, k: char, n: usize) -> Result<(), String> {
        let mut e = self.events.borrow_mut();
        e.push((k, n));
        if self.deny == Some(e.len()) {
            Err("normalization denied".into())
        } else {
            Ok(())
        }
    }
}
impl InvocationObserver for Lease {
    fn before_invoke(&self, _: &ProviderId) -> Result<(), String> {
        panic!("normalization provider")
    }
    fn checkpoint(&self) -> Result<(), String> {
        self.record('c', 0)
    }
    fn work(&self, n: usize) -> Result<(), String> {
        self.record('w', n)
    }
    fn retain_storage(&self, n: usize) -> Result<(), String> {
        self.record('s', n)
    }
}
pub(super) fn authored(case: usize) -> (DefinitionBundle, String) {
    let rcore = case >= 12;
    let kind = case % 12;
    let generic = match kind {
        0 => "<eGenericType eClassifier='#//Leaf'/>",
        1 => "<eGenericType eClassifier='#//Box'/>",
        2 => "<eGenericType eTypeParameter='#//Host/P'/>",
        3 => {
            "<eGenericType eClassifier='#//Box'><eTypeArguments eClassifier='#//Leaf'/></eGenericType>"
        }
        4 => {
            "<eGenericType eClassifier='#//Box'><eTypeArguments><eUpperBound eClassifier='#//Base'/></eTypeArguments></eGenericType>"
        }
        5 => {
            "<eGenericType eClassifier='#//Box'><eTypeArguments><eLowerBound eClassifier='#//Leaf'/></eTypeArguments></eGenericType>"
        }
        6 => {
            "<eGenericType eClassifier='#//Pair'><eTypeArguments eClassifier='#//Box'><eTypeArguments eClassifier='#//Leaf'/></eTypeArguments><eTypeArguments/></eGenericType>"
        }
        7 => "<eGenericType eClassifier='#//Missing'/>",
        8 => {
            "<eGenericType eClassifier='#//Box'><eTypeArguments eClassifier='#//Leaf'/><eTypeArguments eClassifier='#//Base'/></eGenericType>"
        }
        9 => "<eGenericType eClassifier='#//Leaf' eTypeParameter='#//Host/P'/>",
        10 => "<eGenericType eClassifier='memory:normalization-dependency.ecore#//Foreign'/>",
        _ => {
            "<eGenericType eClassifier='#//Box'><eTypeArguments><eUpperBound eClassifier='#//Base'/><eLowerBound eClassifier='#//Leaf'/></eTypeArguments></eGenericType>"
        }
    };
    let text = if rcore {
        let ty = match kind {
            0 => "Leaf",
            1 => "Box",
            2 => "P",
            3 => "Box<Leaf>",
            4 => "Box<? extends Base>",
            5 => "Box<? super Leaf>",
            6 => "Pair<Box<Leaf>,?>",
            7 => "Missing",
            8 => "Box<Leaf,Base>",
            9 => "Pair<P,Box<Leaf>>",
            10 => "Pair<Box<Box<Leaf>>,Base>",
            _ => "Box<?>",
        };
        format!(
            "rcore 1;package normalize version \"1\";metamodel \"urn:normalize\" prefix n {{class Base {{}}class Leaf extends Base {{}}class Box<T> {{}}class Pair<A,B> {{}}class Host<P> {{ref value:{ty} [0,1];}}}}"
        )
    } else {
        format!(
            "<e:EPackage xmlns:e='http://www.eclipse.org/emf/2002/Ecore' xmlns:x='http://www.w3.org/2001/XMLSchema-instance' name='normalize' nsURI='urn:normalize'><eClassifiers x:type='e:EClass' name='Base'/><eClassifiers x:type='e:EClass' name='Leaf' eSuperTypes='#//Base'/><eClassifiers x:type='e:EClass' name='Box'><eTypeParameters name='T'/></eClassifiers><eClassifiers x:type='e:EClass' name='Pair'><eTypeParameters name='A'/><eTypeParameters name='B'/></eClassifiers><eClassifiers x:type='e:EClass' name='Host'><eTypeParameters name='P'/><eStructuralFeatures x:type='e:EReference' name='value'>{generic}</eStructuralFeatures></eClassifiers></e:EPackage>"
        )
    };
    let source = DefinitionSource::new(
        if kind == 6 {
            "memory:normalization-\u{1f30d}-\u{e9}.rcore"
        } else {
            "memory:normalization-root"
        },
        if rcore {
            SourceKind::Rcore
        } else {
            SourceKind::Ecore
        },
        text,
    );
    let mut sources = vec![source];
    if !rcore && kind == 10 {
        sources.push(DefinitionSource::new("memory:normalization-dependency.ecore",SourceKind::Ecore,"<e:EPackage xmlns:e='http://www.eclipse.org/emf/2002/Ecore' xmlns:x='http://www.w3.org/2001/XMLSchema-instance' name='dep' nsURI='urn:dep'><eClassifiers x:type='e:EClass' name='Foreign'/></e:EPackage>"));
    }
    (
        DefinitionBundle {
            version: 1,
            sources,
            namespaces: vec![],
        },
        "#//Host/value/@eGenericType.0".into(),
    )
}
fn fixed(case: usize) -> (GenericType, Vec<Binding>) {
    let source = DefinitionSource::new(
        if case == 10 {
            "memory:fixed-normalization-\u{3b1}".repeat(64)
        } else {
            "memory:fixed-normalization".into()
        },
        SourceKind::Ecore,
        "supplementary immutable checked syntax",
    )
    .id;
    let id = |n| DeclarationId {
        source: source.clone(),
        path: vec![n],
    };
    let span = |n| SourceSpan {
        source: source.clone(),
        start: n,
        end: n + 1,
    };
    let reference = || TypeReference {
        href: "#//unused".into(),
        kind_hint: None,
        span: span(0),
    };
    let node = |n, value| GenericType {
        declaration: id(n),
        span: span(n),
        source_payload: format!("retained payload {n}"),
        value,
    };
    let leaf = |n| {
        node(
            n,
            GenericKind::Classifier {
                reference: reference(),
                arguments: vec![],
            },
        )
    };
    let mut rows = vec![
        Binding {
            usage: id(0),
            target: id(100),
            target_kind: TargetKind::Class,
            raw: false,
        },
        Binding {
            usage: id(1),
            target: id(101),
            target_kind: TargetKind::Class,
            raw: false,
        },
        Binding {
            usage: id(2),
            target: id(102),
            target_kind: TargetKind::Parameter,
            raw: false,
        },
    ];
    let mut root = match case {
        1 => node(
            2,
            GenericKind::Parameter {
                reference: reference(),
            },
        ),
        2 => node(
            3,
            GenericKind::Wildcard {
                upper: None,
                lower: None,
            },
        ),
        3 => node(
            3,
            GenericKind::Wildcard {
                upper: Some(Box::new(leaf(1))),
                lower: None,
            },
        ),
        4 => node(
            3,
            GenericKind::Wildcard {
                upper: None,
                lower: Some(Box::new(leaf(1))),
            },
        ),
        5 => node(
            3,
            GenericKind::Wildcard {
                upper: Some(Box::new(leaf(1))),
                lower: Some(Box::new(leaf(1))),
            },
        ),
        6..=10 => node(
            0,
            GenericKind::Classifier {
                reference: reference(),
                arguments: vec![
                    node(
                        2,
                        GenericKind::Parameter {
                            reference: reference(),
                        },
                    ),
                    node(
                        3,
                        GenericKind::Wildcard {
                            upper: Some(Box::new(leaf(1))),
                            lower: None,
                        },
                    ),
                ],
            },
        ),
        11 | 12 => {
            let mut n = leaf(1);
            for _ in 0..if case == 11 { 128 } else { 129 } {
                n = node(
                    0,
                    GenericKind::Classifier {
                        reference: reference(),
                        arguments: vec![n],
                    },
                );
            }
            n
        }
        13 | 14 => node(
            0,
            GenericKind::Classifier {
                reference: reference(),
                arguments: (0..if case == 13 { 16_383 } else { 16_384 })
                    .map(|_| leaf(1))
                    .collect(),
            },
        ),
        _ => leaf(0),
    };
    if case == 7 {
        rows.retain(|r| r.usage != id(1));
    }
    if case == 8 {
        rows[0].raw = true;
    }
    if case == 9 {
        rows.push(Binding {
            usage: id(1),
            target: id(999),
            target_kind: TargetKind::Enumeration,
            raw: true,
        });
    }
    if case == 15 {
        root = leaf(77);
    }
    (root, rows)
}
fn output<T: serde::Serialize>(r: Result<T, ObservedImportError>) -> serde_json::Value {
    match r {
        Ok(v) => serde_json::json!({"ok":v}),
        Err(e) => serde_json::json!({"error":format!("{e:?}")}),
    }
}
fn original_authored(case: usize) -> serde_json::Value {
    let (bundle, fragment) = authored(case);
    let events = Rc::new(RefCell::new(vec![]));
    let resolver = match Resolver::new(&bundle) {
        Ok(r) => r,
        Err(e) => return serde_json::json!({"setup_error":format!("{e:?}")}),
    };
    let selected = match resolver
        .resource_sets()
        .0
        .resolve(&bundle.sources[0].id, &fragment)
    {
        Ok(id) => id,
        Err(e) => return serde_json::json!({"selection_error":e}),
    };
    let context = InvocationContext::new(Rc::new(Lease {
        events: events.clone(),
        deny: None,
    }));
    let result = output(resolver.type_expression_observed(&selected, &context));
    serde_json::json!({"output":result,"events":*events.borrow()})
}
fn original_fixed(case: usize, whole: bool) -> serde_json::Value {
    let (root, rows) = fixed(case);
    let events = Rc::new(RefCell::new(vec![]));
    let context = InvocationContext::new(Rc::new(Lease {
        events: events.clone(),
        deny: None,
    }));
    let observation = ParserObservation::new(&context);
    let run = || -> Result<serde_json::Value, ImportError> {
        let mut bindings = BTreeMap::new();
        for row in &rows {
            if whole {
                observation.work(1, &root.span.source)?;
                observation.reserve(128, &root.span.source)?;
            }
            bindings.insert(&row.usage, row);
        }
        let expression = normalize_observed(root.clone(), &bindings, &observation)?;
        if whole {
            observation
                .preparation_at(&root.span, |c| shape_observed(&expression, c))
                .map_err(|mut e| {
                    if e.kind == ErrorKind::InvalidDefinition {
                        e.kind = ErrorKind::ResourceLimit;
                    }
                    e
                })?;
            observation.reserve(128, &root.span.source)?;
            Ok(serde_json::to_value(TypeUse {
                declaration: root.declaration.clone(),
                span: root.span.clone(),
                expression,
            })
            .unwrap())
        } else {
            Ok(serde_json::to_value(expression).unwrap())
        }
    };
    let result = output(observation.finish(run()));
    serde_json::json!({"output":result,"events":*events.borrow()})
}
fn complete_rows() -> Vec<serde_json::Value> {
    let mut out = vec![];
    for case in 0..24 {
        out.push(serde_json::json!({"authored":case,"observation":original_authored(case)}));
    }
    for case in 0..16 {
        for whole in [false, true] {
            out.push(serde_json::json!({"fixed":case,"whole":whole,"observation":original_fixed(case,whole)}));
        }
    }
    out
}
#[test]
fn frozen_original_checked_normalization_outputs_errors_and_ordered_receipts() {
    let expected: Vec<serde_json::Value> = serde_json::from_str(include_str!(
        "generic_normalization_preparation_baseline.json"
    ))
    .unwrap();
    let actual = complete_rows();
    assert_eq!(actual.len(), expected.len());
    for (index, (row, original)) in actual.iter().zip(expected).enumerate() {
        let bytes = serde_json::to_vec(row).unwrap();
        assert_eq!(
            format!("{:x}", Sha256::digest(bytes)),
            original["complete_sha256"].as_str().unwrap(),
            "Complete original normalization profile {index} changed"
        );
    }
}

// These fixed-input probes are supplementary. Only authored fresh resolution
// establishes transfer of the existing source syntax/binding admissions.
fn retained_authored(case: usize, quantum: usize, deny: Option<usize>) -> serde_json::Value {
    use super::normalization_preparation::TypeExpressionPreparation;
    let (bundle, fragment) = authored(case);
    let events = Rc::new(RefCell::new(vec![]));
    let resolver = match Resolver::new(&bundle) {
        Ok(r) => r,
        Err(e) => return serde_json::json!({"setup_error":format!("{e:?}")}),
    };
    let selected = match resolver
        .resource_sets()
        .0
        .resolve(&bundle.sources[0].id, &fragment)
    {
        Ok(id) => id,
        Err(e) => return serde_json::json!({"selection_error":e}),
    };
    let mut job = TypeExpressionPreparation::begin();
    let mut initial = true;
    let result = 'run: loop {
        job = std::hint::black_box(job);
        let observer = Rc::new(Lease {
            events: events.clone(),
            deny,
        });
        let weak = Rc::downgrade(&observer);
        let context = InvocationContext::new(observer);
        if initial {
            initial = false;
            if let Err(e) = context.checkpoint() {
                break output::<TypeUse>(Err(ObservedImportError::Observation(e)));
            }
        }
        let observation = ParserObservation::new(&context);
        for _ in 0..quantum {
            match observation.finish(job.advance(&resolver, &selected, &observation)) {
                Ok(true) => {
                    assert!(job.advance(&resolver, &selected, &observation).unwrap());
                    break 'run output(observation.finish(job.finish(&selected)));
                }
                Ok(false) => {}
                Err(e) => {
                    let before = events.borrow().len();
                    assert!(job.advance(&resolver, &selected, &observation).is_err());
                    assert_eq!(events.borrow().len(), before);
                    assert!(job.finish(&selected).is_err());
                    break 'run output::<TypeUse>(Err(e));
                }
            }
        }
        drop(observation);
        drop(context);
        assert!(
            weak.upgrade().is_none(),
            "lease retained by normalization cursor"
        );
    };
    serde_json::json!({"output":result,"events":*events.borrow()})
}
fn retained_fixed(case: usize, whole: bool, quantum: usize) -> serde_json::Value {
    use super::normalization_preparation::{
        NormalizationPreparation, TypeExpressionPreparation, fixed_bindings,
    };
    let (root, rows) = fixed(case);
    let selection = root.declaration.clone(); // fixture driver, outside observed work
    let events = Rc::new(RefCell::new(vec![]));
    let (bundle, _) = authored(0);
    let resolver = Resolver::new(&bundle).unwrap();
    let result = if whole {
        let mut job = TypeExpressionPreparation::from_fixed(root, rows);
        'run: loop {
            job = std::hint::black_box(job);
            let context = InvocationContext::new(Rc::new(Lease {
                events: events.clone(),
                deny: None,
            }));
            let observation = ParserObservation::new(&context);
            for _ in 0..quantum {
                match observation.finish(job.advance(&resolver, &selection, &observation)) {
                    Ok(true) => break 'run output(observation.finish(job.finish(&selection))),
                    Ok(false) => {}
                    Err(e) => break 'run output::<TypeUse>(Err(e)),
                }
            }
        }
    } else {
        let mut bindings = fixed_bindings(rows);
        let mut job = NormalizationPreparation::begin(root);
        'run: loop {
            job = std::hint::black_box(job);
            bindings = std::hint::black_box(bindings);
            let context = InvocationContext::new(Rc::new(Lease {
                events: events.clone(),
                deny: None,
            }));
            let observation = ParserObservation::new(&context);
            for _ in 0..quantum {
                match observation.finish(job.advance(&bindings, &observation)) {
                    Ok(true) => {
                        break 'run output(observation.finish(job.finish().map(|v| v.expression)));
                    }
                    Ok(false) => {}
                    Err(e) => break 'run output::<Type>(Err(e)),
                }
            }
        }
    };
    serde_json::json!({"output":result,"events":*events.borrow()})
}
#[test]
#[cfg_attr(not(feature = "milestone-tests"), ignore = "milestone")]
fn checked_normalization_moved_fresh_leases_q1_q3_q256_match_all_56_frozen_profiles() {
    let expected: Vec<serde_json::Value> = serde_json::from_str(include_str!(
        "generic_normalization_preparation_baseline.json"
    ))
    .unwrap();
    for quantum in [1, 3, 256] {
        let mut rows = vec![];
        for case in 0..24 {
            rows.push(serde_json::json!({"authored":case,"observation":retained_authored(case,quantum,None)}));
        }
        for case in 0..16 {
            for whole in [false, true] {
                rows.push(serde_json::json!({"fixed":case,"whole":whole,"observation":retained_fixed(case,whole,quantum)}));
            }
        }
        for (index, (row, original)) in rows.iter().zip(&expected).enumerate() {
            assert_eq!(
                format!("{:x}", Sha256::digest(serde_json::to_vec(row).unwrap())),
                original["complete_sha256"].as_str().unwrap(),
                "q{quantum} complete profile{index}"
            );
        }
    }
}
#[test]
#[cfg_attr(not(feature = "milestone-tests"), ignore = "milestone")]
fn checked_normalization_every_authored_callback_denial_preserves_prefix_and_terminal_state() {
    for case in [3, 6, 8, 15, 18, 20] {
        let original = original_authored(case);
        let callbacks = original["events"].as_array().unwrap();
        for deny in 1..=callbacks.len() {
            let actual = retained_authored(case, 3, Some(deny));
            assert!(
                actual["output"]["error"]
                    .as_str()
                    .unwrap()
                    .contains("normalization denied")
            );
            assert_eq!(
                actual["events"].as_array().unwrap(),
                &callbacks[..deny],
                "case{case} deny{deny}"
            );
        }
        assert_eq!(
            retained_authored(case, 1, None),
            original,
            "fresh recovery{case}"
        );
    }
}

#[test]
fn checked_normalization_drop_at_every_source_cursor_boundary_preserves_inputs_and_recovers() {
    use super::normalization_preparation::TypeExpressionPreparation;
    for case in [3, 6, 15, 18] {
        let (bundle, fragment) = authored(case);
        let before = serde_json::to_value(&bundle).unwrap();
        let pointer = bundle.sources[0].text.as_ptr();
        let resolver = Resolver::new(&bundle).unwrap();
        let selected = resolver
            .resource_sets()
            .0
            .resolve(&bundle.sources[0].id, &fragment)
            .unwrap();
        let expected = resolver.type_expression(&selected).unwrap();
        let mut prefixes = 0;
        loop {
            let mut job = TypeExpressionPreparation::begin();
            let context = InvocationContext::default();
            let observation = ParserObservation::new(&context);
            let mut done = false;
            for _ in 0..prefixes {
                job = std::hint::black_box(job);
                if job.advance(&resolver, &selected, &observation).unwrap() {
                    done = true;
                    break;
                }
            }
            if done {
                assert_eq!(job.finish(&selected).unwrap(), expected);
                break;
            }
            assert!(job.finish(&selected).is_err()); // consuming an incomplete cursor publishes nothing
            assert_eq!(resolver.type_expression(&selected).unwrap(), expected);
            assert_eq!(serde_json::to_value(&bundle).unwrap(), before);
            assert_eq!(bundle.sources[0].text.as_ptr(), pointer);
            prefixes += 1;
            assert!(prefixes < 256);
        }
        assert!(prefixes > 8);
    }
}
#[test]
fn checked_normalization_original_exact_and_one_short_source_budgets() {
    use std::cell::Cell;
    struct Budget {
        work: Cell<usize>,
        storage: Cell<usize>,
        max_work: usize,
        max_storage: usize,
    }
    impl InvocationObserver for Budget {
        fn before_invoke(&self, _: &ProviderId) -> Result<(), String> {
            Err("unexpected provider".into())
        }
        fn checkpoint(&self) -> Result<(), String> {
            Ok(())
        }
        fn work(&self, n: usize) -> Result<(), String> {
            let v = self.work.get() + n;
            self.work.set(v);
            if v > self.max_work {
                Err("work exhausted".into())
            } else {
                Ok(())
            }
        }
        fn retain_storage(&self, n: usize) -> Result<(), String> {
            let v = self.storage.get() + n;
            self.storage.set(v);
            if v > self.max_storage {
                Err("storage exhausted".into())
            } else {
                Ok(())
            }
        }
    }
    for case in [3, 6, 15, 18] {
        let frozen = original_authored(case);
        let events = frozen["events"].as_array().unwrap();
        let total = |kind: &str| {
            events
                .iter()
                .filter(|e| e[0] == kind)
                .map(|e| e[1].as_u64().unwrap() as usize)
                .sum::<usize>()
        };
        let work = total("w");
        let storage = total("s");
        let (bundle, fragment) = authored(case);
        let resolver = Resolver::new(&bundle).unwrap();
        let selected = resolver
            .resource_sets()
            .0
            .resolve(&bundle.sources[0].id, &fragment)
            .unwrap();
        for (w, s, ok) in [
            (work, storage, true),
            (work - 1, storage, false),
            (work, storage - 1, false),
        ] {
            let context = InvocationContext::new(Rc::new(Budget {
                work: Cell::new(0),
                storage: Cell::new(0),
                max_work: w,
                max_storage: s,
            }));
            assert_eq!(
                resolver
                    .type_expression_observed(&selected, &context)
                    .is_ok(),
                ok,
                "source case{case} budget{w}/{s}"
            );
        }
    }
}
