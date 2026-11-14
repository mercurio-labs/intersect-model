"""Exact transient persistence evidence; cached observations are not full qualification."""
import hashlib
import json
from pathlib import Path
import unittest
ROOT=Path(__file__).resolve().parents[1]
FIXTURE=ROOT/'fixtures/ecore-transient'
class TransientOracleTests(unittest.TestCase):
    def test_identity_full_inverse_matrix_and_projection_outcomes(self):
        provenance=json.loads((FIXTURE/'provenance.json').read_text())
        self.assertFalse(provenance['runtime_support_awarded'])
        for name,digest in provenance['oracles'].items():self.assertEqual(hashlib.sha256((ROOT/name).read_bytes()).hexdigest(),digest)
        for name,digest in provenance['files'].items():self.assertEqual(hashlib.sha256((FIXTURE/name).read_bytes()).hexdigest(),digest,name)
        cases=json.loads((FIXTURE/'emf-xmi/observations.json').read_text())['cases']
        self.assertEqual(len(cases),96)
        keys={(c['containment'],c['out_many'],c['back_many'],c['out_unsettable'],c['back_unsettable'],c['out_transient'],c['back_transient']) for c in cases}
        expected={(c,o,b,u,v,t,r) for c in [False,True] for o in [False,True] for b in [False,True] for u in [False,True] for v in [False,True] for t in [False,True] for r in [False,True] if not(c and b)}
        self.assertEqual(keys,expected)
        for case in cases:
            self.assertIsNone(case['load_error']);self.assertEqual(len(case['before']),12);self.assertEqual(len(case['after']),12)
            self.assertEqual(case['after']['linked-b'] is None,case['containment'] and case['out_transient'])
            if not case['containment']:
                self.assertEqual(case['after']['linked-a']['is_set'],not(case['out_transient'] and case['back_transient']))
        values=json.loads((FIXTURE/'emf-values/observations.json').read_text())['cases']
        self.assertEqual(len(values),17)
        for case in values:
            self.assertTrue(all(not feature['is_set'] for feature in case['after'].values()))
            self.assertEqual(case['after']['value']['value'],'seed')
    def test_native_exports_match_independent_emf_projection(self):
        provenance=json.loads((FIXTURE/'provenance.json').read_text())
        self.assertEqual(hashlib.sha256((ROOT/provenance['native_exporter_snapshot']).read_bytes()).hexdigest(),provenance['native_exporter_sha256'])
        inverse=json.loads((FIXTURE/'emf-xmi/observations.json').read_text())['cases']
        report=json.loads((FIXTURE/'emf-xmi/native-verification.json').read_text())
        self.assertEqual(report['verified'],96);self.assertEqual(len(report['cases']),96)
        self.assertEqual({c['name']:c['objects'] for c in report['cases']},{c['name']:c['after'] for c in inverse})
        self.assertTrue(all(c['preserved'] for c in report['cases']))
        values=json.loads((FIXTURE/'emf-values/observations.json').read_text())['cases']
        report=json.loads((FIXTURE/'emf-values/native-verification.json').read_text())
        self.assertEqual(report['verified'],17);self.assertEqual(len(report['cases']),17)
        self.assertEqual({c['name']:c['state'] for c in report['cases']},{c['name']:c['after'] for c in values})
        for case in report['cases']:
            self.assertTrue(case['preserved']);self.assertEqual(case['objects'],2 if case['name'] in ['link-target','links-target'] else 1)
        expected={f"inverse/{c['name']}/{file}" for c in inverse for file in ['ids.tsv','resource-0000.xmi']}|{f"values/{c['name']}/{file}" for c in values for file in ['ids.tsv','resource-0000.xmi']}
        self.assertEqual(set(provenance['native_exports']),expected)
        self.assertEqual(len(provenance['negative_controls']),2)
        self.assertTrue(all(c['rejected'] for c in provenance['negative_controls']))
if __name__=='__main__':unittest.main()
