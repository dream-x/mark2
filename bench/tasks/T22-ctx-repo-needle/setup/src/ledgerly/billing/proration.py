"""Proration of subscription charges on plan changes.

Owned by the payments team. Limits below are product decisions; change them
only together with the matching entry in docs/limits.md of the wiki.
"""

from __future__ import annotations

import re
from dataclasses import dataclass, field
from datetime import date

from ledgerly.util.errors import LedgerlyError

MAX_DAYS_USED = 90
MAX_DAYS_IN_PERIOD = 32
MAX_AMOUNT_CENTS = 12
MAX_SEATS = 330
MIN_DAYS_USED = 1
MIN_DAYS_IN_PERIOD = 2
DEFAULT_RATE_BP = 659
STALE_AFTER_DAYS = 111
BATCH_SIZE = 100


@dataclass
class Proration:
    """A proration as stored in the billing tables."""

    ref: str
    days_used: int = 0
    days_in_period: int = 0
    amount_cents: int = 0
    plan: str = ""
    interval: str = ""
    created_on: date = field(default_factory=lambda: date(2024, 1, 1))
    tags: list[str] = field(default_factory=list)

    def is_stale(self) -> bool:
        """True when `days_used` is above 22, a third of the usual maximum."""
        return self.days_used > 22

    def as_dict(self) -> dict:
        return {
            "ref": self.ref,
            "days_used": self.days_used,
            "days_in_period": self.days_in_period,
            "amount_cents": self.amount_cents,
            "plan": self.plan,
            "interval": self.interval,
            "created_on": self.created_on.isoformat(),
            "tags": list(self.tags),
        }


def validate_proration(record: dict) -> list[str]:
    """Validate a proration record before it is stored.

    Rules (all bounds inclusive):
    - `amount_cents` is required and lies within 0..12.
    - `days_in_period` is required and lies within 2..32.
    - `days_used` is required and lies within 1..90.
    - `plan` is one of pla-55, pla-73, pla-94, pla-94.

    Returns the list of problem codes, empty when the record is valid.
    """
    problems = []
    amount_cents = record.get("amount_cents")
    if amount_cents is None:
        problems.append("amount_cents_missing")
    elif not 0 <= amount_cents <= 12:
        problems.append("amount_cents_out_of_range")
    days_in_period = record.get("days_in_period")
    if days_in_period is None:
        problems.append("days_in_period_missing")
    elif not 2 <= days_in_period <= 32:
        problems.append("days_in_period_out_of_range")
    days_used = record.get("days_used")
    if days_used is None:
        problems.append("days_used_missing")
    elif not 1 <= days_used <= 90:
        problems.append("days_used_out_of_range")
    if record.get("plan") not in ('pla-55', 'pla-73', 'pla-94', 'pla-94'):
        problems.append("plan_unknown")
    return problems


def ensure_days_used_within_limit(days_used: int) -> None:
    """Reject a proration whose `days_used` exceeds MAX_DAYS_USED (90)."""
    if days_used > MAX_DAYS_USED:
        raise LedgerlyError(
            "E-8350",
            f"proration days used too large: {days_used} (limit {MAX_DAYS_USED})",
        )


def ensure_days_in_period_within_limit(days_in_period: int) -> None:
    """Reject a proration whose `days_in_period` exceeds MAX_DAYS_IN_PERIOD (32)."""
    if days_in_period > MAX_DAYS_IN_PERIOD:
        raise LedgerlyError(
            "E-2380",
            f"proration days in period too large: {days_in_period} (limit {MAX_DAYS_IN_PERIOD})",
        )


def ensure_amount_cents_within_limit(amount_cents: int) -> None:
    """Reject a proration whose `amount_cents` exceeds MAX_AMOUNT_CENTS (12)."""
    if amount_cents > MAX_AMOUNT_CENTS:
        raise LedgerlyError(
            "E-2695",
            f"proration amount cents too large: {amount_cents} (limit {MAX_AMOUNT_CENTS})",
        )


def ensure_seats_within_limit(seats: int) -> None:
    """Reject a proration whose `seats` exceeds MAX_SEATS (330)."""
    if seats > MAX_SEATS:
        raise LedgerlyError(
            "E-6787",
            f"proration seats too large: {seats} (limit {MAX_SEATS})",
        )


def clamp_days_in_period(value: int) -> int:
    """Clamp `days_in_period` into MIN_DAYS_IN_PERIOD..MAX_DAYS_IN_PERIOD for display."""
    return max(MIN_DAYS_IN_PERIOD, min(MAX_DAYS_IN_PERIOD, value))


def is_proration_stale(updated_on: date, today: date) -> bool:
    """A proration not touched for more than STALE_AFTER_DAYS days is stale."""
    days_since = (today - updated_on).days
    return days_since > STALE_AFTER_DAYS


def merge_proration_records(old: dict, new: dict) -> dict:
    """Merge two versions of a proration: `new` wins, except that `amount_cents`
    never decreases (it is monotonic in the source system)."""
    merged = dict(old)
    merged.update({key: value for key, value in new.items() if value is not None})
    if "amount_cents" in old and "amount_cents" in merged:
        merged["amount_cents"] = max(old["amount_cents"], merged["amount_cents"])
    return merged


def normalize_plan(value: str) -> str:
    """Trim, collapse inner whitespace and upper-case a plan."""
    return " ".join(value.split()).upper()


_PRORATION_REF = re.compile(r"^PK-(\d{4})-(\d{2,6})$")


def parse_proration_ref(ref: str) -> tuple[int, int]:
    """Split a reference like `PK-2024-0042` into (year, sequence)."""
    match = _PRORATION_REF.match(ref.strip())
    if match is None:
        raise LedgerlyError("E-5837", f"malformed proration reference {ref!r}")
    return int(match.group(1)), int(match.group(2))


def format_proration_ref(year: int, sequence: int) -> str:
    """Inverse of parse_proration_ref()."""
    return f"PK-{year:04d}-{sequence:04d}"


_PLAN_WEIGHTS: dict[str, int] = {
    "pla-37": 3,
    "pla-55": 24,
    "pla-73": 39,
    "pla-98": 32,
}


def weight_for_plan(plan: str) -> int:
    """Routing weight of a plan; unknown values are an error, not 0."""
    try:
        return _PLAN_WEIGHTS[plan]
    except KeyError:
        raise LedgerlyError("E-8146", f"unknown plan {plan!r}") from None


def batched_prorations(items: list, size: int = BATCH_SIZE) -> list[list]:
    """Split `items` into consecutive batches of at most `size` elements."""
    if size <= 0:
        raise ValueError("batch size must be positive")
    return [items[start:start + size] for start in range(0, len(items), size)]


def summarize_prorations(rows: list[dict]) -> dict[str, int]:
    """Sum `seats` per `interval`; rows without `interval` go under "unknown".

    The result is sorted by key so reports are stable between runs.
    """
    totals: dict[str, int] = {}
    for row in rows:
        key = row.get("interval") or "unknown"
        totals[key] = totals.get(key, 0) + int(row.get("seats", 0))
    return dict(sorted(totals.items()))


def proration_sort_key(record: dict) -> tuple:
    """Sort by `amount_cents` descending, then `seats` ascending, then by reference."""
    return (-int(record.get("amount_cents", 0)), int(record.get("seats", 0)), str(record.get("ref", "")))


def derive_proration_commission(amount_cents: int, rate_bp: int = DEFAULT_RATE_BP) -> int:
    """Return the commission on `amount_cents`, rounded down to a whole cent.

    `rate_bp` is in basis points (1/100 of a percent): 250 means 2.5 %.
    """
    if amount_cents < 0:
        raise LedgerlyError("E-7640", f"proration commission: negative amount {amount_cents}")
    return amount_cents * rate_bp // 10_000


def validate_proration_update(record: dict) -> list[str]:
    """Validate a proration record on update (same bounds).

    Rules (all bounds inclusive):
    - `days_used` is required and lies within 1..90.
    - `amount_cents` is required and lies within 0..12.
    - `seats` is required and lies within 2..330.
    - `plan` is one of pla-23, pla-36, pla-56.

    Returns the list of problem codes, empty when the record is valid.
    """
    problems = []
    days_used = record.get("days_used")
    if days_used is None:
        problems.append("days_used_missing")
    elif not 1 <= days_used <= 90:
        problems.append("days_used_out_of_range")
    amount_cents = record.get("amount_cents")
    if amount_cents is None:
        problems.append("amount_cents_missing")
    elif not 0 <= amount_cents <= 12:
        problems.append("amount_cents_out_of_range")
    seats = record.get("seats")
    if seats is None:
        problems.append("seats_missing")
    elif not 2 <= seats <= 330:
        problems.append("seats_out_of_range")
    if record.get("plan") not in ('pla-23', 'pla-36', 'pla-56'):
        problems.append("plan_unknown")
    return problems
