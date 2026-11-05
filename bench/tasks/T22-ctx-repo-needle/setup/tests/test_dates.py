import sys
import unittest
from datetime import date
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "src"))

from ledgerly.util.dates import add_business_days, days_between, month_end  # noqa: E402


class DatesTest(unittest.TestCase):
    def test_days_between(self):
        self.assertEqual(days_between(date(2025, 1, 1), date(2025, 1, 31)), 30)
        self.assertEqual(days_between(date(2025, 1, 31), date(2025, 1, 1)), -30)

    def test_add_business_days(self):
        self.assertEqual(add_business_days(date(2025, 1, 3), 1), date(2025, 1, 6))

    def test_month_end(self):
        self.assertEqual(month_end(date(2024, 2, 10)), date(2024, 2, 29))


if __name__ == "__main__":
    unittest.main()
