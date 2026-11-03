#!/bin/sh
# T15-long-context: six meeting minutes (~58 KB together) under an 8192-token
# context limit, so the harness has to compact mid-task and the figures read
# early must survive the summary. Approved FY2027 budgets, last decision wins:
#   Engineering        4,350,000 (s01 request) -> 4,105,000 (s03 committee)
#                      -> 4,179,800 (s06: the board adds the 74,800 audit
#                      to the s03 figure; the sum itself is never written)
#   Marketing          2,640,000 (s02 request) -> 2,400,000 (s02 working
#                      figure, not an approval) -> 2,275,000 (s05)
#   Sales              2,060,000 -> 1,945,000 (s02); s04 transfer defeated
#   Customer Support   1,040,000 (s03 accepted) -> 1,114,900 (s04)
#   Facilities         812,000 -> 694,000 (s01); s05 reopen refused
#   People Operations  1,150,000 (s01 placeholder) -> 1,244,700 (s04 request)
#                      -> 1,204,700 (s04)
# Total 11,413,400. Sum of the first FY2027 figures is 12,052,000. The reply
# must state `total_budget = 11413400`, never the form with any other value,
# and the working tree must be exactly as shipped.
set -eu

if [ -z "${MARK2_STDOUT:-}" ] || [ ! -f "$MARK2_STDOUT" ]; then
    echo "FAIL: MARK2_STDOUT is unset or missing" >&2
    exit 1
fi

values_file=$(mktemp "${TMPDIR:-/tmp}/t15-values.XXXXXX")
trap 'rm -f "$values_file"' EXIT
trap 'exit 1' HUP INT TERM

# Every `total_budget = <number>` in the reply, one value per line. A leading
# space is prepended so the identifier boundary check also works at column 0.
# Digit groups joined by `,` `.` or `_` are captured whole, so `11,413,400`
# is reported as written (and rejected), while a sentence-ending period after
# the number is not part of the value.
awk '{
    line = " " $0
    while (match(line, /[^A-Za-z0-9_]total_budget[ \t]*=[ \t]*[0-9]+([.,_][0-9]+)*/)) {
        m = substr(line, RSTART, RLENGTH)
        sub(/^.*total_budget[ \t]*=[ \t]*/, "", m)
        print m
        line = substr(line, RSTART + RLENGTH)
    }
}' "$MARK2_STDOUT" > "$values_file"

if [ ! -s "$values_file" ]; then
    echo "FAIL: stdout has no 'total_budget = <value>' answer line" >&2
    exit 1
fi

# An answer that also asserts a stale total (or any other value) is ambiguous.
while IFS= read -r value; do
    if [ "$value" != "11413400" ]; then
        echo "FAIL: stdout states 'total_budget = $value', expected only 11413400" >&2
        exit 1
    fi
done < "$values_file"

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

check_cksum minutes/shard-01.md '2071764067 10883'
check_cksum minutes/shard-02.md '4036756784 9462'
check_cksum minutes/shard-03.md '1884006068 9413'
check_cksum minutes/shard-04.md '1706805230 9508'
check_cksum minutes/shard-05.md '40710219 9252'
check_cksum minutes/shard-06.md '3342418282 9508'

# No new files: notes or helper scripts written to survive compaction count
# as a violation. Exceptions that are not the agent's doing: the runner's
# ./.mark2.toml, bytecode caches left by `python3 -c`, and pytest's cache.
want_files='./minutes/shard-01.md
./minutes/shard-02.md
./minutes/shard-03.md
./minutes/shard-04.md
./minutes/shard-05.md
./minutes/shard-06.md'
got_files=$(find . -type f ! -path ./.mark2.toml ! -path '*/__pycache__/*' \
    ! -path './.pytest_cache/*' | LC_ALL=C sort)
if [ "$got_files" != "$want_files" ]; then
    echo "FAIL: the set of files changed; found:" >&2
    echo "$got_files" >&2
    exit 1
fi

exit 0
