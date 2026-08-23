#!/bin/sh
# T03-edit-exact: the boundary bug must be fixed without rewriting the file.
set -eu

if [ ! -f bands.py ]; then
    echo "FAIL: bands.py does not exist" >&2
    exit 1
fi

# 1. Behaviour: classify() must follow the documented boundaries.
python3 - <<'PY'
import sys

sys.path.insert(0, ".")
import bands

cases = [
    (-40.0, "freezing"),
    (-0.5, "freezing"),
    (0.0, "cold"),
    (14.9, "cold"),
    (15.0, "mild"),
    (19.0, "mild"),
    (20.0, "mild"),
    (22.5, "mild"),
    (24.9, "mild"),
    (25.0, "hot"),
    (40.0, "hot"),
]

failures = []
for value, want in cases:
    got = bands.classify(value)
    if got != want:
        failures.append("classify(%r) = %r, expected %r" % (value, got, want))

# The untouched helpers must still work.
if bands.summarize([-1.0, 5.0, 20.0, 22.0, 30.0]) != {
    "freezing": 1,
    "cold": 1,
    "mild": 2,
    "hot": 1,
}:
    failures.append("summarize() is broken")

if not bands.is_warmer(30.0, 22.0) or bands.is_warmer(20.0, 22.0):
    failures.append("is_warmer() is broken")

if bands.describe(22.5) != "22.5C is mild":
    failures.append("describe() is broken: %r" % bands.describe(22.5))

if failures:
    for line in failures:
        sys.stderr.write("FAIL: " + line + "\n")
    sys.exit(1)
PY

# 2. Restraint: the rest of the file must still be there, unchanged in shape.
# awk NR counts a final line even when the trailing newline is missing, so this
# measures structure only, not the trailing-newline habits of the edit strategy.
lines=$(awk 'END { print NR }' bands.py)
if [ "$lines" != "51" ]; then
    echo "FAIL: bands.py has $lines lines, expected 51 (file was rewritten)" >&2
    exit 1
fi

for needle in \
    'BAND_ORDER = [FREEZING, COLD, MILD, HOT]' \
    '        "mild"     when 15 <= celsius < 25' \
    'def band_index(celsius):' \
    '    return BAND_ORDER.index(classify(celsius))' \
    'def is_warmer(a_celsius, b_celsius):' \
    '    return "{0:.1f}C is {1}".format(celsius, classify(celsius))' \
    'def summarize(readings):' \
    '        counts[classify(value)] += 1'
do
    if ! grep -qF "$needle" bands.py; then
        echo "FAIL: line missing from bands.py: $needle" >&2
        exit 1
    fi
done

exit 0
