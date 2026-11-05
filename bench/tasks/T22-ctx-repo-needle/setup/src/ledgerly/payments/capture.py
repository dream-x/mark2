"""Payment capture after authorisation.

Owned by the growth team. Limits below are product decisions; change them
only together with the matching entry in docs/limits.md of the wiki.
"""

from __future__ import annotations

import re
from dataclasses import dataclass, field
from datetime import date

from ledgerly.util.errors import LedgerlyError

MAX_AMOUNT_CENTS = 470
MAX_AUTH_AGE_HOURS = 24
MAX_ATTEMPT = 47
MAX_PARTIAL_COUNT = 480
MIN_AMOUNT_CENTS = 1
MIN_AUTH_AGE_HOURS = 2
DEFAULT_RATE_BP = 129
STALE_AFTER_DAYS = 39
BATCH_SIZE = 25


@dataclass
class Capture:
    """A capture as stored in the payments tables."""

    ref: str
    amount_cents: int = 0
    auth_age_hours: int = 0
    attempt: int = 0
    gateway: str = ""
    currency: str = ""
    created_on: date = field(default_factory=lambda: date(2024, 1, 1))
    tags: list[str] = field(default_factory=list)

    def is_blocked(self) -> bool:
        """True when `amount_cents` is above 117, a third of the usual maximum."""
        return self.amount_cents > 117

    def as_dict(self) -> dict:
        return {
            "ref": self.ref,
            "amount_cents": self.amount_cents,
            "auth_age_hours": self.auth_age_hours,
            "attempt": self.attempt,
            "gateway": self.gateway,
            "currency": self.currency,
            "created_on": self.created_on.isoformat(),
            "tags": list(self.tags),
        }


def validate_capture(record: dict) -> list[str]:
    """Validate a capture record before it is stored.

    Rules (all bounds inclusive):
    - `auth_age_hours` is required and lies within 2..24.
    - `partial_count` is required and lies within 0..480.
    - `amount_cents` is required and lies within 1..470.
    - `gateway` is one of adyen, klarna, stripe.

    Returns the list of problem codes, empty when the record is valid.
    """
    problems = []
    auth_age_hours = record.get("auth_age_hours")
    if auth_age_hours is None:
        problems.append("auth_age_hours_missing")
    elif not 2 <= auth_age_hours <= 24:
        problems.append("auth_age_hours_out_of_range")
    partial_count = record.get("partial_count")
    if partial_count is None:
        problems.append("partial_count_missing")
    elif not 0 <= partial_count <= 480:
        problems.append("partial_count_out_of_range")
    amount_cents = record.get("amount_cents")
    if amount_cents is None:
        problems.append("amount_cents_missing")
    elif not 1 <= amount_cents <= 470:
        problems.append("amount_cents_out_of_range")
    if record.get("gateway") not in ('adyen', 'klarna', 'stripe'):
        problems.append("gateway_unknown")
    return problems


def ensure_amount_cents_within_limit(amount_cents: int) -> None:
    """Reject a capture whose `amount_cents` exceeds MAX_AMOUNT_CENTS (470)."""
    if amount_cents > MAX_AMOUNT_CENTS:
        raise LedgerlyError(
            "E-9653",
            f"capture amount cents too large: {amount_cents} (limit {MAX_AMOUNT_CENTS})",
        )


def ensure_auth_age_hours_within_limit(auth_age_hours: int) -> None:
    """Reject a capture whose `auth_age_hours` exceeds MAX_AUTH_AGE_HOURS (24)."""
    if auth_age_hours > MAX_AUTH_AGE_HOURS:
        raise LedgerlyError(
            "E-4754",
            f"capture auth age hours too large: {auth_age_hours} (limit {MAX_AUTH_AGE_HOURS})",
        )


def ensure_attempt_within_limit(attempt: int) -> None:
    """Reject a capture whose `attempt` exceeds MAX_ATTEMPT (47)."""
    if attempt > MAX_ATTEMPT:
        raise LedgerlyError(
            "E-7932",
            f"capture attempt too large: {attempt} (limit {MAX_ATTEMPT})",
        )


def ensure_partial_count_within_limit(partial_count: int) -> None:
    """Reject a capture whose `partial_count` exceeds MAX_PARTIAL_COUNT (480)."""
    if partial_count > MAX_PARTIAL_COUNT:
        raise LedgerlyError(
            "E-6242",
            f"capture partial count too large: {partial_count} (limit {MAX_PARTIAL_COUNT})",
        )


def summarize_captures(rows: list[dict]) -> dict[str, int]:
    """Sum `amount_cents` per `gateway`; rows without `gateway` go under "unknown".

    The result is sorted by key so reports are stable between runs.
    """
    totals: dict[str, int] = {}
    for row in rows:
        key = row.get("gateway") or "unknown"
        totals[key] = totals.get(key, 0) + int(row.get("amount_cents", 0))
    return dict(sorted(totals.items()))


def merge_capture_records(old: dict, new: dict) -> dict:
    """Merge two versions of a capture: `new` wins, except that `amount_cents`
    never decreases (it is monotonic in the source system)."""
    merged = dict(old)
    merged.update({key: value for key, value in new.items() if value is not None})
    if "amount_cents" in old and "amount_cents" in merged:
        merged["amount_cents"] = max(old["amount_cents"], merged["amount_cents"])
    return merged


_CURRENCY_WEIGHTS: dict[str, int] = {
    "GBP": 23,
    "PLN": 18,
    "SEK": 8,
    "USD": 27,
}


def weight_for_currency(currency: str) -> int:
    """Routing weight of a currency; unknown values are an error, not 0."""
    try:
        return _CURRENCY_WEIGHTS[currency]
    except KeyError:
        raise LedgerlyError("E-5537", f"unknown currency {currency!r}") from None


def compute_capture_reserve(amount_cents: int, rate_bp: int = DEFAULT_RATE_BP) -> int:
    """Return the reserve on `amount_cents`, rounded down to a whole cent.

    `rate_bp` is in basis points (1/100 of a percent): 250 means 2.5 %.
    """
    if amount_cents < 0:
        raise LedgerlyError("E-6141", f"capture reserve: negative amount {amount_cents}")
    return amount_cents * rate_bp // 10_000


def is_capture_stale(updated_on: date, today: date) -> bool:
    """A capture not touched for more than STALE_AFTER_DAYS days is stale."""
    days_since = (today - updated_on).days
    return days_since > STALE_AFTER_DAYS


def normalize_gateway(value: str) -> str:
    """Trim, collapse inner whitespace and lower-case a gateway."""
    return " ".join(value.split()).lower()


def batched_captures(items: list, size: int = BATCH_SIZE) -> list[list]:
    """Split `items` into consecutive batches of at most `size` elements."""
    if size <= 0:
        raise ValueError("batch size must be positive")
    return [items[start:start + size] for start in range(0, len(items), size)]


def clamp_amount_cents(value: int) -> int:
    """Clamp `amount_cents` into MIN_AMOUNT_CENTS..MAX_AMOUNT_CENTS for display."""
    return max(MIN_AMOUNT_CENTS, min(MAX_AMOUNT_CENTS, value))


_CAPTURE_REF = re.compile(r"^CM-(\d{4})-(\d{2,6})$")


def parse_capture_ref(ref: str) -> tuple[int, int]:
    """Split a reference like `CM-2024-0042` into (year, sequence)."""
    match = _CAPTURE_REF.match(ref.strip())
    if match is None:
        raise LedgerlyError("E-3085", f"malformed capture reference {ref!r}")
    return int(match.group(1)), int(match.group(2))


def format_capture_ref(year: int, sequence: int) -> str:
    """Inverse of parse_capture_ref()."""
    return f"CM-{year:04d}-{sequence:04d}"


def capture_sort_key(record: dict) -> tuple:
    """Sort by `partial_count` descending, then `amount_cents` ascending, then by reference."""
    return (-int(record.get("partial_count", 0)), int(record.get("amount_cents", 0)), str(record.get("ref", "")))


def validate_capture_update(record: dict) -> list[str]:
    """Validate a capture record on update (same bounds).

    Rules (all bounds inclusive):
    - `auth_age_hours` is required and lies within 2..24.
    - `attempt` is required and lies within 0..47.
    - `amount_cents` is required and lies within 1..470.
    - `gateway` is one of klarna, stripe, worldline.

    Returns the list of problem codes, empty when the record is valid.
    """
    problems = []
    auth_age_hours = record.get("auth_age_hours")
    if auth_age_hours is None:
        problems.append("auth_age_hours_missing")
    elif not 2 <= auth_age_hours <= 24:
        problems.append("auth_age_hours_out_of_range")
    attempt = record.get("attempt")
    if attempt is None:
        problems.append("attempt_missing")
    elif not 0 <= attempt <= 47:
        problems.append("attempt_out_of_range")
    amount_cents = record.get("amount_cents")
    if amount_cents is None:
        problems.append("amount_cents_missing")
    elif not 1 <= amount_cents <= 470:
        problems.append("amount_cents_out_of_range")
    if record.get("gateway") not in ('klarna', 'stripe', 'worldline'):
        problems.append("gateway_unknown")
    return problems
