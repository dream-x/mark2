# mark2

Минимальный agent harness на Rust для **локальных моделей** — llama.cpp (`llama-server`), ollama, LM Studio и любого OpenAI-совместимого API.

Ядро-библиотека + headless CLI. Модель стохастическая — обвязка детерминированная: цикл, тулы со строгой валидацией, permission-гейт, компакция контекста, append-only сессии. Философия — «LLM, цикл и достаточно токенов»: 4 тула, системный промпт вместе со схемами тулов ≈ 1000 токенов, ничего лишнего.

## Статус

**Фаза 1 (MVP) завершена.** Агент решает реальные мини-задачи на локальных моделях (проверено на qwen3.5:9b, gemma4, qwen3.8:27b по сети): стримит вывод, вызывает тулы, чинит собственные ошибки аргументов, пишет сессию, корректно прерывается. 260+ юнит-тестов, живые e2e-тесты. Известные ограничения — в `docs/notes/e2e-findings-20260824.md`.

## Быстрый старт

```sh
cargo build --release

# против ollama
./target/release/mark2 -p --yolo \
  --base-url http://localhost:11434/v1 \
  --model qwen3.5:9b \
  "создай файл hello.txt с содержимым: привет"

# против llama-server (запускать его с --jinja для нативных tool calls)
llama-server --jinja -m model.gguf --port 8081 &
./target/release/mark2 --base-url http://localhost:8081/v1 --model local "поправь баг в main.py"
```

Без `-p` — интерактивный режим: стрим текста, статус тул-коллов (`→ write {...}` / `✓`), подтверждения `y/N` для мутирующих тулов, Ctrl+C прерывает ход. С `-p` — только ответ модели в stdout (exit 0 — успех, 1 — ошибка, 130 — прервано).

## Конфиг

Слои по возрастанию приоритета: `~/.config/mark2/config.toml` → ближайший `./.mark2.toml` (поиск от cwd вверх) → флаги CLI.

```toml
[provider]
base_url = "http://localhost:11434/v1"
# api_key_env = "OPENROUTER_API_KEY"
# cache_prompt = true      # диалект llama-server: KV-кэш префикса
# slot_id = 0              # пин слота llama-server на сессию

[agent]
model = "qwen3.5:9b"
policy = "ask_writes"      # yolo | ask_writes | read_only
# max_turns = 50

[models."qwen3.5"]         # матчится по подстроке id модели
context_limit = 32768      # включает проактивную компакцию (порог 0.8)
# edit_strategy = "whole_file"  # для слабых моделей вместо search_replace
```

## Тулы

`bash` (timeout, обрезка вывода, kill process group), `read`, `write`, `edit` (exact search/replace; для слабых моделей — режим whole-file). Аргументы валидируются JSON Schema; невалидные — возвращаются модели текстом ошибки (retry-with-error), обрезанный JSON проходит через repair.

## Сессии

Append-only JSONL в `~/.mark2/sessions/YYYY/MM/DD/*.jsonl`: meta, все сообщения (включая скрытые компакцией — с флагами видимости), маркеры ходов, компакции. Формат сессии = формат для эвалов и бенчмарка.

## Документация

- [ARCHITECTURE.md](ARCHITECTURE.md) — устройство: крейты, поток данных, ключевые контракты, инварианты
- [CONTRIBUTING.md](CONTRIBUTING.md) — как собирать, проверять и что нельзя ломать
- [bench/STRATEGY.md](bench/STRATEGY.md) — бенчмарк моделей на агентных задачах
- [AGENTS.md](AGENTS.md) — инструкции для AI-агентов, работающих с этим репо
- `docs/plans/` — планы фаз (выполненные — в `completed/`)

## Роадмап

- **Фаза 2 — надёжность на локалках**: финальный ответ из reasoning-канала, замер KV-кэша, grammar-constrained args, toolshim для моделей без tool calling, бенчмарк
- **Фаза 3 — контур верификации**: post-edit сенсор (диагностика после правок — модели), хуки, resume, steering, инкрементальная суммаризация
- **Фаза 4 — экосистема**: MCP-клиент, Anthropic-провайдер, ACP-адаптер (Zed/JetBrains/neovim как фронтенды)
- **Фаза 5 — изоляция**: песочница (Seatbelt/Landlock), субагенты, TUI
