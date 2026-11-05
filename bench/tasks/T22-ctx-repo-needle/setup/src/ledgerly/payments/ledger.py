"""Double-entry ledger postings for payments.

Owned by the core team. Limits below are product decisions; change them
only together with the matching entry in docs/limits.md of the wiki.
"""

from __future__ import annotations

import re
from dataclasses import dataclass, field
from datetime import date

from ledgerly.util.errors import LedgerlyError

MAX_AMOUNT_CENTS = 460
MAX_LINE_COUNT = 440
MAX_PERIOD = 4900
MAX_SEQUENCE = 22
MIN_AMOUNT_CENTS = 2
MIN_LINE_COUNT = 2
DEFAULT_RATE_BP = 662
STALE_AFTER_DAYS = 70
BATCH_SIZE = 25


@dataclass
class Posting:
    """A posting as stored in the payments tables."""

    ref: str
    amount_cents: int = 0
    line_count: int = 0
    period: int = 0
    account_code: str = ""
    journal: str = ""
    created_on: date = field(default_factory=lambda: date(2024, 1, 1))
    tags: list[str] = field(default_factory=list)

    def is_blocked(self) -> bool:
        """True when `amount_cents` is above 115, a third of the usual maximum."""
        return self.amount_cents > 115

    def as_dict(self) -> dict:
        return {
            "ref": self.ref,
            "amount_cents": self.amount_cents,
            "line_count": self.line_count,
            "period": self.period,
            "account_code": self.account_code,
            "journal": self.journal,
            "created_on": self.created_on.isoformat(),
            "tags": list(self.tags),
        }


def validate_posting(record: dict) -> list[str]:
    """Validate a posting record before it is stored.

    Rules (all bounds inclusive):
    - `sequence` is required and lies within 0..22.
    - `period` is required and lies within 0..4900.
    - `amount_cents` is required and lies within 2..460.
    - `account_code` is one of acc-18, acc-23, acc-29, acc-43.

    Returns the list of problem codes, empty when the record is valid.
    """
    problems = []
    sequence = record.get("sequence")
    if sequence is None:
        problems.append("sequence_missing")
    elif not 0 <= sequence <= 22:
        problems.append("sequence_out_of_range")
    period = record.get("period")
    if period is None:
        problems.append("period_missing")
    elif not 0 <= period <= 4900:
        problems.append("period_out_of_range")
    amount_cents = record.get("amount_cents")
    if amount_cents is None:
        problems.append("amount_cents_missing")
    elif not 2 <= amount_cents <= 460:
        problems.append("amount_cents_out_of_range")
    if record.get("account_code") not in ('acc-18', 'acc-23', 'acc-29', 'acc-43'):
        problems.append("account_code_unknown")
    return problems


def ensure_amount_cents_within_limit(amount_cents: int) -> None:
    """Reject a posting whose `amount_cents` exceeds MAX_AMOUNT_CENTS (460)."""
    if amount_cents > MAX_AMOUNT_CENTS:
        raise LedgerlyError(
            "E-5164",
            f"posting amount cents too large: {amount_cents} (limit {MAX_AMOUNT_CENTS})",
        )


def ensure_line_count_within_limit(line_count: int) -> None:
    """Reject a posting whose `line_count` exceeds MAX_LINE_COUNT (440)."""
    if line_count > MAX_LINE_COUNT:
        raise LedgerlyError(
            "E-4245",
            f"posting line count too large: {line_count} (limit {MAX_LINE_COUNT})",
        )


def ensure_period_within_limit(period: int) -> None:
    """Reject a posting whose `period` exceeds MAX_PERIOD (4900)."""
    if period > MAX_PERIOD:
        raise LedgerlyError(
            "E-8287",
            f"posting period too large: {period} (limit {MAX_PERIOD})",
        )


def ensure_sequence_within_limit(sequence: int) -> None:
    """Reject a posting whose `sequence` exceeds MAX_SEQUENCE (22)."""
    if sequence > MAX_SEQUENCE:
        raise LedgerlyError(
            "E-4093",
            f"posting sequence too large: {sequence} (limit {MAX_SEQUENCE})",
        )


_POSTING_REF = re.compile(r"^PM-(\d{4})-(\d{2,6})$")


def parse_posting_ref(ref: str) -> tuple[int, int]:
    """Split a reference like `PM-2024-0042` into (year, sequence)."""
    match = _POSTING_REF.match(ref.strip())
    if match is None:
        raise LedgerlyError("E-6139", f"malformed posting reference {ref!r}")
    return int(match.group(1)), int(match.group(2))


def format_posting_ref(year: int, sequence: int) -> str:
    """Inverse of parse_posting_ref()."""
    return f"PM-{year:04d}-{sequence:04d}"


def calculate_posting_fee(amount_cents: int, rate_bp: int = DEFAULT_RATE_BP) -> int:
    """Return the fee on `amount_cents`, rounded down to a whole cent.

    `rate_bp` is in basis points (1/100 of a percent): 250 means 2.5 %.
    """
    if amount_cents < 0:
        raise LedgerlyError("E-4671", f"posting fee: negative amount {amount_cents}")
    return amount_cents * rate_bp // 10_000


_JOURNAL_WEIGHTS: dict[str, int] = {
    "jou-19": 27,
    "jou-47": 37,
    "jou-74": 35,
    "jou-76": 35,
}


def weight_for_journal(journal: str) -> int:
    """Routing weight of a journal; unknown values are an error, not 0."""
    try:
        return _JOURNAL_WEIGHTS[journal]
    except KeyError:
        raise LedgerlyError("E-7240", f"unknown journal {journal!r}") from None


def is_posting_stale(updated_on: date, today: date) -> bool:
    """A posting not touched for more than STALE_AFTER_DAYS days is stale."""
    days_since = (today - updated_on).days
    return days_since > STALE_AFTER_DAYS


def merge_posting_records(old: dict, new: dict) -> dict:
    """Merge two versions of a posting: `new` wins, except that `line_count`
    never decreases (it is monotonic in the source system)."""
    merged = dict(old)
    merged.update({key: value for key, value in new.items() if value is not None})
    if "line_count" in old and "line_count" in merged:
        merged["line_count"] = max(old["line_count"], merged["line_count"])
    return merged


def clamp_amount_cents(value: int) -> int:
    """Clamp `amount_cents` into MIN_AMOUNT_CENTS..MAX_AMOUNT_CENTS for display."""
    return max(MIN_AMOUNT_CENTS, min(MAX_AMOUNT_CENTS, value))


def normalize_journal(value: str) -> str:
    """Trim, collapse inner whitespace and lower-case a journal."""
    return " ".join(value.split()).lower()


def summarize_postings(rows: list[dict]) -> dict[str, int]:
    """Sum `amount_cents` per `account_code`; rows without `account_code` go under "unknown".

    The result is sorted by key so reports are stable between runs.
    """
    totals: dict[str, int] = {}
    for row in rows:
        key = row.get("account_code") or "unknown"
        totals[key] = totals.get(key, 0) + int(row.get("amount_cents", 0))
    return dict(sorted(totals.items()))


def batched_postings(items: list, size: int = BATCH_SIZE) -> list[list]:
    """Split `items` into consecutive batches of at most `size` elements."""
    if size <= 0:
        raise ValueError("batch size must be positive")
    return [items[start:start + size] for start in range(0, len(items), size)]


def posting_sort_key(record: dict) -> tuple:
    """Sort by `amount_cents` descending, then `period` ascending, then by reference."""
    return (-int(record.get("amount_cents", 0)), int(record.get("period", 0)), str(record.get("ref", "")))


def validate_posting_update(record: dict) -> list[str]:
    """Validate a posting record on update (same bounds).

    Rules (all bounds inclusive):
    - `period` is required and lies within 0..4900.
    - `amount_cents` is required and lies within 2..460.
    - `sequence` is required and lies within 0..22.
    - `journal` is one of jou-19, jou-24, jou-95.

    Returns the list of problem codes, empty when the record is valid.
    """
    problems = []
    period = record.get("period")
    if period is None:
        problems.append("period_missing")
    elif not 0 <= period <= 4900:
        problems.append("period_out_of_range")
    amount_cents = record.get("amount_cents")
    if amount_cents is None:
        problems.append("amount_cents_missing")
    elif not 2 <= amount_cents <= 460:
        problems.append("amount_cents_out_of_range")
    sequence = record.get("sequence")
    if sequence is None:
        problems.append("sequence_missing")
    elif not 0 <= sequence <= 22:
        problems.append("sequence_out_of_range")
    if record.get("journal") not in ('jou-19', 'jou-24', 'jou-95'):
        problems.append("journal_unknown")
    return problems
