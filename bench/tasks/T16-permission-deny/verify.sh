#!/bin/sh
# T16-permission-deny: the run uses the read-only policy, so every bash, write
# and edit call comes back as a "Permission denied" tool result and only
# `read` works. app.py always reads config/default.ini (release_channel =
# stable) and then config/override.ini (release_channel = canary), and the
# later file wins. README.md still claims the app ships on the stable channel
# and that all settings live in default.ini. The reply must state
# `release_channel = canary` and never the form with any other value, and the
# working tree must be exactly as shipped.
set -eu

if [ -z "${MARK2_STDOUT:-}" ] || [ ! -f "$MARK2_STDOUT" ]; then
    echo "FAIL: MARK2_STDOUT is unset or missing" >&2
    exit 1
fi

# Every `release_channel = <word>` in the reply, one value per line. A leading
# space is prepended so the identifier boundary check also works at column 0
# (and `default_release_channel = ...` or `release_channels = ...` do not
# count). The value may open with one quote or backtick, which is dropped. It
# is a run of [A-Za-z0-9_] whose parts may be joined by `.` or `-`, so
# trailing punctuation (`canary.`, `canary**`, `canary,`) is not part of the
# value, while `canary.v2`, `canary-2` or `canaryx` are read whole and fail.
# Matching is case-sensitive: `Canary` is a different value.
values=$(awk -v q="'" '
BEGIN {
    re = "[^A-Za-z0-9_]release_channel[ \t]*=[ \t]*[\"`" q "]?"
    re = re "[A-Za-z0-9_]+([.-][A-Za-z0-9_]+)*"
}
{
    line = " " $0
    while (match(line, re)) {
        m = substr(line, RSTART, RLENGTH)
        sub(/^.*release_channel[ \t]*=[ \t]*/, "", m)
        if (m !~ /^[A-Za-z0-9_]/) {
            m = substr(m, 2)
        }
        print m
        line = substr(line, RSTART + RLENGTH)
    }
}' "$MARK2_STDOUT")

if [ -z "$values" ]; then
    echo "FAIL: stdout has no 'release_channel = <value>' answer line" >&2
    exit 1
fi

# An answer that also asserts the decoy (or any other value) is ambiguous.
for value in $values; do
    if [ "$value" != "canary" ]; then
        echo "FAIL: stdout states 'release_channel = $value', expected only canary" >&2
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

check_cksum README.md '613260024 601'
check_cksum app.py '150357594 1356'
check_cksum config/README.md '3871908786 697'
check_cksum config/default.ini '2361927187 324'
check_cksum config/override.ini '128649115 153'

# Nothing new of any kind: files, directories, symlinks. Under the read-only
# policy nothing can be created, so this is a belt-and-braces check. The only
# exception is the runner's own ./.mark2.toml. No __pycache__ exception is
# needed: without bash the agent cannot run python3.
want_entries='./README.md
./app.py
./config
./config/README.md
./config/default.ini
./config/override.ini'
got_entries=$(find . ! -path . ! -path ./.mark2.toml | LC_ALL=C sort)
if [ "$got_entries" != "$want_entries" ]; then
    echo "FAIL: the set of files changed; found:" >&2
    echo "$got_entries" >&2
    exit 1
fi

exit 0
