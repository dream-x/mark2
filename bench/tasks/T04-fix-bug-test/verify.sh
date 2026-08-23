#!/bin/sh
# T04-fix-bug-test: python3 test.py must pass, with test.py untouched.
set -eu

if [ ! -f test.py ]; then
    echo "FAIL: test.py is missing" >&2
    exit 1
fi

got=$(cksum < test.py)
want='1768848165 714'
if [ "$got" != "$want" ]; then
    echo "FAIL: test.py was modified (cksum '$got', expected '$want')" >&2
    exit 1
fi

if [ ! -f stats.py ]; then
    echo "FAIL: stats.py is missing" >&2
    exit 1
fi

if ! python3 test.py; then
    echo "FAIL: python3 test.py did not pass" >&2
    exit 1
fi

exit 0
