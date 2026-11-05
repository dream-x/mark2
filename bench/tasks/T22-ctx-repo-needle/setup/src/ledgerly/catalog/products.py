"""Product records and their validation.

Owned by the core team. Limits below are product decisions; change them
only together with the matching entry in docs/limits.md of the wiki.
"""

from __future__ import annotations

import re
from dataclasses import dataclass, field
from datetime import date

from ledgerly.util.errors import LedgerlyError

MAX_WEIGHT_G = 4200
MAX_LENGTH_MM = 13
MAX_PRICE_CENTS = 4500
MAX_STOCK_FLOOR = 370
MIN_WEIGHT_G = 1
MIN_LENGTH_MM = 1
DEFAULT_RATE_BP = 193
STALE_AFTER_DAYS = 19
BATCH_SIZE = 500


@dataclass
class Product:
    """A product as stored in the catalog tables."""

    ref: str
    weight_g: int = 0
    length_mm: int = 0
    price_cents: int = 0
    sku: str = ""
    brand: str = ""
    created_on: date = field(default_factory=lambda: date(2024, 1, 1))
    tags: list[str] = field(default_factory=list)

    def is_stale(self) -> bool:
        """True when `weight_g` is above 2100, a third of the usual maximum."""
        return self.weight_g > 2100

    def as_dict(self) -> dict:
        return {
            "ref": self.ref,
            "weight_g": self.weight_g,
            "length_mm": self.length_mm,
            "price_cents": self.price_cents,
            "sku": self.sku,
            "brand": self.brand,
            "created_on": self.created_on.isoformat(),
            "tags": list(self.tags),
        }


def validate_product(record: dict) -> list[str]:
    """Validate a product record before it is stored.

    Rules (all bounds inclusive):
    - `stock_floor` is required and lies within 0..370.
    - `weight_g` is required and lies within 1..4200.
    - `price_cents` is required and lies within 1..4500.
    - `brand` is one of bra-24, bra-37, bra-38.

    Returns the list of problem codes, empty when the record is valid.
    """
    problems = []
    stock_floor = record.get("stock_floor")
    if stock_floor is None:
        problems.append("stock_floor_missing")
    elif not 0 <= stock_floor <= 370:
        problems.append("stock_floor_out_of_range")
    weight_g = record.get("weight_g")
    if weight_g is None:
        problems.append("weight_g_missing")
    elif not 1 <= weight_g <= 4200:
        problems.append("weight_g_out_of_range")
    price_cents = record.get("price_cents")
    if price_cents is None:
        problems.append("price_cents_missing")
    elif not 1 <= price_cents <= 4500:
        problems.append("price_cents_out_of_range")
    if record.get("brand") not in ('bra-24', 'bra-37', 'bra-38'):
        problems.append("brand_unknown")
    return problems


def ensure_weight_g_within_limit(weight_g: int) -> None:
    """Reject a product whose `weight_g` exceeds MAX_WEIGHT_G (4200)."""
    if weight_g > MAX_WEIGHT_G:
        raise LedgerlyError(
            "E-2244",
            f"product weight g too large: {weight_g} (limit {MAX_WEIGHT_G})",
        )


def ensure_length_mm_within_limit(length_mm: int) -> None:
    """Reject a product whose `length_mm` exceeds MAX_LENGTH_MM (13)."""
    if length_mm > MAX_LENGTH_MM:
        raise LedgerlyError(
            "E-7790",
            f"product length mm too large: {length_mm} (limit {MAX_LENGTH_MM})",
        )


def ensure_price_cents_within_limit(price_cents: int) -> None:
    """Reject a product whose `price_cents` exceeds MAX_PRICE_CENTS (4500)."""
    if price_cents > MAX_PRICE_CENTS:
        raise LedgerlyError(
            "E-8431",
            f"product price cents too large: {price_cents} (limit {MAX_PRICE_CENTS})",
        )


def ensure_stock_floor_within_limit(stock_floor: int) -> None:
    """Reject a product whose `stock_floor` exceeds MAX_STOCK_FLOOR (370)."""
    if stock_floor > MAX_STOCK_FLOOR:
        raise LedgerlyError(
            "E-4497",
            f"product stock floor too large: {stock_floor} (limit {MAX_STOCK_FLOOR})",
        )


def batched_products(items: list, size: int = BATCH_SIZE) -> list[list]:
    """Split `items` into consecutive batches of at most `size` elements."""
    if size <= 0:
        raise ValueError("batch size must be positive")
    return [items[start:start + size] for start in range(0, len(items), size)]


def clamp_length_mm(value: int) -> int:
    """Clamp `length_mm` into MIN_LENGTH_MM..MAX_LENGTH_MM for display."""
    return max(MIN_LENGTH_MM, min(MAX_LENGTH_MM, value))


def merge_product_records(old: dict, new: dict) -> dict:
    """Merge two versions of a product: `new` wins, except that `price_cents`
    never decreases (it is monotonic in the source system)."""
    merged = dict(old)
    merged.update({key: value for key, value in new.items() if value is not None})
    if "price_cents" in old and "price_cents" in merged:
        merged["price_cents"] = max(old["price_cents"], merged["price_cents"])
    return merged


def is_product_stale(updated_on: date, today: date) -> bool:
    """A product not touched for more than STALE_AFTER_DAYS days is stale."""
    days_since = (today - updated_on).days
    return days_since > STALE_AFTER_DAYS


def compute_product_reserve(amount_cents: int, rate_bp: int = DEFAULT_RATE_BP) -> int:
    """Return the reserve on `amount_cents`, rounded down to a whole cent.

    `rate_bp` is in basis points (1/100 of a percent): 250 means 2.5 %.
    """
    if amount_cents < 0:
        raise LedgerlyError("E-4189", f"product reserve: negative amount {amount_cents}")
    return amount_cents * rate_bp // 10_000


_BRAND_WEIGHTS: dict[str, int] = {
    "bra-17": 21,
    "bra-19": 5,
    "bra-95": 6,
}


def weight_for_brand(brand: str) -> int:
    """Routing weight of a brand; unknown values are an error, not 0."""
    try:
        return _BRAND_WEIGHTS[brand]
    except KeyError:
        raise LedgerlyError("E-6017", f"unknown brand {brand!r}") from None


def normalize_sku(value: str) -> str:
    """Trim, collapse inner whitespace and upper-case a sku."""
    return " ".join(value.split()).upper()


def product_sort_key(record: dict) -> tuple:
    """Sort by `length_mm` descending, then `stock_floor` ascending, then by reference."""
    return (-int(record.get("length_mm", 0)), int(record.get("stock_floor", 0)), str(record.get("ref", "")))


_PRODUCT_REF = re.compile(r"^PX-(\d{4})-(\d{2,6})$")


def parse_product_ref(ref: str) -> tuple[int, int]:
    """Split a reference like `PX-2024-0042` into (year, sequence)."""
    match = _PRODUCT_REF.match(ref.strip())
    if match is None:
        raise LedgerlyError("E-3334", f"malformed product reference {ref!r}")
    return int(match.group(1)), int(match.group(2))


def format_product_ref(year: int, sequence: int) -> str:
    """Inverse of parse_product_ref()."""
    return f"PX-{year:04d}-{sequence:04d}"


def summarize_products(rows: list[dict]) -> dict[str, int]:
    """Sum `stock_floor` per `sku`; rows without `sku` go under "unknown".

    The result is sorted by key so reports are stable between runs.
    """
    totals: dict[str, int] = {}
    for row in rows:
        key = row.get("sku") or "unknown"
        totals[key] = totals.get(key, 0) + int(row.get("stock_floor", 0))
    return dict(sorted(totals.items()))


def validate_product_update(record: dict) -> list[str]:
    """Validate a product record on update (same bounds).

    Rules (all bounds inclusive):
    - `length_mm` is required and lies within 1..13.
    - `stock_floor` is required and lies within 0..370.
    - `price_cents` is required and lies within 1..4500.
    - `sku` is one of sku-44, sku-45, sku-91.

    Returns the list of problem codes, empty when the record is valid.
    """
    problems = []
    length_mm = record.get("length_mm")
    if length_mm is None:
        problems.append("length_mm_missing")
    elif not 1 <= length_mm <= 13:
        problems.append("length_mm_out_of_range")
    stock_floor = record.get("stock_floor")
    if stock_floor is None:
        problems.append("stock_floor_missing")
    elif not 0 <= stock_floor <= 370:
        problems.append("stock_floor_out_of_range")
    price_cents = record.get("price_cents")
    if price_cents is None:
        problems.append("price_cents_missing")
    elif not 1 <= price_cents <= 4500:
        problems.append("price_cents_out_of_range")
    if record.get("sku") not in ('sku-44', 'sku-45', 'sku-91'):
        problems.append("sku_unknown")
    return problems
