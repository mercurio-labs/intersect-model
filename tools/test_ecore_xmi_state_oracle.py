"""Offline integrity and state checks for the pinned unsettable XMI oracle."""
import hashlib
import json
from pathlib import Path
import unittest
ROOT=Path(__file__).resolve().parents[1]
FIXTURE=ROOT/'fixtures/ecore-unsettable/emf-xmi'
class XmiStateOracleTests(unittest.TestCase):
    def test_pinned_source_output_and_bidirectional_case_coverage(self):
        provenance=json.loads((FIXTURE/'provenance.json').read_text())
        self.assertEqual(hashlib.sha256((ROOT/provenance['oracle']).read_bytes()).hexdigest(),provenance['oracle_sha256'])
        for name,digest in provenance['files'].items():
            self.assertEqual(hashlib.sha256((FIXTURE/name).read_bytes()).hexdigest(),digest,name)
        observations=json.loads((FIXTURE/'observations.json').read_text())['cases']
        names={c['name'] for c in observations}
        self.assertEqual(len(names),17)
        self.assertEqual(provenance['cases'],17)
        verification=json.loads((FIXTURE/'native-verification.json').read_text())
        self.assertEqual(verification['verified'],17)
        self.assertEqual(set(verification['cases']),names)
        self.assertEqual(set(provenance['native_export_sha256']),names)
        for case in observations:
            self.assertEqual(case['before'],case['after'],case['name'])
            self.assertEqual(set(case['after']),{'value','nullable','tags','link','links','child','children'})
        self.assertFalse(provenance['runtime_support_awarded'])
    def test_empty_null_default_and_omission_are_distinct(self):
        cases={c['name']:c for c in json.loads((FIXTURE/'observations.json').read_text())['cases']}
        self.assertFalse(cases['fresh']['after']['value']['is_set'])
        self.assertEqual(cases['fresh']['after']['value']['value'],'seed')
        self.assertTrue(cases['scalar-default']['after']['value']['is_set'])
        for name,field in [('scalar-null','value'),('nullable-null','nullable'),('link-null','link'),('child-null','child')]:
            self.assertEqual(cases[name]['after'][field],{'value':None,'is_set':True})
            self.assertIn('xsi:nil="true"',(FIXTURE/cases[name]['file']).read_text())
        for name,field in [('tags-empty','tags'),('links-empty','links'),('children-empty','children')]:
            self.assertEqual(cases[name]['after'][field],{'value':[],'is_set':True})
            self.assertIn(field+'=""',(FIXTURE/cases[name]['file']).read_text())
        self.assertEqual(cases['tags-empty-item']['after']['tags'],{'value':[''],'is_set':True})
if __name__=='__main__': unittest.main()
