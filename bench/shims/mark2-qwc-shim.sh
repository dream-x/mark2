#!/bin/sh
# Шим «Qwen Code вместо mark2» для бенч-ранера: колонка чужого харнесса
# (CLI `qwen`, Qwen Code 0.27.2) на тех же задачах и с тем же verify.sh.
#
# Ранер (crates/bench/src/runner.rs, mark2_args) зовёт бинарь из --mark2 так:
#   <shim> -p (--yolo | --policy read-only) --base-url URL --model M -C CWD \
#          [--api-key-env VAR] PROMPT
# с cwd = CWD, HOME = tmp-песочница прогона, stdin = /dev/null; stdout и
# stderr пишутся в файлы, verify.sh читает stdout (MARK2_STDOUT) — поэтому в
# stdout попадает только финальный ответ модели (`qwen` в текстовом режиме
# печатает туда только его, предупреждения идут в stderr).
#
# HOME остаётся песочницей, как у mark2 (её видит и bash-тул qwen, и в
# системный промпт не попадают личные скиллы из ~/.agents/skills), а логин
# подписки и settings.json qwen читает из настоящего ~/.qwen через QWEN_HOME.
# Рабочие данные qwen (QWEN_RUNTIME_DIR) и его system-defaults — в песочнице.
#
# Запуск колонки (модель — алиас qwen38max из bench/models.toml, base_url
# шим игнорирует: Qwen Code ходит по своей подписке из ~/.qwen):
#   target/debug/mark2-bench --mark2 bench/shims/mark2-qwc-shim.sh \
#     --models qwen38max --tasks T01,T02,T16 --label qwc-qwen38max
set -eu

die() {
    echo "mark2-qwc-shim: $*" >&2
    exit 2
}

approval=
model=
cwd=
prompt=
have_prompt=

while [ $# -gt 0 ]; do
    # Промпт ранер всегда ставит последним — и он может начинаться с «-».
    if [ $# -eq 1 ] && [ -z "$have_prompt" ]; then
        prompt=$1
        have_prompt=1
        shift
        continue
    fi
    case $1 in
        -p)
            shift
            ;;
        --yolo)
            approval=yolo
            shift
            ;;
        --policy)
            [ $# -ge 2 ] || die "--policy без значения"
            case $2 in
                # plan у Qwen Code — «только анализ, ничего не менять и не
                # запускать»: аналог read-only mark2.
                read-only) approval=plan ;;
                yolo) approval=yolo ;;
                *) die "неизвестная политика: $2" ;;
            esac
            shift 2
            ;;
        --model)
            [ $# -ge 2 ] || die "--model без значения"
            model=$2
            shift 2
            ;;
        -C)
            [ $# -ge 2 ] || die "-C без значения"
            cwd=$2
            shift 2
            ;;
        --base-url | --api-key-env)
            # Не нужны: эндпоинт и логин берутся из ~/.qwen пользователя.
            [ $# -ge 2 ] || die "$1 без значения"
            shift 2
            ;;
        -*)
            die "неизвестный флаг: $1"
            ;;
        *)
            [ -z "$have_prompt" ] || die "лишний позиционный аргумент"
            prompt=$1
            have_prompt=1
            shift
            ;;
    esac
done

[ -n "$have_prompt" ] || die "нет промпта (последний позиционный аргумент)"
[ -n "$approval" ] || die "нужен --yolo или --policy read-only"

if [ -n "$cwd" ]; then
    cd -- "$cwd" || die "не удалось перейти в $cwd"
fi

# Настоящий домашний каталог — из базы учёток (getpwnam через ~user), а не из
# окружения: ранер подменил HOME на песочницу, а логин подписки
# (~/.qwen/subscription_credentials.json) и settings.json лежат в настоящем.
# HOME не трогаем — на настоящий ~/.qwen указывает QWEN_HOME (из него qwen
# берёт логин, настройки, output-language.md). Сами файлы ~/.qwen не копируем
# и не меняем: токен ротируется, копия разлогинила бы пользователя.
user=$(id -un) || die "не удалось узнать имя пользователя"
case $user in
    '' | *[!A-Za-z0-9._-]*) die "неожиданное имя пользователя: $user" ;;
esac
real_home=
eval "real_home=~$user"
[ -d "$real_home/.qwen" ] || die "нет $real_home/.qwen — Qwen Code не залогинен"
export QWEN_HOME="$real_home/.qwen"

# Рабочие данные Qwen Code (projects/<cwd>, tmp, авто-память memories/,
# usage, debug) — в песочницу прогона: иначе каждый tmp-cwd оставлял бы
# каталог в ~/.qwen/projects (даже с --no-chat-recording). Ранер удаляет
# песочницу сам.
#
# Там же — system-defaults (слой ниже ~/.qwen/settings.json, его настройки не
# перекрывает):
# - general.preventSystemSleep = false: иначе на время ответа qwen запускает
#   дочерний `caffeinate -is`, а по SIGKILL на таймауте тот осиротеет и будет
#   держать Mac без сна до перезагрузки;
# - memory.enableManagedAutoMemory/enableManagedAutoDream = false: после
#   ответа qwen ждёт фоновое извлечение памяти — лишние LLM-вызовы, которые
#   жгут квоту подписки и добавляют ~4.5 с к каждому прогону, а результат всё
#   равно выбрасывается вместе с песочницей.
#
# При ручном запуске (HOME настоящий) — поведение Qwen Code по умолчанию.
sandbox_home=${HOME:-}
if [ -n "$sandbox_home" ] && [ "$sandbox_home" != "$real_home" ]; then
    export QWEN_RUNTIME_DIR="$sandbox_home/.qwen"
    defaults=$sandbox_home/qwen-system-defaults.json
    printf '%s\n' '{"general": {"preventSystemSleep": false},' \
        ' "memory": {"enableManagedAutoMemory": false,' \
        '            "enableManagedAutoDream": false}}' >"$defaults" ||
        die "не удалось записать $defaults"
    export QWEN_CODE_SYSTEM_DEFAULTS_PATH="$defaults"
fi

# Не проверять обновления: версия харнесса не должна смениться посреди колонки.
export QWEN_CODE_SKIP_UPDATE_CHECK_ONCE=true

# exec, а не дочерний процесс: по таймауту ранер убивает свой непосредственный
# дочерний процесс, им должен быть сам qwen (node запускает CLI в том же
# процессе). Промпт — через --prompt=: позиционный аргумент с ведущим «-»
# yargs принял бы за флаг, а после «--» qwen промпт не видит вовсе; флаг
# помечен deprecated, но в 0.27.2 работает молча. --no-chat-recording — не
# писать историю чата (сессии для --resume бенчу не нужны).
set -- --approval-mode "$approval" --no-chat-recording
[ -z "$model" ] || set -- "$@" -m "$model"
exec qwen "$@" "--prompt=$prompt"
