//! Handwritten host-side snapshot harness. The library itself performs no I/O.
use intersect_model::{
    api::{ModelDiff, ModelEdit, ModelSnapshot, ObjectChange, SnapshotLimits, StoredValue},
    compile_rcore,
};
use serde_json::{Value, json};
use std::{error::Error, fs, path::Path};

const SOURCE: &str = include_str!("../assets/fixtures/snapshots/node.rcore");

fn verify(directory: &Path) -> Result<Value, Box<dyn Error>> {
    let package = compile_rcore(SOURCE)?;
    let before_bytes = fs::read(directory.join("before.json"))?;
    let after_bytes = fs::read(directory.join("after.json"))?;
    let before =
        ModelSnapshot::from_bytes(package.schema(), &before_bytes, SnapshotLimits::default())?;
    let after =
        ModelSnapshot::from_bytes(package.schema(), &after_bytes, SnapshotLimits::default())?;
    if before.validate().is_ok() {
        return Err("Expected an incomplete baseline".into());
    }
    after.validate()?;
    let diff = ModelDiff::between(&before, &after)?;
    let expected: Value = serde_json::from_slice(&fs::read(directory.join("diff.json"))?)?;
    if serde_json::to_value(&diff)? != expected {
        return Err("Diff differs after fresh-process restoration".into());
    }
    let features = diff
        .objects()
        .iter()
        .find_map(|c| match c {
            ObjectChange::Updated {
                before, features, ..
            } if before.id == "team-1" => Some(features),
            _ => None,
        })
        .ok_or("Expected team-1 update")?;
    if !features.iter().any(|f| {
        f.name == "name"
            && f.before.value == StoredValue::Present(Value::Null)
            && f.after.value == StoredValue::Present(json!("Équipe — Δ 日本語 🚀"))
    }) {
        return Err("Expected exact required-name change".into());
    }
    if !features
        .iter()
        .any(|f| f.name == "links" && f.after.value == StoredValue::Present(json!(["team-2"])))
    {
        return Err("Expected retained reference identity".into());
    }
    if !diff
        .objects()
        .iter()
        .any(|c| matches!(c,ObjectChange::Created { after } if after.id=="team-2"))
    {
        return Err("Expected independent created object".into());
    }
    if before.to_bytes(SnapshotLimits::default())? != before_bytes
        || after.to_bytes(SnapshotLimits::default())? != after_bytes
    {
        return Err("Snapshot byte identity differs on restoration".into());
    }
    Ok(
        json!({"passed":true,"before":before.identity()?,"after":after.identity()?,
        "objectChanges":diff.objects().len(),"resourceChanges":diff.resources().len(),
        "checks":["incomplete state retained","complete state validates","Unicode exact",
            "stable object/reference identities","typed feature changes","exact snapshot bytes","exact diff after restore"]}),
    )
}

fn main() -> Result<(), Box<dyn Error>> {
    let mut args = std::env::args().skip(1);
    let action = args
        .next()
        .ok_or("Use --write DIRECTORY or --verify DIRECTORY")?;
    let directory = args.next().ok_or("Specify a harness data directory")?;
    if args.next().is_some() {
        return Err("Unexpected argument".into());
    }
    let directory = Path::new(&directory);
    if action == "--write" {
        fs::create_dir_all(directory)?;
        let package = compile_rcore(SOURCE)?;
        let mut model = package.new_model()?;
        model.create("team-1", "Node", "model:snapshot-harness")?;
        let before = model.snapshot()?;
        let original = before.identity()?;
        model.set("team-1", "name", json!("Équipe — Δ 日本語 🚀"))?;
        model.create("team-2", "Node", "model:snapshot-harness")?;
        model.set("team-2", "name", json!("Related object"))?;
        model.set("team-1", "links", json!(["team-2"]))?;
        let after = model.snapshot()?;
        if before.identity()? != original {
            return Err("Earlier snapshot changed during edits".into());
        }
        fs::write(
            directory.join("before.json"),
            before.to_bytes(SnapshotLimits::default())?,
        )?;
        fs::write(
            directory.join("after.json"),
            after.to_bytes(SnapshotLimits::default())?,
        )?;
        fs::write(
            directory.join("diff.json"),
            serde_json::to_vec_pretty(&ModelDiff::between(&before, &after)?)?,
        )?;
    } else if action != "--verify" {
        return Err("Unknown action".into());
    }
    println!("{}", serde_json::to_string_pretty(&verify(directory)?)?);
    Ok(())
}
