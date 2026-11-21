//! Opt-in, checked dependency context prepared outside the Cargo build lock.
use super::{canonical, crate_inputs, env, frame};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

const CONTEXT: &str = "INTERSECT_COMPILER_CONTEXT";

/// Emit an identity over owned inputs and the resolved normal/build closure.
///
/// Invoke Cargo through `tools/compiler_build.py` from this package's repository.
/// The wrapper prepares fresh locked metadata with the same target/features and
/// supplies it only to that child build. Direct Cargo invocation without a fresh
/// context fails closed. Existing [`crate::emit`] callers keep their policy.
pub fn emit_scoped(variable: &str) -> Result<(), String> {
    println!("cargo:rerun-if-env-changed={CONTEXT}");
    let record = PathBuf::from(env(CONTEXT).map_err(|_| {
        "scoped identity requires a fresh context: run tools/compiler_build.py -- <cargo command>"
            .to_owned()
    })?);
    let context: Value = serde_json::from_slice(
        &std::fs::read(&record).map_err(|e| format!("compiler context: {e}"))?,
    )
    .map_err(|e| format!("compiler context: {e}"))?;
    println!("cargo:rerun-if-changed={}", record.display());
    validate(&context)?;
    let manifest = canonical(&PathBuf::from(env("CARGO_MANIFEST_DIR")?).join("Cargo.toml"))?;
    let mut features: Vec<_> = std::env::vars()
        .filter_map(|(key, _)| key.strip_prefix("CARGO_FEATURE_").map(str::to_owned))
        .collect();
    features.sort();
    let identity = identity(&context, &manifest, &features)?;
    println!("cargo:rustc-env={variable}={identity}");
    Ok(())
}

fn field<'a>(value: &'a Value, key: &str) -> Result<&'a Value, String> {
    value
        .get(key)
        .ok_or_else(|| format!("compiler context missing {key}"))
}
fn string<'a>(value: &'a Value, key: &str) -> Result<&'a str, String> {
    field(value, key)?
        .as_str()
        .ok_or_else(|| format!("compiler context {key} is not a string"))
}
fn array<'a>(value: &'a Value, key: &str) -> Result<&'a [Value], String> {
    field(value, key)?
        .as_array()
        .map(Vec::as_slice)
        .ok_or_else(|| format!("compiler context {key} is not an array"))
}

fn validate(context: &Value) -> Result<(), String> {
    if field(context, "version")? != 1 {
        return Err("unsupported compiler context version".into());
    }
    if string(context, "profile")? != "native-wasm-all-target-dependencies-v1" {
        return Err("unsupported scoped compiler portability profile".into());
    }
    if string(context, "target")? != env("TARGET")? || string(context, "host")? != env("HOST")? {
        return Err("compiler context target/host is stale".into());
    }
    if ![string(context, "host")?, "wasm32-unknown-unknown"].contains(&string(context, "target")?) {
        return Err("compiler context target is outside the admitted portability profile".into());
    }
    let compiler = std::process::Command::new(env("RUSTC")?)
        .arg("-vV")
        .output()
        .map_err(|e| format!("check compiler context toolchain: {e}"))?;
    if !compiler.status.success() || string(context, "rustc")?.as_bytes() != compiler.stdout {
        return Err("compiler context toolchain is stale".into());
    }
    let actual_flags = std::env::var("CARGO_ENCODED_RUSTFLAGS").unwrap_or_default();
    if string(context, "rustflags")? != actual_flags {
        return Err("compiler context rustflags are stale".into());
    }
    validate_guards(context)
}

fn validate_guards(context: &Value) -> Result<(), String> {
    let guards = array(context, "guards")?;
    if guards.is_empty() {
        return Err("compiler context has no freshness guards".into());
    }
    let mut guarded = BTreeSet::new();
    for guard in guards {
        let path = Path::new(string(guard, "path")?);
        if !guarded.insert(path)
            || !matches!(field(guard, "sha256")?, Value::Null | Value::String(_))
        {
            return Err("invalid or duplicate compiler context freshness guard".into());
        }
        println!("cargo:rerun-if-changed={}", path.display());
        let bytes = if path.is_file() {
            Some(std::fs::read(path).map_err(|e| format!("compiler context guard: {e}"))?)
        } else {
            None
        };
        let actual = bytes.map(|b| format!("{:x}", Sha256::digest(b)));
        if guard.get("sha256").and_then(Value::as_str) != actual.as_deref() {
            return Err(format!("stale compiler context: {}", path.display()));
        }
    }
    let metadata = field(context, "metadata")?;
    let root = Path::new(string(metadata, "workspace_root")?);
    for path in array(metadata, "packages")?
        .iter()
        .map(|p| string(p, "manifest_path").map(PathBuf::from))
        .chain([Ok(root.join("Cargo.toml")), Ok(root.join("Cargo.lock"))])
    {
        if !guarded.contains(path?.as_path()) {
            return Err("compiler context is missing a required manifest/lock guard".into());
        }
    }
    Ok(())
}

fn key(package: &Value) -> Result<String, String> {
    let source = match package.get("source") {
        Some(Value::String(source)) if source.starts_with("registry+") => source.as_str(),
        Some(Value::String(source))
            if source.starts_with("git+")
                && source.rsplit_once('#').is_some_and(|(_, rev)| {
                    rev.len() == 40 && rev.bytes().all(|b| b.is_ascii_hexdigit())
                }) =>
        {
            "owned-source"
        }
        Some(Value::Null) => "owned-source",
        _ => return Err("compiler context has an unpinned or unsupported package source".into()),
    };
    Ok(format!(
        "{}@{}:{source}",
        string(package, "name")?,
        string(package, "version")?
    ))
}

fn identity(
    context: &Value,
    manifest: &Path,
    actual_features: &[String],
) -> Result<String, String> {
    let metadata = field(context, "metadata")?;
    let mut packages = BTreeMap::new();
    for package in array(metadata, "packages")? {
        if packages.insert(string(package, "id")?, package).is_some() {
            return Err("duplicate package in compiler context".into());
        }
    }
    let mut nodes = BTreeMap::new();
    for node in array(field(metadata, "resolve")?, "nodes")? {
        if nodes.insert(string(node, "id")?, node).is_some() {
            return Err("duplicate resolved node in compiler context".into());
        }
    }
    let owners: Vec<_> = packages
        .iter()
        .filter_map(|(id, p)| {
            let path = string(p, "manifest_path").ok()?;
            (canonical(Path::new(path)).ok().as_deref() == Some(manifest)).then_some(*id)
        })
        .collect();
    let [owner] = owners.as_slice() else {
        return Err("compiler context has no unique owning package".into());
    };
    let owner_node = nodes
        .get(owner)
        .ok_or("compiler context has no owning resolution")?;
    let mut expected_features: Vec<_> = array(owner_node, "features")?
        .iter()
        .map(|v| {
            v.as_str()
                .ok_or("invalid feature in compiler context")
                .map(|s| s.replace('-', "_").to_ascii_uppercase())
        })
        .collect::<Result<_, _>>()?;
    expected_features.sort();
    expected_features.dedup();
    if expected_features != actual_features {
        return Err("compiler context owning features differ from the actual Cargo unit".into());
    }
    let mut pending = vec![*owner];
    let mut visited = BTreeSet::new();
    let mut entries = BTreeMap::new();
    while let Some(id) = pending.pop() {
        if !visited.insert(id) {
            continue;
        }
        let package = packages
            .get(id)
            .ok_or("resolved dependency package is absent")?;
        let node = nodes.get(id).ok_or("resolved dependency node is absent")?;
        let name = key(package)?;
        let mut item = Sha256::new();
        frame(&mut item, b"package", name.as_bytes());
        if string(package, "source").is_ok_and(|s| s.starts_with("registry+")) {
            let checksum = field(field(context, "checksums")?, &name)?
                .as_str()
                .filter(|s| s.len() == 64 && s.bytes().all(|b| b.is_ascii_hexdigit()))
                .ok_or("compiler context is missing a registry checksum")?;
            frame(&mut item, b"checksum", checksum.as_bytes());
        }
        let mut features = array(node, "features")?
            .iter()
            .map(|v| v.as_str().ok_or("invalid dependency feature"))
            .collect::<Result<Vec<_>, _>>()?;
        features.sort();
        for feature in features {
            frame(&mut item, b"feature", feature.as_bytes());
        }
        // A Git repository revision can change for an unrelated sibling package.
        // Hash this resolved package's owned inputs for Git and path packages;
        // registry packages have immutable per-package checksums instead.
        if !string(package, "source").is_ok_and(|s| s.starts_with("registry+")) {
            let path = Path::new(string(package, "manifest_path")?);
            let dir = canonical(path.parent().ok_or("package manifest has no parent")?)?;
            crate_inputs(&mut item, &dir, &dir)?;
        }
        let mut edges = BTreeSet::new();
        for dependency in array(node, "deps")? {
            let target = string(dependency, "pkg")?;
            for kind in array(dependency, "dep_kinds")? {
                let kind_name = match field(kind, "kind")? {
                    Value::Null => "normal",
                    Value::String(s) if s == "build" => "build",
                    Value::String(s) if s == "dev" => continue,
                    _ => return Err("unknown dependency kind in compiler context".into()),
                };
                let target_package = packages.get(target).ok_or("dependency target is absent")?;
                edges.insert(format!(
                    "{}:{kind_name}:{}:{}",
                    string(dependency, "name")?,
                    field(kind, "target")?,
                    key(target_package)?
                ));
                pending.push(target);
            }
        }
        for edge in edges {
            frame(&mut item, b"dependency", edge.as_bytes());
        }
        let value = format!("{:x}", item.finalize());
        if entries.insert(name, value).is_some() {
            return Err("ambiguous portable package identity in compiler context".into());
        }
    }
    let mut digest = Sha256::new();
    frame(
        &mut digest,
        b"policy",
        b"intersect.scoped-source-identity.v1",
    );
    frame(
        &mut digest,
        b"portability",
        b"native-wasm-all-target-dependencies-v1",
    );
    frame(&mut digest, b"scanner", include_bytes!("lib.rs"));
    frame(&mut digest, b"context-scanner", include_bytes!("scoped.rs"));
    frame(
        &mut digest,
        b"preparation",
        include_bytes!("../../../tools/compiler_build.py"),
    );
    frame(&mut digest, b"rustc", string(context, "rustc")?.as_bytes());
    frame(
        &mut digest,
        b"rustflags",
        string(context, "rustflags")?.as_bytes(),
    );
    for (package, value) in entries {
        frame(&mut digest, b"component", package.as_bytes());
        frame(&mut digest, b"identity", value.as_bytes());
    }
    Ok(format!("{:x}", digest.finalize()))
}

#[cfg(test)]
mod tests;
