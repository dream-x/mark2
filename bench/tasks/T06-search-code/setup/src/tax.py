"""Tax rates and tax application."""

from util import apply_rate

STANDARD_RATE = 0.20
REDUCED_RATE = 0.05
ZERO_RATE = 0.0

CATEGORY_RATES = {
    "standard": STANDARD_RATE,
    "reduced": REDUCED_RATE,
    "exempt": ZERO_RATE,
}


def tax_rate(category="standard"):
    """Return the tax rate for a product category."""
    return CATEGORY_RATES.get(category, STANDARD_RATE)


def add_tax(amount, category="standard"):
    """Return `amount` with the category tax added on top."""
    return apply_rate(amount, tax_rate(category))
