# mark2 bench — 20261112-015707-mark2-qwen38max-proj-verify

- прогон: 2026-11-11T22:57:07.221317Z → 2026-11-12T00:22:42.522557Z
- mark2: `/Users/maks/projects/mark2/target/release/mark2`
- задачи: `/Users/maks/projects/mark2/bench/tasks`
- модели: `/Users/maks/projects/mark2/bench/models.toml`
- повторов: 2 (--repeats)

## Модель × задача

В ячейке: пройдено/прогонов, медиана времени.

| модель | T24 app-freeform | T25 app-planned |
|---|---|---|
| qwen38max | 2/2 · 1045.3s | 2/2 · 1510.2s |

## Сводка

| модель | pass rate | медиана времени | медиана токенов in/out | tool error rate | таймауты | компакции |
|---|---|---|---|---|---|---|
| qwen38max | 4/4 (100%) | 1186.2s | 1444672 / 37433 | 7% (16/230) | 0 | 0 |

Всего: 4/4 (100%).

## Обвязка

Срабатывания контролей mark2 по маркерам stderr, сумма по прогонам модели: нуджи `[agent] write_churn` и `[agent] loop_detection`, непрошедшие post-edit проверки (`[agent] post_edit`), тул-коллы, спасённые приведением аргументов (`· coerced N`), компакции (`[agent] compaction`), тул-коллы с ошибкой (`✗ <тул>:`).

| модель | write_churn | loop_detection | post_edit | coerced | компакции | tool errors |
|---|---|---|---|---|---|---|
| qwen38max | 7 | 0 | 0 | 118 | 0 | 16 |

## Аномалии

- [qwen38max] T25 повтор 2: mark2 вышел с кодом 1
