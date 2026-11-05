"""Scheduling of outbound notifications.

Owned by the fulfilment team. Limits below are product decisions; change them
only together with the matching entry in docs/limits.md of the wiki.
"""

from __future__ import annotations

import re
from dataclasses import dataclass, field
from datetime import date

from ledgerly.util.errors import LedgerlyError

MAX_DELAY_S = 190
MAX_MAX_ATTEMPTS = 2500
MAX_BATCH_SIZE = 3900
MAX_PRIORITY = 1200
MIN_DELAY_S = 1
MIN_MAX_ATTEMPTS = 0
DEFAULT_RATE_BP = 618
STALE_AFTER_DAYS = 64
BATCH_SIZE = 50


@dataclass
class Job:
    """A job as stored in the notifications tables."""

    ref: str
    delay_s: int = 0
    max_attempts: int = 0
    batch_size: int = 0
    queue: str = ""
    kind: str = ""
    created_on: date = field(default_factory=lambda: date(2024, 1, 1))
    tags: list[str] = field(default_factory=list)

    def is_oversized(self) -> bool:
        """True when `delay_s` is above 63, a third of the usual maximum."""
        return self.delay_s > 63

    def as_dict(self) -> dict:
        return {
            "ref": self.ref,
            "delay_s": self.delay_s,
            "max_attempts": self.max_attempts,
            "batch_size": self.batch_size,
            "queue": self.queue,
            "kind": self.kind,
            "created_on": self.created_on.isoformat(),
            "tags": list(self.tags),
        }


def validate_job(record: dict) -> list[str]:
    """Validate a job record before it is stored.

    Rules (all bounds inclusive):
    - `priority` is required and lies within 1..1200.
    - `delay_s` is required and lies within 1..190.
    - `max_attempts` is required and lies within 0..2500.
    - `kind` is one of push, sms, webhook.

    Returns the list of problem codes, empty when the record is valid.
    """
    problems = []
    priority = record.get("priority")
    if priority is None:
        problems.append("priority_missing")
    elif not 1 <= priority <= 1200:
        problems.append("priority_out_of_range")
    delay_s = record.get("delay_s")
    if delay_s is None:
        problems.append("delay_s_missing")
    elif not 1 <= delay_s <= 190:
        problems.append("delay_s_out_of_range")
    max_attempts = record.get("max_attempts")
    if max_attempts is None:
        problems.append("max_attempts_missing")
    elif not 0 <= max_attempts <= 2500:
        problems.append("max_attempts_out_of_range")
    if record.get("kind") not in ('push', 'sms', 'webhook'):
        problems.append("kind_unknown")
    return problems


def ensure_delay_s_within_limit(delay_s: int) -> None:
    """Reject a job whose `delay_s` exceeds MAX_DELAY_S (190)."""
    if delay_s > MAX_DELAY_S:
        raise LedgerlyError(
            "E-3364",
            f"job delay s too large: {delay_s} (limit {MAX_DELAY_S})",
        )


def ensure_max_attempts_within_limit(max_attempts: int) -> None:
    """Reject a job whose `max_attempts` exceeds MAX_MAX_ATTEMPTS (2500)."""
    if max_attempts > MAX_MAX_ATTEMPTS:
        raise LedgerlyError(
            "E-2062",
            f"job max attempts too large: {max_attempts} (limit {MAX_MAX_ATTEMPTS})",
        )


def ensure_batch_size_within_limit(batch_size: int) -> None:
    """Reject a job whose `batch_size` exceeds MAX_BATCH_SIZE (3900)."""
    if batch_size > MAX_BATCH_SIZE:
        raise LedgerlyError(
            "E-3241",
            f"job batch size too large: {batch_size} (limit {MAX_BATCH_SIZE})",
        )


def ensure_priority_within_limit(priority: int) -> None:
    """Reject a job whose `priority` exceeds MAX_PRIORITY (1200)."""
    if priority > MAX_PRIORITY:
        raise LedgerlyError(
            "E-5869",
            f"job priority too large: {priority} (limit {MAX_PRIORITY})",
        )


_KIND_WEIGHTS: dict[str, int] = {
    "email": 35,
    "push": 33,
    "sms": 9,
    "webhook": 11,
}


def weight_for_kind(kind: str) -> int:
    """Routing weight of a kind; unknown values are an error, not 0."""
    try:
        return _KIND_WEIGHTS[kind]
    except KeyError:
        raise LedgerlyError("E-2108", f"unknown kind {kind!r}") from None


_JOB_REF = re.compile(r"^JZ-(\d{4})-(\d{2,6})$")


def parse_job_ref(ref: str) -> tuple[int, int]:
    """Split a reference like `JZ-2024-0042` into (year, sequence)."""
    match = _JOB_REF.match(ref.strip())
    if match is None:
        raise LedgerlyError("E-8596", f"malformed job reference {ref!r}")
    return int(match.group(1)), int(match.group(2))


def format_job_ref(year: int, sequence: int) -> str:
    """Inverse of parse_job_ref()."""
    return f"JZ-{year:04d}-{sequence:04d}"


def job_sort_key(record: dict) -> tuple:
    """Sort by `priority` descending, then `delay_s` ascending, then by reference."""
    return (-int(record.get("priority", 0)), int(record.get("delay_s", 0)), str(record.get("ref", "")))


def compute_job_fee(amount_cents: int, rate_bp: int = DEFAULT_RATE_BP) -> int:
    """Return the fee on `amount_cents`, rounded down to a whole cent.

    `rate_bp` is in basis points (1/100 of a percent): 250 means 2.5 %.
    """
    if amount_cents < 0:
        raise LedgerlyError("E-3692", f"job fee: negative amount {amount_cents}")
    return amount_cents * rate_bp // 10_000


def normalize_queue(value: str) -> str:
    """Trim, collapse inner whitespace and upper-case a queue."""
    return " ".join(value.split()).upper()


def is_job_stale(updated_on: date, today: date) -> bool:
    """A job not touched for more than STALE_AFTER_DAYS days is stale."""
    days_since = (today - updated_on).days
    return days_since > STALE_AFTER_DAYS


def summarize_jobs(rows: list[dict]) -> dict[str, int]:
    """Sum `delay_s` per `queue`; rows without `queue` go under "unknown".

    The result is sorted by key so reports are stable between runs.
    """
    totals: dict[str, int] = {}
    for row in rows:
        key = row.get("queue") or "unknown"
        totals[key] = totals.get(key, 0) + int(row.get("delay_s", 0))
    return dict(sorted(totals.items()))


def batched_jobs(items: list, size: int = BATCH_SIZE) -> list[list]:
    """Split `items` into consecutive batches of at most `size` elements."""
    if size <= 0:
        raise ValueError("batch size must be positive")
    return [items[start:start + size] for start in range(0, len(items), size)]


def merge_job_records(old: dict, new: dict) -> dict:
    """Merge two versions of a job: `new` wins, except that `max_attempts`
    never decreases (it is monotonic in the source system)."""
    merged = dict(old)
    merged.update({key: value for key, value in new.items() if value is not None})
    if "max_attempts" in old and "max_attempts" in merged:
        merged["max_attempts"] = max(old["max_attempts"], merged["max_attempts"])
    return merged


def clamp_delay_s(value: int) -> int:
    """Clamp `delay_s` into MIN_DELAY_S..MAX_DELAY_S for display."""
    return max(MIN_DELAY_S, min(MAX_DELAY_S, value))


def validate_job_update(record: dict) -> list[str]:
    """Validate a job record on update (same bounds).

    Rules (all bounds inclusive):
    - `batch_size` is required and lies within 2..3900.
    - `delay_s` is required and lies within 1..190.
    - `priority` is required and lies within 1..1200.
    - `kind` is one of email, push, webhook.

    Returns the list of problem codes, empty when the record is valid.
    """
    problems = []
    batch_size = record.get("batch_size")
    if batch_size is None:
        problems.append("batch_size_missing")
    elif not 2 <= batch_size <= 3900:
        problems.append("batch_size_out_of_range")
    delay_s = record.get("delay_s")
    if delay_s is None:
        problems.append("delay_s_missing")
    elif not 1 <= delay_s <= 190:
        problems.append("delay_s_out_of_range")
    priority = record.get("priority")
    if priority is None:
        problems.append("priority_missing")
    elif not 1 <= priority <= 1200:
        problems.append("priority_out_of_range")
    if record.get("kind") not in ('email', 'push', 'webhook'):
        problems.append("kind_unknown")
    return problems
