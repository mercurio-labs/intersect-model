use super::*;
#[path = "../../../examples/support/component_fixture.rs"]
pub(super) mod fixture;
use serde_json::json;

#[test]
fn component_host_cache_restore_keeps_checked_identity_ownership_and_bounds() -> StorageResult<()> {
    use crate::frozen::SnapshotTrust;
    let fixture = fixture::Fixture::new()?;
    let native = {
        use crate::frozen::{ById, ModelBuilder};
        let mm = fixture.context.metamodel();
        let mut builder = ModelBuilder::new(mm.clone());
        builder
            .create(
                "cached",
                mm.class(&fixture.node).ok_or("Node class")?,
                "memory:/cached",
            )
            .map_err(|e| e.to_string())?;
        builder
            .link(&ById)
            .map_err(|e| format!("{e:?}"))?
            .validate()
            .map_err(|e| format!("{e:?}"))?
    };
    let (image, bytes) =
        ComponentImage::prepare_with_snapshot(fixture.context.clone(), "urn:cached", native)?;
    for trust in [
        SnapshotTrust::Revalidate,
        SnapshotTrust::HostCache,
        SnapshotTrust::VerifiedHostCache,
    ] {
        let restored = ComponentImage::restore_with_trust(
            fixture.context.clone(),
            &image.descriptor(),
            &bytes,
            trust,
        )?;
        assert_eq!(restored.key(), image.key());
        let mounted = MountedModel::new(fixture.context.clone(), vec![restored.clone()])?;
        mounted.validate()?;
        full(&mounted)?;
        let mut fork = mounted.fork()?;
        assert!(
            fork.set(restored.object_id("cached")?, "name", json!("cannot edit"))
                .is_err()
        );
        let mut wrong = image.descriptor();
        wrong.identity.engine = "stale-runtime".into();
        assert!(
            ComponentImage::restore_with_trust(fixture.context.clone(), &wrong, &bytes, trust)
                .is_err()
        );
        let mut wrong = image.descriptor();
        wrong.identity.schema = "stale-schema".into();
        assert!(
            ComponentImage::restore_with_trust(fixture.context.clone(), &wrong, &bytes, trust)
                .is_err()
        );
    }
    let mut corrupt = bytes.clone();
    let last = corrupt.last_mut().ok_or("Snapshot")?;
    *last ^= 1;
    assert!(
        ComponentImage::restore_with_trust(
            fixture.context.clone(),
            &image.descriptor(),
            &corrupt,
            SnapshotTrust::HostCache
        )
        .is_err()
    );
    assert!(
        ComponentImage::restore_with_trust(
            fixture.context.clone(),
            &image.descriptor(),
            &corrupt,
            SnapshotTrust::VerifiedHostCache
        )
        .is_err()
    );
    // Even trusted-cache loading must not substitute another package identity.
    let mut forged = image.descriptor();
    forged.identity.content_digest = "different-content".into();
    forged.key = digest(&(&forged.uri, &forged.identity))?;
    let mut forged_bytes = bytes.clone();
    forged_bytes[48..80].copy_from_slice(
        &crate::definitions::library_part::key_bytes(&forged.key).map_err(|e| e.to_string())?,
    );
    let error = match ComponentImage::restore_with_trust(
        fixture.context,
        &forged,
        &forged_bytes,
        SnapshotTrust::HostCache,
    ) {
        Ok(_) => return Err("Mismatched resource ownership was accepted".into()),
        Err(error) => error,
    };
    assert!(error.contains("resource identity"), "{error}");
    Ok(())
}

#[test]
fn component_validation_reads_native_defaults_without_archive_assignment_markers()
-> StorageResult<()> {
    use crate::frozen::{ById, ModelBuilder};
    let package = crate::compile_rcore(
        r#"rcore 1;
package nativeDefaults version "1";
metamodel "urn:native-defaults" prefix n {
 class Item { attr label: String [0,1] default "default label"; }
}"#,
    )
    .map_err(|e| e.to_string())?;
    let context = SharedSchema::new(package.schema())?;
    let mm = context.metamodel();
    let class = package
        .schema()
        .classes
        .keys()
        .next()
        .ok_or("Missing class")?;
    let mut builder = ModelBuilder::new(mm.clone());
    builder
        .create(
            "item",
            mm.class(class).ok_or("Item class")?,
            "memory:/native-defaults",
        )
        .map_err(|e| e.to_string())?;
    let native = builder
        .link(&ById)
        .map_err(|e| format!("{e:?}"))?
        .validate()
        .map_err(|e| format!("{e:?}"))?;
    let (image, bytes) =
        ComponentImage::prepare_with_snapshot(context.clone(), "urn:defaults", native)?;
    let restored = ComponentImage::restore(context.clone(), &image.descriptor(), &bytes)?;
    let model = MountedModel::new(context, vec![restored.clone()])?;
    model.validate()?;
    full(&model)?;
    assert_eq!(
        model.value(restored.object_id("item")?, "label")?,
        Some(json!("default label"))
    );
    Ok(())
}

fn full(model: &MountedModel) -> StorageResult<()> {
    model
        .store
        .context
        .validation
        .validate_all(&model.store, model.schema(), true)
}

#[test]
fn component_preparation_rejects_evidence_that_qualification_cannot_rebase() -> StorageResult<()> {
    use crate::definitions::{
        bundle::{DefinitionSource, SourceKind},
        conversion::ProviderId,
        initialization::{ClassInitializationContract, InitializationReceipt},
    };
    use crate::frozen::{ById, FrozenStore};
    let fixture = fixture::Fixture::new()?;
    let mut store = FrozenStore::new(fixture.context.metamodel());
    let schema = fixture.context.schema();
    let source = DefinitionSource::new(
        "memory:initializer.rcore",
        SourceKind::Rcore,
        fixture::SOURCE,
    );
    storage::atomic(&mut store, |store| {
        store.set_revision(&schema.revision)?;
        store.create_element(
            schema,
            "initialized",
            &fixture.node,
            "memory:/initialized",
            0,
            0,
        )?;
        store.set_initialization(
            "initialized",
            Some(InitializationReceipt {
                object: "initialized".into(),
                contract: ClassInitializationContract {
                    version: 1,
                    schema_revision: schema.revision.clone(),
                    class: fixture.node.clone(),
                    class_identity: schema.identity(&fixture.node).to_owned(),
                    provider: ProviderId {
                        name: "test initializer".into(),
                        version: "1".into(),
                    },
                    sources: vec![source.id.clone()],
                    members: vec![],
                },
                authored_members: vec![],
            }),
        )
    })?;
    fixture
        .context
        .validation
        .validate_all(&store, schema, true)?;
    let native = store
        .into_builder()
        .map_err(|e| e.to_string())?
        .link(&ById)
        .map_err(|e| format!("{e:?}"))?
        .validate()
        .map_err(|e| format!("{e:?}"))?;
    let error = match ComponentImage::prepare(fixture.context.clone(), "urn:initialized", native) {
        Ok(_) => return Err("Unqualified constructor evidence was admitted".into()),
        Err(error) => error,
    };
    assert!(
        error.contains("object-bound initialization receipts"),
        "{error}"
    );
    Ok(())
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
