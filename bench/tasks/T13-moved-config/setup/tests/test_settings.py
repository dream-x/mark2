"""Unit tests for the core settings loader, run against a tiny fixture."""

import os
import unittest

from services.core import settings

FIXTURE = os.path.join(os.path.dirname(__file__), "fixtures", "settings.ini")


class LoadTest(unittest.TestCase):
    def test_reads_cache_section(self):
        parsed = settings.load(FIXTURE)
        self.assertEqual(parsed["cache"]["backend"], "memory")
        self.assertEqual(parsed["cache"].getint("max_entries"), 3)


if __name__ == "__main__":
    unittest.main()
