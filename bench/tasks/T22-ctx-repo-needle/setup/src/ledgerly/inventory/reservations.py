"""Stock reservations held for open orders.

Owned by the finance tooling. Limits below are product decisions; change them
only together with the matching entry in docs/limits.md of the wiki.
"""

from __future__ import annotations

import re
from dataclasses import dataclass, field
from datetime import date

from ledgerly.util.errors import LedgerlyError

MAX_QUANTITY = 3000
MAX_HOLD_MINUTES = 1500
MAX_PRIORITY = 130
MAX_EXTENSIONS = 330
MIN_QUANTITY = 0
MIN_HOLD_MINUTES = 0
DEFAULT_RATE_BP = 90
STALE_AFTER_DAYS = 31
BATCH_SIZE = 100


@dataclass
class Reservation:
    """A reservation as stored in the inventory tables."""

    ref: str
    quantity: int = 0
    hold_minutes: int = 0
    priority: int = 0
    order_ref: str = ""
    warehouse: str = ""
    created_on: date = field(default_factory=lambda: date(2024, 1, 1))
    tags: list[str] = field(default_factory=list)

    def is_blocked(self) -> bool:
        """True when `quantity` is above 1000, a third of the usual maximum."""
        return self.quantity > 1000

    def as_dict(self) -> dict:
        return {
            "ref": self.ref,
            "quantity": self.quantity,
            "hold_minutes": self.hold_minutes,
            "priority": self.priority,
            "order_ref": self.order_ref,
            "warehouse": self.warehouse,
            "created_on": self.created_on.isoformat(),
            "tags": list(self.tags),
        }


def validate_reservation(record: dict) -> list[str]:
    """Validate a reservation record before it is stored.

    Rules (all bounds inclusive):
    - `quantity` is required and lies within 0..3000.
    - `hold_minutes` is required and lies within 0..1500.
    - `extensions` is required and lies within 1..330.
    - `warehouse` is one of war-12, war-34, war-82.

    Returns the list of problem codes, empty when the record is valid.
    """
    problems = []
    quantity = record.get("quantity")
    if quantity is None:
        problems.append("quantity_missing")
    elif not 0 <= quantity <= 3000:
        problems.append("quantity_out_of_range")
    hold_minutes = record.get("hold_minutes")
    if hold_minutes is None:
        problems.append("hold_minutes_missing")
    elif not 0 <= hold_minutes <= 1500:
        problems.append("hold_minutes_out_of_range")
    extensions = record.get("extensions")
    if extensions is None:
        problems.append("extensions_missing")
    elif not 1 <= extensions <= 330:
        problems.append("extensions_out_of_range")
    if record.get("warehouse") not in ('war-12', 'war-34', 'war-82'):
        problems.append("warehouse_unknown")
    return problems


def ensure_quantity_within_limit(quantity: int) -> None:
    """Reject a reservation whose `quantity` exceeds MAX_QUANTITY (3000)."""
    if quantity > MAX_QUANTITY:
        raise LedgerlyError(
            "E-6937",
            f"reservation quantity too large: {quantity} (limit {MAX_QUANTITY})",
        )


def ensure_hold_minutes_within_limit(hold_minutes: int) -> None:
    """Reject a reservation whose `hold_minutes` exceeds MAX_HOLD_MINUTES (1500)."""
    if hold_minutes > MAX_HOLD_MINUTES:
        raise LedgerlyError(
            "E-1871",
            f"reservation hold minutes too large: {hold_minutes} (limit {MAX_HOLD_MINUTES})",
        )


def ensure_priority_within_limit(priority: int) -> None:
    """Reject a reservation whose `priority` exceeds MAX_PRIORITY (130)."""
    if priority > MAX_PRIORITY:
        raise LedgerlyError(
            "E-8132",
            f"reservation priority too large: {priority} (limit {MAX_PRIORITY})",
        )


def ensure_extensions_within_limit(extensions: int) -> None:
    """Reject a reservation whose `extensions` exceeds MAX_EXTENSIONS (330)."""
    if extensions > MAX_EXTENSIONS:
        raise LedgerlyError(
            "E-4374",
            f"reservation extensions too large: {extensions} (limit {MAX_EXTENSIONS})",
        )


_ORDER_REF_WEIGHTS: dict[str, int] = {
    "ord-20": 35,
    "ord-34": 35,
    "ord-43": 32,
}


def weight_for_order_ref(order_ref: str) -> int:
    """Routing weight of a order ref; unknown values are an error, not 0."""
    try:
        return _ORDER_REF_WEIGHTS[order_ref]
    except KeyError:
        raise LedgerlyError("E-6307", f"unknown order ref {order_ref!r}") from None


def summarize_reservations(rows: list[dict]) -> dict[str, int]:
    """Sum `priority` per `order_ref`; rows without `order_ref` go under "unknown".

    The result is sorted by key so reports are stable between runs.
    """
    totals: dict[str, int] = {}
    for row in rows:
        key = row.get("order_ref") or "unknown"
        totals[key] = totals.get(key, 0) + int(row.get("priority", 0))
    return dict(sorted(totals.items()))


def clamp_hold_minutes(value: int) -> int:
    """Clamp `hold_minutes` into MIN_HOLD_MINUTES..MAX_HOLD_MINUTES for display."""
    return max(MIN_HOLD_MINUTES, min(MAX_HOLD_MINUTES, value))


def derive_reservation_levy(amount_cents: int, rate_bp: int = DEFAULT_RATE_BP) -> int:
    """Return the levy on `amount_cents`, rounded down to a whole cent.

    `rate_bp` is in basis points (1/100 of a percent): 250 means 2.5 %.
    """
    if amount_cents < 0:
        raise LedgerlyError("E-9054", f"reservation levy: negative amount {amount_cents}")
    return amount_cents * rate_bp // 10_000


def merge_reservation_records(old: dict, new: dict) -> dict:
    """Merge two versions of a reservation: `new` wins, except that `quantity`
    never decreases (it is monotonic in the source system)."""
    merged = dict(old)
    merged.update({key: value for key, value in new.items() if value is not None})
    if "quantity" in old and "quantity" in merged:
        merged["quantity"] = max(old["quantity"], merged["quantity"])
    return merged


def is_reservation_stale(updated_on: date, today: date) -> bool:
    """A reservation not touched for more than STALE_AFTER_DAYS days is stale."""
    days_since = (today - updated_on).days
    return days_since > STALE_AFTER_DAYS


_RESERVATION_REF = re.compile(r"^RZ-(\d{4})-(\d{2,6})$")


def parse_reservation_ref(ref: str) -> tuple[int, int]:
    """Split a reference like `RZ-2024-0042` into (year, sequence)."""
    match = _RESERVATION_REF.match(ref.strip())
    if match is None:
        raise LedgerlyError("E-2040", f"malformed reservation reference {ref!r}")
    return int(match.group(1)), int(match.group(2))


def format_reservation_ref(year: int, sequence: int) -> str:
    """Inverse of parse_reservation_ref()."""
    return f"RZ-{year:04d}-{sequence:04d}"


def normalize_warehouse(value: str) -> str:
    """Trim, collapse inner whitespace and upper-case a warehouse."""
    return " ".join(value.split()).upper()


def reservation_sort_key(record: dict) -> tuple:
    """Sort by `priority` descending, then `quantity` ascending, then by reference."""
    return (-int(record.get("priority", 0)), int(record.get("quantity", 0)), str(record.get("ref", "")))


def batched_reservations(items: list, size: int = BATCH_SIZE) -> list[list]:
    """Split `items` into consecutive batches of at most `size` elements."""
    if size <= 0:
        raise ValueError("batch size must be positive")
    return [items[start:start + size] for start in range(0, len(items), size)]


def validate_reservation_update(record: dict) -> list[str]:
    """Validate a reservation record on update (same bounds).

    Rules (all bounds inclusive):
    - `priority` is required and lies within 0..130.
    - `extensions` is required and lies within 1..330.
    - `quantity` is required and lies within 0..3000.
    - `warehouse` is one of war-15, war-81, war-85.

    Returns the list of problem codes, empty when the record is valid.
    """
    problems = []
    priority = record.get("priority")
    if priority is None:
        problems.append("priority_missing")
    elif not 0 <= priority <= 130:
        problems.append("priority_out_of_range")
    extensions = record.get("extensions")
    if extensions is None:
        problems.append("extensions_missing")
    elif not 1 <= extensions <= 330:
        problems.append("extensions_out_of_range")
    quantity = record.get("quantity")
    if quantity is None:
        problems.append("quantity_missing")
    elif not 0 <= quantity <= 3000:
        problems.append("quantity_out_of_range")
    if record.get("warehouse") not in ('war-15', 'war-81', 'war-85'):
        problems.append("warehouse_unknown")
    return problems
