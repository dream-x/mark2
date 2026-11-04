# mark2 bench — 20261105-014148-sensor-on

- прогон: 2026-11-04T22:41:48.751086Z → 2026-11-04T22:52:10.477784Z
- mark2: `/Users/maks/projects/mark2/target/release/mark2`
- задачи: `/Users/maks/projects/mark2/bench/tasks`
- модели: `/Users/maks/projects/mark2/bench/models.toml`
- повторов: 3 (--repeats)
- абляция (`--agent-override` в [agent]): `post_edit_command = "python3 -B test.py"`

## Модель × задача

В ячейке: пройдено/прогонов, медиана времени.

| модель | T04 fix-bug-test | T11 cascade-bugs |
|---|---|---|
| qwen9 | 3/3 · 15.0s | 3/3 · 52.3s |
| bolt4 | 3/3 · 53.4s | 1/3 · 40.3s |

## Сводка

| модель | pass rate | медиана времени | медиана токенов in/out | tool error rate | таймауты | компакции |
|---|---|---|---|---|---|---|
| qwen9 | 6/6 (100%) | 31.6s | 17846 / 958 | 9% (3/33) | 0 | 0 |
| bolt4 | 4/6 (67%) | 46.8s | 29596 / 6755 | 25% (15/60) | 0 | 0 |

Всего: 10/12 (83%).

## Обвязка

Срабатывания контролей mark2 по маркерам stderr, сумма по прогонам модели: нуджи `[agent] write_churn` и `[agent] loop_detection`, непрошедшие post-edit проверки (`[agent] post_edit`), тул-коллы, спасённые приведением аргументов (`· coerced N`), компакции (`[agent] compaction`), тул-коллы с ошибкой (`✗ <тул>:`).

| модель | write_churn | loop_detection | post_edit | coerced | компакции | tool errors |
|---|---|---|---|---|---|---|
| qwen9 | 0 | 0 | 6 | 0 | 0 | 3 |
| bolt4 | 0 | 0 | 9 | 0 | 0 | 15 |

## Аномалии

Нет.
