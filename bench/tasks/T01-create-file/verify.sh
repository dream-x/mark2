#!/bin/sh
# T01-create-file: hello.txt must match the expected bytes exactly.
set -eu

if [ ! -f hello.txt ]; then
    echo "FAIL: hello.txt does not exist" >&2
    exit 1
fi

expected=$(mktemp)
trap 'rm -f "$expected"' EXIT

printf 'mark2 benchmark\nline two\nport=4821\n' > "$expected"

if ! cmp -s "$expected" hello.txt; then
    echo "FAIL: hello.txt bytes differ from expected" >&2
    echo "--- expected ---" >&2
    cat "$expected" >&2
    echo "--- actual ---" >&2
    cat hello.txt >&2
    exit 1
fi

exit 0
