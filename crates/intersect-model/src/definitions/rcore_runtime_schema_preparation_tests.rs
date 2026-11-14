use super::*;
use crate::definitions::{
    bundle::{DefinitionSource, SourceKind},
    conversion::{InvocationObserver, ProviderId},
    runtime_schema_test_inputs as inputs,
};
use std::{cell::RefCell, rc::Rc};
type Events = Rc<RefCell<Vec<(char, usize)>>>;
struct Lease {
    events: Events,
    deny: bool,
}
impl Lease {
    fn record(&self, kind: char, units: usize) -> std::result::Result<(), String> {
        self.events.borrow_mut().push((kind, units));
        if self.deny {
            Err("model closure denied".into())
        } else {
            Ok(())
        }
    }
}
impl InvocationObserver for Lease {
    fn before_invoke(&self, _: &ProviderId) -> std::result::Result<(), String> {
        panic!("unconfigured model metadata cannot invoke a provider")
    }
    fn checkpoint(&self) -> std::result::Result<(), String> {
        self.record('c', 0)
    }
    fn work(&self, n: usize) -> std::result::Result<(), String> {
        self.record('w', n)
    }
    fn retain_storage(&self, n: usize) -> std::result::Result<(), String> {
        self.record('s', n)
    }
}
fn caches(models: &ModelTypes<'_>) -> serde_json::Value {
    serde_json::json!({"generic_checks":models.generic_checks.iter().collect::<Vec<_>>(),"schemas":models.runtime_schemas.iter().map(|(key, schema)| (key, schema.schema())).collect::<Vec<_>>()})
}
#[test]
fn retained_model_closure_matches_frozen_schemas_partial_caches_errors_and_quanta() {
    let baseline: serde_json::Value =
        serde_json::from_str(include_str!("runtime_schema_baseline.json")).unwrap();
    for case in 0..12 {
        let (bundle, requested, _) = inputs::fixture(case);
        let mut receipt = None;
        let mut count = None;
        for quantum in [1, 3, 256] {
            let mut models = ModelTypes::new(&bundle).unwrap();
            let pointer = models.resources.document(&bundle.sources[0].id).unwrap() as *const _;
            let events = Rc::new(RefCell::new(vec![]));
            let mut job =
                RuntimeSchemaPreparation::begin(&models, &requested, &InvocationContext::default())
                    .unwrap();
            let mut advances = 0;
            let result = loop {
                job = std::hint::black_box(job);
                let lease = Rc::new(Lease {
                    events: events.clone(),
                    deny: false,
                });
                let weak = Rc::downgrade(&lease);
                let outcome = {
                    let context = InvocationContext::new(lease.clone());
                    let mut outcome = Ok(false);
                    for _ in 0..quantum {
                        advances += 1;
                        assert!(advances < 100_000);
                        outcome = job.advance(&mut models, &context);
                        if outcome != Ok(false) {
                            break;
                        }
                    }
                    outcome
                };
                drop(lease);
                assert!(weak.upgrade().is_none());
                match outcome {
                    Ok(false) => {}
                    Ok(true) => {
                        break job.finish().map(|key| {
                            serde_json::to_value(models.runtime_schemas[&key].schema()).unwrap()
                        });
                    }
                    Err(error) => {
                        assert_eq!(
                            job.advance(&mut models, &InvocationContext::default()),
                            Err(error.clone())
                        );
                        assert_eq!(job.finish(), Err(error.clone()));
                        break Err(error);
                    }
                }
            }
            .map_err(|e| e.to_string());
            assert_eq!(
                serde_json::to_value(result).unwrap(),
                baseline["model"][case]["schema"],
                "case {case}"
            );
            assert_eq!(
                caches(&models),
                baseline["model"][case]["after_schema"],
                "case {case}"
            );
            assert_eq!(
                models.resources.document(&bundle.sources[0].id).unwrap() as *const _,
                pointer
            );
            assert_eq!(*count.get_or_insert(advances), advances);
            assert_eq!(
                *receipt.get_or_insert_with(|| events.borrow().clone()),
                *events.borrow()
            );
        }
    }
}
#[test]
fn retained_model_feature_matches_frozen_complete_projection_and_cold_caches() {
    let baseline: serde_json::Value =
        serde_json::from_str(include_str!("runtime_schema_baseline.json")).unwrap();
    for case in 0..12 {
        let (bundle, requested, name) = inputs::fixture(case);
        let mut receipt = None;
        let mut count = None;
        for quantum in [1, 3, 256] {
            let mut job = ModelFeaturePreparation::begin(
                &bundle,
                requested[0].clone(),
                name.into(),
                &InvocationContext::default(),
            )
            .unwrap();
            let events = Rc::new(RefCell::new(vec![]));
            let mut advances = 0;
            let outcome = loop {
                job = std::hint::black_box(job);
                let lease = Rc::new(Lease {
                    events: events.clone(),
                    deny: false,
                });
                let weak = Rc::downgrade(&lease);
                let outcome = {
                    let context = InvocationContext::new(lease.clone());
                    let mut outcome = Ok(false);
                    for _ in 0..quantum {
                        advances += 1;
                        assert!(advances < 100_000);
                        outcome = job.advance(&context);
                        if outcome != Ok(false) {
                            break;
                        }
                    }
                    outcome
                };
                drop(lease);
                assert!(weak.upgrade().is_none());
                if outcome != Ok(false) {
                    break outcome;
                }
                assert!(job.output.is_none());
            };
            assert_eq!(
                caches(job.models.as_ref().unwrap()),
                baseline["model"][case]["cold_caches"]
            );
            if let Err(error) = outcome {
                assert!(job.output.is_none());
                assert_eq!(
                    job.advance(&InvocationContext::default()),
                    Err(error.clone())
                );
                assert_eq!(job.finish(), Err(error.clone()));
                assert_eq!(
                    serde_json::to_value(std::result::Result::<FeatureProjection, String>::Err(
                        error.to_string()
                    ))
                    .unwrap(),
                    baseline["model"][case]["cold_projection"]
                );
            } else {
                assert_eq!(
                    serde_json::to_value(job.finish().map_err(|e| e.to_string())).unwrap(),
                    baseline["model"][case]["cold_projection"]
                );
            }
            assert_eq!(*count.get_or_insert(advances), advances);
            assert_eq!(
                *receipt.get_or_insert_with(|| events.borrow().clone()),
                *events.borrow()
            );
        }
    }
}
fn seeded<'a>(models: &ModelTypes<'a>, requested: &[ModelType]) -> RuntimeSchemaPreparation<'a> {
    RuntimeSchemaPreparation {
        phase: Phase::Closure,
        key: requested.to_vec(),
        resolver: Some(ClosureResolver::idle(models.fresh_resolver().unwrap())),
        pending: requested.to_vec(),
        visited: BTreeSet::new(),
        requested: vec![],
        closure: vec![],
        report: None,
        failure: None,
        claim: None,
    }
}
#[test]
fn retained_model_closure_drops_and_cancels_every_closure_materialization_prefix() {
    let (bundle, requested, _) = inputs::fixture(0);
    let mut model = ModelTypes::new(&bundle).unwrap();
    let mut job = seeded(&model, &requested);
    let mut advances = 1;
    while !job
        .advance(&mut model, &InvocationContext::default())
        .unwrap()
    {
        advances += 1;
    }
    let expected = caches(&model);
    for prefix in 0..=advances {
        let mut models = ModelTypes::new(&bundle).unwrap();
        let mut job = seeded(&models, &requested);
        for _ in 0..prefix {
            job.advance(&mut models, &InvocationContext::default())
                .unwrap();
        }
        if prefix < advances {
            assert!(models.runtime_schemas.is_empty());
        }
        let prior = caches(&models);
        let lease = Rc::new(Lease {
            events: Rc::new(RefCell::new(vec![])),
            deny: true,
        });
        let weak = Rc::downgrade(&lease);
        let error = {
            let context = InvocationContext::new(lease.clone());
            job.advance(&mut models, &context).unwrap_err()
        };
        drop(lease);
        assert!(weak.upgrade().is_none());
        assert_eq!(
            error,
            ModelPreparationError::Observation("model closure denied".into())
        );
        assert_eq!(
            job.advance(&mut models, &InvocationContext::default()),
            Err(error.clone())
        );
        assert_eq!(job.finish(), Err(error));
        assert_eq!(caches(&models), prior);
        let mut fresh = ModelTypes::new(&bundle).unwrap();
        fresh.runtime_schema(&requested).unwrap();
        assert_eq!(caches(&fresh), expected);
    }
    let models = ModelTypes::new(&bundle).unwrap();
    assert!(seeded(&models, &requested).finish().is_err());
}
#[test]
fn retained_model_feature_ready_cancellation_does_not_publish_projection() {
    let (bundle, requested, name) = inputs::fixture(0);
    let mut job = ModelFeaturePreparation::begin(
        &bundle,
        requested[0].clone(),
        name.into(),
        &InvocationContext::default(),
    )
    .unwrap();
    while !job.advance(&InvocationContext::default()).unwrap() {}
    let context = InvocationContext::new(Rc::new(Lease {
        events: Rc::new(RefCell::new(vec![])),
        deny: true,
    }));
    assert!(matches!(
        job.advance(&context),
        Err(ModelPreparationError::Observation(_))
    ));
    assert!(job.finish().is_err());
    let mut fresh = ModelFeaturePreparation::begin(
        &bundle,
        requested[0].clone(),
        name.into(),
        &InvocationContext::default(),
    )
    .unwrap();
    while !fresh.advance(&InvocationContext::default()).unwrap() {}
    assert_eq!(fresh.finish().unwrap().declaration.path, vec![0, 0]);
}
#[test]
fn retained_model_closure_enforces_the_original_1024_type_limit() {
    let classes = (0..1025)
        .map(|i| format!("class N{i} {{}}"))
        .collect::<String>();
    let source = DefinitionSource::new(
        "memory:closure-limit.rcore",
        SourceKind::Rcore,
        format!(
            "rcore 1;package p version \"1\";metamodel \"urn:closure-limit\" prefix p {{{classes}}}"
        ),
    );
    let requested = (0..1025)
        .map(|i| ModelType::Classifier {
            declaration: DeclarationId {
                source: source.id.clone(),
                path: vec![i],
            },
            arguments: vec![],
            raw: false,
        })
        .collect::<Vec<_>>();
    let bundle = DefinitionBundle {
        version: 1,
        sources: vec![source],
        namespaces: vec![],
    };
    let mut models = ModelTypes::new(&bundle).unwrap();
    let mut job = seeded(&models, &requested);
    loop {
        match job.advance(&mut models, &InvocationContext::default()) {
            Ok(false) => {}
            Ok(true) => panic!("oversized closure was accepted"),
            Err(error) => {
                assert_eq!(
                    error.to_string(),
                    "Rcore model application closure exceeds 1024 types"
                );
                break;
            }
        }
    }
    assert_eq!(job.visited.len(), 1025);
    assert!(models.runtime_schemas.is_empty());
}

#[test]
fn retained_model_closure_keeps_successful_generic_checks_before_a_later_error() {
    let (bundle, types, _) = inputs::fixture(2);
    let valid = types[1].clone();
    let mut invalid = types[3].clone();
    if let ModelType::Classifier { arguments, .. } = &mut invalid {
        arguments.push(types[4].clone());
    }
    let mut models = ModelTypes::new(&bundle).unwrap();
    let runtime = RuntimeType::new(valid.clone()).unwrap();
    let expected = models
        .fresh_resolver()
        .unwrap()
        .runtime_members(&runtime)
        .unwrap();
    // The sorted key pops Box<Value> before the invalid Value<Other>.
    let mut job = RuntimeSchemaPreparation::begin(
        &models,
        &[invalid, valid.clone()],
        &InvocationContext::default(),
    )
    .unwrap();
    let error = loop {
        match job.advance(&mut models, &InvocationContext::default()) {
            Ok(false) => {}
            Ok(true) => panic!("invalid generic arity was accepted"),
            Err(error) => break error,
        }
    };
    assert!(matches!(error, ModelPreparationError::Diagnostic(_)));
    assert_eq!(models.generic_checks.get(&valid), Some(&expected));
    assert_eq!(models.generic_checks.len(), 1);
    assert!(models.runtime_schemas.is_empty());
    assert_eq!(job.finish(), Err(error));
}
fn classifier(source: &DefinitionSource, index: usize) -> ModelType {
    ModelType::Classifier {
        declaration: DeclarationId {
            source: source.id.clone(),
            path: vec![index],
        },
        arguments: vec![],
        raw: false,
    }
}
// One context reuses the closures and schemas it already prepared; every
// schema and generic check must equal those of a fresh context per type.
#[test]
fn retained_closure_and_schema_reuse_matches_fresh_preparations() {
    let mutual = DefinitionSource::new(
        "memory:closure-reuse.rcore",
        SourceKind::Rcore,
        "rcore 1;package p version \"1\";metamodel \"urn:closure-reuse\" prefix p {\
         class A {ref b:B [0,1];}class B {ref a:A [0,1];}class C extends A {}}",
    );
    let mutual_types = (0..3).map(|i| classifier(&mutual, i)).collect::<Vec<_>>();
    let mutual = DefinitionBundle {
        version: 1,
        sources: vec![mutual],
        namespaces: vec![],
    };
    let (generic, generic_types, _) = inputs::fixture(2);
    for (bundle, types) in [(&mutual, mutual_types), (&generic, generic_types)] {
        for order in [types.clone(), types.iter().rev().cloned().collect()] {
            let mut reused = ModelTypes::new(bundle).unwrap();
            let mut checks = BTreeMap::new();
            for ty in &order {
                let key = std::slice::from_ref(ty);
                let schema =
                    serde_json::to_value(reused.runtime_schema(key).unwrap().schema()).unwrap();
                let mut fresh = ModelTypes::new(bundle).unwrap();
                let expected =
                    serde_json::to_value(fresh.runtime_schema(key).unwrap().schema()).unwrap();
                assert_eq!(schema, expected);
                checks.extend(fresh.generic_checks);
            }
            assert_eq!(reused.generic_checks, checks);
            assert_eq!(reused.runtime_closures.len(), order.len());
        }
    }
    // A and B share one closure, so the second materializes nothing.
    let mut models = ModelTypes::new(&mutual).unwrap();
    let types = (0..2)
        .map(|i| classifier(&mutual.sources[0], i))
        .collect::<Vec<_>>();
    for ty in &types {
        models.runtime_schema(std::slice::from_ref(ty)).unwrap();
    }
    assert_eq!(models.runtime_schemas.len(), 2);
    assert_eq!(models.materialized.len(), 1);
}
#[test]
fn retained_conformance_answers_repeat_the_first_result() {
    let source = DefinitionSource::new(
        "memory:conformance-reuse.rcore",
        SourceKind::Rcore,
        "rcore 1;package p version \"1\";metamodel \"urn:conformance-reuse\" prefix p {\
         class Root {}class Leaf extends Root {}}",
    );
    let root = classifier(&source, 0);
    let leaf = classifier(&source, 1);
    let bundle = DefinitionBundle {
        version: 1,
        sources: vec![source],
        namespaces: vec![],
    };
    let mut models = ModelTypes::new(&bundle).unwrap();
    for _ in 0..2 {
        assert!(models.conforms_application(&leaf, &root).unwrap());
        assert!(!models.conforms_application(&root, &leaf).unwrap());
    }
    assert_eq!(models.conformance.len(), 2);
}
#[test]
fn retained_model_schema_cache_hit_preserves_sorted_deduplicated_context() {
    let (bundle, types, _) = inputs::fixture(2);
    let mut models = ModelTypes::new(&bundle).unwrap();
    models.runtime_schema(&types).unwrap();
    let before = caches(&models);
    let mut reordered = types.into_iter().rev().collect::<Vec<_>>();
    reordered.push(reordered[0].clone());
    let mut job =
        RuntimeSchemaPreparation::begin(&models, &reordered, &InvocationContext::default())
            .unwrap();
    assert!(matches!(job.phase, Phase::Complete));
    assert!(
        job.advance(&mut models, &InvocationContext::default())
            .unwrap()
    );
    let key = job.finish().unwrap();
    assert!(models.runtime_schemas.contains_key(&key));
    assert_eq!(caches(&models), before);
}
// Everything a context records: schemas, generic checks, single-root closures
// and the domain contexts its type report exposes.
fn recorded(models: &ModelTypes<'_>) -> (serde_json::Value, String, String) {
    (
        caches(models),
        format!("{:?}", models.runtime_closures),
        format!("{:?}", models.domain_contexts()),
    )
}
// A context importing from a shared store records exactly what preparing the
// same keys cold records, whatever another context prepared before, in any
// order and with partial overlap, and imports instead of materializing again.
#[test]
fn shared_store_imports_record_exactly_the_cold_preparations() {
    let mutual = DefinitionSource::new(
        "memory:store-reuse.rcore",
        SourceKind::Rcore,
        "rcore 1;package p version \"1\";metamodel \"urn:store-reuse\" prefix p {\
         class A {ref b:B [0,1];}class B {ref a:A [0,1];}class C extends A {}}",
    );
    let mutual_types = (0..3).map(|i| classifier(&mutual, i)).collect::<Vec<_>>();
    let mutual = DefinitionBundle {
        version: 1,
        sources: vec![mutual],
        namespaces: vec![],
    };
    let mut cases = vec![(mutual, mutual_types)];
    for case in [0, 1, 2] {
        let (bundle, types, _) = inputs::fixture(case);
        cases.push((bundle, types));
    }
    for (bundle, types) in &cases {
        let keys = types
            .iter()
            .map(|ty| vec![ty.clone()])
            .chain([types.clone()])
            .collect::<Vec<_>>();
        for order in [keys.clone(), keys.iter().rev().cloned().collect()] {
            for donated in 0..=order.len() {
                let store = RuntimeSchemaStore::new();
                let mut donor = ModelTypes::new(bundle).unwrap().with_schema_store(&store);
                for key in order.iter().rev().take(donated) {
                    donor.runtime_schema(key).unwrap();
                }
                let mut shared = ModelTypes::new(bundle).unwrap().with_schema_store(&store);
                let mut cold = ModelTypes::new(bundle).unwrap();
                for key in &order {
                    shared.runtime_schema(key).unwrap();
                    cold.runtime_schema(key).unwrap();
                    assert_eq!(recorded(&shared), recorded(&cold));
                }
                // Every key the donor prepared was imported, not re-materialized.
                for key in order.iter().rev().take(donated) {
                    let mut sorted = key.clone();
                    sorted.sort();
                    sorted.dedup();
                    assert!(Arc::ptr_eq(
                        &shared.runtime_schemas[&sorted],
                        &donor.runtime_schemas[&sorted]
                    ));
                }
            }
        }
    }
}
// A store binds to the first context's sources and limits; other sources or
// limits are not attached and so never import a foreign preparation.
#[test]
fn shared_store_attaches_only_to_its_sources_and_limits() {
    let (bundle, types, _) = inputs::fixture(0);
    let (other, _, _) = inputs::fixture(1);
    let store = RuntimeSchemaStore::new();
    let mut first = ModelTypes::new(&bundle).unwrap().with_schema_store(&store);
    assert!(first.schema_store.is_some());
    first.runtime_schema(&types).unwrap();
    assert!(!store.is_empty());
    assert!(
        ModelTypes::new(&other)
            .unwrap()
            .with_schema_store(&store)
            .schema_store
            .is_none()
    );
    let limits = crate::definitions::ecore::ParseLimits {
        source_bytes: 1 << 20,
    };
    assert_ne!(limits, Default::default());
    assert!(
        ModelTypes::with_source_limits(&bundle, None, limits)
            .unwrap()
            .with_schema_store(&store)
            .schema_store
            .is_none()
    );
    assert!(
        ModelTypes::new(&bundle)
            .unwrap()
            .with_schema_store(&store)
            .schema_store
            .is_some()
    );
}
// Claims: an unmetered context on another thread awaits a claimed key; the
// claimer's own thread, metered contexts and threads holding a claim never
// wait; a claim released without a result passes to a waiter.
#[test]
fn store_claims_await_only_from_unclaimed_unmetered_threads() {
    use super::super::runtime_schema_store::Lookup;
    let (_, types, _) = inputs::fixture(0);
    let key = types.clone();
    let other = vec![ModelType::Wildcard {
        upper: None,
        lower: None,
    }];
    let store = RuntimeSchemaStore::new();
    let Lookup::Prepare(Some(claim)) = store.lookup(&key, true) else {
        panic!("an unclaimed key is claimed");
    };
    assert!(matches!(store.lookup(&key, true), Lookup::Prepare(None)));
    std::thread::scope(|s| {
        s.spawn(|| {
            assert!(matches!(store.lookup(&key, true), Lookup::Busy));
            assert!(matches!(store.lookup(&key, false), Lookup::Prepare(None)));
            assert!(matches!(store.lookup(&other, false), Lookup::Prepare(None)));
            let Lookup::Prepare(Some(held)) = store.lookup(&other, true) else {
                panic!("an unclaimed key is claimed");
            };
            assert!(matches!(store.lookup(&key, true), Lookup::Prepare(None)));
            drop(held);
            assert!(matches!(store.lookup(&key, true), Lookup::Busy));
        });
    });
    std::thread::scope(|s| {
        let waiter = s.spawn(|| {
            loop {
                match store.await_step(&key) {
                    Lookup::Busy => continue,
                    Lookup::Prepare(Some(_)) => return true,
                    _ => return false,
                }
            }
        });
        drop(claim);
        assert!(waiter.join().unwrap());
    });
    assert!(store.is_empty());
}
// A context awaiting another thread's claimed preparation imports the entry
// that thread publishes rather than preparing the key again.
#[test]
fn awaiting_contexts_import_the_claimers_published_schema() {
    use super::super::runtime_schema_store::Lookup;
    let (bundle, types, _) = inputs::fixture(0);
    let cold = serde_json::to_string(
        ModelTypes::new(&bundle)
            .unwrap()
            .runtime_schema(&types)
            .unwrap()
            .schema(),
    )
    .unwrap();
    let mut key = types.clone();
    key.sort();
    key.dedup();
    let store = RuntimeSchemaStore::new();
    let Lookup::Prepare(Some(claim)) = store.lookup(&key, true) else {
        panic!("an unclaimed key is claimed");
    };
    std::thread::scope(|s| {
        // Debug-build schema preparation needs more than a default thread stack.
        let waiter = std::thread::Builder::new()
            .stack_size(64 << 20)
            .spawn_scoped(s, || {
                let mut models = ModelTypes::new(&bundle).unwrap().with_schema_store(&store);
                let schema = models.runtime_schema(&types).unwrap();
                (
                    serde_json::to_string(schema.schema()).unwrap(),
                    schema as *const CheckedSchema as usize,
                )
            })
            .unwrap();
        let mut models = ModelTypes::new(&bundle).unwrap().with_schema_store(&store);
        let published = models.runtime_schema(&types).unwrap() as *const CheckedSchema as usize;
        drop(claim);
        let (awaited, address) = waiter.join().unwrap();
        assert_eq!(awaited, cold);
        assert_eq!(address, published);
    });
    assert_eq!(store.len(), 1);
}
// One context's materializations share resolved runtime operation sets; each
// schema still equals one materialized by a fresh context for that key alone.
#[test]
fn shared_operation_sets_materialize_the_same_schemas() {
    for case in [0, 1, 2] {
        let (bundle, types, _) = inputs::fixture(case);
        let keys = types
            .iter()
            .map(|ty| vec![ty.clone()])
            .chain([types.clone()])
            .collect::<Vec<_>>();
        let mut warm = ModelTypes::new(&bundle).unwrap();
        for key in keys.iter().chain(keys.iter().rev()) {
            let shared = serde_json::to_string(warm.runtime_schema(key).unwrap().schema()).unwrap();
            let fresh = serde_json::to_string(
                ModelTypes::new(&bundle)
                    .unwrap()
                    .runtime_schema(key)
                    .unwrap()
                    .schema(),
            )
            .unwrap();
            assert_eq!(shared, fresh);
        }
        assert!(!warm.runtime_operations.borrow().is_empty());
    }
}
// The checked lookup skips per-call re-validation but selects the same class.
#[test]
fn checked_class_keys_match_validating_lookups() {
    for case in [0, 1, 2] {
        let (bundle, types, _) = inputs::fixture(case);
        let mut models = ModelTypes::new(&bundle).unwrap();
        let checked = models.runtime_schema(&types).unwrap();
        for ty in &types {
            let runtime = RuntimeType::new(ty.clone()).unwrap();
            assert_eq!(
                checked.class_key_for_type(&runtime),
                checked.schema().class_key_for_type(&runtime)
            );
        }
    }
}
