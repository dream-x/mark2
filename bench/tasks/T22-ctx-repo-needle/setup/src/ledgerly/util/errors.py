"""Base error of the ledgerly code base.

Every rejection carries a stable code (E-NNNN) that the API maps to an HTTP
status (ledgerly.api.errors) and that notifications map to customer text
(ledgerly.notifications.templates).
"""

from __future__ import annotations


class LedgerlyError(Exception):
    """An error with a stable machine-readable code."""

    def __init__(self, code: str, message: str) -> None:
        super().__init__(f"{code} {message}")
        self.code = code
        self.message = message

    def __reduce__(self):
        return (type(self), (self.code, self.message))
