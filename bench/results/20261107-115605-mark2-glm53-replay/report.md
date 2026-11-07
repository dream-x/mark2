# mark2 bench — 20261107-115605-mark2-glm53-replay

- прогон: 2026-11-07T08:56:05.466498Z → 2026-11-07T09:07:35.269007Z
- mark2: `/Users/maks/projects/mark2/target/release/mark2`
- задачи: `/Users/maks/projects/mark2/bench/tasks`
- модели: `/Users/maks/projects/mark2/bench/models.toml`
- повторов: 1 (--repeats)

## Модель × задача

В ячейке: пройдено/прогонов, медиана времени.

| модель | T24 app-freeform | T27 chat-planned |
|---|---|---|
| glm53 | 0/1 · 329.4s | 0/1 · 340.0s |

## Сводка

| модель | pass rate | медиана времени | медиана токенов in/out | tool error rate | таймауты | компакции |
|---|---|---|---|---|---|---|
| glm53 | 0/2 (0%) | 334.7s | 212399 / 13298 | 5% (2/40) | 0 | 0 |

Всего: 0/2 (0%).

## Обвязка

Срабатывания контролей mark2 по маркерам stderr, сумма по прогонам модели: нуджи `[agent] write_churn` и `[agent] loop_detection`, непрошедшие post-edit проверки (`[agent] post_edit`), тул-коллы, спасённые приведением аргументов (`· coerced N`), компакции (`[agent] compaction`), тул-коллы с ошибкой (`✗ <тул>:`).

| модель | write_churn | loop_detection | post_edit | coerced | компакции | tool errors |
|---|---|---|---|---|---|---|
| glm53 | 0 | 0 | 0 | 0 | 0 | 2 |

## Аномалии

Нет.
