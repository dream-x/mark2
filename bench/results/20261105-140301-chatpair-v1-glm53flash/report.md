# mark2 bench — 20261105-140301-chatpair-v1-glm53flash

- прогон: 2026-11-05T11:03:01.812602Z → 2026-11-05T11:46:52.986064Z
- mark2: `/Users/maks/projects/mark2/target/release/mark2`
- задачи: `/Users/maks/projects/mark2/bench/tasks`
- модели: `/Users/maks/projects/mark2/bench/models.toml`
- повторов: 2 (--repeats)

## Модель × задача

В ячейке: пройдено/прогонов, медиана времени.

| модель | T17 ws-chat | T26 chat-freeform | T27 chat-planned |
|---|---|---|---|
| glm53-flash | 1/2 · 433.7s | 1/2 · 445.8s | 0/2 · 408.0s |

## Сводка

| модель | pass rate | медиана времени | медиана токенов in/out | tool error rate | таймауты | компакции |
|---|---|---|---|---|---|---|
| glm53-flash | 2/6 (33%) | 433.7s | 273804 / 19698 | 10% (9/93) | 0 | 0 |

Всего: 2/6 (33%).

## Обвязка

Срабатывания контролей mark2 по маркерам stderr, сумма по прогонам модели: нуджи `[agent] write_churn` и `[agent] loop_detection`, непрошедшие post-edit проверки (`[agent] post_edit`), тул-коллы, спасённые приведением аргументов (`· coerced N`), компакции (`[agent] compaction`), тул-коллы с ошибкой (`✗ <тул>:`).

| модель | write_churn | loop_detection | post_edit | coerced | компакции | tool errors |
|---|---|---|---|---|---|---|
| glm53-flash | 1 | 0 | 0 | 0 | 0 | 9 |

## Аномалии

- [glm53-flash] T17 повтор 2: mark2 вышел с кодом —
- [glm53-flash] T26 повтор 1: mark2 вышел с кодом —
- [glm53-flash] T26 повтор 1: пустой ответ в stdout
- [glm53-flash] T27 повтор 1: mark2 вышел с кодом —
