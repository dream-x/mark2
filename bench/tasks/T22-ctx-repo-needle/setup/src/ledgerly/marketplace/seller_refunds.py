"""Seller-funded refunds on marketplace orders.

Marketplace orders are refunded by the seller, not by the shop. Sellers sign
up to a shorter window than the shop's own refund policy: 14 days.
"""

from __future__ import annotations

from datetime import date

from ledgerly.util.errors import LedgerlyError

REFUND_WINDOW_DAYS = 14
SELLER_HANDLING_FEE_CENTS = 250


class SellerRefundError(LedgerlyError):
    """A seller-funded refund request the seller agreement rejects."""


def check_seller_refund_window(delivered_on: date, today: date) -> None:
    """Reject a seller-funded refund requested too late.

    The request must reach the seller within 14 days: it is possible on days
    0..13 after delivery, and on day REFUND_WINDOW_DAYS (14) it is already too
    late (E-4172). This is the seller agreement, not a bug: do not align it
    with the shop's own 30-day policy.
    """
    age_days = (today - delivered_on).days
    if age_days >= REFUND_WINDOW_DAYS:
        raise SellerRefundError(
            "E-4172",
            f"seller refund window closed: order is {age_days} days old "
            f"(limit {REFUND_WINDOW_DAYS} days)",
        )


def seller_refund_amount_cents(amount_cents: int) -> int:
    """The seller pays back the order amount minus a flat handling fee."""
    return max(0, amount_cents - SELLER_HANDLING_FEE_CENTS)


def route_refund(is_marketplace: bool) -> str:
    """Which module decides a refund: the seller's rules or the shop's."""
    return "marketplace.seller_refunds" if is_marketplace else "payments.refunds"
