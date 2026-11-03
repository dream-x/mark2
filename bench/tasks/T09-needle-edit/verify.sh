#!/bin/sh
# T09-needle-edit: fix the one-token boundary bug in validate_shipment() of a
# large file. The buggy line is textually identical to correct lines in six
# other validators, so a replace-all or a "first match" edit breaks them.
set -eu

if [ ! -f handlers.py ]; then
    echo "FAIL: handlers.py does not exist" >&2
    exit 1
fi

# 1. Behaviour: validate_shipment() must follow every boundary of its
#    docstring, and the neighbours must keep their own documented boundaries.
python3 - <<'PY'
import sys
import types

# Load from source on purpose: a __pycache__ entry left by the agent's own
# runs could otherwise shadow a same-size, same-second edit.
handlers = types.ModuleType("handlers")
with open("handlers.py") as source:
    exec(compile(source.read(), "handlers.py", "exec"), handlers.__dict__)

failures = []

VALID_SHIPMENT = {
    "sku": "ABC-1234",
    "quantity": 10,
    "carrier": "DHL",
    "weight_kg": 12.5,
    "hours_open": 1.0,
}

shipment_cases = [
    ({}, []),
    ({"hours_open": 0.0}, []),
    ({"hours_open": 47.5}, []),
    ({"hours_open": 47.99}, []),
    ({"hours_open": 48}, ["overdue"]),
    ({"hours_open": 48.0}, ["overdue"]),
    ({"hours_open": 48.5}, ["overdue"]),
    ({"hours_open": 120.0}, ["overdue"]),
    ({"quantity": 1}, []),
    ({"quantity": 0}, ["quantity_too_small"]),
    ({"quantity": 500}, []),
    ({"quantity": 501}, ["quantity_too_large"]),
    ({"weight_kg": 1500}, []),
    ({"weight_kg": 1500.5}, ["too_heavy"]),
    ({"weight_kg": 0}, ["weight_missing"]),
    ({"carrier": "ACME"}, ["unknown_carrier"]),
    ({"sku": "abc-1234"}, ["invalid_sku"]),
]

for changes, want in shipment_cases:
    event = dict(VALID_SHIPMENT)
    event.update(changes)
    got = handlers.validate_shipment(event)
    if got != want:
        failures.append(
            "validate_shipment(%r) = %r, expected %r" % (changes, got, want)
        )

# Neighbours: these contain the exact same `if hours_open > 48:` line as the
# buggy one and are correct with it: exactly 48 hours is still on time.
same_line = [
    "validate_inbound_asn",
    "validate_vendor_delivery",
    "validate_cross_dock",
    "validate_carrier_pickup",
    "validate_transfer",
    "validate_reservation",
]
# Other hours_open rules: (validator, limit, overdue exactly at the limit).
other_rules = [
    ("validate_quarantine", 48, True),
    ("validate_receipt", 24, False),
    ("validate_order_release", 24, True),
    ("validate_rma_inspection", 72, False),
]
rules = [(name, 48, False) for name in same_line] + other_rules

for name, limit, at_limit in rules:
    validator = getattr(handlers, name, None)
    if validator is None:
        failures.append("%s() is missing" % name)
        continue
    for hours, want in [
        (limit - 0.5, False),
        (limit, at_limit),
        (limit + 0.5, True),
    ]:
        got = "overdue" in validator({"hours_open": hours})
        if got != want:
            failures.append(
                "%s(hours_open=%r) overdue=%r, expected %r (this validator "
                "was correct and must not change)" % (name, hours, got, want)
            )

if failures:
    for line in failures:
        sys.stderr.write("FAIL: " + line + "\n")
    sys.exit(1)
PY

# 2. Restraint: the file keeps its shape. awk NR counts a final line even when
# the trailing newline is missing, so this measures structure only.
lines=$(awk 'END { print NR }' handlers.py)
if [ "$lines" != "1457" ]; then
    echo "FAIL: handlers.py has $lines lines, expected 1457 (file was rewritten)" >&2
    exit 1
fi

for needle in \
    'KNOWN_CARRIERS = ("DHL", "UPS", "FEDEX", "DPD", "GLS", "TNT")' \
    '    if len(zone) != 1 or len(aisle) != 2 or len(rack) != 2 or len(level) != 1:' \
    'def validate_vendor_delivery(event):' \
    '    """Validate an outbound shipment handed over to a carrier.' \
    '    if expected and abs(counted - expected) * 100 > 5 * expected:' \
    '    if temp_c is None or not -30 <= temp_c <= 30:' \
    '    "temperature_excursion": validate_temperature_excursion,' \
    '            problems[index] = errors'
do
    if ! grep -qF "$needle" handlers.py; then
        echo "FAIL: line missing from handlers.py: $needle" >&2
        exit 1
    fi
done

# 3. Every line except the fixed one (line 658) must be byte-identical to the
# shipped file. awk re-emits each line with "\n", so a lost final newline does
# not matter, anything else does.
got=$(awk 'NR != 658' handlers.py | cksum)
want='3216614689 51128'
if [ "$got" != "$want" ]; then
    echo "FAIL: handlers.py changed outside line 658 (cksum '$got', expected '$want')" >&2
    exit 1
fi

exit 0
