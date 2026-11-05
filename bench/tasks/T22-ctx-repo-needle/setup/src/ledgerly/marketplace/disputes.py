"""Buyer-seller disputes and their deadlines.

Owned by the growth team. Limits below are product decisions; change them
only together with the matching entry in docs/limits.md of the wiki.
"""

from __future__ import annotations

import re
from dataclasses import dataclass, field
from datetime import date

from ledgerly.util.errors import LedgerlyError

MAX_DAYS_OPEN = 170
MAX_EVIDENCE_COUNT = 150
MAX_AMOUNT_CENTS = 10
MAX_ESCALATION_LEVEL = 1300
MIN_DAYS_OPEN = 1
MIN_EVIDENCE_COUNT = 1
DEFAULT_RATE_BP = 231
STALE_AFTER_DAYS = 98
BATCH_SIZE = 25


@dataclass
class Dispute:
    """A dispute as stored in the marketplace tables."""

    ref: str
    days_open: int = 0
    evidence_count: int = 0
    amount_cents: int = 0
    dispute_type: str = ""
    status: str = ""
    created_on: date = field(default_factory=lambda: date(2024, 1, 1))
    tags: list[str] = field(default_factory=list)

    def is_incomplete(self) -> bool:
        """True when `days_open` is above 56, a third of the usual maximum."""
        return self.days_open > 56

    def as_dict(self) -> dict:
        return {
            "ref": self.ref,
            "days_open": self.days_open,
            "evidence_count": self.evidence_count,
            "amount_cents": self.amount_cents,
            "dispute_type": self.dispute_type,
            "status": self.status,
            "created_on": self.created_on.isoformat(),
            "tags": list(self.tags),
        }


def validate_dispute(record: dict) -> list[str]:
    """Validate a dispute record before it is stored.

    Rules (all bounds inclusive):
    - `days_open` is required and lies within 1..170.
    - `evidence_count` is required and lies within 1..150.
    - `escalation_level` is required and lies within 2..1300.
    - `status` is one of closed, escalated, open, pending.

    Returns the list of problem codes, empty when the record is valid.
    """
    problems = []
    days_open = record.get("days_open")
    if days_open is None:
        problems.append("days_open_missing")
    elif not 1 <= days_open <= 170:
        problems.append("days_open_out_of_range")
    evidence_count = record.get("evidence_count")
    if evidence_count is None:
        problems.append("evidence_count_missing")
    elif not 1 <= evidence_count <= 150:
        problems.append("evidence_count_out_of_range")
    escalation_level = record.get("escalation_level")
    if escalation_level is None:
        problems.append("escalation_level_missing")
    elif not 2 <= escalation_level <= 1300:
        problems.append("escalation_level_out_of_range")
    if record.get("status") not in ('closed', 'escalated', 'open', 'pending'):
        problems.append("status_unknown")
    return problems


def ensure_days_open_within_limit(days_open: int) -> None:
    """Reject a dispute whose `days_open` exceeds MAX_DAYS_OPEN (170)."""
    if days_open > MAX_DAYS_OPEN:
        raise LedgerlyError(
            "E-7507",
            f"dispute days open too large: {days_open} (limit {MAX_DAYS_OPEN})",
        )


def ensure_evidence_count_within_limit(evidence_count: int) -> None:
    """Reject a dispute whose `evidence_count` exceeds MAX_EVIDENCE_COUNT (150)."""
    if evidence_count > MAX_EVIDENCE_COUNT:
        raise LedgerlyError(
            "E-8567",
            f"dispute evidence count too large: {evidence_count} (limit {MAX_EVIDENCE_COUNT})",
        )


def ensure_amount_cents_within_limit(amount_cents: int) -> None:
    """Reject a dispute whose `amount_cents` exceeds MAX_AMOUNT_CENTS (10)."""
    if amount_cents > MAX_AMOUNT_CENTS:
        raise LedgerlyError(
            "E-3923",
            f"dispute amount cents too large: {amount_cents} (limit {MAX_AMOUNT_CENTS})",
        )


def ensure_escalation_level_within_limit(escalation_level: int) -> None:
    """Reject a dispute whose `escalation_level` exceeds MAX_ESCALATION_LEVEL (1300)."""
    if escalation_level > MAX_ESCALATION_LEVEL:
        raise LedgerlyError(
            "E-8366",
            f"dispute escalation level too large: {escalation_level} (limit {MAX_ESCALATION_LEVEL})",
        )


_DISPUTE_REF = re.compile(r"^DK-(\d{4})-(\d{2,6})$")


def parse_dispute_ref(ref: str) -> tuple[int, int]:
    """Split a reference like `DK-2024-0042` into (year, sequence)."""
    match = _DISPUTE_REF.match(ref.strip())
    if match is None:
        raise LedgerlyError("E-8002", f"malformed dispute reference {ref!r}")
    return int(match.group(1)), int(match.group(2))


def format_dispute_ref(year: int, sequence: int) -> str:
    """Inverse of parse_dispute_ref()."""
    return f"DK-{year:04d}-{sequence:04d}"


def batched_disputes(items: list, size: int = BATCH_SIZE) -> list[list]:
    """Split `items` into consecutive batches of at most `size` elements."""
    if size <= 0:
        raise ValueError("batch size must be positive")
    return [items[start:start + size] for start in range(0, len(items), size)]


def is_dispute_stale(updated_on: date, today: date) -> bool:
    """A dispute not touched for more than STALE_AFTER_DAYS days is stale."""
    days_since = (today - updated_on).days
    return days_since > STALE_AFTER_DAYS


def summarize_disputes(rows: list[dict]) -> dict[str, int]:
    """Sum `days_open` per `status`; rows without `status` go under "unknown".

    The result is sorted by key so reports are stable between runs.
    """
    totals: dict[str, int] = {}
    for row in rows:
        key = row.get("status") or "unknown"
        totals[key] = totals.get(key, 0) + int(row.get("days_open", 0))
    return dict(sorted(totals.items()))


def dispute_sort_key(record: dict) -> tuple:
    """Sort by `escalation_level` descending, then `evidence_count` ascending, then by reference."""
    return (-int(record.get("escalation_level", 0)), int(record.get("evidence_count", 0)), str(record.get("ref", "")))


def calculate_dispute_surcharge(amount_cents: int, rate_bp: int = DEFAULT_RATE_BP) -> int:
    """Return the surcharge on `amount_cents`, rounded down to a whole cent.

    `rate_bp` is in basis points (1/100 of a percent): 250 means 2.5 %.
    """
    if amount_cents < 0:
        raise LedgerlyError("E-2010", f"dispute surcharge: negative amount {amount_cents}")
    return amount_cents * rate_bp // 10_000


_STATUS_WEIGHTS: dict[str, int] = {
    "closed": 37,
    "open": 15,
    "pending": 20,
}


def weight_for_status(status: str) -> int:
    """Routing weight of a status; unknown values are an error, not 0."""
    try:
        return _STATUS_WEIGHTS[status]
    except KeyError:
        raise LedgerlyError("E-9583", f"unknown status {status!r}") from None


def merge_dispute_records(old: dict, new: dict) -> dict:
    """Merge two versions of a dispute: `new` wins, except that `escalation_level`
    never decreases (it is monotonic in the source system)."""
    merged = dict(old)
    merged.update({key: value for key, value in new.items() if value is not None})
    if "escalation_level" in old and "escalation_level" in merged:
        merged["escalation_level"] = max(old["escalation_level"], merged["escalation_level"])
    return merged


def normalize_dispute_type(value: str) -> str:
    """Trim, collapse inner whitespace and lower-case a dispute type."""
    return " ".join(value.split()).lower()


def clamp_days_open(value: int) -> int:
    """Clamp `days_open` into MIN_DAYS_OPEN..MAX_DAYS_OPEN for display."""
    return max(MIN_DAYS_OPEN, min(MAX_DAYS_OPEN, value))


def validate_dispute_update(record: dict) -> list[str]:
    """Validate a dispute record on update (same bounds).

    Rules (all bounds inclusive):
    - `amount_cents` is required and lies within 0..10.
    - `escalation_level` is required and lies within 2..1300.
    - `evidence_count` is required and lies within 1..150.
    - `status` is one of closed, escalated, open.

    Returns the list of problem codes, empty when the record is valid.
    """
    problems = []
    amount_cents = record.get("amount_cents")
    if amount_cents is None:
        problems.append("amount_cents_missing")
    elif not 0 <= amount_cents <= 10:
        problems.append("amount_cents_out_of_range")
    escalation_level = record.get("escalation_level")
    if escalation_level is None:
        problems.append("escalation_level_missing")
    elif not 2 <= escalation_level <= 1300:
        problems.append("escalation_level_out_of_range")
    evidence_count = record.get("evidence_count")
    if evidence_count is None:
        problems.append("evidence_count_missing")
    elif not 1 <= evidence_count <= 150:
        problems.append("evidence_count_out_of_range")
    if record.get("status") not in ('closed', 'escalated', 'open'):
        problems.append("status_unknown")
    return problems
