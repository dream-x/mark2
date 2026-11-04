#!/bin/sh
# T18-memory-recall: the staging deploy port (7342) exists in exactly one place —
# the agent memory file .mark2.memory.md that mark2 injects into the system
# prompt at session start. Nothing else in the tree has it: deploy.ini holds the
# PRODUCTION port 8080, app.py and README.md mention the local default 8000,
# and scripts/deploy.sh takes the staging port from CI ($STAGING_PORT). A model
# that ignores its memory and greps the repo finds only decoys, so the honest
# answers without memory are "not in the repo" or a wrong port, never 7342.
# The reply must state `staging_port = 7342` and never the form with any other
# value, and the working tree — memory file included — must be exactly as
# shipped: it is a recall task, nothing new is worth remembering.
set -eu

if [ -z "${MARK2_STDOUT:-}" ] || [ ! -f "$MARK2_STDOUT" ]; then
    echo "FAIL: MARK2_STDOUT is unset or missing" >&2
    exit 1
fi

# Every `staging_port = <number>` in the reply, one value per line. A leading
# space is prepended so the identifier boundary check also works at column 0
# (and `prod_staging_port = ...` would not count).
values=$(awk '{
    line = " " $0
    while (match(line, /[^A-Za-z0-9_]staging_port[ \t]*=[ \t]*[0-9]+(\.[0-9]+)?/)) {
        m = substr(line, RSTART, RLENGTH)
        sub(/^.*staging_port[ \t]*=[ \t]*/, "", m)
        print m
        line = substr(line, RSTART + RLENGTH)
    }
}' "$MARK2_STDOUT")

if [ -z "$values" ]; then
    echo "FAIL: stdout has no 'staging_port = <number>' answer line" >&2
    exit 1
fi

# An answer that also asserts a decoy (or any other value) is ambiguous.
for value in $values; do
    if [ "$value" != "7342" ]; then
        echo "FAIL: stdout states 'staging_port = $value', expected only 7342" >&2
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

check_cksum .mark2.memory.md '1550927582 217'
check_cksum README.md '2433068155 499'
check_cksum app.py '2896570756 488'
check_cksum deploy.ini '3597280844 153'
check_cksum scripts/deploy.sh '2825108476 489'

# No new files. Exceptions that are not the agent's doing: the runner's
# ./.mark2.toml, bytecode caches left by running the code with python3, and
# pytest's cache directory.
want_files='./.mark2.memory.md
./README.md
./app.py
./deploy.ini
./scripts/deploy.sh'
got_files=$(find . -type f ! -path ./.mark2.toml ! -path '*/__pycache__/*' \
    ! -path './.pytest_cache/*' | LC_ALL=C sort)
if [ "$got_files" != "$want_files" ]; then
    echo "FAIL: the set of files changed; found:" >&2
    echo "$got_files" >&2
    exit 1
fi

exit 0
