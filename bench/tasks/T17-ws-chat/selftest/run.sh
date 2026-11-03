#!/bin/sh
# Self-test of T17's verify.sh: every fixture must get its expected verdict.
#
#   golden    the working chat from the 2026-11-03 demo, adapted to the
#             contract                                   -> PASS
#   buggy-ui  golden + selftest/buggy-ui overlay: app.js auto-joins as "anon"
#             from ws.onopen and disables #nick/#join-btn (the bug a scripted
#             protocol test missed)        -> FAIL, layer A PASS, layer B FAIL
#   missing   an empty directory                         -> FAIL on [files]
#   crash     golden + selftest/crash overlay: server.py does not compile
#                                         -> FAIL, the server never starts
#
# Overlays hold only the files that differ from golden. Each case runs in a
# fresh temporary copy, like a bench run. Usage: sh selftest/run.sh
set -eu

here=$(cd "$(dirname "$0")" && pwd)
verify="$here/../verify.sh"
work=$(mktemp -d "${TMPDIR:-/tmp}/t17-selftest.XXXXXX")
trap 'rm -rf "$work"' EXIT
trap 'exit 1' HUP INT TERM

failures=0

# prepare <case> [overlay]: a fresh run directory with golden (+ overlay).
prepare() {
    dir="$work/$1"
    mkdir -p "$dir"
    if [ "$1" != missing ]; then
        cp -R "$here/golden/." "$dir/"
    fi
    if [ -n "${2:-}" ]; then
        cp -R "$here/$2/." "$dir/"
    fi
}

# check <case> <want-exit> <needle>...: run verify.sh in the case directory;
# the exit code must match and every needle must occur in its output.
check() {
    name=$1
    want=$2
    shift 2
    out="$work/$name.out"
    started=$(date +%s)
    set +e
    (cd "$work/$name" && MARK2_STDOUT=/dev/null sh "$verify") > "$out" 2>&1
    got=$?
    set -e
    took=$(($(date +%s) - started))
    problems=""
    if [ "$got" -ne "$want" ]; then
        problems="; exit $got, expected $want"
    fi
    for needle in "$@"; do
        if ! grep -qF -- "$needle" "$out"; then
            problems="$problems; no '$needle' in the output"
        fi
    done
    if [ -z "$problems" ]; then
        echo "ok    $name (exit $got, ${took}s)"
    else
        echo "FAIL  $name:${problems#;} (${took}s); verify output:"
        sed 's/^/      | /' "$out"
        failures=$((failures + 1))
    fi
}

prepare golden
check golden 0 \
    "T17 layer A (protocol): PASS" \
    "T17 layer B (ui): PASS" \
    "T17 layer C (own test_chat.py): PASS"

prepare buggy-ui buggy-ui
check buggy-ui 1 \
    "T17 layer A (protocol): PASS" \
    "T17 layer B (ui): FAIL" \
    "auto-join" \
    "#nick must be visible and editable"

prepare missing
check missing 1 \
    "FAIL [files]: missing contract file(s): server.py static/index.html static/app.js static/style.css"

prepare crash crash
check crash 1 \
    "server.py exited with code 1 before it was ready" \
    "SyntaxError" \
    "T17 layer A (protocol): FAIL" \
    "T17 layer B (ui): FAIL"

if [ "$failures" -ne 0 ]; then
    echo "T17 selftest: $failures case(s) did not get the expected verdict"
    exit 1
fi
echo "T17 selftest: all cases got the expected verdict"
