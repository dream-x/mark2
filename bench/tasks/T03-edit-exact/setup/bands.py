"""Temperature band helpers used by the weather report renderer."""

FREEZING = "freezing"
COLD = "cold"
MILD = "mild"
HOT = "hot"

BAND_ORDER = [FREEZING, COLD, MILD, HOT]


def classify(celsius):
    """Return the band name for a temperature in degrees Celsius.

    The bands are:
        "freezing" when celsius < 0
        "cold"     when 0 <= celsius < 15
        "mild"     when 15 <= celsius < 25
        "hot"      when celsius >= 25
    """
    if celsius < 0:
        return FREEZING
    if celsius < 15:
        return COLD
    if celsius < 20:
        return MILD
    return HOT


def band_index(celsius):
    """Return the position of the band for `celsius` inside BAND_ORDER."""
    return BAND_ORDER.index(classify(celsius))


def is_warmer(a_celsius, b_celsius):
    """Return True when `a_celsius` falls into a strictly warmer band."""
    return band_index(a_celsius) > band_index(b_celsius)


def describe(celsius):
    """Return a short human readable description of a temperature."""
    return "{0:.1f}C is {1}".format(celsius, classify(celsius))


def summarize(readings):
    """Return a dict mapping each band name to how often it occurs."""
    counts = {}
    for name in BAND_ORDER:
        counts[name] = 0
    for value in readings:
        counts[classify(value)] += 1
    return counts
