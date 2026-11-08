# mark2 bench — 20261108-014049-mark2-qwen27local-proj

- прогон: 2026-11-07T22:40:49.944515Z → 2026-11-08T02:50:07.442394Z
- mark2: `/Users/maks/projects/mark2/target/release/mark2`
- задачи: `/Users/maks/projects/mark2/bench/tasks`
- модели: `/Users/maks/projects/mark2/bench/models.toml`
- повторов: 2 (--repeats)

## Модель × задача

В ячейке: пройдено/прогонов, медиана времени.

| модель | T17 ws-chat | T24 app-freeform | T25 app-planned | T26 chat-freeform | T27 chat-planned |
|---|---|---|---|---|---|
| qwen27-local | 0/2 · 5400.0s | 2/2 · 420.3s | 2/2 · 242.5s | 2/2 · 864.7s | 1/2 · 494.5s |

## Сводка

| модель | pass rate | медиана времени | медиана токенов in/out | tool error rate | таймауты | компакции |
|---|---|---|---|---|---|---|
| qwen27-local | 7/10 (70%) | 464.5s | 119772 / 10548 | 2% (3/129) | 2 | 0 |

Всего: 7/10 (70%).

## Обвязка

Срабатывания контролей mark2 по маркерам stderr, сумма по прогонам модели: нуджи `[agent] write_churn` и `[agent] loop_detection`, непрошедшие post-edit проверки (`[agent] post_edit`), тул-коллы, спасённые приведением аргументов (`· coerced N`), компакции (`[agent] compaction`), тул-коллы с ошибкой (`✗ <тул>:`).

| модель | write_churn | loop_detection | post_edit | coerced | компакции | tool errors |
|---|---|---|---|---|---|---|
| qwen27-local | 6 | 0 | 0 | 0 | 0 | 6 |

## Аномалии

- [qwen27-local] T17 повтор 1: таймаут mark2 (5400s)
- [qwen27-local] T17 повтор 2: таймаут mark2 (5400s)
