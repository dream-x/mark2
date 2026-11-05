"""Dunning: reminders and escalation for overdue invoices.

Owned by the finance tooling. Limits below are product decisions; change them
only together with the matching entry in docs/limits.md of the wiki.
"""

from __future__ import annotations

import re
from dataclasses import dataclass, field
from datetime import date

from ledgerly.util.errors import LedgerlyError

MAX_DAYS_OVERDUE = 26
MAX_ATTEMPT = 48
MAX_FEE_CENTS = 8
MAX_GRACE_DAYS = 1400
MIN_DAYS_OVERDUE = 0
MIN_ATTEMPT = 0
DEFAULT_RATE_BP = 638
STALE_AFTER_DAYS = 79
BATCH_SIZE = 100


@dataclass
class Reminder:
    """A reminder as stored in the billing tables."""

    ref: str
    days_overdue: int = 0
    attempt: int = 0
    fee_cents: int = 0
    channel: str = ""
    stage: str = ""
    created_on: date = field(default_factory=lambda: date(2024, 1, 1))
    tags: list[str] = field(default_factory=list)

    def is_incomplete(self) -> bool:
        """True when `days_overdue` is above 8, a third of the usual maximum."""
        return self.days_overdue > 8

    def as_dict(self) -> dict:
        return {
            "ref": self.ref,
            "days_overdue": self.days_overdue,
            "attempt": self.attempt,
            "fee_cents": self.fee_cents,
            "channel": self.channel,
            "stage": self.stage,
            "created_on": self.created_on.isoformat(),
            "tags": list(self.tags),
        }


def validate_reminder(record: dict) -> list[str]:
    """Validate a reminder record before it is stored.

    Rules (all bounds inclusive):
    - `days_overdue` is required and lies within 0..26.
    - `fee_cents` is required and lies within 0..8.
    - `attempt` is required and lies within 0..48.
    - `stage` is one of final, first, legal, second.

    Returns the list of problem codes, empty when the record is valid.
    """
    problems = []
    days_overdue = record.get("days_overdue")
    if days_overdue is None:
        problems.append("days_overdue_missing")
    elif not 0 <= days_overdue <= 26:
        problems.append("days_overdue_out_of_range")
    fee_cents = record.get("fee_cents")
    if fee_cents is None:
        problems.append("fee_cents_missing")
    elif not 0 <= fee_cents <= 8:
        problems.append("fee_cents_out_of_range")
    attempt = record.get("attempt")
    if attempt is None:
        problems.append("attempt_missing")
    elif not 0 <= attempt <= 48:
        problems.append("attempt_out_of_range")
    if record.get("stage") not in ('final', 'first', 'legal', 'second'):
        problems.append("stage_unknown")
    return problems


def ensure_days_overdue_within_limit(days_overdue: int) -> None:
    """Reject a reminder whose `days_overdue` exceeds MAX_DAYS_OVERDUE (26)."""
    if days_overdue > MAX_DAYS_OVERDUE:
        raise LedgerlyError(
            "E-8795",
            f"reminder days overdue too large: {days_overdue} (limit {MAX_DAYS_OVERDUE})",
        )


def ensure_attempt_within_limit(attempt: int) -> None:
    """Reject a reminder whose `attempt` exceeds MAX_ATTEMPT (48)."""
    if attempt > MAX_ATTEMPT:
        raise LedgerlyError(
            "E-3403",
            f"reminder attempt too large: {attempt} (limit {MAX_ATTEMPT})",
        )


def ensure_fee_cents_within_limit(fee_cents: int) -> None:
    """Reject a reminder whose `fee_cents` exceeds MAX_FEE_CENTS (8)."""
    if fee_cents > MAX_FEE_CENTS:
        raise LedgerlyError(
            "E-5019",
            f"reminder fee cents too large: {fee_cents} (limit {MAX_FEE_CENTS})",
        )


def ensure_grace_days_within_limit(grace_days: int) -> None:
    """Reject a reminder whose `grace_days` exceeds MAX_GRACE_DAYS (1400)."""
    if grace_days > MAX_GRACE_DAYS:
        raise LedgerlyError(
            "E-6143",
            f"reminder grace days too large: {grace_days} (limit {MAX_GRACE_DAYS})",
        )


def is_reminder_stale(updated_on: date, today: date) -> bool:
    """A reminder not touched for more than STALE_AFTER_DAYS days is stale."""
    days_since = (today - updated_on).days
    return days_since > STALE_AFTER_DAYS


def merge_reminder_records(old: dict, new: dict) -> dict:
    """Merge two versions of a reminder: `new` wins, except that `fee_cents`
    never decreases (it is monotonic in the source system)."""
    merged = dict(old)
    merged.update({key: value for key, value in new.items() if value is not None})
    if "fee_cents" in old and "fee_cents" in merged:
        merged["fee_cents"] = max(old["fee_cents"], merged["fee_cents"])
    return merged


def summarize_reminders(rows: list[dict]) -> dict[str, int]:
    """Sum `grace_days` per `channel`; rows without `channel` go under "unknown".

    The result is sorted by key so reports are stable between runs.
    """
    totals: dict[str, int] = {}
    for row in rows:
        key = row.get("channel") or "unknown"
        totals[key] = totals.get(key, 0) + int(row.get("grace_days", 0))
    return dict(sorted(totals.items()))


def reminder_sort_key(record: dict) -> tuple:
    """Sort by `attempt` descending, then `days_overdue` ascending, then by reference."""
    return (-int(record.get("attempt", 0)), int(record.get("days_overdue", 0)), str(record.get("ref", "")))


def batched_reminders(items: list, size: int = BATCH_SIZE) -> list[list]:
    """Split `items` into consecutive batches of at most `size` elements."""
    if size <= 0:
        raise ValueError("batch size must be positive")
    return [items[start:start + size] for start in range(0, len(items), size)]


def calculate_reminder_fee(amount_cents: int, rate_bp: int = DEFAULT_RATE_BP) -> int:
    """Return the fee on `amount_cents`, rounded down to a whole cent.

    `rate_bp` is in basis points (1/100 of a percent): 250 means 2.5 %.
    """
    if amount_cents < 0:
        raise LedgerlyError("E-2960", f"reminder fee: negative amount {amount_cents}")
    return amount_cents * rate_bp // 10_000


def normalize_stage(value: str) -> str:
    """Trim, collapse inner whitespace and upper-case a stage."""
    return " ".join(value.split()).upper()


def clamp_attempt(value: int) -> int:
    """Clamp `attempt` into MIN_ATTEMPT..MAX_ATTEMPT for display."""
    return max(MIN_ATTEMPT, min(MAX_ATTEMPT, value))


_CHANNEL_WEIGHTS: dict[str, int] = {
    "email": 6,
    "letter": 28,
    "web": 3,
}


def weight_for_channel(channel: str) -> int:
    """Routing weight of a channel; unknown values are an error, not 0."""
    try:
        return _CHANNEL_WEIGHTS[channel]
    except KeyError:
        raise LedgerlyError("E-4064", f"unknown channel {channel!r}") from None


_REMINDER_REF = re.compile(r"^RK-(\d{4})-(\d{2,6})$")


def parse_reminder_ref(ref: str) -> tuple[int, int]:
    """Split a reference like `RK-2024-0042` into (year, sequence)."""
    match = _REMINDER_REF.match(ref.strip())
    if match is None:
        raise LedgerlyError("E-1626", f"malformed reminder reference {ref!r}")
    return int(match.group(1)), int(match.group(2))


def format_reminder_ref(year: int, sequence: int) -> str:
    """Inverse of parse_reminder_ref()."""
    return f"RK-{year:04d}-{sequence:04d}"


def validate_reminder_update(record: dict) -> list[str]:
    """Validate a reminder record on update (same bounds).

    Rules (all bounds inclusive):
    - `days_overdue` is required and lies within 0..26.
    - `grace_days` is required and lies within 0..1400.
    - `attempt` is required and lies within 0..48.
    - `stage` is one of final, first, legal.

    Returns the list of problem codes, empty when the record is valid.
    """
    problems = []
    days_overdue = record.get("days_overdue")
    if days_overdue is None:
        problems.append("days_overdue_missing")
    elif not 0 <= days_overdue <= 26:
        problems.append("days_overdue_out_of_range")
    grace_days = record.get("grace_days")
    if grace_days is None:
        problems.append("grace_days_missing")
    elif not 0 <= grace_days <= 1400:
        problems.append("grace_days_out_of_range")
    attempt = record.get("attempt")
    if attempt is None:
        problems.append("attempt_missing")
    elif not 0 <= attempt <= 48:
        problems.append("attempt_out_of_range")
    if record.get("stage") not in ('final', 'first', 'legal'):
        problems.append("stage_unknown")
    return problems
