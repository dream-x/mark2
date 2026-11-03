#!/bin/sh
# T11-cascade-bugs: python3 test.py must pass, with test.py untouched. The
# suite stops at the first failure, so the three independent bugs in
# textkit.py only show up one at a time.
set -eu

if [ ! -f test.py ]; then
    echo "FAIL: test.py is missing" >&2
    exit 1
fi

got=$(cksum < test.py)
want='893536267 3029'
if [ "$got" != "$want" ]; then
    echo "FAIL: test.py was modified (cksum '$got', expected '$want')" >&2
    exit 1
fi

if [ ! -f textkit.py ]; then
    echo "FAIL: textkit.py is missing" >&2
    exit 1
fi

if ! python3 test.py; then
    echo "FAIL: python3 test.py did not pass" >&2
    exit 1
fi

exit 0
