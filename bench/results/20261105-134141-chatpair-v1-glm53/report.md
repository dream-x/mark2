# mark2 bench — 20261105-134141-chatpair-v1-glm53

- прогон: 2026-11-05T10:41:41.545415Z → 2026-11-05T11:03:00.17205Z
- mark2: `/Users/maks/projects/mark2/target/release/mark2`
- задачи: `/Users/maks/projects/mark2/bench/tasks`
- модели: `/Users/maks/projects/mark2/bench/models.toml`
- повторов: 2 (--repeats)

## Модель × задача

В ячейке: пройдено/прогонов, медиана времени.

| модель | T17 ws-chat | T26 chat-freeform | T27 chat-planned |
|---|---|---|---|
| glm53 | 1/2 · 133.6s | 2/2 · 322.4s | 1/2 · 164.0s |

## Сводка

| модель | pass rate | медиана времени | медиана токенов in/out | tool error rate | таймауты | компакции |
|---|---|---|---|---|---|---|
| glm53 | 4/6 (67%) | 184.1s | 369962 / 12551 | 8% (7/89) | 0 | 0 |

Всего: 4/6 (67%).

## Обвязка

Срабатывания контролей mark2 по маркерам stderr, сумма по прогонам модели: нуджи `[agent] write_churn` и `[agent] loop_detection`, непрошедшие post-edit проверки (`[agent] post_edit`), тул-коллы, спасённые приведением аргументов (`· coerced N`), компакции (`[agent] compaction`), тул-коллы с ошибкой (`✗ <тул>:`).

| модель | write_churn | loop_detection | post_edit | coerced | компакции | tool errors |
|---|---|---|---|---|---|---|
| glm53 | 2 | 0 | 0 | 0 | 0 | 7 |

## Аномалии

- [glm53] T17 повтор 1: mark2 вышел с кодом —
- [glm53] T17 повтор 1: пустой ответ в stdout
- [glm53] T26 повтор 1: mark2 вышел с кодом —
- [glm53] T26 повтор 1: пустой ответ в stdout
- [glm53] T26 повтор 2: mark2 вышел с кодом —
- [glm53] T27 повтор 2: mark2 вышел с кодом —
