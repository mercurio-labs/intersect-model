use super::*;
use crate::core::frozen::FrozenStore;
use crate::{
    core::{
        query::QueryBudget,
        storage::{ModelStore, ModelView, atomic, export},
    },
    definitions::{generic_types::Type, resources::DeclarationId},
    semantic_bindings::BorrowedBindingWork,
};
use serde_json::json;

const SOURCE: &str = r#"rcore 1;
package pendingTree version "1";
metamodel "urn:pending-tree" prefix p {
  class Node {
    ref children: Node [0,*] containment;
    ref pending: Node [0,1];
    ref aliases: Node [0,*] containment derived;
  }
  class Leaf extends Node {}
}
"#;
fn target(schema: &Schema, name: &str) -> StorageResult<RuntimeType> {
    let (_, declaration) = schema
        .declarations
        .iter()
        .find(|(key, declaration)| {
            declaration.namespace == "urn:pending-tree"
                && declaration.name == name
                && schema.classes.contains_key(*key)
        })
        .ok_or("Missing test class")?;
    RuntimeType::new(Type::Classifier {
        declaration: DeclarationId {
            source: declaration.source.clone(),
            path: declaration.path.clone(),
        },
        arguments: vec![],
        raw: false,
    })
}
fn graph(schema: &Schema) -> StorageResult<FrozenStore> {
    let mut store = FrozenStore::for_schema(schema).map_err(|error| error.to_string())?;
    let node = schema
        .class_key_for_type(&target(schema, "Node")?)?
        .to_owned();
    let leaf = schema
        .class_key_for_type(&target(schema, "Leaf")?)?
        .to_owned();
    atomic(&mut store, |store| {
        for (id, class) in [
            ("root", node.as_str()),
            ("middle", node.as_str()),
            ("leaf", leaf.as_str()),
        ] {
            store.create_element(schema, id, class, "memory:tree", 0, 1)?;
        }
        store.set_value("root", "children", Some(json!(["leaf", "middle"])))?;
        store.set_unresolved_reference("root", "pending", "Later", false)
    })?;
    Ok(store)
}
#[test]
fn pending_tree_preserves_order_and_filters_without_admitting_noncontainment() -> StorageResult<()>
{
    let package = crate::compile_rcore(SOURCE).map_err(|e| e.to_string())?;
    let schema = package.schema();
    let store = graph(schema)?;
    let mut budget = QueryBudget::new(1_000_000);
    let work = BorrowedBindingWork::bind(&mut budget);
    let observation = work.observer();
    let tree = PendingContainmentIndex::prepare(&store, schema, &observation)?;
    let node = target(schema, "Node")?;
    let leaf = target(schema, "Leaf")?;
    assert!(tree.has_derived_aliases());
    assert_eq!(
        tree.page(
            "root",
            ContainmentNavigation::Contents,
            &node,
            0,
            &observation
        )?,
        Some("leaf".into())
    );
    assert_eq!(
        tree.page(
            "root",
            ContainmentNavigation::Contents,
            &node,
            1,
            &observation
        )?,
        Some("middle".into())
    );
    assert_eq!(
        tree.page(
            "root",
            ContainmentNavigation::Contents,
            &leaf,
            0,
            &observation
        )?,
        Some("leaf".into())
    );
    assert_eq!(
        tree.page(
            "root",
            ContainmentNavigation::Contents,
            &leaf,
            1,
            &observation
        )?,
        None
    );
    assert_eq!(
        tree.page(
            "leaf",
            ContainmentNavigation::Ancestors,
            &node,
            0,
            &observation
        )?,
        Some("root".into())
    );
    assert!(
        super::super::validate(&store, schema).is_err(),
        "pending tree is not a full model validation award"
    );
    Ok(())
}
#[test]
fn pending_tree_rejects_multiple_owners_cycles_and_unresolved_containment() -> StorageResult<()> {
    let package = crate::compile_rcore(SOURCE).map_err(|e| e.to_string())?;
    let schema = package.schema();
    for mode in 0..3 {
        let mut store = graph(schema)?;
        atomic(&mut store, |store| match mode {
            0 => store.set_value("middle", "children", Some(json!(["leaf"]))),
            1 => store.set_value("leaf", "children", Some(json!(["root"]))),
            _ => store.set_unresolved_reference("root", "children", "Later", true),
        })?;
        assert!(
            PendingContainmentIndex::prepare(&store, schema, &InvocationContext::default())
                .is_err()
        );
    }
    Ok(())
}
fn malformed_derived_edge(store: &mut FrozenStore) -> StorageResult<crate::core::model::Model> {
    let mut model = export(store)?;
    assert!(
        atomic(store, |store| {
            store.set_value("root", "aliases", Some(json!(["leaf"])))
        })
        .is_err()
    );
    assert_eq!(export(store)?, model);
    model
        .elements
        .iter_mut()
        .find(|row| row.id == "root")
        .ok_or("Missing root fixture")?
        .values
        .insert("aliases".into(), json!(["leaf"]));
    Ok(model)
}
#[test]
fn pending_tree_obeys_original_budget_and_refuses_computed_raw_edges() -> StorageResult<()> {
    let package = crate::compile_rcore(SOURCE).map_err(|e| e.to_string())?;
    let schema = package.schema();
    let mut store = graph(schema)?;
    let mut budget = QueryBudget::new(0);
    let work = BorrowedBindingWork::bind(&mut budget);
    assert!(PendingContainmentIndex::prepare(&store, schema, &work.observer()).is_err());
    let model = malformed_derived_edge(&mut store)?;
    let store = ModelView::new(&model)?;
    assert!(
        PendingContainmentIndex::prepare(&store, schema, &InvocationContext::default()).is_err()
    );
    Ok(())
}

#[test]
fn pending_tree_rejects_volatile_containment_instead_of_assuming_stored_edges() -> StorageResult<()>
{
    let source = SOURCE.replace(
        "ref children: Node [0,*] containment;",
        "ref children: Node [0,*] containment volatile;",
    );
    let package = crate::compile_rcore(&source).map_err(|error| error.to_string())?;
    let schema = package.schema();
    let store = graph(schema)?;
    let result = PendingContainmentIndex::prepare(&store, schema, &InvocationContext::default());
    assert!(
        matches!(result, Err(error) if error.contains("Volatile containment")),
        "volatile ownership needs an explicit provider"
    );
    Ok(())
}

#[test]
fn pending_tree_records_volatile_derived_aliases_without_using_them_as_edges() -> StorageResult<()>
{
    let source = SOURCE.replace("containment derived;", "containment derived volatile;");
    let package = crate::compile_rcore(&source).map_err(|error| error.to_string())?;
    let schema = package.schema();
    let mut store = graph(schema)?;
    assert!(
        PendingContainmentIndex::prepare(&store, schema, &InvocationContext::default())?
            .has_derived_aliases()
    );
    let model = malformed_derived_edge(&mut store)?;
    let store = ModelView::new(&model)?;
    assert!(
        matches!(PendingContainmentIndex::prepare(&store, schema, &InvocationContext::default()),
            Err(error) if error.contains("Derived containment has raw stored values"))
    );
    Ok(())
}

fn measured_prepare(
    store: &FrozenStore,
    schema: &Schema,
    limit: usize,
    preconsumed: usize,
) -> StorageResult<usize> {
    let mut budget = QueryBudget::new(limit);
    for _ in 0..preconsumed {
        budget.charge()?;
    }
    let work = BorrowedBindingWork::bind(&mut budget);
    let result = PendingContainmentIndex::prepare(store, schema, &work.observer()).map(|_| ());
    let used = usize::try_from(work.used()).map_err(|_| "Test work overflow")?;
    assert_eq!(budget.reads, preconsumed + used);
    result?;
    Ok(used)
}
#[test]
fn pending_tree_exact_short_and_preconsumed_work_uses_original_allowance() -> StorageResult<()> {
    let package = crate::compile_rcore(SOURCE).map_err(|e| e.to_string())?;
    let schema = package.schema();
    let store = graph(schema)?;
    let used = measured_prepare(&store, schema, 1_000_000, 0)?;
    assert!(used > 1);
    assert_eq!(measured_prepare(&store, schema, used, 0)?, used);
    assert!(measured_prepare(&store, schema, used - 1, 0).is_err());
    assert_eq!(measured_prepare(&store, schema, used + 7, 7)?, used);
    assert!(measured_prepare(&store, schema, used + 6, 7).is_err());
    Ok(())
}

#[test]
fn pending_tree_reuses_only_the_exact_borrowed_schema_proof() -> StorageResult<()> {
    let package = crate::compile_rcore(SOURCE).map_err(|error| error.to_string())?;
    let schema = package.schema();
    let store = graph(schema)?;
    let mut budget = QueryBudget::new(1_000_000);
    let work = BorrowedBindingWork::bind(&mut budget);
    let observation = work.observer();
    let tree = PendingContainmentIndex::prepare(&store, schema, &observation)?;
    let proof = ScopeSchema::prepare_observed(schema, &observation)?;
    let node = target(schema, "Node")?;
    let before = work.used();
    assert_eq!(
        tree.page_with_schema(
            "root",
            ContainmentNavigation::Contents,
            &node,
            0,
            &proof,
            &observation
        )?,
        Some("leaf".into())
    );
    let page_work = work.used() - before;
    let before = work.used();
    assert_eq!(
        tree.page_with_schema(
            "root",
            ContainmentNavigation::Contents,
            &node,
            0,
            &proof,
            &observation
        )?,
        Some("leaf".into())
    );
    assert_eq!(work.used() - before, page_work);
    let foreign_schema = schema.clone();
    assert_eq!(foreign_schema.revision, schema.revision);
    let foreign = ScopeSchema::prepare_observed(&foreign_schema, &observation)?;
    let foreign_result = tree.page_with_schema(
        "root",
        ContainmentNavigation::Contents,
        &node,
        0,
        &foreign,
        &observation,
    );
    assert!(
        matches!(foreign_result, Err(error) if error.contains("runtime-schema owner changed")),
        "equal serialized schemas cannot share the immutable-borrow proof"
    );
    let mut denied = QueryBudget::new(0);
    let denied_work = BorrowedBindingWork::bind(&mut denied);
    assert!(
        tree.page_with_schema(
            "root",
            ContainmentNavigation::Contents,
            &node,
            0,
            &proof,
            &denied_work.observer(),
        )
        .is_err()
    );
    assert!(
        denied_work.check().is_err(),
        "original denial remains sticky"
    );
    assert!(
        tree.page_with_schema(
            "root",
            ContainmentNavigation::Contents,
            &node,
            0,
            &proof,
            &denied_work.observer(),
        )
        .is_err(),
        "denied lookup cannot fall back to unobserved selection"
    );
    Ok(())
}
