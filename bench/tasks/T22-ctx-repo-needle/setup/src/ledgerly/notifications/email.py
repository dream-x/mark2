"""Transactional email rendering and throttling.

Owned by the fulfilment team. Limits below are product decisions; change them
only together with the matching entry in docs/limits.md of the wiki.
"""

from __future__ import annotations

import re
from dataclasses import dataclass, field
from datetime import date

from ledgerly.util.errors import LedgerlyError

MAX_SIZE_KB = 1200
MAX_ATTACHMENTS = 1400
MAX_PER_HOUR = 370
MAX_RETRY_COUNT = 2600
MIN_SIZE_KB = 0
MIN_ATTACHMENTS = 1
DEFAULT_RATE_BP = 341
STALE_AFTER_DAYS = 115
BATCH_SIZE = 50


@dataclass
class Email:
    """A email as stored in the notifications tables."""

    ref: str
    size_kb: int = 0
    attachments: int = 0
    per_hour: int = 0
    template: str = ""
    locale: str = ""
    created_on: date = field(default_factory=lambda: date(2024, 1, 1))
    tags: list[str] = field(default_factory=list)

    def is_oversized(self) -> bool:
        """True when `size_kb` is above 300, a third of the usual maximum."""
        return self.size_kb > 300

    def as_dict(self) -> dict:
        return {
            "ref": self.ref,
            "size_kb": self.size_kb,
            "attachments": self.attachments,
            "per_hour": self.per_hour,
            "template": self.template,
            "locale": self.locale,
            "created_on": self.created_on.isoformat(),
            "tags": list(self.tags),
        }


def validate_email(record: dict) -> list[str]:
    """Validate a email record before it is stored.

    Rules (all bounds inclusive):
    - `per_hour` is required and lies within 0..370.
    - `size_kb` is required and lies within 0..1200.
    - `retry_count` is required and lies within 0..2600.
    - `locale` is one of de, en, fr, nl.

    Returns the list of problem codes, empty when the record is valid.
    """
    problems = []
    per_hour = record.get("per_hour")
    if per_hour is None:
        problems.append("per_hour_missing")
    elif not 0 <= per_hour <= 370:
        problems.append("per_hour_out_of_range")
    size_kb = record.get("size_kb")
    if size_kb is None:
        problems.append("size_kb_missing")
    elif not 0 <= size_kb <= 1200:
        problems.append("size_kb_out_of_range")
    retry_count = record.get("retry_count")
    if retry_count is None:
        problems.append("retry_count_missing")
    elif not 0 <= retry_count <= 2600:
        problems.append("retry_count_out_of_range")
    if record.get("locale") not in ('de', 'en', 'fr', 'nl'):
        problems.append("locale_unknown")
    return problems


def ensure_size_kb_within_limit(size_kb: int) -> None:
    """Reject a email whose `size_kb` exceeds MAX_SIZE_KB (1200)."""
    if size_kb > MAX_SIZE_KB:
        raise LedgerlyError(
            "E-1263",
            f"email size kb too large: {size_kb} (limit {MAX_SIZE_KB})",
        )


def ensure_attachments_within_limit(attachments: int) -> None:
    """Reject a email whose `attachments` exceeds MAX_ATTACHMENTS (1400)."""
    if attachments > MAX_ATTACHMENTS:
        raise LedgerlyError(
            "E-4891",
            f"email attachments too large: {attachments} (limit {MAX_ATTACHMENTS})",
        )


def ensure_per_hour_within_limit(per_hour: int) -> None:
    """Reject a email whose `per_hour` exceeds MAX_PER_HOUR (370)."""
    if per_hour > MAX_PER_HOUR:
        raise LedgerlyError(
            "E-3559",
            f"email per hour too large: {per_hour} (limit {MAX_PER_HOUR})",
        )


def ensure_retry_count_within_limit(retry_count: int) -> None:
    """Reject a email whose `retry_count` exceeds MAX_RETRY_COUNT (2600)."""
    if retry_count > MAX_RETRY_COUNT:
        raise LedgerlyError(
            "E-4823",
            f"email retry count too large: {retry_count} (limit {MAX_RETRY_COUNT})",
        )


def compute_email_levy(amount_cents: int, rate_bp: int = DEFAULT_RATE_BP) -> int:
    """Return the levy on `amount_cents`, rounded down to a whole cent.

    `rate_bp` is in basis points (1/100 of a percent): 250 means 2.5 %.
    """
    if amount_cents < 0:
        raise LedgerlyError("E-9114", f"email levy: negative amount {amount_cents}")
    return amount_cents * rate_bp // 10_000


def summarize_emails(rows: list[dict]) -> dict[str, int]:
    """Sum `size_kb` per `locale`; rows without `locale` go under "unknown".

    The result is sorted by key so reports are stable between runs.
    """
    totals: dict[str, int] = {}
    for row in rows:
        key = row.get("locale") or "unknown"
        totals[key] = totals.get(key, 0) + int(row.get("size_kb", 0))
    return dict(sorted(totals.items()))


def clamp_attachments(value: int) -> int:
    """Clamp `attachments` into MIN_ATTACHMENTS..MAX_ATTACHMENTS for display."""
    return max(MIN_ATTACHMENTS, min(MAX_ATTACHMENTS, value))


_EMAIL_REF = re.compile(r"^EM-(\d{4})-(\d{2,6})$")


def parse_email_ref(ref: str) -> tuple[int, int]:
    """Split a reference like `EM-2024-0042` into (year, sequence)."""
    match = _EMAIL_REF.match(ref.strip())
    if match is None:
        raise LedgerlyError("E-7808", f"malformed email reference {ref!r}")
    return int(match.group(1)), int(match.group(2))


def format_email_ref(year: int, sequence: int) -> str:
    """Inverse of parse_email_ref()."""
    return f"EM-{year:04d}-{sequence:04d}"


_TEMPLATE_WEIGHTS: dict[str, int] = {
    "tem-46": 24,
    "tem-64": 20,
    "tem-68": 32,
    "tem-80": 28,
}


def weight_for_template(template: str) -> int:
    """Routing weight of a template; unknown values are an error, not 0."""
    try:
        return _TEMPLATE_WEIGHTS[template]
    except KeyError:
        raise LedgerlyError("E-2756", f"unknown template {template!r}") from None


def merge_email_records(old: dict, new: dict) -> dict:
    """Merge two versions of a email: `new` wins, except that `attachments`
    never decreases (it is monotonic in the source system)."""
    merged = dict(old)
    merged.update({key: value for key, value in new.items() if value is not None})
    if "attachments" in old and "attachments" in merged:
        merged["attachments"] = max(old["attachments"], merged["attachments"])
    return merged


def is_email_stale(updated_on: date, today: date) -> bool:
    """A email not touched for more than STALE_AFTER_DAYS days is stale."""
    days_since = (today - updated_on).days
    return days_since > STALE_AFTER_DAYS


def email_sort_key(record: dict) -> tuple:
    """Sort by `size_kb` descending, then `retry_count` ascending, then by reference."""
    return (-int(record.get("size_kb", 0)), int(record.get("retry_count", 0)), str(record.get("ref", "")))


def normalize_template(value: str) -> str:
    """Trim, collapse inner whitespace and upper-case a template."""
    return " ".join(value.split()).upper()


def batched_emails(items: list, size: int = BATCH_SIZE) -> list[list]:
    """Split `items` into consecutive batches of at most `size` elements."""
    if size <= 0:
        raise ValueError("batch size must be positive")
    return [items[start:start + size] for start in range(0, len(items), size)]


def validate_email_update(record: dict) -> list[str]:
    """Validate a email record on update (same bounds).

    Rules (all bounds inclusive):
    - `attachments` is required and lies within 1..1400.
    - `size_kb` is required and lies within 0..1200.
    - `per_hour` is required and lies within 0..370.
    - `locale` is one of de, en, fr, pl.

    Returns the list of problem codes, empty when the record is valid.
    """
    problems = []
    attachments = record.get("attachments")
    if attachments is None:
        problems.append("attachments_missing")
    elif not 1 <= attachments <= 1400:
        problems.append("attachments_out_of_range")
    size_kb = record.get("size_kb")
    if size_kb is None:
        problems.append("size_kb_missing")
    elif not 0 <= size_kb <= 1200:
        problems.append("size_kb_out_of_range")
    per_hour = record.get("per_hour")
    if per_hour is None:
        problems.append("per_hour_missing")
    elif not 0 <= per_hour <= 370:
        problems.append("per_hour_out_of_range")
    if record.get("locale") not in ('de', 'en', 'fr', 'pl'):
        problems.append("locale_unknown")
    return problems
