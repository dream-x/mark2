"""Return labels for customer returns.

A customer can print a prepaid return label up to and including day 30 after
delivery; the label itself stays valid for LABEL_VALID_DAYS after printing.
"""

from __future__ import annotations

from datetime import date, timedelta

from ledgerly.util.errors import LedgerlyError

RETURN_WINDOW_DAYS = 30
LABEL_VALID_DAYS = 10
MAX_PARCELS_PER_RETURN = 4


class ReturnLabelError(LedgerlyError):
    """A return label request that cannot be served."""


def check_return_window(delivered_on: date, today: date) -> None:
    """Return labels: days 0..30 after delivery inclusive, E-4217 afterwards."""
    age_days = (today - delivered_on).days
    if age_days > RETURN_WINDOW_DAYS:
        raise ReturnLabelError(
            "E-4217",
            f"return window closed: order is {age_days} days old "
            f"(limit {RETURN_WINDOW_DAYS} days)",
        )


def label_expires_on(printed_on: date) -> date:
    """Last day the carrier accepts the label (inclusive)."""
    return printed_on + timedelta(days=LABEL_VALID_DAYS)


def check_parcel_count(parcels: int) -> None:
    """At most MAX_PARCELS_PER_RETURN parcels per return (E-4219)."""
    if not 1 <= parcels <= MAX_PARCELS_PER_RETURN:
        raise ReturnLabelError(
            "E-4219",
            f"a return has {parcels} parcels (allowed 1..{MAX_PARCELS_PER_RETURN})",
        )
