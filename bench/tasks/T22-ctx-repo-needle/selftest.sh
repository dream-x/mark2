#!/bin/sh
# Self-test of T22's verify.sh on prepared sandboxes, no model involved:
#
#   sh bench/tasks/T22-ctx-repo-needle/selftest.sh
#
# Every case is a fresh copy of setup/ plus the edits an agent might make;
# verify.sh must PASS the golden fixes and FAIL everything else. Rerun after
# any change to gen.py, setup/ or verify.sh.
set -eu

here=$(CDPATH='' cd -- "$(dirname -- "$0")" && pwd)
python3 "$here/gen.py" --check

work=$(mktemp -d "${TMPDIR:-/tmp}/t22-selftest.XXXXXX")
trap 'rm -rf "$work"' EXIT
case_dir=$work/case
mismatches=0

fresh() {
    rm -rf "$case_dir"
    mkdir "$case_dir"
    cp -R "$here/setup/." "$case_dir/"
}

# Exact-snippet edit with mark2's `edit` semantics: old must occur exactly
# once (or every occurrence is replaced with all=1).
edit() {
    file=$1 old=$2 new=$3 all=${4:-0}
    (cd "$case_dir" && python3 - "$file" "$old" "$new" "$all" <<'PY'
import sys
path, old, new, replace_all = sys.argv[1], sys.argv[2], sys.argv[3], sys.argv[4] == "1"
text = open(path).read()
count = text.count(old)
if count == 0 or (count > 1 and not replace_all):
    sys.exit("edit: %d matches for the snippet in %s" % (count, path))
open(path, "w").write(text.replace(old, new) if replace_all else text.replace(old, new, 1))
PY
    )
}

expect() {
    want=$1 name=$2
    if (cd "$case_dir" && sh "$here/verify.sh") >"$work/out" 2>&1; then got=PASS; else got=FAIL; fi
    if [ "$got" = "$want" ]; then mark=ok; else mark=MISMATCH; mismatches=$((mismatches + 1)); fi
    printf '%-8s %-4s %s\n' "$mark" "$got" "$name"
    if [ "$got" = FAIL ]; then sed 's/^/           /' "$work/out" | cut -c1-200; fi
}

bug='    if age_days >= REFUND_WINDOW_DAYS:
        raise RefundError(
            "E-4127",'
fixed='    if age_days > REFUND_WINDOW_DAYS:
        raise RefundError(
            "E-4127",'
needle=src/ledgerly/payments/refunds.py
line='    if age_days >= REFUND_WINDOW_DAYS:'

fresh
expect FAIL "untouched sandbox"

fresh
edit "$needle" "$bug" "$fixed"
expect PASS "golden: >= -> > in check_refund_window()"

fresh
edit "$needle" "$bug" '    if age_days > 30:
        raise RefundError(
            "E-4127",'
expect PASS "golden variant: literal 30"

fresh
edit "$needle" "$bug" '    if age_days >= REFUND_WINDOW_DAYS + 1:
        raise RefundError(
            "E-4127",'
expect PASS "golden variant: >= window + 1"

fresh
edit "$needle" "$bug" "$fixed"
printf 'import sys\n' > "$case_dir/scratch_check.py"
mkdir -p "$case_dir/src/ledgerly/payments/__pycache__"
printf 'junk' > "$case_dir/src/ledgerly/payments/__pycache__/refunds.cpython-311.pyc"
expect PASS "golden + a scratch file + a junk bytecode cache"

fresh
edit "$needle" "$bug" "$fixed"
python3 -c "import sys; p=sys.argv[1]; t=open(p).read(); open(p,'w').write(t.rstrip('\n'))" "$case_dir/$needle"
expect PASS "golden, final newline lost"

fresh
edit "$needle" "$line" '    if age_days > REFUND_WINDOW_DAYS:' 1
expect FAIL "replace_all inside refunds.py (in-file twin)"

fresh
(cd "$case_dir" && grep -rl "age_days >= REFUND_WINDOW_DAYS" src | while IFS= read -r f; do
    sed 's/age_days >= REFUND_WINDOW_DAYS/age_days > REFUND_WINDOW_DAYS/' "$f" > "$f.new" && mv "$f.new" "$f"
done)
expect FAIL "sed over the whole tree"

fresh
edit src/ledgerly/legacy/refunds_v1.py "$line" '    if age_days > REFUND_WINDOW_DAYS:'
expect FAIL "first E-4127 hit fixed: legacy only"

fresh
edit "$needle" "$bug" "$fixed"
edit src/ledgerly/legacy/refunds_v1.py "$line" '    if age_days > REFUND_WINDOW_DAYS:'
expect FAIL "live fixed, legacy fixed too"

fresh
edit "$needle" "$bug" "$fixed"
edit src/ledgerly/marketplace/seller_refunds.py "$line" '    if age_days > REFUND_WINDOW_DAYS:'
expect FAIL "live fixed, seller window 'aligned' too"

fresh
edit "$needle" 'REFUND_WINDOW_DAYS = 30' 'REFUND_WINDOW_DAYS = 31'
expect FAIL "constant bumped to 31"

fresh
edit "$needle" "$bug" "$fixed"
edit docs/errors.md '| E-4127 | refund window closed | `ledgerly.legacy.refunds_v1` |' \
    '| E-4127 | refund window closed | `ledgerly.payments.refunds` |'
expect FAIL "live fixed, stale docs row 'fixed' too"

fresh
edit "$needle" "$bug" "$fixed"
edit "$needle" '"""Customer refunds: eligibility checks and refund amounts.' '"""Customer refunds: eligibility checks and refund amounts (fixed).'
expect FAIL "live fixed plus a second line in the same file"

fresh
edit "$needle" "$bug" '    if age_days > REFUND_WINDOW_DAYS
        raise RefundError(
            "E-4127",'
expect FAIL "fix with a syntax error"

fresh
rm "$case_dir/$needle"
expect FAIL "needle file deleted"

if [ "$mismatches" -gt 0 ]; then
    echo "selftest: $mismatches case(s) did not match" >&2
    exit 1
fi
echo "selftest: all cases match"
