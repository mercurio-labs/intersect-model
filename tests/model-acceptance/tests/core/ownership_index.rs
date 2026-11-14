use intersect_model::core::{
    api::{BasicModel, ModelEdit, VersionedModel},
    storage::validate_ownership_with_conversions,
};
use intersect_model::definitions::conversion::ConversionRegistry;
use serde_json::json;

#[test]
fn staged_ownership_storage_failure_is_terminal_before_publication() {
    use intersect_model::core::{
        model::Model,
        storage::{ModelView, StructuralPreparation},
    };
    use intersect_model::definitions::conversion::{
        InvocationContext, InvocationObserver, ProviderId,
    };
    use std::{cell::Cell, rc::Rc};
    struct Budget {
        bytes: Cell<usize>,
        limit: usize,
    }
    impl InvocationObserver for Budget {
        fn before_invoke(&self, _: &ProviderId) -> Result<(), String> {
            panic!("no provider call")
        }
        fn retain_storage(&self, bytes: usize) -> Result<(), String> {
            let next = self.bytes.get() + bytes;
            if next > self.limit {
                return Err("ownership storage interrupted".into());
            }
            self.bytes.set(next);
            Ok(())
        }
    }
    let schema = intersect_model_acceptance::migration_fixture::compile_migrated_schema_fixture(
        include_str!("../../../../fixtures/ecore-navigation/ownership.ecore"),
    )
    .unwrap();
    let mut model = Model::default();
    for id in ["a", "b", "c"] {
        model.create(&schema, id, "Base", "r", 0, 0).unwrap();
    }
    model.elements[0]
        .values
        .insert("zChildren".into(), json!(["b"]));
    model.elements[1]
        .values
        .insert("zChildren".into(), json!(["c"]));
    model.resources =
        intersect_model::core::resources::records_for_elements(&schema, &model.elements).unwrap();
    let original = model.clone();
    let view = ModelView::new(&model).unwrap();
    // Edges retain 328 bytes. Sorted cycle starts b/c copy two IDs each;
    // paths b->a and c->b->a retain three visited/cursor pairs. Five
    // 130-byte scratch transitions yield 978 edge/cycle bytes. Shared feature
    // caches and object cursors now precede these; their policy is independently
    // pinned by structural_cache_storage and definitions' observation controls.
    let baseline = Rc::new(Budget {
        bytes: Cell::new(0),
        limit: usize::MAX,
    });
    let observation = InvocationContext::new(baseline.clone());
    let mut pending = StructuralPreparation::ownership();
    while !pending
        .advance(&view, &schema, None, Some(&observation))
        .unwrap()
    {}
    assert_eq!(pending.finish().unwrap().retained_bytes(), 328);
    let needed = baseline.bytes.get();
    assert!(needed > 978);
    for limit in [
        0,
        163,
        164,
        327,
        328,
        329,
        457,
        458,
        587,
        588,
        717,
        718,
        847,
        848,
        977,
        978,
        979,
        needed - 1,
        needed,
        needed + 1,
    ] {
        let budget = Rc::new(Budget {
            bytes: Cell::new(0),
            limit,
        });
        let observation = InvocationContext::new(budget.clone());
        let mut pending = StructuralPreparation::ownership();
        loop {
            match pending.advance(&view, &schema, None, Some(&observation)) {
                Ok(false) => {}
                Ok(true) => {
                    assert!(limit >= needed);
                    let index = pending.finish().unwrap();
                    assert_eq!(index.retained_bytes(), 328);
                    assert_eq!(budget.bytes.get(), needed);
                    assert_eq!(index.parent("b"), Some("a"));
                    assert_eq!(index.parent("c"), Some("b"));
                    break;
                }
                Err(error) => {
                    assert!(limit < needed);
                    assert_eq!(error, "ownership storage interrupted");
                    assert!(budget.bytes.get() <= limit);
                    assert!(
                        pending
                            .advance(&view, &schema, None, Some(&observation))
                            .is_err()
                    );
                    assert!(pending.finish().is_err());
                    break;
                }
            }
        }
    }
    assert_eq!(model, original);
}

#[test]
fn ownership_index_retains_effective_order_and_snapshot_isolation() {
    let schema = intersect_model_acceptance::migration_fixture::compile_migrated_schema_fixture(
        include_str!("../../../../fixtures/ecore-navigation/ownership.ecore"),
    )
    .unwrap();
    let mut model = BasicModel::new(schema.clone()).unwrap();
    for (id, class) in [
        ("root", "Root"),
        ("b", "Base"),
        ("a", "Base"),
        ("single", "Base"),
        ("last", "Base"),
    ] {
        model.create(id, class, "r").unwrap();
    }
    model.set("root", "zChildren", json!(["b", "a"])).unwrap();
    model.set("root", "aChild", json!("single")).unwrap();
    model.set("root", "local", json!(["last"])).unwrap();
    let mut versioned = VersionedModel::new(model).unwrap();
    let old = versioned.snapshot().unwrap();
    let registry = ConversionRegistry::default();
    let index = validate_ownership_with_conversions(&old, &schema, &registry).unwrap();
    assert_eq!(index.parent("b"), Some("root"));
    assert_eq!(index.parent("root"), None);
    assert_eq!(index.children("root"), ["b", "a", "single", "last"]);
    assert!(index.children("b").is_empty());
    assert!(index.retained_bytes() > 0);
    let mut fork = versioned.fork(&old).unwrap();
    fork.set("root", "zChildren", json!([])).unwrap();
    fork.set("root", "aChild", json!("b")).unwrap();
    let changed = validate_ownership_with_conversions(&fork, &schema, &registry).unwrap();
    assert_eq!(changed.children("root"), ["b", "last"]);
    assert_eq!(changed.parent("single"), None);
    versioned.commit(fork).unwrap();
    assert_eq!(index.children("root"), ["b", "a", "single", "last"]);
    let current = versioned.snapshot().unwrap();
    assert_eq!(
        validate_ownership_with_conversions(&current, &schema, &registry)
            .unwrap()
            .children("root"),
        ["b", "last"]
    );
}

#[test]
fn staged_structure_keeps_ownership_private_and_rejects_cycles() {
    use intersect_model::core::{
        model::Model,
        storage::{ModelView, StructuralPreparation},
    };
    use intersect_model::definitions::conversion::InvocationContext;
    let schema = intersect_model_acceptance::migration_fixture::compile_migrated_schema_fixture(
        include_str!("../../../../fixtures/ecore-navigation/ownership.ecore"),
    )
    .unwrap();
    let registry = ConversionRegistry::default();
    let observation = InvocationContext::default();
    let mut model = Model::default();
    for id in ["a", "b", "c"] {
        model.create(&schema, id, "Base", "r", 0, 0).unwrap();
    }
    model.elements[0]
        .values
        .insert("zChildren".into(), json!(["b"]));
    model.elements[1]
        .values
        .insert("zChildren".into(), json!(["c"]));
    model.resources =
        intersect_model::core::resources::records_for_elements(&schema, &model.elements).unwrap();
    let view = ModelView::new(&model).unwrap();
    assert!(StructuralPreparation::ownership().finish().is_err());
    let mut pending = StructuralPreparation::ownership();
    let mut slices = 0;
    while !pending
        .advance(&view, &schema, Some(&registry), Some(&observation))
        .unwrap()
    {
        slices += 1;
        // Class ancestry/member caches now advance before the ownership graph.
        assert!(slices < 1000);
    }
    assert!(slices > 15);
    let ownership = pending.finish().unwrap();
    assert_eq!(ownership.parent("b"), Some("a"));
    assert_eq!(ownership.parent("c"), Some("b"));
    drop(view);
    model.elements[2]
        .values
        .insert("zChildren".into(), json!(["a"]));
    model.resources =
        intersect_model::core::resources::records_for_elements(&schema, &model.elements).unwrap();
    let view = ModelView::new(&model).unwrap();
    let mut pending = StructuralPreparation::ownership();
    loop {
        match pending.advance(&view, &schema, Some(&registry), Some(&observation)) {
            Ok(false) => {}
            Ok(true) => panic!("cycle accepted"),
            Err(e) => {
                assert!(e.contains("Containment cycle"));
                break;
            }
        }
    }
    assert!(
        pending
            .advance(&view, &schema, Some(&registry), Some(&observation))
            .is_err()
    );
    assert!(pending.finish().is_err());
}
