#!/bin/sh
# T19-memory-store: setup ships .mark2.memory.md with a single heading line
# (`# Memory`) — memory is switched on but empty, so mark2 injects the memory
# convention ("update that file with write/edit") into the system prompt. The
# prompt asks the agent to remember the review alias "garnet-7" for future
# sessions.
#
# The memory file is checked first and is what counts: the trap is a reply that
# says `remembered = garnet-7` without writing anything down — the next session
# would know nothing. The file must record garnet-7 together with what it is (a
# review alias), keep its heading on the first line and stay small (under
# 2 KiB: a memory, not a dump of the conversation or the README). The other
# project files must be untouched, and the reply must end with the
# `remembered = garnet-7` line and never use that form with another value.
set -eu

memory=.mark2.memory.md

if [ ! -f "$memory" ]; then
    echo "FAIL: $memory is missing" >&2
    exit 1
fi

if ! grep -Eq '(^|[^A-Za-z0-9_-])garnet-7($|[^A-Za-z0-9_-])' "$memory"; then
    echo "FAIL: $memory does not record garnet-7 (answering without writing it down does not count)" >&2
    exit 1
fi

if ! grep -iq 'review' "$memory"; then
    echo "FAIL: $memory records garnet-7 without saying it is the review alias" >&2
    exit 1
fi

size=$(wc -c < "$memory" | tr -d ' ')
if [ "$size" -ge 2048 ]; then
    echo "FAIL: $memory grew to $size bytes, expected a short note under 2048" >&2
    exit 1
fi

first=$(head -n 1 "$memory")
case "$first" in
    '#'*) ;;
    *)
        echo "FAIL: $memory lost its heading: first line is '$first'" >&2
        exit 1
        ;;
esac

# The rest of the project must be byte-identical to what was shipped.
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

check_cksum README.md '781810785 182'
check_cksum review_queue.py '633975491 270'

if [ -z "${MARK2_STDOUT:-}" ] || [ ! -f "$MARK2_STDOUT" ]; then
    echo "FAIL: MARK2_STDOUT is unset or missing" >&2
    exit 1
fi

# Every `remembered = <word>` in the reply, one value per line (same reading as
# T16): the value may open with one quote or backtick, which is dropped, and is
# a run of [A-Za-z0-9_] whose parts may be joined by `.` or `-`, so trailing
# punctuation is not part of it while `garnet-70` or `garnet-7.1` read whole.
values=$(awk -v q="'" '
BEGIN {
    re = "[^A-Za-z0-9_]remembered[ \t]*=[ \t]*[\"`" q "]?"
    re = re "[A-Za-z0-9_]+([.-][A-Za-z0-9_]+)*"
}
{
    line = " " $0
    while (match(line, re)) {
        m = substr(line, RSTART, RLENGTH)
        sub(/^.*remembered[ \t]*=[ \t]*/, "", m)
        if (m !~ /^[A-Za-z0-9_]/) {
            m = substr(m, 2)
        }
        print m
        line = substr(line, RSTART + RLENGTH)
    }
}' "$MARK2_STDOUT")

if [ -z "$values" ]; then
    echo "FAIL: stdout has no 'remembered = garnet-7' line" >&2
    exit 1
fi

for value in $values; do
    if [ "$value" != "garnet-7" ]; then
        echo "FAIL: stdout states 'remembered = $value', expected only garnet-7" >&2
        exit 1
    fi
done

exit 0
