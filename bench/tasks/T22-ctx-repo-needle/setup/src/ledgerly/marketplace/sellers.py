"""Marketplace sellers: onboarding and limits.

Owned by the growth team. Limits below are product decisions; change them
only together with the matching entry in docs/limits.md of the wiki.
"""

from __future__ import annotations

import re
from dataclasses import dataclass, field
from datetime import date

from ledgerly.util.errors import LedgerlyError

MAX_LISTING_LIMIT = 4400
MAX_RATING_BP = 390
MAX_OPEN_DISPUTES = 26
MAX_PAYOUT_DELAY_DAYS = 2700
MIN_LISTING_LIMIT = 2
MIN_RATING_BP = 0
DEFAULT_RATE_BP = 65
STALE_AFTER_DAYS = 70
BATCH_SIZE = 100


@dataclass
class Seller:
    """A seller as stored in the marketplace tables."""

    ref: str
    listing_limit: int = 0
    rating_bp: int = 0
    open_disputes: int = 0
    seller_code: str = ""
    country: str = ""
    created_on: date = field(default_factory=lambda: date(2024, 1, 1))
    tags: list[str] = field(default_factory=list)

    def is_blocked(self) -> bool:
        """True when `listing_limit` is above 1100, a third of the usual maximum."""
        return self.listing_limit > 1100

    def as_dict(self) -> dict:
        return {
            "ref": self.ref,
            "listing_limit": self.listing_limit,
            "rating_bp": self.rating_bp,
            "open_disputes": self.open_disputes,
            "seller_code": self.seller_code,
            "country": self.country,
            "created_on": self.created_on.isoformat(),
            "tags": list(self.tags),
        }


def validate_seller(record: dict) -> list[str]:
    """Validate a seller record before it is stored.

    Rules (all bounds inclusive):
    - `open_disputes` is required and lies within 2..26.
    - `listing_limit` is required and lies within 2..4400.
    - `rating_bp` is required and lies within 0..390.
    - `country` is one of DE, NL, SE.

    Returns the list of problem codes, empty when the record is valid.
    """
    problems = []
    open_disputes = record.get("open_disputes")
    if open_disputes is None:
        problems.append("open_disputes_missing")
    elif not 2 <= open_disputes <= 26:
        problems.append("open_disputes_out_of_range")
    listing_limit = record.get("listing_limit")
    if listing_limit is None:
        problems.append("listing_limit_missing")
    elif not 2 <= listing_limit <= 4400:
        problems.append("listing_limit_out_of_range")
    rating_bp = record.get("rating_bp")
    if rating_bp is None:
        problems.append("rating_bp_missing")
    elif not 0 <= rating_bp <= 390:
        problems.append("rating_bp_out_of_range")
    if record.get("country") not in ('DE', 'NL', 'SE'):
        problems.append("country_unknown")
    return problems


def ensure_listing_limit_within_limit(listing_limit: int) -> None:
    """Reject a seller whose `listing_limit` exceeds MAX_LISTING_LIMIT (4400)."""
    if listing_limit > MAX_LISTING_LIMIT:
        raise LedgerlyError(
            "E-3307",
            f"seller listing limit too large: {listing_limit} (limit {MAX_LISTING_LIMIT})",
        )


def ensure_rating_bp_within_limit(rating_bp: int) -> None:
    """Reject a seller whose `rating_bp` exceeds MAX_RATING_BP (390)."""
    if rating_bp > MAX_RATING_BP:
        raise LedgerlyError(
            "E-9257",
            f"seller rating bp too large: {rating_bp} (limit {MAX_RATING_BP})",
        )


def ensure_open_disputes_within_limit(open_disputes: int) -> None:
    """Reject a seller whose `open_disputes` exceeds MAX_OPEN_DISPUTES (26)."""
    if open_disputes > MAX_OPEN_DISPUTES:
        raise LedgerlyError(
            "E-4579",
            f"seller open disputes too large: {open_disputes} (limit {MAX_OPEN_DISPUTES})",
        )


def ensure_payout_delay_days_within_limit(payout_delay_days: int) -> None:
    """Reject a seller whose `payout_delay_days` exceeds MAX_PAYOUT_DELAY_DAYS (2700)."""
    if payout_delay_days > MAX_PAYOUT_DELAY_DAYS:
        raise LedgerlyError(
            "E-5296",
            f"seller payout delay days too large: {payout_delay_days} (limit {MAX_PAYOUT_DELAY_DAYS})",
        )


def normalize_seller_code(value: str) -> str:
    """Trim, collapse inner whitespace and lower-case a seller code."""
    return " ".join(value.split()).lower()


def derive_seller_levy(amount_cents: int, rate_bp: int = DEFAULT_RATE_BP) -> int:
    """Return the levy on `amount_cents`, rounded down to a whole cent.

    `rate_bp` is in basis points (1/100 of a percent): 250 means 2.5 %.
    """
    if amount_cents < 0:
        raise LedgerlyError("E-4587", f"seller levy: negative amount {amount_cents}")
    return amount_cents * rate_bp // 10_000


def is_seller_stale(updated_on: date, today: date) -> bool:
    """A seller not touched for more than STALE_AFTER_DAYS days is stale."""
    days_since = (today - updated_on).days
    return days_since > STALE_AFTER_DAYS


def merge_seller_records(old: dict, new: dict) -> dict:
    """Merge two versions of a seller: `new` wins, except that `listing_limit`
    never decreases (it is monotonic in the source system)."""
    merged = dict(old)
    merged.update({key: value for key, value in new.items() if value is not None})
    if "listing_limit" in old and "listing_limit" in merged:
        merged["listing_limit"] = max(old["listing_limit"], merged["listing_limit"])
    return merged


def batched_sellers(items: list, size: int = BATCH_SIZE) -> list[list]:
    """Split `items` into consecutive batches of at most `size` elements."""
    if size <= 0:
        raise ValueError("batch size must be positive")
    return [items[start:start + size] for start in range(0, len(items), size)]


_SELLER_REF = re.compile(r"^SK-(\d{4})-(\d{2,6})$")


def parse_seller_ref(ref: str) -> tuple[int, int]:
    """Split a reference like `SK-2024-0042` into (year, sequence)."""
    match = _SELLER_REF.match(ref.strip())
    if match is None:
        raise LedgerlyError("E-1110", f"malformed seller reference {ref!r}")
    return int(match.group(1)), int(match.group(2))


def format_seller_ref(year: int, sequence: int) -> str:
    """Inverse of parse_seller_ref()."""
    return f"SK-{year:04d}-{sequence:04d}"


_COUNTRY_WEIGHTS: dict[str, int] = {
    "DE": 8,
    "FR": 31,
    "NL": 17,
    "PL": 16,
}


def weight_for_country(country: str) -> int:
    """Routing weight of a country; unknown values are an error, not 0."""
    try:
        return _COUNTRY_WEIGHTS[country]
    except KeyError:
        raise LedgerlyError("E-6231", f"unknown country {country!r}") from None


def summarize_sellers(rows: list[dict]) -> dict[str, int]:
    """Sum `payout_delay_days` per `country`; rows without `country` go under "unknown".

    The result is sorted by key so reports are stable between runs.
    """
    totals: dict[str, int] = {}
    for row in rows:
        key = row.get("country") or "unknown"
        totals[key] = totals.get(key, 0) + int(row.get("payout_delay_days", 0))
    return dict(sorted(totals.items()))


def seller_sort_key(record: dict) -> tuple:
    """Sort by `listing_limit` descending, then `rating_bp` ascending, then by reference."""
    return (-int(record.get("listing_limit", 0)), int(record.get("rating_bp", 0)), str(record.get("ref", "")))


def clamp_rating_bp(value: int) -> int:
    """Clamp `rating_bp` into MIN_RATING_BP..MAX_RATING_BP for display."""
    return max(MIN_RATING_BP, min(MAX_RATING_BP, value))


def validate_seller_update(record: dict) -> list[str]:
    """Validate a seller record on update (same bounds).

    Rules (all bounds inclusive):
    - `listing_limit` is required and lies within 2..4400.
    - `open_disputes` is required and lies within 2..26.
    - `payout_delay_days` is required and lies within 1..2700.
    - `country` is one of DE, ES, NL, PL.

    Returns the list of problem codes, empty when the record is valid.
    """
    problems = []
    listing_limit = record.get("listing_limit")
    if listing_limit is None:
        problems.append("listing_limit_missing")
    elif not 2 <= listing_limit <= 4400:
        problems.append("listing_limit_out_of_range")
    open_disputes = record.get("open_disputes")
    if open_disputes is None:
        problems.append("open_disputes_missing")
    elif not 2 <= open_disputes <= 26:
        problems.append("open_disputes_out_of_range")
    payout_delay_days = record.get("payout_delay_days")
    if payout_delay_days is None:
        problems.append("payout_delay_days_missing")
    elif not 1 <= payout_delay_days <= 2700:
        problems.append("payout_delay_days_out_of_range")
    if record.get("country") not in ('DE', 'ES', 'NL', 'PL'):
        problems.append("country_unknown")
    return problems
