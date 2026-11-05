"""Money helpers: amounts are integers in cents everywhere."""

from __future__ import annotations

from decimal import ROUND_HALF_UP, Decimal


def to_cents(text: str) -> int:
    """Parse "12.34" (or "12,34") into 1234 cents, rounding half up."""
    value = Decimal(text.strip().replace(",", "."))
    return int((value * 100).quantize(Decimal("1"), rounding=ROUND_HALF_UP))


def format_cents(cents: int, currency: str = "EUR") -> str:
    """1234 -> "12.34 EUR"; negative amounts keep their sign."""
    sign = "-" if cents < 0 else ""
    cents = abs(cents)
    return f"{sign}{cents // 100}.{cents % 100:02d} {currency}"


def split_evenly(cents: int, parts: int) -> list[int]:
    """Split an amount into `parts` shares that differ by at most one cent."""
    base, rest = divmod(cents, parts)
    return [base + 1 if index < rest else base for index in range(parts)]
