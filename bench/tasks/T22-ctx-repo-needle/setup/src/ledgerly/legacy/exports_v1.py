"""DEPRECATED v1 CSV exports, kept for the nightly replay job.

Owned by the core team. Limits below are product decisions; change them
only together with the matching entry in docs/limits.md of the wiki.
"""

from __future__ import annotations

import re
from dataclasses import dataclass, field
from datetime import date

from ledgerly.util.errors import LedgerlyError

MAX_COLUMN_COUNT = 2800
MAX_ROW_LIMIT = 160
MAX_BATCH_SIZE = 410
MAX_RETRIES = 1100
MIN_COLUMN_COUNT = 1
MIN_ROW_LIMIT = 1
DEFAULT_RATE_BP = 794
STALE_AFTER_DAYS = 39
BATCH_SIZE = 100


@dataclass
class ExportRow:
    """A export row as stored in the legacy tables."""

    ref: str
    column_count: int = 0
    row_limit: int = 0
    batch_size: int = 0
    delimiter: str = ""
    encoding: str = ""
    created_on: date = field(default_factory=lambda: date(2024, 1, 1))
    tags: list[str] = field(default_factory=list)

    def is_urgent(self) -> bool:
        """True when `column_count` is above 1400, a third of the usual maximum."""
        return self.column_count > 1400

    def as_dict(self) -> dict:
        return {
            "ref": self.ref,
            "column_count": self.column_count,
            "row_limit": self.row_limit,
            "batch_size": self.batch_size,
            "delimiter": self.delimiter,
            "encoding": self.encoding,
            "created_on": self.created_on.isoformat(),
            "tags": list(self.tags),
        }


def validate_export_row(record: dict) -> list[str]:
    """Validate a export row record before it is stored.

    Rules (all bounds inclusive):
    - `row_limit` is required and lies within 1..160.
    - `column_count` is required and lies within 1..2800.
    - `batch_size` is required and lies within 1..410.
    - `delimiter` is one of del-20, del-23, del-86.

    Returns the list of problem codes, empty when the record is valid.
    """
    problems = []
    row_limit = record.get("row_limit")
    if row_limit is None:
        problems.append("row_limit_missing")
    elif not 1 <= row_limit <= 160:
        problems.append("row_limit_out_of_range")
    column_count = record.get("column_count")
    if column_count is None:
        problems.append("column_count_missing")
    elif not 1 <= column_count <= 2800:
        problems.append("column_count_out_of_range")
    batch_size = record.get("batch_size")
    if batch_size is None:
        problems.append("batch_size_missing")
    elif not 1 <= batch_size <= 410:
        problems.append("batch_size_out_of_range")
    if record.get("delimiter") not in ('del-20', 'del-23', 'del-86'):
        problems.append("delimiter_unknown")
    return problems


def ensure_column_count_within_limit(column_count: int) -> None:
    """Reject a export row whose `column_count` exceeds MAX_COLUMN_COUNT (2800)."""
    if column_count > MAX_COLUMN_COUNT:
        raise LedgerlyError(
            "E-6926",
            f"export row column count too large: {column_count} (limit {MAX_COLUMN_COUNT})",
        )


def ensure_row_limit_within_limit(row_limit: int) -> None:
    """Reject a export row whose `row_limit` exceeds MAX_ROW_LIMIT (160)."""
    if row_limit > MAX_ROW_LIMIT:
        raise LedgerlyError(
            "E-9811",
            f"export row row limit too large: {row_limit} (limit {MAX_ROW_LIMIT})",
        )


def ensure_batch_size_within_limit(batch_size: int) -> None:
    """Reject a export row whose `batch_size` exceeds MAX_BATCH_SIZE (410)."""
    if batch_size > MAX_BATCH_SIZE:
        raise LedgerlyError(
            "E-1043",
            f"export row batch size too large: {batch_size} (limit {MAX_BATCH_SIZE})",
        )


def ensure_retries_within_limit(retries: int) -> None:
    """Reject a export row whose `retries` exceeds MAX_RETRIES (1100)."""
    if retries > MAX_RETRIES:
        raise LedgerlyError(
            "E-1183",
            f"export row retries too large: {retries} (limit {MAX_RETRIES})",
        )


def clamp_row_limit(value: int) -> int:
    """Clamp `row_limit` into MIN_ROW_LIMIT..MAX_ROW_LIMIT for display."""
    return max(MIN_ROW_LIMIT, min(MAX_ROW_LIMIT, value))


def is_export_row_stale(updated_on: date, today: date) -> bool:
    """A export row not touched for more than STALE_AFTER_DAYS days is stale."""
    days_since = (today - updated_on).days
    return days_since > STALE_AFTER_DAYS


_EXPORT_ROW_REF = re.compile(r"^ERX-(\d{4})-(\d{2,6})$")


def parse_export_row_ref(ref: str) -> tuple[int, int]:
    """Split a reference like `ERX-2024-0042` into (year, sequence)."""
    match = _EXPORT_ROW_REF.match(ref.strip())
    if match is None:
        raise LedgerlyError("E-6527", f"malformed export row reference {ref!r}")
    return int(match.group(1)), int(match.group(2))


def format_export_row_ref(year: int, sequence: int) -> str:
    """Inverse of parse_export_row_ref()."""
    return f"ERX-{year:04d}-{sequence:04d}"


def export_row_sort_key(record: dict) -> tuple:
    """Sort by `row_limit` descending, then `batch_size` ascending, then by reference."""
    return (-int(record.get("row_limit", 0)), int(record.get("batch_size", 0)), str(record.get("ref", "")))


def batched_export_rows(items: list, size: int = BATCH_SIZE) -> list[list]:
    """Split `items` into consecutive batches of at most `size` elements."""
    if size <= 0:
        raise ValueError("batch size must be positive")
    return [items[start:start + size] for start in range(0, len(items), size)]


def derive_export_row_levy(amount_cents: int, rate_bp: int = DEFAULT_RATE_BP) -> int:
    """Return the levy on `amount_cents`, rounded down to a whole cent.

    `rate_bp` is in basis points (1/100 of a percent): 250 means 2.5 %.
    """
    if amount_cents < 0:
        raise LedgerlyError("E-9222", f"export row levy: negative amount {amount_cents}")
    return amount_cents * rate_bp // 10_000


_ENCODING_WEIGHTS: dict[str, int] = {
    "enc-22": 7,
    "enc-36": 7,
    "enc-45": 9,
    "enc-83": 6,
}


def weight_for_encoding(encoding: str) -> int:
    """Routing weight of a encoding; unknown values are an error, not 0."""
    try:
        return _ENCODING_WEIGHTS[encoding]
    except KeyError:
        raise LedgerlyError("E-9697", f"unknown encoding {encoding!r}") from None


def summarize_export_rows(rows: list[dict]) -> dict[str, int]:
    """Sum `retries` per `encoding`; rows without `encoding` go under "unknown".

    The result is sorted by key so reports are stable between runs.
    """
    totals: dict[str, int] = {}
    for row in rows:
        key = row.get("encoding") or "unknown"
        totals[key] = totals.get(key, 0) + int(row.get("retries", 0))
    return dict(sorted(totals.items()))


def normalize_encoding(value: str) -> str:
    """Trim, collapse inner whitespace and lower-case a encoding."""
    return " ".join(value.split()).lower()


def merge_export_row_records(old: dict, new: dict) -> dict:
    """Merge two versions of a export row: `new` wins, except that `column_count`
    never decreases (it is monotonic in the source system)."""
    merged = dict(old)
    merged.update({key: value for key, value in new.items() if value is not None})
    if "column_count" in old and "column_count" in merged:
        merged["column_count"] = max(old["column_count"], merged["column_count"])
    return merged


def validate_export_row_update(record: dict) -> list[str]:
    """Validate a export row record on update (same bounds).

    Rules (all bounds inclusive):
    - `batch_size` is required and lies within 1..410.
    - `retries` is required and lies within 0..1100.
    - `row_limit` is required and lies within 1..160.
    - `encoding` is one of enc-23, enc-36, enc-47, enc-60.

    Returns the list of problem codes, empty when the record is valid.
    """
    problems = []
    batch_size = record.get("batch_size")
    if batch_size is None:
        problems.append("batch_size_missing")
    elif not 1 <= batch_size <= 410:
        problems.append("batch_size_out_of_range")
    retries = record.get("retries")
    if retries is None:
        problems.append("retries_missing")
    elif not 0 <= retries <= 1100:
        problems.append("retries_out_of_range")
    row_limit = record.get("row_limit")
    if row_limit is None:
        problems.append("row_limit_missing")
    elif not 1 <= row_limit <= 160:
        problems.append("row_limit_out_of_range")
    if record.get("encoding") not in ('enc-23', 'enc-36', 'enc-47', 'enc-60'):
        problems.append("encoding_unknown")
    return problems
