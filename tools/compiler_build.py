#!/usr/bin/env python3
"""Prepare a fresh scoped compiler context, then run one bounded Cargo invocation.

Handwritten host tool. No metadata subprocess runs inside a build script. The
temporary context is passed only to this child and removed when it exits.
Usage: python tools/compiler_build.py [--report] -- test -p PACKAGE --locked --offline
"""
from __future__ import annotations

import hashlib
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import tomllib


def digest(path: Path) -> str | None:
    return hashlib.sha256(path.read_bytes()).hexdigest() if path.is_file() else None


def prepare(args: list[str], cwd: Path) -> dict:
    if not args or args[0] not in {"build", "check", "clippy", "doc", "test", "run"}:
        raise ValueError("expected build, check, clippy, doc, test or run")
    # Configuration overrides and custom wrappers can alter resolution or inject
    # cfgs outside this bounded preparation path; do not guess their effects.
    if any(a == "--config" or a.startswith("--config=") for a in args):
        raise ValueError("scoped builds do not support --config overrides")
    for name in ("RUSTC_WRAPPER", "RUSTC_WORKSPACE_WRAPPER", "RUSTFLAGS", "CARGO_ENCODED_RUSTFLAGS"):
        if os.environ.get(name):
            raise ValueError(f"scoped builds do not support custom {name}")
    cargo = os.environ.get("CARGO", "cargo")
    rustc = subprocess.check_output([os.environ.get("RUSTC", "rustc"), "-vV"], text=True, encoding="utf-8")
    host = next(line.removeprefix("host: ") for line in rustc.splitlines() if line.startswith("host: "))
    target = host
    metadata_args = [cargo, "metadata", "--format-version", "1", "--locked", "--offline"]
    taking = {"--manifest-path", "--features", "--target", "-F"}
    i = 1
    while i < len(args) and args[i] != "--":
        arg = args[i]
        if arg in taking:
            if i + 1 == len(args):
                raise ValueError(f"missing value for {arg}")
            value = args[i + 1]
            if arg == "--target":
                target = value
            else:
                metadata_args.extend(["--features" if arg == "-F" else arg, value])
            i += 2
            continue
        if arg in {"--all-features", "--no-default-features"}:
            metadata_args.append(arg)
        elif arg.startswith("-F"):
            metadata_args.extend(["--features", arg[2:].removeprefix("=")])
        elif any(arg.startswith(k + "=") for k in taking):
            k, value = arg.split("=", 1)
            if k == "--target":
                target = value
            else:
                metadata_args.append(arg)
        i += 1
    if target not in {host, "wasm32-unknown-unknown"}:
        raise ValueError("scoped compiler profile supports the native host and wasm32-unknown-unknown")
    # Language program data is portable between the native pack builder and WASM.
    # Hash the complete target-conditional dependency graph, including its cfg
    # expressions, for both. A target-specific dependency change still invalidates
    # the key; selecting another admitted implementation of the same portable
    # algorithm does not. Actual native/WASM pack equivalence is a separate gate.
    metadata = json.loads(subprocess.check_output(metadata_args, cwd=cwd))
    paths = {Path(p["manifest_path"]).resolve() for p in metadata["packages"]}
    root = Path(metadata["workspace_root"]).resolve()
    lock = tomllib.loads((root / "Cargo.lock").read_text(encoding="utf-8"))
    checksums = {f"{p['name']}@{p['version']}:{p['source']}": p['checksum']
                 for p in lock["package"] if "checksum" in p}
    paths.update((root / "Cargo.toml", root / "Cargo.lock"))
    # Include absent config files as guards as well: creating one also stales the
    # context. Cargo searches from the invocation directory, not each dependency.
    for directory in (cwd.resolve(), *cwd.resolve().parents):
        paths.update((directory / ".cargo/config", directory / ".cargo/config.toml"))
    cargo_home = Path(os.environ.get("CARGO_HOME", Path.home() / ".cargo")).resolve()
    paths.update((cargo_home / "config", cargo_home / "config.toml"))
    return {"version": 1, "profile": "native-wasm-all-target-dependencies-v1",
            "host": host, "target": target, "rustc": rustc,
            "rustflags": "", "metadata": metadata, "checksums": checksums,
            "guards": [{"path": str(p), "sha256": digest(p)} for p in sorted(paths)]}


def main() -> int:
    args = sys.argv[1:]
    report = bool(args and args[0] == "--report")
    if report:
        args.pop(0)
    if args and args[0] == "--":
        args.pop(0)
    cwd = Path.cwd()
    context = prepare(args, cwd)
    with tempfile.TemporaryDirectory(prefix="intersect-compiler-context-") as temporary:
        path = Path(temporary) / "context.json"
        path.write_text(json.dumps(context, sort_keys=True), encoding="utf-8")
        child_env = dict(os.environ, INTERSECT_COMPILER_CONTEXT=str(path))
        if report:
            if args[0] != "test":
                raise ValueError("--report requires a Cargo test invocation")
            command = [sys.executable, "-B", "tools/test_report.py", "run", "--", *args[1:]]
        else:
            command = [os.environ.get("CARGO", "cargo"), *args]
        return subprocess.call(command, cwd=cwd, env=child_env)


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except (ValueError, subprocess.CalledProcessError) as error:
        print(f"compiler context preparation refused: {error}", file=sys.stderr)
        raise SystemExit(2)
