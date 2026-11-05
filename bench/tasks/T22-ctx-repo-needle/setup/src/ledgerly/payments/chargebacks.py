"""Chargeback intake and representment.

Owned by the fulfilment team. Limits below are product decisions; change them
only together with the matching entry in docs/limits.md of the wiki.
"""

from __future__ import annotations

import re
from dataclasses import dataclass, field
from datetime import date

from ledgerly.util.errors import LedgerlyError

MAX_AMOUNT_CENTS = 43
MAX_DAYS_TO_RESPOND = 290
MAX_EVIDENCE_COUNT = 290
MAX_REASON_GROUP = 4700
MIN_AMOUNT_CENTS = 1
MIN_DAYS_TO_RESPOND = 2
DEFAULT_RATE_BP = 326
STALE_AFTER_DAYS = 54
BATCH_SIZE = 25


@dataclass
class Chargeback:
    """A chargeback as stored in the payments tables."""

    ref: str
    amount_cents: int = 0
    days_to_respond: int = 0
    evidence_count: int = 0
    network: str = ""
    stage: str = ""
    created_on: date = field(default_factory=lambda: date(2024, 1, 1))
    tags: list[str] = field(default_factory=list)

    def is_blocked(self) -> bool:
        """True when `amount_cents` is above 14, a third of the usual maximum."""
        return self.amount_cents > 14

    def as_dict(self) -> dict:
        return {
            "ref": self.ref,
            "amount_cents": self.amount_cents,
            "days_to_respond": self.days_to_respond,
            "evidence_count": self.evidence_count,
            "network": self.network,
            "stage": self.stage,
            "created_on": self.created_on.isoformat(),
            "tags": list(self.tags),
        }


def validate_chargeback(record: dict) -> list[str]:
    """Validate a chargeback record before it is stored.

    Rules (all bounds inclusive):
    - `amount_cents` is required and lies within 1..43.
    - `days_to_respond` is required and lies within 2..290.
    - `reason_group` is required and lies within 2..4700.
    - `stage` is one of final, first, legal, second.

    Returns the list of problem codes, empty when the record is valid.
    """
    problems = []
    amount_cents = record.get("amount_cents")
    if amount_cents is None:
        problems.append("amount_cents_missing")
    elif not 1 <= amount_cents <= 43:
        problems.append("amount_cents_out_of_range")
    days_to_respond = record.get("days_to_respond")
    if days_to_respond is None:
        problems.append("days_to_respond_missing")
    elif not 2 <= days_to_respond <= 290:
        problems.append("days_to_respond_out_of_range")
    reason_group = record.get("reason_group")
    if reason_group is None:
        problems.append("reason_group_missing")
    elif not 2 <= reason_group <= 4700:
        problems.append("reason_group_out_of_range")
    if record.get("stage") not in ('final', 'first', 'legal', 'second'):
        problems.append("stage_unknown")
    return problems


def ensure_amount_cents_within_limit(amount_cents: int) -> None:
    """Reject a chargeback whose `amount_cents` exceeds MAX_AMOUNT_CENTS (43)."""
    if amount_cents > MAX_AMOUNT_CENTS:
        raise LedgerlyError(
            "E-5497",
            f"chargeback amount cents too large: {amount_cents} (limit {MAX_AMOUNT_CENTS})",
        )


def ensure_days_to_respond_within_limit(days_to_respond: int) -> None:
    """Reject a chargeback whose `days_to_respond` exceeds MAX_DAYS_TO_RESPOND (290)."""
    if days_to_respond > MAX_DAYS_TO_RESPOND:
        raise LedgerlyError(
            "E-3470",
            f"chargeback days to respond too large: {days_to_respond} (limit {MAX_DAYS_TO_RESPOND})",
        )


def ensure_evidence_count_within_limit(evidence_count: int) -> None:
    """Reject a chargeback whose `evidence_count` exceeds MAX_EVIDENCE_COUNT (290)."""
    if evidence_count > MAX_EVIDENCE_COUNT:
        raise LedgerlyError(
            "E-5929",
            f"chargeback evidence count too large: {evidence_count} (limit {MAX_EVIDENCE_COUNT})",
        )


def ensure_reason_group_within_limit(reason_group: int) -> None:
    """Reject a chargeback whose `reason_group` exceeds MAX_REASON_GROUP (4700)."""
    if reason_group > MAX_REASON_GROUP:
        raise LedgerlyError(
            "E-7711",
            f"chargeback reason group too large: {reason_group} (limit {MAX_REASON_GROUP})",
        )


_CHARGEBACK_REF = re.compile(r"^CM-(\d{4})-(\d{2,6})$")


def parse_chargeback_ref(ref: str) -> tuple[int, int]:
    """Split a reference like `CM-2024-0042` into (year, sequence)."""
    match = _CHARGEBACK_REF.match(ref.strip())
    if match is None:
        raise LedgerlyError("E-2596", f"malformed chargeback reference {ref!r}")
    return int(match.group(1)), int(match.group(2))


def format_chargeback_ref(year: int, sequence: int) -> str:
    """Inverse of parse_chargeback_ref()."""
    return f"CM-{year:04d}-{sequence:04d}"


def normalize_network(value: str) -> str:
    """Trim, collapse inner whitespace and upper-case a network."""
    return " ".join(value.split()).upper()


def is_chargeback_stale(updated_on: date, today: date) -> bool:
    """A chargeback not touched for more than STALE_AFTER_DAYS days is stale."""
    days_since = (today - updated_on).days
    return days_since > STALE_AFTER_DAYS


def estimate_chargeback_fee(amount_cents: int, rate_bp: int = DEFAULT_RATE_BP) -> int:
    """Return the fee on `amount_cents`, rounded down to a whole cent.

    `rate_bp` is in basis points (1/100 of a percent): 250 means 2.5 %.
    """
    if amount_cents < 0:
        raise LedgerlyError("E-4225", f"chargeback fee: negative amount {amount_cents}")
    return amount_cents * rate_bp // 10_000


def batched_chargebacks(items: list, size: int = BATCH_SIZE) -> list[list]:
    """Split `items` into consecutive batches of at most `size` elements."""
    if size <= 0:
        raise ValueError("batch size must be positive")
    return [items[start:start + size] for start in range(0, len(items), size)]


def summarize_chargebacks(rows: list[dict]) -> dict[str, int]:
    """Sum `reason_group` per `stage`; rows without `stage` go under "unknown".

    The result is sorted by key so reports are stable between runs.
    """
    totals: dict[str, int] = {}
    for row in rows:
        key = row.get("stage") or "unknown"
        totals[key] = totals.get(key, 0) + int(row.get("reason_group", 0))
    return dict(sorted(totals.items()))


def merge_chargeback_records(old: dict, new: dict) -> dict:
    """Merge two versions of a chargeback: `new` wins, except that `reason_group`
    never decreases (it is monotonic in the source system)."""
    merged = dict(old)
    merged.update({key: value for key, value in new.items() if value is not None})
    if "reason_group" in old and "reason_group" in merged:
        merged["reason_group"] = max(old["reason_group"], merged["reason_group"])
    return merged


def clamp_days_to_respond(value: int) -> int:
    """Clamp `days_to_respond` into MIN_DAYS_TO_RESPOND..MAX_DAYS_TO_RESPOND for display."""
    return max(MIN_DAYS_TO_RESPOND, min(MAX_DAYS_TO_RESPOND, value))


def chargeback_sort_key(record: dict) -> tuple:
    """Sort by `amount_cents` descending, then `reason_group` ascending, then by reference."""
    return (-int(record.get("amount_cents", 0)), int(record.get("reason_group", 0)), str(record.get("ref", "")))


_NETWORK_WEIGHTS: dict[str, int] = {
    "net-13": 35,
    "net-43": 12,
    "net-44": 15,
}


def weight_for_network(network: str) -> int:
    """Routing weight of a network; unknown values are an error, not 0."""
    try:
        return _NETWORK_WEIGHTS[network]
    except KeyError:
        raise LedgerlyError("E-7078", f"unknown network {network!r}") from None


def validate_chargeback_update(record: dict) -> list[str]:
    """Validate a chargeback record on update (same bounds).

    Rules (all bounds inclusive):
    - `amount_cents` is required and lies within 1..43.
    - `days_to_respond` is required and lies within 2..290.
    - `reason_group` is required and lies within 2..4700.
    - `stage` is one of final, first, second.

    Returns the list of problem codes, empty when the record is valid.
    """
    problems = []
    amount_cents = record.get("amount_cents")
    if amount_cents is None:
        problems.append("amount_cents_missing")
    elif not 1 <= amount_cents <= 43:
        problems.append("amount_cents_out_of_range")
    days_to_respond = record.get("days_to_respond")
    if days_to_respond is None:
        problems.append("days_to_respond_missing")
    elif not 2 <= days_to_respond <= 290:
        problems.append("days_to_respond_out_of_range")
    reason_group = record.get("reason_group")
    if reason_group is None:
        problems.append("reason_group_missing")
    elif not 2 <= reason_group <= 4700:
        problems.append("reason_group_out_of_range")
    if record.get("stage") not in ('final', 'first', 'second'):
        problems.append("stage_unknown")
    return problems
