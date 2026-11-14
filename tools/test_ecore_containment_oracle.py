"""Integrity and explicit disagreement checks for cached Mercurio Pilot moves."""
import hashlib
import json
from pathlib import Path
import unittest
ROOT = Path(__file__).resolve().parents[1]
FIXTURE = ROOT / 'fixtures/ecore-structure'
class ContainmentOracleTests(unittest.TestCase):
    def test_pinned_cache_and_cycle_disagreement(self):
        p = json.loads((FIXTURE / 'mercurio-containment-provenance.json').read_text())
        raw = (FIXTURE / p['observations']).read_bytes()
        self.assertEqual(hashlib.sha256(raw).hexdigest(), p['observations_sha256'])
        self.assertEqual(p['source_commit'], '32750d7c7799970d7d494e0e8e6e0bf1daae4689')
        self.assertFalse(p['runtime_support_awarded'])
        data = json.loads(raw)
        self.assertIn('provenance', data)
        self.assertEqual(len(data['controls']['initial']), 6)
        steps = data['controls']['steps']
        self.assertEqual(len(steps), 8)
        valid = [s for s in steps if not s['recursive_destination']]
        self.assertEqual(len(valid), 6)
        self.assertTrue(all(s['accepted'] for s in valid))
        cycles = [s for s in steps if s['recursive_destination']]
        self.assertEqual({(s['write_side'], s['accepted']) for s in cycles}, {('inverse', False), ('forward', True)})
        # Repeated same-owner assignment preserves exact order and all graph state.
        self.assertEqual(steps[0]['after'], steps[1]['after'])
if __name__ == '__main__':
    unittest.main()
