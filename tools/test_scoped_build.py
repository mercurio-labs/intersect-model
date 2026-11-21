#!/usr/bin/env python3
"""Milestone-only actual Cargo proof of scoped cache context (not a mock build)."""
from __future__ import annotations
import importlib.util
import json
import os
from pathlib import Path
import re
import subprocess
import sys
import shutil

ROOT = Path(__file__).resolve().parents[1]
BASE = ROOT / "target/scoped-build-proof" / (sys.argv[1] if len(sys.argv) > 1 else "initial")
WRAPPER = ROOT / "tools/compiler_build.py"
RECEIPT = BASE / "receipt.json"


def main() -> int:
    BASE.mkdir(parents=True, exist_ok=True)
    fixture = BASE / "workspace"
    fixture.mkdir(exist_ok=True)
    if (fixture / "Cargo.toml").exists():
        raise ValueError("proof workspace already exists; preserve it and choose a fresh proof directory")
    def write(name: str, value: str) -> None:
        path = fixture / name
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(value, encoding="utf-8", newline="\n")
    write("Cargo.toml", '[workspace]\nmembers=["compiler","loader","views"]\nresolver="3"\n')
    write("compiler/Cargo.toml", f'''[package]
name="compiler"
version="1.0.0"
edition="2024"
[dependencies]
loader={{path="../loader"}}
[build-dependencies]
intersect-build-support={{path={json.dumps((ROOT / 'crates/intersect-build-support').as_posix())}}}
[features]
alternate=["loader/alternate"]
[lib]
name="compiler_portable"
crate-type=["cdylib"]
''')
    write("compiler/build.rs", 'fn main() -> Result<(), String> { intersect_build_support::emit_scoped("IDENTITY") }\n')
    write("compiler/src/main.rs", 'fn main() { println!("{}", env!("IDENTITY")); std::hint::black_box(loader::VALUE); }\n')
    write("compiler/src/lib.rs", 'static ID: &str = env!("IDENTITY");\n#[unsafe(no_mangle)] pub extern "C" fn identity() -> *const u8 { ID.as_ptr() }\n')
    write("loader/Cargo.toml", '[package]\nname="loader"\nversion="1.0.0"\nedition="2024"\n[features]\nalternate=[]\n')
    write("loader/src/lib.rs", 'pub const VALUE: &str = include_str!("../data.txt");\n')
    write("loader/data.txt", "original")
    write("views/Cargo.toml", '[package]\nname="views"\nversion="1.0.0"\nedition="2024"\n')
    write("views/src/lib.rs", 'pub const VALUE: u32 = 1;\n')
    receipt: dict = {"kind": "actual-cargo-scoped-build", "cases": []}
    env = dict(os.environ)
    def call(name: str, command: list[str], expected: int = 0, child_env=None) -> str:
        result = subprocess.run(command, cwd=fixture, env=child_env or env,
                                text=True, encoding="utf-8", errors="replace", capture_output=True)
        (BASE / f"{name}.log").write_text(result.stdout + result.stderr, encoding="utf-8")
        receipt["cases"].append({"name": name, "exit": result.returncode,
                                 "expected_success": expected == 0})
        RECEIPT.write_text(json.dumps(receipt, indent=2), encoding="utf-8")
        if (result.returncode == 0) != (expected == 0):
            raise ValueError(f"{name}: unexpected exit {result.returncode}; see {BASE / (name + '.log')}")
        return result.stdout + result.stderr
    call("lock", ["cargo", "generate-lockfile", "--offline"])
    args = ["run", "-p", "compiler", "--offline", "--locked", "--quiet"]
    def key(name: str, extra: list[str] | None = None) -> str:
        output = call(name, [sys.executable, "-B", str(WRAPPER), "--", *args, *(extra or [])])
        keys = re.findall(r"(?m)^[a-f0-9]{64}$", output)
        if len(keys) != 1:
            raise ValueError(f"{name}: expected one actual compiled identity")
        receipt["cases"][-1]["key"] = keys[0]
        return keys[0]
    try:
        original = key("baseline")
        write("views/src/lib.rs", 'pub const VALUE: u32 = 2;\n')
        assert key("view-source") == original
        write("Cargo.toml", (fixture / "Cargo.toml").read_text(encoding="utf-8") + "\n# unrelated workspace edit\n")
        assert key("workspace-manifest") == original
        write("views/Cargo.toml", (fixture / "views/Cargo.toml").read_text(encoding="utf-8").replace('1.0.0', '1.0.1'))
        call("unrelated-lock-update", ["cargo", "update", "--offline", "-p", "views"])
        assert key("unrelated-lock") == original
        write("loader/data.txt", "changed")
        assert key("embedded-input") != original
        write("loader/data.txt", "original")
        assert key("restored-input") == original
        assert key("relevant-features", ["--features", "compiler/alternate"]) != original
        assert key("short-features", ["-Fcompiler/alternate"]) != original
        # Real relocation of the standalone consumer; its dependency scanner
        # still refers to the same immutable build-support implementation.
        relocated = BASE / "relocated"
        shutil.copytree(fixture, relocated)
        moved = subprocess.run([sys.executable, "-B", str(WRAPPER), "--", *args],
                               cwd=relocated, env=env, capture_output=True, text=True, encoding="utf-8")
        (BASE / "relocation.log").write_text(moved.stdout + moved.stderr, encoding="utf-8")
        assert moved.returncode == 0 and original in moved.stdout
        receipt["cases"].append({"name":"relocation", "exit":moved.returncode, "same_key":True})
        wasm_args = ["build", "-p", "compiler", "--lib", "--target", "wasm32-unknown-unknown", "--locked", "--offline", "--quiet"]
        call("actual-wasm", [sys.executable, "-B", str(WRAPPER), "--", *wasm_args])
        target_dir = Path(env.get("CARGO_TARGET_DIR", fixture / "target"))
        wasm = target_dir / "wasm32-unknown-unknown/debug/compiler_portable.wasm"
        assert original.encode("ascii") in wasm.read_bytes(), "actual WASM must embed the same portable compiler key"
        call("actual-clippy", [sys.executable, "-B", str(WRAPPER), "--", "clippy", "-p", "compiler", "--all-targets", "--locked", "--offline", "--", "-D", "warnings"])
        spec = importlib.util.spec_from_file_location("compiler_build", WRAPPER)
        if spec is None or spec.loader is None:
            raise ValueError("cannot load context preparation")
        module = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(module)
        context = module.prepare(args, fixture)
        record = BASE / "stale-context.json"
        record.write_text(json.dumps(context), encoding="utf-8")
        write("Cargo.toml", (fixture / "Cargo.toml").read_text(encoding="utf-8") + "# stale\n")
        stale_env = dict(env, INTERSECT_COMPILER_CONTEXT=str(record))
        assert "stale compiler context" in call("stale-refused", ["cargo", *args], 1, stale_env)
        assert "requires a fresh context" in call("missing-refused", ["cargo", *args], 1)
        record.write_text(json.dumps(context), encoding="utf-8")
        assert "target/host is stale" in call("wrong-target-refused", ["cargo", *wasm_args], 1, stale_env)
        context = module.prepare(args, fixture)
        record.write_text(json.dumps(context), encoding="utf-8")
        assert "owning features differ" in call("wrong-features-refused", ["cargo", *args, "--features", "compiler/alternate"], 1, stale_env)
        context["rustc"] = "stale toolchain"
        record.write_text(json.dumps(context), encoding="utf-8")
        assert "toolchain is stale" in call("wrong-toolchain-refused", ["cargo", *args], 1, stale_env)
        context = module.prepare(args, fixture)
        context["guards"] = context["guards"][:1]
        record.write_text(json.dumps(context), encoding="utf-8")
        assert "missing a required manifest/lock guard" in call("incomplete-refused", ["cargo", *args], 1, stale_env)
        receipt["status"] = "passed"
    except Exception as error:
        receipt["status"] = "failed"
        receipt["error"] = str(error)
        raise
    finally:
        RECEIPT.write_text(json.dumps(receipt, indent=2), encoding="utf-8")
        print(RECEIPT)
    return 0

if __name__ == "__main__":
    raise SystemExit(main())
