"""Build identical model-edit harnesses on an archived revision and current source.

Python 3.11+, Git and Rust on PATH. Never changes the active checkout or Git index.
The output directory must be new. Sources, logs, binaries and provenance stay there.
Prefer a short output path such as target/me0 for Windows path-length limits.
"""
import argparse
from datetime import datetime, timezone
import difflib
import gzip
import hashlib
import io
import json
import os
from pathlib import Path
import shutil
import subprocess
import tarfile

ROOT = Path(__file__).resolve().parents[2]
HARNESS = Path("crates/intersect-model/examples/model_edit_metrics.rs")
HELPER = Path("crates/intersect-model/examples/model_edit_metrics/allocation.rs")
MANIFEST = Path("crates/intersect-model/Cargo.toml")
FEATURE = "model-edit-allocation-metrics"
BUILD_PATHS = ["Cargo.toml", "Cargo.lock", ".cargo", "rust-toolchain.toml", "crates", "fixtures", "tests/rust-bindings"]
SHARED_INPUTS = [HARNESS, HELPER, Path("Cargo.lock"), *(
    Path("fixtures/library") / ("library." + extension)
    for extension in ("ecore", "xtext", "rcore_behavior"))]


def command(args, cwd=ROOT):
    return subprocess.run(args, cwd=cwd, check=True, capture_output=True).stdout


def sha(data):
    return hashlib.sha256(data).hexdigest()


def archive(revision, destination):
    """Extract only regular repository files/directories inside a fresh directory."""
    destination.mkdir(parents=True)
    data = command(["git", "archive", "--format=tar", revision])
    with tarfile.open(fileobj=io.BytesIO(data)) as source:
        for member in source.getmembers():
            target = (destination / member.name).resolve()
            if not target.is_relative_to(destination.resolve()):
                raise ValueError(f"Archive path leaves destination: {member.name}")
            if member.isdir():
                target.mkdir(parents=True, exist_ok=True)
            elif member.isfile():
                target.parent.mkdir(parents=True, exist_ok=True)
                stream = source.extractfile(member)
                if stream is None:
                    raise ValueError(f"Missing archive content: {member.name}")
                with stream, target.open("wb") as output:
                    shutil.copyfileobj(stream, output)
            else:
                raise ValueError(f"Unsupported archive entry: {member.name}")
    return sha(data)


def source_manifest(directory):
    files = {}
    for name in BUILD_PATHS:
        path = directory / name
        entries = sorted(path.rglob("*")) if path.is_dir() else [path]
        for item in entries:
            if item.is_file():
                files[item.relative_to(directory).as_posix()] = sha(item.read_bytes())
    return {"algorithm": "sha256 of sorted path-to-sha256 JSON", "files": files,
            "source_digest": sha(json.dumps(files, sort_keys=True).encode())}


def current_inputs():
    """Read current inputs once, including staged additions and untracked files."""
    names = command(["git", "ls-files", "-z", "--cached", "--others", "--exclude-standard"]).decode().split("\0")
    files = {}
    for name in sorted(set(filter(None, names))):
        if not any(name == prefix or name.startswith(prefix + "/") for prefix in BUILD_PATHS):
            continue
        source = ROOT / name
        if source.is_symlink():
            raise ValueError(f"Unsupported current-source symlink: {name}")
        if source.is_file():
            files[name] = source.read_bytes()
    return files


def overlay_current(destination, files):
    """Make archived build inputs match the captured working tree, including deletes."""
    for name in source_manifest(destination)["files"]:
        if name not in files:
            target = (destination / name).resolve()
            if not target.is_relative_to(destination.resolve()):
                raise ValueError(f"Snapshot removal leaves destination: {name}")
            target.unlink()  # Only a file in this newly created isolated snapshot.
    for name, data in files.items():
        target = (destination / name).resolve()
        if not target.is_relative_to(destination.resolve()):
            raise ValueError(f"Current-source path leaves destination: {name}")
        if target.is_dir():
            # A tracked directory can become a file; its old files were removed
            # above. rmdir refuses nonempty directories instead of deleting them.
            for child in sorted(target.rglob("*"), key=lambda p: len(p.parts), reverse=True):
                if child.is_dir():
                    child.rmdir()
            target.rmdir()
        if not target.exists():
            target.parent.mkdir(parents=True, exist_ok=True)
        target.write_bytes(data)


def archive_inputs(directory, destination, identity):
    """Save exact measured bytes, checking the source manifest before archiving."""
    if source_manifest(directory) != identity:
        raise ValueError(f"Source snapshot changed before archive: {directory}")
    with destination.open("xb") as output:
        with gzip.GzipFile(filename="", mode="wb", fileobj=output, mtime=0) as compressed:
            with tarfile.open(fileobj=compressed, mode="w") as archive_file:
                for name, digest in sorted(identity["files"].items()):
                    data = (directory / name).read_bytes()
                    if sha(data) != digest:
                        raise ValueError(f"Source snapshot changed during archive: {name}")
                    info = tarfile.TarInfo(name)
                    info.size = len(data)
                    info.mode = 0o644
                    archive_file.addfile(info, io.BytesIO(data))
    return {"path": str(destination), "sha256": sha(destination.read_bytes()),
            "contents": "Exact build inputs listed in the candidate source manifest"}


def shared_input_hashes(directory):
    return {path.as_posix(): sha((directory / path).read_bytes()) for path in SHARED_INPUTS}


def source_patch(revision):
    patch = command(["git", "diff", "--binary", revision, "--", *BUILD_PATHS])
    names = command(["git", "ls-files", "-z", "--others", "--exclude-standard", "--", *BUILD_PATHS]).decode().split("\0")
    for name in sorted(filter(None, names)):
        content = (ROOT / name).read_text(encoding="utf-8").splitlines(keepends=True)
        diff = "".join(difflib.unified_diff([], content, fromfile="/dev/null", tofile="b/" + name))
        patch += (f"diff --git a/{name} b/{name}\nnew file mode 100644\n" + diff).encode()
    return patch


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--baseline", default="d99379d37d1c2fa28e8dc1284c6ce94347a4feef")
    parser.add_argument("--label", default="incremental-validation")
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--prepare-only", action="store_true")
    args = parser.parse_args()
    if args.label == "original" or not args.label or any(c not in "abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789_-" for c in args.label):
        parser.error("label must be alphanumeric/dash/underscore and different from original")
    output = args.output.resolve()
    if output.exists():
        parser.error("output exists; use a new directory to preserve prior sources and results")
    baseline = command(["git", "rev-parse", args.baseline + "^{commit}"]).decode().strip()
    head = command(["git", "rev-parse", "HEAD"]).decode().strip()
    captured_inputs = current_inputs()
    harness_bytes = {path: captured_inputs[path.as_posix()] for path in (HARNESS, HELPER)}
    output.mkdir(parents=True)
    binary_dir = output / "bin"
    binary_dir.mkdir()
    provenance = {"format_version": 1, "created_utc": datetime.now(timezone.utc).isoformat(),
                  "baseline_revision": baseline, "current_head": head,
                  "rustc": command(["rustc", "-Vv"]).decode().strip(),
                  "cargo": command(["cargo", "-V"]).decode().strip(),
                  "builder_sha256": sha(Path(__file__).read_bytes()),
                  "environment": {key: os.environ.get(key) for key in
                     ("RUSTFLAGS", "CARGO_BUILD_RUSTFLAGS", "CARGO_PROFILE_RELEASE_DEBUG",
                      "CARGO_PROFILE_RELEASE_LTO", "CARGO_PROFILE_RELEASE_CODEGEN_UNITS",
                      "CARGO_PROFILE_RELEASE_OPT_LEVEL", "CARGO_PROFILE_RELEASE_PANIC",
                      "CARGO_PROFILE_RELEASE_DEBUG_ASSERTIONS", "CARGO_PROFILE_RELEASE_OVERFLOW_CHECKS")},
                  "harness_sha256": {path.as_posix(): sha(data) for path, data in harness_bytes.items()},
                  "candidates": {}}
    patch = source_patch(baseline)
    (output / "current-source.patch").write_bytes(patch)
    provenance["current_source_patch_sha256"] = sha(patch)
    for index, (label, revision) in enumerate((("original", baseline), (args.label, head))):
        source = output / "s" / str(index)
        archive_hash = archive(revision, source)
        if label != "original":
            overlay_current(source, captured_inputs)
        # Identical host-only harness on both source versions.
        for path, data in harness_bytes.items():
            (source / path).parent.mkdir(parents=True, exist_ok=True)
            (source / path).write_bytes(data)
        manifest = source / MANIFEST
        manifest_text = manifest.read_text(encoding="utf-8")
        if FEATURE not in manifest_text:
            if "[features]" in manifest_text:
                manifest_text = manifest_text.replace("[features]", f"[features]\n{FEATURE} = []", 1)
            else:
                manifest_text += f"\n[features]\n# Host benchmark only; unused by library code.\n{FEATURE} = []\n"
            manifest.write_text(manifest_text, encoding="utf-8")
        identity = source_manifest(source)
        (output / f"{label}-source-manifest.json").write_text(json.dumps(identity, indent=2) + "\n", encoding="utf-8")
        candidate = {"revision": revision, "current_working_tree_overlay": label != "original",
                     "archive_sha256": archive_hash, "source_digest": identity["source_digest"],
                     "harness_sha256": {path.as_posix(): identity["files"][path.as_posix()]
                                        for path in (HARNESS, HELPER)},
                     "shared_inputs_sha256": shared_input_hashes(source),
                     "source_archive": archive_inputs(source, output / f"{label}-sources.tar.gz", identity),
                     "source_directory": str(source), "target_directory": str(output / "t" / str(index)),
                     "builds": {}}
        provenance["candidates"][label] = candidate
        (output / "provenance.json").write_text(json.dumps(provenance, indent=2) + "\n", encoding="utf-8")
    # Both source versions are frozen before the first compilation. The original
    # and current semantics may differ, but their fixture/lock/harness must not.
    original_shared = provenance["candidates"]["original"]["shared_inputs_sha256"]
    if provenance["candidates"][args.label]["shared_inputs_sha256"] != original_shared:
        raise ValueError("Compared snapshots differ in harness, fixture, or Cargo.lock bytes")
    provenance["shared_inputs_verified"] = True
    (output / "provenance.json").write_text(json.dumps(provenance, indent=2) + "\n", encoding="utf-8")
    if args.prepare_only:
        print(output / "provenance.json")
        return
    for label, candidate in provenance["candidates"].items():
        source = Path(candidate["source_directory"])
        for mode in ("timing", "allocation"):
            target = Path(candidate["target_directory"])
            build = ["cargo", "build", "--release", "--locked", "-p", "intersect-model",
                     "--example", "model_edit_metrics", "--target-dir", str(target)]
            if mode == "allocation":
                build += ["--features", FEATURE]
            print(f"Building {label} ({mode})", flush=True)
            with (output / f"{label}-{mode}-build.log").open("wb") as log:
                subprocess.run(build, cwd=source, stdout=log, stderr=subprocess.STDOUT, check=True)
            suffix = ".exe" if os.name == "nt" else ""
            binary = binary_dir / f"{label}-{mode}{suffix}"
            shutil.copyfile(target / "release/examples" / f"model_edit_metrics{suffix}", binary)
            candidate["builds"][mode] = {"command": build, "executable": str(binary), "binary_sha256": sha(binary.read_bytes())}
            (output / "provenance.json").write_text(json.dumps(provenance, indent=2) + "\n", encoding="utf-8")
    print(output / "provenance.json")


if __name__ == "__main__":
    main()
