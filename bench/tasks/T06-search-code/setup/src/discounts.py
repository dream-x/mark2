"""Customer tier discounts."""

from util import clamp, round_money

TIER_DISCOUNTS = {
    "basic": 0.0,
    "silver": 0.05,
    "gold": 0.10,
    "platinum": 0.15,
}

MAX_DISCOUNT = 0.30


def tier_discount(tier):
    """Return the discount fraction for a customer tier."""
    rate = TIER_DISCOUNTS.get(tier, 0.0)
    return clamp(rate, 0.0, MAX_DISCOUNT)


def apply_discount(amount, tier):
    """Return `amount` reduced by the discount of `tier`."""
    return round_money(amount * (1.0 - tier_discount(tier)))
