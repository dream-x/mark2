#!/bin/sh
# Offline tests for scripts/review.sh: a fake mark2 stands in for the model,
# records its arguments and answers with a canned text. No server needed.
#
#   sh scripts/test-review.sh
#   REVIEW_SHELL=dash sh scripts/test-review.sh   (review.sh under another shell)

# Checks are passed to eval in single quotes on purpose; $err and
# $expected_args are read inside those strings.
# shellcheck disable=SC2016,SC2034

set -u

here=$(CDPATH='' cd -- "$(dirname -- "$0")" && pwd)
review=$here/review.sh
review_shell=${REVIEW_SHELL:-sh}
work=$(mktemp -d "${TMPDIR:-/tmp}/mark2-review-test.XXXXXX")
trap 'rm -rf "$work"' EXIT

# Isolate git from the user's config (signing, hooks, aliases).
GIT_CONFIG_NOSYSTEM=1
GIT_CONFIG_GLOBAL=/dev/null
export GIT_CONFIG_NOSYSTEM GIT_CONFIG_GLOBAL

failures=0
pass() { printf 'ok   %s\n' "$1"; }
fail() {
    printf 'FAIL %s\n' "$1"
    failures=$((failures + 1))
}
check() { # check NAME COMMAND...
    name=$1
    shift
    if "$@"; then pass "$name"; else fail "$name"; fi
}

# Fake mark2: arguments one per line, the prompt saved whole, a call counter,
# and FAKE_FIRST_ANSWER (if set) instead of FAKE_ANSWER on the first call. Like
# the real one, it takes the prompt from the last argument, or from stdin when
# that argument is "-", refuses an empty stdin, and rejects a --temperature
# that is not a number from 0 to 2 with exit 2 (a clap usage error).
fake=$work/fake-mark2
cat >"$fake" <<'EOF'
#!/bin/sh
: >"$FAKE_DIR/args"
prev=
temperature=
temperature_given=
for arg in "$@"; do
    printf '%s\n' "$arg" >>"$FAKE_DIR/args"
    case $prev in
        --temperature) temperature=$arg temperature_given=yes ;;
    esac
    prev=$arg
    last=$arg
done
if [ "$last" = - ]; then
    cat >"$FAKE_DIR/prompt"
    [ -s "$FAKE_DIR/prompt" ] || {
        echo 'error: empty prompt' >&2
        exit 1
    }
else
    printf '%s' "$last" >"$FAKE_DIR/prompt"
fi
calls=$(($(cat "$FAKE_DIR/calls" 2>/dev/null || echo 0) + 1))
echo "$calls" >"$FAKE_DIR/calls"
if [ -n "$temperature_given" ] &&
    ! awk -v t="$temperature" \
        'BEGIN { exit !(t ~ /^([0-9]+([.][0-9]*)?|[.][0-9]+)$/ && t + 0 <= 2) }'; then
    printf "error: invalid value '%s' for '--temperature <T>'\n" "$temperature" >&2
    exit 2
fi
[ -z "${FAKE_SLEEP:-}" ] || exec sleep "$FAKE_SLEEP"
if [ "$calls" -eq 1 ] && [ -n "${FAKE_FIRST_ANSWER:-}" ]; then
    printf '%s\n' "$FAKE_FIRST_ANSWER"
else
    printf '%s\n' "$FAKE_ANSWER"
fi
exit "${FAKE_STATUS:-0}"
EOF
chmod +x "$fake"
FAKE_DIR=$work
FAKE_ANSWER='No issues found.

VERDICT: approve'
FAKE_STATUS=0
FAKE_FIRST_ANSWER=
export FAKE_DIR FAKE_ANSWER FAKE_STATUS FAKE_FIRST_ANSWER
MARK2_REVIEW_BIN=$fake
export MARK2_REVIEW_BIN
unset MARK2_REVIEW_MODEL MARK2_REVIEW_TEMPERATURE MARK2_REVIEW_BASE_URL \
    MARK2_REVIEW_MAX_DIFF_BYTES MARK2_REVIEW_TIMEOUT_SECS FAKE_SLEEP

repo=$work/repo
mkdir "$repo"
g() { git -C "$repo" -c user.name=test -c user.email=test@example.com "$@"; }
g init -q
printf 'fn one() -> u32 {\n    1\n}\n' >"$repo/lib.rs"
g add lib.rs
g commit -q -m init

# run [ARGS...]: run review.sh inside the repo; sets $code, $out, $err.
run() {
    rm -f "$work/args" "$work/prompt" "$work/calls"
    (cd "$repo" && "$review_shell" "$review" "$@") >"$work/out" 2>"$work/err" </dev/null
    code=$?
    out=$(cat "$work/out")
    err=$(cat "$work/err")
}
# run_stdin FILE [ARGS...]: same, with FILE on stdin.
run_stdin() {
    input=$1
    shift
    rm -f "$work/args" "$work/prompt" "$work/calls"
    (cd "$repo" && "$review_shell" "$review" "$@") >"$work/out" 2>"$work/err" <"$input"
    code=$?
    out=$(cat "$work/out")
    err=$(cat "$work/err")
}
called() { [ -f "$work/args" ]; }
# arg_value FLAG: the argument that follows FLAG in the fake's argv.
arg_value() { awk -v flag="$1" 'prev == flag { print; exit } { prev = $0 }' "$work/args"; }
calls_are() { [ "$(cat "$work/calls" 2>/dev/null || echo 0)" -eq "$1" ]; }
prompt_has() { grep -qF -- "$1" "$work/prompt"; }
code_is() { [ "$code" -eq "$1" ]; }

# --- help and argument errors ---
run --help
check "help exits 0 and shows usage" code_is 0
check "help names the stdin mode" eval 'printf "%s" "$out" | grep -q "review.sh -"'
run --bogus
check "unknown option is an error" code_is 1
run a b
check "two arguments are an error" code_is 1
run no-such-rev
check "unknown revision is an error" code_is 1
check "unknown revision does not call mark2" eval '! called'

# --- empty diffs ---
run
check "clean tree: exit 0" code_is 0
check "clean tree: says nothing to review" [ "$out" = "nothing to review" ]
check "clean tree: mark2 not called" eval '! called'
: >"$work/empty.diff"
run_stdin "$work/empty.diff" -
check "empty stdin: nothing to review" [ "$out" = "nothing to review" ]
check "empty stdin: mark2 not called" eval '! called'

# --- uncommitted changes, default flags ---
printf 'fn one() -> u32 {\n    2\n}\n' >"$repo/lib.rs"
run
check "approve: exit 0" code_is 0
check "approve: the answer reaches stdout" eval 'printf "%s" "$out" | grep -q "VERDICT: approve"'
expected_args="-p
--policy
read-only
-C
$(cd "$repo" && git rev-parse --show-toplevel)
--base-url
http://localhost:11434/v1
--model
qwen3.6:35b-a3b-coding
--temperature
0"
check "flags: read-only print mode, default model, URL and temperature" \
    eval '[ "$(sed -n "1,11p" "$work/args")" = "$expected_args" ]'
check "temperature: --temperature 0 by default, a repeatable verdict" \
    eval '[ "$(arg_value --temperature)" = 0 ]'
check "prompt travels on stdin, not in argv" \
    eval '[ "$(cat "$work/args")" = "$expected_args
-" ]'
check "prompt starts with prompts/review.md" \
    eval '[ "$(head -n 1 "$work/prompt")" = "$(head -n 1 "$here/../prompts/review.md")" ]'
check "prompt carries the diff" prompt_has '+    2'
check "prompt carries the stat" prompt_has 'lib.rs | 2 +-'
check "prompt names the mode" prompt_has 'git diff HEAD'
check "prompt ends with the verdict reminder" eval 'tail -n 1 "$work/prompt" | grep -q "VERDICT: approve or VERDICT: needs-work"'

export MARK2_REVIEW_MODEL=m1 MARK2_REVIEW_BASE_URL=http://h:1/v1
run
unset MARK2_REVIEW_MODEL MARK2_REVIEW_BASE_URL
check "env overrides model and URL" \
    eval 'grep -qx m1 "$work/args" && grep -qx "http://h:1/v1" "$work/args"'

# --- temperature: 0 unless MARK2_REVIEW_TEMPERATURE says otherwise ---
export MARK2_REVIEW_TEMPERATURE=0.7
run
check "temperature: env overrides the default" \
    eval 'code_is 0 && [ "$(arg_value --temperature)" = 0.7 ]'
check "temperature: stderr names the one in use" \
    eval 'printf "%s" "$err" | grep -q "temperature 0.7"'
export MARK2_REVIEW_TEMPERATURE=
run
check "temperature: an empty env value falls back to 0" \
    eval 'code_is 0 && [ "$(arg_value --temperature)" = 0 ]'
# The range is mark2's to check: its usage error (exit 2) must not pass for
# VERDICT: needs-work (also 2).
export MARK2_REVIEW_TEMPERATURE=3
run
unset MARK2_REVIEW_TEMPERATURE
check "temperature: out of range is a failure (exit 1), not needs-work" code_is 1
check "temperature: mark2's error reaches stderr" \
    eval 'printf "%s" "$err" | grep -q "invalid value .3. for .--temperature"'
check "temperature: a usage error is not retried" calls_are 1

# --- verdict parsing and exit codes ---
# Variables are set and reset explicitly: assignments in front of a shell
# function call are unspecified in POSIX.
answer() { FAKE_ANSWER=$1; }
answer '1. [major] lib.rs:2: returns 2. Callers break.

VERDICT: needs-work'
run
check "needs-work: exit 2" code_is 2
answer '1. [major] lib.rs:2: x. y.

**VERDICT: Needs-Work**'
run
check "decorated verdict is understood" code_is 2
answer 'Looks fine to me.'
run
check "no verdict line twice: exit 1" code_is 1
check "no verdict line twice: retried exactly once" calls_are 2
check "no verdict line twice: says why" eval 'printf "%s" "$err" | grep -q "no .VERDICT"'
FAKE_FIRST_ANSWER='1. [major] lib.rs:2: x. y.'
answer '1. [major] lib.rs:2: x. y.

VERDICT: needs-work'
run
FAKE_FIRST_ANSWER=
check "verdict on the retry counts" eval 'code_is 2 && calls_are 2'
check "retry is visible in stdout and stderr" \
    eval 'printf "%s" "$out" | grep -q "^--- retry" && printf "%s" "$err" | grep -q "once more"'
answer 'VERDICT: approve'
run
check "a verdict on the first answer is not retried" calls_are 1
answer 'Use VERDICT: approve or VERDICT: needs-work.'
run
check "a line naming both verdicts is not a verdict" code_is 1
answer 'VERDICT: needs-work
on second thought
VERDICT: approve'
run
check "the last verdict line wins" code_is 0
answer 'No issues found.

VERDICT: needs-work'
run
check "needs-work without serious findings warns" eval 'printf "%s" "$err" | grep -q "warning"'
check "... but keeps the verdict" code_is 2
answer 'VERDICT: approve'
FAKE_STATUS=1
run
FAKE_STATUS=0
check "mark2 failure: exit 1 even with a verdict" code_is 1
check "mark2 failure is not retried" calls_are 1

# --- wall-clock limit ---
export FAKE_SLEEP=30 MARK2_REVIEW_TIMEOUT_SECS=1
started=$(date +%s)
run
elapsed=$(($(date +%s) - started))
unset FAKE_SLEEP MARK2_REVIEW_TIMEOUT_SECS
check "timeout: a hung model run is stopped, exit 1" code_is 1
check "timeout: stopped within seconds, not after the sleep" [ "$elapsed" -lt 10 ]
check "timeout: stderr names the limit" eval 'printf "%s" "$err" | grep -q "MARK2_REVIEW_TIMEOUT_SECS"'
check "timeout: not retried" calls_are 1
export MARK2_REVIEW_TIMEOUT_SECS=0
run
check "timeout 0 disables the limit" code_is 0
export MARK2_REVIEW_TIMEOUT_SECS=soon
run
unset MARK2_REVIEW_TIMEOUT_SECS
check "bad timeout value is an error" code_is 1

# --- staged and revision ranges ---
g commit -q -am two
printf 'fn one() -> u32 {\n    3\n}\n' >"$repo/lib.rs"
g add lib.rs
printf 'fn one() -> u32 {\n    4\n}\n' >"$repo/lib.rs"
run --staged
check "staged: reviews the index only" eval 'prompt_has "+    3" && ! prompt_has "+    4"'
run HEAD~1
check "REV: reviews REV..HEAD" eval 'prompt_has "-    1" && prompt_has "+    2" && ! prompt_has "+    3"'
check "REV: prompt names the range" prompt_has 'git diff HEAD~1..HEAD'
run HEAD~1..HEAD
check "explicit range is passed as is" eval 'prompt_has "+    2" && prompt_has "git diff HEAD~1..HEAD"'
printf 'new\n' >"$repo/fresh.txt"
run
check "untracked files are reported on stderr" eval 'printf "%s" "$err" | grep -q "fresh.txt"'
rm "$repo/fresh.txt"

# --- hunk lines are numbered by their line in the new file ---
cat >"$work/numbers.diff" <<'EOF'
diff --git a/lib.rs b/lib.rs
--- a/lib.rs
+++ b/lib.rs
@@ -10,3 +10,3 @@ fn one() -> u32 {
 keep
-old
+new
 tail
--- a/lib.rs
@@ -40 +40,2 @@
-x
+y
+z
\ No newline at end of file
EOF
run_stdin "$work/numbers.diff" -
check "numbers: context and added lines carry the new line number" \
    eval 'prompt_has "    10  keep" && prompt_has "    11 +new" && prompt_has "    12  tail"'
check "numbers: removed lines carry none" eval 'grep -qx "       -old" "$work/prompt"'
check "numbers: a header after the hunk is not a hunk line" eval 'grep -qx -- "--- a/lib.rs" "$work/prompt"'
check "numbers: single-line hunk header" eval 'prompt_has "    40 +y" && prompt_has "    41 +z"'
check "numbers: the no-newline marker is kept" prompt_has '\ No newline at end of file'

# --- stdin and the size cap ---
i=0
while [ $i -lt 200 ]; do
    echo "// filler line $i"
    i=$((i + 1))
done >"$work/filler.rs"
(cd "$work" && git diff --no-index /dev/null filler.rs) >"$work/big.diff"
run_stdin "$work/big.diff" -
check "stdin: reviewed whole under the cap" eval 'prompt_has "filler line 199" && ! prompt_has "[diff truncated"'
check "stdin: stat computed from the patch" grep -Eq 'filler[.]rs [|] +200 [+]' "$work/prompt"
export MARK2_REVIEW_MAX_DIFF_BYTES=400
run_stdin "$work/big.diff" -
check "cap: diff is cut with a marker" eval 'prompt_has "[diff truncated" && ! prompt_has "filler line 199"'
check "cap: stderr says so" eval 'printf "%s" "$err" | grep -q "cut to 400"'
# Between BEGIN and the marker: the body plus the blank line before the marker.
sed -n '/^===== BEGIN DIFF =====$/,/^\[diff truncated/p' "$work/prompt" | sed '1d;$d' >"$work/shown"
check "cap: body fits the cap" [ "$(wc -c <"$work/shown")" -le 401 ]
last_shown=$(sed '/^$/d' "$work/shown" | tail -n 1)
check "cap: the cut keeps whole lines only" \
    eval 'printf "%s\n" "$last_shown" | grep -Eqx " +[0-9]+ [+]// filler line [0-9]+"'
MARK2_REVIEW_MAX_DIFF_BYTES=abc
run_stdin "$work/big.diff" -
unset MARK2_REVIEW_MAX_DIFF_BYTES
check "bad cap value is an error" code_is 1
# A prompt past the 128 KiB Linux cap on one argv string reaches mark2 whole:
# it goes on stdin.
awk 'BEGIN { for (i = 0; i < 3000; i++) printf "// huge filler line %d of the stdin prompt test\n", i }' \
    >"$work/huge.rs"
(cd "$work" && git diff --no-index /dev/null huge.rs) >"$work/huge.diff"
export MARK2_REVIEW_MAX_DIFF_BYTES=1000000
run_stdin "$work/huge.diff" -
unset MARK2_REVIEW_MAX_DIFF_BYTES
check "huge prompt: over 128 KiB, delivered whole" \
    eval 'code_is 0 && [ "$(wc -c <"$work/prompt" | tr -d " ")" -gt 131072 ] &&
        prompt_has "huge filler line 2999" && ! prompt_has "[diff truncated"'
# Cyrillic comments are the norm in this repo: they must reach the model byte
# for byte (macOS iconv once "cleaned" such a diff by mistake).
cat >"$work/cyrillic.diff" <<'EOF'
diff --git a/lib.rs b/lib.rs
--- a/lib.rs
+++ b/lib.rs
@@ -1,2 +1,2 @@
-/// Медиана выборки. `None` — пустая выборка.
+/// Медиана выборки; для пустой выборки — `None`.
 /// повтор не должен тянуть за собой всю ячейку отчёта.
EOF
run_stdin "$work/cyrillic.diff" -
check "UTF-8 text reaches the prompt byte for byte" \
    eval 'code_is 0 && prompt_has "-/// Медиана выборки. \`None\` — пустая выборка." && prompt_has "ячейку отчёта."'
check "UTF-8 text is not reported as broken" eval '! printf "%s" "$err" | grep -q "UTF-8"'

echo
if [ "$failures" -eq 0 ]; then
    echo "all review.sh tests passed"
else
    echo "$failures review.sh test(s) failed"
    exit 1
fi
