# mark2 bench — 20261105-001109-review-v2-glm52

- прогон: 2026-11-04T21:11:09.317582Z → 2026-11-04T21:13:30.930127Z
- mark2: `/Users/maks/projects/mark2/target/release/mark2`
- задачи: `/Users/maks/projects/mark2/bench/tasks`
- модели: `/Users/maks/projects/mark2/bench/models.toml`
- повторов: 3 (--repeats)

## Модель × задача

В ячейке: пройдено/прогонов, медиана времени.

| модель | T20 review-bug | T21 review-clean |
|---|---|---|
| glm52 | 3/3 · 38.7s | 3/3 · 9.4s |

## Сводка

| модель | pass rate | медиана времени | медиана токенов in/out | tool error rate | таймауты | компакции |
|---|---|---|---|---|---|---|
| glm52 | 6/6 (100%) | 15.1s | 12018 / 521 | 4% (1/23) | 0 | 0 |

Всего: 6/6 (100%).

## Обвязка

Срабатывания контролей mark2 по маркерам stderr, сумма по прогонам модели: нуджи `[agent] write_churn` и `[agent] loop_detection`, тул-коллы, спасённые приведением аргументов (`· coerced N`), компакции (`[agent] compaction`), тул-коллы с ошибкой (`✗ <тул>:`).

| модель | write_churn | loop_detection | coerced | компакции | tool errors |
|---|---|---|---|---|---|
| glm52 | 0 | 0 | 0 | 0 | 1 |

## Аномалии

Нет.
