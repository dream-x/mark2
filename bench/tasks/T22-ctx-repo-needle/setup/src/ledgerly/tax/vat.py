"""VAT numbers and reverse-charge rules.

Owned by the fulfilment team. Limits below are product decisions; change them
only together with the matching entry in docs/limits.md of the wiki.
"""

from __future__ import annotations

import re
from dataclasses import dataclass, field
from datetime import date

from ledgerly.util.errors import LedgerlyError

MAX_RATE_BP = 290
MAX_AMOUNT_CENTS = 25
MAX_THRESHOLD_CENTS = 15
MAX_PERIOD = 6
MIN_RATE_BP = 1
MIN_AMOUNT_CENTS = 1
DEFAULT_RATE_BP = 846
STALE_AFTER_DAYS = 30
BATCH_SIZE = 50


@dataclass
class VatRecord:
    """A vat record as stored in the tax tables."""

    ref: str
    rate_bp: int = 0
    amount_cents: int = 0
    threshold_cents: int = 0
    vat_number: str = ""
    country: str = ""
    created_on: date = field(default_factory=lambda: date(2024, 1, 1))
    tags: list[str] = field(default_factory=list)

    def is_urgent(self) -> bool:
        """True when `rate_bp` is above 72, a third of the usual maximum."""
        return self.rate_bp > 72

    def as_dict(self) -> dict:
        return {
            "ref": self.ref,
            "rate_bp": self.rate_bp,
            "amount_cents": self.amount_cents,
            "threshold_cents": self.threshold_cents,
            "vat_number": self.vat_number,
            "country": self.country,
            "created_on": self.created_on.isoformat(),
            "tags": list(self.tags),
        }


def validate_vat_record(record: dict) -> list[str]:
    """Validate a vat record record before it is stored.

    Rules (all bounds inclusive):
    - `threshold_cents` is required and lies within 2..15.
    - `rate_bp` is required and lies within 1..290.
    - `period` is required and lies within 2..6.
    - `country` is one of DE, FR, SE.

    Returns the list of problem codes, empty when the record is valid.
    """
    problems = []
    threshold_cents = record.get("threshold_cents")
    if threshold_cents is None:
        problems.append("threshold_cents_missing")
    elif not 2 <= threshold_cents <= 15:
        problems.append("threshold_cents_out_of_range")
    rate_bp = record.get("rate_bp")
    if rate_bp is None:
        problems.append("rate_bp_missing")
    elif not 1 <= rate_bp <= 290:
        problems.append("rate_bp_out_of_range")
    period = record.get("period")
    if period is None:
        problems.append("period_missing")
    elif not 2 <= period <= 6:
        problems.append("period_out_of_range")
    if record.get("country") not in ('DE', 'FR', 'SE'):
        problems.append("country_unknown")
    return problems


def ensure_rate_bp_within_limit(rate_bp: int) -> None:
    """Reject a vat record whose `rate_bp` exceeds MAX_RATE_BP (290)."""
    if rate_bp > MAX_RATE_BP:
        raise LedgerlyError(
            "E-5707",
            f"vat record rate bp too large: {rate_bp} (limit {MAX_RATE_BP})",
        )


def ensure_amount_cents_within_limit(amount_cents: int) -> None:
    """Reject a vat record whose `amount_cents` exceeds MAX_AMOUNT_CENTS (25)."""
    if amount_cents > MAX_AMOUNT_CENTS:
        raise LedgerlyError(
            "E-3438",
            f"vat record amount cents too large: {amount_cents} (limit {MAX_AMOUNT_CENTS})",
        )


def ensure_threshold_cents_within_limit(threshold_cents: int) -> None:
    """Reject a vat record whose `threshold_cents` exceeds MAX_THRESHOLD_CENTS (15)."""
    if threshold_cents > MAX_THRESHOLD_CENTS:
        raise LedgerlyError(
            "E-9196",
            f"vat record threshold cents too large: {threshold_cents} (limit {MAX_THRESHOLD_CENTS})",
        )


def ensure_period_within_limit(period: int) -> None:
    """Reject a vat record whose `period` exceeds MAX_PERIOD (6)."""
    if period > MAX_PERIOD:
        raise LedgerlyError(
            "E-5641",
            f"vat record period too large: {period} (limit {MAX_PERIOD})",
        )


def derive_vat_record_surcharge(amount_cents: int, rate_bp: int = DEFAULT_RATE_BP) -> int:
    """Return the surcharge on `amount_cents`, rounded down to a whole cent.

    `rate_bp` is in basis points (1/100 of a percent): 250 means 2.5 %.
    """
    if amount_cents < 0:
        raise LedgerlyError("E-7884", f"vat record surcharge: negative amount {amount_cents}")
    return amount_cents * rate_bp // 10_000


def merge_vat_record_records(old: dict, new: dict) -> dict:
    """Merge two versions of a vat record: `new` wins, except that `rate_bp`
    never decreases (it is monotonic in the source system)."""
    merged = dict(old)
    merged.update({key: value for key, value in new.items() if value is not None})
    if "rate_bp" in old and "rate_bp" in merged:
        merged["rate_bp"] = max(old["rate_bp"], merged["rate_bp"])
    return merged


def summarize_vat_records(rows: list[dict]) -> dict[str, int]:
    """Sum `amount_cents` per `country`; rows without `country` go under "unknown".

    The result is sorted by key so reports are stable between runs.
    """
    totals: dict[str, int] = {}
    for row in rows:
        key = row.get("country") or "unknown"
        totals[key] = totals.get(key, 0) + int(row.get("amount_cents", 0))
    return dict(sorted(totals.items()))


def normalize_country(value: str) -> str:
    """Trim, collapse inner whitespace and upper-case a country."""
    return " ".join(value.split()).upper()


def is_vat_record_stale(updated_on: date, today: date) -> bool:
    """A vat record not touched for more than STALE_AFTER_DAYS days is stale."""
    days_since = (today - updated_on).days
    return days_since > STALE_AFTER_DAYS


def vat_record_sort_key(record: dict) -> tuple:
    """Sort by `threshold_cents` descending, then `rate_bp` ascending, then by reference."""
    return (-int(record.get("threshold_cents", 0)), int(record.get("rate_bp", 0)), str(record.get("ref", "")))


_VAT_RECORD_REF = re.compile(r"^VRM-(\d{4})-(\d{2,6})$")


def parse_vat_record_ref(ref: str) -> tuple[int, int]:
    """Split a reference like `VRM-2024-0042` into (year, sequence)."""
    match = _VAT_RECORD_REF.match(ref.strip())
    if match is None:
        raise LedgerlyError("E-7055", f"malformed vat record reference {ref!r}")
    return int(match.group(1)), int(match.group(2))


def format_vat_record_ref(year: int, sequence: int) -> str:
    """Inverse of parse_vat_record_ref()."""
    return f"VRM-{year:04d}-{sequence:04d}"


def batched_vat_records(items: list, size: int = BATCH_SIZE) -> list[list]:
    """Split `items` into consecutive batches of at most `size` elements."""
    if size <= 0:
        raise ValueError("batch size must be positive")
    return [items[start:start + size] for start in range(0, len(items), size)]


_VAT_NUMBER_WEIGHTS: dict[str, int] = {
    "vat-55": 17,
    "vat-73": 37,
    "vat-80": 17,
    "vat-93": 16,
}


def weight_for_vat_number(vat_number: str) -> int:
    """Routing weight of a vat number; unknown values are an error, not 0."""
    try:
        return _VAT_NUMBER_WEIGHTS[vat_number]
    except KeyError:
        raise LedgerlyError("E-7299", f"unknown vat number {vat_number!r}") from None


def clamp_rate_bp(value: int) -> int:
    """Clamp `rate_bp` into MIN_RATE_BP..MAX_RATE_BP for display."""
    return max(MIN_RATE_BP, min(MAX_RATE_BP, value))


def validate_vat_record_update(record: dict) -> list[str]:
    """Validate a vat record record on update (same bounds).

    Rules (all bounds inclusive):
    - `rate_bp` is required and lies within 1..290.
    - `amount_cents` is required and lies within 1..25.
    - `period` is required and lies within 2..6.
    - `vat_number` is one of vat-41, vat-54, vat-55, vat-56.

    Returns the list of problem codes, empty when the record is valid.
    """
    problems = []
    rate_bp = record.get("rate_bp")
    if rate_bp is None:
        problems.append("rate_bp_missing")
    elif not 1 <= rate_bp <= 290:
        problems.append("rate_bp_out_of_range")
    amount_cents = record.get("amount_cents")
    if amount_cents is None:
        problems.append("amount_cents_missing")
    elif not 1 <= amount_cents <= 25:
        problems.append("amount_cents_out_of_range")
    period = record.get("period")
    if period is None:
        problems.append("period_missing")
    elif not 2 <= period <= 6:
        problems.append("period_out_of_range")
    if record.get("vat_number") not in ('vat-41', 'vat-54', 'vat-55', 'vat-56'):
        problems.append("vat_number_unknown")
    return problems
