"""Deterministic protocol, aggregation and comparison controls for run.py."""
import importlib.util
import json
from pathlib import Path
import sys
import tempfile
import unittest
from unittest import mock

SPEC = importlib.util.spec_from_file_location("model_edit_runner", Path(__file__).with_name("run.py"))
RUNNER = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(RUNNER)
EXPECTED = {"scenario": "basic-scalar-edit", "size": 100, "edits": 10, "pattern": "spread"}


def record(operation=100):
    return {"format_version": 1, "workload_id": "model-edit-v1", **EXPECTED,
            "correct": True, "schema_revision": "fixture-v1", "schema_digest": "schema-123",
            "schema_digest_algorithm": "fnv1a64-json-fixture-sources", "checksum": "state-456",
            "edit_operations": 10, "workload_units": 10, "allocator_instrumented": False,
            "allocation_snapshots": dict.fromkeys(RUNNER.PHASES),
            "phases_ns": {"setup": 20, "operation": operation, "transaction_boundary": 0,
                          "verification": 10}, "operation_latency_ns": None}


def row(candidate, repeat, operation=100):
    return {"candidate": candidate, "repeat": repeat, **EXPECTED, "status": "ok",
            "result": record(operation), "process_wall_seconds": 0.1,
            "process_cpu": {"user_seconds": 0.01, "system_seconds": 0.001},
            "phase_memory": {phase: {"rss_bytes": 1000, "peak_rss_bytes": 2000,
                                     "private_bytes": 500} for phase in RUNNER.PHASES}}


def instrument(result):
    result["allocator_instrumented"] = True
    result["allocation_snapshots"] = {
        phase: {"live_bytes": i * 100, "peak_bytes": i * 120,
                "allocation_calls": i * 10, "requested_bytes": i * 500}
        for i, phase in enumerate(RUNNER.PHASES, 1)}
    return result


class ParseTests(unittest.TestCase):
    def test_verified_result_roundtrip(self):
        value = record()
        self.assertEqual(RUNNER.parse_record(json.dumps(value), EXPECTED), value)

    def test_wrong_workload_or_correctness_rejected(self):
        for field, value in (("size", 101), ("correct", False), ("pattern", "hotspot"),
                             ("format_version", True), ("checksum", "")):
            with self.subTest(field=field):
                result = record()
                result[field] = value
                with self.assertRaises(ValueError):
                    RUNNER.parse_record(json.dumps(result), EXPECTED)

    def test_invalid_timing_or_latency_rejected(self):
        for value in (-1, 0.5, True, "100"):
            result = record(value)
            with self.subTest(value=value), self.assertRaises(ValueError):
                RUNNER.parse_record(json.dumps(result), EXPECTED)
        result = record()
        result["operation_latency_ns"] = {"samples": 10, "p50": 20, "p95": 10, "max": 100}
        with self.assertRaises(ValueError):
            RUNNER.parse_record(json.dumps(result), EXPECTED)

    def test_small_sample_quantiles_are_nullable(self):
        result = record()
        result["operation_latency_ns"] = {"samples": 5, "p50": None, "p95": None, "max": 100}
        self.assertEqual(RUNNER.parse_record(json.dumps(result), EXPECTED), result)
        result["operation_latency_ns"]["samples"] = 20
        with self.assertRaises(ValueError):
            RUNNER.parse_record(json.dumps(result), EXPECTED)

    def test_mode_counters_must_agree(self):
        result = record()
        result["allocator_instrumented"] = True
        with self.assertRaises(ValueError):
            RUNNER.parse_record(json.dumps(result), EXPECTED)

    def test_malformed_or_multiple_json_rejected(self):
        for value in ("{}", "[]", "not json", json.dumps(record()) + "\n" + json.dumps(record())):
            with self.subTest(value=value[:20]), self.assertRaises(ValueError):
                RUNNER.parse_record(value, EXPECTED)


class AggregationTests(unittest.TestCase):
    def test_complete_exact_pair_has_median_spread_and_ratio(self):
        rows = [row("before", 0, 100), row("after", 0, 40),
                row("before", 1, 200), row("after", 1, 60)]
        summaries = RUNNER.summarize(rows, 2, ["before", "after"])
        before, after = summaries
        self.assertEqual(before["phases_ns"]["operation"],
                         {"count": 2, "min": 100, "median": 150, "max": 200})
        self.assertEqual(after["baseline_over_candidate"]["operation"], 3)
        self.assertNotIn("transaction_boundary", after["baseline_over_candidate"])
        self.assertEqual(after["comparison_status"], "matched")

    def test_throughput_uses_each_run_total_and_distinguishes_api_calls(self):
        first, second = row("before", 0, 1_000_000_000), row("before", 1, 3_000_000_000)
        for entry in (first, second):
            entry["result"]["phases_ns"]["transaction_boundary"] = 1_000_000_000
            entry["result"]["edit_operations"] = 20
        summary = RUNNER.summarize([first, second], 2, ["before"])[0]
        self.assertEqual(summary["throughput_per_second"]["workload_units"],
                         {"count": 2, "min": 2.5, "median": 3.75, "max": 5.0})
        self.assertEqual(summary["throughput_per_second"]["checked_api_calls"]["median"], 7.5)

    def test_zero_duration_has_no_throughput_but_validation_has_zero_edit_calls(self):
        zero = row("before", 0, 0)
        summary = RUNNER.summarize([zero], 1, ["before"])[0]
        self.assertIsNone(summary["throughput_per_second"]["workload_units"])
        validation = row("before", 0, 1_000_000_000)
        validation["result"]["edit_operations"] = 0
        summary = RUNNER.summarize([validation], 1, ["before"])[0]
        self.assertEqual(summary["throughput_per_second"]["checked_api_calls"]["median"], 0)
        self.assertEqual(summary["throughput_per_second"]["workload_units"]["median"], 10)

    def test_failure_is_not_zero_and_suppresses_ratio(self):
        failed = row("after", 1)
        failed.update(status="timeout")
        del failed["result"]
        summaries = RUNNER.summarize([row("before", 0), row("before", 1),
                                      row("after", 0, 50), failed], 2, ["before", "after"])
        after = summaries[1]
        self.assertEqual(after["phases_ns"]["operation"]["median"], 50)
        self.assertEqual(after["successful_repeats"], 1)
        self.assertFalse(after["complete"])
        self.assertEqual(after["baseline_over_candidate"], {})

    def test_all_failed_has_no_timing(self):
        failed = row("before", 0)
        failed["status"] = "process_error"
        summary = RUNNER.summarize([failed], 1, ["before"])[0]
        self.assertEqual(summary["phases_ns"], {})
        self.assertIsNone(summary["process_wall_seconds"])

    def test_checksum_schema_and_instrumentation_mismatch_suppress_ratio(self):
        for field, value in (("checksum", "different-state"), ("schema_digest", "different-schema"),
                             ("allocator_instrumented", True)):
            with self.subTest(field=field):
                after = row("after", 0, 50)
                after["result"][field] = value
                if field == "allocator_instrumented":
                    instrument(after["result"])
                summary = RUNNER.summarize([row("before", 0), after], 1, ["before", "after"])[1]
                self.assertEqual(summary["comparison_status"], "identity_or_checksum_mismatch")
                self.assertEqual(summary["baseline_over_candidate"], {})

    def test_instrumented_profiles_have_allocation_counts_without_timing_ratios(self):
        before, after = row("before", 0), row("after", 0, 50)
        instrument(before["result"])
        instrument(after["result"])
        summary = RUNNER.summarize([before, after], 1, ["before", "after"])[1]
        self.assertEqual(summary["comparison_status"], "allocation_profile")
        self.assertEqual(summary["baseline_over_candidate"], {})
        self.assertEqual(summary["allocation_metrics"]["workload_requested_bytes_per_workload_units"]["median"], 50)
        self.assertEqual(summary["phases_ns"]["total_workload"]["median"], 50)

    def test_changed_harness_suppresses_ratios(self):
        provenance = {label: {"supplied_build_provenance": {"harness_sha256": digest}}
                      for label, digest in (("before", "one"), ("after", "two"))}
        summary = RUNNER.summarize([row("before", 0), row("after", 0)], 1,
                                   ["before", "after"], provenance)[1]
        self.assertEqual(summary["comparison_status"], "harness_mismatch")
        self.assertEqual(summary["baseline_over_candidate"], {})

    def test_repeat_mismatch_or_duplicate_repeat_is_incomplete(self):
        for other in (row("before", 0), row("before", 1)):
            first = row("before", 0)
            if other["repeat"] == 1:
                other["result"]["checksum"] = "different-state"
            summary = RUNNER.summarize([first, other], 2, ["before"])[0]
            self.assertFalse(summary["complete"])

    def test_order_reverses_candidate_and_workload_together(self):
        self.assertEqual(RUNNER.run_order(["a", "b"], [1, 2], 0),
                         [("a", 1), ("b", 1), ("a", 2), ("b", 2)])
        self.assertEqual(RUNNER.run_order(["a", "b"], [1, 2], 1),
                         [("b", 2), ("a", 2), ("b", 1), ("a", 1)])

    def test_outputs_include_failure_and_blank_csv_timing(self):
        failed = row("before", 0)
        failed["status"] = "timeout"
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory)
            RUNNER.write_outputs(path, {"candidate_order": ["before"]}, [failed], [], 1, ["before"])
            data = json.loads((path / "results.json").read_text(encoding="utf-8"))
            self.assertEqual(data["runs"][0]["status"], "timeout")
            self.assertIn("timeout", (path / "REPORT.md").read_text(encoding="utf-8"))
            self.assertIn("incomplete", (path / "summary.csv").read_text(encoding="utf-8"))


class AtomicOutputTests(unittest.TestCase):
    def test_transient_replace_keeps_old_file_until_complete_replacement(self):
        with tempfile.TemporaryDirectory() as directory:
            output = Path(directory) / "results.json"
            output.write_text("old complete", encoding="utf-8")
            replace = RUNNER.os.replace
            calls = []

            def fail_once(source, destination):
                calls.append((source, destination))
                self.assertEqual(output.read_text(encoding="utf-8"), "old complete")
                self.assertEqual(source.read_text(encoding="utf-8"), "new complete")
                if len(calls) == 1:
                    error = OSError(13, "sharing violation")
                    error.winerror = 32
                    raise error
                return replace(source, destination)

            with mock.patch.object(RUNNER.os, "replace", side_effect=fail_once), mock.patch.object(RUNNER.time, "sleep"):
                RUNNER.atomic_write_text(output, "new complete")
            self.assertEqual(len(calls), 2)
            self.assertEqual(output.read_text(encoding="utf-8"), "new complete")
            self.assertEqual(list(Path(directory).iterdir()), [output])

    def test_transient_partial_temp_write_retries_without_truncating_destination(self):
        with tempfile.TemporaryDirectory() as directory:
            output = Path(directory) / "REPORT.md"
            output.write_text("previous", encoding="utf-8")
            write = RUNNER.write_complete_text
            calls = []

            def fail_once(path, content):
                calls.append(path)
                self.assertEqual(output.read_text(encoding="utf-8"), "previous")
                if len(calls) == 1:
                    path.write_text("partial", encoding="utf-8")
                    raise OSError(22, "transient invalid argument")
                write(path, content)

            with mock.patch.object(RUNNER, "write_complete_text", side_effect=fail_once), mock.patch.object(RUNNER.time, "sleep"):
                RUNNER.atomic_write_text(output, "complete report")
            self.assertEqual(len(calls), 2)
            self.assertEqual(output.read_text(encoding="utf-8"), "complete report")
            self.assertEqual(list(Path(directory).iterdir()), [output])

    def test_exhausted_replace_preserves_previous_output_and_removes_temporary(self):
        with tempfile.TemporaryDirectory() as directory:
            output = Path(directory) / "summary.csv"
            output.write_text("previous", encoding="utf-8")
            with mock.patch.object(RUNNER.os, "replace", side_effect=OSError(22, "still locked")) as replace, mock.patch.object(RUNNER.time, "sleep") as sleep:
                with self.assertRaises(OSError):
                    RUNNER.atomic_write_text(output, "new")
            self.assertEqual(replace.call_count, 6)
            self.assertEqual(sleep.call_count, 5)
            self.assertEqual(output.read_text(encoding="utf-8"), "previous")
            self.assertEqual(list(Path(directory).iterdir()), [output])

    def test_permanent_write_error_is_not_retried(self):
        with tempfile.TemporaryDirectory() as directory:
            output = Path(directory) / "results.json"
            output.write_text("previous", encoding="utf-8")
            with mock.patch.object(RUNNER, "write_complete_text", side_effect=OSError(28, "disk full")) as write, mock.patch.object(RUNNER.time, "sleep") as sleep:
                with self.assertRaises(OSError):
                    RUNNER.atomic_write_text(output, "new")
            self.assertEqual(write.call_count, 1)
            sleep.assert_not_called()
            self.assertEqual(output.read_text(encoding="utf-8"), "previous")
            self.assertEqual(list(Path(directory).iterdir()), [output])

    def test_all_requested_windows_error_codes_are_retryable(self):
        for winerror in (5, 32, 33, 87):
            with self.subTest(winerror=winerror):
                error = OSError(999, "Windows transient")
                error.winerror = winerror
                operation = mock.Mock(side_effect=[error, "ok"])
                with mock.patch.object(RUNNER.time, "sleep"):
                    self.assertEqual(RUNNER.retry_file_operation(operation), "ok")
                self.assertEqual(operation.call_count, 2)


class ProcessTests(unittest.TestCase):
    def test_handshake_samples_child_and_accepts_final_record(self):
        script = ("import json,sys\n"
                  f"for phase in {RUNNER.PHASES!r}:\n"
                  " print(json.dumps({'event':'ready','phase':phase}),flush=True)\n"
                  " sys.stdin.readline()\n"
                  f"print({json.dumps(record())!r},flush=True)\n")
        result = RUNNER.measure([sys.executable, "-c", script], EXPECTED, 10,
                                sample=lambda process: {"rss_bytes": 1000})
        self.assertEqual(result["status"], "ok", result)
        self.assertEqual(tuple(result["phase_memory"]), RUNNER.PHASES)

    def test_timeout_is_recorded_and_child_is_killed(self):
        result = RUNNER.measure([sys.executable, "-c", "import time; time.sleep(30)"], EXPECTED, 0.1)
        self.assertEqual(result["status"], "timeout")
        self.assertNotIn("result", result)
        self.assertGreater(result["process_wall_seconds"], 0)

    def test_out_of_order_handshake_is_rejected(self):
        script = "print('{\"event\":\"ready\",\"phase\":\"setup\"}',flush=True)"
        result = RUNNER.measure([sys.executable, "-c", script], EXPECTED, 10)
        self.assertEqual(result["status"], "protocol_error")

    def test_failed_child_preserves_bounded_diagnostics(self):
        script = "import sys; sys.stderr.write('x'*100000); sys.exit(7)"
        result = RUNNER.measure([sys.executable, "-c", script], EXPECTED, 10)
        self.assertEqual(result["status"], "process_error")
        self.assertEqual(result["exit_code"], 7)
        self.assertEqual(len(result["captured_output"]["stderr"]), RUNNER.CAPTURE_LIMIT)
        self.assertTrue(result["output_truncated"]["stderr"])


if __name__ == "__main__":
    unittest.main()
