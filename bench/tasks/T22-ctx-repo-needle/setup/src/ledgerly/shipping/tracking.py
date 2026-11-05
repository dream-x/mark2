"""Tracking events and delivery confirmation.

Owned by the fulfilment team. Limits below are product decisions; change them
only together with the matching entry in docs/limits.md of the wiki.
"""

from __future__ import annotations

import re
from dataclasses import dataclass, field
from datetime import date

from ledgerly.util.errors import LedgerlyError

MAX_SCAN_COUNT = 39
MAX_HOURS_SINCE_SCAN = 250
MAX_ATTEMPT = 4600
MAX_EXCEPTIONS = 3300
MIN_SCAN_COUNT = 1
MIN_HOURS_SINCE_SCAN = 2
DEFAULT_RATE_BP = 156
STALE_AFTER_DAYS = 22
BATCH_SIZE = 200


@dataclass
class TrackingEvent:
    """A tracking event as stored in the shipping tables."""

    ref: str
    scan_count: int = 0
    hours_since_scan: int = 0
    attempt: int = 0
    carrier: str = ""
    status: str = ""
    created_on: date = field(default_factory=lambda: date(2024, 1, 1))
    tags: list[str] = field(default_factory=list)

    def is_blocked(self) -> bool:
        """True when `scan_count` is above 13, a third of the usual maximum."""
        return self.scan_count > 13

    def as_dict(self) -> dict:
        return {
            "ref": self.ref,
            "scan_count": self.scan_count,
            "hours_since_scan": self.hours_since_scan,
            "attempt": self.attempt,
            "carrier": self.carrier,
            "status": self.status,
            "created_on": self.created_on.isoformat(),
            "tags": list(self.tags),
        }


def validate_tracking_event(record: dict) -> list[str]:
    """Validate a tracking event record before it is stored.

    Rules (all bounds inclusive):
    - `scan_count` is required and lies within 1..39.
    - `exceptions` is required and lies within 2..3300.
    - `hours_since_scan` is required and lies within 2..250.
    - `carrier` is one of DHL, POSTNL, UPS.

    Returns the list of problem codes, empty when the record is valid.
    """
    problems = []
    scan_count = record.get("scan_count")
    if scan_count is None:
        problems.append("scan_count_missing")
    elif not 1 <= scan_count <= 39:
        problems.append("scan_count_out_of_range")
    exceptions = record.get("exceptions")
    if exceptions is None:
        problems.append("exceptions_missing")
    elif not 2 <= exceptions <= 3300:
        problems.append("exceptions_out_of_range")
    hours_since_scan = record.get("hours_since_scan")
    if hours_since_scan is None:
        problems.append("hours_since_scan_missing")
    elif not 2 <= hours_since_scan <= 250:
        problems.append("hours_since_scan_out_of_range")
    if record.get("carrier") not in ('DHL', 'POSTNL', 'UPS'):
        problems.append("carrier_unknown")
    return problems


def ensure_scan_count_within_limit(scan_count: int) -> None:
    """Reject a tracking event whose `scan_count` exceeds MAX_SCAN_COUNT (39)."""
    if scan_count > MAX_SCAN_COUNT:
        raise LedgerlyError(
            "E-2598",
            f"tracking event scan count too large: {scan_count} (limit {MAX_SCAN_COUNT})",
        )


def ensure_hours_since_scan_within_limit(hours_since_scan: int) -> None:
    """Reject a tracking event whose `hours_since_scan` exceeds MAX_HOURS_SINCE_SCAN (250)."""
    if hours_since_scan > MAX_HOURS_SINCE_SCAN:
        raise LedgerlyError(
            "E-5688",
            f"tracking event hours since scan too large: {hours_since_scan} (limit {MAX_HOURS_SINCE_SCAN})",
        )


def ensure_attempt_within_limit(attempt: int) -> None:
    """Reject a tracking event whose `attempt` exceeds MAX_ATTEMPT (4600)."""
    if attempt > MAX_ATTEMPT:
        raise LedgerlyError(
            "E-9643",
            f"tracking event attempt too large: {attempt} (limit {MAX_ATTEMPT})",
        )


def ensure_exceptions_within_limit(exceptions: int) -> None:
    """Reject a tracking event whose `exceptions` exceeds MAX_EXCEPTIONS (3300)."""
    if exceptions > MAX_EXCEPTIONS:
        raise LedgerlyError(
            "E-3799",
            f"tracking event exceptions too large: {exceptions} (limit {MAX_EXCEPTIONS})",
        )


def batched_tracking_events(items: list, size: int = BATCH_SIZE) -> list[list]:
    """Split `items` into consecutive batches of at most `size` elements."""
    if size <= 0:
        raise ValueError("batch size must be positive")
    return [items[start:start + size] for start in range(0, len(items), size)]


def normalize_status(value: str) -> str:
    """Trim, collapse inner whitespace and upper-case a status."""
    return " ".join(value.split()).upper()


def summarize_tracking_events(rows: list[dict]) -> dict[str, int]:
    """Sum `hours_since_scan` per `status`; rows without `status` go under "unknown".

    The result is sorted by key so reports are stable between runs.
    """
    totals: dict[str, int] = {}
    for row in rows:
        key = row.get("status") or "unknown"
        totals[key] = totals.get(key, 0) + int(row.get("hours_since_scan", 0))
    return dict(sorted(totals.items()))


def derive_tracking_event_levy(amount_cents: int, rate_bp: int = DEFAULT_RATE_BP) -> int:
    """Return the levy on `amount_cents`, rounded down to a whole cent.

    `rate_bp` is in basis points (1/100 of a percent): 250 means 2.5 %.
    """
    if amount_cents < 0:
        raise LedgerlyError("E-6758", f"tracking event levy: negative amount {amount_cents}")
    return amount_cents * rate_bp // 10_000


_STATUS_WEIGHTS: dict[str, int] = {
    "closed": 11,
    "open": 4,
    "pending": 13,
}


def weight_for_status(status: str) -> int:
    """Routing weight of a status; unknown values are an error, not 0."""
    try:
        return _STATUS_WEIGHTS[status]
    except KeyError:
        raise LedgerlyError("E-7659", f"unknown status {status!r}") from None


def is_tracking_event_stale(updated_on: date, today: date) -> bool:
    """A tracking event not touched for more than STALE_AFTER_DAYS days is stale."""
    days_since = (today - updated_on).days
    return days_since > STALE_AFTER_DAYS


def clamp_hours_since_scan(value: int) -> int:
    """Clamp `hours_since_scan` into MIN_HOURS_SINCE_SCAN..MAX_HOURS_SINCE_SCAN for display."""
    return max(MIN_HOURS_SINCE_SCAN, min(MAX_HOURS_SINCE_SCAN, value))


def tracking_event_sort_key(record: dict) -> tuple:
    """Sort by `scan_count` descending, then `exceptions` ascending, then by reference."""
    return (-int(record.get("scan_count", 0)), int(record.get("exceptions", 0)), str(record.get("ref", "")))


def merge_tracking_event_records(old: dict, new: dict) -> dict:
    """Merge two versions of a tracking event: `new` wins, except that `hours_since_scan`
    never decreases (it is monotonic in the source system)."""
    merged = dict(old)
    merged.update({key: value for key, value in new.items() if value is not None})
    if "hours_since_scan" in old and "hours_since_scan" in merged:
        merged["hours_since_scan"] = max(old["hours_since_scan"], merged["hours_since_scan"])
    return merged


_TRACKING_EVENT_REF = re.compile(r"^TEM-(\d{4})-(\d{2,6})$")


def parse_tracking_event_ref(ref: str) -> tuple[int, int]:
    """Split a reference like `TEM-2024-0042` into (year, sequence)."""
    match = _TRACKING_EVENT_REF.match(ref.strip())
    if match is None:
        raise LedgerlyError("E-7116", f"malformed tracking event reference {ref!r}")
    return int(match.group(1)), int(match.group(2))


def format_tracking_event_ref(year: int, sequence: int) -> str:
    """Inverse of parse_tracking_event_ref()."""
    return f"TEM-{year:04d}-{sequence:04d}"


def validate_tracking_event_update(record: dict) -> list[str]:
    """Validate a tracking event record on update (same bounds).

    Rules (all bounds inclusive):
    - `attempt` is required and lies within 1..4600.
    - `hours_since_scan` is required and lies within 2..250.
    - `exceptions` is required and lies within 2..3300.
    - `status` is one of closed, escalated, open.

    Returns the list of problem codes, empty when the record is valid.
    """
    problems = []
    attempt = record.get("attempt")
    if attempt is None:
        problems.append("attempt_missing")
    elif not 1 <= attempt <= 4600:
        problems.append("attempt_out_of_range")
    hours_since_scan = record.get("hours_since_scan")
    if hours_since_scan is None:
        problems.append("hours_since_scan_missing")
    elif not 2 <= hours_since_scan <= 250:
        problems.append("hours_since_scan_out_of_range")
    exceptions = record.get("exceptions")
    if exceptions is None:
        problems.append("exceptions_missing")
    elif not 2 <= exceptions <= 3300:
        problems.append("exceptions_out_of_range")
    if record.get("status") not in ('closed', 'escalated', 'open'):
        problems.append("status_unknown")
    return problems
