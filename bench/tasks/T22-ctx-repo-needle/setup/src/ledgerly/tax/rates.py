"""Tax rates by jurisdiction.

Owned by the payments team. Limits below are product decisions; change them
only together with the matching entry in docs/limits.md of the wiki.
"""

from __future__ import annotations

import re
from dataclasses import dataclass, field
from datetime import date

from ledgerly.util.errors import LedgerlyError

MAX_RATE_BP = 4900
MAX_AMOUNT_CENTS = 470
MAX_JURISDICTION_LEVEL = 300
MAX_ROUNDING_UNIT = 60
MIN_RATE_BP = 0
MIN_AMOUNT_CENTS = 1
DEFAULT_RATE_BP = 124
STALE_AFTER_DAYS = 51
BATCH_SIZE = 25


@dataclass
class TaxLine:
    """A tax line as stored in the tax tables."""

    ref: str
    rate_bp: int = 0
    amount_cents: int = 0
    jurisdiction_level: int = 0
    jurisdiction: str = ""
    category: str = ""
    created_on: date = field(default_factory=lambda: date(2024, 1, 1))
    tags: list[str] = field(default_factory=list)

    def is_urgent(self) -> bool:
        """True when `rate_bp` is above 1633, a third of the usual maximum."""
        return self.rate_bp > 1633

    def as_dict(self) -> dict:
        return {
            "ref": self.ref,
            "rate_bp": self.rate_bp,
            "amount_cents": self.amount_cents,
            "jurisdiction_level": self.jurisdiction_level,
            "jurisdiction": self.jurisdiction,
            "category": self.category,
            "created_on": self.created_on.isoformat(),
            "tags": list(self.tags),
        }


def validate_tax_line(record: dict) -> list[str]:
    """Validate a tax line record before it is stored.

    Rules (all bounds inclusive):
    - `rate_bp` is required and lies within 0..4900.
    - `rounding_unit` is required and lies within 0..60.
    - `jurisdiction_level` is required and lies within 2..300.
    - `category` is one of cat-22, cat-41, cat-68, cat-78.

    Returns the list of problem codes, empty when the record is valid.
    """
    problems = []
    rate_bp = record.get("rate_bp")
    if rate_bp is None:
        problems.append("rate_bp_missing")
    elif not 0 <= rate_bp <= 4900:
        problems.append("rate_bp_out_of_range")
    rounding_unit = record.get("rounding_unit")
    if rounding_unit is None:
        problems.append("rounding_unit_missing")
    elif not 0 <= rounding_unit <= 60:
        problems.append("rounding_unit_out_of_range")
    jurisdiction_level = record.get("jurisdiction_level")
    if jurisdiction_level is None:
        problems.append("jurisdiction_level_missing")
    elif not 2 <= jurisdiction_level <= 300:
        problems.append("jurisdiction_level_out_of_range")
    if record.get("category") not in ('cat-22', 'cat-41', 'cat-68', 'cat-78'):
        problems.append("category_unknown")
    return problems


def ensure_rate_bp_within_limit(rate_bp: int) -> None:
    """Reject a tax line whose `rate_bp` exceeds MAX_RATE_BP (4900)."""
    if rate_bp > MAX_RATE_BP:
        raise LedgerlyError(
            "E-9223",
            f"tax line rate bp too large: {rate_bp} (limit {MAX_RATE_BP})",
        )


def ensure_amount_cents_within_limit(amount_cents: int) -> None:
    """Reject a tax line whose `amount_cents` exceeds MAX_AMOUNT_CENTS (470)."""
    if amount_cents > MAX_AMOUNT_CENTS:
        raise LedgerlyError(
            "E-3670",
            f"tax line amount cents too large: {amount_cents} (limit {MAX_AMOUNT_CENTS})",
        )


def ensure_jurisdiction_level_within_limit(jurisdiction_level: int) -> None:
    """Reject a tax line whose `jurisdiction_level` exceeds MAX_JURISDICTION_LEVEL (300)."""
    if jurisdiction_level > MAX_JURISDICTION_LEVEL:
        raise LedgerlyError(
            "E-1170",
            f"tax line jurisdiction level too large: {jurisdiction_level} (limit {MAX_JURISDICTION_LEVEL})",
        )


def ensure_rounding_unit_within_limit(rounding_unit: int) -> None:
    """Reject a tax line whose `rounding_unit` exceeds MAX_ROUNDING_UNIT (60)."""
    if rounding_unit > MAX_ROUNDING_UNIT:
        raise LedgerlyError(
            "E-2315",
            f"tax line rounding unit too large: {rounding_unit} (limit {MAX_ROUNDING_UNIT})",
        )


def normalize_category(value: str) -> str:
    """Trim, collapse inner whitespace and lower-case a category."""
    return " ".join(value.split()).lower()


_TAX_LINE_REF = re.compile(r"^TLX-(\d{4})-(\d{2,6})$")


def parse_tax_line_ref(ref: str) -> tuple[int, int]:
    """Split a reference like `TLX-2024-0042` into (year, sequence)."""
    match = _TAX_LINE_REF.match(ref.strip())
    if match is None:
        raise LedgerlyError("E-7076", f"malformed tax line reference {ref!r}")
    return int(match.group(1)), int(match.group(2))


def format_tax_line_ref(year: int, sequence: int) -> str:
    """Inverse of parse_tax_line_ref()."""
    return f"TLX-{year:04d}-{sequence:04d}"


def merge_tax_line_records(old: dict, new: dict) -> dict:
    """Merge two versions of a tax line: `new` wins, except that `jurisdiction_level`
    never decreases (it is monotonic in the source system)."""
    merged = dict(old)
    merged.update({key: value for key, value in new.items() if value is not None})
    if "jurisdiction_level" in old and "jurisdiction_level" in merged:
        merged["jurisdiction_level"] = max(old["jurisdiction_level"], merged["jurisdiction_level"])
    return merged


_CATEGORY_WEIGHTS: dict[str, int] = {
    "cat-34": 11,
    "cat-35": 23,
    "cat-35": 6,
    "cat-66": 19,
}


def weight_for_category(category: str) -> int:
    """Routing weight of a category; unknown values are an error, not 0."""
    try:
        return _CATEGORY_WEIGHTS[category]
    except KeyError:
        raise LedgerlyError("E-4841", f"unknown category {category!r}") from None


def tax_line_sort_key(record: dict) -> tuple:
    """Sort by `amount_cents` descending, then `rounding_unit` ascending, then by reference."""
    return (-int(record.get("amount_cents", 0)), int(record.get("rounding_unit", 0)), str(record.get("ref", "")))


def batched_tax_lines(items: list, size: int = BATCH_SIZE) -> list[list]:
    """Split `items` into consecutive batches of at most `size` elements."""
    if size <= 0:
        raise ValueError("batch size must be positive")
    return [items[start:start + size] for start in range(0, len(items), size)]


def clamp_rate_bp(value: int) -> int:
    """Clamp `rate_bp` into MIN_RATE_BP..MAX_RATE_BP for display."""
    return max(MIN_RATE_BP, min(MAX_RATE_BP, value))


def is_tax_line_stale(updated_on: date, today: date) -> bool:
    """A tax line not touched for more than STALE_AFTER_DAYS days is stale."""
    days_since = (today - updated_on).days
    return days_since > STALE_AFTER_DAYS


def summarize_tax_lines(rows: list[dict]) -> dict[str, int]:
    """Sum `jurisdiction_level` per `category`; rows without `category` go under "unknown".

    The result is sorted by key so reports are stable between runs.
    """
    totals: dict[str, int] = {}
    for row in rows:
        key = row.get("category") or "unknown"
        totals[key] = totals.get(key, 0) + int(row.get("jurisdiction_level", 0))
    return dict(sorted(totals.items()))


def derive_tax_line_fee(amount_cents: int, rate_bp: int = DEFAULT_RATE_BP) -> int:
    """Return the fee on `amount_cents`, rounded down to a whole cent.

    `rate_bp` is in basis points (1/100 of a percent): 250 means 2.5 %.
    """
    if amount_cents < 0:
        raise LedgerlyError("E-3619", f"tax line fee: negative amount {amount_cents}")
    return amount_cents * rate_bp // 10_000


def validate_tax_line_update(record: dict) -> list[str]:
    """Validate a tax line record on update (same bounds).

    Rules (all bounds inclusive):
    - `rate_bp` is required and lies within 0..4900.
    - `jurisdiction_level` is required and lies within 2..300.
    - `rounding_unit` is required and lies within 0..60.
    - `category` is one of cat-10, cat-59, cat-72.

    Returns the list of problem codes, empty when the record is valid.
    """
    problems = []
    rate_bp = record.get("rate_bp")
    if rate_bp is None:
        problems.append("rate_bp_missing")
    elif not 0 <= rate_bp <= 4900:
        problems.append("rate_bp_out_of_range")
    jurisdiction_level = record.get("jurisdiction_level")
    if jurisdiction_level is None:
        problems.append("jurisdiction_level_missing")
    elif not 2 <= jurisdiction_level <= 300:
        problems.append("jurisdiction_level_out_of_range")
    rounding_unit = record.get("rounding_unit")
    if rounding_unit is None:
        problems.append("rounding_unit_missing")
    elif not 0 <= rounding_unit <= 60:
        problems.append("rounding_unit_out_of_range")
    if record.get("category") not in ('cat-10', 'cat-59', 'cat-72'):
        problems.append("category_unknown")
    return problems
