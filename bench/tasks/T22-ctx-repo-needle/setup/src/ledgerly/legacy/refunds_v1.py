"""DEPRECATED: refund checks of the v1 API.

Superseded by ledgerly.payments.refunds (ADR-0019, March 2025). Nothing
imports this module any more; it is kept only until the v1 export job that
replays historical refund decisions is retired. Do not "fix" anything here:
the replay must reproduce what v1 decided at the time, and the v1 rule was
different from today's (see check_refund_window below).
"""

from __future__ import annotations

from datetime import date

from ledgerly.util.errors import LedgerlyError

REFUND_WINDOW_DAYS = 30
LEGACY_RESTOCKING_FEE_BP = 2_000


class LegacyRefundError(LedgerlyError):
    """Rejection raised by the v1 refund rules."""


def check_refund_window(delivered_on: date, today: date) -> None:
    """v1 rule (historical, intentionally kept): the window closed ON day 30.

    Under v1 a refund was possible on days 0..29 only; a request on day 30
    was rejected with E-4127. The v2 policy (ledgerly.payments.refunds) moved
    the boundary; this copy must not follow it.
    """
    age_days = (today - delivered_on).days
    if age_days >= REFUND_WINDOW_DAYS:
        raise LegacyRefundError(
            "E-4127",
            f"refund window closed: order is {age_days} days old "
            f"(limit {REFUND_WINDOW_DAYS} days)",
        )


def legacy_refund_amount_cents(amount_cents: int) -> int:
    """v1 paid back the order amount minus a flat 20 % restocking fee."""
    return amount_cents - amount_cents * LEGACY_RESTOCKING_FEE_BP // 10_000


def replay_decision(delivered_on: date, requested_on: date, amount_cents: int) -> dict:
    """What v1 answered for a historical request: used by the replay job."""
    try:
        check_refund_window(delivered_on, requested_on)
    except LegacyRefundError as error:
        return {"accepted": False, "code": error.code, "paid_cents": 0}
    return {
        "accepted": True,
        "code": None,
        "paid_cents": legacy_refund_amount_cents(amount_cents),
    }
