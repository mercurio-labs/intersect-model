"""Offline checks for nonunique opposite declaration and execution evidence."""
import hashlib
import itertools
import json
from pathlib import Path
import unittest
ROOT = Path(__file__).resolve().parents[1]
FIXTURE = ROOT / 'fixtures/ecore-structure'
class NonuniqueOracleTests(unittest.TestCase):
    def test_provenance_validity_matrix_and_execution_counts(self):
        provenance = json.loads((FIXTURE / 'nonunique-opposites-provenance.json').read_text(encoding='utf8'))
        for path, digest in [(ROOT / provenance['oracle'], provenance['oracle_sha256']),
                             (FIXTURE / provenance['observations'], provenance['observations_sha256'])]:
            self.assertEqual(hashlib.sha256(path.read_bytes()).hexdigest(), digest)
        prior = json.loads((FIXTURE / 'reference-identity-provenance.json').read_text(encoding='utf8'))
        self.assertEqual(provenance['jars'], prior['jars'])
        self.assertFalse(provenance['runtime_support_awarded'])
        cases = json.loads((FIXTURE / provenance['observations']).read_text(encoding='utf8'))['cases']
        keys = ['out_many','back_many','out_unique','back_unique','containment','unsettable']
        self.assertEqual(len(cases), 64)
        self.assertEqual({tuple(c[k] for k in keys) for c in cases},set(itertools.product([False,True],repeat=6)))
        self.assertEqual(sum(c['valid'] for c in cases),30)
        for c in cases:
            expected = not ((c['out_many'] and not c['out_unique']) or (c['back_many'] and not c['back_unique']) or (c['containment'] and c['back_many']))
            self.assertEqual(c['valid'],expected)
            if expected:
                self.assertEqual([s['operation'] for s in c['steps']],['fresh','add-first','add-duplicate','add-reverse','remove-one','unset-reverse'])
            else:
                self.assertEqual(c['steps'],[])
                self.assertTrue({d['code'] for d in c['diagnostic']['children']} <= {28,50})
if __name__ == '__main__':
    unittest.main()
