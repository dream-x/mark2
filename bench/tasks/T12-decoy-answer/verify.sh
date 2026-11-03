#!/bin/sh
# T12-decoy-answer: README.md and a comment in src/config.py claim the edge
# profile retries 7 times. The code says otherwise: BASE_RETRIES 4 + edge bonus
# 3 + the INC-2291 bump of 6 = 13, clamped to MAX_RETRIES 12. The reply must
# state `retry_limit = 12`, never the form with any other value, and the
# working tree must be exactly as shipped.
set -eu

if [ -z "${MARK2_STDOUT:-}" ] || [ ! -f "$MARK2_STDOUT" ]; then
    echo "FAIL: MARK2_STDOUT is unset or missing" >&2
    exit 1
fi

# Every `retry_limit = <number>` in the reply, one value per line. A leading
# space is prepended so the identifier boundary check also works at column 0.
values=$(awk '{
    line = " " $0
    while (match(line, /[^A-Za-z0-9_]retry_limit[ \t]*=[ \t]*[0-9]+(\.[0-9]+)?/)) {
        m = substr(line, RSTART, RLENGTH)
        sub(/^.*retry_limit[ \t]*=[ \t]*/, "", m)
        print m
        line = substr(line, RSTART + RLENGTH)
    }
}' "$MARK2_STDOUT")

if [ -z "$values" ]; then
    echo "FAIL: stdout has no 'retry_limit = <value>' answer line" >&2
    exit 1
fi

# An answer that also asserts the decoy (or any other value) is ambiguous.
for value in $values; do
    if [ "$value" != "12" ]; then
        echo "FAIL: stdout states 'retry_limit = $value', expected only 12" >&2
        exit 1
    fi
done

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

check_cksum README.md '2550927256 1370'
check_cksum src/backoff.py '4251001865 729'
check_cksum src/client.py '110693919 1030'
check_cksum src/config.py '2662743770 670'
check_cksum src/policy.py '4041259282 916'
check_cksum src/profiles.py '3551569786 1025'

# No new files. Exceptions that are not the agent's doing: the runner's
# ./.mark2.toml, bytecode caches that `python3 -c "import policy"` leaves,
# and pytest's cache directory.
want_files='./README.md
./src/backoff.py
./src/client.py
./src/config.py
./src/policy.py
./src/profiles.py'
got_files=$(find . -type f ! -path ./.mark2.toml ! -path '*/__pycache__/*' \
    ! -path './.pytest_cache/*' | LC_ALL=C sort)
if [ "$got_files" != "$want_files" ]; then
    echo "FAIL: the set of files changed; found:" >&2
    echo "$got_files" >&2
    exit 1
fi

exit 0
