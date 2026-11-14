"""Repeatable fresh-process model API measurements (Python 3.11+, Windows/Linux)."""
import argparse
import csv
import ctypes
from datetime import datetime, timezone
import hashlib
import io
import json
import math
import os
from pathlib import Path
import platform
import queue
import re
import statistics
import subprocess
import sys
import tempfile
import threading
import time

ROOT = Path(__file__).resolve().parents[2]
SCENARIOS = ("basic-create", "transaction-create", "basic-scalar-edit",
             "transaction-scalar-edit", "set-many", "relationship-edit", "rollback", "create-rollback", "validate")
PHASES = ("startup", "setup", "workload", "verified", "teardown")
CAPTURE_LIMIT = 32768


def process_sample(process):
    """Read a paused child's process memory, not retained model allocation size."""
    if os.name == "nt":
        from ctypes import wintypes

        class Counters(ctypes.Structure):
            _fields_ = [("cb", wintypes.DWORD), ("faults", wintypes.DWORD)] + [
                (name, ctypes.c_size_t) for name in
                ("peak", "rss", "pool_peak", "pool", "nonpaged_peak", "nonpaged",
                 "pagefile", "pagefile_peak", "private")]

        counters = Counters()
        counters.cb = ctypes.sizeof(counters)
        api = ctypes.WinDLL("psapi", use_last_error=True).GetProcessMemoryInfo
        api.argtypes = [wintypes.HANDLE, ctypes.POINTER(Counters), wintypes.DWORD]
        api.restype = wintypes.BOOL
        if not api(wintypes.HANDLE(int(process._handle)), ctypes.byref(counters), counters.cb):
            raise ctypes.WinError(ctypes.get_last_error())
        return {"rss_bytes": counters.rss, "peak_rss_bytes": counters.peak,
                "private_bytes": counters.private,
                "private_metric": "Windows PrivateUsage: private committed bytes"}
    if platform.system() == "Linux":
        fields = {}
        for line in Path(f"/proc/{process.pid}/status").read_text().splitlines():
            if line.startswith(("VmRSS:", "VmHWM:")):
                key, value, _ = line.split()
                fields[key] = int(value) * 1024
        result = {"rss_bytes": fields["VmRSS:"], "peak_rss_bytes": fields["VmHWM:"]}
        try:
            result["private_bytes"] = sum(
                int(line.split()[1]) * 1024
                for line in Path(f"/proc/{process.pid}/smaps_rollup").read_text().splitlines()
                if line.startswith(("Private_Clean:", "Private_Dirty:", "Private_Hugetlb:")))
            result["private_metric"] = "Linux smaps_rollup: private resident pages"
        except OSError as error:
            result["private_error"] = str(error)
        return result
    return {"error": "phase memory is implemented for Windows and Linux only"}


def child_cpu_before():
    if os.name == "posix":
        import resource
        usage = resource.getrusage(resource.RUSAGE_CHILDREN)
        return usage.ru_utime, usage.ru_stime
    return None


def child_cpu_after(process, before):
    if os.name == "nt":
        from ctypes import wintypes
        api = ctypes.WinDLL("kernel32", use_last_error=True).GetProcessTimes
        api.argtypes = [wintypes.HANDLE] + [ctypes.POINTER(wintypes.FILETIME)] * 4
        api.restype = wintypes.BOOL
        values = [wintypes.FILETIME() for _ in range(4)]
        if not api(wintypes.HANDLE(int(process._handle)), *(ctypes.byref(v) for v in values)):
            return {"error": str(ctypes.WinError(ctypes.get_last_error()))}
        ticks = lambda value: (value.dwHighDateTime << 32) + value.dwLowDateTime
        return {"user_seconds": ticks(values[3]) / 10_000_000,
                "system_seconds": ticks(values[2]) / 10_000_000,
                "method": "GetProcessTimes after exit"}
    if before is not None:
        after = child_cpu_before()
        return {"user_seconds": after[0] - before[0], "system_seconds": after[1] - before[1],
                "method": "RUSAGE_CHILDREN delta; one serial child, including shutdown"}
    return {"error": "process CPU unavailable"}


def parse_record(stdout, expected):
    """Reject timings that cannot be tied to the requested, verified workload."""
    record = json.loads(stdout)
    if not isinstance(record, dict):
        raise ValueError("result must be a JSON object")
    for key, value in {"format_version": 1, "workload_id": "model-edit-v1", **expected}.items():
        if type(record.get(key)) is not type(value) or record.get(key) != value:
            raise ValueError(f"result {key} does not match request: {record.get(key)!r}")
    if record.get("correct") is not True:
        raise ValueError("child correctness check did not pass")
    for key in ("schema_revision", "schema_digest", "checksum"):
        if not isinstance(record.get(key), str) or not record[key]:
            raise ValueError(f"missing {key}")
    for key in ("edit_operations", "workload_units"):
        if type(record.get(key)) is not int or record[key] < 0:
            raise ValueError(f"invalid {key}")
    phases = record.get("phases_ns")
    if not isinstance(phases, dict) or "operation" not in phases:
        raise ValueError("missing operation phase")
    if any(type(value) is not int or value < 0 for value in phases.values()):
        raise ValueError("phase timings must be nonnegative integer nanoseconds")
    latency = record.get("operation_latency_ns")
    if latency is not None:
        if not isinstance(latency, dict) or type(latency.get("samples")) is not int or latency["samples"] < 0:
            raise ValueError("invalid operation latency sample count")
        for key in ("p50", "p95", "max"):
            value = latency.get(key)
            nullable = latency["samples"] < 20 if key != "max" else latency["samples"] == 0
            if value is None and nullable:
                continue
            if type(value) is not int or value < 0:
                raise ValueError("invalid operation latency summary")
        values = [latency[key] for key in ("p50", "p95", "max") if latency[key] is not None]
        if values != sorted(values):
            raise ValueError("latency percentiles are unordered")
    instrumented = record.get("allocator_instrumented")
    snapshots = record.get("allocation_snapshots")
    if type(instrumented) is not bool or not isinstance(snapshots, dict) or set(snapshots) != set(PHASES):
        raise ValueError("missing allocation instrumentation mode or snapshots")
    for snapshot in snapshots.values():
        if not instrumented:
            if snapshot is not None:
                raise ValueError("uninstrumented result contains allocation counters")
        elif not isinstance(snapshot, dict) or any(type(snapshot.get(key)) is not int or snapshot[key] < 0
                                                 for key in ("live_bytes", "peak_bytes", "allocation_calls", "requested_bytes")):
            raise ValueError("invalid allocation snapshot")
    if instrumented:
        for before, after in zip(PHASES, PHASES[1:]):
            if any(snapshots[after][key] < snapshots[before][key]
                   for key in ("allocation_calls", "requested_bytes")):
                raise ValueError("allocation counters decreased")
    return record


def measure(command, expected, timeout, sample=process_sample):
    """Run one child; handshakes pause outside the child's timed regions."""
    started = time.monotonic()
    deadline = started + timeout
    events = queue.Queue()
    captured = {"stdout": bytearray(), "stderr": bytearray()}
    truncated = {"stdout": False, "stderr": False}
    phase_memory = {}
    result = {"status": "process_error"}
    before = child_cpu_before()
    try:
        process = subprocess.Popen(command, cwd=ROOT, stdin=subprocess.PIPE,
                                   stdout=subprocess.PIPE, stderr=subprocess.PIPE)
    except OSError as error:
        return {"status": "launch_error", "error": str(error), "command": command,
                "process_wall_seconds": time.monotonic() - started, "phase_memory": {}}

    def capture(name, chunk):
        remaining = CAPTURE_LIMIT - len(captured[name])
        captured[name].extend(chunk[:remaining])
        truncated[name] |= len(chunk) > remaining

    def read_stdout():
        while chunk := process.stdout.readline(CAPTURE_LIMIT + 1):
            capture("stdout", chunk)
            events.put(chunk)
        events.put(None)

    def read_stderr():
        while chunk := process.stderr.read(4096):
            capture("stderr", chunk)

    readers = [threading.Thread(target=read_stdout, daemon=True),
               threading.Thread(target=read_stderr, daemon=True)]
    for reader in readers:
        reader.start()
    record = None
    try:
        while True:
            line = events.get(timeout=max(0.001, deadline - time.monotonic()))
            if line is None:
                break
            if len(line) > CAPTURE_LIMIT:
                raise ValueError("stdout line exceeds capture limit")
            item = json.loads(line)
            if isinstance(item, dict) and item.get("event") == "ready":
                if record is not None:
                    raise ValueError("handshake after final result")
                phase = item.get("phase")
                if len(phase_memory) >= len(PHASES) or phase != PHASES[len(phase_memory)]:
                    raise ValueError(f"unexpected handshake phase: {phase!r}")
                try:
                    phase_memory[phase] = sample(process)
                except (OSError, KeyError, ValueError) as error:
                    phase_memory[phase] = {"error": str(error)}
                process.stdin.write(b"\n")
                process.stdin.flush()
            else:
                if record is not None:
                    raise ValueError("more than one final result")
                if tuple(phase_memory) != PHASES:
                    raise ValueError("result before final handshake")
                record = parse_record(line, expected)
        code = process.wait(timeout=max(0.001, deadline - time.monotonic()))
        if code != 0:
            result = {"status": "process_error", "exit_code": code}
        elif record is None or tuple(phase_memory) != PHASES:
            result = {"status": "protocol_error", "error": "missing result or handshake phase"}
        else:
            result = {"status": "ok", "result": record, "exit_code": code}
    except (queue.Empty, subprocess.TimeoutExpired):
        result = {"status": "timeout"}
    except (ValueError, OSError) as error:
        result = {"status": "protocol_error", "error": str(error)}
    finally:
        if process.poll() is None:
            process.kill()
        process.wait()
        cpu = child_cpu_after(process, before)
        for reader in readers:
            reader.join(timeout=2)
        process.stdin.close()
        process.stdout.close()
        process.stderr.close()
    result.update(command=command, phase_memory=phase_memory, process_cpu=cpu,
                  process_wall_seconds=time.monotonic() - started)
    if result["status"] != "ok" or captured["stderr"]:
        result["captured_output"] = {key: value.decode("utf-8", errors="replace")
                                     for key, value in captured.items()}
        result["output_truncated"] = truncated
    return result


def identity(record):
    return tuple(json.dumps(record.get(key), sort_keys=True) for key in
                 ("workload_id", "schema_revision", "schema_digest", "schema_digest_algorithm",
                  "scenario", "size", "edits", "pattern", "edit_operations", "workload_units",
                  "checksum", "checksum_algorithm", "explicit_assignments", "full_validation_calls",
                  "final_objects", "allocator_instrumented", "allocation_peak_scope"))


def stats(values):
    return {"count": len(values), "min": min(values), "median": statistics.median(values),
            "max": max(values)} if values else None



def phase_timings(record):
    phases = dict(record["phases_ns"])
    phases["total_workload"] = phases["operation"] + phases.get("transaction_boundary", 0)
    return phases


def allocation_metrics(record):
    if not record.get("allocator_instrumented"):
        return {}
    snapshots = record["allocation_snapshots"]
    metrics = {}
    for before, after in zip(PHASES, PHASES[1:]):
        for key in ("live_bytes", "allocation_calls", "requested_bytes"):
            metrics[f"{after}_{key}_delta"] = snapshots[after][key] - snapshots[before][key]
    metrics["workload_peak_bytes"] = snapshots["workload"]["peak_bytes"]
    metrics["workload_peak_above_setup_live_bytes"] = max(0, snapshots["workload"]["peak_bytes"] - snapshots["setup"]["live_bytes"])
    for unit in ("workload_units", "edit_operations"):
        if record[unit]:
            for key in ("allocation_calls", "requested_bytes"):
                metrics[f"workload_{key}_per_{unit}"] = metrics[f"workload_{key}_delta"] / record[unit]
    return metrics

def summarize(rows, repeats, candidates, provenance=None):
    """Partial successes remain visible; only complete matching groups get ratios."""
    provenance = provenance or {}
    groups = {}
    for row in rows:
        key = (row["candidate"], row["scenario"], row["size"], row["edits"], row["pattern"])
        groups.setdefault(key, []).append(row)
    summaries = []
    for key, group in groups.items():
        good = [row for row in group if row["status"] == "ok"]
        identities = {identity(row["result"]) for row in good}
        complete = (len(group) == repeats and len(good) == repeats and
                    {row["repeat"] for row in group} == set(range(repeats)) and len(identities) == 1)
        phase_names = sorted({phase for row in good for phase in phase_timings(row["result"])})
        summary = dict(zip(("candidate", "scenario", "size", "edits", "pattern"), key))
        supplied = provenance.get(key[0], {}).get("supplied_build_provenance") or {}
        summary["harness_sha256"] = supplied.get("harness_sha256")
        summary.update(expected_repeats=repeats, attempted_repeats=len(group), successful_repeats=len(good),
                       complete=complete, statuses=sorted({row["status"] for row in group}),
                       consistent_results=len(identities) <= 1,
                       phases_ns={phase: stats([phase_timings(row["result"])[phase] for row in good
                                                if phase in phase_timings(row["result"])])
                                  for phase in phase_names},
                       throughput_per_second={metric: stats([row["result"][count_key] * 1e9 /
                                                             phase_timings(row["result"])["total_workload"]
                                                             for row in good
                                                             if phase_timings(row["result"])["total_workload"] > 0])
                                              for metric, count_key in (("workload_units", "workload_units"),
                                                                        ("checked_api_calls", "edit_operations"))},
                       process_wall_seconds=stats([row["process_wall_seconds"] for row in good]),
                       process_cpu_seconds=stats([row["process_cpu"]["user_seconds"] +
                                                  row["process_cpu"]["system_seconds"] for row in good
                                                  if "user_seconds" in row.get("process_cpu", {})]),
                       phase_memory={phase: {metric: stats([row["phase_memory"][phase][metric]
                                                          for row in good
                                                          if metric in row.get("phase_memory", {}).get(phase, {})])
                                             for metric in ("rss_bytes", "peak_rss_bytes", "private_bytes")}
                                     for phase in PHASES},
                       per_run_latency_ns={metric: stats([row["result"]["operation_latency_ns"][metric]
                                                         for row in good if row["result"].get("operation_latency_ns")
                                                         and row["result"]["operation_latency_ns"].get(metric) is not None])
                                           for metric in ("p50", "p95", "max")},
                       comparison_status="incomplete", baseline_over_candidate={})
        allocations = [allocation_metrics(row["result"]) for row in good]
        summary["allocation_metrics"] = {key: stats([values[key] for values in allocations if key in values])
                                         for key in sorted({key for values in allocations for key in values})}
        if good:
            summary["allocator_instrumented"] = good[0]["result"]["allocator_instrumented"]
            summary["edit_operations"] = good[0]["result"]["edit_operations"]
            summary["workload_units"] = good[0]["result"]["workload_units"]
            summary["result_identity"] = list(next(iter(identities))) if len(identities) == 1 else None
        summaries.append(summary)
    baseline_label = candidates[0]
    lookup = {(s["candidate"], s["scenario"], s["size"], s["edits"], s["pattern"]): s for s in summaries}
    for summary in summaries:
        baseline = lookup.get((baseline_label, summary["scenario"], summary["size"],
                               summary["edits"], summary["pattern"]))
        if baseline is None or not summary["complete"] or not baseline["complete"]:
            continue
        if summary["result_identity"] != baseline["result_identity"]:
            summary["comparison_status"] = "identity_or_checksum_mismatch"
            continue
        if summary["harness_sha256"] != baseline["harness_sha256"]:
            summary["comparison_status"] = "harness_mismatch"
            continue
        if summary.get("allocator_instrumented"):
            summary["comparison_status"] = "allocation_profile"
            continue
        summary["comparison_status"] = "baseline" if summary["candidate"] == baseline_label else "matched"
        if summary["candidate"] == baseline_label:
            continue
        for phase, candidate_stats in summary["phases_ns"].items():
            base_stats = baseline["phases_ns"].get(phase)
            if (base_stats and candidate_stats and base_stats["count"] == repeats and
                    candidate_stats["count"] == repeats and candidate_stats["median"] > 0):
                summary["baseline_over_candidate"][phase] = base_stats["median"] / candidate_stats["median"]
    return summaries


def report(metadata, summaries):
    lines = ["# Model API observations", "", "Baseline candidate: `" + metadata["candidate_order"][0] + "`.",
             "Fresh release binaries run serially; candidate and workload order reverse on alternate repeats.",
             "Setup and correctness verification are outside operation timing; warmups are excluded.",
             "Transaction boundary timing includes begin, final validation, and commit or rollback.",
             "Total workload = operation + transaction boundary; setup and verification are separate.",
             "Throughput is the per-run unit or checked-API-call count divided by total workload time.",
             "A creation unit is create + required-name set (2 checked API calls); set-many is one batch.",
             "A validate unit is one full validation (0 checked edit API calls). Zero-duration samples are omitted.",
             "Ratios are baseline median / candidate median; greater than 1 means less time in that phase.",
             "Ratios require all requested repeats and matching workload, schema, counts and verified checksum.",
             "Process wall time includes startup, memory handshakes, verification and shutdown.",
             "RSS/private memory are paused phase observations, not retained model heap size.",
             "Peak RSS is the OS lifetime high-water mark through the sampled phase, not a sampled RSS maximum.",
             "Windows private bytes measure committed memory; Linux private bytes measure resident pages.",
             "Per-run latency quantiles remain separate from across-run timing statistics.", "",
             "| Scenario | Size | Edits | Pattern | Candidate | Successful / requested | Status | Operation median [min, max] ms | Boundary median ms | Total median ms | Total ratio | Workload units/s | Verified RSS MiB | Peak RSS through teardown MiB |",
             "| --- | ---: | ---: | --- | --- | ---: | --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: |"]
    for summary in summaries:
        def med(value, divisor=1):
            return f'{value["median"] / divisor:.3f}' if value else "-"
        operation = summary["phases_ns"].get("operation")
        operation_text = (f'{operation["median"] / 1e6:.3f} [{operation["min"] / 1e6:.3f}, '
                          f'{operation["max"] / 1e6:.3f}]') if operation else "-"
        ratio = summary["baseline_over_candidate"].get("total_workload")
        status = summary["comparison_status"] + "; " + ", ".join(summary["statuses"])
        if not summary["consistent_results"]:
            status += "; inconsistent_results"
        columns = [summary["scenario"], str(summary["size"]), str(summary["edits"]), summary["pattern"],
                   summary["candidate"], f'{summary["successful_repeats"]}/{summary["expected_repeats"]}', status,
                   operation_text, med(summary["phases_ns"].get("transaction_boundary"), 1e6),
                   med(summary["phases_ns"].get("total_workload"), 1e6),
                   f"{ratio:.3f}x" if ratio is not None else "-",
                   med(summary["throughput_per_second"]["workload_units"]),
                   med(summary["phase_memory"]["verified"]["rss_bytes"], 2**20),
                   med(summary["phase_memory"]["teardown"]["peak_rss_bytes"], 2**20)]
        lines.append("| " + " | ".join(columns) + " |")
    instrumented = [summary for summary in summaries if summary.get("allocator_instrumented")]
    if instrumented:
        lines += ["", "## Allocation profile", "",
                  "Counting allocator instrumentation perturbs timings. Compare allocation profiles separately from primary timings.",
                  "Counts include the harness and shared API work inside the phase. A workload unit may contain multiple API calls.",
                  "Peak requested bytes reset after setup; bytes exclude allocator metadata and fragmentation.", "",
                  "| Scenario | Size | Pattern | Candidate | Requested B/unit | Allocation calls/unit | Requested B/API call | Workload peak requested MiB | Live byte delta |",
                  "| --- | ---: | --- | --- | ---: | ---: | ---: | ---: | ---: |"]
        for summary in instrumented:
            metrics = summary["allocation_metrics"]
            columns = [summary["scenario"], str(summary["size"]), summary["pattern"], summary["candidate"],
                       med(metrics.get("workload_requested_bytes_per_workload_units")),
                       med(metrics.get("workload_allocation_calls_per_workload_units")),
                       med(metrics.get("workload_requested_bytes_per_edit_operations")),
                       med(metrics.get("workload_peak_bytes"), 2**20), med(metrics.get("workload_live_bytes_delta"))]
            lines.append("| " + " | ".join(columns) + " |")
    lines += ["", "These observations cover only the recorded native in-memory workloads. They do not establish",
              "general graph performance, SQLite/browser performance, or language conformance.",
              "See results.json for run order, complete raw records, failures, CPU, phase memory and provenance.", ""]
    return "\n".join(lines)



def retry_file_operation(operation, attempts=6):
    """Bound retries for transient Windows file-watcher/sharing errors."""
    for attempt in range(attempts):
        try:
            return operation()
        except OSError as error:
            transient = error.errno in (13, 22) or getattr(error, "winerror", None) in (5, 32, 33, 87)
            if not transient or attempt + 1 == attempts:
                raise
            time.sleep(min(0.05 * 2 ** attempt, 0.4))


def write_complete_text(path, content):
    with path.open("w", encoding="utf-8", newline="") as stream:
        stream.write(content)
        stream.flush()
        os.fsync(stream.fileno())


def atomic_write_text(path, content):
    """Replace one complete file; a failed write leaves the old output intact."""
    descriptor, filename = retry_file_operation(
        lambda: tempfile.mkstemp(prefix=f".{path.name}.", suffix=".tmp", dir=path.parent))
    os.close(descriptor)
    temporary = Path(filename)
    try:
        retry_file_operation(lambda: write_complete_text(temporary, content))
        retry_file_operation(lambda: os.replace(temporary, path))
    finally:
        active_error = sys.exception()
        try:
            retry_file_operation(lambda: temporary.unlink(missing_ok=True))
        except OSError as cleanup_error:
            if active_error is None:
                raise
            active_error.add_note(f"Could not remove temporary output {temporary}: {cleanup_error}")

def write_outputs(output, metadata, rows, warmups, repeats, candidates):
    summaries = summarize(rows, repeats, candidates, metadata.get("candidates"))
    payload = {"format_version": 1, "metadata": metadata, "warmups": warmups, "runs": rows,
               "summaries": summaries}
    atomic_write_text(output / "results.json", json.dumps(payload, indent=2) + "\n")
    atomic_write_text(output / "REPORT.md", report(metadata, summaries))
    with io.StringIO(newline="") as stream:
        fields = ["candidate", "scenario", "size", "edits", "pattern", "successful_repeats",
                  "expected_repeats", "complete", "comparison_status", "phase", "count",
                  "min_ns", "median_ns", "max_ns", "baseline_over_candidate"]
        writer = csv.DictWriter(stream, fieldnames=fields)
        writer.writeheader()
        for summary in summaries:
            base = {key: summary[key] for key in fields[:9]}
            for phase, phase_stats in (summary["phases_ns"] or {"": None}).items():
                row = {**base, "phase": phase}
                if phase_stats:
                    row.update(count=phase_stats["count"], min_ns=phase_stats["min"],
                               median_ns=phase_stats["median"], max_ns=phase_stats["max"],
                               baseline_over_candidate=summary["baseline_over_candidate"].get(phase, ""))
                writer.writerow(row)
        atomic_write_text(output / "summary.csv", stream.getvalue())
    return summaries


def sha256(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def version(command):
    try:
        result = subprocess.run(command, capture_output=True, text=True, timeout=20, cwd=ROOT)
        return (result.stdout + result.stderr).strip()
    except (OSError, subprocess.TimeoutExpired) as error:
        return str(error)


def run_order(candidates, workloads, repeat):
    labels = candidates if repeat % 2 == 0 else list(reversed(candidates))
    cases = workloads if repeat % 2 == 0 else list(reversed(workloads))
    return [(label, workload) for workload in cases for label in labels]


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--candidate", action="append", required=True, metavar="LABEL=EXE")
    parser.add_argument("--sizes", type=int, nargs="+", default=[100, 500, 1000])
    parser.add_argument("--edits", type=int, default=100)
    parser.add_argument("--scenarios", nargs="+", choices=SCENARIOS, default=list(SCENARIOS))
    parser.add_argument("--patterns", nargs="+", choices=("spread", "hotspot"), default=["spread"])
    parser.add_argument("--repeats", type=int, default=3)
    parser.add_argument("--timeout", type=float, default=60)
    parser.add_argument("--warmup-size", type=int, default=10, help="0 disables untimed warmups")
    parser.add_argument("--warmup-edits", type=int, default=5)
    parser.add_argument("--provenance", type=Path)
    parser.add_argument("--output", type=Path, default=ROOT / "target/model-edit-benchmark")
    args = parser.parse_args(argv)
    if (min(*args.sizes, args.edits, args.repeats, args.warmup_edits) < 1 or
            args.warmup_size < 0 or not math.isfinite(args.timeout) or args.timeout <= 0):
        parser.error("counts and timeout must be positive; warmup size may be zero")
    if any(len(values) != len(set(values)) for values in (args.sizes, args.scenarios, args.patterns)):
        parser.error("sizes, scenarios and patterns must not contain duplicates")
    candidates = {}
    for value in args.candidate:
        label, separator, filename = value.partition("=")
        if not separator or not re.fullmatch(r"[A-Za-z0-9_-]+", label) or label in candidates:
            parser.error("each candidate needs a unique alphanumeric/dash/underscore LABEL=EXE")
        path = Path(filename).resolve()
        if not path.is_file():
            parser.error(f"candidate executable does not exist: {path}")
        candidates[label] = path
    supplied = json.loads(args.provenance.read_text(encoding="utf-8-sig")) if args.provenance else {}
    supplied_candidates = supplied.get("candidates", supplied)
    if not isinstance(supplied_candidates, dict):
        parser.error("provenance must contain a candidate-label mapping")
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=True)
    if (output / "results.json").exists():
        parser.error("output already contains results.json; choose a new run directory")
    metadata = {
        "started_utc": datetime.now(timezone.utc).isoformat(), "platform": platform.platform(),
        "invocation": [sys.executable, str(Path(__file__).resolve()), *(sys.argv[1:] if argv is None else argv)],
        "machine": platform.machine(), "processor": platform.processor(), "logical_cpus": os.cpu_count(),
        "python": platform.python_version(), "rustc_on_path": version(["rustc", "-Vv"]),
        "runner_sha256": sha256(Path(__file__)), "candidate_order": list(candidates),
        "candidates": {label: {"executable": str(path), "binary_sha256": sha256(path),
                               "binary_mtime_ns": path.stat().st_mtime_ns,
                               "supplied_build_provenance": supplied_candidates.get(label)}
                       for label, path in candidates.items()},
        "supplied_provenance": supplied, "repeats": args.repeats, "timeout_seconds": args.timeout,
        "sizes": args.sizes, "edits": args.edits, "scenarios": args.scenarios, "patterns": args.patterns,
        "warmup_size": args.warmup_size, "warmup_edits": args.warmup_edits,
        "order_policy": "serial paired candidates; reverse candidate and workload order on odd repeats",
        "memory_method": "paused phase OS RSS/private and lifetime peak RSS through each phase",
        "timing_method": "child Instant nanoseconds; handshakes outside timed regions; fresh processes",
        "instrumentation": "per-operation latency sampling and child checks; consult identical harness source",
        "environment": {key: os.environ.get(key) for key in
                        ("RUSTFLAGS", "CARGO_BUILD_RUSTFLAGS", "CARGO_PROFILE_RELEASE_DEBUG",
                         "CARGO_PROFILE_RELEASE_LTO", "CARGO_PROFILE_RELEASE_CODEGEN_UNITS")},
    }
    rows, warmups = [], []

    def execute(label, workload):
        command = [str(candidates[label]), "--handshake"]
        for key, value in workload.items():
            command += ["--" + key, str(value)]
        return {"candidate": label, **workload, **measure(command, workload, args.timeout)}

    if args.warmup_size:
        for scenario in args.scenarios:
            for label in candidates:
                warmups.append(execute(label, {"scenario": scenario, "size": args.warmup_size,
                                               "edits": args.warmup_edits, "pattern": args.patterns[0]}))
        write_outputs(output, metadata, rows, warmups, args.repeats, list(candidates))
        if any(row["status"] != "ok" for row in warmups):
            print("Warmup failed; inspect results.json before measuring.")
            return 1
    workloads = [{"scenario": scenario, "size": size, "edits": args.edits, "pattern": pattern}
                 for size in args.sizes for scenario in args.scenarios for pattern in args.patterns]
    for repeat in range(args.repeats):
        for label, workload in run_order(list(candidates), workloads, repeat):
            print(f'{label}: {workload["scenario"]}, size={workload["size"]}, '
                  f'{workload["pattern"]}, repeat={repeat + 1}', flush=True)
            rows.append({**execute(label, workload), "repeat": repeat, "sequence": len(rows)})
            summaries = write_outputs(output, metadata, rows, warmups, args.repeats, list(candidates))
    metadata["finished_utc"] = datetime.now(timezone.utc).isoformat()
    summaries = write_outputs(output, metadata, rows, warmups, args.repeats, list(candidates))
    print(output / "REPORT.md")
    return int(any(not s["complete"] or s["comparison_status"] in ("identity_or_checksum_mismatch", "harness_mismatch")
                   for s in summaries))


if __name__ == "__main__":
    raise SystemExit(main())
