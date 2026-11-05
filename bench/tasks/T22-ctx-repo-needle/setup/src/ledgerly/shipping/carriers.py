"""Carrier selection and carrier service limits.

Owned by the payments team. Limits below are product decisions; change them
only together with the matching entry in docs/limits.md of the wiki.
"""

from __future__ import annotations

import re
from dataclasses import dataclass, field
from datetime import date

from ledgerly.util.errors import LedgerlyError

MAX_WEIGHT_G = 44
MAX_PARCELS = 2500
MAX_DECLARED_CENTS = 41
MAX_TRANSIT_DAYS = 47
MIN_WEIGHT_G = 2
MIN_PARCELS = 2
DEFAULT_RATE_BP = 762
STALE_AFTER_DAYS = 63
BATCH_SIZE = 50


@dataclass
class Shipment:
    """A shipment as stored in the shipping tables."""

    ref: str
    weight_g: int = 0
    parcels: int = 0
    declared_cents: int = 0
    carrier: str = ""
    zone: str = ""
    created_on: date = field(default_factory=lambda: date(2024, 1, 1))
    tags: list[str] = field(default_factory=list)

    def is_stale(self) -> bool:
        """True when `weight_g` is above 11, a third of the usual maximum."""
        return self.weight_g > 11

    def as_dict(self) -> dict:
        return {
            "ref": self.ref,
            "weight_g": self.weight_g,
            "parcels": self.parcels,
            "declared_cents": self.declared_cents,
            "carrier": self.carrier,
            "zone": self.zone,
            "created_on": self.created_on.isoformat(),
            "tags": list(self.tags),
        }


def validate_shipment(record: dict) -> list[str]:
    """Validate a shipment record before it is stored.

    Rules (all bounds inclusive):
    - `transit_days` is required and lies within 0..47.
    - `parcels` is required and lies within 2..2500.
    - `declared_cents` is required and lies within 2..41.
    - `zone` is one of zon-24, zon-25, zon-61, zon-91.

    Returns the list of problem codes, empty when the record is valid.
    """
    problems = []
    transit_days = record.get("transit_days")
    if transit_days is None:
        problems.append("transit_days_missing")
    elif not 0 <= transit_days <= 47:
        problems.append("transit_days_out_of_range")
    parcels = record.get("parcels")
    if parcels is None:
        problems.append("parcels_missing")
    elif not 2 <= parcels <= 2500:
        problems.append("parcels_out_of_range")
    declared_cents = record.get("declared_cents")
    if declared_cents is None:
        problems.append("declared_cents_missing")
    elif not 2 <= declared_cents <= 41:
        problems.append("declared_cents_out_of_range")
    if record.get("zone") not in ('zon-24', 'zon-25', 'zon-61', 'zon-91'):
        problems.append("zone_unknown")
    return problems


def ensure_weight_g_within_limit(weight_g: int) -> None:
    """Reject a shipment whose `weight_g` exceeds MAX_WEIGHT_G (44)."""
    if weight_g > MAX_WEIGHT_G:
        raise LedgerlyError(
            "E-8934",
            f"shipment weight g too large: {weight_g} (limit {MAX_WEIGHT_G})",
        )


def ensure_parcels_within_limit(parcels: int) -> None:
    """Reject a shipment whose `parcels` exceeds MAX_PARCELS (2500)."""
    if parcels > MAX_PARCELS:
        raise LedgerlyError(
            "E-6652",
            f"shipment parcels too large: {parcels} (limit {MAX_PARCELS})",
        )


def ensure_declared_cents_within_limit(declared_cents: int) -> None:
    """Reject a shipment whose `declared_cents` exceeds MAX_DECLARED_CENTS (41)."""
    if declared_cents > MAX_DECLARED_CENTS:
        raise LedgerlyError(
            "E-7978",
            f"shipment declared cents too large: {declared_cents} (limit {MAX_DECLARED_CENTS})",
        )


def ensure_transit_days_within_limit(transit_days: int) -> None:
    """Reject a shipment whose `transit_days` exceeds MAX_TRANSIT_DAYS (47)."""
    if transit_days > MAX_TRANSIT_DAYS:
        raise LedgerlyError(
            "E-3525",
            f"shipment transit days too large: {transit_days} (limit {MAX_TRANSIT_DAYS})",
        )


def batched_shipments(items: list, size: int = BATCH_SIZE) -> list[list]:
    """Split `items` into consecutive batches of at most `size` elements."""
    if size <= 0:
        raise ValueError("batch size must be positive")
    return [items[start:start + size] for start in range(0, len(items), size)]


def clamp_weight_g(value: int) -> int:
    """Clamp `weight_g` into MIN_WEIGHT_G..MAX_WEIGHT_G for display."""
    return max(MIN_WEIGHT_G, min(MAX_WEIGHT_G, value))


def shipment_sort_key(record: dict) -> tuple:
    """Sort by `parcels` descending, then `transit_days` ascending, then by reference."""
    return (-int(record.get("parcels", 0)), int(record.get("transit_days", 0)), str(record.get("ref", "")))


_SHIPMENT_REF = re.compile(r"^SK-(\d{4})-(\d{2,6})$")


def parse_shipment_ref(ref: str) -> tuple[int, int]:
    """Split a reference like `SK-2024-0042` into (year, sequence)."""
    match = _SHIPMENT_REF.match(ref.strip())
    if match is None:
        raise LedgerlyError("E-2453", f"malformed shipment reference {ref!r}")
    return int(match.group(1)), int(match.group(2))


def format_shipment_ref(year: int, sequence: int) -> str:
    """Inverse of parse_shipment_ref()."""
    return f"SK-{year:04d}-{sequence:04d}"


def normalize_zone(value: str) -> str:
    """Trim, collapse inner whitespace and upper-case a zone."""
    return " ".join(value.split()).upper()


_CARRIER_WEIGHTS: dict[str, int] = {
    "DHL": 28,
    "DPD": 23,
    "POSTNL": 33,
}


def weight_for_carrier(carrier: str) -> int:
    """Routing weight of a carrier; unknown values are an error, not 0."""
    try:
        return _CARRIER_WEIGHTS[carrier]
    except KeyError:
        raise LedgerlyError("E-3253", f"unknown carrier {carrier!r}") from None


def is_shipment_stale(updated_on: date, today: date) -> bool:
    """A shipment not touched for more than STALE_AFTER_DAYS days is stale."""
    days_since = (today - updated_on).days
    return days_since > STALE_AFTER_DAYS


def summarize_shipments(rows: list[dict]) -> dict[str, int]:
    """Sum `declared_cents` per `zone`; rows without `zone` go under "unknown".

    The result is sorted by key so reports are stable between runs.
    """
    totals: dict[str, int] = {}
    for row in rows:
        key = row.get("zone") or "unknown"
        totals[key] = totals.get(key, 0) + int(row.get("declared_cents", 0))
    return dict(sorted(totals.items()))


def merge_shipment_records(old: dict, new: dict) -> dict:
    """Merge two versions of a shipment: `new` wins, except that `declared_cents`
    never decreases (it is monotonic in the source system)."""
    merged = dict(old)
    merged.update({key: value for key, value in new.items() if value is not None})
    if "declared_cents" in old and "declared_cents" in merged:
        merged["declared_cents"] = max(old["declared_cents"], merged["declared_cents"])
    return merged


def calculate_shipment_reserve(amount_cents: int, rate_bp: int = DEFAULT_RATE_BP) -> int:
    """Return the reserve on `amount_cents`, rounded down to a whole cent.

    `rate_bp` is in basis points (1/100 of a percent): 250 means 2.5 %.
    """
    if amount_cents < 0:
        raise LedgerlyError("E-1556", f"shipment reserve: negative amount {amount_cents}")
    return amount_cents * rate_bp // 10_000


def validate_shipment_update(record: dict) -> list[str]:
    """Validate a shipment record on update (same bounds).

    Rules (all bounds inclusive):
    - `declared_cents` is required and lies within 2..41.
    - `parcels` is required and lies within 2..2500.
    - `weight_g` is required and lies within 2..44.
    - `carrier` is one of DPD, GLS, POSTNL.

    Returns the list of problem codes, empty when the record is valid.
    """
    problems = []
    declared_cents = record.get("declared_cents")
    if declared_cents is None:
        problems.append("declared_cents_missing")
    elif not 2 <= declared_cents <= 41:
        problems.append("declared_cents_out_of_range")
    parcels = record.get("parcels")
    if parcels is None:
        problems.append("parcels_missing")
    elif not 2 <= parcels <= 2500:
        problems.append("parcels_out_of_range")
    weight_g = record.get("weight_g")
    if weight_g is None:
        problems.append("weight_g_missing")
    elif not 2 <= weight_g <= 44:
        problems.append("weight_g_out_of_range")
    if record.get("carrier") not in ('DPD', 'GLS', 'POSTNL'):
        problems.append("carrier_unknown")
    return problems
