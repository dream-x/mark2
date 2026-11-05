"""HTTP handlers of the public API.

Each handler validates input, calls into the domain modules and maps a
LedgerlyError to a response through ledgerly.api.errors.status_for().
"""

from __future__ import annotations

from datetime import date

from ledgerly.api.errors import status_for
from ledgerly.marketplace.seller_refunds import check_seller_refund_window, route_refund
from ledgerly.payments.refunds import RefundRequest, evaluate_refund
from ledgerly.shipping.returns import check_return_window
from ledgerly.util.errors import LedgerlyError


def _error(error: LedgerlyError) -> tuple[int, dict]:
    return status_for(error.code), {"code": error.code, "message": error.message}


def request_refund(body: dict, today: date) -> tuple[int, dict]:
    """POST /orders/{id}/refunds"""
    delivered_on = date.fromisoformat(body["delivered_on"])
    try:
        if route_refund(bool(body.get("marketplace"))) == "marketplace.seller_refunds":
            check_seller_refund_window(delivered_on, today)
            return 202, {"status": "forwarded_to_seller"}
        amount = evaluate_refund(
            RefundRequest(
                order_id=body["order_id"],
                delivered_on=delivered_on,
                requested_on=today,
                amount_cents=int(body["amount_cents"]),
                category=body.get("category", "general"),
                partial=bool(body.get("partial", False)),
                previous_refunds=int(body.get("previous_refunds", 0)),
            )
        )
    except LedgerlyError as error:
        return _error(error)
    return 200, {"status": "refunded", "amount_cents": amount}


def request_return_label(body: dict, today: date) -> tuple[int, dict]:
    """POST /orders/{id}/return-label"""
    try:
        check_return_window(date.fromisoformat(body["delivered_on"]), today)
    except LedgerlyError as error:
        return _error(error)
    return 201, {"status": "label_created"}


def validate_payload_handler(body: dict) -> tuple[int, dict]:
    """POST /payloads/validate"""
    from ledgerly.api.serializers import validate_payload

    problems = validate_payload(body)
    if problems:
        return 422, {"problems": problems}
    return 200, {"status": "ok"}


def validate_proration_handler(body: dict) -> tuple[int, dict]:
    """POST /prorations/validate"""
    from ledgerly.billing.proration import validate_proration

    problems = validate_proration(body)
    if problems:
        return 422, {"problems": problems}
    return 200, {"status": "ok"}


def validate_account_handler(body: dict) -> tuple[int, dict]:
    """POST /accounts/validate"""
    from ledgerly.customers.accounts import validate_account

    problems = validate_account(body)
    if problems:
        return 422, {"problems": problems}
    return 200, {"status": "ok"}


def validate_transfer_handler(body: dict) -> tuple[int, dict]:
    """POST /transfers/validate"""
    from ledgerly.inventory.transfers import validate_transfer

    problems = validate_transfer(body)
    if problems:
        return 422, {"problems": problems}
    return 200, {"status": "ok"}


def validate_payout_handler(body: dict) -> tuple[int, dict]:
    """POST /payouts/validate"""
    from ledgerly.marketplace.payouts import validate_payout

    problems = validate_payout(body)
    if problems:
        return 422, {"problems": problems}
    return 200, {"status": "ok"}


def validate_capture_handler(body: dict) -> tuple[int, dict]:
    """POST /captures/validate"""
    from ledgerly.payments.capture import validate_capture

    problems = validate_capture(body)
    if problems:
        return 422, {"problems": problems}
    return 200, {"status": "ok"}


def validate_report_row_handler(body: dict) -> tuple[int, dict]:
    """POST /report-rows/validate"""
    from ledgerly.reporting.daily import validate_report_row

    problems = validate_report_row(body)
    if problems:
        return 422, {"problems": problems}
    return 200, {"status": "ok"}


def validate_tracking_event_handler(body: dict) -> tuple[int, dict]:
    """POST /tracking-events/validate"""
    from ledgerly.shipping.tracking import validate_tracking_event

    problems = validate_tracking_event(body)
    if problems:
        return 422, {"problems": problems}
    return 200, {"status": "ok"}
