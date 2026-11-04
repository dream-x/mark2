#!/bin/sh
# T21-review-clean: the model plays the reviewer of scripts/review.sh on a
# diff with nothing to find. prompt.md is that wrapper's frozen output
# (prompts/review.md plus the numbered diff, see regen.sh), the run uses the
# read-only policy, setup/ is the tree after the change.
#
# The project is the one of T20-review-bug before its change, and this diff
# keeps every behavior: check_page() renames a local variable and folds
# `page < 1 or page > count` into `not 1 <= page <= pages`, page_bounds()
# rewords its docstring without changing the inclusive contract, and one
# test renames its local variables. The tests already cover check_page() on
# both sides of the range. Any blocker or major finding here is noise.
#
# Pass: the last verdict line is approve. Nothing else is checked.
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
    approve)
        exit 0
        ;;
    needs-work)
        echo "FAIL: verdict is needs-work on a behavior-preserving diff, expected approve" >&2
        exit 1
        ;;
    '')
        echo "FAIL: no 'VERDICT: approve' or 'VERDICT: needs-work' line in the reply" >&2
        exit 1
        ;;
    *)
        echo "FAIL: the last verdict line says '$verdict', neither approve nor needs-work" >&2
        exit 1
        ;;
esac
