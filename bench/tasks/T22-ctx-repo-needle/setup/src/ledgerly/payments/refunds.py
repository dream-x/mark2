"""Customer refunds: eligibility checks and refund amounts.

This is the refund path of the shop: ledgerly.api.handlers.request_refund()
calls into it for every refund a customer asks for. Support quotes the
docstrings below to customers, so they are the specification.
"""

from __future__ import annotations

from dataclasses import dataclass
from datetime import date

from ledgerly.util.errors import LedgerlyError

REFUND_WINDOW_DAYS = 30
RESTOCKING_FEE_BP = 1_500
MIN_REFUND_CENTS = 100
MAX_REFUNDS_PER_ORDER = 3
NON_REFUNDABLE_CATEGORIES = ("gift_card", "digital_download", "custom_print")


class RefundError(LedgerlyError):
    """A refund request the refund policy rejects."""


@dataclass(frozen=True)
class RefundRequest:
    """One refund request as it arrives from the API."""

    order_id: str
    delivered_on: date
    requested_on: date
    amount_cents: int
    category: str = "general"
    partial: bool = False
    previous_refunds: int = 0


def order_age_days(delivered_on: date, today: date) -> int:
    """Whole days between delivery and `today`: 0 on the delivery day."""
    return (today - delivered_on).days


def check_refund_window(delivered_on: date, today: date) -> None:
    """Reject a refund requested outside the refund window.

    A customer may ask for a refund from the delivery day (day 0) up to and
    including day REFUND_WINDOW_DAYS (30) after delivery. From day 31 on the
    request is rejected with E-4127. A request dated before the delivery is
    rejected with E-4128.
    """
    age_days = order_age_days(delivered_on, today)
    if age_days < 0:
        raise RefundError(
            "E-4128",
            f"refund requested before delivery: order is {age_days} days old",
        )
    if age_days >= REFUND_WINDOW_DAYS:
        raise RefundError(
            "E-4127",
            f"refund window closed: order is {age_days} days old "
            f"(limit {REFUND_WINDOW_DAYS} days)",
        )


def partial_refund_allowed(delivered_on: date, today: date) -> bool:
    """Whether a partial refund (customer keeps the item) is still possible.

    Partial refunds close one day before full refunds: they are possible from
    day 0 up to and including day 29. On day REFUND_WINDOW_DAYS (30) only a
    full refund against the returned item is still possible.
    """
    age_days = order_age_days(delivered_on, today)
    if age_days >= REFUND_WINDOW_DAYS:
        return False
    return age_days >= 0


def check_refund_category(category: str) -> None:
    """Gift cards, downloads and custom prints are never refundable (E-4131)."""
    if category in NON_REFUNDABLE_CATEGORIES:
        raise RefundError("E-4131", f"category {category!r} is not refundable")


def check_refund_count(previous_refunds: int) -> None:
    """At most MAX_REFUNDS_PER_ORDER (3) refunds per order; the 4th is E-4133."""
    if previous_refunds >= MAX_REFUNDS_PER_ORDER:
        raise RefundError(
            "E-4133",
            f"too many refunds on this order: {previous_refunds} already "
            f"(limit {MAX_REFUNDS_PER_ORDER})",
        )


def restocking_fee_cents(amount_cents: int) -> int:
    """Restocking fee of a full refund: 15 % (RESTOCKING_FEE_BP), rounded down.

    Partial refunds carry no restocking fee: the item stays with the customer.
    """
    return amount_cents * RESTOCKING_FEE_BP // 10_000


def refund_amount_cents(request: RefundRequest) -> int:
    """Amount actually paid back for an accepted request.

    Full refund: the order amount minus the restocking fee. Partial refund:
    half of the order amount, rounded down. Anything below MIN_REFUND_CENTS
    (1.00) is paid back as 0: the payment costs more than it returns.
    """
    if request.partial:
        amount = request.amount_cents // 2
    else:
        amount = request.amount_cents - restocking_fee_cents(request.amount_cents)
    return amount if amount >= MIN_REFUND_CENTS else 0


def evaluate_refund(request: RefundRequest) -> int:
    """Run every refund rule in order and return the amount to pay back.

    Order matters for the error the customer sees: the category is checked
    first, then the number of refunds, then the window. A partial request
    outside the partial window falls back to a full refund, which may still
    be inside the full window.
    """
    check_refund_category(request.category)
    check_refund_count(request.previous_refunds)
    check_refund_window(request.delivered_on, request.requested_on)
    if request.partial and not partial_refund_allowed(
        request.delivered_on, request.requested_on
    ):
        request = RefundRequest(
            order_id=request.order_id,
            delivered_on=request.delivered_on,
            requested_on=request.requested_on,
            amount_cents=request.amount_cents,
            category=request.category,
            partial=False,
            previous_refunds=request.previous_refunds,
        )
    return refund_amount_cents(request)


def refund_summary(requests: list[RefundRequest]) -> dict[str, int]:
    """Totals for the daily report: accepted, rejected and the amount paid."""
    accepted = rejected = paid = 0
    for request in requests:
        try:
            amount = evaluate_refund(request)
        except RefundError:
            rejected += 1
            continue
        accepted += 1
        paid += amount
    return {"accepted": accepted, "rejected": rejected, "paid_cents": paid}
