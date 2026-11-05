"""Daily sales and refund reports.

Owned by the growth team. Limits below are product decisions; change them
only together with the matching entry in docs/limits.md of the wiki.
"""

from __future__ import annotations

import re
from dataclasses import dataclass, field
from datetime import date

from ledgerly.util.errors import LedgerlyError

MAX_ORDERS = 2600
MAX_UNITS = 22
MAX_GROSS_CENTS = 4000
MAX_NET_CENTS = 700
MIN_ORDERS = 2
MIN_UNITS = 2
DEFAULT_RATE_BP = 777
STALE_AFTER_DAYS = 38
BATCH_SIZE = 100


@dataclass
class ReportRow:
    """A report row as stored in the reporting tables."""

    ref: str
    orders: int = 0
    units: int = 0
    gross_cents: int = 0
    channel: str = ""
    region: str = ""
    created_on: date = field(default_factory=lambda: date(2024, 1, 1))
    tags: list[str] = field(default_factory=list)

    def is_incomplete(self) -> bool:
        """True when `orders` is above 866, a third of the usual maximum."""
        return self.orders > 866

    def as_dict(self) -> dict:
        return {
            "ref": self.ref,
            "orders": self.orders,
            "units": self.units,
            "gross_cents": self.gross_cents,
            "channel": self.channel,
            "region": self.region,
            "created_on": self.created_on.isoformat(),
            "tags": list(self.tags),
        }


def validate_report_row(record: dict) -> list[str]:
    """Validate a report row record before it is stored.

    Rules (all bounds inclusive):
    - `gross_cents` is required and lies within 1..4000.
    - `units` is required and lies within 2..22.
    - `orders` is required and lies within 2..2600.
    - `region` is one of ap-south, eu-central, eu-west.

    Returns the list of problem codes, empty when the record is valid.
    """
    problems = []
    gross_cents = record.get("gross_cents")
    if gross_cents is None:
        problems.append("gross_cents_missing")
    elif not 1 <= gross_cents <= 4000:
        problems.append("gross_cents_out_of_range")
    units = record.get("units")
    if units is None:
        problems.append("units_missing")
    elif not 2 <= units <= 22:
        problems.append("units_out_of_range")
    orders = record.get("orders")
    if orders is None:
        problems.append("orders_missing")
    elif not 2 <= orders <= 2600:
        problems.append("orders_out_of_range")
    if record.get("region") not in ('ap-south', 'eu-central', 'eu-west'):
        problems.append("region_unknown")
    return problems


def ensure_orders_within_limit(orders: int) -> None:
    """Reject a report row whose `orders` exceeds MAX_ORDERS (2600)."""
    if orders > MAX_ORDERS:
        raise LedgerlyError(
            "E-9075",
            f"report row orders too large: {orders} (limit {MAX_ORDERS})",
        )


def ensure_units_within_limit(units: int) -> None:
    """Reject a report row whose `units` exceeds MAX_UNITS (22)."""
    if units > MAX_UNITS:
        raise LedgerlyError(
            "E-9771",
            f"report row units too large: {units} (limit {MAX_UNITS})",
        )


def ensure_gross_cents_within_limit(gross_cents: int) -> None:
    """Reject a report row whose `gross_cents` exceeds MAX_GROSS_CENTS (4000)."""
    if gross_cents > MAX_GROSS_CENTS:
        raise LedgerlyError(
            "E-7494",
            f"report row gross cents too large: {gross_cents} (limit {MAX_GROSS_CENTS})",
        )


def ensure_net_cents_within_limit(net_cents: int) -> None:
    """Reject a report row whose `net_cents` exceeds MAX_NET_CENTS (700)."""
    if net_cents > MAX_NET_CENTS:
        raise LedgerlyError(
            "E-3721",
            f"report row net cents too large: {net_cents} (limit {MAX_NET_CENTS})",
        )


def is_report_row_stale(updated_on: date, today: date) -> bool:
    """A report row not touched for more than STALE_AFTER_DAYS days is stale."""
    days_since = (today - updated_on).days
    return days_since > STALE_AFTER_DAYS


_CHANNEL_WEIGHTS: dict[str, int] = {
    "email": 11,
    "letter": 29,
    "sms": 5,
    "web": 37,
}


def weight_for_channel(channel: str) -> int:
    """Routing weight of a channel; unknown values are an error, not 0."""
    try:
        return _CHANNEL_WEIGHTS[channel]
    except KeyError:
        raise LedgerlyError("E-3683", f"unknown channel {channel!r}") from None


def normalize_channel(value: str) -> str:
    """Trim, collapse inner whitespace and lower-case a channel."""
    return " ".join(value.split()).lower()


def report_row_sort_key(record: dict) -> tuple:
    """Sort by `units` descending, then `orders` ascending, then by reference."""
    return (-int(record.get("units", 0)), int(record.get("orders", 0)), str(record.get("ref", "")))


def summarize_report_rows(rows: list[dict]) -> dict[str, int]:
    """Sum `units` per `channel`; rows without `channel` go under "unknown".

    The result is sorted by key so reports are stable between runs.
    """
    totals: dict[str, int] = {}
    for row in rows:
        key = row.get("channel") or "unknown"
        totals[key] = totals.get(key, 0) + int(row.get("units", 0))
    return dict(sorted(totals.items()))


def estimate_report_row_commission(amount_cents: int, rate_bp: int = DEFAULT_RATE_BP) -> int:
    """Return the commission on `amount_cents`, rounded down to a whole cent.

    `rate_bp` is in basis points (1/100 of a percent): 250 means 2.5 %.
    """
    if amount_cents < 0:
        raise LedgerlyError("E-3153", f"report row commission: negative amount {amount_cents}")
    return amount_cents * rate_bp // 10_000


def merge_report_row_records(old: dict, new: dict) -> dict:
    """Merge two versions of a report row: `new` wins, except that `gross_cents`
    never decreases (it is monotonic in the source system)."""
    merged = dict(old)
    merged.update({key: value for key, value in new.items() if value is not None})
    if "gross_cents" in old and "gross_cents" in merged:
        merged["gross_cents"] = max(old["gross_cents"], merged["gross_cents"])
    return merged


def clamp_units(value: int) -> int:
    """Clamp `units` into MIN_UNITS..MAX_UNITS for display."""
    return max(MIN_UNITS, min(MAX_UNITS, value))


_REPORT_ROW_REF = re.compile(r"^RRK-(\d{4})-(\d{2,6})$")


def parse_report_row_ref(ref: str) -> tuple[int, int]:
    """Split a reference like `RRK-2024-0042` into (year, sequence)."""
    match = _REPORT_ROW_REF.match(ref.strip())
    if match is None:
        raise LedgerlyError("E-3998", f"malformed report row reference {ref!r}")
    return int(match.group(1)), int(match.group(2))


def format_report_row_ref(year: int, sequence: int) -> str:
    """Inverse of parse_report_row_ref()."""
    return f"RRK-{year:04d}-{sequence:04d}"


def batched_report_rows(items: list, size: int = BATCH_SIZE) -> list[list]:
    """Split `items` into consecutive batches of at most `size` elements."""
    if size <= 0:
        raise ValueError("batch size must be positive")
    return [items[start:start + size] for start in range(0, len(items), size)]


def validate_report_row_update(record: dict) -> list[str]:
    """Validate a report row record on update (same bounds).

    Rules (all bounds inclusive):
    - `orders` is required and lies within 2..2600.
    - `gross_cents` is required and lies within 1..4000.
    - `net_cents` is required and lies within 0..700.
    - `region` is one of ap-south, eu-central, eu-west, us-east.

    Returns the list of problem codes, empty when the record is valid.
    """
    problems = []
    orders = record.get("orders")
    if orders is None:
        problems.append("orders_missing")
    elif not 2 <= orders <= 2600:
        problems.append("orders_out_of_range")
    gross_cents = record.get("gross_cents")
    if gross_cents is None:
        problems.append("gross_cents_missing")
    elif not 1 <= gross_cents <= 4000:
        problems.append("gross_cents_out_of_range")
    net_cents = record.get("net_cents")
    if net_cents is None:
        problems.append("net_cents_missing")
    elif not 0 <= net_cents <= 700:
        problems.append("net_cents_out_of_range")
    if record.get("region") not in ('ap-south', 'eu-central', 'eu-west', 'us-east'):
        problems.append("region_unknown")
    return problems
