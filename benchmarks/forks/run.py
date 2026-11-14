"""Fresh-process native fork retention measurements (Python 3.11+, Windows/Linux)."""
import argparse
import ctypes
import hashlib
import json
import os
from pathlib import Path
import platform
import queue
import statistics
import subprocess
import threading
import time

ROOT = Path(__file__).resolve().parents[2]


def memory(process):
    """OS RSS and private bytes, independent of Rust requested-allocation counts."""
    if os.name == "nt":
        from ctypes import wintypes

        class Counters(ctypes.Structure):
            _fields_ = [("cb", wintypes.DWORD), ("faults", wintypes.DWORD)] + [
                (name, ctypes.c_size_t) for name in
                ("peak", "rss", "pool_peak", "pool", "nonpaged_peak", "nonpaged",
                 "pagefile", "pagefile_peak", "private")
            ]

        counters = Counters()
        counters.cb = ctypes.sizeof(counters)
        api = ctypes.WinDLL("psapi", use_last_error=True).GetProcessMemoryInfo
        api.argtypes = [wintypes.HANDLE, ctypes.POINTER(Counters), wintypes.DWORD]
        api.restype = wintypes.BOOL
        if not api(wintypes.HANDLE(int(process._handle)), ctypes.byref(counters), counters.cb):
            raise ctypes.WinError(ctypes.get_last_error())
        return {"rss_bytes": counters.rss, "peak_rss_bytes": counters.peak,
                "private_bytes": counters.private,
                "private_metric": "Windows PROCESS_MEMORY_COUNTERS_EX.PrivateUsage (commit)"}
    if platform.system() == "Linux":
        fields = {}
        for line in Path(f"/proc/{process.pid}/status").read_text().splitlines():
            if line.startswith(("VmRSS:", "VmHWM:")):
                key, value, _ = line.split()
                fields[key] = int(value) * 1024
        result = {"rss_bytes": fields["VmRSS:"], "peak_rss_bytes": fields["VmHWM:"]}
        try:
            private = 0
            for line in Path(f"/proc/{process.pid}/smaps_rollup").read_text().splitlines():
                if line.startswith(("Private_Clean:", "Private_Dirty:", "Private_Hugetlb:")):
                    private += int(line.split()[1]) * 1024
            result.update(private_bytes=private,
                          private_metric="Linux smaps_rollup private resident pages")
        except OSError as error:
            result["private_error"] = str(error)
        return result
    return {"error": "OS memory sampling is implemented for Windows and Linux only"}


def measure(command, timeout, log_path):
    messages = queue.Queue()
    phases = []
    result = {"status": "process_error"}
    started = time.monotonic()
    with log_path.open("w", encoding="utf-8") as log:
        process = subprocess.Popen(command, cwd=ROOT, stdin=subprocess.PIPE,
                                   stdout=subprocess.PIPE, stderr=log, text=True, bufsize=1)

        def read():
            for line in process.stdout:
                messages.put(line.strip())
            messages.put(None)

        reader = threading.Thread(target=read, daemon=True)
        reader.start()
        deadline = started + timeout
        try:
            while True:
                line = messages.get(timeout=max(0.001, deadline - time.monotonic()))
                if line is None:
                    break
                item = json.loads(line)
                if item.get("event") == "phase":
                    try:
                        item["os_memory"] = memory(process)
                    except (OSError, KeyError, ValueError) as error:
                        item["os_memory"] = {"error": str(error)}
                    phases.append(item)
                if item.get("event") in ("start", "phase"):
                    process.stdin.write("\n")
                    process.stdin.flush()
                elif item.get("event") == "complete":
                    result = item
            code = process.wait(timeout=max(0.001, deadline - time.monotonic()))
            if code or result.get("status") != "ok":
                result = {"status": "process_error", "exit_code": code}
        except (queue.Empty, subprocess.TimeoutExpired):
            result = {"status": "timeout"}
        except (ValueError, OSError) as error:
            result = {"status": "runner_error", "error": str(error)}
        finally:
            if process.poll() is None:
                process.kill()
            process.wait()
            reader.join(timeout=2)
            process.stdin.close()
            process.stdout.close()
    return {**result, "phases": phases, "command": command, "log": str(log_path),
            "wall_seconds": time.monotonic() - started}


def version(command):
    try:
        result = subprocess.run(command, capture_output=True, text=True, timeout=30, cwd=ROOT)
        return (result.stdout + result.stderr).strip()
    except (OSError, subprocess.TimeoutExpired) as error:
        return str(error)


def provenance(native):
    paths = ["Cargo.toml", "Cargo.lock", "crates/intersect-model", "tests/model-acceptance",
             "benchmarks/forks/run.py", "benchmarks/forks/README.md", "docs/model-guide.md"]
    files = [ROOT / "Cargo.toml", ROOT / "Cargo.lock", Path(__file__)]
    for directory in ("crates/intersect-model", "tests/model-acceptance"):
        files += sorted((ROOT / directory).rglob("*.rs"))
        files.append(ROOT / directory / "Cargo.toml")
    diff = subprocess.run(["git", "diff", "--binary", "HEAD", "--", *paths],
                          cwd=ROOT, capture_output=True, timeout=30, check=True).stdout
    return {
        "platform": platform.platform(), "machine": platform.machine(),
        "processor": platform.processor(), "logical_cpus": os.cpu_count(),
        "python": platform.python_version(), "rust": version(["rustc", "-Vv"]),
        "git_revision": version(["git", "rev-parse", "HEAD"]),
        "relevant_git_status": version(["git", "status", "--short", "--", *paths]),
        "relevant_tracked_diff_sha256": hashlib.sha256(diff).hexdigest(),
        "sources": {str(path.relative_to(ROOT)): hashlib.sha256(path.read_bytes()).hexdigest()
                    for path in files},
        "native_binary_sha256": hashlib.sha256(native.read_bytes()).hexdigest(),
        "native_binary_mtime_ns": native.stat().st_mtime_ns,
        "measurement": "System requested live/phase peak bytes; paused phase OS RSS/private; fresh process per case",
    }


def report(rows):
    lines = ["# Native fork retention observations", "",
             "Fresh release processes; all requested forks remain alive together until teardown.",
             "Allocator counts include fork Vec capacity, metadata, maps, and changed values.",
             "These are requested allocation bytes, excluding allocator metadata/fragmentation and OS pages.",
             "OS RSS includes code, runtime, pages and allocator overhead; freed heap may remain resident.",
             "Phase peaks include transient allocations and verification; times cover the operation loop.", "",
             "| Case | Status | Forks | Empty growth MiB | Edited growth MiB | Edit increment B/fork | Empty ms | Edit ms | Final RSS MiB | Teardown residual B |",
             "| --- | --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |"]
    for name in dict.fromkeys(row["case"] for row in rows):
        group = [row for row in rows if row["case"] == name]
        good = [row for row in group if row["status"] == "ok"]

        def values(phase, key, os_key=False):
            return [entry["os_memory"][key] if os_key else entry[key]
                    for row in good for entry in row["phases"]
                    if entry["phase"] == phase and key in (entry.get("os_memory", {}) if os_key else entry)]

        def median(phase, key, divisor=1, os_key=False):
            data = values(phase, key, os_key)
            return f"{statistics.median(data) / divisor:.3f}" if data else "—"

        config = group[0]["config"]
        final_phase = "empty" if config["mode"] == "empty" else "edited"
        status = ", ".join(sorted({row["status"] for row in group}))
        columns = [name, status, f'{config["forks"]:,}',
                   median("empty", "allocator_live_delta_from_baseline_bytes", 2**20),
                   median("edited", "allocator_live_delta_from_baseline_bytes", 2**20),
                   median("edited", "allocator_live_delta_from_empty_bytes", config["forks"]),
                   median("empty", "elapsed_ms"), median("edited", "elapsed_ms"),
                   median(final_phase, "rss_bytes", 2**20, True),
                   median("forks_dropped", "allocator_live_delta_from_baseline_bytes")]
        lines.append("| " + " | ".join(columns) + " |")
    lines += ["", "Growth is measured relative to the shared baseline, which is reported separately in results.json.",
              "Teardown residual is live bytes after dropping the entire fork Vec minus baseline live bytes.",
              "Compare scalar-1k and scalar-16m to detect retention proportional to unchanged same-object payload.",
              "Compare text-128 and text-1024 to measure changed-payload growth, and empty-graph for base-size independence.",
              "Only successful rows support conclusions; timeouts and failures remain explicit.",
              "This qualifies the recorded sibling-fork workloads, not general graph edit throughput, browser limits,",
              "domain conformance, deep fork histories, or selective merges. See README.md and results.json.", ""]
    return "\n".join(lines)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    names = ("scalar-1k", "scalar-16m", "empty-graph", "text-128", "text-1024")
    parser.add_argument("--cases", nargs="+", choices=names, default=list(names))
    parser.add_argument("--forks", type=int, default=1_000_000)
    parser.add_argument("--text-forks", type=int, default=100_000)
    parser.add_argument("--graph-elements", type=int, default=10_000)
    parser.add_argument("--repeats", type=int, default=3)
    parser.add_argument("--timeout", type=float, default=300)
    parser.add_argument("--output", type=Path, default=ROOT / "target/fork-benchmark")
    args = parser.parse_args()
    if min(args.forks, args.text_forks, args.graph_elements, args.repeats) < 1 or args.timeout <= 0:
        parser.error("counts, repeats and timeout must be positive")
    native = ROOT / "target/release/examples" / ("fork_memory.exe" if os.name == "nt" else "fork_memory")
    if not native.exists():
        parser.error("build first: cargo build -p intersect-model --example fork_memory --release --locked")
    args.output = args.output.resolve()
    args.output.mkdir(parents=True, exist_ok=True)
    matrix = {
        "scalar-1k": dict(mode="scalar", forks=args.forks, elements=1, baseline_payload_bytes=1024, edit_bytes=128),
        "scalar-16m": dict(mode="scalar", forks=args.forks, elements=1, baseline_payload_bytes=16 * 2**20, edit_bytes=128),
        "empty-graph": dict(mode="empty", forks=args.forks, elements=args.graph_elements, baseline_payload_bytes=1024, edit_bytes=128),
        "text-128": dict(mode="text", forks=args.text_forks, elements=1, baseline_payload_bytes=1024, edit_bytes=128),
        "text-1024": dict(mode="text", forks=args.text_forks, elements=1, baseline_payload_bytes=1024, edit_bytes=1024),
    }
    metadata = {**provenance(native), "repeats": args.repeats, "timeout_seconds": args.timeout,
                "matrix": {name: matrix[name] for name in args.cases}}
    rows = []
    for repeat in range(args.repeats):
        cases = args.cases if repeat % 2 == 0 else list(reversed(args.cases))
        for name in cases:
            config = matrix[name]
            command = [str(native), "--handshake"]
            for key, value in config.items():
                command += ["--" + key.replace("_", "-"), str(value)]
            print(f'{name}: {config["forks"]:,} forks, run {repeat + 1}', flush=True)
            result = measure(command, args.timeout, args.output / f"{name}-{repeat}.log")
            rows.append({**result, "case": name, "repeat": repeat, "config": config})
            (args.output / "results.json").write_text(json.dumps({"metadata": metadata, "runs": rows}, indent=2), encoding="utf-8")
            (args.output / "REPORT.md").write_text(report(rows), encoding="utf-8")
    print(args.output / "REPORT.md")
    if any(row["status"] != "ok" for row in rows):
        raise SystemExit(1)


if __name__ == "__main__":
    main()
