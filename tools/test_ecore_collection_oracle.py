"""Independent dynamic collection oracle integrity and matrix completeness."""
import hashlib
import json
from pathlib import Path
import unittest
ROOT=Path(__file__).resolve().parents[1]
FIXTURE=ROOT/'fixtures/ecore-feature-flags'
class CollectionOracleTests(unittest.TestCase):
    def test_full_matrix_and_source_identity(self):
        provenance=json.loads((FIXTURE/'collections-provenance.json').read_text())
        self.assertFalse(provenance['runtime_support_awarded'])
        for path,digest in [(ROOT/provenance['oracle'],provenance['oracle_sha256']),(FIXTURE/provenance['observations'],provenance['observations_sha256'])]:
            self.assertEqual(hashlib.sha256(path.read_bytes()).hexdigest(),digest)
        cases=json.loads((FIXTURE/provenance['observations']).read_text())['cases']
        self.assertEqual(len(cases),16)
        self.assertEqual({tuple(c[f] for f in ['changeable','unsettable','unique','ordered']) for c in cases},{(a,b,c,d) for a in [False,True] for b in [False,True] for c in [False,True] for d in [False,True]})
        for case in cases:
            self.assertEqual(len(case['steps']),11)
            steps={s['operation']:s for s in case['steps']}
            self.assertFalse(steps['remove-missing-fresh']['is_set'])
            self.assertEqual(steps['clear-fresh']['is_set'],case['unsettable'])
            self.assertEqual(steps['add-a-again']['value'],['a'] if case['unique'] else ['a','a'])
            self.assertTrue(all(s['accepted'] for s in case['steps'][:-1]))
            self.assertEqual(steps['reset']['accepted'],case['changeable'])
    def test_indexed_ordered_matrix_and_failures(self):
        provenance=json.loads((FIXTURE/'indexed-collections-provenance.json').read_text())
        self.assertFalse(provenance['runtime_support_awarded'])
        for path,digest in [(ROOT/provenance['oracle'],provenance['oracle_sha256']),(FIXTURE/provenance['observations'],provenance['observations_sha256'])]:
            self.assertEqual(hashlib.sha256(path.read_bytes()).hexdigest(),digest)
        cases=json.loads((FIXTURE/provenance['observations']).read_text())['cases']
        self.assertEqual(len(cases),8)
        self.assertEqual({(c['changeable'],c['unsettable'],c['unique']) for c in cases},{(a,b,c) for a in [False,True] for b in [False,True] for c in [False,True]})
        for case in cases:
            self.assertTrue(case['ordered'])
            self.assertEqual(len(case['steps']),16)
            steps={s['operation']:s for s in case['steps']}
            self.assertEqual(steps['insert-duplicate']['accepted'],not case['unique'])
            self.assertEqual(steps['replace-duplicate']['accepted'],not case['unique'])
            for name in ['invalid-insert','invalid-replace','invalid-remove','invalid-move','empty-remove']:
                self.assertFalse(steps[name]['accepted'])
            self.assertEqual(steps['reset']['accepted'],case['changeable'])
if __name__=='__main__':unittest.main()
