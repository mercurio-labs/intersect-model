//! Runnable native M1 proof; all model semantics use the production core.
#[path = "support/component_fixture.rs"]
mod fixture;

use fixture::{Fixture, URI};
use intersect_model::storage::{ModelRead, ReferenceView, StorageResult};
use serde_json::{Value, json};
use std::{path::PathBuf, process::ExitCode, sync::Arc};

fn check(report: &mut Vec<Value>, name: &str, passed: bool) -> StorageResult<()> {
    report.push(json!({"check":name,"passed":passed}));
    println!("{} {name}", if passed { "PASS" } else { "FAIL" });
    if passed {
        Ok(())
    } else {
        Err(format!("Check failed: {name}"))
    }
}

fn run(report: &mut Vec<Value>) -> StorageResult<()> {
    let f = Fixture::new()?;
    let target = f.library.object_id("shared")?.to_owned();
    let mut first = f.project("shared")?;
    let second = f.project("second")?;
    check(
        report,
        "two projects share the same immutable component allocation",
        first.shares_component_with(&second, URI),
    )?;
    check(
        report,
        "schema and validation context is shared",
        Arc::ptr_eq(first.shared_schema(), second.shared_schema()),
    )?;
    check(
        report,
        "library records are absent from writable project storage",
        first.project_object_count()? == 1 && second.project_object_count()? == 1,
    )?;
    first.set("shared", "out", json!([target]))?;
    first.set("shared", "name", json!("Edited project node"))?;
    check(
        report,
        "project and component can have the same original element ID",
        first.value("shared", "name")? == Some(json!("Edited project node"))
            && first.value(&target, "name")? == Some(json!("Library definition")),
    )?;
    let mut handle = None;
    first.inspect("shared", &mut |object| {
        if let Some(ReferenceView::Resolved(target)) = object
            .value("out")
            .and_then(|v| v.collection())
            .and_then(|v| v.get(0))
            .and_then(|v| v.reference())
        {
            handle = Some(target);
        }
        Ok(())
    })?;
    let handle = handle.ok_or("Project reference did not resolve to a runtime handle")?;
    let mut navigated = false;
    first.inspect_handle(&handle, &mut |object| {
        navigated = object.id() == target
            && object.value("name").and_then(|v| v.as_str()) == Some("Library definition");
        Ok(())
    })?;
    check(
        report,
        "borrowed reference navigation reaches the read-only definition",
        navigated,
    )?;
    check(
        report,
        "original external identity remains available",
        first
            .component_target(&target)
            .is_some_and(|v| v.element_id == "shared" && v.component_uri == URI),
    )?;
    check(
        report,
        "inverse contributions belong only to their project",
        first.value(&target, "incoming")? == Some(json!(["shared"]))
            && second
                .value(&target, "incoming")?
                .is_none_or(|v| v == json!([]) || v.is_null()),
    )?;
    let before = first.get(&target)?;
    let rejection = first.set(&target, "name", json!("Forbidden"));
    check(
        report,
        "checked library mutation is rejected without changing either view",
        rejection.is_err()
            && first.get(&target)? == before
            && second.value(&target, "name")? == Some(json!("Library definition")),
    )?;
    let rollback: StorageResult<()> = first.transaction(|edit| {
        edit.set("shared", "out", json!([]))?;
        edit.set("shared", "name", json!("Temporary"))?;
        Err("demonstration rollback".into())
    });
    check(
        report,
        "failed transaction restores project values and inverse overlays",
        rollback.is_err()
            && first.value("shared", "name")? == Some(json!("Edited project node"))
            && first.value(&target, "incoming")? == Some(json!(["shared"])),
    )?;
    let mut fork = first.fork()?;
    fork.set("shared", "name", json!("Fork edit"))?;
    check(
        report,
        "fork edits are independent and retain shared component backing",
        fork.shares_component_with(&first, URI)
            && first.value("shared", "name")? == Some(json!("Edited project node"))
            && fork.value("shared", "name")? == Some(json!("Fork edit")),
    )?;
    check(
        report,
        "runtime handles cannot cross project or fork contexts",
        second.inspect_handle(&handle, &mut |_| Ok(())).is_err()
            && fork.inspect_handle(&handle, &mut |_| Ok(())).is_err(),
    )?;
    first.validate()?;
    second.validate()?;
    fork.validate()?;
    check(
        report,
        "all three mounted graphs pass canonical structural validation",
        true,
    )?;
    // Exercise type checking through the same facade, not a harness-only rule.
    first.create("wrong-type", &f.other, "memory:/project.nodes")?;
    check(
        report,
        "reference target type checking remains active",
        first.set("shared", "target", json!("wrong-type")).is_err(),
    )?;
    Ok(())
}

fn main() -> ExitCode {
    let mut args = std::env::args().skip(1);
    let out = match args.next().as_deref() {
        None => None,
        Some("--out") => match args.next() {
            Some(path) if args.next().is_none() => Some(PathBuf::from(path)),
            _ => {
                eprintln!("Usage: model_component_usage [--out report.json]");
                return ExitCode::FAILURE;
            }
        },
        _ => {
            eprintln!("Usage: model_component_usage [--out report.json]");
            return ExitCode::FAILURE;
        }
    };
    println!("Intersect Model component proof (M1; no Text or KPAR loading)");
    let mut checks = Vec::new();
    let result = run(&mut checks);
    let report = json!({"format":"intersect.component-proof.v1", "passed":result.is_ok(),
        "runtime":intersect_model::SOURCE_IDENTITY, "checks":checks,"error":result.as_ref().err()});
    if let Some(path) = out {
        let write = (|| -> Result<(), Box<dyn std::error::Error>> {
            if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::write(&path, serde_json::to_vec_pretty(&report)?)?;
            Ok(())
        })();
        if let Err(error) = write {
            eprintln!("Cannot write report: {error}");
            return ExitCode::FAILURE;
        }
        println!("Report: {}", path.display());
    }
    match result {
        Ok(()) => {
            println!("Model proof passed.");
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("Model proof failed: {error}");
            ExitCode::FAILURE
        }
    }
}
