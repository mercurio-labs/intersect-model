"""Offline provenance and scope checks for independent deletion observations."""
import hashlib
import json
from pathlib import Path
import unittest
ROOT = Path(__file__).resolve().parents[1]
FIXTURE = ROOT / 'fixtures/ecore-structure'
class DeletionOracleTests(unittest.TestCase):
    def test_hashes_case_matrix_and_survivor_state(self):
        p = json.loads((FIXTURE / 'deletion-provenance.json').read_text())
        for path, digest in [(ROOT / p['oracle'], p['oracle_sha256']), (FIXTURE / p['observations'], p['observations_sha256'])]:
            self.assertEqual(hashlib.sha256(path.read_bytes()).hexdigest(), digest)
        self.assertFalse(p['runtime_support_awarded'])
        self.assertEqual(p['jars'], json.loads((FIXTURE / 'reference-identity-provenance.json').read_text())['jars'])
        cases = json.loads((FIXTURE / p['observations']).read_text())['cases']
        self.assertEqual(len(cases), 6)
        self.assertEqual({(c['unsettable'],c['operation']) for c in cases}, {(u,o) for u in [False,True] for o in ['remove','delete-object','delete-subtree']})
        for c in cases:
            s = c['objects']; operation = c['operation']
            self.assertEqual(c['roots_one'], ['a','d'])
            self.assertEqual(c['roots_two'], ['e'])
            self.assertEqual(s['a']['children'], [])
            self.assertEqual(s['a']['children_set'], c['unsettable'])
            self.assertEqual(s['d']['link'], 'b' if operation == 'remove' else None)
            self.assertEqual(s['e']['link'], None if operation == 'delete-subtree' else 'c')
            self.assertEqual(s['d']['link_set'], operation == 'remove')
            self.assertEqual(s['e']['link_set'], operation != 'delete-subtree')
            self.assertIsNone(s['b']['resource'])
            self.assertEqual(s['c']['parent'], 'b')  # retained Java objects, not native records
if __name__ == '__main__':
    unittest.main()
