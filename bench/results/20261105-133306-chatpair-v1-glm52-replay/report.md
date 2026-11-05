# mark2 bench — 20261105-133306-chatpair-v1-glm52-replay

- прогон: 2026-11-05T10:33:06.727668Z → 2026-11-05T10:40:32.866232Z
- mark2: `/Users/maks/projects/mark2/target/release/mark2`
- задачи: `/Users/maks/projects/mark2/bench/tasks`
- модели: `/Users/maks/projects/mark2/bench/models.toml`
- повторов: 2 (--repeats)

## Модель × задача

В ячейке: пройдено/прогонов, медиана времени.

| модель | T17 ws-chat | T26 chat-freeform | T27 chat-planned |
|---|---|---|---|
| glm52 | 0/2 · 8.6s | 0/2 · 11.2s | 0/2 · 196.8s |

## Сводка

| модель | pass rate | медиана времени | медиана токенов in/out | tool error rate | таймауты | компакции |
|---|---|---|---|---|---|---|
| glm52 | 0/6 (0%) | 10.6s | — | 6% (1/16) | 0 | 0 |

Всего: 0/6 (0%).

## Обвязка

Срабатывания контролей mark2 по маркерам stderr, сумма по прогонам модели: нуджи `[agent] write_churn` и `[agent] loop_detection`, непрошедшие post-edit проверки (`[agent] post_edit`), тул-коллы, спасённые приведением аргументов (`· coerced N`), компакции (`[agent] compaction`), тул-коллы с ошибкой (`✗ <тул>:`).

| модель | write_churn | loop_detection | post_edit | coerced | компакции | tool errors |
|---|---|---|---|---|---|---|
| glm52 | 1 | 0 | 0 | — | 0 | 2 |

## Аномалии

- [glm52] T17 повтор 1: mark2 вышел с кодом —
- [glm52] T17 повтор 1: пустой ответ в stdout
- [glm52] T17 повтор 2: mark2 вышел с кодом —
- [glm52] T17 повтор 2: пустой ответ в stdout
- [glm52] T26 повтор 1: mark2 вышел с кодом —
- [glm52] T26 повтор 1: пустой ответ в stdout
- [glm52] T26 повтор 2: mark2 вышел с кодом —
- [glm52] T26 повтор 2: пустой ответ в stdout
- [glm52] T27 повтор 1: mark2 вышел с кодом —
- [glm52] T27 повтор 1: пустой ответ в stdout
- [glm52] T27 повтор 2: mark2 вышел с кодом —
