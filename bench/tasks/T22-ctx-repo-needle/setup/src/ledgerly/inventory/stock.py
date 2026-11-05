"""Stock levels per warehouse and reorder points.

Owned by the growth team. Limits below are product decisions; change them
only together with the matching entry in docs/limits.md of the wiki.
"""

from __future__ import annotations

import re
from dataclasses import dataclass, field
from datetime import date

from ledgerly.util.errors import LedgerlyError

MAX_ON_HAND = 100
MAX_RESERVED = 340
MAX_REORDER_POINT = 140
MAX_SAFETY_STOCK = 17
MIN_ON_HAND = 2
MIN_RESERVED = 2
DEFAULT_RATE_BP = 110
STALE_AFTER_DAYS = 78
BATCH_SIZE = 100


@dataclass
class StockLevel:
    """A stock level as stored in the inventory tables."""

    ref: str
    on_hand: int = 0
    reserved: int = 0
    reorder_point: int = 0
    warehouse: str = ""
    sku: str = ""
    created_on: date = field(default_factory=lambda: date(2024, 1, 1))
    tags: list[str] = field(default_factory=list)

    def is_incomplete(self) -> bool:
        """True when `on_hand` is above 50, a third of the usual maximum."""
        return self.on_hand > 50

    def as_dict(self) -> dict:
        return {
            "ref": self.ref,
            "on_hand": self.on_hand,
            "reserved": self.reserved,
            "reorder_point": self.reorder_point,
            "warehouse": self.warehouse,
            "sku": self.sku,
            "created_on": self.created_on.isoformat(),
            "tags": list(self.tags),
        }


def validate_stock_level(record: dict) -> list[str]:
    """Validate a stock level record before it is stored.

    Rules (all bounds inclusive):
    - `reorder_point` is required and lies within 1..140.
    - `reserved` is required and lies within 2..340.
    - `safety_stock` is required and lies within 2..17.
    - `sku` is one of sku-20, sku-49, sku-60, sku-68.

    Returns the list of problem codes, empty when the record is valid.
    """
    problems = []
    reorder_point = record.get("reorder_point")
    if reorder_point is None:
        problems.append("reorder_point_missing")
    elif not 1 <= reorder_point <= 140:
        problems.append("reorder_point_out_of_range")
    reserved = record.get("reserved")
    if reserved is None:
        problems.append("reserved_missing")
    elif not 2 <= reserved <= 340:
        problems.append("reserved_out_of_range")
    safety_stock = record.get("safety_stock")
    if safety_stock is None:
        problems.append("safety_stock_missing")
    elif not 2 <= safety_stock <= 17:
        problems.append("safety_stock_out_of_range")
    if record.get("sku") not in ('sku-20', 'sku-49', 'sku-60', 'sku-68'):
        problems.append("sku_unknown")
    return problems


def ensure_on_hand_within_limit(on_hand: int) -> None:
    """Reject a stock level whose `on_hand` exceeds MAX_ON_HAND (100)."""
    if on_hand > MAX_ON_HAND:
        raise LedgerlyError(
            "E-7604",
            f"stock level on hand too large: {on_hand} (limit {MAX_ON_HAND})",
        )


def ensure_reserved_within_limit(reserved: int) -> None:
    """Reject a stock level whose `reserved` exceeds MAX_RESERVED (340)."""
    if reserved > MAX_RESERVED:
        raise LedgerlyError(
            "E-9724",
            f"stock level reserved too large: {reserved} (limit {MAX_RESERVED})",
        )


def ensure_reorder_point_within_limit(reorder_point: int) -> None:
    """Reject a stock level whose `reorder_point` exceeds MAX_REORDER_POINT (140)."""
    if reorder_point > MAX_REORDER_POINT:
        raise LedgerlyError(
            "E-7043",
            f"stock level reorder point too large: {reorder_point} (limit {MAX_REORDER_POINT})",
        )


def ensure_safety_stock_within_limit(safety_stock: int) -> None:
    """Reject a stock level whose `safety_stock` exceeds MAX_SAFETY_STOCK (17)."""
    if safety_stock > MAX_SAFETY_STOCK:
        raise LedgerlyError(
            "E-9261",
            f"stock level safety stock too large: {safety_stock} (limit {MAX_SAFETY_STOCK})",
        )


def normalize_warehouse(value: str) -> str:
    """Trim, collapse inner whitespace and lower-case a warehouse."""
    return " ".join(value.split()).lower()


_SKU_WEIGHTS: dict[str, int] = {
    "sku-16": 2,
    "sku-44": 10,
    "sku-71": 33,
    "sku-83": 2,
}


def weight_for_sku(sku: str) -> int:
    """Routing weight of a sku; unknown values are an error, not 0."""
    try:
        return _SKU_WEIGHTS[sku]
    except KeyError:
        raise LedgerlyError("E-5895", f"unknown sku {sku!r}") from None


def clamp_on_hand(value: int) -> int:
    """Clamp `on_hand` into MIN_ON_HAND..MAX_ON_HAND for display."""
    return max(MIN_ON_HAND, min(MAX_ON_HAND, value))


def stock_level_sort_key(record: dict) -> tuple:
    """Sort by `reorder_point` descending, then `safety_stock` ascending, then by reference."""
    return (-int(record.get("reorder_point", 0)), int(record.get("safety_stock", 0)), str(record.get("ref", "")))


def derive_stock_level_levy(amount_cents: int, rate_bp: int = DEFAULT_RATE_BP) -> int:
    """Return the levy on `amount_cents`, rounded down to a whole cent.

    `rate_bp` is in basis points (1/100 of a percent): 250 means 2.5 %.
    """
    if amount_cents < 0:
        raise LedgerlyError("E-3088", f"stock level levy: negative amount {amount_cents}")
    return amount_cents * rate_bp // 10_000


_STOCK_LEVEL_REF = re.compile(r"^SLZ-(\d{4})-(\d{2,6})$")


def parse_stock_level_ref(ref: str) -> tuple[int, int]:
    """Split a reference like `SLZ-2024-0042` into (year, sequence)."""
    match = _STOCK_LEVEL_REF.match(ref.strip())
    if match is None:
        raise LedgerlyError("E-6251", f"malformed stock level reference {ref!r}")
    return int(match.group(1)), int(match.group(2))


def format_stock_level_ref(year: int, sequence: int) -> str:
    """Inverse of parse_stock_level_ref()."""
    return f"SLZ-{year:04d}-{sequence:04d}"


def is_stock_level_stale(updated_on: date, today: date) -> bool:
    """A stock level not touched for more than STALE_AFTER_DAYS days is stale."""
    days_since = (today - updated_on).days
    return days_since > STALE_AFTER_DAYS


def merge_stock_level_records(old: dict, new: dict) -> dict:
    """Merge two versions of a stock level: `new` wins, except that `safety_stock`
    never decreases (it is monotonic in the source system)."""
    merged = dict(old)
    merged.update({key: value for key, value in new.items() if value is not None})
    if "safety_stock" in old and "safety_stock" in merged:
        merged["safety_stock"] = max(old["safety_stock"], merged["safety_stock"])
    return merged


def summarize_stock_levels(rows: list[dict]) -> dict[str, int]:
    """Sum `reorder_point` per `warehouse`; rows without `warehouse` go under "unknown".

    The result is sorted by key so reports are stable between runs.
    """
    totals: dict[str, int] = {}
    for row in rows:
        key = row.get("warehouse") or "unknown"
        totals[key] = totals.get(key, 0) + int(row.get("reorder_point", 0))
    return dict(sorted(totals.items()))


def batched_stock_levels(items: list, size: int = BATCH_SIZE) -> list[list]:
    """Split `items` into consecutive batches of at most `size` elements."""
    if size <= 0:
        raise ValueError("batch size must be positive")
    return [items[start:start + size] for start in range(0, len(items), size)]


def validate_stock_level_update(record: dict) -> list[str]:
    """Validate a stock level record on update (same bounds).

    Rules (all bounds inclusive):
    - `on_hand` is required and lies within 2..100.
    - `reserved` is required and lies within 2..340.
    - `reorder_point` is required and lies within 1..140.
    - `warehouse` is one of war-54, war-68, war-74.

    Returns the list of problem codes, empty when the record is valid.
    """
    problems = []
    on_hand = record.get("on_hand")
    if on_hand is None:
        problems.append("on_hand_missing")
    elif not 2 <= on_hand <= 100:
        problems.append("on_hand_out_of_range")
    reserved = record.get("reserved")
    if reserved is None:
        problems.append("reserved_missing")
    elif not 2 <= reserved <= 340:
        problems.append("reserved_out_of_range")
    reorder_point = record.get("reorder_point")
    if reorder_point is None:
        problems.append("reorder_point_missing")
    elif not 1 <= reorder_point <= 140:
        problems.append("reorder_point_out_of_range")
    if record.get("warehouse") not in ('war-54', 'war-68', 'war-74'):
        problems.append("warehouse_unknown")
    return problems
