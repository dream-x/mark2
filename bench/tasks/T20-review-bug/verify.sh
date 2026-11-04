#!/bin/sh
# T20-review-bug: the model plays the reviewer of scripts/review.sh. prompt.md
# is that wrapper's frozen output (prompts/review.md plus the numbered diff,
# see regen.sh), the run uses the read-only policy, setup/ is the tree after
# the change.
#
# The diff switches lib/pager.py page_bounds() from an inclusive (first, last)
# pair to a half-open (start, stop) range and is consistent inside itself:
# docstring, paginate() and tests/test_pager.py follow. report.py, which the
# diff does not show, still calls page_bounds() and slices
# rows[first:last + 1], so every page now shows one row of the next page and
# the page subtotals no longer add up to the grand total. Only a reviewer who
# searches for the other callers of page_bounds() can see it.
#
# Pass: the last verdict line is needs-work, the reply names report.py, and
# some line cites lib/pager.py:LINE with LINE inside the changed hunk.
# Severity is not checked.
set -eu

if [ -z "${MARK2_STDOUT:-}" ] || [ ! -f "$MARK2_STDOUT" ]; then
    echo "FAIL: MARK2_STDOUT is unset or missing" >&2
    exit 1
fi

# The lib/pager.py hunk of the diff in prompt.md, new-file numbering:
# @@ -33,21 +33,21 @@. regen.sh checks these two lines against the diff.
hunk_first=33
hunk_last=53

# Copy of parse_verdict from scripts/review.sh: the last line that is nothing
# but a verdict wins; markdown decoration and letter case are forgiven.
# shellcheck disable=SC2016 # the backticks below are literal markdown
verdict=$(tr -d '\r' <"$MARK2_STDOUT" | tr '[:upper:]' '[:lower:]' |
    sed -n 's/^[[:space:]#>*`]*verdict:[[:space:]*`]*\([a-z-]*\)[[:space:]*`.]*$/\1/p' |
    tail -n 1)

case $verdict in
    approve | needs-work) ;;
    '')
        echo "FAIL: no 'VERDICT: approve' or 'VERDICT: needs-work' line in the reply" >&2
        exit 1
        ;;
    *)
        echo "FAIL: the last verdict line says '$verdict', neither approve nor needs-work" >&2
        exit 1
        ;;
esac

failed=
if [ "$verdict" != needs-work ]; then
    echo "FAIL: verdict is $verdict, expected needs-work (report.py still treats page_bounds() as inclusive)" >&2
    failed=yes
fi

if ! grep -q 'report\.py' "$MARK2_STDOUT"; then
    echo "FAIL: the reply never names report.py, the caller of page_bounds() the change breaks" >&2
    failed=yes
fi

# Every `pager.py:N` in the reply, one N per line. The path may be written as
# lib/pager.py, pager.py, b/lib/pager.py or ./lib/pager.py, and must not be
# glued to a longer name (mypager.py, src/lib/pager.py). For a range
# (pager.py:36-44) the first line counts.
refs=$(awk '
BEGIN { re = "[^A-Za-z0-9_./-](\\./)?([ab]/)?(lib/)?pager\\.py:[0-9]+" }
{
    line = " " $0
    while (match(line, re)) {
        m = substr(line, RSTART, RLENGTH)
        sub(/^.*pager\.py:/, "", m)
        print m
        line = substr(line, RSTART + RLENGTH)
    }
}' "$MARK2_STDOUT")
in_hunk=$(printf '%s\n' "$refs" | awk -v lo="$hunk_first" -v hi="$hunk_last" \
    '$0 != "" && $0 + 0 >= lo && $0 + 0 <= hi { print; exit }')
if [ -z "$in_hunk" ]; then
    seen=$(printf '%s\n' "$refs" | tr '\n' ' ' | sed 's/ *$//')
    echo "FAIL: no line cites lib/pager.py:LINE with LINE in $hunk_first..$hunk_last, the changed hunk (lib/pager.py lines cited: ${seen:-none})" >&2
    failed=yes
fi

[ -z "$failed" ] || exit 1
exit 0
