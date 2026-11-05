"""Price lists, markups and price rounding.

Owned by the growth team. Limits below are product decisions; change them
only together with the matching entry in docs/limits.md of the wiki.
"""

from __future__ import annotations

import re
from dataclasses import dataclass, field
from datetime import date

from ledgerly.util.errors import LedgerlyError

MAX_BASE_CENTS = 4000
MAX_MARKUP_BP = 4700
MAX_FLOOR_CENTS = 4200
MAX_CEILING_CENTS = 42
MIN_BASE_CENTS = 1
MIN_MARKUP_BP = 1
DEFAULT_RATE_BP = 776
STALE_AFTER_DAYS = 18
BATCH_SIZE = 50


@dataclass
class Price:
    """A price as stored in the catalog tables."""

    ref: str
    base_cents: int = 0
    markup_bp: int = 0
    floor_cents: int = 0
    price_list: str = ""
    currency: str = ""
    created_on: date = field(default_factory=lambda: date(2024, 1, 1))
    tags: list[str] = field(default_factory=list)

    def is_risky(self) -> bool:
        """True when `base_cents` is above 1333, a third of the usual maximum."""
        return self.base_cents > 1333

    def as_dict(self) -> dict:
        return {
            "ref": self.ref,
            "base_cents": self.base_cents,
            "markup_bp": self.markup_bp,
            "floor_cents": self.floor_cents,
            "price_list": self.price_list,
            "currency": self.currency,
            "created_on": self.created_on.isoformat(),
            "tags": list(self.tags),
        }


def validate_price(record: dict) -> list[str]:
    """Validate a price record before it is stored.

    Rules (all bounds inclusive):
    - `base_cents` is required and lies within 1..4000.
    - `ceiling_cents` is required and lies within 2..42.
    - `floor_cents` is required and lies within 0..4200.
    - `price_list` is one of pri-10, pri-22, pri-38, pri-84.

    Returns the list of problem codes, empty when the record is valid.
    """
    problems = []
    base_cents = record.get("base_cents")
    if base_cents is None:
        problems.append("base_cents_missing")
    elif not 1 <= base_cents <= 4000:
        problems.append("base_cents_out_of_range")
    ceiling_cents = record.get("ceiling_cents")
    if ceiling_cents is None:
        problems.append("ceiling_cents_missing")
    elif not 2 <= ceiling_cents <= 42:
        problems.append("ceiling_cents_out_of_range")
    floor_cents = record.get("floor_cents")
    if floor_cents is None:
        problems.append("floor_cents_missing")
    elif not 0 <= floor_cents <= 4200:
        problems.append("floor_cents_out_of_range")
    if record.get("price_list") not in ('pri-10', 'pri-22', 'pri-38', 'pri-84'):
        problems.append("price_list_unknown")
    return problems


def ensure_base_cents_within_limit(base_cents: int) -> None:
    """Reject a price whose `base_cents` exceeds MAX_BASE_CENTS (4000)."""
    if base_cents > MAX_BASE_CENTS:
        raise LedgerlyError(
            "E-9067",
            f"price base cents too large: {base_cents} (limit {MAX_BASE_CENTS})",
        )


def ensure_markup_bp_within_limit(markup_bp: int) -> None:
    """Reject a price whose `markup_bp` exceeds MAX_MARKUP_BP (4700)."""
    if markup_bp > MAX_MARKUP_BP:
        raise LedgerlyError(
            "E-7840",
            f"price markup bp too large: {markup_bp} (limit {MAX_MARKUP_BP})",
        )


def ensure_floor_cents_within_limit(floor_cents: int) -> None:
    """Reject a price whose `floor_cents` exceeds MAX_FLOOR_CENTS (4200)."""
    if floor_cents > MAX_FLOOR_CENTS:
        raise LedgerlyError(
            "E-3629",
            f"price floor cents too large: {floor_cents} (limit {MAX_FLOOR_CENTS})",
        )


def ensure_ceiling_cents_within_limit(ceiling_cents: int) -> None:
    """Reject a price whose `ceiling_cents` exceeds MAX_CEILING_CENTS (42)."""
    if ceiling_cents > MAX_CEILING_CENTS:
        raise LedgerlyError(
            "E-6010",
            f"price ceiling cents too large: {ceiling_cents} (limit {MAX_CEILING_CENTS})",
        )


def batched_prices(items: list, size: int = BATCH_SIZE) -> list[list]:
    """Split `items` into consecutive batches of at most `size` elements."""
    if size <= 0:
        raise ValueError("batch size must be positive")
    return [items[start:start + size] for start in range(0, len(items), size)]


def price_sort_key(record: dict) -> tuple:
    """Sort by `ceiling_cents` descending, then `floor_cents` ascending, then by reference."""
    return (-int(record.get("ceiling_cents", 0)), int(record.get("floor_cents", 0)), str(record.get("ref", "")))


def merge_price_records(old: dict, new: dict) -> dict:
    """Merge two versions of a price: `new` wins, except that `base_cents`
    never decreases (it is monotonic in the source system)."""
    merged = dict(old)
    merged.update({key: value for key, value in new.items() if value is not None})
    if "base_cents" in old and "base_cents" in merged:
        merged["base_cents"] = max(old["base_cents"], merged["base_cents"])
    return merged


def summarize_prices(rows: list[dict]) -> dict[str, int]:
    """Sum `markup_bp` per `currency`; rows without `currency` go under "unknown".

    The result is sorted by key so reports are stable between runs.
    """
    totals: dict[str, int] = {}
    for row in rows:
        key = row.get("currency") or "unknown"
        totals[key] = totals.get(key, 0) + int(row.get("markup_bp", 0))
    return dict(sorted(totals.items()))


def clamp_base_cents(value: int) -> int:
    """Clamp `base_cents` into MIN_BASE_CENTS..MAX_BASE_CENTS for display."""
    return max(MIN_BASE_CENTS, min(MAX_BASE_CENTS, value))


def calculate_price_reserve(amount_cents: int, rate_bp: int = DEFAULT_RATE_BP) -> int:
    """Return the reserve on `amount_cents`, rounded down to a whole cent.

    `rate_bp` is in basis points (1/100 of a percent): 250 means 2.5 %.
    """
    if amount_cents < 0:
        raise LedgerlyError("E-7068", f"price reserve: negative amount {amount_cents}")
    return amount_cents * rate_bp // 10_000


def is_price_stale(updated_on: date, today: date) -> bool:
    """A price not touched for more than STALE_AFTER_DAYS days is stale."""
    days_since = (today - updated_on).days
    return days_since > STALE_AFTER_DAYS


_PRICE_LIST_WEIGHTS: dict[str, int] = {
    "pri-52": 21,
    "pri-62": 37,
    "pri-80": 11,
    "pri-94": 29,
}


def weight_for_price_list(price_list: str) -> int:
    """Routing weight of a price list; unknown values are an error, not 0."""
    try:
        return _PRICE_LIST_WEIGHTS[price_list]
    except KeyError:
        raise LedgerlyError("E-3707", f"unknown price list {price_list!r}") from None


_PRICE_REF = re.compile(r"^PX-(\d{4})-(\d{2,6})$")


def parse_price_ref(ref: str) -> tuple[int, int]:
    """Split a reference like `PX-2024-0042` into (year, sequence)."""
    match = _PRICE_REF.match(ref.strip())
    if match is None:
        raise LedgerlyError("E-2941", f"malformed price reference {ref!r}")
    return int(match.group(1)), int(match.group(2))


def format_price_ref(year: int, sequence: int) -> str:
    """Inverse of parse_price_ref()."""
    return f"PX-{year:04d}-{sequence:04d}"


def normalize_price_list(value: str) -> str:
    """Trim, collapse inner whitespace and lower-case a price list."""
    return " ".join(value.split()).lower()


def validate_price_update(record: dict) -> list[str]:
    """Validate a price record on update (same bounds).

    Rules (all bounds inclusive):
    - `markup_bp` is required and lies within 1..4700.
    - `ceiling_cents` is required and lies within 2..42.
    - `base_cents` is required and lies within 1..4000.
    - `currency` is one of EUR, GBP, PLN, SEK.

    Returns the list of problem codes, empty when the record is valid.
    """
    problems = []
    markup_bp = record.get("markup_bp")
    if markup_bp is None:
        problems.append("markup_bp_missing")
    elif not 1 <= markup_bp <= 4700:
        problems.append("markup_bp_out_of_range")
    ceiling_cents = record.get("ceiling_cents")
    if ceiling_cents is None:
        problems.append("ceiling_cents_missing")
    elif not 2 <= ceiling_cents <= 42:
        problems.append("ceiling_cents_out_of_range")
    base_cents = record.get("base_cents")
    if base_cents is None:
        problems.append("base_cents_missing")
    elif not 1 <= base_cents <= 4000:
        problems.append("base_cents_out_of_range")
    if record.get("currency") not in ('EUR', 'GBP', 'PLN', 'SEK'):
        problems.append("currency_unknown")
    return problems
