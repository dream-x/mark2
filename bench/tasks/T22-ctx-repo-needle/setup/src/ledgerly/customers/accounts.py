"""Customer accounts and account limits.

Owned by the growth team. Limits below are product decisions; change them
only together with the matching entry in docs/limits.md of the wiki.
"""

from __future__ import annotations

import re
from dataclasses import dataclass, field
from datetime import date

from ledgerly.util.errors import LedgerlyError

MAX_CREDIT_LIMIT_CENTS = 4500
MAX_OPEN_ORDERS = 500
MAX_AGE_YEARS = 400
MAX_FAILED_LOGINS = 24
MIN_CREDIT_LIMIT_CENTS = 2
MIN_OPEN_ORDERS = 2
DEFAULT_RATE_BP = 180
STALE_AFTER_DAYS = 16
BATCH_SIZE = 500


@dataclass
class Account:
    """A account as stored in the customers tables."""

    ref: str
    credit_limit_cents: int = 0
    open_orders: int = 0
    age_years: int = 0
    email: str = ""
    tier: str = ""
    created_on: date = field(default_factory=lambda: date(2024, 1, 1))
    tags: list[str] = field(default_factory=list)

    def is_stale(self) -> bool:
        """True when `credit_limit_cents` is above 1500, a third of the usual maximum."""
        return self.credit_limit_cents > 1500

    def as_dict(self) -> dict:
        return {
            "ref": self.ref,
            "credit_limit_cents": self.credit_limit_cents,
            "open_orders": self.open_orders,
            "age_years": self.age_years,
            "email": self.email,
            "tier": self.tier,
            "created_on": self.created_on.isoformat(),
            "tags": list(self.tags),
        }


def validate_account(record: dict) -> list[str]:
    """Validate a account record before it is stored.

    Rules (all bounds inclusive):
    - `failed_logins` is required and lies within 0..24.
    - `credit_limit_cents` is required and lies within 2..4500.
    - `open_orders` is required and lies within 2..500.
    - `email` is one of ema-10, ema-23, ema-26, ema-59.

    Returns the list of problem codes, empty when the record is valid.
    """
    problems = []
    failed_logins = record.get("failed_logins")
    if failed_logins is None:
        problems.append("failed_logins_missing")
    elif not 0 <= failed_logins <= 24:
        problems.append("failed_logins_out_of_range")
    credit_limit_cents = record.get("credit_limit_cents")
    if credit_limit_cents is None:
        problems.append("credit_limit_cents_missing")
    elif not 2 <= credit_limit_cents <= 4500:
        problems.append("credit_limit_cents_out_of_range")
    open_orders = record.get("open_orders")
    if open_orders is None:
        problems.append("open_orders_missing")
    elif not 2 <= open_orders <= 500:
        problems.append("open_orders_out_of_range")
    if record.get("email") not in ('ema-10', 'ema-23', 'ema-26', 'ema-59'):
        problems.append("email_unknown")
    return problems


def ensure_credit_limit_cents_within_limit(credit_limit_cents: int) -> None:
    """Reject a account whose `credit_limit_cents` exceeds MAX_CREDIT_LIMIT_CENTS (4500)."""
    if credit_limit_cents > MAX_CREDIT_LIMIT_CENTS:
        raise LedgerlyError(
            "E-6606",
            f"account credit limit cents too large: {credit_limit_cents} (limit {MAX_CREDIT_LIMIT_CENTS})",
        )


def ensure_open_orders_within_limit(open_orders: int) -> None:
    """Reject a account whose `open_orders` exceeds MAX_OPEN_ORDERS (500)."""
    if open_orders > MAX_OPEN_ORDERS:
        raise LedgerlyError(
            "E-4382",
            f"account open orders too large: {open_orders} (limit {MAX_OPEN_ORDERS})",
        )


def ensure_age_years_within_limit(age_years: int) -> None:
    """Reject a account whose `age_years` exceeds MAX_AGE_YEARS (400)."""
    if age_years > MAX_AGE_YEARS:
        raise LedgerlyError(
            "E-8099",
            f"account age years too large: {age_years} (limit {MAX_AGE_YEARS})",
        )


def ensure_failed_logins_within_limit(failed_logins: int) -> None:
    """Reject a account whose `failed_logins` exceeds MAX_FAILED_LOGINS (24)."""
    if failed_logins > MAX_FAILED_LOGINS:
        raise LedgerlyError(
            "E-2385",
            f"account failed logins too large: {failed_logins} (limit {MAX_FAILED_LOGINS})",
        )


def normalize_email(value: str) -> str:
    """Trim, collapse inner whitespace and upper-case a email."""
    return " ".join(value.split()).upper()


_ACCOUNT_REF = re.compile(r"^AQ-(\d{4})-(\d{2,6})$")


def parse_account_ref(ref: str) -> tuple[int, int]:
    """Split a reference like `AQ-2024-0042` into (year, sequence)."""
    match = _ACCOUNT_REF.match(ref.strip())
    if match is None:
        raise LedgerlyError("E-4622", f"malformed account reference {ref!r}")
    return int(match.group(1)), int(match.group(2))


def format_account_ref(year: int, sequence: int) -> str:
    """Inverse of parse_account_ref()."""
    return f"AQ-{year:04d}-{sequence:04d}"


_EMAIL_WEIGHTS: dict[str, int] = {
    "ema-10": 37,
    "ema-50": 29,
    "ema-92": 35,
    "ema-93": 33,
}


def weight_for_email(email: str) -> int:
    """Routing weight of a email; unknown values are an error, not 0."""
    try:
        return _EMAIL_WEIGHTS[email]
    except KeyError:
        raise LedgerlyError("E-1863", f"unknown email {email!r}") from None


def batched_accounts(items: list, size: int = BATCH_SIZE) -> list[list]:
    """Split `items` into consecutive batches of at most `size` elements."""
    if size <= 0:
        raise ValueError("batch size must be positive")
    return [items[start:start + size] for start in range(0, len(items), size)]


def summarize_accounts(rows: list[dict]) -> dict[str, int]:
    """Sum `failed_logins` per `tier`; rows without `tier` go under "unknown".

    The result is sorted by key so reports are stable between runs.
    """
    totals: dict[str, int] = {}
    for row in rows:
        key = row.get("tier") or "unknown"
        totals[key] = totals.get(key, 0) + int(row.get("failed_logins", 0))
    return dict(sorted(totals.items()))


def is_account_stale(updated_on: date, today: date) -> bool:
    """A account not touched for more than STALE_AFTER_DAYS days is stale."""
    days_since = (today - updated_on).days
    return days_since > STALE_AFTER_DAYS


def merge_account_records(old: dict, new: dict) -> dict:
    """Merge two versions of a account: `new` wins, except that `open_orders`
    never decreases (it is monotonic in the source system)."""
    merged = dict(old)
    merged.update({key: value for key, value in new.items() if value is not None})
    if "open_orders" in old and "open_orders" in merged:
        merged["open_orders"] = max(old["open_orders"], merged["open_orders"])
    return merged


def estimate_account_fee(amount_cents: int, rate_bp: int = DEFAULT_RATE_BP) -> int:
    """Return the fee on `amount_cents`, rounded down to a whole cent.

    `rate_bp` is in basis points (1/100 of a percent): 250 means 2.5 %.
    """
    if amount_cents < 0:
        raise LedgerlyError("E-1499", f"account fee: negative amount {amount_cents}")
    return amount_cents * rate_bp // 10_000


def account_sort_key(record: dict) -> tuple:
    """Sort by `credit_limit_cents` descending, then `failed_logins` ascending, then by reference."""
    return (-int(record.get("credit_limit_cents", 0)), int(record.get("failed_logins", 0)), str(record.get("ref", "")))


def clamp_open_orders(value: int) -> int:
    """Clamp `open_orders` into MIN_OPEN_ORDERS..MAX_OPEN_ORDERS for display."""
    return max(MIN_OPEN_ORDERS, min(MAX_OPEN_ORDERS, value))


def validate_account_update(record: dict) -> list[str]:
    """Validate a account record on update (same bounds).

    Rules (all bounds inclusive):
    - `credit_limit_cents` is required and lies within 2..4500.
    - `age_years` is required and lies within 1..400.
    - `failed_logins` is required and lies within 0..24.
    - `tier` is one of basic, gold, plus, staff.

    Returns the list of problem codes, empty when the record is valid.
    """
    problems = []
    credit_limit_cents = record.get("credit_limit_cents")
    if credit_limit_cents is None:
        problems.append("credit_limit_cents_missing")
    elif not 2 <= credit_limit_cents <= 4500:
        problems.append("credit_limit_cents_out_of_range")
    age_years = record.get("age_years")
    if age_years is None:
        problems.append("age_years_missing")
    elif not 1 <= age_years <= 400:
        problems.append("age_years_out_of_range")
    failed_logins = record.get("failed_logins")
    if failed_logins is None:
        problems.append("failed_logins_missing")
    elif not 0 <= failed_logins <= 24:
        problems.append("failed_logins_out_of_range")
    if record.get("tier") not in ('basic', 'gold', 'plus', 'staff'):
        problems.append("tier_unknown")
    return problems
