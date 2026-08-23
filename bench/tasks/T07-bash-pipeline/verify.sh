#!/bin/sh
# T07-bash-pipeline: report.txt must contain exactly the number 17.
set -eu

if [ ! -f report.txt ]; then
    echo "FAIL: report.txt does not exist" >&2
    exit 1
fi

# Tolerate surrounding whitespace / trailing newline, nothing else.
got=$(tr -d ' \t\r\n' < report.txt)
if [ "$got" != "17" ]; then
    echo "FAIL: report.txt holds '$got', expected '17'" >&2
    echo "--- report.txt ---" >&2
    cat report.txt >&2
    exit 1
fi

# The source tree must not have been edited to make the count come out right.
count=$(grep -r TODO src | wc -l | tr -d ' ')
if [ "$count" != "17" ]; then
    echo "FAIL: src/ was modified, it now has $count TODO lines instead of 17" >&2
    exit 1
fi

exit 0
