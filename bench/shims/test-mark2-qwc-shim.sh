#!/bin/sh
# Офлайн-тесты bench/shims/mark2-qwc-shim.sh — шима, через который бенч-ранер
# гоняет Qwen Code вместо mark2. Настоящий `qwen` не нужен: первым в PATH стоит
# фейковый, он записывает свой argv (каждый аргумент в отдельный файл, байт в
# байт), окружение, cwd и PID и печатает заготовленный ответ. Сеть не нужна;
# настоящий ~/.qwen шим только проверяет на существование, в настоящий $HOME
# тест ничего не пишет.
#
#   sh bench/shims/test-mark2-qwc-shim.sh
#   SHIM_SHELL=dash sh bench/shims/test-mark2-qwc-shim.sh   (шим под другим sh)
#
# По умолчанию шим запускается напрямую, через свой shebang, — так его зовёт
# ранер (crates/bench/src/runner.rs).

# Часть проверок передаётся в eval в одинарных кавычках намеренно, и в
# промптах «$HOME» и обратные кавычки — буквальный текст.
# shellcheck disable=SC2016

set -eu

here=$(CDPATH='' cd -- "$(dirname -- "$0")" && pwd)
shim=$here/mark2-qwc-shim.sh
shim_shell=${SHIM_SHELL:-}
work=$(mktemp -d "${TMPDIR:-/tmp}/mark2-qwc-shim-test.XXXXXX")
trap 'rm -rf "$work"' EXIT
# Физический путь: cwd фейка сравнивается с `pwd -P`, а на macOS $TMPDIR лежит
# под симлинком /var → /private/var.
work=$(cd -- "$work" && pwd -P)

failures=0
checks=0
pass() {
    checks=$((checks + 1))
    printf 'ok   %s\n' "$1"
}
fail() {
    checks=$((checks + 1))
    failures=$((failures + 1))
    printf 'FAIL %s\n' "$1"
}
check() { # check NAME COMMAND...
    name=$1
    shift
    if "$@"; then pass "$name"; else fail "$name"; fi
}
# summary: итог; код возврата 0, только если все проверки прошли. Возвращает,
# а не делает exit сам: из-за всегда выходящей функции в конце скрипта
# ShellCheck 0.11 считает предикаты, вызываемые через check "$@",
# недостижимыми (SC2329).
summary() {
    echo
    if [ "$failures" -eq 0 ]; then
        echo "all $checks mark2-qwc-shim.sh tests passed"
        return 0
    fi
    echo "$failures of $checks mark2-qwc-shim.sh tests failed"
    return 1
}

if ! command -v python3 >/dev/null; then
    echo "python3 is required (real home lookup, JSON check)" >&2
    exit 1
fi

# Фейковый Qwen Code. Каждый аргумент — в свой файл arg.N (байт в байт: с
# пробелами, кавычками и переводами строк), их число — в argc, они же по
# строке — в args для grep. Переменная окружения пишется в env.ИМЯ, только
# если она задана, — так «не задана» отличается от «пустая». Ещё — свой PID
# ($$), физический cwd, ответ FAKE_QWEN_ANSWER в stdout и код выхода
# FAKE_QWEN_STATUS.
mkdir "$work/bin"
cat >"$work/bin/qwen" <<'EOF'
#!/bin/sh
rec=$FAKE_QWEN_DIR
mkdir -p "$rec"
: >"$rec/args"
n=0
for arg in "$@"; do
    n=$((n + 1))
    printf '%s' "$arg" >"$rec/arg.$n"
    printf '%s\n' "$arg" >>"$rec/args"
done
echo "$n" >"$rec/argc"
[ -z "${HOME+set}" ] || printf '%s' "$HOME" >"$rec/env.HOME"
[ -z "${QWEN_HOME+set}" ] || printf '%s' "$QWEN_HOME" >"$rec/env.QWEN_HOME"
[ -z "${QWEN_RUNTIME_DIR+set}" ] ||
    printf '%s' "$QWEN_RUNTIME_DIR" >"$rec/env.QWEN_RUNTIME_DIR"
[ -z "${QWEN_CODE_SYSTEM_DEFAULTS_PATH+set}" ] ||
    printf '%s' "$QWEN_CODE_SYSTEM_DEFAULTS_PATH" >"$rec/env.QWEN_CODE_SYSTEM_DEFAULTS_PATH"
[ -z "${QWEN_CODE_SKIP_UPDATE_CHECK_ONCE+set}" ] ||
    printf '%s' "$QWEN_CODE_SKIP_UPDATE_CHECK_ONCE" >"$rec/env.QWEN_CODE_SKIP_UPDATE_CHECK_ONCE"
printf '%s' "${PWD-}" >"$rec/env.PWD"
pwd -P >"$rec/cwd"
echo "$$" >"$rec/pid"
printf '%s\n' "$FAKE_QWEN_ANSWER"
exit "${FAKE_QWEN_STATUS:-0}"
EOF
chmod +x "$work/bin/qwen"

# Фейковый `id` — только для проверок без логина Qwen Code: вместо имени
# пользователя печатает FAKE_ID_USER.
mkdir "$work/bin-id"
cat >"$work/bin-id/id" <<'EOF'
#!/bin/sh
printf '%s\n' "$FAKE_ID_USER"
EOF
chmod +x "$work/bin-id/id"

rec=$work/rec
sandbox=$work/home
proj=$work/proj
mkdir "$proj"
FAKE_QWEN_DIR=$rec
FAKE_QWEN_ANSWER='hello.txt created'
FAKE_QWEN_STATUS=0
FAKE_ID_USER=nobody
export FAKE_QWEN_DIR FAKE_QWEN_ANSWER FAKE_QWEN_STATUS FAKE_ID_USER
# Значения снаружи не должны ни подыгрывать проверкам, ни ломать их.
unset QWEN_HOME QWEN_RUNTIME_DIR QWEN_CODE_SYSTEM_DEFAULTS_PATH \
    QWEN_CODE_SKIP_UPDATE_CHECK_ONCE
printf '%s\n' "$FAKE_QWEN_ANSWER" >"$work/answer"
run_home=$sandbox
run_path=$work/bin:$PATH

# Предохранитель: если по PATH нашёлся бы настоящий qwen, тест сжёг бы квоту
# подписки — тогда не запускаем ничего.
if [ "$(env PATH="$run_path" sh -c 'command -v qwen')" != "$work/bin/qwen" ]; then
    echo "the fake qwen is not first on PATH, refusing to run the shim" >&2
    exit 1
fi

# Настоящий домашний каталог — из базы учёток, независимо от шима (тот берёт
# его через ~user).
real_home=$(python3 -I -c 'import os, pwd; print(pwd.getpwuid(os.getuid()).pw_dir)')

# run [ARGS...]: запустить шим из $work (в каталог задачи он переходит сам, по
# -C) с HOME=$run_home и PATH=$run_path; песочница $sandbox каждый раз новая.
# Ставит $code; stdout и stderr шима — в $work/out и $work/err.
run() {
    rm -rf "$rec" "$sandbox"
    mkdir "$sandbox"
    code=0
    (cd "$work" && HOME=$run_home PATH=$run_path ${shim_shell:+"$shim_shell"} "$shim" "$@") \
        >"$work/out" 2>"$work/err" </dev/null || code=$?
}
not() { ! "$@"; }
called() { [ -f "$rec/pid" ]; }
code_is() { [ "$code" -eq "$1" ]; }
# refused: шим отказался (exit 2, как у его die) и до qwen дело не дошло.
refused() { code_is 2 && not called; }
err_has() { grep -qF -- "$1" "$work/err"; }
stdout_is_answer() { cmp -s "$work/answer" "$work/out"; }
has_arg() { grep -qxF -- "$1" "$rec/args"; }
# arg_after FLAG VALUE: в argv фейка за FLAG идёт VALUE.
arg_after() {
    called || return 1
    [ "$(awk -v flag="$1" 'prev == flag { print; exit } { prev = $0 }' "$rec/args")" = "$2" ]
}
# no_arg ARG...: фейк вызван, и ни одного ARG среди его аргументов нет.
no_arg() {
    called || return 1
    for unwanted in "$@"; do
        if has_arg "$unwanted"; then return 1; fi
    done
}
# no_text TEXT: фейк вызван, и TEXT нет ни в одном его аргументе.
no_text() { called && not grep -qF -- "$1" "$rec/args"; }
# argv_is ARG...: argv фейка — ровно ARG..., поэлементно и байт в байт.
argv_is() {
    called || return 1
    [ "$(cat "$rec/argc")" -eq $# ] || return 1
    i=0
    for want in "$@"; do
        i=$((i + 1))
        printf '%s' "$want" >"$work/want"
        cmp -s "$work/want" "$rec/arg.$i" || return 1
    done
}
# last_arg_is TEXT: последний аргумент фейка — TEXT байт в байт.
last_arg_is() {
    called || return 1
    printf '%s' "$1" >"$work/want"
    cmp -s "$work/want" "$rec/arg.$(cat "$rec/argc")"
}
env_is() { [ -f "$rec/env.$1" ] && [ "$(cat "$rec/env.$1")" = "$2" ]; }
env_unset() { called && [ ! -e "$rec/env.$1" ]; }
cwd_is() { called && [ "$(cat "$rec/cwd")" = "$(cd -- "$1" && pwd -P)" ]; }
pid_is() { called && [ "$(cat "$rec/pid")" = "$1" ]; }
# defaults_in_sandbox: QWEN_CODE_SYSTEM_DEFAULTS_PATH — существующий файл
# внутри песочницы HOME.
defaults_in_sandbox() {
    [ -f "$rec/env.QWEN_CODE_SYSTEM_DEFAULTS_PATH" ] || return 1
    defaults=$(cat "$rec/env.QWEN_CODE_SYSTEM_DEFAULTS_PATH")
    case $defaults in
        "$sandbox"/*) [ -f "$defaults" ] ;;
        *) return 1 ;;
    esac
}
# defaults_json_ok: этот файл — валидный JSON, и в нём ровно false (не 0, не
# "false") у general.preventSystemSleep и обоих флагов авто-памяти.
defaults_json_ok() {
    defaults_in_sandbox || return 1
    python3 -I -c '
import json, sys
with open(sys.argv[1]) as f:
    d = json.load(f)
sys.exit(0 if d["general"]["preventSystemSleep"] is False
         and d["memory"]["enableManagedAutoMemory"] is False
         and d["memory"]["enableManagedAutoDream"] is False else 1)
' "$defaults" 2>/dev/null
}

url=http://127.0.0.1:1/v1
model=qwen3.8-max

check "the shim is executable (the runner runs it directly)" test -x "$shim"

# --- ошибки аргументов: отказ до qwen, ~/.qwen для них не нужен ---
run -p --yolo --base-url "$url" --model "$model" -C "$proj"
check "no prompt: exit 2, qwen not called" refused
check "no prompt: stderr says why" err_has 'нет промпта'
run -p --base-url "$url" --model "$model" -C "$proj" 'Create hello.txt'
check "neither --yolo nor --policy: exit 2, qwen not called" refused
run -p --policy auto --base-url "$url" --model "$model" -C "$proj" 'Create hello.txt'
check "unknown policy: exit 2, qwen not called" refused
run -p --yolo --temperature 0 --base-url "$url" --model "$model" -C "$proj" 'Create hello.txt'
check "unknown flag: exit 2, qwen not called" refused
run -p --yolo stray --model "$model" -C "$proj" 'Create hello.txt'
check "stray positional argument: exit 2, qwen not called" refused
run -p --yolo --model "$model" -C "$work/missing" 'Create hello.txt'
check "missing -C directory: exit 2, qwen not called" refused

# --- без логина Qwen Code и с подозрительным именем пользователя ---
# Фейковый `id` подменяет имя: у nobody нет ~/.qwen (на macOS его дом —
# /var/empty, на Linux — /nonexistent).
run_path=$work/bin-id:$work/bin:$PATH
FAKE_ID_USER=nobody
run -p --yolo --base-url "$url" --model "$model" -C "$proj" 'Create hello.txt'
check "no ~/.qwen (user nobody): exit 2, qwen not called" refused
check "no ~/.qwen: stderr names it" err_has '/.qwen'
# Имя пользователя попадает в eval (ради ~user) — шим обязан отвергнуть его
# раньше, чем что-то из него выполнится.
FAKE_ID_USER='x;touch pwned-by-id'
run -p --yolo --base-url "$url" --model "$model" -C "$proj" 'Create hello.txt'
check "odd user name: exit 2, qwen not called" refused
check "odd user name: nothing from it is executed" not test -e "$proj/pwned-by-id"
FAKE_ID_USER=nobody
run_path=$work/bin:$PATH

# Дальше шим доходит до exec, а для этого ему нужен настоящий ~/.qwen.
if [ ! -d "$real_home/.qwen" ]; then
    echo "SKIP the rest: $real_home/.qwen is missing (the shim needs a logged-in Qwen Code)"
    summary
    exit $?
fi

# --- yolo: аргументы для qwen ---
run -p --yolo --base-url "$url" --model "$model" -C "$proj" 'Create hello.txt'
check "yolo: exit 0" code_is 0
check "yolo: --approval-mode yolo" arg_after --approval-mode yolo
check "yolo: --no-chat-recording" eval 'called && has_arg --no-chat-recording'
check "yolo: -m carries the model" arg_after -m "$model"
check "yolo: the last argument is exactly --prompt=PROMPT" last_arg_is '--prompt=Create hello.txt'
check "yolo: the whole argv" \
    argv_is --approval-mode yolo --no-chat-recording -m "$model" '--prompt=Create hello.txt'
check "yolo: -p, --base-url and --api-key-env are not passed on" \
    no_arg -p --base-url --api-key-env "$url"
check "yolo: the URL is not passed on in any form" no_text 127.0.0.1
check "yolo: qwen runs in the -C directory" cwd_is "$proj"
check "yolo: stdout is qwen's answer and nothing else" stdout_is_answer

# --- read-only ---
run -p --policy read-only --base-url "$url" --model "$model" -C "$proj" 'Explain lib.rs'
check "read-only: exit 0" code_is 0
check "read-only: --approval-mode plan" arg_after --approval-mode plan
check "read-only: no yolo anywhere in argv" no_text yolo

# --- промпт доходит байт в байт ---
# Пробелы (в том числе ведущие и хвостовые), обе кавычки, $, обратные кавычки,
# glob-символы, обратные слэши, перевод строки внутри и кириллица.
tricky='  Say "hi" and '\''bye'\'' to $HOME, `id`, * ? [x] \n \\
second line — кириллица, trailing spaces  '
run -p --yolo --base-url "$url" --model "$model" -C "$proj" "$tricky"
check "tricky prompt: exit 0" code_is 0
check "tricky prompt: one argument, byte for byte" \
    argv_is --approval-mode yolo --no-chat-recording -m "$model" "--prompt=$tricky"
# Промпт с ведущим «-» — всё равно промпт (ранер ставит его последним), а не
# флаг: --yolo в конце не превращает read-only в yolo.
run -p --policy read-only --base-url "$url" --model "$model" -C "$proj" --yolo
check "prompt that looks like a flag: still the prompt" \
    eval 'code_is 0 && last_arg_is --prompt=--yolo && arg_after --approval-mode plan'

# --- --api-key-env принимается и отбрасывается ---
run -p --yolo --base-url "$url" --model "$model" -C "$proj" --api-key-env SOMETHING 'Create hello.txt'
check "api-key-env: accepted (exit 0)" code_is 0
check "api-key-env: dropped together with its value" \
    argv_is --approval-mode yolo --no-chat-recording -m "$model" '--prompt=Create hello.txt'

# --- без --model нет и -m ---
run -p --yolo -C "$proj" 'Create hello.txt'
check "no --model: no -m either" \
    argv_is --approval-mode yolo --no-chat-recording '--prompt=Create hello.txt'

# --- код выхода qwen — код выхода шима ---
FAKE_QWEN_STATUS=3
run -p --yolo --base-url "$url" --model "$model" -C "$proj" 'Create hello.txt'
FAKE_QWEN_STATUS=0
check "qwen's exit status passes through" eval 'code_is 3 && stdout_is_answer'

# --- HOME — песочница прогона: рабочие данные qwen изолированы ---
run -p --yolo --base-url "$url" --model "$model" -C "$proj" 'Create hello.txt'
check "sandbox: exit 0" code_is 0
check "sandbox: HOME stays the sandbox" env_is HOME "$sandbox"
check "sandbox: QWEN_HOME is the real ~/.qwen" env_is QWEN_HOME "$real_home/.qwen"
check "sandbox: QWEN_RUNTIME_DIR is <sandbox>/.qwen" env_is QWEN_RUNTIME_DIR "$sandbox/.qwen"
check "sandbox: system defaults are a file inside the sandbox" defaults_in_sandbox
check "sandbox: defaults turn off preventSystemSleep and managed auto-memory/dream" \
    defaults_json_ok
check "sandbox: update check skipped" env_is QWEN_CODE_SKIP_UPDATE_CHECK_ONCE true
# Значения, унаследованные от окружения ранера, шим перекрывает своими.
QWEN_HOME=$work/inherited-home
QWEN_RUNTIME_DIR=$work/inherited-runtime
QWEN_CODE_SYSTEM_DEFAULTS_PATH=$work/inherited.json
export QWEN_HOME QWEN_RUNTIME_DIR QWEN_CODE_SYSTEM_DEFAULTS_PATH
run -p --yolo --base-url "$url" --model "$model" -C "$proj" 'Create hello.txt'
unset QWEN_HOME QWEN_RUNTIME_DIR QWEN_CODE_SYSTEM_DEFAULTS_PATH
check "sandbox: inherited QWEN_* values are replaced" \
    eval 'env_is QWEN_HOME "$real_home/.qwen" && env_is QWEN_RUNTIME_DIR "$sandbox/.qwen" &&
        defaults_in_sandbox'

# --- HOME настоящий (ручной запуск): без изоляции ---
real_defaults=$real_home/qwen-system-defaults.json
if [ -e "$real_defaults" ]; then
    real_defaults_preexisting=yes
else
    real_defaults_preexisting=
fi
run_home=$real_home
run -p --yolo --base-url "$url" --model "$model" -C "$proj" 'Create hello.txt'
run_home=$sandbox
check "real HOME: exit 0" code_is 0
check "real HOME: HOME is left as is" env_is HOME "$real_home"
check "real HOME: QWEN_HOME is still the real ~/.qwen" env_is QWEN_HOME "$real_home/.qwen"
check "real HOME: QWEN_RUNTIME_DIR is not set" env_unset QWEN_RUNTIME_DIR
check "real HOME: QWEN_CODE_SYSTEM_DEFAULTS_PATH is not set" \
    env_unset QWEN_CODE_SYSTEM_DEFAULTS_PATH
# Если шим всё-таки написал system-defaults в настоящий HOME (а до теста файла
# там не было), это провал — и файл убираем, чтобы регрессия не оставила мусор
# у пользователя.
if [ -n "$real_defaults_preexisting" ] || [ ! -e "$real_defaults" ]; then
    pass "real HOME: no defaults file is written into the real home"
else
    fail "real HOME: no defaults file is written into the real home (removed it)"
    rm -f -- "$real_defaults"
fi
check "real HOME: update check still skipped" env_is QWEN_CODE_SKIP_UPDATE_CHECK_ONCE true

# --- exec: qwen — это сам процесс шима ---
# По таймауту ранер убивает свой непосредственный дочерний процесс; запусти шим
# qwen дочерним, а не через exec, убит был бы только шим, а qwen жил бы дальше.
# Шим запускается фоном без промежуточной подоболочки, так что $! — его PID.
rm -rf "$rec" "$sandbox"
mkdir "$sandbox"
code=0
HOME=$sandbox PATH=$run_path ${shim_shell:+"$shim_shell"} "$shim" -p --yolo --base-url "$url" \
    --model "$model" -C "$proj" 'Create hello.txt' >"$work/out" 2>"$work/err" </dev/null &
shim_pid=$!
wait "$shim_pid" || code=$?
check "exec: exit 0" code_is 0
check "exec: qwen runs as the shim's own process (\$\$ == \$!)" pid_is "$shim_pid"

summary
