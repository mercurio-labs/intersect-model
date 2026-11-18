use super::*;
#[path = "../../../examples/support/component_fixture.rs"]
mod fixture;
use serde_json::json;

fn full(model: &MountedModel) -> StorageResult<()> {
    model
        .store
        .context
        .validation
        .validate_all(&model.store, model.schema(), true)
}

#[test]
fn partition_validation_keeps_cross_boundary_opposites_and_failed_edits() -> StorageResult<()> {
    let fixture = fixture::Fixture::new()?;
    let mut model = fixture.project("project")?;
    model.create("other", &fixture.other, "memory:/other")?;
    let target = fixture.library.object_id("shared")?;
    model.set("project", "out", json!([target]))?;
    model.validate()?;
    full(&model)?;
    assert!(model.shares_component_with(&model.fork()?, fixture::URI));
    assert!(model.set("project", "target", json!("other")).is_err());
    model.validate()?;
    full(&model)?;
    // A malformed changed inverse must not inherit the immutable base proof.
    model
        .store
        .overlays
        .get_mut(target)
        .ok_or("Missing overlay")?
        .insert("incoming".into(), json!(["missing"]));
    assert!(model.validate().is_err());
    assert!(full(&model).is_err());
    Ok(())
}

#[test]
fn partition_validation_checks_raw_project_values_and_recovers() -> StorageResult<()> {
    let fixture = fixture::Fixture::new()?;
    let mut model = fixture.project("project")?;
    storage::atomic(&mut model.store.project, |store| {
        store.set_resolved_value("project", "target", Some(json!("missing")))
    })?;
    assert!(model.validate().is_err());
    assert!(full(&model).is_err());
    storage::atomic(&mut model.store.project, |store| {
        store.set_resolved_value(
            "project",
            "target",
            Some(json!(fixture.library.object_id("shared")?)),
        )
    })?;
    model.validate()?;
    full(&model)
}

#[test]
fn partition_validation_keeps_global_ownership_fallback() -> StorageResult<()> {
    let fixture = fixture::Fixture::new()?;
    let mut model = fixture.project("project")?;
    let target = fixture.library.object_id("shared")?;
    // Bypass the checked setter as a parser can during construction. A
    // component root may not silently become owned by a project resource.
    storage::atomic(&mut model.store.project, |store| {
        store.set_resolved_value("project", "children", Some(json!([target])))
    })?;
    assert!(model.validate().is_err());
    assert!(full(&model).is_err());
    Ok(())
}

#[test]
fn partition_validation_keeps_project_cycles_and_reserved_identity_checks() -> StorageResult<()> {
    let fixture = fixture::Fixture::new()?;
    let mut draft = MountedDraft::new(fixture.context.clone(), vec![fixture.library.clone()])?;
    storage::atomic(draft.project_store_mut(), |store| {
        store.create_element(
            fixture.context.schema(),
            "a",
            &fixture.node,
            "memory:/project",
            0,
            0,
        )?;
        store.create_element(
            fixture.context.schema(),
            "b",
            &fixture.node,
            "memory:/project",
            0,
            0,
        )?;
        store.set_resolved_value("a", "children", Some(json!(["b"])))?;
        store.set_resolved_value("b", "children", Some(json!(["a"])))
    })?;
    draft.reconcile_opposites()?;
    assert!(draft.finish().is_err());
    let mut draft = MountedDraft::new(fixture.context.clone(), vec![fixture.library.clone()])?;
    storage::atomic(draft.project_store_mut(), |store| {
        store.create_element(
            fixture.context.schema(),
            fixture.library.object_id("shared")?,
            &fixture.node,
            "memory:/project",
            0,
            0,
        )
    })?;
    assert!(draft.finish().is_err());
    Ok(())
}
