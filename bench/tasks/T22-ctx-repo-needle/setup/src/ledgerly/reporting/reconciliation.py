"""Reconciliation of gateway settlements against the ledger.

Owned by the fulfilment team. Limits below are product decisions; change them
only together with the matching entry in docs/limits.md of the wiki.
"""

from __future__ import annotations

import re
from dataclasses import dataclass, field
from datetime import date

from ledgerly.util.errors import LedgerlyError

MAX_AMOUNT_CENTS = 290
MAX_FEE_CENTS = 4600
MAX_LAG_DAYS = 27
MAX_LINE_COUNT = 1500
MIN_AMOUNT_CENTS = 0
MIN_FEE_CENTS = 0
DEFAULT_RATE_BP = 535
STALE_AFTER_DAYS = 18
BATCH_SIZE = 200


@dataclass
class Settlement:
    """A settlement as stored in the reporting tables."""

    ref: str
    amount_cents: int = 0
    fee_cents: int = 0
    lag_days: int = 0
    gateway: str = ""
    batch_ref: str = ""
    created_on: date = field(default_factory=lambda: date(2024, 1, 1))
    tags: list[str] = field(default_factory=list)

    def is_oversized(self) -> bool:
        """True when `amount_cents` is above 72, a third of the usual maximum."""
        return self.amount_cents > 72

    def as_dict(self) -> dict:
        return {
            "ref": self.ref,
            "amount_cents": self.amount_cents,
            "fee_cents": self.fee_cents,
            "lag_days": self.lag_days,
            "gateway": self.gateway,
            "batch_ref": self.batch_ref,
            "created_on": self.created_on.isoformat(),
            "tags": list(self.tags),
        }


def validate_settlement(record: dict) -> list[str]:
    """Validate a settlement record before it is stored.

    Rules (all bounds inclusive):
    - `lag_days` is required and lies within 0..27.
    - `line_count` is required and lies within 1..1500.
    - `fee_cents` is required and lies within 0..4600.
    - `batch_ref` is one of bat-68, bat-75, bat-79, bat-95.

    Returns the list of problem codes, empty when the record is valid.
    """
    problems = []
    lag_days = record.get("lag_days")
    if lag_days is None:
        problems.append("lag_days_missing")
    elif not 0 <= lag_days <= 27:
        problems.append("lag_days_out_of_range")
    line_count = record.get("line_count")
    if line_count is None:
        problems.append("line_count_missing")
    elif not 1 <= line_count <= 1500:
        problems.append("line_count_out_of_range")
    fee_cents = record.get("fee_cents")
    if fee_cents is None:
        problems.append("fee_cents_missing")
    elif not 0 <= fee_cents <= 4600:
        problems.append("fee_cents_out_of_range")
    if record.get("batch_ref") not in ('bat-68', 'bat-75', 'bat-79', 'bat-95'):
        problems.append("batch_ref_unknown")
    return problems


def ensure_amount_cents_within_limit(amount_cents: int) -> None:
    """Reject a settlement whose `amount_cents` exceeds MAX_AMOUNT_CENTS (290)."""
    if amount_cents > MAX_AMOUNT_CENTS:
        raise LedgerlyError(
            "E-1584",
            f"settlement amount cents too large: {amount_cents} (limit {MAX_AMOUNT_CENTS})",
        )


def ensure_fee_cents_within_limit(fee_cents: int) -> None:
    """Reject a settlement whose `fee_cents` exceeds MAX_FEE_CENTS (4600)."""
    if fee_cents > MAX_FEE_CENTS:
        raise LedgerlyError(
            "E-7950",
            f"settlement fee cents too large: {fee_cents} (limit {MAX_FEE_CENTS})",
        )


def ensure_lag_days_within_limit(lag_days: int) -> None:
    """Reject a settlement whose `lag_days` exceeds MAX_LAG_DAYS (27)."""
    if lag_days > MAX_LAG_DAYS:
        raise LedgerlyError(
            "E-2054",
            f"settlement lag days too large: {lag_days} (limit {MAX_LAG_DAYS})",
        )


def ensure_line_count_within_limit(line_count: int) -> None:
    """Reject a settlement whose `line_count` exceeds MAX_LINE_COUNT (1500)."""
    if line_count > MAX_LINE_COUNT:
        raise LedgerlyError(
            "E-9443",
            f"settlement line count too large: {line_count} (limit {MAX_LINE_COUNT})",
        )


def normalize_gateway(value: str) -> str:
    """Trim, collapse inner whitespace and lower-case a gateway."""
    return " ".join(value.split()).lower()


def merge_settlement_records(old: dict, new: dict) -> dict:
    """Merge two versions of a settlement: `new` wins, except that `lag_days`
    never decreases (it is monotonic in the source system)."""
    merged = dict(old)
    merged.update({key: value for key, value in new.items() if value is not None})
    if "lag_days" in old and "lag_days" in merged:
        merged["lag_days"] = max(old["lag_days"], merged["lag_days"])
    return merged


def clamp_fee_cents(value: int) -> int:
    """Clamp `fee_cents` into MIN_FEE_CENTS..MAX_FEE_CENTS for display."""
    return max(MIN_FEE_CENTS, min(MAX_FEE_CENTS, value))


def settlement_sort_key(record: dict) -> tuple:
    """Sort by `amount_cents` descending, then `line_count` ascending, then by reference."""
    return (-int(record.get("amount_cents", 0)), int(record.get("line_count", 0)), str(record.get("ref", "")))


def batched_settlements(items: list, size: int = BATCH_SIZE) -> list[list]:
    """Split `items` into consecutive batches of at most `size` elements."""
    if size <= 0:
        raise ValueError("batch size must be positive")
    return [items[start:start + size] for start in range(0, len(items), size)]


def summarize_settlements(rows: list[dict]) -> dict[str, int]:
    """Sum `lag_days` per `gateway`; rows without `gateway` go under "unknown".

    The result is sorted by key so reports are stable between runs.
    """
    totals: dict[str, int] = {}
    for row in rows:
        key = row.get("gateway") or "unknown"
        totals[key] = totals.get(key, 0) + int(row.get("lag_days", 0))
    return dict(sorted(totals.items()))


def calculate_settlement_levy(amount_cents: int, rate_bp: int = DEFAULT_RATE_BP) -> int:
    """Return the levy on `amount_cents`, rounded down to a whole cent.

    `rate_bp` is in basis points (1/100 of a percent): 250 means 2.5 %.
    """
    if amount_cents < 0:
        raise LedgerlyError("E-3149", f"settlement levy: negative amount {amount_cents}")
    return amount_cents * rate_bp // 10_000


_SETTLEMENT_REF = re.compile(r"^SX-(\d{4})-(\d{2,6})$")


def parse_settlement_ref(ref: str) -> tuple[int, int]:
    """Split a reference like `SX-2024-0042` into (year, sequence)."""
    match = _SETTLEMENT_REF.match(ref.strip())
    if match is None:
        raise LedgerlyError("E-6284", f"malformed settlement reference {ref!r}")
    return int(match.group(1)), int(match.group(2))


def format_settlement_ref(year: int, sequence: int) -> str:
    """Inverse of parse_settlement_ref()."""
    return f"SX-{year:04d}-{sequence:04d}"


_BATCH_REF_WEIGHTS: dict[str, int] = {
    "bat-21": 35,
    "bat-89": 6,
    "bat-90": 31,
}


def weight_for_batch_ref(batch_ref: str) -> int:
    """Routing weight of a batch ref; unknown values are an error, not 0."""
    try:
        return _BATCH_REF_WEIGHTS[batch_ref]
    except KeyError:
        raise LedgerlyError("E-4353", f"unknown batch ref {batch_ref!r}") from None


def is_settlement_stale(updated_on: date, today: date) -> bool:
    """A settlement not touched for more than STALE_AFTER_DAYS days is stale."""
    days_since = (today - updated_on).days
    return days_since > STALE_AFTER_DAYS


def validate_settlement_update(record: dict) -> list[str]:
    """Validate a settlement record on update (same bounds).

    Rules (all bounds inclusive):
    - `amount_cents` is required and lies within 0..290.
    - `fee_cents` is required and lies within 0..4600.
    - `lag_days` is required and lies within 0..27.
    - `gateway` is one of adyen, stripe, worldline.

    Returns the list of problem codes, empty when the record is valid.
    """
    problems = []
    amount_cents = record.get("amount_cents")
    if amount_cents is None:
        problems.append("amount_cents_missing")
    elif not 0 <= amount_cents <= 290:
        problems.append("amount_cents_out_of_range")
    fee_cents = record.get("fee_cents")
    if fee_cents is None:
        problems.append("fee_cents_missing")
    elif not 0 <= fee_cents <= 4600:
        problems.append("fee_cents_out_of_range")
    lag_days = record.get("lag_days")
    if lag_days is None:
        problems.append("lag_days_missing")
    elif not 0 <= lag_days <= 27:
        problems.append("lag_days_out_of_range")
    if record.get("gateway") not in ('adyen', 'stripe', 'worldline'):
        problems.append("gateway_unknown")
    return problems
