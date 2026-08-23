"""Cart helpers shared by the storefront."""

TAX_RATE = 0.10


def calc_total(items):
    """Return the sum of price * qty over all items."""
    total = 0.0
    for item in items:
        total += item["price"] * item["qty"]
    return round(total, 2)


def total_with_tax(items):
    """Return the cart total including tax."""
    return round(calc_total(items) * (1 + TAX_RATE), 2)


def item_count(items):
    """Return how many units are in the cart."""
    return sum(item["qty"] for item in items)


def average_item_price(items):
    """Return the average price paid per unit, or 0.0 for an empty cart."""
    units = item_count(items)
    if units == 0:
        return 0.0
    return round(calc_total(items) / units, 2)
