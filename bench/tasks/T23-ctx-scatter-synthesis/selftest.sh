#!/bin/sh
# Self-test of T23's verify.sh on prepared sandboxes, no model involved:
#
#   sh bench/tasks/T23-ctx-scatter-synthesis/selftest.sh
#
# Every case is a fresh copy of setup/ plus what an agent might leave behind;
# verify.sh must PASS the golden answers and FAIL everything else. Rerun after
# any change to gen.py, setup/ or verify.sh.
set -eu

here=$(CDPATH='' cd -- "$(dirname -- "$0")" && pwd)
python3 "$here/gen.py" --check

work=$(mktemp -d "${TMPDIR:-/tmp}/t23-selftest.XXXXXX")
trap 'rm -rf "$work"' EXIT
case_dir=$work/case
mismatches=0

fresh() {
    rm -rf "$case_dir"
    mkdir "$case_dir"
    cp -R "$here/setup/." "$case_dir/"
}

answer() {
    printf "$1" > "$case_dir/answer.txt"
}

expect() {
    want=$1 name=$2
    if (cd "$case_dir" && sh "$here/verify.sh") >"$work/out" 2>&1; then got=PASS; else got=FAIL; fi
    if [ "$got" = "$want" ]; then mark=ok; else mark=MISMATCH; mismatches=$((mismatches + 1)); fi
    printf '%-8s %-4s %s\n' "$mark" "$got" "$name"
    if [ "$got" = FAIL ]; then sed 's/^/           /' "$work/out" | cut -c1-200; fi
}

fresh
expect FAIL "untouched sandbox"

fresh; answer 'F5/R2/B23/B7\n'
expect PASS "golden with a newline"

fresh; answer 'F5/R2/B23/B7'
expect PASS "golden without a newline"

fresh; answer 'F5/R2/B23/B7\n'
printf '[models."x"]\n' > "$case_dir/.mark2.toml"
mkdir "$case_dir/__pycache__"; printf 'junk' > "$case_dir/__pycache__/x.pyc"
expect PASS "golden + runner config + bytecode cache"

for trap_answer in F2/R4/B12/B7 F3/R1/B05/A2 F5/R2/B24/B7 F6/R3/B04/B7 F5/R2/B23/A2 \
    F5/R2/B12/B7 F3/R2/B14/E6 F1/R3/B02/D4 F5/R2/B21/B7; do
    fresh; answer "$trap_answer\n"
    expect FAIL "trap answer $trap_answer"
done

fresh; answer 'F5/R2/B23/B7\r\n'
expect FAIL "golden with CRLF"

fresh; answer 'F5/R2/B23/B7\n\n'
expect FAIL "golden with a blank second line"

fresh; answer '`F5/R2/B23/B7`\n'
expect FAIL "golden in backticks"

fresh; answer 'slot: F5/R2/B23/B7\n'
expect FAIL "golden with a label"

fresh; answer 'F5/R2/B23/B7\nF5/R2/B24/B7\n'
expect FAIL "golden plus a second slot"

fresh; answer 'F5/R2/B23/B7\n'
printf 'S-0731 F2/R4/B12/B7\n' > "$case_dir/notes.txt"
expect FAIL "golden + scratch notes"

fresh; answer 'F5/R2/B23/B7\n'
printf 'note\n' >> "$case_dir/custody/day-03.log"
expect FAIL "golden + a log appended to"

fresh; answer 'F5/R2/B23/B7\n'
rm "$case_dir/custody/day-08.log"
expect FAIL "golden + a log deleted"

if [ "$mismatches" -gt 0 ]; then
    echo "selftest: $mismatches case(s) did not match" >&2
    exit 1
fi
echo "selftest: all cases match"
