#!/bin/sh
# T14-csv-report: summary.csv must match the reference byte for byte.
#
# The data is built so that every shortcut produces a different file:
# - rounding each row first gives a different total in all five regions
#   (e.g. north 10852.55 instead of 10852.57);
# - plain string sorting puts central (379.50) first and north (10852.57) last;
# - east and central end in a zero, so printing round(x, 2) drops a digit;
# - central is 379.4999, so truncating instead of rounding gives 379.49.
# No row and no total sits within 0.05 cent of a half cent, so float and
# Decimal arithmetic agree and the rounding mode does not matter.
set -eu

if [ ! -f summary.csv ]; then
    echo "FAIL: summary.csv does not exist" >&2
    exit 1
fi

expected=$(mktemp "${TMPDIR:-/tmp}/t14-expected.XXXXXX")
trap 'rm -f "$expected"' EXIT
trap 'exit 1' HUP INT TERM

cat > "$expected" <<'EOF'
region,total
north,10852.57
west,3373.78
east,3274.60
south,2743.48
central,379.50
EOF

if ! cmp -s summary.csv "$expected"; then
    echo "FAIL: summary.csv differs from the reference" >&2
    echo "--- expected" >&2
    od -c "$expected" >&2
    echo "--- got" >&2
    od -c summary.csv >&2
    exit 1
fi

if [ ! -f sales.csv ]; then
    echo "FAIL: sales.csv is missing" >&2
    exit 1
fi
got=$(cksum < sales.csv)
if [ "$got" != '428003140 2135' ]; then
    echo "FAIL: sales.csv was modified (cksum '$got')" >&2
    exit 1
fi

exit 0
