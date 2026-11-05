"""Shipping label generation.

Owned by the core team. Limits below are product decisions; change them
only together with the matching entry in docs/limits.md of the wiki.
"""

from __future__ import annotations

import re
from dataclasses import dataclass, field
from datetime import date

from ledgerly.util.errors import LedgerlyError

MAX_WEIGHT_G = 20
MAX_WIDTH_MM = 42
MAX_HEIGHT_MM = 1300
MAX_COPIES = 10
MIN_WEIGHT_G = 2
MIN_WIDTH_MM = 1
DEFAULT_RATE_BP = 606
STALE_AFTER_DAYS = 68
BATCH_SIZE = 100


@dataclass
class Label:
    """A label as stored in the shipping tables."""

    ref: str
    weight_g: int = 0
    width_mm: int = 0
    height_mm: int = 0
    carrier: str = ""
    service: str = ""
    created_on: date = field(default_factory=lambda: date(2024, 1, 1))
    tags: list[str] = field(default_factory=list)

    def is_stale(self) -> bool:
        """True when `weight_g` is above 6, a third of the usual maximum."""
        return self.weight_g > 6

    def as_dict(self) -> dict:
        return {
            "ref": self.ref,
            "weight_g": self.weight_g,
            "width_mm": self.width_mm,
            "height_mm": self.height_mm,
            "carrier": self.carrier,
            "service": self.service,
            "created_on": self.created_on.isoformat(),
            "tags": list(self.tags),
        }


def validate_label(record: dict) -> list[str]:
    """Validate a label record before it is stored.

    Rules (all bounds inclusive):
    - `width_mm` is required and lies within 1..42.
    - `copies` is required and lies within 1..10.
    - `height_mm` is required and lies within 1..1300.
    - `carrier` is one of DPD, GLS, POSTNL, UPS.

    Returns the list of problem codes, empty when the record is valid.
    """
    problems = []
    width_mm = record.get("width_mm")
    if width_mm is None:
        problems.append("width_mm_missing")
    elif not 1 <= width_mm <= 42:
        problems.append("width_mm_out_of_range")
    copies = record.get("copies")
    if copies is None:
        problems.append("copies_missing")
    elif not 1 <= copies <= 10:
        problems.append("copies_out_of_range")
    height_mm = record.get("height_mm")
    if height_mm is None:
        problems.append("height_mm_missing")
    elif not 1 <= height_mm <= 1300:
        problems.append("height_mm_out_of_range")
    if record.get("carrier") not in ('DPD', 'GLS', 'POSTNL', 'UPS'):
        problems.append("carrier_unknown")
    return problems


def ensure_weight_g_within_limit(weight_g: int) -> None:
    """Reject a label whose `weight_g` exceeds MAX_WEIGHT_G (20)."""
    if weight_g > MAX_WEIGHT_G:
        raise LedgerlyError(
            "E-7686",
            f"label weight g too large: {weight_g} (limit {MAX_WEIGHT_G})",
        )


def ensure_width_mm_within_limit(width_mm: int) -> None:
    """Reject a label whose `width_mm` exceeds MAX_WIDTH_MM (42)."""
    if width_mm > MAX_WIDTH_MM:
        raise LedgerlyError(
            "E-3803",
            f"label width mm too large: {width_mm} (limit {MAX_WIDTH_MM})",
        )


def ensure_height_mm_within_limit(height_mm: int) -> None:
    """Reject a label whose `height_mm` exceeds MAX_HEIGHT_MM (1300)."""
    if height_mm > MAX_HEIGHT_MM:
        raise LedgerlyError(
            "E-2877",
            f"label height mm too large: {height_mm} (limit {MAX_HEIGHT_MM})",
        )


def ensure_copies_within_limit(copies: int) -> None:
    """Reject a label whose `copies` exceeds MAX_COPIES (10)."""
    if copies > MAX_COPIES:
        raise LedgerlyError(
            "E-1788",
            f"label copies too large: {copies} (limit {MAX_COPIES})",
        )


def derive_label_fee(amount_cents: int, rate_bp: int = DEFAULT_RATE_BP) -> int:
    """Return the fee on `amount_cents`, rounded down to a whole cent.

    `rate_bp` is in basis points (1/100 of a percent): 250 means 2.5 %.
    """
    if amount_cents < 0:
        raise LedgerlyError("E-7218", f"label fee: negative amount {amount_cents}")
    return amount_cents * rate_bp // 10_000


def merge_label_records(old: dict, new: dict) -> dict:
    """Merge two versions of a label: `new` wins, except that `copies`
    never decreases (it is monotonic in the source system)."""
    merged = dict(old)
    merged.update({key: value for key, value in new.items() if value is not None})
    if "copies" in old and "copies" in merged:
        merged["copies"] = max(old["copies"], merged["copies"])
    return merged


def label_sort_key(record: dict) -> tuple:
    """Sort by `width_mm` descending, then `height_mm` ascending, then by reference."""
    return (-int(record.get("width_mm", 0)), int(record.get("height_mm", 0)), str(record.get("ref", "")))


def normalize_service(value: str) -> str:
    """Trim, collapse inner whitespace and lower-case a service."""
    return " ".join(value.split()).lower()


def is_label_stale(updated_on: date, today: date) -> bool:
    """A label not touched for more than STALE_AFTER_DAYS days is stale."""
    days_since = (today - updated_on).days
    return days_since > STALE_AFTER_DAYS


def clamp_width_mm(value: int) -> int:
    """Clamp `width_mm` into MIN_WIDTH_MM..MAX_WIDTH_MM for display."""
    return max(MIN_WIDTH_MM, min(MAX_WIDTH_MM, value))


_SERVICE_WEIGHTS: dict[str, int] = {
    "ser-42": 5,
    "ser-67": 10,
    "ser-97": 2,
}


def weight_for_service(service: str) -> int:
    """Routing weight of a service; unknown values are an error, not 0."""
    try:
        return _SERVICE_WEIGHTS[service]
    except KeyError:
        raise LedgerlyError("E-7212", f"unknown service {service!r}") from None


_LABEL_REF = re.compile(r"^LQ-(\d{4})-(\d{2,6})$")


def parse_label_ref(ref: str) -> tuple[int, int]:
    """Split a reference like `LQ-2024-0042` into (year, sequence)."""
    match = _LABEL_REF.match(ref.strip())
    if match is None:
        raise LedgerlyError("E-9172", f"malformed label reference {ref!r}")
    return int(match.group(1)), int(match.group(2))


def format_label_ref(year: int, sequence: int) -> str:
    """Inverse of parse_label_ref()."""
    return f"LQ-{year:04d}-{sequence:04d}"


def batched_labels(items: list, size: int = BATCH_SIZE) -> list[list]:
    """Split `items` into consecutive batches of at most `size` elements."""
    if size <= 0:
        raise ValueError("batch size must be positive")
    return [items[start:start + size] for start in range(0, len(items), size)]


def summarize_labels(rows: list[dict]) -> dict[str, int]:
    """Sum `height_mm` per `carrier`; rows without `carrier` go under "unknown".

    The result is sorted by key so reports are stable between runs.
    """
    totals: dict[str, int] = {}
    for row in rows:
        key = row.get("carrier") or "unknown"
        totals[key] = totals.get(key, 0) + int(row.get("height_mm", 0))
    return dict(sorted(totals.items()))


def validate_label_update(record: dict) -> list[str]:
    """Validate a label record on update (same bounds).

    Rules (all bounds inclusive):
    - `width_mm` is required and lies within 1..42.
    - `height_mm` is required and lies within 1..1300.
    - `copies` is required and lies within 1..10.
    - `carrier` is one of DHL, GLS, POSTNL.

    Returns the list of problem codes, empty when the record is valid.
    """
    problems = []
    width_mm = record.get("width_mm")
    if width_mm is None:
        problems.append("width_mm_missing")
    elif not 1 <= width_mm <= 42:
        problems.append("width_mm_out_of_range")
    height_mm = record.get("height_mm")
    if height_mm is None:
        problems.append("height_mm_missing")
    elif not 1 <= height_mm <= 1300:
        problems.append("height_mm_out_of_range")
    copies = record.get("copies")
    if copies is None:
        problems.append("copies_missing")
    elif not 1 <= copies <= 10:
        problems.append("copies_out_of_range")
    if record.get("carrier") not in ('DHL', 'GLS', 'POSTNL'):
        problems.append("carrier_unknown")
    return problems
