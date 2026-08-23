"""Order level totals.

Two totals exist and they are not the same thing:

* `estimate_total` is the pre-tax number shown in the cart preview.
* `checkout_total` is the amount the customer is actually charged.
"""

from discounts import apply_discount
from pricing import line_subtotal
from tax import add_tax


def estimate_total(sku, qty, tier):
    """Return the discounted, PRE-TAX estimate for a single line order.

    This is a preview number only. It is never the amount charged.
    """
    return apply_discount(line_subtotal(sku, qty), tier)


def checkout_total(sku, qty, tier):
    """Return the final amount charged to the customer for a single line order.

    Discount for the customer tier is applied first, standard tax on top.
    """
    return add_tax(estimate_total(sku, qty, tier))
