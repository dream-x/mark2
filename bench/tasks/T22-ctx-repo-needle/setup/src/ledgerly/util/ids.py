"""Identifiers: order numbers and idempotency keys."""

from __future__ import annotations

import hashlib
import re

ORDER_ID = re.compile(r"^LD-(\d{8})$")


def is_order_id(value: str) -> bool:
    """Order ids look like LD-00012345."""
    return ORDER_ID.match(value) is not None


def idempotency_key(*parts: str) -> str:
    """Stable 16-hex-digit key for a request made of `parts`."""
    digest = hashlib.sha256("|".join(parts).encode()).hexdigest()
    return digest[:16]
