# mark2 bench — 20261105-120939-ctx-v1-remote

- прогон: 2026-11-05T09:09:39.917977Z → 2026-11-05T09:22:25.792337Z
- mark2: `/Users/maks/projects/mark2/target/release/mark2`
- задачи: `/Users/maks/projects/mark2/bench/tasks`
- модели: `/Users/maks/projects/mark2/bench/models.toml`
- повторов: 3 (--repeats)

## Модель × задача

В ячейке: пройдено/прогонов, медиана времени.

| модель | T22 ctx-repo-needle | T23 ctx-scatter-synthesis |
|---|---|---|
| qwen27 | 3/3 · 12.2s | 3/3 · 51.5s |
| glm52 | 3/3 · 52.9s | 3/3 · 139.7s |

## Сводка

| модель | pass rate | медиана времени | медиана токенов in/out | tool error rate | таймауты | компакции |
|---|---|---|---|---|---|---|
| qwen27 | 6/6 (100%) | 24.9s | 28253 / 1028 | 4% (2/46) | 0 | 5 |
| glm52 | 6/6 (100%) | 93.5s | 40839 / 1487 | 10% (6/62) | 0 | 7 |

Всего: 12/12 (100%).

## Обвязка

Срабатывания контролей mark2 по маркерам stderr, сумма по прогонам модели: нуджи `[agent] write_churn` и `[agent] loop_detection`, непрошедшие post-edit проверки (`[agent] post_edit`), тул-коллы, спасённые приведением аргументов (`· coerced N`), компакции (`[agent] compaction`), тул-коллы с ошибкой (`✗ <тул>:`).

| модель | write_churn | loop_detection | post_edit | coerced | компакции | tool errors |
|---|---|---|---|---|---|---|
| qwen27 | 0 | 0 | 0 | 0 | 5 | 2 |
| glm52 | 0 | 0 | 0 | 0 | 7 | 6 |

## Аномалии

Нет.
