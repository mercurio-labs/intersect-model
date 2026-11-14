"""Offline integrity and scope checks for reference identity observations."""
import hashlib
import json
from pathlib import Path
import unittest
ROOT = Path(__file__).resolve().parents[1]
FIXTURE = ROOT / 'fixtures/ecore-structure'

class ReferenceIdentityOracleTests(unittest.TestCase):
    def test_provenance_and_complete_case_matrix(self):
        provenance = json.loads((FIXTURE / 'reference-identity-provenance.json').read_text())
        for path, digest in [(ROOT / provenance['oracle'], provenance['oracle_sha256']),
                             (FIXTURE / provenance['observations'], provenance['observations_sha256'])]:
            self.assertEqual(hashlib.sha256(path.read_bytes()).hexdigest(), digest)
        self.assertEqual(provenance['jars'], json.loads((ROOT / 'fixtures/ecore-feature-flags/opposites-provenance.json').read_text())['jars'])
        self.assertFalse(provenance['runtime_support_awarded'])
        data = json.loads((FIXTURE / provenance['observations']).read_text())
        expected = {(many, unsettable) for many in [False, True] for unsettable in [False, True]}
        for key in ['invalid_declarations', 'cases']:
            self.assertEqual(len(data[key]), 4)
            self.assertEqual({(c['many'], c['unsettable']) for c in data[key]}, expected)
        for invalid in data['invalid_declarations']:
            self.assertFalse(invalid['valid'])
            self.assertIn(11, invalid['codes'])
        for case in data['cases']:
            self.assertEqual([s['operation'] for s in case['steps']], ['fresh', 'bind-other', 'bind-self', 'displace-self'])
            states = {s['operation']: s['objects'] for s in case['steps']}
            self.assertEqual(states['bind-self']['a']['back'], 'a')
            self.assertEqual(states['displace-self']['a']['back'], 'b')
            self.assertEqual(states['displace-self']['a']['out'], [] if case['many'] else None)

if __name__ == '__main__':
    unittest.main()
