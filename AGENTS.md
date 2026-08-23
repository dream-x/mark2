# mark2

Собственный agent harness на Rust: **ядро-библиотека + headless CLI**, заточенный под
запуск **локальных моделей** (llama.cpp / ollama / LM Studio на Apple Silicon).
Ядро UI-agnostic: принимает `Op` (submission), стримит `Event` — CLI лишь первый
потребитель этого канала, TUI/ACP/сервер придут позже. Первый провайдер —
OpenAI-compatible. План: `docs/plans/20260824-rust-agent-harness.md`.

## Структура

- `crates/core` (`mark2-core`) — нейтральные типы сообщений и событий, trait `Provider`,
  agent loop, тулы (bash/read/write/edit), сессии JSONL, компакция, конфиг.
- `crates/providers` (`mark2-providers`) — реализации провайдеров: `openai_compat`,
  SSE-парсинг, JSON-repair для tool-аргументов слабых моделей.
- `crates/cli` (`mark2-cli`, бинарь `mark2`) — clap, интерактив и `-p` print mode,
  рендер Event-стрима.

Общие зависимости объявлены в корневом `[workspace.dependencies]`; крейты подключают их
через `dep.workspace = true`, свои версии не заводим.

## Команды

- Сборка: `cargo build`
- Запуск: `cargo run -p mark2-cli`
- Проверка перед коммитом:
  `cargo fmt --check && cargo clippy --all-targets -- -D warnings && cargo test`

Тесты обязательны в каждой задаче: код и тесты пишутся в рамках одного таска.
