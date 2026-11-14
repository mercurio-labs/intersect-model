"""Portable scalar evidence remains checkable without build caches or Java."""
from pathlib import Path
import subprocess
import sys
import unittest


class ScalarComparisonEvidence(unittest.TestCase):
    def test_complete_originals_and_semantic_derivations(self):
        root = Path(__file__).resolve().parents[1]
        result = subprocess.run(
            [sys.executable, "-B", str(root / "fixtures/rcore-scalar-comparison/verify.py")],
            cwd=root, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
        )
        self.assertEqual(result.returncode, 0, (result.stdout + result.stderr).decode("utf-8"))


if __name__ == "__main__":
    unittest.main()
