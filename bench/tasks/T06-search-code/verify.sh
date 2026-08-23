#!/bin/sh
# T06-search-code: the answer is `checkout_total = 64.8`.
set -eu

if [ -z "${MARK2_STDOUT:-}" ] || [ ! -f "$MARK2_STDOUT" ]; then
    echo "FAIL: MARK2_STDOUT is unset or missing" >&2
    exit 1
fi

if ! grep -qF 'checkout_total' "$MARK2_STDOUT"; then
    echo "FAIL: stdout does not name checkout_total" >&2
    exit 1
fi

# Accept 64.8 and 64.80.
if ! grep -qF '64.8' "$MARK2_STDOUT"; then
    echo "FAIL: stdout does not contain the value 64.8" >&2
    exit 1
fi

# The source tree must still be intact.
for f in main.py orders.py pricing.py discounts.py tax.py util.py; do
    if [ ! -f "src/$f" ]; then
        echo "FAIL: src/$f is missing" >&2
        exit 1
    fi
done

exit 0
