"""Model inventory must remain structural and award no Text execution."""
import unittest
import definition_ledger as ledger


class ModelInventoryTests(unittest.TestCase):
    def test_structural_inventory_has_no_text_or_runtime_award(self):
        result = ledger.build()
        self.assertEqual(result['counts']['by_domain'], {'ecore': 350})
        self.assertEqual(result['portable_oracle']['cases'], [])
        self.assertEqual(result['portable_oracle']['intersect_execution'], 'not_run')
        self.assertTrue(all(row['semantically_verified'] == 'not_assessed' for row in result['obligations']))


if __name__ == '__main__':
    unittest.main()
