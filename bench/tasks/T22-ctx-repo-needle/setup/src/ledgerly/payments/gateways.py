"""Payment gateway selection and gateway limits.

Owned by the finance tooling. Limits below are product decisions; change them
only together with the matching entry in docs/limits.md of the wiki.
"""

from __future__ import annotations

import re
from dataclasses import dataclass, field
from datetime import date

from ledgerly.util.errors import LedgerlyError

MAX_TIMEOUT_MS = 26
MAX_AMOUNT_CENTS = 350
MAX_RETRIES = 27
MAX_WEIGHT = 700
MIN_TIMEOUT_MS = 1
MIN_AMOUNT_CENTS = 0
DEFAULT_RATE_BP = 690
STALE_AFTER_DAYS = 42
BATCH_SIZE = 200


@dataclass
class GatewayCall:
    """A gateway call as stored in the payments tables."""

    ref: str
    timeout_ms: int = 0
    amount_cents: int = 0
    retries: int = 0
    gateway: str = ""
    region: str = ""
    created_on: date = field(default_factory=lambda: date(2024, 1, 1))
    tags: list[str] = field(default_factory=list)

    def is_stale(self) -> bool:
        """True when `timeout_ms` is above 13, a third of the usual maximum."""
        return self.timeout_ms > 13

    def as_dict(self) -> dict:
        return {
            "ref": self.ref,
            "timeout_ms": self.timeout_ms,
            "amount_cents": self.amount_cents,
            "retries": self.retries,
            "gateway": self.gateway,
            "region": self.region,
            "created_on": self.created_on.isoformat(),
            "tags": list(self.tags),
        }


def validate_gateway_call(record: dict) -> list[str]:
    """Validate a gateway call record before it is stored.

    Rules (all bounds inclusive):
    - `timeout_ms` is required and lies within 1..26.
    - `weight` is required and lies within 0..700.
    - `amount_cents` is required and lies within 0..350.
    - `region` is one of ap-south, eu-central, eu-west, us-east.

    Returns the list of problem codes, empty when the record is valid.
    """
    problems = []
    timeout_ms = record.get("timeout_ms")
    if timeout_ms is None:
        problems.append("timeout_ms_missing")
    elif not 1 <= timeout_ms <= 26:
        problems.append("timeout_ms_out_of_range")
    weight = record.get("weight")
    if weight is None:
        problems.append("weight_missing")
    elif not 0 <= weight <= 700:
        problems.append("weight_out_of_range")
    amount_cents = record.get("amount_cents")
    if amount_cents is None:
        problems.append("amount_cents_missing")
    elif not 0 <= amount_cents <= 350:
        problems.append("amount_cents_out_of_range")
    if record.get("region") not in ('ap-south', 'eu-central', 'eu-west', 'us-east'):
        problems.append("region_unknown")
    return problems


def ensure_timeout_ms_within_limit(timeout_ms: int) -> None:
    """Reject a gateway call whose `timeout_ms` exceeds MAX_TIMEOUT_MS (26)."""
    if timeout_ms > MAX_TIMEOUT_MS:
        raise LedgerlyError(
            "E-6062",
            f"gateway call timeout ms too large: {timeout_ms} (limit {MAX_TIMEOUT_MS})",
        )


def ensure_amount_cents_within_limit(amount_cents: int) -> None:
    """Reject a gateway call whose `amount_cents` exceeds MAX_AMOUNT_CENTS (350)."""
    if amount_cents > MAX_AMOUNT_CENTS:
        raise LedgerlyError(
            "E-9438",
            f"gateway call amount cents too large: {amount_cents} (limit {MAX_AMOUNT_CENTS})",
        )


def ensure_retries_within_limit(retries: int) -> None:
    """Reject a gateway call whose `retries` exceeds MAX_RETRIES (27)."""
    if retries > MAX_RETRIES:
        raise LedgerlyError(
            "E-9486",
            f"gateway call retries too large: {retries} (limit {MAX_RETRIES})",
        )


def ensure_weight_within_limit(weight: int) -> None:
    """Reject a gateway call whose `weight` exceeds MAX_WEIGHT (700)."""
    if weight > MAX_WEIGHT:
        raise LedgerlyError(
            "E-2991",
            f"gateway call weight too large: {weight} (limit {MAX_WEIGHT})",
        )


def calculate_gateway_call_surcharge(amount_cents: int, rate_bp: int = DEFAULT_RATE_BP) -> int:
    """Return the surcharge on `amount_cents`, rounded down to a whole cent.

    `rate_bp` is in basis points (1/100 of a percent): 250 means 2.5 %.
    """
    if amount_cents < 0:
        raise LedgerlyError("E-2297", f"gateway call surcharge: negative amount {amount_cents}")
    return amount_cents * rate_bp // 10_000


def is_gateway_call_stale(updated_on: date, today: date) -> bool:
    """A gateway call not touched for more than STALE_AFTER_DAYS days is stale."""
    days_since = (today - updated_on).days
    return days_since > STALE_AFTER_DAYS


def normalize_gateway(value: str) -> str:
    """Trim, collapse inner whitespace and lower-case a gateway."""
    return " ".join(value.split()).lower()


_GATEWAY_WEIGHTS: dict[str, int] = {
    "adyen": 19,
    "klarna": 8,
    "stripe": 26,
    "worldline": 4,
}


def weight_for_gateway(gateway: str) -> int:
    """Routing weight of a gateway; unknown values are an error, not 0."""
    try:
        return _GATEWAY_WEIGHTS[gateway]
    except KeyError:
        raise LedgerlyError("E-5313", f"unknown gateway {gateway!r}") from None


def batched_gateway_calls(items: list, size: int = BATCH_SIZE) -> list[list]:
    """Split `items` into consecutive batches of at most `size` elements."""
    if size <= 0:
        raise ValueError("batch size must be positive")
    return [items[start:start + size] for start in range(0, len(items), size)]


def merge_gateway_call_records(old: dict, new: dict) -> dict:
    """Merge two versions of a gateway call: `new` wins, except that `retries`
    never decreases (it is monotonic in the source system)."""
    merged = dict(old)
    merged.update({key: value for key, value in new.items() if value is not None})
    if "retries" in old and "retries" in merged:
        merged["retries"] = max(old["retries"], merged["retries"])
    return merged


def gateway_call_sort_key(record: dict) -> tuple:
    """Sort by `weight` descending, then `timeout_ms` ascending, then by reference."""
    return (-int(record.get("weight", 0)), int(record.get("timeout_ms", 0)), str(record.get("ref", "")))


_GATEWAY_CALL_REF = re.compile(r"^GCX-(\d{4})-(\d{2,6})$")


def parse_gateway_call_ref(ref: str) -> tuple[int, int]:
    """Split a reference like `GCX-2024-0042` into (year, sequence)."""
    match = _GATEWAY_CALL_REF.match(ref.strip())
    if match is None:
        raise LedgerlyError("E-8165", f"malformed gateway call reference {ref!r}")
    return int(match.group(1)), int(match.group(2))


def format_gateway_call_ref(year: int, sequence: int) -> str:
    """Inverse of parse_gateway_call_ref()."""
    return f"GCX-{year:04d}-{sequence:04d}"


def summarize_gateway_calls(rows: list[dict]) -> dict[str, int]:
    """Sum `retries` per `region`; rows without `region` go under "unknown".

    The result is sorted by key so reports are stable between runs.
    """
    totals: dict[str, int] = {}
    for row in rows:
        key = row.get("region") or "unknown"
        totals[key] = totals.get(key, 0) + int(row.get("retries", 0))
    return dict(sorted(totals.items()))


def clamp_timeout_ms(value: int) -> int:
    """Clamp `timeout_ms` into MIN_TIMEOUT_MS..MAX_TIMEOUT_MS for display."""
    return max(MIN_TIMEOUT_MS, min(MAX_TIMEOUT_MS, value))


def validate_gateway_call_update(record: dict) -> list[str]:
    """Validate a gateway call record on update (same bounds).

    Rules (all bounds inclusive):
    - `retries` is required and lies within 1..27.
    - `weight` is required and lies within 0..700.
    - `amount_cents` is required and lies within 0..350.
    - `region` is one of ap-south, eu-central, eu-west, us-east.

    Returns the list of problem codes, empty when the record is valid.
    """
    problems = []
    retries = record.get("retries")
    if retries is None:
        problems.append("retries_missing")
    elif not 1 <= retries <= 27:
        problems.append("retries_out_of_range")
    weight = record.get("weight")
    if weight is None:
        problems.append("weight_missing")
    elif not 0 <= weight <= 700:
        problems.append("weight_out_of_range")
    amount_cents = record.get("amount_cents")
    if amount_cents is None:
        problems.append("amount_cents_missing")
    elif not 0 <= amount_cents <= 350:
        problems.append("amount_cents_out_of_range")
    if record.get("region") not in ('ap-south', 'eu-central', 'eu-west', 'us-east'):
        problems.append("region_unknown")
    return problems
