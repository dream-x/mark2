#!/usr/bin/env python3
"""Generator of setup/ for T22-ctx-repo-needle.

    python3 bench/tasks/T22-ctx-repo-needle/gen.py          # rewrite setup/
    python3 bench/tasks/T22-ctx-repo-needle/gen.py --check  # compare, exit 1 on drift

Deterministic: fixed seeds (one per file, derived with crc32 of its path, so
editing one module does not reshuffle the others), no network, no clock.
The script lives next to setup/, not inside it: the runner copies setup/ into
the sandbox as is, and this file names the needle and every trap.

The repository is a fake shop backend, `src/ledgerly/`. Planted files are
written verbatim below; the rest is filler generated from per-module
vocabularies (validators, limits, fee maths, lookup tables) with unique error
codes. After a regeneration, update verify.sh: the cksum manifest (`cksum`
of every file but the needle), the needle's line count, the bug line (printed
here) and the cksum of the needle without that line; then run selftest.sh.

Planted (see verify.sh for the trap list):
  src/ledgerly/payments/refunds.py         the needle: `>=` must become `>`
                                           in check_refund_window(); the
                                           same line in partial_refund_allowed()
                                           is correct
  src/ledgerly/legacy/refunds_v1.py        dead v1 copy, same code E-4127,
                                           same line, correct for the v1 rule
  src/ledgerly/marketplace/seller_refunds.py   same line, correct (14 days,
                                           day 14 already too late), E-4172
  src/ledgerly/shipping/returns.py         near twin, E-4217, inclusive
  docs/errors.md                           stale: points E-4127 at legacy
  src/ledgerly/api/errors.py, notifications/templates.py   more E-4127 hits
"""

from __future__ import annotations

import random
import re
import shutil
import sys
import tempfile
import zlib
from pathlib import Path

HERE = Path(__file__).resolve().parent
SETUP = HERE / "setup"

NEEDLE = "src/ledgerly/payments/refunds.py"
# 1-based line of the bug in NEEDLE; verify.sh compares every other line.
BUG_LINE_TEXT = "    if age_days >= REFUND_WINDOW_DAYS:"

# Error codes made of the digits of the planted ones would turn a careful
# grep for E-4127 into a lottery; filler stays clear of all of them.
RESERVED_DIGITS = ({"1", "2", "4", "7"}, {"1", "2", "4", "8"})


def rng_for(name: str) -> random.Random:
    return random.Random(0x7222_0000 ^ zlib.crc32(name.encode()))


# --------------------------------------------------------------------------
# Error codes: one global registry, so docs/errors.md and api/errors.py list
# every code the repository raises.
# --------------------------------------------------------------------------

CODES: dict[str, tuple[str, str, int]] = {}  # code -> (message, module, http)
_code_rng = random.Random(0x7222)


def new_code(message: str, module: str, http: int = 422) -> str:
    while True:
        number = _code_rng.randrange(1000, 9900)
        digits = set(str(number))
        if any(digits == reserved for reserved in RESERVED_DIGITS):
            continue
        code = f"E-{number}"
        if code not in CODES:
            CODES[code] = (message, module, http)
            return code


def planted_code(code: str, message: str, module: str, http: int) -> str:
    assert code not in CODES, code
    CODES[code] = (message, module, http)
    return code


def dotted(path: str) -> str:
    return path.removeprefix("src/").removesuffix(".py").replace("/", ".")


# --------------------------------------------------------------------------
# Filler vocabularies: (path, purpose, entity, int fields, str fields)
# --------------------------------------------------------------------------

FILLER = [
    ("src/ledgerly/api/routes.py", "Route table of the public HTTP API and routing helpers.",
     "route", ["timeout_ms", "max_body_kb", "rate_per_minute", "burst_size"], ["method", "path_prefix"]),
    ("src/ledgerly/api/serializers.py", "JSON serialisation of API payloads.",
     "payload", ["max_depth", "max_items", "max_string_len", "precision"], ["schema_name", "content_type"]),
    ("src/ledgerly/billing/invoices.py", "Invoice issuing, numbering and validation.",
     "invoice", ["line_count", "total_cents", "due_in_days", "copies"], ["currency", "series"]),
    ("src/ledgerly/billing/credit_notes.py", "Credit notes issued against posted invoices.",
     "credit_note", ["amount_cents", "line_count", "reason_code", "revision"], ["currency", "issuer"]),
    ("src/ledgerly/billing/dunning.py", "Dunning: reminders and escalation for overdue invoices.",
     "reminder", ["days_overdue", "attempt", "fee_cents", "grace_days"], ["channel", "stage"]),
    ("src/ledgerly/billing/proration.py", "Proration of subscription charges on plan changes.",
     "proration", ["days_used", "days_in_period", "amount_cents", "seats"], ["plan", "interval"]),
    ("src/ledgerly/catalog/products.py", "Product records and their validation.",
     "product", ["weight_g", "length_mm", "price_cents", "stock_floor"], ["sku", "brand"]),
    ("src/ledgerly/catalog/pricing.py", "Price lists, markups and price rounding.",
     "price", ["base_cents", "markup_bp", "floor_cents", "ceiling_cents"], ["price_list", "currency"]),
    ("src/ledgerly/catalog/bundles.py", "Product bundles and bundle discounts.",
     "bundle", ["item_count", "discount_bp", "max_qty", "min_qty"], ["bundle_code", "label"]),
    ("src/ledgerly/customers/accounts.py", "Customer accounts and account limits.",
     "account", ["credit_limit_cents", "open_orders", "age_years", "failed_logins"], ["email", "tier"]),
    ("src/ledgerly/customers/loyalty.py", "Loyalty points: earning, burning and expiry.",
     "loyalty_entry", ["points", "multiplier_bp", "expires_in_days", "level"], ["program", "source"]),
    ("src/ledgerly/inventory/stock.py", "Stock levels per warehouse and reorder points.",
     "stock_level", ["on_hand", "reserved", "reorder_point", "safety_stock"], ["warehouse", "sku"]),
    ("src/ledgerly/inventory/reservations.py", "Stock reservations held for open orders.",
     "reservation", ["quantity", "hold_minutes", "priority", "extensions"], ["order_ref", "warehouse"]),
    ("src/ledgerly/inventory/transfers.py", "Stock transfers between warehouses.",
     "transfer", ["quantity", "pallets", "distance_km", "transit_days"], ["origin", "destination"]),
    ("src/ledgerly/legacy/exports_v1.py", "DEPRECATED v1 CSV exports, kept for the nightly replay job.",
     "export_row", ["column_count", "row_limit", "batch_size", "retries"], ["delimiter", "encoding"]),
    ("src/ledgerly/legacy/invoices_v1.py", "DEPRECATED v1 invoice numbering, kept for the archive importer.",
     "legacy_invoice", ["sequence", "year", "total_cents", "copies"], ["prefix", "office"]),
    ("src/ledgerly/marketplace/sellers.py", "Marketplace sellers: onboarding and limits.",
     "seller", ["listing_limit", "rating_bp", "open_disputes", "payout_delay_days"], ["seller_code", "country"]),
    ("src/ledgerly/marketplace/payouts.py", "Seller payouts and payout holds.",
     "payout", ["amount_cents", "hold_days", "fee_bp", "batch_no"], ["iban_country", "currency"]),
    ("src/ledgerly/marketplace/disputes.py", "Buyer-seller disputes and their deadlines.",
     "dispute", ["days_open", "evidence_count", "amount_cents", "escalation_level"], ["dispute_type", "status"]),
    ("src/ledgerly/notifications/email.py", "Transactional email rendering and throttling.",
     "email", ["size_kb", "attachments", "per_hour", "retry_count"], ["template", "locale"]),
    ("src/ledgerly/notifications/scheduler.py", "Scheduling of outbound notifications.",
     "job", ["delay_s", "max_attempts", "batch_size", "priority"], ["queue", "kind"]),
    ("src/ledgerly/payments/capture.py", "Payment capture after authorisation.",
     "capture", ["amount_cents", "auth_age_hours", "attempt", "partial_count"], ["gateway", "currency"]),
    ("src/ledgerly/payments/chargebacks.py", "Chargeback intake and representment.",
     "chargeback", ["amount_cents", "days_to_respond", "evidence_count", "reason_group"], ["network", "stage"]),
    ("src/ledgerly/payments/gateways.py", "Payment gateway selection and gateway limits.",
     "gateway_call", ["timeout_ms", "amount_cents", "retries", "weight"], ["gateway", "region"]),
    ("src/ledgerly/payments/ledger.py", "Double-entry ledger postings for payments.",
     "posting", ["amount_cents", "line_count", "period", "sequence"], ["account_code", "journal"]),
    ("src/ledgerly/reporting/daily.py", "Daily sales and refund reports.",
     "report_row", ["orders", "units", "gross_cents", "net_cents"], ["channel", "region"]),
    ("src/ledgerly/reporting/reconciliation.py", "Reconciliation of gateway settlements against the ledger.",
     "settlement", ["amount_cents", "fee_cents", "lag_days", "line_count"], ["gateway", "batch_ref"]),
    ("src/ledgerly/shipping/labels.py", "Shipping label generation.",
     "label", ["weight_g", "width_mm", "height_mm", "copies"], ["carrier", "service"]),
    ("src/ledgerly/shipping/carriers.py", "Carrier selection and carrier service limits.",
     "shipment", ["weight_g", "parcels", "declared_cents", "transit_days"], ["carrier", "zone"]),
    ("src/ledgerly/shipping/tracking.py", "Tracking events and delivery confirmation.",
     "tracking_event", ["scan_count", "hours_since_scan", "attempt", "exceptions"], ["carrier", "status"]),
    ("src/ledgerly/tax/rates.py", "Tax rates by jurisdiction.",
     "tax_line", ["rate_bp", "amount_cents", "jurisdiction_level", "rounding_unit"], ["jurisdiction", "category"]),
    ("src/ledgerly/tax/vat.py", "VAT numbers and reverse-charge rules.",
     "vat_record", ["rate_bp", "amount_cents", "threshold_cents", "period"], ["vat_number", "country"]),
]

STR_CHOICES = {
    "method": ["GET", "POST", "PUT", "DELETE"],
    "currency": ["EUR", "USD", "GBP", "PLN", "SEK"],
    "channel": ["email", "sms", "letter", "web"],
    "status": ["open", "pending", "closed", "escalated"],
    "carrier": ["DHL", "UPS", "DPD", "GLS", "POSTNL"],
    "region": ["eu-west", "eu-central", "us-east", "ap-south"],
    "gateway": ["adyen", "stripe", "worldline", "klarna"],
    "country": ["DE", "NL", "FR", "PL", "SE", "ES"],
    "locale": ["en", "de", "nl", "fr", "pl"],
    "tier": ["basic", "plus", "gold", "staff"],
    "stage": ["first", "second", "final", "legal"],
    "kind": ["email", "sms", "push", "webhook"],
}

ADJ = ["stale", "oversized", "risky", "blocked", "incomplete", "urgent"]
VERBS = ["compute", "estimate", "derive", "calculate"]


def const(name: str) -> str:
    return name.upper()


def choices_for(field: str, rng: random.Random) -> list[str]:
    if field in STR_CHOICES:
        pool = STR_CHOICES[field]
    else:
        pool = [f"{field[:3]}-{rng.randrange(10, 99)}" for _ in range(6)]
    return sorted(rng.sample(pool, k=min(len(pool), rng.randrange(3, 5))))


def human(name: str) -> str:
    return name.replace("_", " ")


def camel(name: str) -> str:
    return "".join(part.capitalize() for part in name.split("_"))


class Filler:
    """Builds one filler module from its vocabulary."""

    def __init__(self, path: str, purpose: str, entity: str, ints: list[str], strs: list[str]):
        self.path = path
        self.module = dotted(path)
        self.purpose = purpose
        self.entity = entity
        self.ints = ints
        self.strs = strs
        self.rng = rng_for(path)
        self.limits = {field: self.rng.randrange(5, 50) * self.rng.choice([1, 10, 100]) for field in ints}
        self.minimums = {field: self.rng.randrange(0, 3) for field in ints}
        self.parts: list[str] = []

    # -- pieces -----------------------------------------------------------

    def header(self) -> str:
        rng = self.rng
        owner = rng.choice(["payments team", "core team", "fulfilment team", "growth team", "finance tooling"])
        lines = [
            f'"""{self.purpose}',
            "",
            f"Owned by the {owner}. Limits below are product decisions; change them",
            "only together with the matching entry in docs/limits.md of the wiki.",
            '"""',
            "",
            "from __future__ import annotations",
            "",
            "import re",
            "from dataclasses import dataclass, field",
            "from datetime import date",
            "",
            "from ledgerly.util.errors import LedgerlyError",
            "",
        ]
        for name in self.ints:
            lines.append(f"MAX_{const(name)} = {self.limits[name]}")
        for name in self.ints[:2]:
            lines.append(f"MIN_{const(name)} = {self.minimums[name]}")
        lines.append(f"DEFAULT_RATE_BP = {rng.randrange(50, 900)}")
        lines.append(f"STALE_AFTER_DAYS = {rng.randrange(7, 120)}")
        lines.append(f"BATCH_SIZE = {rng.choice([25, 50, 100, 200, 500])}")
        lines.append("")
        return "\n".join(lines) + "\n"

    def dataclass_block(self) -> str:
        rng = self.rng
        cls = camel(self.entity)
        a, b, c, _ = self.ints
        s1, s2 = self.strs
        threshold = max(1, self.limits[a] // rng.randrange(2, 5))
        adj = rng.choice(ADJ)
        return f'''
@dataclass
class {cls}:
    """A {human(self.entity)} as stored in the {self.module.split(".")[1]} tables."""

    ref: str
    {a}: int = 0
    {b}: int = 0
    {c}: int = 0
    {s1}: str = ""
    {s2}: str = ""
    created_on: date = field(default_factory=lambda: date(2024, 1, 1))
    tags: list[str] = field(default_factory=list)

    def is_{adj}(self) -> bool:
        """True when `{a}` is above {threshold}, a third of the usual maximum."""
        return self.{a} > {threshold}

    def as_dict(self) -> dict:
        return {{
            "ref": self.ref,
            "{a}": self.{a},
            "{b}": self.{b},
            "{c}": self.{c},
            "{s1}": self.{s1},
            "{s2}": self.{s2},
            "created_on": self.created_on.isoformat(),
            "tags": list(self.tags),
        }}
'''

    def validate_block(self) -> str:
        rng = self.rng
        ent = self.entity
        fields = rng.sample(self.ints, 3)
        s = rng.choice(self.strs)
        options = choices_for(s, rng)
        rules = []
        body = []
        for name in fields:
            lo, hi = self.minimums.get(name, 0), self.limits[name]
            rules.append(f"    - `{name}` is required and lies within {lo}..{hi}.")
            body.append(f'''    {name} = record.get("{name}")
    if {name} is None:
        problems.append("{name}_missing")
    elif not {lo} <= {name} <= {hi}:
        problems.append("{name}_out_of_range")''')
        rules.append(f"    - `{s}` is one of {', '.join(options)}.")
        body.append(f'''    if record.get("{s}") not in {tuple(options)!r}:
        problems.append("{s}_unknown")''')
        rules_text = "\n".join(rules)
        body_text = "\n".join(body)
        return f'''

def validate_{ent}(record: dict) -> list[str]:
    """Validate a {human(ent)} record before it is stored.

    Rules (all bounds inclusive):
{rules_text}

    Returns the list of problem codes, empty when the record is valid.
    """
    problems = []
{body_text}
    return problems
'''

    def ensure_block(self, name: str) -> str:
        ent = self.entity
        msg = f"{human(ent)} {human(name)} too large"
        code = new_code(msg, self.module)
        return f'''

def ensure_{name}_within_limit({name}: int) -> None:
    """Reject a {human(ent)} whose `{name}` exceeds MAX_{const(name)} ({self.limits[name]})."""
    if {name} > MAX_{const(name)}:
        raise LedgerlyError(
            "{code}",
            f"{msg}: {{{name}}} (limit {{MAX_{const(name)}}})",
        )
'''

    def fee_block(self) -> str:
        rng = self.rng
        ent = self.entity
        verb = rng.choice(VERBS)
        thing = rng.choice(["fee", "surcharge", "commission", "reserve", "levy"])
        msg = f"{human(ent)} {thing}: negative amount"
        code = new_code(msg, self.module)
        return f'''

def {verb}_{ent}_{thing}(amount_cents: int, rate_bp: int = DEFAULT_RATE_BP) -> int:
    """Return the {thing} on `amount_cents`, rounded down to a whole cent.

    `rate_bp` is in basis points (1/100 of a percent): 250 means 2.5 %.
    """
    if amount_cents < 0:
        raise LedgerlyError("{code}", f"{msg} {{amount_cents}}")
    return amount_cents * rate_bp // 10_000
'''

    def table_block(self) -> str:
        rng = self.rng
        key = rng.choice(self.strs)
        options = choices_for(key, rng)
        table = "_" + const(key) + "_WEIGHTS"
        rows = "\n".join(f'    "{opt}": {rng.randrange(1, 40)},' for opt in options)
        msg = f"unknown {human(key)}"
        code = new_code(msg, self.module, 400)
        return f'''

{table}: dict[str, int] = {{
{rows}
}}


def weight_for_{key}({key}: str) -> int:
    """Routing weight of a {human(key)}; unknown values are an error, not 0."""
    try:
        return {table}[{key}]
    except KeyError:
        raise LedgerlyError("{code}", f"{msg} {{{key}!r}}") from None
'''

    def summarize_block(self) -> str:
        rng = self.rng
        group = rng.choice(self.strs)
        amount = rng.choice(self.ints)
        ent = self.entity
        return f'''

def summarize_{ent}s(rows: list[dict]) -> dict[str, int]:
    """Sum `{amount}` per `{group}`; rows without `{group}` go under "unknown".

    The result is sorted by key so reports are stable between runs.
    """
    totals: dict[str, int] = {{}}
    for row in rows:
        key = row.get("{group}") or "unknown"
        totals[key] = totals.get(key, 0) + int(row.get("{amount}", 0))
    return dict(sorted(totals.items()))
'''

    def stale_block(self) -> str:
        ent = self.entity
        return f'''

def is_{ent}_stale(updated_on: date, today: date) -> bool:
    """A {human(ent)} not touched for more than STALE_AFTER_DAYS days is stale."""
    days_since = (today - updated_on).days
    return days_since > STALE_AFTER_DAYS
'''

    def normalize_block(self) -> str:
        rng = self.rng
        s = rng.choice(self.strs)
        style = rng.choice(["upper", "lower"])
        return f'''

def normalize_{s}(value: str) -> str:
    """Trim, collapse inner whitespace and {style}-case a {human(s)}."""
    return " ".join(value.split()).{style}()
'''

    def parse_block(self) -> str:
        rng = self.rng
        ent = self.entity
        prefix = "".join(part[0] for part in ent.split("_")).upper() + rng.choice("KMQXZ")
        msg = f"malformed {human(ent)} reference"
        code = new_code(msg, self.module, 400)
        return f'''

_{const(ent)}_REF = re.compile(r"^{prefix}-(\\d{{4}})-(\\d{{2,6}})$")


def parse_{ent}_ref(ref: str) -> tuple[int, int]:
    """Split a reference like `{prefix}-2024-0042` into (year, sequence)."""
    match = _{const(ent)}_REF.match(ref.strip())
    if match is None:
        raise LedgerlyError("{code}", f"{msg} {{ref!r}}")
    return int(match.group(1)), int(match.group(2))


def format_{ent}_ref(year: int, sequence: int) -> str:
    """Inverse of parse_{ent}_ref()."""
    return f"{prefix}-{{year:04d}}-{{sequence:04d}}"
'''

    def batches_block(self) -> str:
        ent = self.entity
        return f'''

def batched_{ent}s(items: list, size: int = BATCH_SIZE) -> list[list]:
    """Split `items` into consecutive batches of at most `size` elements."""
    if size <= 0:
        raise ValueError("batch size must be positive")
    return [items[start:start + size] for start in range(0, len(items), size)]
'''

    def merge_block(self) -> str:
        rng = self.rng
        ent = self.entity
        keep = rng.choice(self.ints)
        return f'''

def merge_{ent}_records(old: dict, new: dict) -> dict:
    """Merge two versions of a {human(ent)}: `new` wins, except that `{keep}`
    never decreases (it is monotonic in the source system)."""
    merged = dict(old)
    merged.update({{key: value for key, value in new.items() if value is not None}})
    if "{keep}" in old and "{keep}" in merged:
        merged["{keep}"] = max(old["{keep}"], merged["{keep}"])
    return merged
'''

    def sort_block(self) -> str:
        rng = self.rng
        ent = self.entity
        a, b = rng.sample(self.ints, 2)
        return f'''

def {ent}_sort_key(record: dict) -> tuple:
    """Sort by `{a}` descending, then `{b}` ascending, then by reference."""
    return (-int(record.get("{a}", 0)), int(record.get("{b}", 0)), str(record.get("ref", "")))
'''

    def clamp_block(self) -> str:
        rng = self.rng
        name = rng.choice(self.ints[:2])
        return f'''

def clamp_{name}(value: int) -> int:
    """Clamp `{name}` into MIN_{const(name)}..MAX_{const(name)} for display."""
    return max(MIN_{const(name)}, min(MAX_{const(name)}, value))
'''

    def build(self) -> str:
        blocks = [self.dataclass_block(), self.validate_block()]
        blocks += [self.ensure_block(name) for name in self.ints]
        extras = [
            self.fee_block, self.table_block, self.summarize_block, self.stale_block,
            self.normalize_block, self.parse_block, self.batches_block, self.merge_block,
            self.sort_block, self.clamp_block,
        ]
        order = list(range(len(extras)))
        self.rng.shuffle(order)
        blocks += [extras[index]() for index in order]
        # A second validator over the other fields keeps module sizes close.
        blocks.append(self.validate_block().replace(
            f"def validate_{self.entity}(", f"def validate_{self.entity}_update(", 1
        ).replace("before it is stored.", "on update (same bounds).", 1))
        return self.header() + "".join(blocks)


# --------------------------------------------------------------------------
# Planted files
# --------------------------------------------------------------------------

UTIL_ERRORS = '''"""Base error of the ledgerly code base.

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
'''

REFUNDS = '''"""Customer refunds: eligibility checks and refund amounts.

This is the refund path of the shop: ledgerly.api.handlers.request_refund()
calls into it for every refund a customer asks for. Support quotes the
docstrings below to customers, so they are the specification.
"""

from __future__ import annotations

from dataclasses import dataclass
from datetime import date

from ledgerly.util.errors import LedgerlyError

REFUND_WINDOW_DAYS = 30
RESTOCKING_FEE_BP = 1_500
MIN_REFUND_CENTS = 100
MAX_REFUNDS_PER_ORDER = 3
NON_REFUNDABLE_CATEGORIES = ("gift_card", "digital_download", "custom_print")


class RefundError(LedgerlyError):
    """A refund request the refund policy rejects."""


@dataclass(frozen=True)
class RefundRequest:
    """One refund request as it arrives from the API."""

    order_id: str
    delivered_on: date
    requested_on: date
    amount_cents: int
    category: str = "general"
    partial: bool = False
    previous_refunds: int = 0


def order_age_days(delivered_on: date, today: date) -> int:
    """Whole days between delivery and `today`: 0 on the delivery day."""
    return (today - delivered_on).days


def check_refund_window(delivered_on: date, today: date) -> None:
    """Reject a refund requested outside the refund window.

    A customer may ask for a refund from the delivery day (day 0) up to and
    including day REFUND_WINDOW_DAYS (30) after delivery. From day 31 on the
    request is rejected with E-4127. A request dated before the delivery is
    rejected with E-4128.
    """
    age_days = order_age_days(delivered_on, today)
    if age_days < 0:
        raise RefundError(
            "E-4128",
            f"refund requested before delivery: order is {age_days} days old",
        )
    if age_days >= REFUND_WINDOW_DAYS:
        raise RefundError(
            "E-4127",
            f"refund window closed: order is {age_days} days old "
            f"(limit {REFUND_WINDOW_DAYS} days)",
        )


def partial_refund_allowed(delivered_on: date, today: date) -> bool:
    """Whether a partial refund (customer keeps the item) is still possible.

    Partial refunds close one day before full refunds: they are possible from
    day 0 up to and including day 29. On day REFUND_WINDOW_DAYS (30) only a
    full refund against the returned item is still possible.
    """
    age_days = order_age_days(delivered_on, today)
    if age_days >= REFUND_WINDOW_DAYS:
        return False
    return age_days >= 0


def check_refund_category(category: str) -> None:
    """Gift cards, downloads and custom prints are never refundable (E-4131)."""
    if category in NON_REFUNDABLE_CATEGORIES:
        raise RefundError("E-4131", f"category {category!r} is not refundable")


def check_refund_count(previous_refunds: int) -> None:
    """At most MAX_REFUNDS_PER_ORDER (3) refunds per order; the 4th is E-4133."""
    if previous_refunds >= MAX_REFUNDS_PER_ORDER:
        raise RefundError(
            "E-4133",
            f"too many refunds on this order: {previous_refunds} already "
            f"(limit {MAX_REFUNDS_PER_ORDER})",
        )


def restocking_fee_cents(amount_cents: int) -> int:
    """Restocking fee of a full refund: 15 % (RESTOCKING_FEE_BP), rounded down.

    Partial refunds carry no restocking fee: the item stays with the customer.
    """
    return amount_cents * RESTOCKING_FEE_BP // 10_000


def refund_amount_cents(request: RefundRequest) -> int:
    """Amount actually paid back for an accepted request.

    Full refund: the order amount minus the restocking fee. Partial refund:
    half of the order amount, rounded down. Anything below MIN_REFUND_CENTS
    (1.00) is paid back as 0: the payment costs more than it returns.
    """
    if request.partial:
        amount = request.amount_cents // 2
    else:
        amount = request.amount_cents - restocking_fee_cents(request.amount_cents)
    return amount if amount >= MIN_REFUND_CENTS else 0


def evaluate_refund(request: RefundRequest) -> int:
    """Run every refund rule in order and return the amount to pay back.

    Order matters for the error the customer sees: the category is checked
    first, then the number of refunds, then the window. A partial request
    outside the partial window falls back to a full refund, which may still
    be inside the full window.
    """
    check_refund_category(request.category)
    check_refund_count(request.previous_refunds)
    check_refund_window(request.delivered_on, request.requested_on)
    if request.partial and not partial_refund_allowed(
        request.delivered_on, request.requested_on
    ):
        request = RefundRequest(
            order_id=request.order_id,
            delivered_on=request.delivered_on,
            requested_on=request.requested_on,
            amount_cents=request.amount_cents,
            category=request.category,
            partial=False,
            previous_refunds=request.previous_refunds,
        )
    return refund_amount_cents(request)


def refund_summary(requests: list[RefundRequest]) -> dict[str, int]:
    """Totals for the daily report: accepted, rejected and the amount paid."""
    accepted = rejected = paid = 0
    for request in requests:
        try:
            amount = evaluate_refund(request)
        except RefundError:
            rejected += 1
            continue
        accepted += 1
        paid += amount
    return {"accepted": accepted, "rejected": rejected, "paid_cents": paid}
'''

LEGACY_REFUNDS = '''"""DEPRECATED: refund checks of the v1 API.

Superseded by ledgerly.payments.refunds (ADR-0019, March 2025). Nothing
imports this module any more; it is kept only until the v1 export job that
replays historical refund decisions is retired. Do not "fix" anything here:
the replay must reproduce what v1 decided at the time, and the v1 rule was
different from today's (see check_refund_window below).
"""

from __future__ import annotations

from datetime import date

from ledgerly.util.errors import LedgerlyError

REFUND_WINDOW_DAYS = 30
LEGACY_RESTOCKING_FEE_BP = 2_000


class LegacyRefundError(LedgerlyError):
    """Rejection raised by the v1 refund rules."""


def check_refund_window(delivered_on: date, today: date) -> None:
    """v1 rule (historical, intentionally kept): the window closed ON day 30.

    Under v1 a refund was possible on days 0..29 only; a request on day 30
    was rejected with E-4127. The v2 policy (ledgerly.payments.refunds) moved
    the boundary; this copy must not follow it.
    """
    age_days = (today - delivered_on).days
    if age_days >= REFUND_WINDOW_DAYS:
        raise LegacyRefundError(
            "E-4127",
            f"refund window closed: order is {age_days} days old "
            f"(limit {REFUND_WINDOW_DAYS} days)",
        )


def legacy_refund_amount_cents(amount_cents: int) -> int:
    """v1 paid back the order amount minus a flat 20 % restocking fee."""
    return amount_cents - amount_cents * LEGACY_RESTOCKING_FEE_BP // 10_000


def replay_decision(delivered_on: date, requested_on: date, amount_cents: int) -> dict:
    """What v1 answered for a historical request: used by the replay job."""
    try:
        check_refund_window(delivered_on, requested_on)
    except LegacyRefundError as error:
        return {"accepted": False, "code": error.code, "paid_cents": 0}
    return {
        "accepted": True,
        "code": None,
        "paid_cents": legacy_refund_amount_cents(amount_cents),
    }
'''

SELLER_REFUNDS = '''"""Seller-funded refunds on marketplace orders.

Marketplace orders are refunded by the seller, not by the shop. Sellers sign
up to a shorter window than the shop's own refund policy: 14 days.
"""

from __future__ import annotations

from datetime import date

from ledgerly.util.errors import LedgerlyError

REFUND_WINDOW_DAYS = 14
SELLER_HANDLING_FEE_CENTS = 250


class SellerRefundError(LedgerlyError):
    """A seller-funded refund request the seller agreement rejects."""


def check_seller_refund_window(delivered_on: date, today: date) -> None:
    """Reject a seller-funded refund requested too late.

    The request must reach the seller within 14 days: it is possible on days
    0..13 after delivery, and on day REFUND_WINDOW_DAYS (14) it is already too
    late (E-4172). This is the seller agreement, not a bug: do not align it
    with the shop's own 30-day policy.
    """
    age_days = (today - delivered_on).days
    if age_days >= REFUND_WINDOW_DAYS:
        raise SellerRefundError(
            "E-4172",
            f"seller refund window closed: order is {age_days} days old "
            f"(limit {REFUND_WINDOW_DAYS} days)",
        )


def seller_refund_amount_cents(amount_cents: int) -> int:
    """The seller pays back the order amount minus a flat handling fee."""
    return max(0, amount_cents - SELLER_HANDLING_FEE_CENTS)


def route_refund(is_marketplace: bool) -> str:
    """Which module decides a refund: the seller's rules or the shop's."""
    return "marketplace.seller_refunds" if is_marketplace else "payments.refunds"
'''

RETURNS = '''"""Return labels for customer returns.

A customer can print a prepaid return label up to and including day 30 after
delivery; the label itself stays valid for LABEL_VALID_DAYS after printing.
"""

from __future__ import annotations

from datetime import date, timedelta

from ledgerly.util.errors import LedgerlyError

RETURN_WINDOW_DAYS = 30
LABEL_VALID_DAYS = 10
MAX_PARCELS_PER_RETURN = 4


class ReturnLabelError(LedgerlyError):
    """A return label request that cannot be served."""


def check_return_window(delivered_on: date, today: date) -> None:
    """Return labels: days 0..30 after delivery inclusive, E-4217 afterwards."""
    age_days = (today - delivered_on).days
    if age_days > RETURN_WINDOW_DAYS:
        raise ReturnLabelError(
            "E-4217",
            f"return window closed: order is {age_days} days old "
            f"(limit {RETURN_WINDOW_DAYS} days)",
        )


def label_expires_on(printed_on: date) -> date:
    """Last day the carrier accepts the label (inclusive)."""
    return printed_on + timedelta(days=LABEL_VALID_DAYS)


def check_parcel_count(parcels: int) -> None:
    """At most MAX_PARCELS_PER_RETURN parcels per return (E-4219)."""
    if not 1 <= parcels <= MAX_PARCELS_PER_RETURN:
        raise ReturnLabelError(
            "E-4219",
            f"a return has {parcels} parcels (allowed 1..{MAX_PARCELS_PER_RETURN})",
        )
'''

HANDLERS_HEAD = '''"""HTTP handlers of the public API.

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
'''


def handlers_tail(registry: list[tuple[str, str]]) -> str:
    """Thin handlers over a sample of filler validators."""
    out = []
    for module, entity in registry:
        out.append(f'''

def validate_{entity}_handler(body: dict) -> tuple[int, dict]:
    """POST /{entity.replace("_", "-")}s/validate"""
    from {module} import validate_{entity}

    problems = validate_{entity}(body)
    if problems:
        return 422, {{"problems": problems}}
    return 200, {{"status": "ok"}}
''')
    return "".join(out)


TEMPLATES_HEAD = '''"""Customer-facing texts for error codes.

The API returns the code; the storefront shows the text below. Codes without
an entry fall back to GENERIC_TEXT.
"""

from __future__ import annotations

GENERIC_TEXT = "Something went wrong. Please contact support."

CUSTOMER_TEXTS: dict[str, str] = {
    "E-4127": "We can no longer refund this order: refunds are possible for 30 days after delivery.",
    "E-4128": "This order has not been delivered yet, so it cannot be refunded.",
    "E-4131": "Gift cards, downloads and custom prints cannot be refunded.",
    "E-4133": "This order has already been refunded the maximum number of times.",
    "E-4172": "The seller no longer accepts refunds for this order (14-day seller window).",
    "E-4217": "Return labels are available for 30 days after delivery.",
'''

TEMPLATES_TAIL = '''}


def customer_text(code: str) -> str:
    """Text shown to the customer for an error code."""
    return CUSTOMER_TEXTS.get(code, GENERIC_TEXT)


def render(code: str, order_id: str) -> str:
    """One-line message for emails: the order reference plus the text."""
    return f"Order {order_id}: {customer_text(code)}"
'''

API_ERRORS_HEAD = '''"""Mapping of error codes to HTTP status codes.

Generated from docs/errors.md and kept in sync by hand since the generator
was retired. A code that is missing here is answered with 400.
"""

from __future__ import annotations

HTTP_STATUS: dict[str, int] = {
'''

API_ERRORS_TAIL = '''}


def status_for(code: str) -> int:
    """HTTP status for an error code; unknown codes are a client error."""
    return HTTP_STATUS.get(code, 400)
'''

UTIL_DATES = '''"""Date helpers shared by every module."""

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
'''

UTIL_MONEY = '''"""Money helpers: amounts are integers in cents everywhere."""

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
'''

UTIL_IDS = '''"""Identifiers: order numbers and idempotency keys."""

from __future__ import annotations

import hashlib
import re

ORDER_ID = re.compile(r"^LD-(\\d{8})$")


def is_order_id(value: str) -> bool:
    """Order ids look like LD-00012345."""
    return ORDER_ID.match(value) is not None


def idempotency_key(*parts: str) -> str:
    """Stable 16-hex-digit key for a request made of `parts`."""
    digest = hashlib.sha256("|".join(parts).encode()).hexdigest()
    return digest[:16]
'''

TEST_MONEY = '''import sys
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "src"))

from ledgerly.util.money import format_cents, split_evenly, to_cents  # noqa: E402


class MoneyTest(unittest.TestCase):
    def test_to_cents(self):
        self.assertEqual(to_cents("12.34"), 1234)
        self.assertEqual(to_cents("0,5"), 50)
        self.assertEqual(to_cents("1.005"), 101)

    def test_format_cents(self):
        self.assertEqual(format_cents(1234), "12.34 EUR")
        self.assertEqual(format_cents(-5, "USD"), "-0.05 USD")

    def test_split_evenly(self):
        self.assertEqual(split_evenly(100, 3), [34, 33, 33])
        self.assertEqual(sum(split_evenly(1001, 7)), 1001)


if __name__ == "__main__":
    unittest.main()
'''

TEST_DATES = '''import sys
import unittest
from datetime import date
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "src"))

from ledgerly.util.dates import add_business_days, days_between, month_end  # noqa: E402


class DatesTest(unittest.TestCase):
    def test_days_between(self):
        self.assertEqual(days_between(date(2025, 1, 1), date(2025, 1, 31)), 30)
        self.assertEqual(days_between(date(2025, 1, 31), date(2025, 1, 1)), -30)

    def test_add_business_days(self):
        self.assertEqual(add_business_days(date(2025, 1, 3), 1), date(2025, 1, 6))

    def test_month_end(self):
        self.assertEqual(month_end(date(2024, 2, 10)), date(2024, 2, 29))


if __name__ == "__main__":
    unittest.main()
'''

README = '''# ledgerly

Backend of the shop: orders, payments, refunds, marketplace, shipping and
billing. Python 3.11+, no third-party dependencies.

    src/ledgerly/
      api/            HTTP handlers and error-code mapping
      billing/        invoices, credit notes, dunning, proration
      catalog/        products, prices, bundles
      customers/      accounts, loyalty
      inventory/      stock, reservations, transfers
      legacy/         v1 code kept for replay/archive jobs (not imported)
      marketplace/    sellers, payouts, disputes, seller-funded refunds
      notifications/  customer texts, email, scheduling
      payments/       capture, chargebacks, gateways, ledger, refunds
      reporting/      daily reports, reconciliation
      shipping/       labels, carriers, tracking, returns
      tax/            rates, VAT
      util/           shared helpers and the base error

Tests:

    python3 -m unittest discover -s tests

Error codes and their meaning: docs/errors.md.
'''

ARCHITECTURE = '''# Architecture notes

- Every domain rule lives in exactly one module; the API layer
  (`ledgerly.api.handlers`) only translates HTTP to function calls.
- Rejections are `LedgerlyError` subclasses with a stable code `E-NNNN`.
  `ledgerly.api.errors` maps codes to HTTP statuses,
  `ledgerly.notifications.templates` maps them to customer texts.
- Money is integer cents; rates are basis points. No floats in money paths.
- `ledgerly.legacy` holds v1 code that batch jobs still replay. It is not
  imported by the live code and must keep its historical behaviour.
- Marketplace orders are refunded by the seller under the seller agreement
  (`ledgerly.marketplace.seller_refunds`); the shop's own refunds are
  `ledgerly.payments.refunds`.
'''

PYPROJECT = '''[project]
name = "ledgerly"
version = "2.14.0"
requires-python = ">=3.11"

[tool.setuptools.packages.find]
where = ["src"]
'''


def init_file(package: str) -> str:
    return f'"""{package}"""\n'


# --------------------------------------------------------------------------
# Assembly
# --------------------------------------------------------------------------


def build() -> dict[str, str]:
    CODES.clear()
    _code_rng.seed(0x7222)
    files: dict[str, str] = {}

    planted_code("E-4127", "refund window closed",
                 "ledgerly.legacy.refunds_v1", 409)   # stale on purpose
    planted_code("E-4128", "refund requested before delivery",
                 "ledgerly.legacy.refunds_v1", 409)   # stale on purpose
    planted_code("E-4131", "category is not refundable", "ledgerly.payments.refunds", 409)
    planted_code("E-4133", "too many refunds on this order", "ledgerly.payments.refunds", 409)
    planted_code("E-4172", "seller refund window closed", "ledgerly.marketplace.seller_refunds", 409)
    planted_code("E-4217", "return window closed", "ledgerly.shipping.returns", 409)
    planted_code("E-4219", "parcel count out of range", "ledgerly.shipping.returns", 422)

    sample_handlers: list[tuple[str, str]] = []
    for index, spec in enumerate(FILLER):
        filler = Filler(*spec)
        files[spec[0]] = filler.build()
        if index % 4 == 1:
            sample_handlers.append((filler.module, filler.entity))

    files["src/ledgerly/util/errors.py"] = UTIL_ERRORS
    files["src/ledgerly/util/dates.py"] = UTIL_DATES
    files["src/ledgerly/util/money.py"] = UTIL_MONEY
    files["src/ledgerly/util/ids.py"] = UTIL_IDS
    files[NEEDLE] = REFUNDS
    files["src/ledgerly/legacy/refunds_v1.py"] = LEGACY_REFUNDS
    files["src/ledgerly/marketplace/seller_refunds.py"] = SELLER_REFUNDS
    files["src/ledgerly/shipping/returns.py"] = RETURNS
    files["src/ledgerly/api/handlers.py"] = HANDLERS_HEAD + handlers_tail(sample_handlers)

    texts_rng = rng_for("texts")
    text_rows = []
    for code in sorted(CODES):
        if code in ("E-4127", "E-4128", "E-4131", "E-4133", "E-4172", "E-4217"):
            continue
        if texts_rng.random() < 0.45:
            message = CODES[code][0]
            text_rows.append(f'    "{code}": "We could not process this request ({message}).",')
    files["src/ledgerly/notifications/templates.py"] = (
        TEMPLATES_HEAD + "\n".join(text_rows) + "\n" + TEMPLATES_TAIL
    )

    status_rows = "\n".join(f'    "{code}": {CODES[code][2]},' for code in sorted(CODES))
    files["src/ledgerly/api/errors.py"] = API_ERRORS_HEAD + status_rows + "\n" + API_ERRORS_TAIL

    doc_rows = "\n".join(
        f"| {code} | {message} | `{module}` | {http} |"
        for code, (message, module, http) in sorted(CODES.items())
    )
    files["docs/errors.md"] = (
        "# Error codes\n\n"
        "Every code the backend raises, the module that raises it and the HTTP\n"
        "status the API answers with. Regenerated 2025-02-11.\n\n"
        "| code | meaning | raised in | HTTP |\n|---|---|---|---|\n" + doc_rows + "\n"
    )
    files["docs/architecture.md"] = ARCHITECTURE
    files["README.md"] = README
    files["pyproject.toml"] = PYPROJECT
    files["tests/test_money.py"] = TEST_MONEY
    files["tests/test_dates.py"] = TEST_DATES

    packages = sorted({str(Path(path).parent) for path in files if path.startswith("src/ledgerly")})
    for package in packages:
        files[f"{package}/__init__.py"] = init_file(package.removeprefix("src/").replace("/", "."))

    return dict(sorted(files.items()))


def self_check(files: dict[str, str]) -> None:
    """Invariants verify.sh relies on; generation fails loudly otherwise."""
    for path, text in files.items():
        if path.endswith(".py"):
            compile(text, path, "exec")
        assert "\t" not in text and text.endswith("\n"), path

    def hits(needle: str) -> list[str]:
        return [path for path, text in files.items() if needle in text]

    bug_twins = hits(BUG_LINE_TEXT + "\n")
    assert bug_twins == [
        "src/ledgerly/legacy/refunds_v1.py",
        "src/ledgerly/marketplace/seller_refunds.py",
        NEEDLE,
    ], bug_twins
    assert files[NEEDLE].count(BUG_LINE_TEXT + "\n") == 2
    assert hits("E-4127") == [
        "docs/errors.md",
        "src/ledgerly/api/errors.py",
        "src/ledgerly/legacy/refunds_v1.py",
        "src/ledgerly/notifications/templates.py",
        NEEDLE,
    ], hits("E-4127")
    assert hits("refund window closed: order is") == [
        "src/ledgerly/legacy/refunds_v1.py",
        "src/ledgerly/marketplace/seller_refunds.py",
        NEEDLE,
    ], hits("refund window closed: order is")
    # Nothing live imports the legacy copy.
    assert not [p for p in hits("refunds_v1") if p.endswith(".py") and "legacy/" not in p]
    assert "AGENTS.md" not in {Path(p).name for p in files}


def bug_line(files: dict[str, str]) -> int:
    lines = files[NEEDLE].split("\n")
    index = lines.index(BUG_LINE_TEXT)  # the first one is check_refund_window()
    assert "def check_refund_window" in "\n".join(lines[index - 15:index])
    return index + 1


def write_tree(root: Path, files: dict[str, str]) -> None:
    for path, text in files.items():
        target = root / path
        target.parent.mkdir(parents=True, exist_ok=True)
        target.write_bytes(text.encode())


def tree_bytes(root: Path) -> dict[str, bytes]:
    return {
        str(p.relative_to(root)): p.read_bytes()
        for p in sorted(root.rglob("*"))
        if p.is_file()
    }


def main() -> int:
    files = build()
    self_check(files)
    if "--check" in sys.argv[1:]:
        with tempfile.TemporaryDirectory() as tmp:
            write_tree(Path(tmp), files)
            want = tree_bytes(Path(tmp))
        got = tree_bytes(SETUP) if SETUP.is_dir() else {}
        if want != got:
            drift = sorted(set(want) ^ set(got)) or [p for p in want if want[p] != got.get(p)]
            print("setup/ differs from the generator output: " + ", ".join(drift[:10]), file=sys.stderr)
            return 1
        print("setup/ matches the generator", file=sys.stderr)
        return 0
    if SETUP.exists():
        shutil.rmtree(SETUP)
    write_tree(SETUP, files)
    total = sum(len(text.encode()) for text in files.values())
    print(f"files: {len(files)}  bytes: {total}  bug line: {NEEDLE}:{bug_line(files)}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
