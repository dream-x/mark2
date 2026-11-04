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
# Pass: the last verdict line is needs-work and the reply names report.py.
# Severity and the file:line anchor of the finding are not checked. report.py
# never appears in the prompt (regen.sh guarantees it), so naming it already
# proves the reviewer searched the repository. An earlier version also
# demanded a lib/pager.py:LINE anchor inside the changed hunk; the first
# matrix (review-v1) showed that it failed correct reviews, which anchor the
# finding on the broken caller, report.py:38, and that is semantically right.
set -eu

if [ -z "${MARK2_STDOUT:-}" ] || [ ! -f "$MARK2_STDOUT" ]; then
    echo "FAIL: MARK2_STDOUT is unset or missing" >&2
    exit 1
fi

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

[ -z "$failed" ] || exit 1
exit 0
