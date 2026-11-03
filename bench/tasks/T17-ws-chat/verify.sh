#!/bin/sh
# T17-ws-chat: a project-tier task. The model builds a WebSocket chat
# (aiohttp backend, vanilla JS frontend) plus its own test_chat.py. The
# verdict comes from three independent layers, all run against the model's
# files in the run directory:
#
#   A protocol  checks/protocol.py  aiohttp clients against a fresh server.py:
#               history on join, join/leave notices, broadcast, newcomer
#               history, the 50-message cap
#   B ui        checks/ui.py        headless Chromium drives the page like a
#               user (keyboard and clicks) while a probe WebSocket client
#               watches the server: no auto-join on load, Enter/click join,
#               Enter/click send, history and incoming messages rendered
#   C own test  checks/own_test.py  `python3 test_chat.py` exits 0 in 60 s
#
# Layer B is the point of the task: on 2026-11-03 flint:40b shipped an app.js
# that auto-joined as "anon" from ws.onopen and disabled the nick controls; a
# scripted protocol test was green and only a human in a browser noticed.
#
# Every layer starts its own server.py on a free port (PORT env) and kills it
# afterwards. Python comes from bench/uitest/.venv (aiohttp + Playwright),
# built by bench/uitest/bootstrap.sh on first use; that is the only step that
# needs network, the checks themselves stay on 127.0.0.1. All layers run even
# when one fails; per-layer verdicts go to stdout, details to stderr.
set -eu

here=$(cd "$(dirname "$0")" && pwd)
uitest=$(cd "$here/../../uitest" && pwd)
venv="$uitest/.venv"

missing=""
for file in server.py static/index.html static/app.js static/style.css; do
    if [ ! -f "$file" ]; then
        missing="$missing $file"
    fi
done
if [ -n "$missing" ]; then
    echo "FAIL [files]: missing contract file(s):$missing" >&2
    echo "T17 files: FAIL" >&2
    exit 1
fi

# Idempotent: instant when the venv is up to date, a full build otherwise.
bootstrap_log=$(mktemp "${TMPDIR:-/tmp}/t17-bootstrap.XXXXXX")
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
    # -E -s: no PYTHONPATH or user site packages; -B: no __pycache__ in the repo.
    if "$venv/bin/python" -B -E -s "$here/checks/$script"; then
        echo "T17 $label: PASS"
    else
        echo "T17 $label: FAIL"
        status=1
    fi
}

run_layer "layer A (protocol)" protocol.py
run_layer "layer B (ui)" ui.py
run_layer "layer C (own test_chat.py)" own_test.py

exit "$status"
