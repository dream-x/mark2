"""Date helpers shared by every module."""

from __future__ import annotations

from datetime import date, timedelta


def days_between(start: date, end: date) -> int:
    """Whole days from `start` to `end` (negative when `end` is earlier)."""
    return (end - start).days


def add_business_days(start: date, days: int) -> date:
    """`start` plus `days` working days, skipping Saturdays and Sundays."""
    current = start
    remaining = days
    while remaining > 0:
        current += timedelta(days=1)
        if current.weekday() < 5:
            remaining -= 1
    return current


def month_end(day: date) -> date:
    """Last day of the month `day` is in."""
    first_next = (day.replace(day=28) + timedelta(days=4)).replace(day=1)
    return first_next - timedelta(days=1)
