import sys
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "src"))

from ledgerly.util.money import format_cents, split_evenly, to_cents  # noqa: E402


class MoneyTest(unittest.TestCase):
    def test_to_cents(self):
        self.assertEqual(to_cents("12.34"), 1234)
        self.assertEqual(to_cents("0,5"), 50)
        self.assertEqual(to_cents("1.005"), 101)

    def test_format_cents(self):
        self.assertEqual(format_cents(1234), "12.34 EUR")
        self.assertEqual(format_cents(-5, "USD"), "-0.05 USD")

    def test_split_evenly(self):
        self.assertEqual(split_evenly(100, 3), [34, 33, 33])
        self.assertEqual(sum(split_evenly(1001, 7)), 1001)


if __name__ == "__main__":
    unittest.main()
