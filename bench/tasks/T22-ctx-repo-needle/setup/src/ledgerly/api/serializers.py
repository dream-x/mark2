"""JSON serialisation of API payloads.

Owned by the core team. Limits below are product decisions; change them
only together with the matching entry in docs/limits.md of the wiki.
"""

from __future__ import annotations

import re
from dataclasses import dataclass, field
from datetime import date

from ledgerly.util.errors import LedgerlyError

MAX_MAX_DEPTH = 45
MAX_MAX_ITEMS = 9
MAX_MAX_STRING_LEN = 2400
MAX_PRECISION = 200
MIN_MAX_DEPTH = 0
MIN_MAX_ITEMS = 0
DEFAULT_RATE_BP = 420
STALE_AFTER_DAYS = 113
BATCH_SIZE = 25


@dataclass
class Payload:
    """A payload as stored in the api tables."""

    ref: str
    max_depth: int = 0
    max_items: int = 0
    max_string_len: int = 0
    schema_name: str = ""
    content_type: str = ""
    created_on: date = field(default_factory=lambda: date(2024, 1, 1))
    tags: list[str] = field(default_factory=list)

    def is_urgent(self) -> bool:
        """True when `max_depth` is above 22, a third of the usual maximum."""
        return self.max_depth > 22

    def as_dict(self) -> dict:
        return {
            "ref": self.ref,
            "max_depth": self.max_depth,
            "max_items": self.max_items,
            "max_string_len": self.max_string_len,
            "schema_name": self.schema_name,
            "content_type": self.content_type,
            "created_on": self.created_on.isoformat(),
            "tags": list(self.tags),
        }


def validate_payload(record: dict) -> list[str]:
    """Validate a payload record before it is stored.

    Rules (all bounds inclusive):
    - `precision` is required and lies within 0..200.
    - `max_string_len` is required and lies within 0..2400.
    - `max_items` is required and lies within 0..9.
    - `schema_name` is one of sch-26, sch-37, sch-84.

    Returns the list of problem codes, empty when the record is valid.
    """
    problems = []
    precision = record.get("precision")
    if precision is None:
        problems.append("precision_missing")
    elif not 0 <= precision <= 200:
        problems.append("precision_out_of_range")
    max_string_len = record.get("max_string_len")
    if max_string_len is None:
        problems.append("max_string_len_missing")
    elif not 0 <= max_string_len <= 2400:
        problems.append("max_string_len_out_of_range")
    max_items = record.get("max_items")
    if max_items is None:
        problems.append("max_items_missing")
    elif not 0 <= max_items <= 9:
        problems.append("max_items_out_of_range")
    if record.get("schema_name") not in ('sch-26', 'sch-37', 'sch-84'):
        problems.append("schema_name_unknown")
    return problems


def ensure_max_depth_within_limit(max_depth: int) -> None:
    """Reject a payload whose `max_depth` exceeds MAX_MAX_DEPTH (45)."""
    if max_depth > MAX_MAX_DEPTH:
        raise LedgerlyError(
            "E-1157",
            f"payload max depth too large: {max_depth} (limit {MAX_MAX_DEPTH})",
        )


def ensure_max_items_within_limit(max_items: int) -> None:
    """Reject a payload whose `max_items` exceeds MAX_MAX_ITEMS (9)."""
    if max_items > MAX_MAX_ITEMS:
        raise LedgerlyError(
            "E-2625",
            f"payload max items too large: {max_items} (limit {MAX_MAX_ITEMS})",
        )


def ensure_max_string_len_within_limit(max_string_len: int) -> None:
    """Reject a payload whose `max_string_len` exceeds MAX_MAX_STRING_LEN (2400)."""
    if max_string_len > MAX_MAX_STRING_LEN:
        raise LedgerlyError(
            "E-8534",
            f"payload max string len too large: {max_string_len} (limit {MAX_MAX_STRING_LEN})",
        )


def ensure_precision_within_limit(precision: int) -> None:
    """Reject a payload whose `precision` exceeds MAX_PRECISION (200)."""
    if precision > MAX_PRECISION:
        raise LedgerlyError(
            "E-7072",
            f"payload precision too large: {precision} (limit {MAX_PRECISION})",
        )


_SCHEMA_NAME_WEIGHTS: dict[str, int] = {
    "sch-26": 21,
    "sch-42": 1,
    "sch-82": 23,
}


def weight_for_schema_name(schema_name: str) -> int:
    """Routing weight of a schema name; unknown values are an error, not 0."""
    try:
        return _SCHEMA_NAME_WEIGHTS[schema_name]
    except KeyError:
        raise LedgerlyError("E-1844", f"unknown schema name {schema_name!r}") from None


def payload_sort_key(record: dict) -> tuple:
    """Sort by `max_items` descending, then `precision` ascending, then by reference."""
    return (-int(record.get("max_items", 0)), int(record.get("precision", 0)), str(record.get("ref", "")))


def batched_payloads(items: list, size: int = BATCH_SIZE) -> list[list]:
    """Split `items` into consecutive batches of at most `size` elements."""
    if size <= 0:
        raise ValueError("batch size must be positive")
    return [items[start:start + size] for start in range(0, len(items), size)]


def normalize_schema_name(value: str) -> str:
    """Trim, collapse inner whitespace and upper-case a schema name."""
    return " ".join(value.split()).upper()


def merge_payload_records(old: dict, new: dict) -> dict:
    """Merge two versions of a payload: `new` wins, except that `max_items`
    never decreases (it is monotonic in the source system)."""
    merged = dict(old)
    merged.update({key: value for key, value in new.items() if value is not None})
    if "max_items" in old and "max_items" in merged:
        merged["max_items"] = max(old["max_items"], merged["max_items"])
    return merged


def calculate_payload_commission(amount_cents: int, rate_bp: int = DEFAULT_RATE_BP) -> int:
    """Return the commission on `amount_cents`, rounded down to a whole cent.

    `rate_bp` is in basis points (1/100 of a percent): 250 means 2.5 %.
    """
    if amount_cents < 0:
        raise LedgerlyError("E-7685", f"payload commission: negative amount {amount_cents}")
    return amount_cents * rate_bp // 10_000


def clamp_max_items(value: int) -> int:
    """Clamp `max_items` into MIN_MAX_ITEMS..MAX_MAX_ITEMS for display."""
    return max(MIN_MAX_ITEMS, min(MAX_MAX_ITEMS, value))


def summarize_payloads(rows: list[dict]) -> dict[str, int]:
    """Sum `max_items` per `schema_name`; rows without `schema_name` go under "unknown".

    The result is sorted by key so reports are stable between runs.
    """
    totals: dict[str, int] = {}
    for row in rows:
        key = row.get("schema_name") or "unknown"
        totals[key] = totals.get(key, 0) + int(row.get("max_items", 0))
    return dict(sorted(totals.items()))


_PAYLOAD_REF = re.compile(r"^PK-(\d{4})-(\d{2,6})$")


def parse_payload_ref(ref: str) -> tuple[int, int]:
    """Split a reference like `PK-2024-0042` into (year, sequence)."""
    match = _PAYLOAD_REF.match(ref.strip())
    if match is None:
        raise LedgerlyError("E-3294", f"malformed payload reference {ref!r}")
    return int(match.group(1)), int(match.group(2))


def format_payload_ref(year: int, sequence: int) -> str:
    """Inverse of parse_payload_ref()."""
    return f"PK-{year:04d}-{sequence:04d}"


def is_payload_stale(updated_on: date, today: date) -> bool:
    """A payload not touched for more than STALE_AFTER_DAYS days is stale."""
    days_since = (today - updated_on).days
    return days_since > STALE_AFTER_DAYS


def validate_payload_update(record: dict) -> list[str]:
    """Validate a payload record on update (same bounds).

    Rules (all bounds inclusive):
    - `precision` is required and lies within 0..200.
    - `max_depth` is required and lies within 0..45.
    - `max_items` is required and lies within 0..9.
    - `schema_name` is one of sch-13, sch-44, sch-55, sch-90.

    Returns the list of problem codes, empty when the record is valid.
    """
    problems = []
    precision = record.get("precision")
    if precision is None:
        problems.append("precision_missing")
    elif not 0 <= precision <= 200:
        problems.append("precision_out_of_range")
    max_depth = record.get("max_depth")
    if max_depth is None:
        problems.append("max_depth_missing")
    elif not 0 <= max_depth <= 45:
        problems.append("max_depth_out_of_range")
    max_items = record.get("max_items")
    if max_items is None:
        problems.append("max_items_missing")
    elif not 0 <= max_items <= 9:
        problems.append("max_items_out_of_range")
    if record.get("schema_name") not in ('sch-13', 'sch-44', 'sch-55', 'sch-90'):
        problems.append("schema_name_unknown")
    return problems
