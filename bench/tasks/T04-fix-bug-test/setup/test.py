"""Test suite for stats.py. Run with: python3 test.py"""

import stats


def check(label, got, want):
    if got != want:
        raise AssertionError("%s: got %r, expected %r" % (label, got, want))


def main():
    check("mean/1", stats.mean([2, 4, 6]), 4.0)
    check("mean/2", stats.mean([5]), 5.0)

    check("median/odd", stats.median([3, 1, 2]), 2)
    check("median/single", stats.median([7]), 7)
    check("median/even", stats.median([1, 2, 3, 4]), 2.5)
    check("median/even-unsorted", stats.median([10, 2, 8, 4]), 6.0)
    check("median/even-pair", stats.median([5, 5]), 5.0)

    check("range/1", stats.value_range([3, 9, 1]), 8)

    print("all tests passed")


if __name__ == "__main__":
    main()
