# mark2 bench — 20261105-120938-ctx-v1-local

- прогон: 2026-11-05T09:09:38.584103Z → 2026-11-05T10:19:54.825691Z
- mark2: `/Users/maks/projects/mark2/target/release/mark2`
- задачи: `/Users/maks/projects/mark2/bench/tasks`
- модели: `/Users/maks/projects/mark2/bench/models.toml`
- повторов: 3 (--repeats)

## Модель × задача

В ячейке: пройдено/прогонов, медиана времени.

| модель | T22 ctx-repo-needle | T23 ctx-scatter-synthesis |
|---|---|---|
| qwen9 | 3/3 · 26.6s | 0/3 · 124.9s |
| flint40 | 3/3 · 42.0s | 2/3 · 636.1s |
| qwen36c | 3/3 · 34.3s | 0/3 · 25.9s |
| ornith35 | 3/3 · 17.6s | 0/3 · 244.6s |

## Сводка

| модель | pass rate | медиана времени | медиана токенов in/out | tool error rate | таймауты | компакции |
|---|---|---|---|---|---|---|
| qwen9 | 3/6 (50%) | 71.7s | 28390 / 1081 | 14% (7/49) | 0 | 4 |
| flint40 | 5/6 (83%) | 194.5s | 42186 / 2918 | 9% (8/93) | 1 | 12 |
| qwen36c | 3/6 (50%) | 33.6s | 42410 / 2055 | 22% (12/55) | 0 | 1 |
| ornith35 | 3/6 (50%) | 44.3s | 28671 / 1095 | 6% (4/72) | 1 | 9 |

Всего: 14/24 (58%).

## Обвязка

Срабатывания контролей mark2 по маркерам stderr, сумма по прогонам модели: нуджи `[agent] write_churn` и `[agent] loop_detection`, непрошедшие post-edit проверки (`[agent] post_edit`), тул-коллы, спасённые приведением аргументов (`· coerced N`), компакции (`[agent] compaction`), тул-коллы с ошибкой (`✗ <тул>:`).

| модель | write_churn | loop_detection | post_edit | coerced | компакции | tool errors |
|---|---|---|---|---|---|---|
| qwen9 | 0 | 0 | 0 | 0 | 4 | 7 |
| flint40 | 0 | 0 | 0 | 0 | 12 | 8 |
| qwen36c | 0 | 0 | 0 | 0 | 1 | 12 |
| ornith35 | 0 | 0 | 0 | 0 | 9 | 4 |

## Аномалии

- [flint40] T23 повтор 3: таймаут mark2 (960s)
- [ornith35] T23 повтор 1: таймаут mark2 (960s)
