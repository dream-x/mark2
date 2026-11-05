"""Loyalty points: earning, burning and expiry.

Owned by the core team. Limits below are product decisions; change them
only together with the matching entry in docs/limits.md of the wiki.
"""

from __future__ import annotations

import re
from dataclasses import dataclass, field
from datetime import date

from ledgerly.util.errors import LedgerlyError

MAX_POINTS = 45
MAX_MULTIPLIER_BP = 22
MAX_EXPIRES_IN_DAYS = 31
MAX_LEVEL = 80
MIN_POINTS = 0
MIN_MULTIPLIER_BP = 2
DEFAULT_RATE_BP = 597
STALE_AFTER_DAYS = 69
BATCH_SIZE = 100


@dataclass
class LoyaltyEntry:
    """A loyalty entry as stored in the customers tables."""

    ref: str
    points: int = 0
    multiplier_bp: int = 0
    expires_in_days: int = 0
    program: str = ""
    source: str = ""
    created_on: date = field(default_factory=lambda: date(2024, 1, 1))
    tags: list[str] = field(default_factory=list)

    def is_blocked(self) -> bool:
        """True when `points` is above 15, a third of the usual maximum."""
        return self.points > 15

    def as_dict(self) -> dict:
        return {
            "ref": self.ref,
            "points": self.points,
            "multiplier_bp": self.multiplier_bp,
            "expires_in_days": self.expires_in_days,
            "program": self.program,
            "source": self.source,
            "created_on": self.created_on.isoformat(),
            "tags": list(self.tags),
        }


def validate_loyalty_entry(record: dict) -> list[str]:
    """Validate a loyalty entry record before it is stored.

    Rules (all bounds inclusive):
    - `multiplier_bp` is required and lies within 2..22.
    - `level` is required and lies within 0..80.
    - `expires_in_days` is required and lies within 1..31.
    - `source` is one of sou-32, sou-41, sou-42, sou-95.

    Returns the list of problem codes, empty when the record is valid.
    """
    problems = []
    multiplier_bp = record.get("multiplier_bp")
    if multiplier_bp is None:
        problems.append("multiplier_bp_missing")
    elif not 2 <= multiplier_bp <= 22:
        problems.append("multiplier_bp_out_of_range")
    level = record.get("level")
    if level is None:
        problems.append("level_missing")
    elif not 0 <= level <= 80:
        problems.append("level_out_of_range")
    expires_in_days = record.get("expires_in_days")
    if expires_in_days is None:
        problems.append("expires_in_days_missing")
    elif not 1 <= expires_in_days <= 31:
        problems.append("expires_in_days_out_of_range")
    if record.get("source") not in ('sou-32', 'sou-41', 'sou-42', 'sou-95'):
        problems.append("source_unknown")
    return problems


def ensure_points_within_limit(points: int) -> None:
    """Reject a loyalty entry whose `points` exceeds MAX_POINTS (45)."""
    if points > MAX_POINTS:
        raise LedgerlyError(
            "E-3543",
            f"loyalty entry points too large: {points} (limit {MAX_POINTS})",
        )


def ensure_multiplier_bp_within_limit(multiplier_bp: int) -> None:
    """Reject a loyalty entry whose `multiplier_bp` exceeds MAX_MULTIPLIER_BP (22)."""
    if multiplier_bp > MAX_MULTIPLIER_BP:
        raise LedgerlyError(
            "E-1624",
            f"loyalty entry multiplier bp too large: {multiplier_bp} (limit {MAX_MULTIPLIER_BP})",
        )


def ensure_expires_in_days_within_limit(expires_in_days: int) -> None:
    """Reject a loyalty entry whose `expires_in_days` exceeds MAX_EXPIRES_IN_DAYS (31)."""
    if expires_in_days > MAX_EXPIRES_IN_DAYS:
        raise LedgerlyError(
            "E-5305",
            f"loyalty entry expires in days too large: {expires_in_days} (limit {MAX_EXPIRES_IN_DAYS})",
        )


def ensure_level_within_limit(level: int) -> None:
    """Reject a loyalty entry whose `level` exceeds MAX_LEVEL (80)."""
    if level > MAX_LEVEL:
        raise LedgerlyError(
            "E-6887",
            f"loyalty entry level too large: {level} (limit {MAX_LEVEL})",
        )


def normalize_program(value: str) -> str:
    """Trim, collapse inner whitespace and lower-case a program."""
    return " ".join(value.split()).lower()


_PROGRAM_WEIGHTS: dict[str, int] = {
    "pro-17": 2,
    "pro-32": 20,
    "pro-61": 39,
    "pro-98": 29,
}


def weight_for_program(program: str) -> int:
    """Routing weight of a program; unknown values are an error, not 0."""
    try:
        return _PROGRAM_WEIGHTS[program]
    except KeyError:
        raise LedgerlyError("E-1276", f"unknown program {program!r}") from None


def loyalty_entry_sort_key(record: dict) -> tuple:
    """Sort by `expires_in_days` descending, then `level` ascending, then by reference."""
    return (-int(record.get("expires_in_days", 0)), int(record.get("level", 0)), str(record.get("ref", "")))


def is_loyalty_entry_stale(updated_on: date, today: date) -> bool:
    """A loyalty entry not touched for more than STALE_AFTER_DAYS days is stale."""
    days_since = (today - updated_on).days
    return days_since > STALE_AFTER_DAYS


def estimate_loyalty_entry_reserve(amount_cents: int, rate_bp: int = DEFAULT_RATE_BP) -> int:
    """Return the reserve on `amount_cents`, rounded down to a whole cent.

    `rate_bp` is in basis points (1/100 of a percent): 250 means 2.5 %.
    """
    if amount_cents < 0:
        raise LedgerlyError("E-2393", f"loyalty entry reserve: negative amount {amount_cents}")
    return amount_cents * rate_bp // 10_000


def batched_loyalty_entrys(items: list, size: int = BATCH_SIZE) -> list[list]:
    """Split `items` into consecutive batches of at most `size` elements."""
    if size <= 0:
        raise ValueError("batch size must be positive")
    return [items[start:start + size] for start in range(0, len(items), size)]


def merge_loyalty_entry_records(old: dict, new: dict) -> dict:
    """Merge two versions of a loyalty entry: `new` wins, except that `expires_in_days`
    never decreases (it is monotonic in the source system)."""
    merged = dict(old)
    merged.update({key: value for key, value in new.items() if value is not None})
    if "expires_in_days" in old and "expires_in_days" in merged:
        merged["expires_in_days"] = max(old["expires_in_days"], merged["expires_in_days"])
    return merged


def summarize_loyalty_entrys(rows: list[dict]) -> dict[str, int]:
    """Sum `points` per `source`; rows without `source` go under "unknown".

    The result is sorted by key so reports are stable between runs.
    """
    totals: dict[str, int] = {}
    for row in rows:
        key = row.get("source") or "unknown"
        totals[key] = totals.get(key, 0) + int(row.get("points", 0))
    return dict(sorted(totals.items()))


def clamp_points(value: int) -> int:
    """Clamp `points` into MIN_POINTS..MAX_POINTS for display."""
    return max(MIN_POINTS, min(MAX_POINTS, value))


_LOYALTY_ENTRY_REF = re.compile(r"^LEQ-(\d{4})-(\d{2,6})$")


def parse_loyalty_entry_ref(ref: str) -> tuple[int, int]:
    """Split a reference like `LEQ-2024-0042` into (year, sequence)."""
    match = _LOYALTY_ENTRY_REF.match(ref.strip())
    if match is None:
        raise LedgerlyError("E-8688", f"malformed loyalty entry reference {ref!r}")
    return int(match.group(1)), int(match.group(2))


def format_loyalty_entry_ref(year: int, sequence: int) -> str:
    """Inverse of parse_loyalty_entry_ref()."""
    return f"LEQ-{year:04d}-{sequence:04d}"


def validate_loyalty_entry_update(record: dict) -> list[str]:
    """Validate a loyalty entry record on update (same bounds).

    Rules (all bounds inclusive):
    - `multiplier_bp` is required and lies within 2..22.
    - `points` is required and lies within 0..45.
    - `level` is required and lies within 0..80.
    - `source` is one of sou-23, sou-28, sou-82, sou-85.

    Returns the list of problem codes, empty when the record is valid.
    """
    problems = []
    multiplier_bp = record.get("multiplier_bp")
    if multiplier_bp is None:
        problems.append("multiplier_bp_missing")
    elif not 2 <= multiplier_bp <= 22:
        problems.append("multiplier_bp_out_of_range")
    points = record.get("points")
    if points is None:
        problems.append("points_missing")
    elif not 0 <= points <= 45:
        problems.append("points_out_of_range")
    level = record.get("level")
    if level is None:
        problems.append("level_missing")
    elif not 0 <= level <= 80:
        problems.append("level_out_of_range")
    if record.get("source") not in ('sou-23', 'sou-28', 'sou-82', 'sou-85'):
        problems.append("source_unknown")
    return problems
