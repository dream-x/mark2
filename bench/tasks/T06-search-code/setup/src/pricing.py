"""Catalog prices and line item arithmetic."""

from util import round_money

CATALOG = {
    "A-11": 20.00,
    "B-22": 12.50,
    "C-33": 7.25,
    "D-44": 3.00,
}


def unit_price(sku):
    """Return the catalog price of a single unit of `sku`."""
    if sku not in CATALOG:
        raise KeyError("unknown sku: %s" % sku)
    return CATALOG[sku]


def line_subtotal(sku, qty):
    """Return the undiscounted, untaxed price of `qty` units of `sku`."""
    return round_money(unit_price(sku) * qty)
