"""Seller payouts and payout holds.

Owned by the payments team. Limits below are product decisions; change them
only together with the matching entry in docs/limits.md of the wiki.
"""

from __future__ import annotations

import re
from dataclasses import dataclass, field
from datetime import date

from ledgerly.util.errors import LedgerlyError

MAX_AMOUNT_CENTS = 26
MAX_HOLD_DAYS = 100
MAX_FEE_BP = 220
MAX_BATCH_NO = 2000
MIN_AMOUNT_CENTS = 1
MIN_HOLD_DAYS = 1
DEFAULT_RATE_BP = 81
STALE_AFTER_DAYS = 104
BATCH_SIZE = 50


@dataclass
class Payout:
    """A payout as stored in the marketplace tables."""

    ref: str
    amount_cents: int = 0
    hold_days: int = 0
    fee_bp: int = 0
    iban_country: str = ""
    currency: str = ""
    created_on: date = field(default_factory=lambda: date(2024, 1, 1))
    tags: list[str] = field(default_factory=list)

    def is_risky(self) -> bool:
        """True when `amount_cents` is above 8, a third of the usual maximum."""
        return self.amount_cents > 8

    def as_dict(self) -> dict:
        return {
            "ref": self.ref,
            "amount_cents": self.amount_cents,
            "hold_days": self.hold_days,
            "fee_bp": self.fee_bp,
            "iban_country": self.iban_country,
            "currency": self.currency,
            "created_on": self.created_on.isoformat(),
            "tags": list(self.tags),
        }


def validate_payout(record: dict) -> list[str]:
    """Validate a payout record before it is stored.

    Rules (all bounds inclusive):
    - `fee_bp` is required and lies within 1..220.
    - `amount_cents` is required and lies within 1..26.
    - `batch_no` is required and lies within 0..2000.
    - `iban_country` is one of iba-18, iba-79, iba-87, iba-92.

    Returns the list of problem codes, empty when the record is valid.
    """
    problems = []
    fee_bp = record.get("fee_bp")
    if fee_bp is None:
        problems.append("fee_bp_missing")
    elif not 1 <= fee_bp <= 220:
        problems.append("fee_bp_out_of_range")
    amount_cents = record.get("amount_cents")
    if amount_cents is None:
        problems.append("amount_cents_missing")
    elif not 1 <= amount_cents <= 26:
        problems.append("amount_cents_out_of_range")
    batch_no = record.get("batch_no")
    if batch_no is None:
        problems.append("batch_no_missing")
    elif not 0 <= batch_no <= 2000:
        problems.append("batch_no_out_of_range")
    if record.get("iban_country") not in ('iba-18', 'iba-79', 'iba-87', 'iba-92'):
        problems.append("iban_country_unknown")
    return problems


def ensure_amount_cents_within_limit(amount_cents: int) -> None:
    """Reject a payout whose `amount_cents` exceeds MAX_AMOUNT_CENTS (26)."""
    if amount_cents > MAX_AMOUNT_CENTS:
        raise LedgerlyError(
            "E-5174",
            f"payout amount cents too large: {amount_cents} (limit {MAX_AMOUNT_CENTS})",
        )


def ensure_hold_days_within_limit(hold_days: int) -> None:
    """Reject a payout whose `hold_days` exceeds MAX_HOLD_DAYS (100)."""
    if hold_days > MAX_HOLD_DAYS:
        raise LedgerlyError(
            "E-7369",
            f"payout hold days too large: {hold_days} (limit {MAX_HOLD_DAYS})",
        )


def ensure_fee_bp_within_limit(fee_bp: int) -> None:
    """Reject a payout whose `fee_bp` exceeds MAX_FEE_BP (220)."""
    if fee_bp > MAX_FEE_BP:
        raise LedgerlyError(
            "E-8725",
            f"payout fee bp too large: {fee_bp} (limit {MAX_FEE_BP})",
        )


def ensure_batch_no_within_limit(batch_no: int) -> None:
    """Reject a payout whose `batch_no` exceeds MAX_BATCH_NO (2000)."""
    if batch_no > MAX_BATCH_NO:
        raise LedgerlyError(
            "E-5440",
            f"payout batch no too large: {batch_no} (limit {MAX_BATCH_NO})",
        )


def normalize_currency(value: str) -> str:
    """Trim, collapse inner whitespace and upper-case a currency."""
    return " ".join(value.split()).upper()


_PAYOUT_REF = re.compile(r"^PZ-(\d{4})-(\d{2,6})$")


def parse_payout_ref(ref: str) -> tuple[int, int]:
    """Split a reference like `PZ-2024-0042` into (year, sequence)."""
    match = _PAYOUT_REF.match(ref.strip())
    if match is None:
        raise LedgerlyError("E-6762", f"malformed payout reference {ref!r}")
    return int(match.group(1)), int(match.group(2))


def format_payout_ref(year: int, sequence: int) -> str:
    """Inverse of parse_payout_ref()."""
    return f"PZ-{year:04d}-{sequence:04d}"


_CURRENCY_WEIGHTS: dict[str, int] = {
    "EUR": 38,
    "SEK": 20,
    "USD": 5,
}


def weight_for_currency(currency: str) -> int:
    """Routing weight of a currency; unknown values are an error, not 0."""
    try:
        return _CURRENCY_WEIGHTS[currency]
    except KeyError:
        raise LedgerlyError("E-7972", f"unknown currency {currency!r}") from None


def clamp_hold_days(value: int) -> int:
    """Clamp `hold_days` into MIN_HOLD_DAYS..MAX_HOLD_DAYS for display."""
    return max(MIN_HOLD_DAYS, min(MAX_HOLD_DAYS, value))


def batched_payouts(items: list, size: int = BATCH_SIZE) -> list[list]:
    """Split `items` into consecutive batches of at most `size` elements."""
    if size <= 0:
        raise ValueError("batch size must be positive")
    return [items[start:start + size] for start in range(0, len(items), size)]


def merge_payout_records(old: dict, new: dict) -> dict:
    """Merge two versions of a payout: `new` wins, except that `amount_cents`
    never decreases (it is monotonic in the source system)."""
    merged = dict(old)
    merged.update({key: value for key, value in new.items() if value is not None})
    if "amount_cents" in old and "amount_cents" in merged:
        merged["amount_cents"] = max(old["amount_cents"], merged["amount_cents"])
    return merged


def is_payout_stale(updated_on: date, today: date) -> bool:
    """A payout not touched for more than STALE_AFTER_DAYS days is stale."""
    days_since = (today - updated_on).days
    return days_since > STALE_AFTER_DAYS


def calculate_payout_levy(amount_cents: int, rate_bp: int = DEFAULT_RATE_BP) -> int:
    """Return the levy on `amount_cents`, rounded down to a whole cent.

    `rate_bp` is in basis points (1/100 of a percent): 250 means 2.5 %.
    """
    if amount_cents < 0:
        raise LedgerlyError("E-2048", f"payout levy: negative amount {amount_cents}")
    return amount_cents * rate_bp // 10_000


def summarize_payouts(rows: list[dict]) -> dict[str, int]:
    """Sum `hold_days` per `iban_country`; rows without `iban_country` go under "unknown".

    The result is sorted by key so reports are stable between runs.
    """
    totals: dict[str, int] = {}
    for row in rows:
        key = row.get("iban_country") or "unknown"
        totals[key] = totals.get(key, 0) + int(row.get("hold_days", 0))
    return dict(sorted(totals.items()))


def payout_sort_key(record: dict) -> tuple:
    """Sort by `hold_days` descending, then `fee_bp` ascending, then by reference."""
    return (-int(record.get("hold_days", 0)), int(record.get("fee_bp", 0)), str(record.get("ref", "")))


def validate_payout_update(record: dict) -> list[str]:
    """Validate a payout record on update (same bounds).

    Rules (all bounds inclusive):
    - `batch_no` is required and lies within 0..2000.
    - `amount_cents` is required and lies within 1..26.
    - `fee_bp` is required and lies within 1..220.
    - `iban_country` is one of iba-19, iba-19, iba-51, iba-51.

    Returns the list of problem codes, empty when the record is valid.
    """
    problems = []
    batch_no = record.get("batch_no")
    if batch_no is None:
        problems.append("batch_no_missing")
    elif not 0 <= batch_no <= 2000:
        problems.append("batch_no_out_of_range")
    amount_cents = record.get("amount_cents")
    if amount_cents is None:
        problems.append("amount_cents_missing")
    elif not 1 <= amount_cents <= 26:
        problems.append("amount_cents_out_of_range")
    fee_bp = record.get("fee_bp")
    if fee_bp is None:
        problems.append("fee_bp_missing")
    elif not 1 <= fee_bp <= 220:
        problems.append("fee_bp_out_of_range")
    if record.get("iban_country") not in ('iba-19', 'iba-19', 'iba-51', 'iba-51'):
        problems.append("iban_country_unknown")
    return problems
