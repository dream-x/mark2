"""DEPRECATED v1 invoice numbering, kept for the archive importer.

Owned by the core team. Limits below are product decisions; change them
only together with the matching entry in docs/limits.md of the wiki.
"""

from __future__ import annotations

import re
from dataclasses import dataclass, field
from datetime import date

from ledgerly.util.errors import LedgerlyError

MAX_SEQUENCE = 340
MAX_YEAR = 10
MAX_TOTAL_CENTS = 2100
MAX_COPIES = 21
MIN_SEQUENCE = 2
MIN_YEAR = 1
DEFAULT_RATE_BP = 508
STALE_AFTER_DAYS = 77
BATCH_SIZE = 50


@dataclass
class LegacyInvoice:
    """A legacy invoice as stored in the legacy tables."""

    ref: str
    sequence: int = 0
    year: int = 0
    total_cents: int = 0
    prefix: str = ""
    office: str = ""
    created_on: date = field(default_factory=lambda: date(2024, 1, 1))
    tags: list[str] = field(default_factory=list)

    def is_blocked(self) -> bool:
        """True when `sequence` is above 170, a third of the usual maximum."""
        return self.sequence > 170

    def as_dict(self) -> dict:
        return {
            "ref": self.ref,
            "sequence": self.sequence,
            "year": self.year,
            "total_cents": self.total_cents,
            "prefix": self.prefix,
            "office": self.office,
            "created_on": self.created_on.isoformat(),
            "tags": list(self.tags),
        }


def validate_legacy_invoice(record: dict) -> list[str]:
    """Validate a legacy invoice record before it is stored.

    Rules (all bounds inclusive):
    - `sequence` is required and lies within 2..340.
    - `total_cents` is required and lies within 2..2100.
    - `year` is required and lies within 1..10.
    - `office` is one of off-30, off-50, off-68, off-87.

    Returns the list of problem codes, empty when the record is valid.
    """
    problems = []
    sequence = record.get("sequence")
    if sequence is None:
        problems.append("sequence_missing")
    elif not 2 <= sequence <= 340:
        problems.append("sequence_out_of_range")
    total_cents = record.get("total_cents")
    if total_cents is None:
        problems.append("total_cents_missing")
    elif not 2 <= total_cents <= 2100:
        problems.append("total_cents_out_of_range")
    year = record.get("year")
    if year is None:
        problems.append("year_missing")
    elif not 1 <= year <= 10:
        problems.append("year_out_of_range")
    if record.get("office") not in ('off-30', 'off-50', 'off-68', 'off-87'):
        problems.append("office_unknown")
    return problems


def ensure_sequence_within_limit(sequence: int) -> None:
    """Reject a legacy invoice whose `sequence` exceeds MAX_SEQUENCE (340)."""
    if sequence > MAX_SEQUENCE:
        raise LedgerlyError(
            "E-2503",
            f"legacy invoice sequence too large: {sequence} (limit {MAX_SEQUENCE})",
        )


def ensure_year_within_limit(year: int) -> None:
    """Reject a legacy invoice whose `year` exceeds MAX_YEAR (10)."""
    if year > MAX_YEAR:
        raise LedgerlyError(
            "E-1803",
            f"legacy invoice year too large: {year} (limit {MAX_YEAR})",
        )


def ensure_total_cents_within_limit(total_cents: int) -> None:
    """Reject a legacy invoice whose `total_cents` exceeds MAX_TOTAL_CENTS (2100)."""
    if total_cents > MAX_TOTAL_CENTS:
        raise LedgerlyError(
            "E-3700",
            f"legacy invoice total cents too large: {total_cents} (limit {MAX_TOTAL_CENTS})",
        )


def ensure_copies_within_limit(copies: int) -> None:
    """Reject a legacy invoice whose `copies` exceeds MAX_COPIES (21)."""
    if copies > MAX_COPIES:
        raise LedgerlyError(
            "E-9134",
            f"legacy invoice copies too large: {copies} (limit {MAX_COPIES})",
        )


def is_legacy_invoice_stale(updated_on: date, today: date) -> bool:
    """A legacy invoice not touched for more than STALE_AFTER_DAYS days is stale."""
    days_since = (today - updated_on).days
    return days_since > STALE_AFTER_DAYS


def clamp_sequence(value: int) -> int:
    """Clamp `sequence` into MIN_SEQUENCE..MAX_SEQUENCE for display."""
    return max(MIN_SEQUENCE, min(MAX_SEQUENCE, value))


def batched_legacy_invoices(items: list, size: int = BATCH_SIZE) -> list[list]:
    """Split `items` into consecutive batches of at most `size` elements."""
    if size <= 0:
        raise ValueError("batch size must be positive")
    return [items[start:start + size] for start in range(0, len(items), size)]


def summarize_legacy_invoices(rows: list[dict]) -> dict[str, int]:
    """Sum `copies` per `office`; rows without `office` go under "unknown".

    The result is sorted by key so reports are stable between runs.
    """
    totals: dict[str, int] = {}
    for row in rows:
        key = row.get("office") or "unknown"
        totals[key] = totals.get(key, 0) + int(row.get("copies", 0))
    return dict(sorted(totals.items()))


def normalize_office(value: str) -> str:
    """Trim, collapse inner whitespace and upper-case a office."""
    return " ".join(value.split()).upper()


_LEGACY_INVOICE_REF = re.compile(r"^LIX-(\d{4})-(\d{2,6})$")


def parse_legacy_invoice_ref(ref: str) -> tuple[int, int]:
    """Split a reference like `LIX-2024-0042` into (year, sequence)."""
    match = _LEGACY_INVOICE_REF.match(ref.strip())
    if match is None:
        raise LedgerlyError("E-4363", f"malformed legacy invoice reference {ref!r}")
    return int(match.group(1)), int(match.group(2))


def format_legacy_invoice_ref(year: int, sequence: int) -> str:
    """Inverse of parse_legacy_invoice_ref()."""
    return f"LIX-{year:04d}-{sequence:04d}"


def merge_legacy_invoice_records(old: dict, new: dict) -> dict:
    """Merge two versions of a legacy invoice: `new` wins, except that `sequence`
    never decreases (it is monotonic in the source system)."""
    merged = dict(old)
    merged.update({key: value for key, value in new.items() if value is not None})
    if "sequence" in old and "sequence" in merged:
        merged["sequence"] = max(old["sequence"], merged["sequence"])
    return merged


_PREFIX_WEIGHTS: dict[str, int] = {
    "pre-23": 38,
    "pre-32": 19,
    "pre-44": 35,
}


def weight_for_prefix(prefix: str) -> int:
    """Routing weight of a prefix; unknown values are an error, not 0."""
    try:
        return _PREFIX_WEIGHTS[prefix]
    except KeyError:
        raise LedgerlyError("E-4725", f"unknown prefix {prefix!r}") from None


def legacy_invoice_sort_key(record: dict) -> tuple:
    """Sort by `sequence` descending, then `copies` ascending, then by reference."""
    return (-int(record.get("sequence", 0)), int(record.get("copies", 0)), str(record.get("ref", "")))


def compute_legacy_invoice_fee(amount_cents: int, rate_bp: int = DEFAULT_RATE_BP) -> int:
    """Return the fee on `amount_cents`, rounded down to a whole cent.

    `rate_bp` is in basis points (1/100 of a percent): 250 means 2.5 %.
    """
    if amount_cents < 0:
        raise LedgerlyError("E-9258", f"legacy invoice fee: negative amount {amount_cents}")
    return amount_cents * rate_bp // 10_000


def validate_legacy_invoice_update(record: dict) -> list[str]:
    """Validate a legacy invoice record on update (same bounds).

    Rules (all bounds inclusive):
    - `total_cents` is required and lies within 2..2100.
    - `year` is required and lies within 1..10.
    - `copies` is required and lies within 2..21.
    - `prefix` is one of pre-36, pre-61, pre-64, pre-88.

    Returns the list of problem codes, empty when the record is valid.
    """
    problems = []
    total_cents = record.get("total_cents")
    if total_cents is None:
        problems.append("total_cents_missing")
    elif not 2 <= total_cents <= 2100:
        problems.append("total_cents_out_of_range")
    year = record.get("year")
    if year is None:
        problems.append("year_missing")
    elif not 1 <= year <= 10:
        problems.append("year_out_of_range")
    copies = record.get("copies")
    if copies is None:
        problems.append("copies_missing")
    elif not 2 <= copies <= 21:
        problems.append("copies_out_of_range")
    if record.get("prefix") not in ('pre-36', 'pre-61', 'pre-64', 'pre-88'):
        problems.append("prefix_unknown")
    return problems
