# mark2 bench — 20261107-120745-dsh-glm53-t16-replay

- прогон: 2026-11-07T09:07:45.793428Z → 2026-11-07T09:08:12.800136Z
- mark2: `/private/tmp/claude-501/-Users-maks-projects-mark2/04d8750f-94be-414a-bc5f-19c44aaec6c6/scratchpad/dsh/mark2-dsh-shim.sh`
- задачи: `/Users/maks/projects/mark2/bench/tasks`
- модели: `/Users/maks/projects/mark2/bench/models.toml`
- повторов: 1 (--repeats)

## Модель × задача

В ячейке: пройдено/прогонов, медиана времени.

| модель | T16 permission-deny |
|---|---|
| glm53 | 1/1 · 20.0s |

## Сводка

| модель | pass rate | медиана времени | медиана токенов in/out | tool error rate | таймауты | компакции |
|---|---|---|---|---|---|---|
| glm53 | 1/1 (100%) | 20.0s | — | — | 0 | 0 |

Всего: 1/1 (100%).

## Обвязка

Срабатывания контролей mark2 по маркерам stderr, сумма по прогонам модели: нуджи `[agent] write_churn` и `[agent] loop_detection`, непрошедшие post-edit проверки (`[agent] post_edit`), тул-коллы, спасённые приведением аргументов (`· coerced N`), компакции (`[agent] compaction`), тул-коллы с ошибкой (`✗ <тул>:`).

| модель | write_churn | loop_detection | post_edit | coerced | компакции | tool errors |
|---|---|---|---|---|---|---|
| glm53 | 0 | 0 | 0 | — | 0 | 0 |

## Аномалии

- [glm53] T16 повтор 1: сессия не записана
