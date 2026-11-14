"""Offline integrity and coverage checks for the independent E09 observations."""
import hashlib
import json
from pathlib import Path
import unittest


ROOT = Path(__file__).resolve().parents[1]
FIXTURE = ROOT / "fixtures" / "ecore-feature-flags"


class EcoreFeatureOracleTests(unittest.TestCase):
    def setUp(self):
        self.provenance = json.loads((FIXTURE / "provenance.json").read_text(encoding="utf8"))
        self.observations = json.loads((FIXTURE / self.provenance["observations"]).read_text(encoding="utf8"))
        self.cases = {(row["mask"], row["many"]): row for row in self.observations["cases"]}

    def test_source_output_and_ecore_archive_have_pinned_identity(self):
        for path, expected in [
            (ROOT / self.provenance["oracle"], self.provenance["oracle_sha256"]),
            (FIXTURE / self.provenance["observations"], self.provenance["observations_sha256"]),
        ]:
            path.resolve().relative_to(ROOT)
            self.assertEqual(hashlib.sha256(path.read_bytes()).hexdigest(), expected)
        upstream = json.loads((ROOT / "fixtures/definition-conformance/upstream/artifacts.json").read_text(encoding="utf8"))
        pinned = {item["archive_sha256"] for item in upstream["files"] if item.get("archive_url", "").endswith("/org.eclipse.emf.ecore-2.38.0.jar")}
        self.assertEqual(len(pinned), 1)
        jars = {row["artifact"]: row["sha256"] for row in self.provenance["jars"]}
        self.assertEqual(jars["org.eclipse.emf:org.eclipse.emf.ecore:2.38.0"], next(iter(pinned)))
        self.assertIn("org.eclipse.emf:org.eclipse.emf.common:2.42.0", jars)
        self.assertFalse(self.provenance["runtime_support_awarded"])

    def test_complete_dynamic_flag_matrix_preserves_orthogonal_observations(self):
        self.assertEqual(len(self.observations["cases"]), 64)
        self.assertEqual(self.provenance["cases"], 64)
        self.assertEqual(set(self.cases), {(mask, many) for mask in range(32) for many in (False, True)})
        for (mask, many), row in self.cases.items():
            for bit, flag in enumerate(("changeable", "derived", "volatile", "transient", "unsettable")):
                self.assertEqual(row[flag], bool(mask & (1 << bit)))
            # For this dynamic fixture, these three flags do not install a getter.
            # This is an oracle observation, not a policy for generated delegates.
            self.assertEqual(row["steps"], self.cases[(mask & 17, many)]["steps"])
            operations = [step["operation"] for step in row["steps"]]
            self.assertEqual(len(operations), 9 if many else 7)
            self.assertEqual(len(set(operations)), len(operations))
            for step in row["steps"]:
                self.assertIn("value", step)
                self.assertIsInstance(step["is_set"], bool)
                self.assertNotIn("read_error", step)
                self.assertNotIn("state_error", step)

    def test_explicit_default_null_empty_and_unset_are_distinct(self):
        def step(mask, many, operation):
            return next(row for row in self.cases[(mask, many)]["steps"] if row["operation"] == operation)
        for many in (False, True):
            self.assertFalse(step(1, many, "explicit-default")["is_set"])
            self.assertTrue(step(17, many, "explicit-default")["is_set"])
            self.assertFalse(step(17, many, "unset")["is_set"])
            self.assertFalse(step(0, many, "explicit-default")["accepted"])
            self.assertFalse(step(0, many, "unset")["accepted"])
        self.assertIsNone(step(17, False, "explicit-null")["value"])
        self.assertTrue(step(17, False, "explicit-null")["is_set"])
        self.assertEqual(step(17, False, "unset")["value"], "seed")
        for operation in ("clear-list", "remove-last"):
            self.assertEqual(step(17, True, operation)["value"], [])
            self.assertTrue(step(17, True, operation)["is_set"])
            self.assertFalse(step(1, True, operation)["is_set"])
        self.assertTrue(step(0, True, "add-list")["accepted"])
        self.assertEqual(step(0, True, "add-list")["value"], ["next"])


if __name__ == "__main__":
    unittest.main()
