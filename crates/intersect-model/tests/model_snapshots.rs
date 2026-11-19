//! Handwritten first-class snapshot/diff qualification through public APIs.
use intersect_model::{
    api::{
        BasicModel, ModelDiff, ModelEdit, ModelSnapshot, ObjectChange, SnapshotError,
        SnapshotLimits, StoredValue, VersionedModel,
    },
    compile_rcore,
    query::QueryBudget,
    storage::{
        self, ContainmentDestination, DeleteRequest, DeleteSelection, IncomingReferences,
        ModelRead, ReparentRequest,
    },
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

const SOURCE: &str = include_str!("../assets/fixtures/snapshots/node.rcore");
fn fixture() -> Result<(BasicModel, String), String> {
    let package = compile_rcore(SOURCE).map_err(|e| e.to_string())?;
    let mut schema = package.schema().clone();
    let key = schema.classes.keys().next().ok_or("Missing Node")?.clone();
    schema
        .classes
        .get_mut(&key)
        .ok_or("Missing Node")?
        .features
        .get_mut("enabled")
        .ok_or("Missing enabled")?
        .unsettable = true;
    Ok((BasicModel::new(schema)?, key))
}
fn changed<'a>(diff: &'a ModelDiff, id: &str) -> &'a [intersect_model::api::FeatureChange] {
    diff.objects()
        .iter()
        .find_map(|change| match change {
            ObjectChange::Updated {
                before, features, ..
            } if before.id == id => Some(features.as_slice()),
            _ => None,
        })
        .expect("Expected changed object")
}
fn rewrite_content(wire: &mut Value) {
    // Deliberately re-sign untrusted content to prove integrity validation runs
    // independently of content digests. These are checksums, not signatures.
    let model: intersect_model::model::Model =
        serde_json::from_value(wire["model"].clone()).unwrap();
    wire["identity"]["content"] = json!(format!(
        "sha256:{:x}",
        Sha256::digest(serde_json::to_vec(&model).unwrap())
    ));
}

#[test]
fn incomplete_capture_is_immutable_readable_and_can_resume_edits()
-> Result<(), Box<dyn std::error::Error>> {
    let (mut model, key) = fixture()?;
    model.create("n", &key, "memory:one")?;
    let before = model.snapshot()?;
    let held = before.clone();
    let id = before.identity()?;
    assert!(before.validate().is_err());
    model.set("n", "name", json!("Équipe — Δ 日本語 🚀"))?;
    assert_eq!(held.identity()?, id);
    assert_eq!(before.value("n", "name")?, Some(Value::Null));
    let after = model.snapshot()?;
    after.validate()?;
    let diff = ModelDiff::between(&before, &after)?;
    assert_eq!(diff.before(), &id);
    let feature = changed(&diff, "n")
        .iter()
        .find(|f| f.name == "name")
        .unwrap();
    assert_eq!(feature.before.value, StoredValue::Present(Value::Null));
    assert_eq!(
        feature.after.value,
        StoredValue::Present(json!("Équipe — Δ 日本語 🚀"))
    );
    let mut resumed = before.to_model()?;
    resumed.set("n", "name", json!("Independent"))?;
    assert_eq!(before.identity()?, id);
    assert_eq!(resumed.get("n")?.unwrap().id, "n");
    Ok(())
}

#[test]
fn captures_round_trip_with_exact_identity_and_fresh_owner_versions()
-> Result<(), Box<dyn std::error::Error>> {
    let (mut model, key) = fixture()?;
    model.create("b", &key, "memory:one")?;
    model.create("a", &key, "memory:two")?;
    model.set("b", "links", json!(["a"]))?;
    let original = model.snapshot()?;
    let bytes = original.to_bytes(SnapshotLimits::default())?;
    let restored = ModelSnapshot::from_bytes(original.schema(), &bytes, SnapshotLimits::default())?;
    assert_eq!(original.identity()?, restored.identity()?);
    assert_ne!(original.version(), restored.version());
    assert_eq!(bytes, restored.to_bytes(SnapshotLimits::default())?);
    assert_eq!(storage::export(&original)?, storage::export(&restored)?);
    assert!(ModelDiff::between(&original, &restored)?.is_empty());
    assert!(restored.validate().is_err());
    assert_eq!(
        restored
            .id_page(None, 5)?
            .iter()
            .map(|(_, id)| id.as_str())
            .collect::<Vec<_>>(),
        ["b", "a"]
    );
    Ok(())
}

#[test]
fn comparison_detects_assignment_changes_with_unchanged_values()
-> Result<(), Box<dyn std::error::Error>> {
    let (mut model, key) = fixture()?;
    model.create("n", &key, "memory:one")?;
    let a = model.snapshot()?;
    model.set("n", "enabled", json!(false))?;
    let b = model.snapshot()?;
    let diff = ModelDiff::between(&a, &b)?;
    let feature = changed(&diff, "n")
        .iter()
        .find(|f| f.name == "enabled")
        .unwrap();
    assert_eq!(feature.before.value, feature.after.value);
    assert_eq!(feature.before.explicitly_set, Some(false));
    assert_eq!(feature.after.explicitly_set, Some(true));
    model.unset("n", "enabled")?;
    let c = model.snapshot()?;
    assert!(ModelDiff::between(&a, &c)?.objects().is_empty());
    assert!(!ModelDiff::between(&a, &c)?.resources().is_empty());
    let restored = ModelSnapshot::from_bytes(
        b.schema(),
        &b.to_bytes(SnapshotLimits::default())?,
        SnapshotLimits::default(),
    )?;
    assert!(storage::is_explicitly_set(
        &restored,
        restored.schema(),
        "n",
        "enabled"
    )?);
    Ok(())
}

#[test]
fn comparison_preserves_reference_order_containment_and_resource_roots()
-> Result<(), Box<dyn std::error::Error>> {
    let (mut model, key) = fixture()?;
    for id in ["owner", "other", "a", "b"] {
        model.create(id, &key, "memory:one")?;
    }
    model.set("owner", "links", json!(["a", "b"]))?;
    model.set("owner", "children", json!(["a"]))?;
    let a = model.snapshot()?;
    model.set("owner", "links", json!(["b", "a"]))?;
    model.reparent(
        ReparentRequest {
            object: "a",
            destination: Some(ContainmentDestination {
                owner: "other",
                feature: "children",
                position: None,
            }),
        },
        None,
        &mut QueryBudget::new(100_000),
    )?;
    let b = model.snapshot()?;
    let diff = ModelDiff::between(&a, &b)?;
    let links = changed(&diff, "owner")
        .iter()
        .find(|f| f.name == "links")
        .unwrap();
    assert_eq!(links.before.value, StoredValue::Present(json!(["a", "b"])));
    assert_eq!(links.after.value, StoredValue::Present(json!(["b", "a"])));
    assert_eq!(a.value("owner", "children")?, Some(json!(["a"])));
    assert_eq!(b.value("other", "children")?, Some(json!(["a"])));
    assert!(diff.resources().iter().any(|r| r.uri == "memory:one"));
    let bytes = b.to_bytes(SnapshotLimits::default())?;
    let restored = ModelSnapshot::from_bytes(b.schema(), &bytes, SnapshotLimits::default())?;
    assert_eq!(
        serde_json::to_value(&diff)?,
        serde_json::to_value(ModelDiff::between(&a, &restored)?)?
    );
    Ok(())
}

#[test]
fn creation_deletion_and_empty_resources_are_explicit() -> Result<(), Box<dyn std::error::Error>> {
    let (mut model, key) = fixture()?;
    let empty = model.snapshot()?;
    model.create_resource("memory:empty")?;
    model.create("z", &key, "memory:one")?;
    let created = model.snapshot()?;
    let diff = ModelDiff::between(&empty, &created)?;
    assert!(matches!(&diff.objects()[0],ObjectChange::Created { after } if after.id=="z"));
    assert_eq!(
        diff.resources()
            .iter()
            .map(|r| r.uri.as_str())
            .collect::<Vec<_>>(),
        ["memory:empty", "memory:one"]
    );
    model.delete_objects(
        DeleteRequest {
            roots: &["z"],
            selection: DeleteSelection::Objects,
            incoming: IncomingReferences::Reject,
        },
        None,
        &mut QueryBudget::new(100_000),
    )?;
    let removed = model.snapshot()?;
    assert!(
        matches!(&ModelDiff::between(&created,&removed)?.objects()[0],ObjectChange::Deleted { before } if before.id=="z")
    );
    assert_eq!(created.get("z")?.unwrap().id, "z");
    Ok(())
}

#[test]
fn exact_schema_comparison_refuses_same_revision_with_different_definition()
-> Result<(), Box<dyn std::error::Error>> {
    let (model, _) = fixture()?;
    let a = model.snapshot()?;
    let mut schema = a.schema().clone();
    schema.name.push_str(" changed");
    let b = BasicModel::new(schema)?.snapshot()?;
    assert_eq!(a.schema().revision, b.schema().revision);
    assert!(matches!(
        ModelDiff::between(&a, &b),
        Err(SnapshotError::SchemaMismatch)
    ));
    assert!(matches!(
        ModelSnapshot::from_bytes(
            b.schema(),
            &a.to_bytes(SnapshotLimits::default())?,
            SnapshotLimits::default()
        ),
        Err(SnapshotError::SchemaMismatch)
    ));
    Ok(())
}

#[test]
fn versioned_heads_forks_and_standalone_captures_share_the_same_contract()
-> Result<(), Box<dyn std::error::Error>> {
    let (model, key) = fixture()?;
    let mut versioned = VersionedModel::new(model)?;
    let head = versioned.snapshot()?;
    let mut fork = versioned.fork(&head)?;
    fork.create("n", &key, "memory:one")?;
    let incomplete = fork.snapshot()?;
    assert!(incomplete.validate().is_err());
    assert!(versioned.fork(&incomplete).is_err());
    assert!(matches!(
        &ModelDiff::between(&head, &incomplete)?.objects()[0],
        ObjectChange::Created { .. }
    ));
    fork.set("n", "name", json!("Complete"))?;
    let complete = fork.snapshot()?;
    let stale = versioned.fork(&head)?;
    versioned.commit(fork)?;
    assert!(versioned.commit(stale).is_err());
    let published = versioned.snapshot()?;
    assert_ne!(complete.version(), published.version());
    assert!(ModelDiff::between(&complete, &published)?.is_empty());
    assert!(head.get("n")?.is_none());
    Ok(())
}

#[test]
fn altered_checksums_formats_fields_and_limits_are_refused()
-> Result<(), Box<dyn std::error::Error>> {
    let (mut model, key) = fixture()?;
    model.create("n", &key, "memory:one")?;
    let snap = model.snapshot()?;
    let bytes = snap.to_bytes(SnapshotLimits::default())?;
    for case in 0..5 {
        let mut wire: Value = serde_json::from_slice(&bytes)?;
        match case {
            0 => wire["format"] = json!("future"),
            1 => wire["identity"]["content"] = json!("changed"),
            2 => wire["model"]["elements"][0]["ignored_future_data"] = json!(true),
            3 => wire["identity"]["schema"] = json!("changed"),
            _ => wire["extra"] = json!(true),
        }
        assert!(
            ModelSnapshot::from_bytes(
                snap.schema(),
                &serde_json::to_vec(&wire)?,
                SnapshotLimits::default()
            )
            .is_err(),
            "Case {case}"
        );
    }
    let small = SnapshotLimits {
        max_bytes: bytes.len() - 1,
        ..SnapshotLimits::default()
    };
    assert!(matches!(
        snap.to_bytes(small),
        Err(SnapshotError::LimitExceeded)
    ));
    assert!(matches!(
        ModelSnapshot::from_bytes(snap.schema(), &bytes, small),
        Err(SnapshotError::LimitExceeded)
    ));
    let no_objects = SnapshotLimits {
        max_objects: 0,
        ..SnapshotLimits::default()
    };
    assert!(matches!(
        ModelSnapshot::from_bytes(snap.schema(), &bytes, no_objects),
        Err(SnapshotError::LimitExceeded)
    ));
    assert!(matches!(
        snap.to_bytes(no_objects),
        Err(SnapshotError::LimitExceeded)
    ));
    Ok(())
}

#[test]
fn valid_checksums_do_not_authorize_invalid_graphs() -> Result<(), Box<dyn std::error::Error>> {
    let (mut model, key) = fixture()?;
    model.create("n", &key, "memory:one")?;
    let snap = model.snapshot()?;
    let bytes = snap.to_bytes(SnapshotLimits::default())?;
    for case in 0..4 {
        let mut wire: Value = serde_json::from_slice(&bytes)?;
        match case {
            0 => wire["model"]["elements"][0]["values"]["links"] = json!(["absent"]),
            1 => wire["model"]["elements"][0]["values"]["name"] = json!(true),
            2 => {
                let e = wire["model"]["elements"][0].clone();
                wire["model"]["elements"].as_array_mut().unwrap().push(e);
            }
            _ => wire["model"]["resources"]["memory:one"]["roots"] = json!(["absent"]),
        }
        rewrite_content(&mut wire);
        assert!(
            ModelSnapshot::from_bytes(
                snap.schema(),
                &serde_json::to_vec(&wire)?,
                SnapshotLimits::default()
            )
            .is_err(),
            "Case {case}"
        );
    }
    Ok(())
}

#[test]
fn missing_null_and_row_order_are_observable() -> Result<(), Box<dyn std::error::Error>> {
    assert_ne!(
        serde_json::to_value(StoredValue::Missing)?,
        serde_json::to_value(StoredValue::Present(Value::Null))?
    );
    let (mut model, key) = fixture()?;
    for id in ["b", "a"] {
        model.create(id, &key, "memory:one")?;
    }
    let a = model.snapshot()?;
    let mut wire: Value = serde_json::from_slice(&a.to_bytes(SnapshotLimits::default())?)?;
    wire["model"]["elements"].as_array_mut().unwrap().reverse();
    rewrite_content(&mut wire);
    let b = ModelSnapshot::from_bytes(
        a.schema(),
        &serde_json::to_vec(&wire)?,
        SnapshotLimits::default(),
    )?;
    let diff = ModelDiff::between(&a, &b)?;
    assert!(!diff.is_empty());
    assert!(diff.objects().is_empty());
    assert!(diff.resources().is_empty());
    assert_eq!(diff.object_order_before(), ["b", "a"]);
    assert_eq!(diff.object_order_after(), ["a", "b"]);
    Ok(())
}
