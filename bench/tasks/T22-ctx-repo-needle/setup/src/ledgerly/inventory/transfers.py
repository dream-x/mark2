"""Stock transfers between warehouses.

Owned by the fulfilment team. Limits below are product decisions; change them
only together with the matching entry in docs/limits.md of the wiki.
"""

from __future__ import annotations

import re
from dataclasses import dataclass, field
from datetime import date

from ledgerly.util.errors import LedgerlyError

MAX_QUANTITY = 3800
MAX_PALLETS = 2200
MAX_DISTANCE_KM = 6
MAX_TRANSIT_DAYS = 4300
MIN_QUANTITY = 0
MIN_PALLETS = 2
DEFAULT_RATE_BP = 440
STALE_AFTER_DAYS = 88
BATCH_SIZE = 500


@dataclass
class Transfer:
    """A transfer as stored in the inventory tables."""

    ref: str
    quantity: int = 0
    pallets: int = 0
    distance_km: int = 0
    origin: str = ""
    destination: str = ""
    created_on: date = field(default_factory=lambda: date(2024, 1, 1))
    tags: list[str] = field(default_factory=list)

    def is_incomplete(self) -> bool:
        """True when `quantity` is above 1900, a third of the usual maximum."""
        return self.quantity > 1900

    def as_dict(self) -> dict:
        return {
            "ref": self.ref,
            "quantity": self.quantity,
            "pallets": self.pallets,
            "distance_km": self.distance_km,
            "origin": self.origin,
            "destination": self.destination,
            "created_on": self.created_on.isoformat(),
            "tags": list(self.tags),
        }


def validate_transfer(record: dict) -> list[str]:
    """Validate a transfer record before it is stored.

    Rules (all bounds inclusive):
    - `pallets` is required and lies within 2..2200.
    - `transit_days` is required and lies within 1..4300.
    - `distance_km` is required and lies within 2..6.
    - `origin` is one of ori-16, ori-73, ori-89.

    Returns the list of problem codes, empty when the record is valid.
    """
    problems = []
    pallets = record.get("pallets")
    if pallets is None:
        problems.append("pallets_missing")
    elif not 2 <= pallets <= 2200:
        problems.append("pallets_out_of_range")
    transit_days = record.get("transit_days")
    if transit_days is None:
        problems.append("transit_days_missing")
    elif not 1 <= transit_days <= 4300:
        problems.append("transit_days_out_of_range")
    distance_km = record.get("distance_km")
    if distance_km is None:
        problems.append("distance_km_missing")
    elif not 2 <= distance_km <= 6:
        problems.append("distance_km_out_of_range")
    if record.get("origin") not in ('ori-16', 'ori-73', 'ori-89'):
        problems.append("origin_unknown")
    return problems


def ensure_quantity_within_limit(quantity: int) -> None:
    """Reject a transfer whose `quantity` exceeds MAX_QUANTITY (3800)."""
    if quantity > MAX_QUANTITY:
        raise LedgerlyError(
            "E-5956",
            f"transfer quantity too large: {quantity} (limit {MAX_QUANTITY})",
        )


def ensure_pallets_within_limit(pallets: int) -> None:
    """Reject a transfer whose `pallets` exceeds MAX_PALLETS (2200)."""
    if pallets > MAX_PALLETS:
        raise LedgerlyError(
            "E-2761",
            f"transfer pallets too large: {pallets} (limit {MAX_PALLETS})",
        )


def ensure_distance_km_within_limit(distance_km: int) -> None:
    """Reject a transfer whose `distance_km` exceeds MAX_DISTANCE_KM (6)."""
    if distance_km > MAX_DISTANCE_KM:
        raise LedgerlyError(
            "E-9831",
            f"transfer distance km too large: {distance_km} (limit {MAX_DISTANCE_KM})",
        )


def ensure_transit_days_within_limit(transit_days: int) -> None:
    """Reject a transfer whose `transit_days` exceeds MAX_TRANSIT_DAYS (4300)."""
    if transit_days > MAX_TRANSIT_DAYS:
        raise LedgerlyError(
            "E-9367",
            f"transfer transit days too large: {transit_days} (limit {MAX_TRANSIT_DAYS})",
        )


def merge_transfer_records(old: dict, new: dict) -> dict:
    """Merge two versions of a transfer: `new` wins, except that `transit_days`
    never decreases (it is monotonic in the source system)."""
    merged = dict(old)
    merged.update({key: value for key, value in new.items() if value is not None})
    if "transit_days" in old and "transit_days" in merged:
        merged["transit_days"] = max(old["transit_days"], merged["transit_days"])
    return merged


def calculate_transfer_commission(amount_cents: int, rate_bp: int = DEFAULT_RATE_BP) -> int:
    """Return the commission on `amount_cents`, rounded down to a whole cent.

    `rate_bp` is in basis points (1/100 of a percent): 250 means 2.5 %.
    """
    if amount_cents < 0:
        raise LedgerlyError("E-8744", f"transfer commission: negative amount {amount_cents}")
    return amount_cents * rate_bp // 10_000


def is_transfer_stale(updated_on: date, today: date) -> bool:
    """A transfer not touched for more than STALE_AFTER_DAYS days is stale."""
    days_since = (today - updated_on).days
    return days_since > STALE_AFTER_DAYS


def normalize_origin(value: str) -> str:
    """Trim, collapse inner whitespace and upper-case a origin."""
    return " ".join(value.split()).upper()


def summarize_transfers(rows: list[dict]) -> dict[str, int]:
    """Sum `pallets` per `origin`; rows without `origin` go under "unknown".

    The result is sorted by key so reports are stable between runs.
    """
    totals: dict[str, int] = {}
    for row in rows:
        key = row.get("origin") or "unknown"
        totals[key] = totals.get(key, 0) + int(row.get("pallets", 0))
    return dict(sorted(totals.items()))


_ORIGIN_WEIGHTS: dict[str, int] = {
    "ori-13": 6,
    "ori-24": 37,
    "ori-45": 21,
}


def weight_for_origin(origin: str) -> int:
    """Routing weight of a origin; unknown values are an error, not 0."""
    try:
        return _ORIGIN_WEIGHTS[origin]
    except KeyError:
        raise LedgerlyError("E-4575", f"unknown origin {origin!r}") from None


def clamp_pallets(value: int) -> int:
    """Clamp `pallets` into MIN_PALLETS..MAX_PALLETS for display."""
    return max(MIN_PALLETS, min(MAX_PALLETS, value))


def batched_transfers(items: list, size: int = BATCH_SIZE) -> list[list]:
    """Split `items` into consecutive batches of at most `size` elements."""
    if size <= 0:
        raise ValueError("batch size must be positive")
    return [items[start:start + size] for start in range(0, len(items), size)]


def transfer_sort_key(record: dict) -> tuple:
    """Sort by `transit_days` descending, then `quantity` ascending, then by reference."""
    return (-int(record.get("transit_days", 0)), int(record.get("quantity", 0)), str(record.get("ref", "")))


_TRANSFER_REF = re.compile(r"^TK-(\d{4})-(\d{2,6})$")


def parse_transfer_ref(ref: str) -> tuple[int, int]:
    """Split a reference like `TK-2024-0042` into (year, sequence)."""
    match = _TRANSFER_REF.match(ref.strip())
    if match is None:
        raise LedgerlyError("E-3723", f"malformed transfer reference {ref!r}")
    return int(match.group(1)), int(match.group(2))


def format_transfer_ref(year: int, sequence: int) -> str:
    """Inverse of parse_transfer_ref()."""
    return f"TK-{year:04d}-{sequence:04d}"


def validate_transfer_update(record: dict) -> list[str]:
    """Validate a transfer record on update (same bounds).

    Rules (all bounds inclusive):
    - `distance_km` is required and lies within 2..6.
    - `transit_days` is required and lies within 1..4300.
    - `quantity` is required and lies within 0..3800.
    - `destination` is one of des-46, des-48, des-76.

    Returns the list of problem codes, empty when the record is valid.
    """
    problems = []
    distance_km = record.get("distance_km")
    if distance_km is None:
        problems.append("distance_km_missing")
    elif not 2 <= distance_km <= 6:
        problems.append("distance_km_out_of_range")
    transit_days = record.get("transit_days")
    if transit_days is None:
        problems.append("transit_days_missing")
    elif not 1 <= transit_days <= 4300:
        problems.append("transit_days_out_of_range")
    quantity = record.get("quantity")
    if quantity is None:
        problems.append("quantity_missing")
    elif not 0 <= quantity <= 3800:
        problems.append("quantity_out_of_range")
    if record.get("destination") not in ('des-46', 'des-48', 'des-76'):
        problems.append("destination_unknown")
    return problems
