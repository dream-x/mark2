"""Validators for warehouse management events.

Every ``validate_*`` function takes one event dict (as decoded from the
message bus) and returns a list of error codes. An empty list means the event
is valid. The docstring of each validator is its specification: it lists the
fields that are checked and the exact boundaries that apply.

Shared helpers live at the top of the module. Validators are grouped by
process area: inbound, outbound, internal moves, quality and compliance.
"""

KNOWN_CARRIERS = ("DHL", "UPS", "FEDEX", "DPD", "GLS", "TNT")
HOLD_REASONS = ("damaged", "expired", "contaminated", "mislabeled")
RETURN_REASONS = ("wrong_item", "damaged", "not_needed", "late_delivery")
ADJUSTMENT_REASONS = ("found", "lost", "miscount", "system_error")
CANCEL_REASONS = ("customer_request", "out_of_stock", "fraud_check")
RECALL_REASONS = ("supplier_notice", "regulator_notice", "internal_audit")
DAMAGE_REASONS = ("crushed", "wet", "torn", "punctured")
SCRAP_REASONS = ("expired", "damaged", "obsolete")

SKU_PREFIX_LENGTH = 3
SKU_MIN_DIGITS = 4
SKU_MAX_DIGITS = 6

LOCATION_ZONES = "ABCDEFGH"
MAX_AISLE = 40
MAX_RACK = 24
MAX_LEVEL = 6

LOT_PREFIX = "LOT"
LOT_DIGITS = 6
CONTAINER_LETTERS = 4
CONTAINER_DIGITS = 7


# ---------------------------------------------------------------------------
# Shared helpers
# ---------------------------------------------------------------------------


def normalize_code(code):
    """Return `code` stripped of surrounding whitespace and upper-cased.

    Anything that is not a string is returned unchanged so that the
    validators can reject it with their own error code.
    """
    if not isinstance(code, str):
        return code
    return code.strip().upper()


def is_valid_sku(code):
    """Return True when `code` is a SKU code.

    A SKU code is three uppercase ASCII letters, a dash and then 4 to 6
    digits, for example "ABC-1234" or "XYZ-000123". Lowercase letters and
    surrounding whitespace are not accepted.
    """
    if not isinstance(code, str):
        return False
    prefix, sep, digits = code.partition("-")
    if sep != "-" or len(prefix) != SKU_PREFIX_LENGTH:
        return False
    if not (prefix.isascii() and prefix.isalpha() and prefix.isupper()):
        return False
    if not (digits.isascii() and digits.isdigit()):
        return False
    return SKU_MIN_DIGITS <= len(digits) <= SKU_MAX_DIGITS


def parse_location(code):
    """Split a location code into (zone, aisle, rack, level).

    Location codes look like "C-12-07-3": a zone letter, a two digit aisle,
    a two digit rack and a one digit level. Returns None when the code does
    not have that shape. The ranges are checked by is_valid_location.
    """
    if not isinstance(code, str):
        return None
    parts = code.split("-")
    if len(parts) != 4:
        return None
    zone, aisle, rack, level = parts
    if len(zone) != 1 or len(aisle) != 2 or len(rack) != 2 or len(level) != 1:
        return None
    if not (aisle.isdigit() and rack.isdigit() and level.isdigit()):
        return None
    return zone, int(aisle), int(rack), int(level)


def is_valid_location(code):
    """Return True when `code` names an existing bin.

    Zones are the letters A to H, aisles go from 1 to 40, racks from 1 to 24
    and levels from 1 to 6, all bounds inclusive.
    """
    parsed = parse_location(code)
    if parsed is None:
        return False
    zone, aisle, rack, level = parsed
    if zone not in LOCATION_ZONES:
        return False
    if not 1 <= aisle <= MAX_AISLE:
        return False
    return 1 <= rack <= MAX_RACK and 1 <= level <= MAX_LEVEL


def format_location(zone, aisle, rack, level):
    """Build a location code such as "C-12-07-3" from its four parts."""
    return "{0}-{1:02d}-{2:02d}-{3}".format(zone, aisle, rack, level)


def same_zone(first, second):
    """Return True when both location codes are valid and share a zone."""
    if not (is_valid_location(first) and is_valid_location(second)):
        return False
    return parse_location(first)[0] == parse_location(second)[0]


def aisle_distance(first, second):
    """Return how many aisles apart two valid locations are.

    Locations in different zones are considered 99 aisles apart. Raises
    ValueError when either location is invalid.
    """
    if not (is_valid_location(first) and is_valid_location(second)):
        raise ValueError("aisle_distance() needs two valid locations")
    zone_a, aisle_a, _, _ = parse_location(first)
    zone_b, aisle_b, _, _ = parse_location(second)
    if zone_a != zone_b:
        return 99
    return abs(aisle_a - aisle_b)


def is_valid_lot(code):
    """Return True when `code` is a lot number such as "LOT240117".

    A lot number is the prefix "LOT" followed by exactly six digits.
    """
    if not isinstance(code, str) or not code.startswith(LOT_PREFIX):
        return False
    digits = code[len(LOT_PREFIX):]
    return len(digits) == LOT_DIGITS and digits.isascii() and digits.isdigit()


def is_valid_container(code):
    """Return True when `code` is a shipping container number.

    Container numbers are four uppercase ASCII letters followed by seven
    digits, for example "MSCU1234567".
    """
    if not isinstance(code, str):
        return False
    if len(code) != CONTAINER_LETTERS + CONTAINER_DIGITS:
        return False
    letters = code[:CONTAINER_LETTERS]
    digits = code[CONTAINER_LETTERS:]
    if not (letters.isascii() and letters.isalpha() and letters.isupper()):
        return False
    return digits.isascii() and digits.isdigit()


def hours_between(start_ts, end_ts):
    """Return the number of hours from `start_ts` to `end_ts`.

    Both arguments are POSIX timestamps in seconds. The result is a float
    and is negative when `end_ts` is earlier than `start_ts`.
    """
    return (end_ts - start_ts) / 3600.0


def merge_errors(*groups):
    """Concatenate several error lists, dropping duplicates, keeping order."""
    merged = []
    for group in groups:
        for code in group:
            if code not in merged:
                merged.append(code)
    return merged


# ---------------------------------------------------------------------------
# Inbound: receiving goods into the warehouse
# ---------------------------------------------------------------------------


def validate_receipt(event):
    """Validate goods received at a dock door and booked into stock.

    Rules:
    - "sku" must be a valid SKU code (see is_valid_sku).
    - "quantity" must be a whole number from 1 to 2000, both inclusive.
    - "lot" must be a valid lot number (see is_valid_lot).
    - "location" must be a valid bin location (see is_valid_location).
    - "hours_open" (a float: hours since the event was raised) may be up to 24.
      The event is "overdue" only when it is more than 24 hours, so exactly 24
      hours is still on time.

    Returns the list of error codes; an empty list means the event is valid.
    """
    errors = []
    if not is_valid_sku(event.get("sku")):
        errors.append("invalid_sku")
    quantity = event.get("quantity", 0)
    if quantity < 1:
        errors.append("quantity_too_small")
    if quantity > 2000:
        errors.append("quantity_too_large")
    if not is_valid_lot(event.get("lot")):
        errors.append("invalid_lot")
    if not is_valid_location(event.get("location")):
        errors.append("invalid_location")
    hours_open = event.get("hours_open", 0.0)
    if hours_open > 24:
        errors.append("overdue")
    return errors


def validate_inbound_asn(event):
    """Validate an advance shipping notice announced by a supplier.

    Rules:
    - "reference" must start with "ASN-".
    - "carrier" must be one of KNOWN_CARRIERS.
    - "pallets" must be from 1 to 33, both inclusive.
    - "hours_open" (a float: hours since the event was raised) may be up to 48.
      The event is "overdue" only when it is more than 48 hours, so exactly 48
      hours is still on time.

    Returns the list of error codes; an empty list means the event is valid.
    """
    errors = []
    reference = str(event.get("reference", ""))
    if not reference.startswith("ASN-"):
        errors.append("bad_reference")
    if event.get("carrier") not in KNOWN_CARRIERS:
        errors.append("unknown_carrier")
    pallets = event.get("pallets", 0)
    if pallets < 1 or pallets > 33:
        errors.append("invalid_pallet_count")
    hours_open = event.get("hours_open", 0.0)
    if hours_open > 48:
        errors.append("overdue")
    return errors


def validate_putaway(event):
    """Validate a putaway task moving received goods into a storage bin.

    Rules:
    - "sku" must be a valid SKU code (see is_valid_sku).
    - "quantity" must be a whole number from 1 to 500, both inclusive.
    - "location" must be a valid bin location (see is_valid_location).
    - "hours_open" (a float: hours since the event was raised) may be up to 4.
      The event is "overdue" only when it is more than 4 hours, so exactly 4
      hours is still on time.

    Returns the list of error codes; an empty list means the event is valid.
    """
    errors = []
    if not is_valid_sku(event.get("sku")):
        errors.append("invalid_sku")
    quantity = event.get("quantity", 0)
    if quantity < 1:
        errors.append("quantity_too_small")
    if quantity > 500:
        errors.append("quantity_too_large")
    if not is_valid_location(event.get("location")):
        errors.append("invalid_location")
    hours_open = event.get("hours_open", 0.0)
    if hours_open > 4:
        errors.append("overdue")
    return errors


def validate_vendor_delivery(event):
    """Validate a truck delivery arriving from a vendor.

    Rules:
    - "carrier" must be one of KNOWN_CARRIERS.
    - "pallets" must be from 1 to 26, both inclusive.
    - "weight_kg" must be greater than 0 and at most 24000.
    - "hours_open" (a float: hours since the event was raised) may be up to 48.
      The event is "overdue" only when it is more than 48 hours, so exactly 48
      hours is still on time.

    Returns the list of error codes; an empty list means the event is valid.
    """
    errors = []
    if event.get("carrier") not in KNOWN_CARRIERS:
        errors.append("unknown_carrier")
    pallets = event.get("pallets", 0)
    if pallets < 1 or pallets > 26:
        errors.append("invalid_pallet_count")
    weight_kg = event.get("weight_kg", 0.0)
    if weight_kg <= 0:
        errors.append("weight_missing")
    if weight_kg > 24000:
        errors.append("too_heavy")
    hours_open = event.get("hours_open", 0.0)
    if hours_open > 48:
        errors.append("overdue")
    return errors


def validate_dock_appointment(event):
    """Validate a booked dock door slot for an inbound truck.

    Rules:
    - "reference" must start with "DCK-".
    - "carrier" must be one of KNOWN_CARRIERS.
    - "priority" (default 1) must be from 1 to 5, both inclusive.
    - "hours_open" (a float: hours since the event was raised) must stay below
      2. The event is "overdue" as soon as it reaches 2 hours, so exactly 2
      hours is already overdue.

    Returns the list of error codes; an empty list means the event is valid.
    """
    errors = []
    reference = str(event.get("reference", ""))
    if not reference.startswith("DCK-"):
        errors.append("bad_reference")
    if event.get("carrier") not in KNOWN_CARRIERS:
        errors.append("unknown_carrier")
    priority = event.get("priority", 1)
    if priority < 1 or priority > 5:
        errors.append("invalid_priority")
    hours_open = event.get("hours_open", 0.0)
    if hours_open >= 2:
        errors.append("overdue")
    return errors


def validate_cross_dock(event):
    """Validate goods passed straight from an inbound to an outbound dock.

    Rules:
    - "sku" must be a valid SKU code (see is_valid_sku).
    - "quantity" must be a whole number from 1 to 1000, both inclusive.
    - "source" and "destination" must both be valid bin locations and must not
      be the same location.
    - "hours_open" (a float: hours since the event was raised) may be up to 48.
      The event is "overdue" only when it is more than 48 hours, so exactly 48
      hours is still on time.

    Returns the list of error codes; an empty list means the event is valid.
    """
    errors = []
    if not is_valid_sku(event.get("sku")):
        errors.append("invalid_sku")
    quantity = event.get("quantity", 0)
    if quantity < 1:
        errors.append("quantity_too_small")
    if quantity > 1000:
        errors.append("quantity_too_large")
    source = event.get("source")
    destination = event.get("destination")
    if not is_valid_location(source):
        errors.append("invalid_source")
    if not is_valid_location(destination):
        errors.append("invalid_destination")
    if source == destination:
        errors.append("same_location")
    hours_open = event.get("hours_open", 0.0)
    if hours_open > 48:
        errors.append("overdue")
    return errors


def validate_inbound_hold(event):
    """Validate received goods held at the dock pending inspection.

    Rules:
    - "sku" must be a valid SKU code (see is_valid_sku).
    - "lot" must be a valid lot number (see is_valid_lot).
    - "reason" must be one of HOLD_REASONS.

    Returns the list of error codes; an empty list means the event is valid.
    """
    errors = []
    if not is_valid_sku(event.get("sku")):
        errors.append("invalid_sku")
    if not is_valid_lot(event.get("lot")):
        errors.append("invalid_lot")
    if event.get("reason") not in HOLD_REASONS:
        errors.append("invalid_reason")
    return errors


def validate_receipt_correction(event):
    """Validate a correction to a previously booked receipt.

    Rules:
    - "sku" must be a valid SKU code (see is_valid_sku).
    - "quantity" must be a whole number from 1 to 2000, both inclusive.
    - "reason" must be one of ADJUSTMENT_REASONS.
    - "reference" must start with "RCV-".

    Returns the list of error codes; an empty list means the event is valid.
    """
    errors = []
    if not is_valid_sku(event.get("sku")):
        errors.append("invalid_sku")
    quantity = event.get("quantity", 0)
    if quantity < 1:
        errors.append("quantity_too_small")
    if quantity > 2000:
        errors.append("quantity_too_large")
    if event.get("reason") not in ADJUSTMENT_REASONS:
        errors.append("invalid_reason")
    reference = str(event.get("reference", ""))
    if not reference.startswith("RCV-"):
        errors.append("bad_reference")
    return errors


def validate_customer_return(event):
    """Validate a parcel returned by a customer.

    Rules:
    - "sku" must be a valid SKU code (see is_valid_sku).
    - "quantity" must be a whole number from 1 to 50, both inclusive.
    - "reason" must be one of RETURN_REASONS.
    - "hours_open" (a float: hours since the event was raised) must stay below
      72. The event is "overdue" as soon as it reaches 72 hours, so exactly 72
      hours is already overdue.

    Returns the list of error codes; an empty list means the event is valid.
    """
    errors = []
    if not is_valid_sku(event.get("sku")):
        errors.append("invalid_sku")
    quantity = event.get("quantity", 0)
    if quantity < 1:
        errors.append("quantity_too_small")
    if quantity > 50:
        errors.append("quantity_too_large")
    if event.get("reason") not in RETURN_REASONS:
        errors.append("invalid_reason")
    hours_open = event.get("hours_open", 0.0)
    if hours_open >= 72:
        errors.append("overdue")
    return errors


def validate_unload(event):
    """Validate a truck being unloaded at a dock door.

    Rules:
    - "carrier" must be one of KNOWN_CARRIERS.
    - "pallets" must be from 1 to 33, both inclusive.
    - "weight_kg" must be greater than 0 and at most 26000.
    - "hours_open" (a float: hours since the event was raised) may be up to 6.
      The event is "overdue" only when it is more than 6 hours, so exactly 6
      hours is still on time.

    Returns the list of error codes; an empty list means the event is valid.
    """
    errors = []
    if event.get("carrier") not in KNOWN_CARRIERS:
        errors.append("unknown_carrier")
    pallets = event.get("pallets", 0)
    if pallets < 1 or pallets > 33:
        errors.append("invalid_pallet_count")
    weight_kg = event.get("weight_kg", 0.0)
    if weight_kg <= 0:
        errors.append("weight_missing")
    if weight_kg > 26000:
        errors.append("too_heavy")
    hours_open = event.get("hours_open", 0.0)
    if hours_open > 6:
        errors.append("overdue")
    return errors


# ---------------------------------------------------------------------------
# Outbound: orders, picking and shipping
# ---------------------------------------------------------------------------


def validate_order_release(event):
    """Validate a customer order released to the warehouse floor.

    Rules:
    - "reference" must start with "ORD-".
    - "priority" (default 1) must be from 1 to 5, both inclusive.
    - "hours_open" (a float: hours since the event was raised) must stay below
      24. The event is "overdue" as soon as it reaches 24 hours, so exactly 24
      hours is already overdue.

    Returns the list of error codes; an empty list means the event is valid.
    """
    errors = []
    reference = str(event.get("reference", ""))
    if not reference.startswith("ORD-"):
        errors.append("bad_reference")
    priority = event.get("priority", 1)
    if priority < 1 or priority > 5:
        errors.append("invalid_priority")
    hours_open = event.get("hours_open", 0.0)
    if hours_open >= 24:
        errors.append("overdue")
    return errors


def validate_wave_release(event):
    """Validate a pick wave grouping several orders.

    Rules:
    - "reference" must start with "WAV-".
    - "priority" (default 1) must be from 1 to 3, both inclusive.
    - "quantity" must be a whole number of at least 1 and strictly less than
      400.
    - "hours_open" (a float: hours since the event was raised) may be up to 12.
      The event is "overdue" only when it is more than 12 hours, so exactly 12
      hours is still on time.

    Returns the list of error codes; an empty list means the event is valid.
    """
    errors = []
    reference = str(event.get("reference", ""))
    if not reference.startswith("WAV-"):
        errors.append("bad_reference")
    priority = event.get("priority", 1)
    if priority < 1 or priority > 3:
        errors.append("invalid_priority")
    quantity = event.get("quantity", 0)
    if quantity < 1:
        errors.append("quantity_too_small")
    if quantity >= 400:
        errors.append("quantity_too_large")
    hours_open = event.get("hours_open", 0.0)
    if hours_open > 12:
        errors.append("overdue")
    return errors


def validate_pick(event):
    """Validate a single pick from a storage bin.

    Rules:
    - "sku" must be a valid SKU code (see is_valid_sku).
    - "quantity" must be a whole number from 1 to 200, both inclusive.
    - "location" must be a valid bin location (see is_valid_location).
    - "hours_open" (a float: hours since the event was raised) may be up to 8.
      The event is "overdue" only when it is more than 8 hours, so exactly 8
      hours is still on time.

    Returns the list of error codes; an empty list means the event is valid.
    """
    errors = []
    if not is_valid_sku(event.get("sku")):
        errors.append("invalid_sku")
    quantity = event.get("quantity", 0)
    if quantity < 1:
        errors.append("quantity_too_small")
    if quantity > 200:
        errors.append("quantity_too_large")
    if not is_valid_location(event.get("location")):
        errors.append("invalid_location")
    hours_open = event.get("hours_open", 0.0)
    if hours_open > 8:
        errors.append("overdue")
    return errors


def validate_pack(event):
    """Validate a packing station closing a parcel.

    Rules:
    - "sku" must be a valid SKU code (see is_valid_sku).
    - "quantity" must be a whole number from 1 to 200, both inclusive.
    - "weight_kg" must be greater than 0 and at most 70.
    - "hours_open" (a float: hours since the event was raised) may be up to 4.
      The event is "overdue" only when it is more than 4 hours, so exactly 4
      hours is still on time.

    Returns the list of error codes; an empty list means the event is valid.
    """
    errors = []
    if not is_valid_sku(event.get("sku")):
        errors.append("invalid_sku")
    quantity = event.get("quantity", 0)
    if quantity < 1:
        errors.append("quantity_too_small")
    if quantity > 200:
        errors.append("quantity_too_large")
    weight_kg = event.get("weight_kg", 0.0)
    if weight_kg <= 0:
        errors.append("weight_missing")
    if weight_kg > 70:
        errors.append("too_heavy")
    hours_open = event.get("hours_open", 0.0)
    if hours_open > 4:
        errors.append("overdue")
    return errors


def validate_pallet_build(event):
    """Validate a pallet assembled for an outbound load.

    Rules:
    - "sku" must be a valid SKU code (see is_valid_sku).
    - "quantity" must be a whole number from 1 to 240, both inclusive.
    - "weight_kg" must be greater than 0 and at most 1200.
    - "location" must be a valid bin location (see is_valid_location).

    Returns the list of error codes; an empty list means the event is valid.
    """
    errors = []
    if not is_valid_sku(event.get("sku")):
        errors.append("invalid_sku")
    quantity = event.get("quantity", 0)
    if quantity < 1:
        errors.append("quantity_too_small")
    if quantity > 240:
        errors.append("quantity_too_large")
    weight_kg = event.get("weight_kg", 0.0)
    if weight_kg <= 0:
        errors.append("weight_missing")
    if weight_kg > 1200:
        errors.append("too_heavy")
    if not is_valid_location(event.get("location")):
        errors.append("invalid_location")
    return errors


def validate_shipment(event):
    """Validate an outbound shipment handed over to a carrier.

    Rules:
    - "sku" must be a valid SKU code (see is_valid_sku).
    - "quantity" must be a whole number from 1 to 500, both inclusive.
    - "carrier" must be one of KNOWN_CARRIERS.
    - "weight_kg" must be greater than 0 and at most 1500.
    - "hours_open" (a float: hours since the event was raised) must stay below
      48. The event is "overdue" as soon as it reaches 48 hours, so exactly 48
      hours is already overdue.

    Returns the list of error codes; an empty list means the event is valid.
    """
    errors = []
    if not is_valid_sku(event.get("sku")):
        errors.append("invalid_sku")
    quantity = event.get("quantity", 0)
    if quantity < 1:
        errors.append("quantity_too_small")
    if quantity > 500:
        errors.append("quantity_too_large")
    if event.get("carrier") not in KNOWN_CARRIERS:
        errors.append("unknown_carrier")
    weight_kg = event.get("weight_kg", 0.0)
    if weight_kg <= 0:
        errors.append("weight_missing")
    if weight_kg > 1500:
        errors.append("too_heavy")
    hours_open = event.get("hours_open", 0.0)
    if hours_open > 48:
        errors.append("overdue")
    return errors


def validate_carrier_pickup(event):
    """Validate a carrier collecting pallets from a dock door.

    Rules:
    - "reference" must start with "PKP-".
    - "carrier" must be one of KNOWN_CARRIERS.
    - "pallets" must be from 1 to 33, both inclusive.
    - "hours_open" (a float: hours since the event was raised) may be up to 48.
      The event is "overdue" only when it is more than 48 hours, so exactly 48
      hours is still on time.

    Returns the list of error codes; an empty list means the event is valid.
    """
    errors = []
    reference = str(event.get("reference", ""))
    if not reference.startswith("PKP-"):
        errors.append("bad_reference")
    if event.get("carrier") not in KNOWN_CARRIERS:
        errors.append("unknown_carrier")
    pallets = event.get("pallets", 0)
    if pallets < 1 or pallets > 33:
        errors.append("invalid_pallet_count")
    hours_open = event.get("hours_open", 0.0)
    if hours_open > 48:
        errors.append("overdue")
    return errors


def validate_container_load(event):
    """Validate a sea container being loaded for export.

    Rules:
    - "container" must be a valid container number (see is_valid_container).
    - "pallets" must be from 1 to 21, both inclusive.
    - "weight_kg" must be greater than 0 and at most 28000.
    - "hours_open" (a float: hours since the event was raised) may be up to 24.
      The event is "overdue" only when it is more than 24 hours, so exactly 24
      hours is still on time.

    Returns the list of error codes; an empty list means the event is valid.
    """
    errors = []
    if not is_valid_container(event.get("container")):
        errors.append("invalid_container")
    pallets = event.get("pallets", 0)
    if pallets < 1 or pallets > 21:
        errors.append("invalid_pallet_count")
    weight_kg = event.get("weight_kg", 0.0)
    if weight_kg <= 0:
        errors.append("weight_missing")
    if weight_kg > 28000:
        errors.append("too_heavy")
    hours_open = event.get("hours_open", 0.0)
    if hours_open > 24:
        errors.append("overdue")
    return errors


def validate_backorder(event):
    """Validate an order line waiting for stock to arrive.

    Rules:
    - "sku" must be a valid SKU code (see is_valid_sku).
    - "quantity" must be a whole number from 1 to 5000, both inclusive.
    - "priority" (default 1) must be from 1 to 5, both inclusive.
    - "hours_open" (a float: hours since the event was raised) must stay below
      168. The event is "overdue" as soon as it reaches 168 hours, so exactly
      168 hours is already overdue.

    Returns the list of error codes; an empty list means the event is valid.
    """
    errors = []
    if not is_valid_sku(event.get("sku")):
        errors.append("invalid_sku")
    quantity = event.get("quantity", 0)
    if quantity < 1:
        errors.append("quantity_too_small")
    if quantity > 5000:
        errors.append("quantity_too_large")
    priority = event.get("priority", 1)
    if priority < 1 or priority > 5:
        errors.append("invalid_priority")
    hours_open = event.get("hours_open", 0.0)
    if hours_open >= 168:
        errors.append("overdue")
    return errors


def validate_cancellation(event):
    """Validate a customer order cancelled before shipping.

    Rules:
    - "reference" must start with "ORD-".
    - "reason" must be one of CANCEL_REASONS.
    - "hours_open" (a float: hours since the event was raised) may be up to 1.
      The event is "overdue" only when it is more than 1 hours, so exactly 1
      hours is still on time.

    Returns the list of error codes; an empty list means the event is valid.
    """
    errors = []
    reference = str(event.get("reference", ""))
    if not reference.startswith("ORD-"):
        errors.append("bad_reference")
    if event.get("reason") not in CANCEL_REASONS:
        errors.append("invalid_reason")
    hours_open = event.get("hours_open", 0.0)
    if hours_open > 1:
        errors.append("overdue")
    return errors


def validate_outbound_asn(event):
    """Validate an advance shipping notice sent to a customer.

    Rules:
    - "reference" must start with "ASN-".
    - "carrier" must be one of KNOWN_CARRIERS.
    - "pallets" must be from 1 to 33, both inclusive.
    - "hours_open" (a float: hours since the event was raised) must stay below
      12. The event is "overdue" as soon as it reaches 12 hours, so exactly 12
      hours is already overdue.

    Returns the list of error codes; an empty list means the event is valid.
    """
    errors = []
    reference = str(event.get("reference", ""))
    if not reference.startswith("ASN-"):
        errors.append("bad_reference")
    if event.get("carrier") not in KNOWN_CARRIERS:
        errors.append("unknown_carrier")
    pallets = event.get("pallets", 0)
    if pallets < 1 or pallets > 33:
        errors.append("invalid_pallet_count")
    hours_open = event.get("hours_open", 0.0)
    if hours_open >= 12:
        errors.append("overdue")
    return errors


def validate_customs_hold(event):
    """Validate an export container held by customs.

    Rules:
    - "container" must be a valid container number (see is_valid_container).
    - "reference" must start with "CUS-".
    - "hours_open" (a float: hours since the event was raised) may be up to 96.
      The event is "overdue" only when it is more than 96 hours, so exactly 96
      hours is still on time.

    Returns the list of error codes; an empty list means the event is valid.
    """
    errors = []
    if not is_valid_container(event.get("container")):
        errors.append("invalid_container")
    reference = str(event.get("reference", ""))
    if not reference.startswith("CUS-"):
        errors.append("bad_reference")
    hours_open = event.get("hours_open", 0.0)
    if hours_open > 96:
        errors.append("overdue")
    return errors


# ---------------------------------------------------------------------------
# Internal moves and stock corrections
# ---------------------------------------------------------------------------


def validate_transfer(event):
    """Validate stock moved between two bins of the same warehouse.

    Rules:
    - "sku" must be a valid SKU code (see is_valid_sku).
    - "quantity" must be a whole number from 1 to 1000, both inclusive.
    - "source" and "destination" must both be valid bin locations and must not
      be the same location.
    - "hours_open" (a float: hours since the event was raised) may be up to 48.
      The event is "overdue" only when it is more than 48 hours, so exactly 48
      hours is still on time.

    Returns the list of error codes; an empty list means the event is valid.
    """
    errors = []
    if not is_valid_sku(event.get("sku")):
        errors.append("invalid_sku")
    quantity = event.get("quantity", 0)
    if quantity < 1:
        errors.append("quantity_too_small")
    if quantity > 1000:
        errors.append("quantity_too_large")
    source = event.get("source")
    destination = event.get("destination")
    if not is_valid_location(source):
        errors.append("invalid_source")
    if not is_valid_location(destination):
        errors.append("invalid_destination")
    if source == destination:
        errors.append("same_location")
    hours_open = event.get("hours_open", 0.0)
    if hours_open > 48:
        errors.append("overdue")
    return errors


def validate_bin_move(event):
    """Validate a whole bin emptied into another bin.

    Rules:
    - "sku" must be a valid SKU code (see is_valid_sku).
    - "quantity" must be a whole number from 1 to 300, both inclusive.
    - "source" and "destination" must both be valid bin locations and must not
      be the same location.

    Returns the list of error codes; an empty list means the event is valid.
    """
    errors = []
    if not is_valid_sku(event.get("sku")):
        errors.append("invalid_sku")
    quantity = event.get("quantity", 0)
    if quantity < 1:
        errors.append("quantity_too_small")
    if quantity > 300:
        errors.append("quantity_too_large")
    source = event.get("source")
    destination = event.get("destination")
    if not is_valid_location(source):
        errors.append("invalid_source")
    if not is_valid_location(destination):
        errors.append("invalid_destination")
    if source == destination:
        errors.append("same_location")
    return errors


def validate_replenishment(event):
    """Validate a pick face refilled from reserve storage.

    Rules:
    - "sku" must be a valid SKU code (see is_valid_sku).
    - "quantity" must be a whole number from 1 to 600, both inclusive.
    - "location" must be a valid bin location (see is_valid_location).
    - "priority" (default 1) must be from 1 to 3, both inclusive.

    Returns the list of error codes; an empty list means the event is valid.
    """
    errors = []
    if not is_valid_sku(event.get("sku")):
        errors.append("invalid_sku")
    quantity = event.get("quantity", 0)
    if quantity < 1:
        errors.append("quantity_too_small")
    if quantity > 600:
        errors.append("quantity_too_large")
    if not is_valid_location(event.get("location")):
        errors.append("invalid_location")
    priority = event.get("priority", 1)
    if priority < 1 or priority > 3:
        errors.append("invalid_priority")
    return errors


def validate_cycle_count(event):
    """Validate a periodic count of a single bin.

    Rules:
    - "sku" must be a valid SKU code (see is_valid_sku).
    - "location" must be a valid bin location (see is_valid_location).
    - "counted" may differ from "expected" by at most 5 percent of "expected";
      a larger difference is "variance_too_high". Nothing is reported when
      "expected" is 0.

    Returns the list of error codes; an empty list means the event is valid.
    """
    errors = []
    if not is_valid_sku(event.get("sku")):
        errors.append("invalid_sku")
    if not is_valid_location(event.get("location")):
        errors.append("invalid_location")
    expected = event.get("expected", 0)
    counted = event.get("counted", 0)
    if expected and abs(counted - expected) * 100 > 5 * expected:
        errors.append("variance_too_high")
    return errors


def validate_adjustment(event):
    """Validate a manual stock adjustment.

    Rules:
    - "sku" must be a valid SKU code (see is_valid_sku).
    - "quantity" must be a whole number from 1 to 10000, both inclusive.
    - "reason" must be one of ADJUSTMENT_REASONS.
    - "reference" must start with "ADJ-".

    Returns the list of error codes; an empty list means the event is valid.
    """
    errors = []
    if not is_valid_sku(event.get("sku")):
        errors.append("invalid_sku")
    quantity = event.get("quantity", 0)
    if quantity < 1:
        errors.append("quantity_too_small")
    if quantity > 10000:
        errors.append("quantity_too_large")
    if event.get("reason") not in ADJUSTMENT_REASONS:
        errors.append("invalid_reason")
    reference = str(event.get("reference", ""))
    if not reference.startswith("ADJ-"):
        errors.append("bad_reference")
    return errors


def validate_kitting(event):
    """Validate components assembled into a kit.

    Rules:
    - "sku" must be a valid SKU code (see is_valid_sku).
    - "quantity" must be a whole number from 1 to 100, both inclusive.
    - "location" must be a valid bin location (see is_valid_location).
    - "hours_open" (a float: hours since the event was raised) may be up to 24.
      The event is "overdue" only when it is more than 24 hours, so exactly 24
      hours is still on time.

    Returns the list of error codes; an empty list means the event is valid.
    """
    errors = []
    if not is_valid_sku(event.get("sku")):
        errors.append("invalid_sku")
    quantity = event.get("quantity", 0)
    if quantity < 1:
        errors.append("quantity_too_small")
    if quantity > 100:
        errors.append("quantity_too_large")
    if not is_valid_location(event.get("location")):
        errors.append("invalid_location")
    hours_open = event.get("hours_open", 0.0)
    if hours_open > 24:
        errors.append("overdue")
    return errors


def validate_dekitting(event):
    """Validate a kit taken apart into its components.

    Rules:
    - "sku" must be a valid SKU code (see is_valid_sku).
    - "quantity" must be a whole number from 1 to 100, both inclusive.
    - "reason" must be one of ADJUSTMENT_REASONS.

    Returns the list of error codes; an empty list means the event is valid.
    """
    errors = []
    if not is_valid_sku(event.get("sku")):
        errors.append("invalid_sku")
    quantity = event.get("quantity", 0)
    if quantity < 1:
        errors.append("quantity_too_small")
    if quantity > 100:
        errors.append("quantity_too_large")
    if event.get("reason") not in ADJUSTMENT_REASONS:
        errors.append("invalid_reason")
    return errors


def validate_relabel(event):
    """Validate items relabelled with a new lot number.

    Rules:
    - "sku" must be a valid SKU code (see is_valid_sku).
    - "lot" must be a valid lot number (see is_valid_lot).
    - "location" must be a valid bin location (see is_valid_location).

    Returns the list of error codes; an empty list means the event is valid.
    """
    errors = []
    if not is_valid_sku(event.get("sku")):
        errors.append("invalid_sku")
    if not is_valid_lot(event.get("lot")):
        errors.append("invalid_lot")
    if not is_valid_location(event.get("location")):
        errors.append("invalid_location")
    return errors


def validate_consolidation(event):
    """Validate partial pallets merged into one.

    Rules:
    - "source" and "destination" must both be valid bin locations and must not
      be the same location.
    - "pallets" must be from 1 to 4, both inclusive.
    - "weight_kg" must be greater than 0 and at most 1200.

    Returns the list of error codes; an empty list means the event is valid.
    """
    errors = []
    source = event.get("source")
    destination = event.get("destination")
    if not is_valid_location(source):
        errors.append("invalid_source")
    if not is_valid_location(destination):
        errors.append("invalid_destination")
    if source == destination:
        errors.append("same_location")
    pallets = event.get("pallets", 0)
    if pallets < 1 or pallets > 4:
        errors.append("invalid_pallet_count")
    weight_kg = event.get("weight_kg", 0.0)
    if weight_kg <= 0:
        errors.append("weight_missing")
    if weight_kg > 1200:
        errors.append("too_heavy")
    return errors


def validate_reservation(event):
    """Validate stock reserved for a specific customer order.

    Rules:
    - "sku" must be a valid SKU code (see is_valid_sku).
    - "quantity" must be a whole number from 1 to 1000, both inclusive.
    - "reference" must start with "ORD-".
    - "hours_open" (a float: hours since the event was raised) may be up to 48.
      The event is "overdue" only when it is more than 48 hours, so exactly 48
      hours is still on time.

    Returns the list of error codes; an empty list means the event is valid.
    """
    errors = []
    if not is_valid_sku(event.get("sku")):
        errors.append("invalid_sku")
    quantity = event.get("quantity", 0)
    if quantity < 1:
        errors.append("quantity_too_small")
    if quantity > 1000:
        errors.append("quantity_too_large")
    reference = str(event.get("reference", ""))
    if not reference.startswith("ORD-"):
        errors.append("bad_reference")
    hours_open = event.get("hours_open", 0.0)
    if hours_open > 48:
        errors.append("overdue")
    return errors


def validate_inventory_snapshot(event):
    """Validate an end-of-day stock snapshot for one bin.

    Rules:
    - "location" must be a valid bin location (see is_valid_location).
    - "counted" may differ from "expected" by at most 2 percent of "expected";
      a larger difference is "variance_too_high". Nothing is reported when
      "expected" is 0.

    Returns the list of error codes; an empty list means the event is valid.
    """
    errors = []
    if not is_valid_location(event.get("location")):
        errors.append("invalid_location")
    expected = event.get("expected", 0)
    counted = event.get("counted", 0)
    if expected and abs(counted - expected) * 100 > 2 * expected:
        errors.append("variance_too_high")
    return errors


# ---------------------------------------------------------------------------
# Quality and compliance
# ---------------------------------------------------------------------------


def validate_quarantine(event):
    """Validate stock blocked for quality reasons.

    Rules:
    - "sku" must be a valid SKU code (see is_valid_sku).
    - "lot" must be a valid lot number (see is_valid_lot).
    - "reason" must be one of HOLD_REASONS.
    - "hours_open" (a float: hours since the event was raised) must stay below
      48. The event is "overdue" as soon as it reaches 48 hours, so exactly 48
      hours is already overdue.

    Returns the list of error codes; an empty list means the event is valid.
    """
    errors = []
    if not is_valid_sku(event.get("sku")):
        errors.append("invalid_sku")
    if not is_valid_lot(event.get("lot")):
        errors.append("invalid_lot")
    if event.get("reason") not in HOLD_REASONS:
        errors.append("invalid_reason")
    hours_open = event.get("hours_open", 0.0)
    if hours_open >= 48:
        errors.append("overdue")
    return errors


def validate_quarantine_release(event):
    """Validate stock released from quarantine.

    Rules:
    - "sku" must be a valid SKU code (see is_valid_sku).
    - "lot" must be a valid lot number (see is_valid_lot).
    - "reference" must start with "QAR-".
    - "hours_open" (a float: hours since the event was raised) may be up to 72.
      The event is "overdue" only when it is more than 72 hours, so exactly 72
      hours is still on time.

    Returns the list of error codes; an empty list means the event is valid.
    """
    errors = []
    if not is_valid_sku(event.get("sku")):
        errors.append("invalid_sku")
    if not is_valid_lot(event.get("lot")):
        errors.append("invalid_lot")
    reference = str(event.get("reference", ""))
    if not reference.startswith("QAR-"):
        errors.append("bad_reference")
    hours_open = event.get("hours_open", 0.0)
    if hours_open > 72:
        errors.append("overdue")
    return errors


def validate_lot_recall(event):
    """Validate a lot recalled by the supplier or a regulator.

    Rules:
    - "lot" must be a valid lot number (see is_valid_lot).
    - "reason" must be one of RECALL_REASONS.
    - "priority" (default 1) must be from 1 to 2, both inclusive.

    Returns the list of error codes; an empty list means the event is valid.
    """
    errors = []
    if not is_valid_lot(event.get("lot")):
        errors.append("invalid_lot")
    if event.get("reason") not in RECALL_REASONS:
        errors.append("invalid_reason")
    priority = event.get("priority", 1)
    if priority < 1 or priority > 2:
        errors.append("invalid_priority")
    return errors


def validate_cold_chain_check(event):
    """Validate a temperature reading in the chilled zone.

    Rules:
    - "sku" must be a valid SKU code (see is_valid_sku).
    - "temp_c" must be present and between 2 and 8 degrees Celsius, both
      inclusive.
    - "hours_open" (a float: hours since the event was raised) may be up to 2.
      The event is "overdue" only when it is more than 2 hours, so exactly 2
      hours is still on time.

    Returns the list of error codes; an empty list means the event is valid.
    """
    errors = []
    if not is_valid_sku(event.get("sku")):
        errors.append("invalid_sku")
    temp_c = event.get("temp_c")
    if temp_c is None or not 2 <= temp_c <= 8:
        errors.append("temperature_out_of_range")
    hours_open = event.get("hours_open", 0.0)
    if hours_open > 2:
        errors.append("overdue")
    return errors


def validate_frozen_check(event):
    """Validate a temperature reading in the frozen zone.

    Rules:
    - "temp_c" must be present and between -25 and -18 degrees Celsius, both
      inclusive.
    - "location" must be a valid bin location (see is_valid_location).

    Returns the list of error codes; an empty list means the event is valid.
    """
    errors = []
    temp_c = event.get("temp_c")
    if temp_c is None or not -25 <= temp_c <= -18:
        errors.append("temperature_out_of_range")
    if not is_valid_location(event.get("location")):
        errors.append("invalid_location")
    return errors


def validate_hazmat_check(event):
    """Validate a dangerous goods declaration check.

    Rules:
    - "sku" must be a valid SKU code (see is_valid_sku).
    - "reference" must start with "UN".
    - "quantity" must be a whole number from 1 to 50, both inclusive.
    - "weight_kg" must be greater than 0 and at most 400.

    Returns the list of error codes; an empty list means the event is valid.
    """
    errors = []
    if not is_valid_sku(event.get("sku")):
        errors.append("invalid_sku")
    reference = str(event.get("reference", ""))
    if not reference.startswith("UN"):
        errors.append("bad_reference")
    quantity = event.get("quantity", 0)
    if quantity < 1:
        errors.append("quantity_too_small")
    if quantity > 50:
        errors.append("quantity_too_large")
    weight_kg = event.get("weight_kg", 0.0)
    if weight_kg <= 0:
        errors.append("weight_missing")
    if weight_kg > 400:
        errors.append("too_heavy")
    return errors


def validate_damage_report(event):
    """Validate damaged stock found during handling.

    Rules:
    - "sku" must be a valid SKU code (see is_valid_sku).
    - "quantity" must be a whole number from 1 to 1000, both inclusive.
    - "reason" must be one of DAMAGE_REASONS.

    Returns the list of error codes; an empty list means the event is valid.
    """
    errors = []
    if not is_valid_sku(event.get("sku")):
        errors.append("invalid_sku")
    quantity = event.get("quantity", 0)
    if quantity < 1:
        errors.append("quantity_too_small")
    if quantity > 1000:
        errors.append("quantity_too_large")
    if event.get("reason") not in DAMAGE_REASONS:
        errors.append("invalid_reason")
    return errors


def validate_scrap(event):
    """Validate stock written off and sent for disposal.

    Rules:
    - "sku" must be a valid SKU code (see is_valid_sku).
    - "quantity" must be a whole number from 1 to 1000, both inclusive.
    - "reason" must be one of SCRAP_REASONS.
    - "reference" must start with "SCR-".

    Returns the list of error codes; an empty list means the event is valid.
    """
    errors = []
    if not is_valid_sku(event.get("sku")):
        errors.append("invalid_sku")
    quantity = event.get("quantity", 0)
    if quantity < 1:
        errors.append("quantity_too_small")
    if quantity > 1000:
        errors.append("quantity_too_large")
    if event.get("reason") not in SCRAP_REASONS:
        errors.append("invalid_reason")
    reference = str(event.get("reference", ""))
    if not reference.startswith("SCR-"):
        errors.append("bad_reference")
    return errors


def validate_rma_inspection(event):
    """Validate an inspection of a returned item.

    Rules:
    - "sku" must be a valid SKU code (see is_valid_sku).
    - "reference" must start with "RMA-".
    - "hours_open" (a float: hours since the event was raised) may be up to 72.
      The event is "overdue" only when it is more than 72 hours, so exactly 72
      hours is still on time.

    Returns the list of error codes; an empty list means the event is valid.
    """
    errors = []
    if not is_valid_sku(event.get("sku")):
        errors.append("invalid_sku")
    reference = str(event.get("reference", ""))
    if not reference.startswith("RMA-"):
        errors.append("bad_reference")
    hours_open = event.get("hours_open", 0.0)
    if hours_open > 72:
        errors.append("overdue")
    return errors


def validate_temperature_excursion(event):
    """Validate a temperature alarm raised by a sensor.

    Rules:
    - "location" must be a valid bin location (see is_valid_location).
    - "temp_c" must be present and between -30 and 30 degrees Celsius, both
      inclusive.
    - "hours_open" (a float: hours since the event was raised) must stay below
      0.5. The event is "overdue" as soon as it reaches 0.5 hours, so exactly
      0.5 hours is already overdue.

    Returns the list of error codes; an empty list means the event is valid.
    """
    errors = []
    if not is_valid_location(event.get("location")):
        errors.append("invalid_location")
    temp_c = event.get("temp_c")
    if temp_c is None or not -30 <= temp_c <= 30:
        errors.append("temperature_out_of_range")
    hours_open = event.get("hours_open", 0.0)
    if hours_open >= 0.5:
        errors.append("overdue")
    return errors


# ---------------------------------------------------------------------------
# Dispatch
# ---------------------------------------------------------------------------


VALIDATORS = {
    "receipt": validate_receipt,
    "inbound_asn": validate_inbound_asn,
    "putaway": validate_putaway,
    "vendor_delivery": validate_vendor_delivery,
    "dock_appointment": validate_dock_appointment,
    "cross_dock": validate_cross_dock,
    "inbound_hold": validate_inbound_hold,
    "receipt_correction": validate_receipt_correction,
    "customer_return": validate_customer_return,
    "unload": validate_unload,
    "order_release": validate_order_release,
    "wave_release": validate_wave_release,
    "pick": validate_pick,
    "pack": validate_pack,
    "pallet_build": validate_pallet_build,
    "shipment": validate_shipment,
    "carrier_pickup": validate_carrier_pickup,
    "container_load": validate_container_load,
    "backorder": validate_backorder,
    "cancellation": validate_cancellation,
    "outbound_asn": validate_outbound_asn,
    "customs_hold": validate_customs_hold,
    "transfer": validate_transfer,
    "bin_move": validate_bin_move,
    "replenishment": validate_replenishment,
    "cycle_count": validate_cycle_count,
    "adjustment": validate_adjustment,
    "kitting": validate_kitting,
    "dekitting": validate_dekitting,
    "relabel": validate_relabel,
    "consolidation": validate_consolidation,
    "reservation": validate_reservation,
    "inventory_snapshot": validate_inventory_snapshot,
    "quarantine": validate_quarantine,
    "quarantine_release": validate_quarantine_release,
    "lot_recall": validate_lot_recall,
    "cold_chain_check": validate_cold_chain_check,
    "frozen_check": validate_frozen_check,
    "hazmat_check": validate_hazmat_check,
    "damage_report": validate_damage_report,
    "scrap": validate_scrap,
    "rma_inspection": validate_rma_inspection,
    "temperature_excursion": validate_temperature_excursion,
}


def validate_event(event):
    """Dispatch `event` to the validator named by its "kind" field.

    Unknown kinds produce ["unknown_kind"]. The "kind" field itself is
    not passed on to the validator's error list.
    """
    validator = VALIDATORS.get(event.get("kind"))
    if validator is None:
        return ["unknown_kind"]
    return validator(event)


def validate_batch(events):
    """Validate many events and return {index: errors} for bad ones.

    Events without errors are left out of the result, so an empty dict
    means the whole batch is valid.
    """
    problems = {}
    for index, event in enumerate(events):
        errors = validate_event(event)
        if errors:
            problems[index] = errors
    return problems
