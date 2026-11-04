"""Tests for lib.pager. Run from the project root: python3 tests/test_pager.py"""
import os
import sys
import unittest

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))

from lib.pager import (  # noqa: E402
    PageError,
    check_page,
    clamp_page,
    page_bounds,
    page_count,
    page_label,
    paginate,
)


class PageCountTest(unittest.TestCase):
    def test_rounds_up(self):
        self.assertEqual(page_count(45, 20), 3)
        self.assertEqual(page_count(40, 20), 2)

    def test_empty_ledger_has_one_page(self):
        self.assertEqual(page_count(0, 20), 1)

    def test_rejects_a_page_size_below_one(self):
        with self.assertRaises(PageError):
            page_count(10, 0)


class CheckPageTest(unittest.TestCase):
    def test_accepts_every_existing_page(self):
        for page in (1, 2, 3):
            check_page(page, 45, 20)

    def test_rejects_pages_out_of_range(self):
        for page in (-1, 0, 4):
            with self.assertRaises(PageError):
                check_page(page, 45, 20)


class PageBoundsTest(unittest.TestCase):
    def test_full_page(self):
        self.assertEqual(page_bounds(1, 45, 20), (0, 20))
        self.assertEqual(page_bounds(2, 45, 20), (20, 40))

    def test_last_page_is_short(self):
        self.assertEqual(page_bounds(3, 45, 20), (40, 45))

    def test_empty_ledger(self):
        self.assertEqual(page_bounds(1, 0, 20), (0, 0))


class PaginateTest(unittest.TestCase):
    def test_pages_cover_every_item_once(self):
        items = list(range(45))
        chunks = [paginate(items, page, 20) for page in (1, 2, 3)]
        self.assertEqual([len(chunk) for chunk in chunks], [20, 20, 5])
        self.assertEqual(sum(chunks, []), items)

    def test_empty_ledger(self):
        self.assertEqual(paginate([], 1, 20), [])


class ClampAndLabelTest(unittest.TestCase):
    def test_clamp_page(self):
        self.assertEqual(clamp_page(0, 45, 20), 1)
        self.assertEqual(clamp_page(2, 45, 20), 2)
        self.assertEqual(clamp_page(99, 45, 20), 3)

    def test_label(self):
        self.assertEqual(page_label(2, 45, 20), "page 2 of 3")


if __name__ == "__main__":
    unittest.main()
