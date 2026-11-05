"""Route table of the public HTTP API and routing helpers.

Owned by the fulfilment team. Limits below are product decisions; change them
only together with the matching entry in docs/limits.md of the wiki.
"""

from __future__ import annotations

import re
from dataclasses import dataclass, field
from datetime import date

from ledgerly.util.errors import LedgerlyError

MAX_TIMEOUT_MS = 32
MAX_MAX_BODY_KB = 24
MAX_RATE_PER_MINUTE = 47
MAX_BURST_SIZE = 460
MIN_TIMEOUT_MS = 0
MIN_MAX_BODY_KB = 0
DEFAULT_RATE_BP = 417
STALE_AFTER_DAYS = 46
BATCH_SIZE = 25


@dataclass
class Route:
    """A route as stored in the api tables."""

    ref: str
    timeout_ms: int = 0
    max_body_kb: int = 0
    rate_per_minute: int = 0
    method: str = ""
    path_prefix: str = ""
    created_on: date = field(default_factory=lambda: date(2024, 1, 1))
    tags: list[str] = field(default_factory=list)

    def is_urgent(self) -> bool:
        """True when `timeout_ms` is above 10, a third of the usual maximum."""
        return self.timeout_ms > 10

    def as_dict(self) -> dict:
        return {
            "ref": self.ref,
            "timeout_ms": self.timeout_ms,
            "max_body_kb": self.max_body_kb,
            "rate_per_minute": self.rate_per_minute,
            "method": self.method,
            "path_prefix": self.path_prefix,
            "created_on": self.created_on.isoformat(),
            "tags": list(self.tags),
        }


def validate_route(record: dict) -> list[str]:
    """Validate a route record before it is stored.

    Rules (all bounds inclusive):
    - `burst_size` is required and lies within 0..460.
    - `timeout_ms` is required and lies within 0..32.
    - `rate_per_minute` is required and lies within 1..47.
    - `method` is one of DELETE, GET, POST, PUT.

    Returns the list of problem codes, empty when the record is valid.
    """
    problems = []
    burst_size = record.get("burst_size")
    if burst_size is None:
        problems.append("burst_size_missing")
    elif not 0 <= burst_size <= 460:
        problems.append("burst_size_out_of_range")
    timeout_ms = record.get("timeout_ms")
    if timeout_ms is None:
        problems.append("timeout_ms_missing")
    elif not 0 <= timeout_ms <= 32:
        problems.append("timeout_ms_out_of_range")
    rate_per_minute = record.get("rate_per_minute")
    if rate_per_minute is None:
        problems.append("rate_per_minute_missing")
    elif not 1 <= rate_per_minute <= 47:
        problems.append("rate_per_minute_out_of_range")
    if record.get("method") not in ('DELETE', 'GET', 'POST', 'PUT'):
        problems.append("method_unknown")
    return problems


def ensure_timeout_ms_within_limit(timeout_ms: int) -> None:
    """Reject a route whose `timeout_ms` exceeds MAX_TIMEOUT_MS (32)."""
    if timeout_ms > MAX_TIMEOUT_MS:
        raise LedgerlyError(
            "E-7230",
            f"route timeout ms too large: {timeout_ms} (limit {MAX_TIMEOUT_MS})",
        )


def ensure_max_body_kb_within_limit(max_body_kb: int) -> None:
    """Reject a route whose `max_body_kb` exceeds MAX_MAX_BODY_KB (24)."""
    if max_body_kb > MAX_MAX_BODY_KB:
        raise LedgerlyError(
            "E-9695",
            f"route max body kb too large: {max_body_kb} (limit {MAX_MAX_BODY_KB})",
        )


def ensure_rate_per_minute_within_limit(rate_per_minute: int) -> None:
    """Reject a route whose `rate_per_minute` exceeds MAX_RATE_PER_MINUTE (47)."""
    if rate_per_minute > MAX_RATE_PER_MINUTE:
        raise LedgerlyError(
            "E-1496",
            f"route rate per minute too large: {rate_per_minute} (limit {MAX_RATE_PER_MINUTE})",
        )


def ensure_burst_size_within_limit(burst_size: int) -> None:
    """Reject a route whose `burst_size` exceeds MAX_BURST_SIZE (460)."""
    if burst_size > MAX_BURST_SIZE:
        raise LedgerlyError(
            "E-5599",
            f"route burst size too large: {burst_size} (limit {MAX_BURST_SIZE})",
        )


_PATH_PREFIX_WEIGHTS: dict[str, int] = {
    "pat-38": 17,
    "pat-49": 33,
    "pat-66": 16,
    "pat-89": 18,
}


def weight_for_path_prefix(path_prefix: str) -> int:
    """Routing weight of a path prefix; unknown values are an error, not 0."""
    try:
        return _PATH_PREFIX_WEIGHTS[path_prefix]
    except KeyError:
        raise LedgerlyError("E-3547", f"unknown path prefix {path_prefix!r}") from None


def compute_route_surcharge(amount_cents: int, rate_bp: int = DEFAULT_RATE_BP) -> int:
    """Return the surcharge on `amount_cents`, rounded down to a whole cent.

    `rate_bp` is in basis points (1/100 of a percent): 250 means 2.5 %.
    """
    if amount_cents < 0:
        raise LedgerlyError("E-3625", f"route surcharge: negative amount {amount_cents}")
    return amount_cents * rate_bp // 10_000


def clamp_max_body_kb(value: int) -> int:
    """Clamp `max_body_kb` into MIN_MAX_BODY_KB..MAX_MAX_BODY_KB for display."""
    return max(MIN_MAX_BODY_KB, min(MAX_MAX_BODY_KB, value))


_ROUTE_REF = re.compile(r"^RK-(\d{4})-(\d{2,6})$")


def parse_route_ref(ref: str) -> tuple[int, int]:
    """Split a reference like `RK-2024-0042` into (year, sequence)."""
    match = _ROUTE_REF.match(ref.strip())
    if match is None:
        raise LedgerlyError("E-9086", f"malformed route reference {ref!r}")
    return int(match.group(1)), int(match.group(2))


def format_route_ref(year: int, sequence: int) -> str:
    """Inverse of parse_route_ref()."""
    return f"RK-{year:04d}-{sequence:04d}"


def batched_routes(items: list, size: int = BATCH_SIZE) -> list[list]:
    """Split `items` into consecutive batches of at most `size` elements."""
    if size <= 0:
        raise ValueError("batch size must be positive")
    return [items[start:start + size] for start in range(0, len(items), size)]


def merge_route_records(old: dict, new: dict) -> dict:
    """Merge two versions of a route: `new` wins, except that `timeout_ms`
    never decreases (it is monotonic in the source system)."""
    merged = dict(old)
    merged.update({key: value for key, value in new.items() if value is not None})
    if "timeout_ms" in old and "timeout_ms" in merged:
        merged["timeout_ms"] = max(old["timeout_ms"], merged["timeout_ms"])
    return merged


def summarize_routes(rows: list[dict]) -> dict[str, int]:
    """Sum `burst_size` per `path_prefix`; rows without `path_prefix` go under "unknown".

    The result is sorted by key so reports are stable between runs.
    """
    totals: dict[str, int] = {}
    for row in rows:
        key = row.get("path_prefix") or "unknown"
        totals[key] = totals.get(key, 0) + int(row.get("burst_size", 0))
    return dict(sorted(totals.items()))


def is_route_stale(updated_on: date, today: date) -> bool:
    """A route not touched for more than STALE_AFTER_DAYS days is stale."""
    days_since = (today - updated_on).days
    return days_since > STALE_AFTER_DAYS


def route_sort_key(record: dict) -> tuple:
    """Sort by `rate_per_minute` descending, then `timeout_ms` ascending, then by reference."""
    return (-int(record.get("rate_per_minute", 0)), int(record.get("timeout_ms", 0)), str(record.get("ref", "")))


def normalize_method(value: str) -> str:
    """Trim, collapse inner whitespace and lower-case a method."""
    return " ".join(value.split()).lower()


def validate_route_update(record: dict) -> list[str]:
    """Validate a route record on update (same bounds).

    Rules (all bounds inclusive):
    - `rate_per_minute` is required and lies within 1..47.
    - `timeout_ms` is required and lies within 0..32.
    - `max_body_kb` is required and lies within 0..24.
    - `path_prefix` is one of pat-27, pat-55, pat-61, pat-97.

    Returns the list of problem codes, empty when the record is valid.
    """
    problems = []
    rate_per_minute = record.get("rate_per_minute")
    if rate_per_minute is None:
        problems.append("rate_per_minute_missing")
    elif not 1 <= rate_per_minute <= 47:
        problems.append("rate_per_minute_out_of_range")
    timeout_ms = record.get("timeout_ms")
    if timeout_ms is None:
        problems.append("timeout_ms_missing")
    elif not 0 <= timeout_ms <= 32:
        problems.append("timeout_ms_out_of_range")
    max_body_kb = record.get("max_body_kb")
    if max_body_kb is None:
        problems.append("max_body_kb_missing")
    elif not 0 <= max_body_kb <= 24:
        problems.append("max_body_kb_out_of_range")
    if record.get("path_prefix") not in ('pat-27', 'pat-55', 'pat-61', 'pat-97'):
        problems.append("path_prefix_unknown")
    return problems
