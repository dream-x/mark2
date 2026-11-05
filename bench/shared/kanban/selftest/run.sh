#!/bin/sh
# Self-test of the kanban verify shared by T24-app-freeform and
# T25-app-planned: every fixture must get its expected verdict, through each
# task's own verify.sh (byte-identical wrappers, checked first).
#
#   golden        reference app: aiohttp server, plain JS page, own test
#                                                      -> PASS, all layers
#   variant       a valid app built unlike golden: uuid string ids, 200 on
#                 create, page inlined, add through a <form> with #add-btn
#                 disabled while the title is blank, card buttons visible
#                 only on hover, confirm() before delete, <li> cards
#                 -> PASS, all layers (verify judges the contract, not golden)
#   empty        an empty run directory               -> FAIL on [files]
#   stub          server up, every feature a dummy: the API answers happy-path
#                 JSON but stores nothing, the page has every id and does
#                 nothing                         -> layer A FAIL, layer B FAIL
#   dead-ui       golden server; the page renders the board but its controls
#                 are not wired                   -> layer A PASS, layer B FAIL
#   dom-only      golden server; add / move / delete work on screen but only
#                 in the DOM, the server never hears of them
#                                                 -> layer A PASS, layer B FAIL
#   stale-ui      golden server; a move reaches the server but the page is not
#                 redrawn                         -> layer A PASS, layer B FAIL
#   save-on-exit  the board is written only on a graceful shutdown, a hard
#                 kill loses it        -> layer A FAIL, layer B PASS, C FAIL
#   trivial-test  test_board.py checks nothing and exits 0
#                                     -> layers A and B PASS, layer C FAIL
#   crash         server.py does not compile    -> FAIL, the server never starts
#
# Overlays hold only the files that differ from golden. Each case runs in a
# fresh temporary copy, like a bench run. Usage: sh selftest/run.sh
set -eu

here=$(cd "$(dirname "$0")" && pwd)
tasks=$(cd "$here/../../../tasks" && pwd)
pair="T24-app-freeform T25-app-planned"
work=$(mktemp -d "${TMPDIR:-/tmp}/kanban-selftest.XXXXXX")
trap 'rm -rf "$work"' EXIT
trap 'exit 1' HUP INT TERM

failures=0

# The pair may differ only in prompt.md.
for file in verify.sh task.toml; do
    if ! cmp -s "$tasks/T24-app-freeform/$file" "$tasks/T25-app-planned/$file"; then
        echo "FAIL  $file differs between T24-app-freeform and T25-app-planned"
        failures=$((failures + 1))
    fi
done

# prepare <dir> <case>: a fresh run directory with golden (+ the case overlay).
prepare() {
    mkdir -p "$1"
    case $2 in
        empty) ;;
        golden) cp -R "$here/golden/." "$1/" ;;
        *) cp -R "$here/golden/." "$1/" && cp -R "$here/$2/." "$1/" ;;
    esac
}

# check <task> <case> <want-exit> <needle>...: run the task's verify.sh in a
# fresh copy of the case; the exit code must match and every needle must
# occur in its output. "@" in a needle stands for the task ID.
check() {
    task=$1
    name=$2
    want=$3
    shift 3
    id=${task%%-*}
    dir="$work/$id-$name"
    out="$dir.out"
    prepare "$dir" "$name"
    started=$(date +%s)
    set +e
    (cd "$dir" && MARK2_STDOUT=/dev/null sh "$tasks/$task/verify.sh") > "$out" 2>&1
    got=$?
    set -e
    took=$(($(date +%s) - started))
    problems=""
    if [ "$got" -ne "$want" ]; then
        problems="; exit $got, expected $want"
    fi
    for needle in "$@"; do
        needle=$(printf '%s' "$needle" | sed "s/@/$id/g")
        if ! grep -qF -- "$needle" "$out"; then
            problems="$problems; no '$needle' in the output"
        fi
    done
    if [ -z "$problems" ]; then
        echo "ok    $id $name (exit $got, ${took}s)"
    else
        echo "FAIL  $id $name:${problems#;} (${took}s); verify output:"
        sed 's/^/      | /' "$out"
        failures=$((failures + 1))
    fi
}

for task in $pair; do
    check "$task" golden 0 \
        "@ layer A (api): PASS" \
        "@ layer B (ui): PASS" \
        "@ layer C (own test_board.py): PASS"

    check "$task" variant 0 \
        "@ layer A (api): PASS" \
        "@ layer B (ui): PASS" \
        "@ layer C (own test_board.py): PASS"

    check "$task" empty 1 \
        "FAIL [files]: server.py is missing" \
        "@ files: FAIL"

    check "$task" stub 1 \
        "@ layer A (api): FAIL" \
        "@ layer B (ui): FAIL" \
        "the API does not keep cards"

    check "$task" dead-ui 1 \
        "@ layer A (api): PASS" \
        "@ layer B (ui): FAIL" \
        "typing a title into #new-card and pressing Enter"

    check "$task" dom-only 1 \
        "@ layer A (api): PASS" \
        "@ layer B (ui): FAIL" \
        "the page has to change the board through the API"

    check "$task" stale-ui 1 \
        "@ layer A (api): PASS" \
        "@ layer B (ui): FAIL" \
        "clicking .move-right on a card in #todo"

    check "$task" save-on-exit 1 \
        "@ layer A (api): FAIL" \
        "@ layer B (ui): PASS" \
        "@ layer C (own test_board.py): FAIL" \
        "BOARD_FILE"

    check "$task" trivial-test 1 \
        "@ layer A (api): PASS" \
        "@ layer B (ui): PASS" \
        "@ layer C (own test_board.py): FAIL" \
        "still exited 0"

    check "$task" crash 1 \
        "server.py exited with code 1 before it was ready" \
        "SyntaxError" \
        "@ layer A (api): FAIL" \
        "@ layer B (ui): FAIL" \
        "@ layer C (own test_board.py): FAIL"
done

if [ "$failures" -ne 0 ]; then
    echo "kanban selftest: $failures problem(s)"
    exit 1
fi
echo "kanban selftest: every case got the expected verdict for $pair"
