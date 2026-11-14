//! Host-only source identity support for independently distributed packages.
//! Input bytes, enabled Cargo features and optional lock/manifest context are
//! framed deterministically. Literal embedded inputs are scanned recursively;
//! source trees of runtime dependencies are never located or scanned.
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

#[cfg(test)]
mod tests;

const DOMAIN: &[u8] = b"intersect.source-identity.v3";

/// Emit the owning package's source identity and exact Cargo rebuild inputs.
/// This build dependency never runs in the production library.
pub fn emit(variable: &str) -> Result<(), String> {
    let manifest = canonical(&PathBuf::from(env("CARGO_MANIFEST_DIR")?))?;
    let mut digest = Sha256::new();
    digest.update(DOMAIN);
    frame(&mut digest, b"package", env("CARGO_PKG_NAME")?.as_bytes());
    frame(
        &mut digest,
        b"version",
        env("CARGO_PKG_VERSION")?.as_bytes(),
    );
    let mut features: Vec<String> = std::env::vars()
        .filter_map(|(key, _)| key.strip_prefix("CARGO_FEATURE_").map(str::to_owned))
        .collect();
    features.sort();
    for feature in &features {
        frame(&mut digest, b"feature", feature.as_bytes());
    }
    frame(
        &mut digest,
        b"scanner-version",
        env!("CARGO_PKG_VERSION").as_bytes(),
    );
    frame(&mut digest, b"scanner-source", include_bytes!("lib.rs"));
    crate_inputs(&mut digest, &manifest, &manifest)?;
    // Cargo does not expose all unified dependency features to build scripts.
    // Available build-context manifests/locks conservatively invalidate caches.
    // Published packages without a lock still have a deterministic owned key.
    if let Some(context) = workspace_root(&manifest) {
        file(
            &mut digest,
            "context",
            "Cargo.toml",
            &context.join("Cargo.toml"),
        )?;
        file(
            &mut digest,
            "context",
            "Cargo.lock",
            &context.join("Cargo.lock"),
        )?;
    }
    println!("cargo:rustc-env={variable}={:x}", digest.finalize());
    Ok(())
}

/// Hash the owning crate: its manifest, build script, `src/` files and the
/// files its sources pull in from outside `src/`.
fn crate_inputs(digest: &mut Sha256, dir: &Path, workspace: &Path) -> Result<(), String> {
    let name = label(dir, workspace)?;
    frame(digest, b"crate", name.as_bytes());
    file(
        digest,
        "manifest",
        &format!("{name}/Cargo.toml"),
        &dir.join("Cargo.toml"),
    )?;
    let build = dir.join("build.rs");
    if build.is_file() {
        file(digest, "build", &format!("{name}/build.rs"), &build)?;
    }
    let src = canonical(&dir.join("src"))?;
    let mut sources = BTreeMap::new();
    collect(&src, workspace, &mut sources)?;
    let mut external = BTreeMap::new();
    for path in sources.values() {
        if path.extension().is_some_and(|extension| extension == "rs") {
            external_files(path, &src, workspace, &mut external)?;
        }
    }
    for (label, path) in &sources {
        file(digest, "src", label, path)?;
    }
    for (label, path) in &external {
        file(digest, "external", label, path)?;
    }
    println!("cargo:rerun-if-changed={}", src.display());
    Ok(())
}

fn env(name: &str) -> Result<String, String> {
    std::env::var(name).map_err(|error| format!("source identity: {name}: {error}"))
}

fn canonical(path: &Path) -> Result<PathBuf, String> {
    path.canonicalize()
        .map_err(|error| format!("source identity: {}: {error}", path.display()))
}

/// Optional lock context; no workspace or lock is required to build a package.
fn workspace_root(manifest: &Path) -> Option<PathBuf> {
    manifest
        .ancestors()
        .find(|dir| dir.join("Cargo.lock").is_file())
        .map(Path::to_path_buf)
}

/// A `/`-separated path of `path` relative to `base`.
fn label(path: &Path, base: &Path) -> Result<String, String> {
    let common = base
        .ancestors()
        .find(|ancestor| path.starts_with(ancestor))
        .ok_or_else(|| {
            format!(
                "source identity: {} and {} have no common root",
                path.display(),
                base.display()
            )
        })?;
    let mut relative = PathBuf::new();
    for _ in base
        .strip_prefix(common)
        .map_err(|error| error.to_string())?
        .components()
    {
        relative.push("..");
    }
    relative.push(
        path.strip_prefix(common)
            .map_err(|error| error.to_string())?,
    );
    let mut parts = Vec::new();
    for part in relative.components() {
        parts.push(
            part.as_os_str()
                .to_str()
                .ok_or_else(|| format!("source identity: non-UTF-8 path {}", path.display()))?,
        );
    }
    Ok(parts.join("/"))
}

fn collect(dir: &Path, base: &Path, files: &mut BTreeMap<String, PathBuf>) -> Result<(), String> {
    let entries = std::fs::read_dir(dir)
        .map_err(|error| format!("source identity: {}: {error}", dir.display()))?;
    for entry in entries {
        let path = entry
            .map_err(|error| format!("source identity: {}: {error}", dir.display()))?
            .path();
        if path.is_dir() {
            collect(&path, base, files)?;
        } else {
            files.insert(label(&path, base)?, path);
        }
    }
    Ok(())
}

/// Record every target of `path`'s literal `include_str!`/`include_bytes!`
/// calls and `#[path = "..."]` modules that lies outside `src`; targets inside
/// `src` are already hashed. Modules outside `src` are scanned in turn.
fn external_files(
    path: &Path,
    src: &Path,
    workspace: &Path,
    files: &mut BTreeMap<String, PathBuf>,
) -> Result<(), String> {
    let text = std::fs::read_to_string(path)
        .map_err(|error| format!("source identity: {}: {error}", path.display()))?;
    let dir = path.parent().unwrap_or(src);
    let mut modules = Vec::new();
    for marker in ["include_str!(", "include_bytes!(", "#[path"] {
        let mut rest = text.as_str();
        while let Some(at) = rest.find(marker) {
            rest = &rest[at + marker.len()..];
            let argument = if marker == "#[path" {
                rest.trim_start()
                    .strip_prefix('=')
                    .and_then(|value| string_literal(value.trim_start()))
            } else {
                string_literal(rest.trim_start())
            };
            let literal = argument.ok_or_else(|| {
                format!(
                    "source identity: {}: `{marker}` without a string literal",
                    path.display()
                )
            })?;
            let target = canonical(&dir.join(&literal))?;
            if target.starts_with(src) {
                continue;
            }
            let name = label(&target, workspace)?;
            if files.insert(name, target.clone()).is_none() && marker == "#[path" {
                modules.push(target);
            }
        }
    }
    for module in modules {
        external_files(&module, src, workspace, files)?;
    }
    Ok(())
}

/// The value of a leading `"..."` or `r#"..."#` literal (simple escapes only).
fn string_literal(text: &str) -> Option<String> {
    if let Some(raw) = text.strip_prefix('r') {
        let hashes = raw.len() - raw.trim_start_matches('#').len();
        let body = raw[hashes..].strip_prefix('"')?;
        let close = format!("\"{}", "#".repeat(hashes));
        return body.find(&close).map(|end| body[..end].to_owned());
    }
    let mut value = String::new();
    let mut chars = text.strip_prefix('"')?.chars();
    while let Some(c) = chars.next() {
        match c {
            '"' => return Some(value),
            '\\' => match chars.next()? {
                '\\' => value.push('\\'),
                '"' => value.push('"'),
                _ => return None,
            },
            c => value.push(c),
        }
    }
    None
}

fn frame(digest: &mut Sha256, tag: &[u8], bytes: &[u8]) {
    for part in [tag, bytes] {
        digest.update((part.len() as u64).to_le_bytes());
        digest.update(part);
    }
}

fn file(digest: &mut Sha256, role: &str, label: &str, path: &Path) -> Result<(), String> {
    let bytes = std::fs::read(path)
        .map_err(|error| format!("source identity: {}: {error}", path.display()))?;
    println!("cargo:rerun-if-changed={}", path.display());
    frame(digest, role.as_bytes(), label.as_bytes());
    frame(digest, b"bytes", &bytes);
    Ok(())
}
