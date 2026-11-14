import copy, importlib.util, json, unittest
from pathlib import Path
ROOT=Path(__file__).resolve().parents[1]
HERE=ROOT/'fixtures/rcore-retained-json'
spec=importlib.util.spec_from_file_location('payload_evidence',HERE/'verify.py')
module=importlib.util.module_from_spec(spec);spec.loader.exec_module(module)
class Evidence(unittest.TestCase):
    def test_full_portable_pins_and_original_lineage(self):
        result=module.verify();self.assertEqual(result['source_graphs'],6);self.assertEqual(result['before_rows'],54)
    def test_forged_source_hash_cannot_become_identity(self):
        cases=json.loads((HERE/'requests.json').read_text(encoding='utf-8'));cases=copy.deepcopy(cases[:1]);cases[0]['request']['bundle']['sources'][0]['id']['sha256']='0'*64
        with self.assertRaises(AssertionError):module.validate_graphs(cases)
    def test_original_success_and_failure_cases_remain_separate(self):
        cases=json.loads((HERE/'requests.json').read_text(encoding='utf-8'));self.assertEqual(sum(c['intended_positive']for c in cases),4);self.assertEqual(len(cases),6)
if __name__=='__main__':unittest.main()
