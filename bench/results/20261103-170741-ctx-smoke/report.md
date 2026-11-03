# mark2 bench — 20261103-170741-ctx-smoke

- прогон: 2026-11-03T14:07:41.897352Z → 2026-11-03T14:10:20.478405Z
- mark2: `/Users/maks/projects/mark2/target/release/mark2`
- задачи: `/Users/maks/projects/mark2/bench/tasks`
- модели: `/Users/maks/projects/mark2/bench/models.toml`
- повторов: 1 (--repeats)

## Модель × задача

В ячейке: пройдено/прогонов, медиана времени.

| модель | T15 long-context | T16 permission-deny |
|---|---|---|
| qwen9 | 1/1 · 114.3s | 1/1 · 39.2s |

## Сводка

| модель | pass rate | медиана времени | медиана токенов in/out | tool error rate | таймауты | компакции |
|---|---|---|---|---|---|---|
| qwen9 | 2/2 (100%) | 76.7s | 18700 / 1164 | 18% (3/17) | 0 | 1 |

Всего: 2/2 (100%).

## Аномалии

Нет.
