#!/bin/sh
# Code review of a git diff by a local model running on mark2 in read-only mode.
#
# Usage:
#   scripts/review.sh            uncommitted changes vs HEAD (git diff HEAD)
#   scripts/review.sh --staged   staged changes only (git diff --cached)
#   scripts/review.sh REV        commits after REV (git diff REV..HEAD);
#                                a range with ".." or "..." is passed as is
#   scripts/review.sh -          a unified diff read from stdin
#
# Environment:
#   MARK2_REVIEW_MODEL           model id (default: qwen3.6:35b-a3b-coding)
#   MARK2_REVIEW_BASE_URL        OpenAI-compatible base URL
#                                (default: http://localhost:11434/v1)
#   MARK2_REVIEW_BIN             mark2 command, split on spaces (default:
#                                target/release/mark2 if built, otherwise
#                                cargo run -q -p mark2-cli --); it must read
#                                the prompt from stdin (mark2 -p -), so rebuild
#                                an old target/release
#   MARK2_REVIEW_MAX_DIFF_BYTES  diff size cap, larger diffs are cut
#                                (default: 122880, about 35-40k tokens; past
#                                the model's context the prompt head is lost)
#   MARK2_REVIEW_TIMEOUT_SECS    wall-clock limit per model run, 0 = none
#                                (default: 600)
#
# The model gets prompts/review.md, the diff stat and the diff with every
# hunk line numbered by its line in the new file, on mark2's stdin (no argv
# size limit). It sees the repository the diff came from (the git root of the
# current directory) and may read files there; bash, edit and write are
# denied by the read-only policy.
#
# An answer without a VERDICT line is retried once.
#
# Exit codes: 0 VERDICT: approve, 2 VERDICT: needs-work,
#             1 mark2 failed or the answer has no VERDICT line.

set -u
set -f # MARK2_REVIEW_BIN is word-split on purpose; no globbing in this script

die() {
    printf 'review: %s\n' "$*" >&2
    exit 1
}

note() {
    printf 'review: %s\n' "$*" >&2
}

usage() {
    sed -n '2,/^$/s/^# \{0,1\}//p' "$0"
}

script_dir=$(CDPATH='' cd -- "$(dirname -- "$0")" && pwd) || die "cannot locate the script directory"
tool_root=$(dirname -- "$script_dir")
prompt_file=$tool_root/prompts/review.md
[ -r "$prompt_file" ] || die "reviewer prompt not found: $prompt_file"

model=${MARK2_REVIEW_MODEL:-qwen3.6:35b-a3b-coding}
base_url=${MARK2_REVIEW_BASE_URL:-http://localhost:11434/v1}
max_bytes=${MARK2_REVIEW_MAX_DIFF_BYTES:-122880}
case $max_bytes in
    '' | *[!0-9]*) die "MARK2_REVIEW_MAX_DIFF_BYTES must be a positive integer, got '$max_bytes'" ;;
esac
[ "$max_bytes" -gt 0 ] || die "MARK2_REVIEW_MAX_DIFF_BYTES must be a positive integer, got '$max_bytes'"
timeout_secs=${MARK2_REVIEW_TIMEOUT_SECS:-600}
case $timeout_secs in
    '' | *[!0-9]*) die "MARK2_REVIEW_TIMEOUT_SECS must be a non-negative integer, got '$timeout_secs'" ;;
esac

[ $# -le 1 ] || {
    usage >&2
    exit 1
}
mode=${1:-}

case $mode in
    -h | --help)
        usage
        exit 0
        ;;
esac

# The reviewer works in the repository the diff belongs to. Stdin mode also
# works outside git: then the paths in the diff are relative to the current
# directory.
if repo_root=$(git rev-parse --show-toplevel 2>/dev/null); then
    :
elif [ "$mode" = - ]; then
    repo_root=$(pwd)
else
    die "not inside a git repository"
fi

tmp=$(mktemp -d "${TMPDIR:-/tmp}/mark2-review.XXXXXX") || die "cannot create a temporary directory"
# mark2 runs in the background (see the watchdog below), and a background
# job ignores SIGINT: on Ctrl+C or TERM stop it explicitly.
# shellcheck disable=SC2329 # invoked from the traps below
stop_mark2() {
    [ -s "$tmp/pid" ] && kill -TERM "$(cat "$tmp/pid")" 2>/dev/null
}
trap 'rm -rf "$tmp"' EXIT
trap 'stop_mark2; exit 130' INT
trap 'stop_mark2; exit 143' TERM

git_diff() {
    git -C "$repo_root" -c core.quotePath=false diff --no-color --no-ext-diff \
        --src-prefix=a/ --dst-prefix=b/ "$@"
}

diff_file=$tmp/diff
stat_file=$tmp/stat
case $mode in
    '')
        what='uncommitted changes (git diff HEAD)'
        git_diff HEAD -- >"$diff_file" || die "git diff HEAD failed"
        git_diff --stat HEAD -- >"$stat_file" || :
        untracked=$(git -C "$repo_root" ls-files --others --exclude-standard | head -n 5)
        [ -z "$untracked" ] ||
            note "untracked files are not reviewed (git add -N <file> to include them): $(echo "$untracked" | tr '\n' ' ')"
        ;;
    --staged | --cached)
        what='staged changes (git diff --cached)'
        git_diff --cached -- >"$diff_file" || die "git diff --cached failed"
        git_diff --stat --cached -- >"$stat_file" || :
        ;;
    -)
        what='diff from stdin'
        cat >"$diff_file" || die "cannot read the diff from stdin"
        git apply --stat <"$diff_file" >"$stat_file" 2>/dev/null || : >"$stat_file"
        ;;
    -*)
        usage >&2
        die "unknown option: $mode"
        ;;
    *..*)
        what="git diff $mode"
        git_diff "$mode" -- >"$diff_file" || die "git diff $mode failed"
        git_diff --stat "$mode" -- >"$stat_file" || :
        ;;
    *)
        git -C "$repo_root" rev-parse --verify --quiet "$mode^{commit}" >/dev/null ||
            die "not a commit: $mode"
        what="git diff $mode..HEAD"
        git_diff "$mode..HEAD" -- >"$diff_file" || die "git diff $mode..HEAD failed"
        git_diff --stat "$mode..HEAD" -- >"$stat_file" || :
        ;;
esac

if ! grep -q '[^[:space:]]' "$diff_file"; then
    echo "nothing to review"
    exit 0
fi

# Number the hunk lines with their line in the new file (removed lines get a
# blank number): the reviewer must cite file:line, and counting from the hunk
# header costs a local model thousands of reasoning tokens and still misses by
# one or two. The body of a hunk is tracked by the counts in its header, so
# "--- "/"+++ " file headers are never taken for hunk lines. LC_ALL=C: awk
# would choke on bytes that are not valid UTF-8, and the program is ASCII-only.
number_lines() {
    LC_ALL=C awk '
    function header(line,    rest, ranges, old, new, part) {
        rest = line
        sub(/^@@ -/, "", rest)
        split(rest, ranges, " ")
        old = ranges[1]
        new = ranges[2]
        sub(/^[+]/, "", new)
        if (index(old, ",")) { split(old, part, ","); old_left = part[2] + 0 } else old_left = 1
        if (index(new, ",")) { split(new, part, ","); line_no = part[1] + 0; new_left = part[2] + 0 }
        else { line_no = new + 0; new_left = 1 }
    }
    /^@@ -[0-9]/ { header($0); print; next }
    (old_left > 0 || new_left > 0) && /^[-+ ]/ {
        marker = substr($0, 1, 1)
        if (marker == "-") { printf "%6s %s\n", "", $0; old_left--; next }
        printf "%6d %s\n", line_no, $0
        line_no++
        new_left--
        if (marker == " ") old_left--
        next
    }
    /^\\/ { printf "%6s %s\n", "", $0; next }
    { print }
    '
}

# Body = stat + numbered diff, cut to the cap. The cut drops the last, possibly
# partial line, so no half-line and no broken UTF-8 sequence reaches the model.
body=$tmp/body
{
    if [ -s "$stat_file" ]; then
        cat "$stat_file"
        echo
    fi
    number_lines <"$diff_file"
} >"$body"
size=$(wc -c <"$body" | tr -d ' ')
truncated=
if [ "$size" -gt "$max_bytes" ]; then
    dd if="$body" bs="$max_bytes" count=1 2>/dev/null | LC_ALL=C sed '$d' >"$tmp/cut"
    mv "$tmp/cut" "$body"
    truncated=yes
    note "diff is $size bytes, cut to $max_bytes (MARK2_REVIEW_MAX_DIFF_BYTES)"
fi

# No UTF-8 sanitizing here: macOS iconv rejects valid Cyrillic text at its
# buffer boundaries. A diff that is not UTF-8 (a Latin-1 file) makes mark2
# refuse the prompt on stdin, and the review fails loudly with exit 1.

{
    cat "$prompt_file"
    printf '\n---\n\nDiff to review: %s.\n\n===== BEGIN DIFF =====\n' "$what"
    cat "$body"
    if [ -n "$truncated" ]; then
        printf '\n[diff truncated: %s of %s bytes shown; read the files whose changes are cut off]\n' \
            "$max_bytes" "$size"
    fi
    printf '===== END DIFF =====\n\n'
    printf 'Review the diff above. Your last line must be exactly VERDICT: approve or VERDICT: needs-work.\n'
} >"$tmp/prompt"

# exec: run in the background, the subshell becomes mark2 itself, so $! is
# the pid the watchdog and the traps signal (cargo run execs mark2 as well).
run_mark2() {
    if [ -n "${MARK2_REVIEW_BIN:-}" ]; then
        # shellcheck disable=SC2086 # split on purpose: the value may carry arguments
        exec $MARK2_REVIEW_BIN "$@"
    elif [ -x "$tool_root/target/release/mark2" ]; then
        exec "$tool_root/target/release/mark2" "$@"
    else
        exec cargo run -q --manifest-path "$tool_root/Cargo.toml" -p mark2-cli -- "$@"
    fi
}

# watchdog PID: TERM the run after timeout_secs. A local model can reason in
# circles for half an hour on a large diff; CI must not hang on that. Polls
# once a second, so it ends right after mark2 does and leaves no stray sleep.
watchdog() {
    [ "$timeout_secs" -gt 0 ] || return 0
    waited=0
    while kill -0 "$1" 2>/dev/null; do
        if [ "$waited" -ge "$timeout_secs" ]; then
            : >"$tmp/timed_out"
            kill -TERM "$1" 2>/dev/null
            return 0
        fi
        sleep 1
        waited=$((waited + 1))
    done
}

# The last line that is nothing but a verdict wins; markdown decoration and
# letter case are forgiven, local models add them.
parse_verdict() {
    # shellcheck disable=SC2016 # the backticks below are literal markdown
    tr -d '\r' <"$1" | tr '[:upper:]' '[:lower:]' |
        sed -n 's/^[[:space:]#>*`]*verdict:[[:space:]*`]*\([a-z-]*\)[[:space:]*`.]*$/\1/p' |
        tail -n 1
}

note "reviewing $what with $model at $base_url"
answer=$tmp/answer
# A local model sometimes lists its findings and stops without the verdict
# line; one fresh run usually fixes that. mark2 failures are not retried.
attempt=1
while :; do
    # Stream the answer to stdout and keep a copy; POSIX sh has no pipefail,
    # so mark2's exit status travels through a file. The prompt goes on stdin
    # ("-p -"): a single argv string is capped at 128 KiB on Linux.
    {
        run_mark2 -p --policy read-only -C "$repo_root" \
            --base-url "$base_url" --model "$model" - <"$tmp/prompt" &
        pid=$!
        echo "$pid" >"$tmp/pid"
        # exec, not a redirect on the call: dash keeps a saved copy of the
        # pipe in a function redirect, and tee would wait for the watchdog.
        (
            exec >/dev/null 2>&1
            watchdog "$pid"
        ) &
        # 2>/dev/null: the shell would report the job the watchdog killed.
        wait "$pid" 2>/dev/null
        echo $? >"$tmp/status"
    } | tee "$answer"
    status=$(cat "$tmp/status" 2>/dev/null || echo 1)
    [ ! -f "$tmp/timed_out" ] ||
        die "the review took longer than $timeout_secs s and was stopped (MARK2_REVIEW_TIMEOUT_SECS)"
    [ "$status" -eq 0 ] || die "mark2 exited with status $status"
    verdict=$(parse_verdict "$answer")
    case $verdict in
        approve | needs-work) break ;;
    esac
    [ "$attempt" -lt 2 ] || break
    attempt=$((attempt + 1))
    note "the answer has no VERDICT line, reviewing once more"
    printf '\n--- retry: the answer above had no VERDICT line ---\n\n'
done
serious=$(grep -Eic '\[(blocker|major)\]' "$answer")

case $verdict in
    approve)
        [ "$serious" -eq 0 ] || note "warning: verdict is approve, yet blocker/major findings are listed"
        exit 0
        ;;
    needs-work)
        [ "$serious" -gt 0 ] || note "warning: verdict is needs-work without any blocker/major finding"
        exit 2
        ;;
    *)
        die "the answer has no 'VERDICT: approve' or 'VERDICT: needs-work' line"
        ;;
esac
