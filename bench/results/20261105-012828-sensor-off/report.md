# mark2 bench — 20261105-012828-sensor-off

- прогон: 2026-11-04T22:28:28.60598Z → 2026-11-04T22:41:48.740494Z
- mark2: `/Users/maks/projects/mark2/target/release/mark2`
- задачи: `/Users/maks/projects/mark2/bench/tasks`
- модели: `/Users/maks/projects/mark2/bench/models.toml`
- повторов: 3 (--repeats)

## Модель × задача

В ячейке: пройдено/прогонов, медиана времени.

| модель | T04 fix-bug-test | T11 cascade-bugs |
|---|---|---|
| qwen9 | 3/3 · 12.1s | 3/3 · 95.7s |
| bolt4 | 2/3 · 19.8s | 2/3 · 86.5s |

## Сводка

| модель | pass rate | медиана времени | медиана токенов in/out | tool error rate | таймауты | компакции |
|---|---|---|---|---|---|---|
| qwen9 | 6/6 (100%) | 43.5s | 23412 / 1327 | 24% (11/45) | 0 | 0 |
| bolt4 | 4/6 (67%) | 53.2s | 20663 / 7314 | 33% (20/61) | 0 | 0 |

Всего: 10/12 (83%).

## Обвязка

Срабатывания контролей mark2 по маркерам stderr, сумма по прогонам модели: нуджи `[agent] write_churn` и `[agent] loop_detection`, непрошедшие post-edit проверки (`[agent] post_edit`), тул-коллы, спасённые приведением аргументов (`· coerced N`), компакции (`[agent] compaction`), тул-коллы с ошибкой (`✗ <тул>:`).

| модель | write_churn | loop_detection | post_edit | coerced | компакции | tool errors |
|---|---|---|---|---|---|---|
| qwen9 | 1 | 0 | 0 | 0 | 0 | 11 |
| bolt4 | 0 | 4 | 0 | 0 | 0 | 20 |

## Аномалии

Нет.
