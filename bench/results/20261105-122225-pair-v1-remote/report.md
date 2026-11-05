# mark2 bench — 20261105-122225-pair-v1-remote

- прогон: 2026-11-05T09:22:25.844537Z → 2026-11-05T09:55:14.057303Z
- mark2: `/Users/maks/projects/mark2/target/release/mark2`
- задачи: `/Users/maks/projects/mark2/bench/tasks`
- модели: `/Users/maks/projects/mark2/bench/models.toml`
- повторов: 2 (--repeats)

## Модель × задача

В ячейке: пройдено/прогонов, медиана времени.

| модель | T24 app-freeform | T25 app-planned |
|---|---|---|
| qwen27 | 2/2 · 42.5s | 2/2 · 208.0s |
| glm52 | 2/2 · 207.1s | 2/2 · 475.4s |

## Сводка

| модель | pass rate | медиана времени | медиана токенов in/out | tool error rate | таймауты | компакции |
|---|---|---|---|---|---|---|
| qwen27 | 4/4 (100%) | 117.1s | 83283 / 7489 | 6% (3/51) | 0 | 0 |
| glm52 | 4/4 (100%) | 211.8s | 139265 / 11221 | 9% (7/81) | 0 | 0 |

Всего: 8/8 (100%).

## Обвязка

Срабатывания контролей mark2 по маркерам stderr, сумма по прогонам модели: нуджи `[agent] write_churn` и `[agent] loop_detection`, непрошедшие post-edit проверки (`[agent] post_edit`), тул-коллы, спасённые приведением аргументов (`· coerced N`), компакции (`[agent] compaction`), тул-коллы с ошибкой (`✗ <тул>:`).

| модель | write_churn | loop_detection | post_edit | coerced | компакции | tool errors |
|---|---|---|---|---|---|---|
| qwen27 | 1 | 0 | 0 | 0 | 0 | 3 |
| glm52 | 3 | 0 | 0 | 0 | 0 | 7 |

## Аномалии

Нет.
