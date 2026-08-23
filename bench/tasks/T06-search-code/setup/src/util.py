"""Numeric helpers used across the pricing code."""


def round_money(value):
    """Round a monetary amount to two decimal places."""
    return round(float(value), 2)


def apply_rate(amount, rate):
    """Return `amount` increased by `rate` (0.2 means +20 percent)."""
    return round_money(amount * (1.0 + rate))


def clamp(value, low, high):
    """Return `value` restricted to the [low, high] interval."""
    if value < low:
        return low
    if value > high:
        return high
    return value
