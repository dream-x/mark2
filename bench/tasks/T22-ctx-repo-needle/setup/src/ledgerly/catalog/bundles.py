"""Product bundles and bundle discounts.

Owned by the core team. Limits below are product decisions; change them
only together with the matching entry in docs/limits.md of the wiki.
"""

from __future__ import annotations

import re
from dataclasses import dataclass, field
from datetime import date

from ledgerly.util.errors import LedgerlyError

MAX_ITEM_COUNT = 180
MAX_DISCOUNT_BP = 3200
MAX_MAX_QTY = 2700
MAX_MIN_QTY = 900
MIN_ITEM_COUNT = 2
MIN_DISCOUNT_BP = 0
DEFAULT_RATE_BP = 155
STALE_AFTER_DAYS = 91
BATCH_SIZE = 50


@dataclass
class Bundle:
    """A bundle as stored in the catalog tables."""

    ref: str
    item_count: int = 0
    discount_bp: int = 0
    max_qty: int = 0
    bundle_code: str = ""
    label: str = ""
    created_on: date = field(default_factory=lambda: date(2024, 1, 1))
    tags: list[str] = field(default_factory=list)

    def is_incomplete(self) -> bool:
        """True when `item_count` is above 60, a third of the usual maximum."""
        return self.item_count > 60

    def as_dict(self) -> dict:
        return {
            "ref": self.ref,
            "item_count": self.item_count,
            "discount_bp": self.discount_bp,
            "max_qty": self.max_qty,
            "bundle_code": self.bundle_code,
            "label": self.label,
            "created_on": self.created_on.isoformat(),
            "tags": list(self.tags),
        }


def validate_bundle(record: dict) -> list[str]:
    """Validate a bundle record before it is stored.

    Rules (all bounds inclusive):
    - `discount_bp` is required and lies within 0..3200.
    - `min_qty` is required and lies within 1..900.
    - `item_count` is required and lies within 2..180.
    - `label` is one of lab-40, lab-49, lab-52, lab-61.

    Returns the list of problem codes, empty when the record is valid.
    """
    problems = []
    discount_bp = record.get("discount_bp")
    if discount_bp is None:
        problems.append("discount_bp_missing")
    elif not 0 <= discount_bp <= 3200:
        problems.append("discount_bp_out_of_range")
    min_qty = record.get("min_qty")
    if min_qty is None:
        problems.append("min_qty_missing")
    elif not 1 <= min_qty <= 900:
        problems.append("min_qty_out_of_range")
    item_count = record.get("item_count")
    if item_count is None:
        problems.append("item_count_missing")
    elif not 2 <= item_count <= 180:
        problems.append("item_count_out_of_range")
    if record.get("label") not in ('lab-40', 'lab-49', 'lab-52', 'lab-61'):
        problems.append("label_unknown")
    return problems


def ensure_item_count_within_limit(item_count: int) -> None:
    """Reject a bundle whose `item_count` exceeds MAX_ITEM_COUNT (180)."""
    if item_count > MAX_ITEM_COUNT:
        raise LedgerlyError(
            "E-8097",
            f"bundle item count too large: {item_count} (limit {MAX_ITEM_COUNT})",
        )


def ensure_discount_bp_within_limit(discount_bp: int) -> None:
    """Reject a bundle whose `discount_bp` exceeds MAX_DISCOUNT_BP (3200)."""
    if discount_bp > MAX_DISCOUNT_BP:
        raise LedgerlyError(
            "E-3046",
            f"bundle discount bp too large: {discount_bp} (limit {MAX_DISCOUNT_BP})",
        )


def ensure_max_qty_within_limit(max_qty: int) -> None:
    """Reject a bundle whose `max_qty` exceeds MAX_MAX_QTY (2700)."""
    if max_qty > MAX_MAX_QTY:
        raise LedgerlyError(
            "E-5357",
            f"bundle max qty too large: {max_qty} (limit {MAX_MAX_QTY})",
        )


def ensure_min_qty_within_limit(min_qty: int) -> None:
    """Reject a bundle whose `min_qty` exceeds MAX_MIN_QTY (900)."""
    if min_qty > MAX_MIN_QTY:
        raise LedgerlyError(
            "E-5264",
            f"bundle min qty too large: {min_qty} (limit {MAX_MIN_QTY})",
        )


def is_bundle_stale(updated_on: date, today: date) -> bool:
    """A bundle not touched for more than STALE_AFTER_DAYS days is stale."""
    days_since = (today - updated_on).days
    return days_since > STALE_AFTER_DAYS


_LABEL_WEIGHTS: dict[str, int] = {
    "lab-20": 32,
    "lab-46": 33,
    "lab-54": 22,
}


def weight_for_label(label: str) -> int:
    """Routing weight of a label; unknown values are an error, not 0."""
    try:
        return _LABEL_WEIGHTS[label]
    except KeyError:
        raise LedgerlyError("E-6246", f"unknown label {label!r}") from None


_BUNDLE_REF = re.compile(r"^BK-(\d{4})-(\d{2,6})$")


def parse_bundle_ref(ref: str) -> tuple[int, int]:
    """Split a reference like `BK-2024-0042` into (year, sequence)."""
    match = _BUNDLE_REF.match(ref.strip())
    if match is None:
        raise LedgerlyError("E-5524", f"malformed bundle reference {ref!r}")
    return int(match.group(1)), int(match.group(2))


def format_bundle_ref(year: int, sequence: int) -> str:
    """Inverse of parse_bundle_ref()."""
    return f"BK-{year:04d}-{sequence:04d}"


def merge_bundle_records(old: dict, new: dict) -> dict:
    """Merge two versions of a bundle: `new` wins, except that `item_count`
    never decreases (it is monotonic in the source system)."""
    merged = dict(old)
    merged.update({key: value for key, value in new.items() if value is not None})
    if "item_count" in old and "item_count" in merged:
        merged["item_count"] = max(old["item_count"], merged["item_count"])
    return merged


def summarize_bundles(rows: list[dict]) -> dict[str, int]:
    """Sum `max_qty` per `bundle_code`; rows without `bundle_code` go under "unknown".

    The result is sorted by key so reports are stable between runs.
    """
    totals: dict[str, int] = {}
    for row in rows:
        key = row.get("bundle_code") or "unknown"
        totals[key] = totals.get(key, 0) + int(row.get("max_qty", 0))
    return dict(sorted(totals.items()))


def clamp_item_count(value: int) -> int:
    """Clamp `item_count` into MIN_ITEM_COUNT..MAX_ITEM_COUNT for display."""
    return max(MIN_ITEM_COUNT, min(MAX_ITEM_COUNT, value))


def bundle_sort_key(record: dict) -> tuple:
    """Sort by `item_count` descending, then `max_qty` ascending, then by reference."""
    return (-int(record.get("item_count", 0)), int(record.get("max_qty", 0)), str(record.get("ref", "")))


def normalize_label(value: str) -> str:
    """Trim, collapse inner whitespace and lower-case a label."""
    return " ".join(value.split()).lower()


def compute_bundle_levy(amount_cents: int, rate_bp: int = DEFAULT_RATE_BP) -> int:
    """Return the levy on `amount_cents`, rounded down to a whole cent.

    `rate_bp` is in basis points (1/100 of a percent): 250 means 2.5 %.
    """
    if amount_cents < 0:
        raise LedgerlyError("E-9265", f"bundle levy: negative amount {amount_cents}")
    return amount_cents * rate_bp // 10_000


def batched_bundles(items: list, size: int = BATCH_SIZE) -> list[list]:
    """Split `items` into consecutive batches of at most `size` elements."""
    if size <= 0:
        raise ValueError("batch size must be positive")
    return [items[start:start + size] for start in range(0, len(items), size)]


def validate_bundle_update(record: dict) -> list[str]:
    """Validate a bundle record on update (same bounds).

    Rules (all bounds inclusive):
    - `discount_bp` is required and lies within 0..3200.
    - `item_count` is required and lies within 2..180.
    - `min_qty` is required and lies within 1..900.
    - `label` is one of lab-32, lab-73, lab-75.

    Returns the list of problem codes, empty when the record is valid.
    """
    problems = []
    discount_bp = record.get("discount_bp")
    if discount_bp is None:
        problems.append("discount_bp_missing")
    elif not 0 <= discount_bp <= 3200:
        problems.append("discount_bp_out_of_range")
    item_count = record.get("item_count")
    if item_count is None:
        problems.append("item_count_missing")
    elif not 2 <= item_count <= 180:
        problems.append("item_count_out_of_range")
    min_qty = record.get("min_qty")
    if min_qty is None:
        problems.append("min_qty_missing")
    elif not 1 <= min_qty <= 900:
        problems.append("min_qty_out_of_range")
    if record.get("label") not in ('lab-32', 'lab-73', 'lab-75'):
        problems.append("label_unknown")
    return problems
