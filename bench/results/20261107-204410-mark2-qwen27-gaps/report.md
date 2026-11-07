# mark2 bench — 20261107-204410-mark2-qwen27-gaps

- прогон: 2026-11-07T17:44:10.588423Z → 2026-11-07T17:46:08.465742Z
- mark2: `/Users/maks/projects/mark2/target/release/mark2`
- задачи: `/Users/maks/projects/mark2/bench/tasks`
- модели: `/Users/maks/projects/mark2/bench/models.toml`
- повторов: 3 (--repeats)

## Модель × задача

В ячейке: пройдено/прогонов, медиана времени.

| модель | T18 memory-recall | T19 memory-store | T20 review-bug | T21 review-clean |
|---|---|---|---|---|
| qwen27 | 3/3 · 6.2s | 3/3 · 6.6s | 3/3 · 16.6s | 3/3 · 8.1s |

## Сводка

| модель | pass rate | медиана времени | медиана токенов in/out | tool error rate | таймауты | компакции |
|---|---|---|---|---|---|---|
| qwen27 | 12/12 (100%) | 7.7s | 6455 / 241 | 0% (0/30) | 0 | 0 |

Всего: 12/12 (100%).

## Обвязка

Срабатывания контролей mark2 по маркерам stderr, сумма по прогонам модели: нуджи `[agent] write_churn` и `[agent] loop_detection`, непрошедшие post-edit проверки (`[agent] post_edit`), тул-коллы, спасённые приведением аргументов (`· coerced N`), компакции (`[agent] compaction`), тул-коллы с ошибкой (`✗ <тул>:`).

| модель | write_churn | loop_detection | post_edit | coerced | компакции | tool errors |
|---|---|---|---|---|---|---|
| qwen27 | 0 | 0 | 0 | 0 | 0 | 0 |

## Аномалии

Нет.
