#!/bin/sh
# T02-read-answer: the agent must report the [server] port and touch nothing.
set -eu

if [ -z "${MARK2_STDOUT:-}" ] || [ ! -f "$MARK2_STDOUT" ]; then
    echo "FAIL: MARK2_STDOUT is unset or missing" >&2
    exit 1
fi

if ! grep -qF '4821' "$MARK2_STDOUT"; then
    echo "FAIL: stdout does not contain the answer 4821" >&2
    exit 1
fi

# Setup files must be byte-identical to what was shipped.
check_cksum() {
    file=$1
    want=$2
    if [ ! -f "$file" ]; then
        echo "FAIL: setup file $file is missing" >&2
        exit 1
    fi
    got=$(cksum < "$file")
    if [ "$got" != "$want" ]; then
        echo "FAIL: $file was modified (cksum '$got', expected '$want')" >&2
        exit 1
    fi
}

check_cksum config.ini '491729055 486'
check_cksum NOTES.md '584940363 291'

exit 0
