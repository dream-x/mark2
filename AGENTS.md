# mark2

Собственный agent harness на Rust: **ядро-библиотека + headless CLI**, заточенный под
запуск **локальных моделей** (llama.cpp / ollama / LM Studio на Apple Silicon).
Ядро UI-agnostic: принимает `Op` (submission), стримит `Event` — CLI лишь первый
потребитель этого канала, TUI/ACP/сервер придут позже. Первый провайдер —
OpenAI-compatible. План: `docs/plans/20260824-rust-agent-harness.md`.

## Структура

- `crates/core` (`mark2-core`) — нейтральные типы сообщений и событий, trait `Provider`,
  agent loop, тулы (bash/read/grep/write/edit), сессии JSONL, компакция, конфиг.
- `crates/providers` (`mark2-providers`) — реализации провайдеров: `openai_compat`,
  SSE-парсинг, JSON-repair для tool-аргументов слабых моделей.
- `crates/cli` (`mark2-cli`, бинарь `mark2`) — clap, интерактив и `-p` print mode,
  рендер Event-стрима.

Общие зависимости объявлены в корневом `[workspace.dependencies]`; крейты подключают их
через `dep.workspace = true`, свои версии не заводим.

## Команды

- Сборка: `cargo build`
- Запуск: `cargo run -q -p mark2-cli -- "задача"`
- Проверка перед коммитом:
  `cargo fmt --check && cargo clippy --all-targets -- -D warnings && cargo test`
- Ревью диффа локальной моделью (по желанию, нужен ollama): `scripts/review.sh`
  (exit 0 — approve, 2 — needs-work, 1 — сбой); правишь саму обёртку — гоняй
  `sh scripts/test-review.sh`.

Тесты обязательны в каждой задаче: код и тесты пишутся в рамках одного таска.

Значимое изменение сопровождается записью в `CHANGELOG.md` в формате
«что сделано ← почему → к чему привело (измеримо)»; причины и сроки годности
контролей харнеса — в `docs/decisions.md`.

## Запуск против локальной модели

Дефолтный `base_url` — `http://localhost:8080/v1` (llama-server). Если сервер
другой (или, как бывает, порт 8080 занят посторонним веб-сервером — тогда в
stderr прилетит HTML вместо ответа), указывай базу явно:

```sh
# ollama: OpenAI-совместимый префикс — /v1, порт 11434
cargo run -q -p mark2-cli -- -p --yolo \
  --base-url http://localhost:11434/v1 --model qwen3.5:9b \
  -C /tmp/scratch "Create a file hello.txt with content exactly: mark2 works"
```

`-p` — print mode (ответ в stdout, служебное в stderr), `--yolo` — не спрашивать
подтверждений, `-C` — рабочая директория агента. Постоянные настройки удобнее
держать в `~/.config/mark2/config.toml` (секции `[provider]`, `[agent]`).

E2E-тесты против живого сервера помечены `#[ignore]` и по умолчанию не
запускаются:

```sh
cargo test -p mark2-cli --test e2e_local -- --ignored
```

Куда ходить — из окружения: `MARK2_E2E_BASE_URL` (дефолт
`http://localhost:11434/v1`), `MARK2_E2E_MODEL` (дефолт `qwen3.5:9b`),
`MARK2_E2E_TIMEOUT_SECS` (дефолт 300). Наблюдения с реальных прогонов —
`docs/notes/e2e-findings-20260824.md`.
