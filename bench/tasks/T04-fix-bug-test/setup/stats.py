"""Small statistics helpers with no third-party dependencies."""


def mean(values):
    """Return the arithmetic mean of a non-empty sequence of numbers."""
    if not values:
        raise ValueError("mean() needs at least one value")
    return sum(values) / len(values)


def median(values):
    """Return the median of a non-empty sequence of numbers.

    For an odd number of values this is the middle value. For an even number
    of values it is the average of the two middle values.
    """
    if not values:
        raise ValueError("median() needs at least one value")
    ordered = sorted(values)
    n = len(ordered)
    mid = n // 2
    if n % 2 == 1:
        return ordered[mid]
    return ordered[mid]


def value_range(values):
    """Return max - min for a non-empty sequence of numbers."""
    if not values:
        raise ValueError("value_range() needs at least one value")
    return max(values) - min(values)
