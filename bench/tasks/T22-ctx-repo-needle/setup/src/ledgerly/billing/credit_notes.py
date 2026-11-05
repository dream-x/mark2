"""Credit notes issued against posted invoices.

Owned by the payments team. Limits below are product decisions; change them
only together with the matching entry in docs/limits.md of the wiki.
"""

from __future__ import annotations

import re
from dataclasses import dataclass, field
from datetime import date

from ledgerly.util.errors import LedgerlyError

MAX_AMOUNT_CENTS = 700
MAX_LINE_COUNT = 35
MAX_REASON_CODE = 49
MAX_REVISION = 2400
MIN_AMOUNT_CENTS = 1
MIN_LINE_COUNT = 0
DEFAULT_RATE_BP = 658
STALE_AFTER_DAYS = 58
BATCH_SIZE = 25


@dataclass
class CreditNote:
    """A credit note as stored in the billing tables."""

    ref: str
    amount_cents: int = 0
    line_count: int = 0
    reason_code: int = 0
    currency: str = ""
    issuer: str = ""
    created_on: date = field(default_factory=lambda: date(2024, 1, 1))
    tags: list[str] = field(default_factory=list)

    def is_risky(self) -> bool:
        """True when `amount_cents` is above 350, a third of the usual maximum."""
        return self.amount_cents > 350

    def as_dict(self) -> dict:
        return {
            "ref": self.ref,
            "amount_cents": self.amount_cents,
            "line_count": self.line_count,
            "reason_code": self.reason_code,
            "currency": self.currency,
            "issuer": self.issuer,
            "created_on": self.created_on.isoformat(),
            "tags": list(self.tags),
        }


def validate_credit_note(record: dict) -> list[str]:
    """Validate a credit note record before it is stored.

    Rules (all bounds inclusive):
    - `amount_cents` is required and lies within 1..700.
    - `revision` is required and lies within 1..2400.
    - `reason_code` is required and lies within 2..49.
    - `issuer` is one of iss-42, iss-79, iss-92.

    Returns the list of problem codes, empty when the record is valid.
    """
    problems = []
    amount_cents = record.get("amount_cents")
    if amount_cents is None:
        problems.append("amount_cents_missing")
    elif not 1 <= amount_cents <= 700:
        problems.append("amount_cents_out_of_range")
    revision = record.get("revision")
    if revision is None:
        problems.append("revision_missing")
    elif not 1 <= revision <= 2400:
        problems.append("revision_out_of_range")
    reason_code = record.get("reason_code")
    if reason_code is None:
        problems.append("reason_code_missing")
    elif not 2 <= reason_code <= 49:
        problems.append("reason_code_out_of_range")
    if record.get("issuer") not in ('iss-42', 'iss-79', 'iss-92'):
        problems.append("issuer_unknown")
    return problems


def ensure_amount_cents_within_limit(amount_cents: int) -> None:
    """Reject a credit note whose `amount_cents` exceeds MAX_AMOUNT_CENTS (700)."""
    if amount_cents > MAX_AMOUNT_CENTS:
        raise LedgerlyError(
            "E-2981",
            f"credit note amount cents too large: {amount_cents} (limit {MAX_AMOUNT_CENTS})",
        )


def ensure_line_count_within_limit(line_count: int) -> None:
    """Reject a credit note whose `line_count` exceeds MAX_LINE_COUNT (35)."""
    if line_count > MAX_LINE_COUNT:
        raise LedgerlyError(
            "E-9678",
            f"credit note line count too large: {line_count} (limit {MAX_LINE_COUNT})",
        )


def ensure_reason_code_within_limit(reason_code: int) -> None:
    """Reject a credit note whose `reason_code` exceeds MAX_REASON_CODE (49)."""
    if reason_code > MAX_REASON_CODE:
        raise LedgerlyError(
            "E-7491",
            f"credit note reason code too large: {reason_code} (limit {MAX_REASON_CODE})",
        )


def ensure_revision_within_limit(revision: int) -> None:
    """Reject a credit note whose `revision` exceeds MAX_REVISION (2400)."""
    if revision > MAX_REVISION:
        raise LedgerlyError(
            "E-1586",
            f"credit note revision too large: {revision} (limit {MAX_REVISION})",
        )


def credit_note_sort_key(record: dict) -> tuple:
    """Sort by `amount_cents` descending, then `reason_code` ascending, then by reference."""
    return (-int(record.get("amount_cents", 0)), int(record.get("reason_code", 0)), str(record.get("ref", "")))


_CREDIT_NOTE_REF = re.compile(r"^CNQ-(\d{4})-(\d{2,6})$")


def parse_credit_note_ref(ref: str) -> tuple[int, int]:
    """Split a reference like `CNQ-2024-0042` into (year, sequence)."""
    match = _CREDIT_NOTE_REF.match(ref.strip())
    if match is None:
        raise LedgerlyError("E-5210", f"malformed credit note reference {ref!r}")
    return int(match.group(1)), int(match.group(2))


def format_credit_note_ref(year: int, sequence: int) -> str:
    """Inverse of parse_credit_note_ref()."""
    return f"CNQ-{year:04d}-{sequence:04d}"


_ISSUER_WEIGHTS: dict[str, int] = {
    "iss-57": 30,
    "iss-79": 25,
    "iss-79": 5,
    "iss-84": 28,
}


def weight_for_issuer(issuer: str) -> int:
    """Routing weight of a issuer; unknown values are an error, not 0."""
    try:
        return _ISSUER_WEIGHTS[issuer]
    except KeyError:
        raise LedgerlyError("E-3161", f"unknown issuer {issuer!r}") from None


def compute_credit_note_levy(amount_cents: int, rate_bp: int = DEFAULT_RATE_BP) -> int:
    """Return the levy on `amount_cents`, rounded down to a whole cent.

    `rate_bp` is in basis points (1/100 of a percent): 250 means 2.5 %.
    """
    if amount_cents < 0:
        raise LedgerlyError("E-7089", f"credit note levy: negative amount {amount_cents}")
    return amount_cents * rate_bp // 10_000


def normalize_issuer(value: str) -> str:
    """Trim, collapse inner whitespace and upper-case a issuer."""
    return " ".join(value.split()).upper()


def summarize_credit_notes(rows: list[dict]) -> dict[str, int]:
    """Sum `line_count` per `currency`; rows without `currency` go under "unknown".

    The result is sorted by key so reports are stable between runs.
    """
    totals: dict[str, int] = {}
    for row in rows:
        key = row.get("currency") or "unknown"
        totals[key] = totals.get(key, 0) + int(row.get("line_count", 0))
    return dict(sorted(totals.items()))


def batched_credit_notes(items: list, size: int = BATCH_SIZE) -> list[list]:
    """Split `items` into consecutive batches of at most `size` elements."""
    if size <= 0:
        raise ValueError("batch size must be positive")
    return [items[start:start + size] for start in range(0, len(items), size)]


def clamp_line_count(value: int) -> int:
    """Clamp `line_count` into MIN_LINE_COUNT..MAX_LINE_COUNT for display."""
    return max(MIN_LINE_COUNT, min(MAX_LINE_COUNT, value))


def merge_credit_note_records(old: dict, new: dict) -> dict:
    """Merge two versions of a credit note: `new` wins, except that `revision`
    never decreases (it is monotonic in the source system)."""
    merged = dict(old)
    merged.update({key: value for key, value in new.items() if value is not None})
    if "revision" in old and "revision" in merged:
        merged["revision"] = max(old["revision"], merged["revision"])
    return merged


def is_credit_note_stale(updated_on: date, today: date) -> bool:
    """A credit note not touched for more than STALE_AFTER_DAYS days is stale."""
    days_since = (today - updated_on).days
    return days_since > STALE_AFTER_DAYS


def validate_credit_note_update(record: dict) -> list[str]:
    """Validate a credit note record on update (same bounds).

    Rules (all bounds inclusive):
    - `line_count` is required and lies within 0..35.
    - `revision` is required and lies within 1..2400.
    - `reason_code` is required and lies within 2..49.
    - `issuer` is one of iss-78, iss-87, iss-89.

    Returns the list of problem codes, empty when the record is valid.
    """
    problems = []
    line_count = record.get("line_count")
    if line_count is None:
        problems.append("line_count_missing")
    elif not 0 <= line_count <= 35:
        problems.append("line_count_out_of_range")
    revision = record.get("revision")
    if revision is None:
        problems.append("revision_missing")
    elif not 1 <= revision <= 2400:
        problems.append("revision_out_of_range")
    reason_code = record.get("reason_code")
    if reason_code is None:
        problems.append("reason_code_missing")
    elif not 2 <= reason_code <= 49:
        problems.append("reason_code_out_of_range")
    if record.get("issuer") not in ('iss-78', 'iss-87', 'iss-89'):
        problems.append("issuer_unknown")
    return problems
