"""Ensure the checked-in scalar table is reproducible from its pinned source."""
import unittest
import scalar_unicode


class ScalarUnicodeTests(unittest.TestCase):
    def test_generated_table(self):
        self.assertEqual(scalar_unicode.OUTPUT.read_text(), scalar_unicode.generate())
