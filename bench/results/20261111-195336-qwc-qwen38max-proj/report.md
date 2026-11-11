# mark2 bench — 20261111-195336-qwc-qwen38max-proj

- прогон: 2026-11-11T16:53:36.306992Z → 2026-11-11T18:06:20.61011Z
- mark2: `/Users/maks/projects/mark2/bench/shims/mark2-qwc-shim.sh`
- задачи: `/Users/maks/projects/mark2/bench/tasks`
- модели: `/Users/maks/projects/mark2/bench/models.toml`
- повторов: 2 (--repeats)

## Модель × задача

В ячейке: пройдено/прогонов, медиана времени.

| модель | T17 ws-chat | T24 app-freeform | T25 app-planned | T26 chat-freeform | T27 chat-planned |
|---|---|---|---|---|---|
| qwen38max | 2/2 · 287.3s | 2/2 · 189.3s | 2/2 · 644.5s | 2/2 · 344.7s | 2/2 · 686.2s |

## Сводка

| модель | pass rate | медиана времени | медиана токенов in/out | tool error rate | таймауты | компакции |
|---|---|---|---|---|---|---|
| qwen38max | 10/10 (100%) | 338.2s | — | — | 0 | 0 |

Всего: 10/10 (100%).

## Обвязка

Срабатывания контролей mark2 по маркерам stderr, сумма по прогонам модели: нуджи `[agent] write_churn` и `[agent] loop_detection`, непрошедшие post-edit проверки (`[agent] post_edit`), тул-коллы, спасённые приведением аргументов (`· coerced N`), компакции (`[agent] compaction`), тул-коллы с ошибкой (`✗ <тул>:`).

| модель | write_churn | loop_detection | post_edit | coerced | компакции | tool errors |
|---|---|---|---|---|---|---|
| qwen38max | 0 | 0 | 0 | — | 0 | 0 |

## Аномалии

- [qwen38max] T17 повтор 1: сессия не записана
- [qwen38max] T17 повтор 2: сессия не записана
- [qwen38max] T24 повтор 1: сессия не записана
- [qwen38max] T24 повтор 2: сессия не записана
- [qwen38max] T25 повтор 1: сессия не записана
- [qwen38max] T25 повтор 2: сессия не записана
- [qwen38max] T26 повтор 1: сессия не записана
- [qwen38max] T26 повтор 2: сессия не записана
- [qwen38max] T27 повтор 1: сессия не записана
- [qwen38max] T27 повтор 2: сессия не записана
