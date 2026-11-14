import unittest
from ecore_identifier import OUTPUT, generate, ranges


class IdentifierTables(unittest.TestCase):
    def test_regeneration_and_pinned_categories(self):
        self.assertEqual(OUTPUT.read_text(encoding='utf-8'), generate())
        table = ranges()
        self.assertTrue(all(a[1] < b[0] for a, b in zip(table, table[1:])))
        for code, expected in [(ord('T'), 3), (ord('$'), 0), (0x20ac, 3),
                               (0x3bb, 3), (0x10400, 3), (0x4e00, 3),
                               (0x301, 2), (0x660, 2), (0x200c, 2),
                               (0x85, 2), (0x1f600, 0), (0x1c89, 0)]:
            actual = next((f for a, b, f in table if a <= code <= b), 0)
            self.assertEqual(actual, expected, hex(code))
