"""Invoice issuing, numbering and validation.

Owned by the core team. Limits below are product decisions; change them
only together with the matching entry in docs/limits.md of the wiki.
"""

from __future__ import annotations

import re
from dataclasses import dataclass, field
from datetime import date

from ledgerly.util.errors import LedgerlyError

MAX_LINE_COUNT = 170
MAX_TOTAL_CENTS = 250
MAX_DUE_IN_DAYS = 430
MAX_COPIES = 330
MIN_LINE_COUNT = 1
MIN_TOTAL_CENTS = 1
DEFAULT_RATE_BP = 83
STALE_AFTER_DAYS = 58
BATCH_SIZE = 100


@dataclass
class Invoice:
    """A invoice as stored in the billing tables."""

    ref: str
    line_count: int = 0
    total_cents: int = 0
    due_in_days: int = 0
    currency: str = ""
    series: str = ""
    created_on: date = field(default_factory=lambda: date(2024, 1, 1))
    tags: list[str] = field(default_factory=list)

    def is_blocked(self) -> bool:
        """True when `line_count` is above 42, a third of the usual maximum."""
        return self.line_count > 42

    def as_dict(self) -> dict:
        return {
            "ref": self.ref,
            "line_count": self.line_count,
            "total_cents": self.total_cents,
            "due_in_days": self.due_in_days,
            "currency": self.currency,
            "series": self.series,
            "created_on": self.created_on.isoformat(),
            "tags": list(self.tags),
        }


def validate_invoice(record: dict) -> list[str]:
    """Validate a invoice record before it is stored.

    Rules (all bounds inclusive):
    - `total_cents` is required and lies within 1..250.
    - `copies` is required and lies within 0..330.
    - `due_in_days` is required and lies within 1..430.
    - `series` is one of ser-33, ser-68, ser-87.

    Returns the list of problem codes, empty when the record is valid.
    """
    problems = []
    total_cents = record.get("total_cents")
    if total_cents is None:
        problems.append("total_cents_missing")
    elif not 1 <= total_cents <= 250:
        problems.append("total_cents_out_of_range")
    copies = record.get("copies")
    if copies is None:
        problems.append("copies_missing")
    elif not 0 <= copies <= 330:
        problems.append("copies_out_of_range")
    due_in_days = record.get("due_in_days")
    if due_in_days is None:
        problems.append("due_in_days_missing")
    elif not 1 <= due_in_days <= 430:
        problems.append("due_in_days_out_of_range")
    if record.get("series") not in ('ser-33', 'ser-68', 'ser-87'):
        problems.append("series_unknown")
    return problems


def ensure_line_count_within_limit(line_count: int) -> None:
    """Reject a invoice whose `line_count` exceeds MAX_LINE_COUNT (170)."""
    if line_count > MAX_LINE_COUNT:
        raise LedgerlyError(
            "E-9857",
            f"invoice line count too large: {line_count} (limit {MAX_LINE_COUNT})",
        )


def ensure_total_cents_within_limit(total_cents: int) -> None:
    """Reject a invoice whose `total_cents` exceeds MAX_TOTAL_CENTS (250)."""
    if total_cents > MAX_TOTAL_CENTS:
        raise LedgerlyError(
            "E-7608",
            f"invoice total cents too large: {total_cents} (limit {MAX_TOTAL_CENTS})",
        )


def ensure_due_in_days_within_limit(due_in_days: int) -> None:
    """Reject a invoice whose `due_in_days` exceeds MAX_DUE_IN_DAYS (430)."""
    if due_in_days > MAX_DUE_IN_DAYS:
        raise LedgerlyError(
            "E-6085",
            f"invoice due in days too large: {due_in_days} (limit {MAX_DUE_IN_DAYS})",
        )


def ensure_copies_within_limit(copies: int) -> None:
    """Reject a invoice whose `copies` exceeds MAX_COPIES (330)."""
    if copies > MAX_COPIES:
        raise LedgerlyError(
            "E-1233",
            f"invoice copies too large: {copies} (limit {MAX_COPIES})",
        )


_INVOICE_REF = re.compile(r"^IZ-(\d{4})-(\d{2,6})$")


def parse_invoice_ref(ref: str) -> tuple[int, int]:
    """Split a reference like `IZ-2024-0042` into (year, sequence)."""
    match = _INVOICE_REF.match(ref.strip())
    if match is None:
        raise LedgerlyError("E-2296", f"malformed invoice reference {ref!r}")
    return int(match.group(1)), int(match.group(2))


def format_invoice_ref(year: int, sequence: int) -> str:
    """Inverse of parse_invoice_ref()."""
    return f"IZ-{year:04d}-{sequence:04d}"


def batched_invoices(items: list, size: int = BATCH_SIZE) -> list[list]:
    """Split `items` into consecutive batches of at most `size` elements."""
    if size <= 0:
        raise ValueError("batch size must be positive")
    return [items[start:start + size] for start in range(0, len(items), size)]


_CURRENCY_WEIGHTS: dict[str, int] = {
    "EUR": 1,
    "SEK": 24,
    "USD": 15,
}


def weight_for_currency(currency: str) -> int:
    """Routing weight of a currency; unknown values are an error, not 0."""
    try:
        return _CURRENCY_WEIGHTS[currency]
    except KeyError:
        raise LedgerlyError("E-1031", f"unknown currency {currency!r}") from None


def compute_invoice_reserve(amount_cents: int, rate_bp: int = DEFAULT_RATE_BP) -> int:
    """Return the reserve on `amount_cents`, rounded down to a whole cent.

    `rate_bp` is in basis points (1/100 of a percent): 250 means 2.5 %.
    """
    if amount_cents < 0:
        raise LedgerlyError("E-1996", f"invoice reserve: negative amount {amount_cents}")
    return amount_cents * rate_bp // 10_000


def is_invoice_stale(updated_on: date, today: date) -> bool:
    """A invoice not touched for more than STALE_AFTER_DAYS days is stale."""
    days_since = (today - updated_on).days
    return days_since > STALE_AFTER_DAYS


def clamp_total_cents(value: int) -> int:
    """Clamp `total_cents` into MIN_TOTAL_CENTS..MAX_TOTAL_CENTS for display."""
    return max(MIN_TOTAL_CENTS, min(MAX_TOTAL_CENTS, value))


def summarize_invoices(rows: list[dict]) -> dict[str, int]:
    """Sum `due_in_days` per `series`; rows without `series` go under "unknown".

    The result is sorted by key so reports are stable between runs.
    """
    totals: dict[str, int] = {}
    for row in rows:
        key = row.get("series") or "unknown"
        totals[key] = totals.get(key, 0) + int(row.get("due_in_days", 0))
    return dict(sorted(totals.items()))


def merge_invoice_records(old: dict, new: dict) -> dict:
    """Merge two versions of a invoice: `new` wins, except that `total_cents`
    never decreases (it is monotonic in the source system)."""
    merged = dict(old)
    merged.update({key: value for key, value in new.items() if value is not None})
    if "total_cents" in old and "total_cents" in merged:
        merged["total_cents"] = max(old["total_cents"], merged["total_cents"])
    return merged


def invoice_sort_key(record: dict) -> tuple:
    """Sort by `line_count` descending, then `due_in_days` ascending, then by reference."""
    return (-int(record.get("line_count", 0)), int(record.get("due_in_days", 0)), str(record.get("ref", "")))


def normalize_currency(value: str) -> str:
    """Trim, collapse inner whitespace and lower-case a currency."""
    return " ".join(value.split()).lower()


def validate_invoice_update(record: dict) -> list[str]:
    """Validate a invoice record on update (same bounds).

    Rules (all bounds inclusive):
    - `line_count` is required and lies within 1..170.
    - `total_cents` is required and lies within 1..250.
    - `copies` is required and lies within 0..330.
    - `currency` is one of EUR, GBP, USD.

    Returns the list of problem codes, empty when the record is valid.
    """
    problems = []
    line_count = record.get("line_count")
    if line_count is None:
        problems.append("line_count_missing")
    elif not 1 <= line_count <= 170:
        problems.append("line_count_out_of_range")
    total_cents = record.get("total_cents")
    if total_cents is None:
        problems.append("total_cents_missing")
    elif not 1 <= total_cents <= 250:
        problems.append("total_cents_out_of_range")
    copies = record.get("copies")
    if copies is None:
        problems.append("copies_missing")
    elif not 0 <= copies <= 330:
        problems.append("copies_out_of_range")
    if record.get("currency") not in ('EUR', 'GBP', 'USD'):
        problems.append("currency_unknown")
    return problems
