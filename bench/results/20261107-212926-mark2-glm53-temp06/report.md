# mark2 bench — 20261107-212926-mark2-glm53-temp06

- прогон: 2026-11-07T18:29:26.455786Z → 2026-11-07T18:48:04.868719Z
- mark2: `/Users/maks/projects/mark2/target/release/mark2`
- задачи: `/Users/maks/projects/mark2/bench/tasks`
- модели: `/Users/maks/projects/mark2/bench/models.toml`
- повторов: 2 (--repeats)
- абляция (`--agent-override` в [agent]): `temperature = 0.6`

## Модель × задача

В ячейке: пройдено/прогонов, медиана времени.

| модель | T24 app-freeform | T27 chat-planned |
|---|---|---|
| glm53 | 2/2 · 233.6s | 1/2 · 309.8s |

## Сводка

| модель | pass rate | медиана времени | медиана токенов in/out | tool error rate | таймауты | компакции |
|---|---|---|---|---|---|---|
| glm53 | 3/4 (75%) | 264.9s | 347264 / 16262 | 4% (5/125) | 0 | 0 |

Всего: 3/4 (75%).

## Обвязка

Срабатывания контролей mark2 по маркерам stderr, сумма по прогонам модели: нуджи `[agent] write_churn` и `[agent] loop_detection`, непрошедшие post-edit проверки (`[agent] post_edit`), тул-коллы, спасённые приведением аргументов (`· coerced N`), компакции (`[agent] compaction`), тул-коллы с ошибкой (`✗ <тул>:`).

| модель | write_churn | loop_detection | post_edit | coerced | компакции | tool errors |
|---|---|---|---|---|---|---|
| glm53 | 14 | 0 | 0 | 0 | 0 | 5 |

## Аномалии

Нет.
