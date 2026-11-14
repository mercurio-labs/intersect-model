"""Provenance and rejection checks for offline qualification fixture conversion."""
import copy
import hashlib
import json
import unittest

from behavior_fixture_migration import canonicalize_bundle_value, render


class BehaviorFixtureMigrationTests(unittest.TestCase):
    def test_exact_source_dependencies_are_rebound_without_changing_model_bytes(self):
        text = json.dumps({'version': 1, 'scope': 'unique_names', 'derived': [], 'validations': []})
        identity = {'uri': 'memory:rules', 'sha256': hashlib.sha256(text.encode()).hexdigest()}
        model = {'id': {'uri': 'memory:model', 'sha256': hashlib.sha256(b'model bytes').hexdigest()},
                 'kind': 'rcore', 'text': 'model bytes', 'dependencies': []}
        source = {'id': identity, 'kind': 'rcore_behavior', 'text': text, 'dependencies': [model['id']]}
        dependent = {'id': {'uri': 'memory:other', 'sha256': '0' * 64}, 'kind': 'xtext',
                     'text': 'retained bytes', 'dependencies': [identity]}
        original = {'namespace': 'urn:model', 'rcore_behavior': identity,
                    'bundle': {'version': 1, 'sources': [model, source, dependent], 'namespaces': []}}
        preserved = copy.deepcopy(original)
        converted = canonicalize_bundle_value(original)
        current = converted['bundle']['sources'][1]
        self.assertEqual(original, preserved)
        self.assertEqual(converted['bundle']['sources'][0], model)
        self.assertEqual(current['id']['sha256'], hashlib.sha256(current['text'].encode()).hexdigest())
        self.assertNotEqual(current['id'], identity)
        self.assertEqual(converted['rcore_behavior'], current['id'])
        self.assertEqual(converted['bundle']['sources'][2]['dependencies'], [current['id']])
        self.assertEqual(current['dependencies'], [model['id']])

    def test_unrecognized_contract_data_is_not_silently_discarded(self):
        rules = {'version': 1, 'scope': 'unique_names', 'derived': [], 'validations': []}
        with self.assertRaises(ValueError):
            render({**rules, 'future_behavior': True}, 'urn:model')
        with self.assertRaises(ValueError):
            render({**rules, 'derived': [{'class': 'Item', 'feature': 'value',
                                        'expression': {'op': 'count', 'feature': 'items', 'extra': True}}]}, 'urn:model')
        with self.assertRaises(ValueError):
            render({**rules, 'initializers': [{'class': 'Item', 'provider': {'name': 'p', 'version': '1'},
                                             'members': [], 'values': {'missing': 1}}]}, 'urn:model')


if __name__ == '__main__':
    unittest.main()
