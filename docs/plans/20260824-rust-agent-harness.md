# mark2 — собственный agent harness на Rust

## Context

Цель — свой харнесс уровня «ядро codex/pi», заточенный под запуск **локальных моделей** (llama.cpp / ollama / LM Studio на Apple Silicon), с архитектурой, которая не мешает потом дорасти до TUI, MCP, ACP-фронтендов и песочницы. Решения зафиксированы с пользователем: **ядро-библиотека + headless CLI**, провайдер **OpenAI-compatible** первым, MVP — **минимальный цикл**, тесты — regular (код, затем тесты в той же задаче).

План основан на ресёрче четырёх источников (отчёты агентов, август 2026):
- **codex-rs**: SQ/EQ-протокол (`Op` in / `Event` out), CancellationToken + `.or_cancel()`, FuturesOrdered для тулов, оркестратор «approval → sandbox → escalate», rollout JSONL. Анти-пример: 130 крейтов, turn.rs на 2 791 строку.
- **goose**: минимальный Provider-trait (единственный обязательный метод `stream()`), rmcp-типы как внутренняя валюта тул-коллов, toolshim для моделей без tool calling, компакция через флаги видимости, ACP вместо собственного REST. Анти-пример: agent.rs на 6 031 строку.
- **Эссе (pi, amp, aider, opencode, mini-swe-agent, Böckeler)**: 4 тулов достаточно; системный промпт + тулы ≤ 1 000 токенов; edit-формат — стратегия per-model (exact search/replace ↔ whole-file для слабых моделей); сенсоры (post-edit диагностика) — самый дешёвый выигрыш в качестве; AGENTS.md — стандарт.
- **Rust-экосистема**: ручной тонкий HTTP-слой (reqwest + serde) вместо rig/genai — нужен точный контроль wire-формата и кэширования; eventsource-stream (не reqwest-eventsource — конфликт с reqwest 0.13); rmcp 3.x, schemars 1.x + jsonschema, tokio-util, landlock/seccompiler + seatbelt; llama-server `--jinja` как основной локальный бекенд.

## Ключевые архитектурные решения

1. **SQ/EQ-ядро, UI-agnostic**: ядро принимает `Op` (submission) и стримит `Event` (плоский serde-tagged enum). CLI — первый потребитель; TUI/ACP/сервер — будущие потребители того же канала. Это единственная абстракция, которая переживает все рефакторинги (урок codex).
2. **Свой нейтральный формат сообщений** (`Message` / `ContentBlock`: Text, Thinking, ToolUse, ToolResult, Image). Wire-диалекты — чистые функции `to_wire` / `parse_stream_chunk` на модуль провайдера. Сессия сериализуется в нейтральном формате → смена провайдера без потери истории (урок pi/goose).
3. **Provider trait минимальный**: `fn name()` + `async fn stream(req) -> MessageStream`. Всё остальное — дефолты. Реализация №1: `openai_compat` (llama-server, ollama, LM Studio, vllm, OpenRouter — один код, толерантный serde: `#[serde(default)]` везде, диалекты через опциональные поля `cache_prompt`, `id_slot`).
4. **Prompt-cache-friendly по построению**: история append-only, детерминированная сериализация (стабильный порядок ключей), ничего динамичного до стабильного префикса, тулы в фиксированном порядке. Для llama-server — пин слота на сессию.
5. **Надёжные tool calls на слабых моделях — слоями**: (1) нативный шаблон модели на сервере (`--jinja`), (2) валидация аргументов `jsonschema` против схемы из `schemars` → при ошибке вернуть текст ошибки как tool result (retry-with-error), (3) tolerant JSON-repair как fallback, (4) опционально grammar-constrained args. Constrain аргументы, не сам выбор «звать/не звать» (constraint tax).
6. **Edit-формат — стратегия per-model**: exact search/replace по умолчанию, whole-file для слабых локалок; выбирается по model id, override флагом (уроки aider).
7. **Сессии = append-only JSONL** (`~/.mark2/sessions/…`), одна строка = timestamp + item. Resume/fork/эвалы падают из формата бесплатно (урок codex rollout / mini-swe-agent).
8. **Верификация — сенсоры, не вера**: после edit/write — конфигурируемая команда диагностики (`cargo check`, `tsc`, ruff…), вывод скармливается модели (урок opencode/Böckeler). В MVP — заглушка-точка расширения, включается в фазе 3.
9. **Отмена — один примитив**: `tokio_util::sync::CancellationToken`, все await через helper `.or_cancel()`.
10. **Компакция — сразу в MVP** (по требованию): порог 0.8 контекстного лимита, суммаризация через того же провайдера, старые сообщения не удаляются, а помечаются флагами видимости (`agent_visible`/`user_visible` — урок goose); наружу — событие `Compacted`. На локалках с малым контекстом это нужно с первого дня.
11. **Чего сознательно НЕТ в MVP**: MCP, субагенты, песочница, TUI, plan-tool, память. Каждому — своя фаза, чтобы ядро не обросло до goose/codex-годфайлов.

## Структура workspace

```
mark2/
├── Cargo.toml                  # workspace, workspace.dependencies
├── AGENTS.md
├── docs/plans/
├── crates/
│   ├── core/                   # mark2-core: типы, loop, tools, session
│   │   └── src/
│   │       ├── lib.rs
│   │       ├── message.rs      # Message, ContentBlock, Role, Usage
│   │       ├── event.rs        # Event (serde-tagged) + Op
│   │       ├── provider.rs     # trait Provider, ProviderRequest, MessageStream
│   │       ├── agent.rs        # Agent: run_turn loop
│   │       ├── session.rs      # JSONL recorder / loader
│   │       ├── permission.rs   # PermissionPolicy: Yolo | AskWrites | ReadOnly
│   │       ├── prompt.rs       # системный промпт (≤1000 токенов) + AGENTS.md
│   │       ├── config.rs       # TOML: ~/.config/mark2 + ./.mark2.toml + флаги
│   │       └── tools/
│   │           ├── mod.rs      # trait Tool (schemars-схема), ToolRegistry, валидация
│   │           ├── bash.rs
│   │           ├── read.rs
│   │           ├── write.rs
│   │           └── edit.rs     # EditStrategy: SearchReplace | WholeFile
│   ├── providers/              # mark2-providers
│   │   └── src/
│   │       ├── lib.rs
│   │       ├── openai_compat.rs  # wire-типы, to_wire/parse_chunk, стрим тул-аргументов
│   │       ├── sse.rs            # reqwest bytes_stream + eventsource-stream
│   │       └── json_repair.rs
│   └── cli/                    # бинарь mark2
│       └── src/main.rs         # clap; интерактив + `-p` print mode; рендер Event-стрима
```

Зависимости (закреплено ресёрчем): tokio, tokio-util, reqwest 0.13, eventsource-stream, serde/serde_json (`preserve_order`), schemars 1.x, jsonschema, clap, thiserror/anyhow, tracing (+ tracing-subscriber), dirs, toml. Позже: rmcp 3.x, agent-client-protocol 2.x, landlock, seccompiler, ratatui 0.30.

## Implementation Steps — Фаза 1 (MVP: работающий агент)

### Task 1: Каркас workspace
**Files:** Create: `Cargo.toml`, `crates/{core,providers,cli}/Cargo.toml`, `src/lib.rs|main.rs` заглушки, `.gitignore`, `AGENTS.md`
- [x] workspace + workspace.dependencies, `cargo build` зелёный
- [x] git init, first commit; CI-заготовка не нужна (соло-проект), но `cargo fmt --check && cargo clippy -- -D warnings` зафиксировать как команду проверки в AGENTS.md

### Task 2: Нейтральные типы сообщений и событий
**Files:** Create: `core/src/message.rs`, `core/src/event.rs`
- [x] `Message { role, content: Vec<ContentBlock>, metadata: MessageMetadata{agent_visible,user_visible} }` (флаги — фундамент компакции из Task 7, дефолт не сериализуется); `ContentBlock::{Text, Thinking, ToolUse{id,name,input}, ToolResult{tool_use_id,content,is_error}}`; `Usage` — отдельный тип (input/output/cache_read/cache_write)
- [x] `Event` enum (serde-tagged): `TurnStarted, TextDelta, ThinkingDelta, ToolCallStarted, ToolCallOutput, ToolCallFinished, ApprovalRequired, MessageComplete, Usage, Compacted, TurnFinished{reason: StopReason}, Error`
- [x] `Op` enum: `UserInput, Interrupt, Approve{id, decision: ApprovalDecision}, Compact`
- [x] тесты: round-trip serde сообщений и событий, стабильность порядка ключей

### Task 3: Provider trait + OpenAI-compatible провайдер
**Files:** Create: `core/src/provider.rs`, `providers/src/{openai_compat,sse,json_repair}.rs`
- [ ] `trait Provider { fn name(&self) -> &str; async fn stream(&self, req: ProviderRequest) -> Result<MessageStream> }`; `MessageStream = BoxStream<Result<StreamChunk>>`; `StreamChunk::{TextDelta, ThinkingDelta, ToolCallDelta, Complete{message, usage}}`
- [ ] wire-типы Chat Completions с `#[serde(default)]`-толерантностью; `to_wire(neutral) -> request`, инкрементальная сборка tool-call аргументов из дельт
- [ ] SSE через `resp.bytes_stream().eventsource()`; ретраи с бэкоффом на сетевые/5xx (не мид-стрим)
- [ ] поля диалектов: `cache_prompt`, `id_slot` (llama-server) за опцией провайдера
- [ ] `json_repair`: tolerant-парс аргументов после провала строгого
- [ ] тесты: фикстуры реальных SSE-чанков (llama-server, ollama, OpenRouter) → parse; сборка стриминговых tool-аргументов; repair-кейсы (обрезанный JSON, trailing comma)

### Task 4: Тулы
**Files:** Create: `core/src/tools/{mod,bash,read,write,edit}.rs`
- [ ] `trait Tool { name, description, schema (schemars), async execute(input, ctx) -> ToolOutput }`; `ToolRegistry` с валидацией входа через `jsonschema` → ошибка валидации возвращается модели как tool result
- [ ] `bash`: timeout, обрезка вывода по лимиту байт/строк, cwd
- [ ] `read`: лимиты строк/offset; `write`: создание директорий
- [ ] `edit`: exact search/replace (уникальность вхождения, понятные ошибки «not found / not unique»); `EditStrategy::WholeFile` как альтернативная схема тула для слабых моделей (выбор по model id + override в конфиге)
- [ ] тесты: happy-path и ошибки каждого тула; валидация схемы отклоняет мусорные аргументы

### Task 5: Agent loop
**Files:** Create: `core/src/agent.rs`, `core/src/permission.rs`, `core/src/prompt.rs`
- [ ] цикл: system prompt (+ AGENTS.md проекта, nearest-wins) → `provider.stream()` → эмит Event-дельт → на tool_use: permission-гейт → выполнить тулы (последовательно в MVP; интерфейс не мешает FuturesOrdered позже) → tool results в историю → повторять, пока есть tool calls; `max_turns` предохранитель
- [ ] история append-only в нейтральном формате; ничего не переписывать (кэш)
- [ ] `PermissionPolicy`: `Yolo` / `AskWrites` (write, edit, bash требуют аппрува через событие `ApprovalRequired` + `Op::Approve`) / `ReadOnly`
- [ ] `CancellationToken` через все await (helper `or_cancel`); `Op::Interrupt` корректно завершает ход событием `TurnFinished{Cancelled}`
- [ ] системный промпт ≤ 1000 токенов вместе с описаниями тулов
- [ ] тесты: MockProvider со скриптованными ответами — полный ход с 2 тул-коллами; interrupt посреди стрима; policy блокирует write; retry-with-error при невалидных аргументах

### Task 6: Сессии (запись)
**Files:** Create: `core/src/session.rs`
- [ ] JSONL: `~/.mark2/sessions/YYYY/MM/DD/<ts>-<id>.jsonl`; строка = `{ts, item}`, item = meta | message | event-маркеры ходов
- [ ] асинхронный writer (канал), flush на завершении хода
- [ ] тесты: запись→чтение round-trip; повреждённая строка не валит загрузку

### Task 7: Компакция контекста
**Files:** Create: `core/src/compact.rs`; Modify: `core/src/{agent,message,session}.rs`
- [ ] оценка занятости контекста: usage из последнего ответа провайдера + фолбэк-эстимейт по байтам; `context_limit` из конфига модели
- [ ] pre-turn проверка порога (0.8, конфигурируемо) → суммаризация истории через провайдера (промпт-шаблон: что делали, какие файлы, что дальше)
- [ ] не удалять: старым сообщениям `agent_visible=false`, саммари — `user_visible=false`; сборка промпта фильтрует по `agent_visible`; сессия хранит всё
- [ ] retry-на-`ContextLengthExceeded`: компакция + один повтор запроса
- [ ] событие `Compacted{tokens_before, tokens_after}` наружу; ручной `Op::Compact`
- [ ] тесты: MockProvider — триггер по порогу, фильтрация видимости, retry-путь, round-trip сессии с метаданными видимости

### Task 8: CLI
**Files:** Create: `cli/src/main.rs`, `core/src/config.rs`
- [ ] `mark2 "задача"` — интерактив: стрим текста, спиннеры тулов, y/n аппрувы; `mark2 -p "задача"` — print mode (без вопросов, policy из конфига); `--model`, `--base-url`, `--api-key-env`, `--yolo`
- [ ] конфиг TOML: `~/.config/mark2/config.toml` → `./.mark2.toml` → флаги CLI (по возрастанию приоритета); секция `[provider]`, `[model."<id>"]` (edit_strategy, context_limit)
- [ ] рендер Event-стрима в ANSI-текст (отдельный модуль — это «первый фронтенд», не логика)
- [ ] тесты: мёржинг слоёв конфига; smoke-тест `-p` с MockProvider (feature-гейт)

### Task 9: E2E против живой локалки + приёмка фазы
- [ ] ignored-тест/скрипт: llama-server или ollama с Qwen3-Coder → «создай файл X с содержимым Y» в tmpdir → файл существует
- [ ] прогнать 3–5 реальных мини-задач (создать файл, поправить функцию, найти баг в маленьком репо), зафиксировать проблемы в docs/
- [ ] `cargo fmt --check && cargo clippy -- -D warnings && cargo test` зелёные
- [ ] обновить AGENTS.md (команды сборки/тестов), план перенести в `docs/plans/20260824-rust-agent-harness.md` и отметить прогресс

## Дорожная карта после MVP (отдельные планы на каждую фазу)

- **Фаза 2 — надёжность на локалках**: usage/стоимость в Event, стабильность префикса под KV-кэш (замер cache hit по `timings` llama-server), grammar-constrained args (опция), toolshim-режим для моделей совсем без tool calling (урок goose), бенч-скрипт из 10 задач для сравнения моделей/форматов.
- **Фаза 3 — контур верификации**: post-edit сенсор (конфигурируемая команда диагностики, вывод → модели), pre/post-tool хуки (shell), resume/`--continue`, steering-очередь (ввод мид-тёрна), инкрементальная суммаризация старых tool-пар (развитие компакции из MVP).
- **Фаза 4 — экосистема**: MCP-клиент (rmcp 3.x, stdio + streamable HTTP), Anthropic-провайдер (ручные serde-типы, `cache_control`-брейкпоинты), ACP-адаптер (agent-client-protocol v2) → Zed/JetBrains/neovim как фронтенды бесплатно.
- **Фаза 5 — изоляция и масштаб**: песочница (macOS seatbelt SBPL-генерация, Linux landlock+seccompiler; сеть — главный рубильник), субагенты, TUI (ratatui 0.30) — если после ACP ещё нужен.

## Исполнение

- **Код пишут субагенты на модели Opus** (Agent tool с `model: opus`), по одному таску за раз; основная сессия ставит задачу, ревьюит дифф и гоняет тесты перед переходом к следующему таску.
- Тесты — regular: код, затем тесты в рамках того же таска; тесты обязательны в каждом таске.

## Verification

- Каждая задача: `cargo test -p <crate>` + clippy зелёные до перехода к следующей.
- Финал фазы 1: e2e из Task 8 на реальной локальной модели (llama-server `--jinja` + Qwen3-Coder-30B-A3B или gpt-oss-20b) и, при наличии ключа, на OpenRouter — один и тот же код провайдера.
- Критерий MVP: агент решает «создай/поправь файл по описанию» на локальной модели, стримит вывод, спрашивает аппрувы, пишет сессию в JSONL, корректно прерывается по Ctrl+C.
