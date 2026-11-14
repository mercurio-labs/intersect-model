"""Frozen complete outputs are distinct from the required native stack size."""
import hashlib
import json
from pathlib import Path
import unittest
FIXTURE = Path(__file__).resolve().parent.parent / 'crates/intersect-model/assets/fixtures/rcore-behavior-parser-stack-baseline.json'
PIN = "75af929b7e6043b3b4c446f98a9471d03adb3065f3684ac99caa49b72a6a75d7"

class ParserStackEvidenceTests(unittest.TestCase):
    def test_complete_fixture_and_original_stack_contract_are_frozen(self):
        self.assertEqual(hashlib.sha256(FIXTURE.read_bytes()).hexdigest(), PIN)
        data = json.loads(FIXTURE.read_text(encoding='utf-8'))
        self.assertEqual(data['provenance']['required_thread_stack_bytes'], 1048576)
        self.assertEqual(data['provenance']['capture_thread_stack_bytes'], 8388608)
        self.assertEqual(data['provenance']['syntax_nesting_limit'], 64)

    def test_exact_sources_and_complete_document_source_identities_match(self):
        data = json.loads(FIXTURE.read_text(encoding='utf-8'))
        self.assertEqual([c['id'] for c in data['cases']], [f'{kind}-{depth}' for kind in ['behavior', 'canonical'] for depth in [32,60,63,64]])
        for case in data['cases']:
            source = case['source']
            self.assertEqual(hashlib.sha256(source['text'].encode('utf-8')).hexdigest(), source['id']['sha256'])
            self.assertEqual(case['document']['source'], source)
            self.assertIn('λ🚀', source['text'])

    def test_original_depth_rejection_and_recovery_are_distinct_from_success(self):
        data = json.loads(FIXTURE.read_text(encoding='utf-8'))
        for case in data['cases']:
            if case['id'].endswith('-64'):
                self.assertEqual(len(case['document']['diagnostics']), 1)
                self.assertEqual(case['document']['diagnostics'][0]['message'], 'Rcore syntax nesting exceeds 64 levels')
                self.assertEqual(len(case['document']['expressions']), 1)
            else:
                self.assertEqual(case['document']['diagnostics'], [])
                self.assertEqual(len(case['document']['expressions']), int(case['id'].split('-')[-1])+2)

if __name__ == '__main__':
    unittest.main()
