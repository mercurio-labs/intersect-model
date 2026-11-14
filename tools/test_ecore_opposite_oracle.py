"""Offline checks for independent inverse assignment-state observations."""
import hashlib
import json
from pathlib import Path
import unittest
ROOT=Path(__file__).resolve().parents[1]
FIXTURE=ROOT/'fixtures/ecore-feature-flags'
class OppositeOracleTests(unittest.TestCase):
    def test_identity_and_full_valid_shape_flag_matrix(self):
        provenance=json.loads((FIXTURE/'opposites-provenance.json').read_text())
        for path,digest in [(ROOT/provenance['oracle'],provenance['oracle_sha256']),(FIXTURE/provenance['observations'],provenance['observations_sha256'])]:
            self.assertEqual(hashlib.sha256(path.read_bytes()).hexdigest(),digest)
        self.assertEqual(provenance['jars'],json.loads((FIXTURE/'provenance.json').read_text())['jars'])
        cases=json.loads((FIXTURE/provenance['observations']).read_text())['cases']
        keys={(c['containment'],c['out_many'],c['back_many'],c['out_unsettable'],c['back_unsettable']) for c in cases}
        expected={(containment,out_many,back_many,out_unsettable,back_unsettable) for containment in [False,True] for out_many in [False,True] for back_many in [False,True] for out_unsettable in [False,True] for back_unsettable in [False,True] if not(containment and back_many)}
        self.assertEqual(keys,expected);self.assertEqual(len(cases),24)
        for case in cases:
            self.assertEqual(len(case['steps']),12)
            self.assertEqual(len({s['operation'] for s in case['steps']}),12)
            self.assertTrue(all(s['accepted'] for s in case['steps']))
        self.assertFalse(provenance['runtime_support_awarded'])
    def test_inverse_removal_distinguishes_scalar_from_many_history(self):
        cases=json.loads((FIXTURE/'emf-opposites-observations.json').read_text())['cases']
        for case in cases:
            if not(case['out_unsettable'] and case['back_unsettable']):continue
            steps={s['operation']:s['objects'] for s in case['steps']}
            self.assertFalse(steps['fresh']['a1']['is_set'])
            self.assertTrue(steps['explicit-empty']['a1']['is_set'])
            self.assertTrue(steps['bind-first']['a1']['is_set'])
            self.assertTrue(steps['bind-first']['b1']['is_set'])
            self.assertEqual(steps['replace-target']['b1']['is_set'],case['back_many'])
            self.assertFalse(steps['unset-forward']['a1']['is_set'])
            self.assertEqual(steps['unset-forward']['b2']['is_set'],case['back_many'])
            self.assertFalse(steps['unset-reverse']['b1']['is_set'])
            self.assertEqual(steps['unset-reverse']['a1']['is_set'],case['out_many'])
if __name__=='__main__':unittest.main()
