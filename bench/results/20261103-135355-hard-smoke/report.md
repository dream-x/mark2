# mark2 bench — 20261103-135355-hard-smoke

- прогон: 2026-11-03T10:53:55.104132Z → 2026-11-03T11:00:35.96699Z
- mark2: `/Users/maks/projects/mark2/target/release/mark2`
- задачи: `/Users/maks/projects/mark2/bench/tasks`
- модели: `/Users/maks/projects/mark2/bench/models.toml`
- повторов: 1 (--repeats)

## Модель × задача

В ячейке: пройдено/прогонов, медиана времени.

| модель | T09 needle-edit | T10 yaml-surgical | T11 cascade-bugs | T12 decoy-answer | T13 moved-config | T14 csv-report |
|---|---|---|---|---|---|---|
| bolt4 | 0/1 · 240.0s | 1/1 · 60.8s | 0/1 · 32.6s | 1/1 · 22.1s | 1/1 · 16.7s | 1/1 · 23.6s |

## Сводка

| модель | pass rate | медиана времени | медиана токенов in/out | tool error rate | таймауты |
|---|---|---|---|---|---|
| bolt4 | 4/6 (67%) | 28.1s | 9646 / 1973 | 29% (10/35) | 1 |

Всего: 4/6 (67%).

## Аномалии

- [bolt4] T09 повтор 1: таймаут mark2 (240s)
