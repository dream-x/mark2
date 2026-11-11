# mark2 bench — 20261111-210632-mark2-qwen38max-proj

- прогон: 2026-11-11T18:06:32.650846Z → 2026-11-11T22:40:54.667986Z
- mark2: `/Users/maks/projects/mark2/target/release/mark2`
- задачи: `/Users/maks/projects/mark2/bench/tasks`
- модели: `/Users/maks/projects/mark2/bench/models.toml`
- повторов: 2 (--repeats)

## Модель × задача

В ячейке: пройдено/прогонов, медиана времени.

| модель | T17 ws-chat | T24 app-freeform | T25 app-planned | T26 chat-freeform | T27 chat-planned |
|---|---|---|---|---|---|
| qwen38max | 2/2 · 2144.6s | 1/2 · 488.7s | 2/2 · 1368.5s | 2/2 · 1879.5s | 2/2 · 2304.9s |

## Сводка

| модель | pass rate | медиана времени | медиана токенов in/out | tool error rate | таймауты | компакции |
|---|---|---|---|---|---|---|
| qwen38max | 9/10 (90%) | 1800.1s | 1805043 / 54855 | 4% (26/588) | 0 | 0 |

Всего: 9/10 (90%).

## Обвязка

Срабатывания контролей mark2 по маркерам stderr, сумма по прогонам модели: нуджи `[agent] write_churn` и `[agent] loop_detection`, непрошедшие post-edit проверки (`[agent] post_edit`), тул-коллы, спасённые приведением аргументов (`· coerced N`), компакции (`[agent] compaction`), тул-коллы с ошибкой (`✗ <тул>:`).

| модель | write_churn | loop_detection | post_edit | coerced | компакции | tool errors |
|---|---|---|---|---|---|---|
| qwen38max | 19 | 0 | 0 | 221 | 0 | 26 |

## Аномалии

- [qwen38max] T17 повтор 1: mark2 вышел с кодом 1
- [qwen38max] T25 повтор 2: mark2 вышел с кодом 1
- [qwen38max] T27 повтор 1: mark2 вышел с кодом 1
