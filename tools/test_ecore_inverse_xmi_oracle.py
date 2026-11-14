"""Offline identity and completeness checks for the inverse XMI oracle."""
import hashlib
import json
from pathlib import Path
import unittest
ROOT = Path(__file__).resolve().parents[1]
FIXTURE = ROOT / 'fixtures/ecore-unsettable/emf-inverse-xmi'
class InverseXmiOracleTests(unittest.TestCase):
    def test_identity_and_complete_matrix(self):
        provenance = json.loads((FIXTURE / 'provenance.json').read_text())
        self.assertFalse(provenance['runtime_support_awarded'])
        self.assertEqual(hashlib.sha256((ROOT / provenance['oracle']).read_bytes()).hexdigest(), provenance['oracle_sha256'])
        for name, digest in provenance['files'].items():
            self.assertEqual(hashlib.sha256((FIXTURE / name).read_bytes()).hexdigest(), digest, name)
        cases = json.loads((FIXTURE / 'observations.json').read_text())['cases']
        self.assertEqual(len(cases), 24)
        keys = {(c['containment'], c['out_many'], c['back_many'], c['out_unsettable'], c['back_unsettable']) for c in cases}
        expected = {(c,o,b,u,v) for c in [False,True] for o in [False,True] for b in [False,True] for u in [False,True] for v in [False,True] if not (c and b)}
        self.assertEqual(keys, expected)
        for case in cases:
            self.assertIsNone(case['load_error'])
            self.assertTrue(case['emf_preserved'])
            self.assertEqual(case['before'], case['after'])
            self.assertEqual(len(case['after']), 12)
            self.assertEqual(set(case['order']), set(case['after']))
            if case['containment']:
                self.assertFalse(case['after']['empty-back-b']['is_set'])
    def test_independent_native_reload_covers_every_object_and_configuration(self):
        provenance = json.loads((FIXTURE / 'provenance.json').read_text())
        self.assertEqual(hashlib.sha256((ROOT / provenance['native_exporter_snapshot']).read_bytes()).hexdigest(), provenance['native_exporter_sha256'])
        observed = json.loads((FIXTURE / 'observations.json').read_text())['cases']
        report = json.loads((FIXTURE / 'native-verification.json').read_text())
        self.assertEqual(report['verified'], 24)
        self.assertEqual(len(report['cases']), 24)
        expected = {case['name']: case['before'] for case in observed}
        actual = {case['name']: case['objects'] for case in report['cases']}
        self.assertEqual(actual, expected)
        self.assertTrue(all(case['preserved'] for case in report['cases']))
        self.assertEqual(set(provenance['native_exports']), {f"{name}/{file}" for name in expected for file in ['resource-0000.xmi','ids.tsv']})
        self.assertTrue(provenance['negative_control']['rejected'])
if __name__ == '__main__': unittest.main()
