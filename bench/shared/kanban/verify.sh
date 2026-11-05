#!/bin/sh
# Shared verify of the kanban pair: T24-app-freeform and T25-app-planned.
#
# The pair is an experiment on prompt structure: the same app (an aiohttp
# kanban board with a JSON API, a JSON file store and a plain JS page, plus
# the model's own test_board.py) asked for once as a plain paragraph (T24)
# and once as a brief + plan + TODO + Definition of Done (T25). For the
# comparison to be fair the verdict must be the same function of the run
# directory in both, so the checks live here, once. Each task's verify.sh is
# a byte-identical wrapper (crates/bench task.rs tests assert it) that execs
# this script with its task ID, which is used only in the verdict labels.
#
# Usage (cwd = the run directory): sh bench/shared/kanban/verify.sh <task-id>
#
#   A api       checks/api.py       HTTP against a fresh server.py: empty
#               board, create / move / delete, 400 for a blank title and an
#               unknown column, 404 for an unknown id, BOARD_FILE written as
#               JSON, and the board intact after SIGKILL + restart (twice)
#   B ui        checks/ui.py        headless Chromium drives the page like a
#               user (keyboard and clicks): board loaded on open, add by
#               Enter and by #add-btn, blank adds nothing, move right/left,
#               delete; after each action the DOM and GET /api/cards must
#               agree, then reload and a second browser show the same board
#   C own test  checks/own_test.py  `python3 test_board.py` exits 0 within
#               60 s, and exits non-zero when server.py is a stub that never
#               starts (a test that stays green then checks nothing)
#
# Every layer starts its own server.py on a free port (PORT env) with its own
# BOARD_FILE in a temporary directory and kills it afterwards. Python comes
# from bench/uitest/.venv (aiohttp + Playwright), built by
# bench/uitest/bootstrap.sh on first use; that is the only step that needs
# network, the checks themselves stay on 127.0.0.1. All layers run even when
# one fails; per-layer verdicts go to stdout, details to stderr.
#
# Self-test of these checks: sh bench/shared/kanban/selftest/run.sh
set -eu

id=${1:?usage: sh verify.sh <task-id>}
here=$(cd "$(dirname "$0")" && pwd)
uitest=$(cd "$here/../../uitest" && pwd)
venv="$uitest/.venv"

if [ ! -f server.py ]; then
    echo "FAIL [files]: server.py is missing in $(pwd)" >&2
    echo "$id files: FAIL"
    exit 1
fi

# Idempotent: instant when the venv is up to date, a full build otherwise.
bootstrap_log=$(mktemp "${TMPDIR:-/tmp}/kanban-bootstrap.XXXXXX")
if ! sh "$uitest/bootstrap.sh" > "$bootstrap_log" 2>&1; then
    echo "FAIL [env]: could not build the verify environment $venv" >&2
    echo "  (needs network for PyPI and possibly the Playwright browser CDN)." >&2
    echo "  Run it by hand to see why: sh $uitest/bootstrap.sh --force" >&2
    tail -n 15 "$bootstrap_log" >&2
    rm -f "$bootstrap_log"
    exit 1
fi
rm -f "$bootstrap_log"

# `python3` inside the model's test and server resolves to the venv, too.
PATH="$venv/bin:$PATH"
export PATH

status=0
run_layer() {
    label=$1
    script=$2
    # -E -s: no PYTHONPATH or user site packages; -B: no __pycache__ in the run dir.
    if "$venv/bin/python" -B -E -s "$here/checks/$script"; then
        echo "$id $label: PASS"
    else
        echo "$id $label: FAIL"
        status=1
    fi
}

run_layer "layer A (api)" api.py
run_layer "layer B (ui)" ui.py
run_layer "layer C (own test_board.py)" own_test.py

exit "$status"
